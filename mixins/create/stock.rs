// Create's stock logistics in the game: stock links joining packagers into logistics networks,
// stock tickers, redstone requesters and factory gauges. A sibling of `create_packages`
// (packagers pack and send what is ordered); the rules are `create_engine::stock`'s. State is
// typed, per dimension, on the server thread and saved with its column as `create_stock`.
// Nothing here ticks: a network's stock is summed when something asks for it (a ticker's dump,
// an order, a requester's pulse, a gauge's look) and kept for Create's cache times (one tick for
// orders, 20 for gauges); gauges are looked at on their interval only.
use super::Game;
use super::create::*;
use super::create_logistics::*;
use super::create_package_body as body;
use super::create_packages as packages;
use create_engine::Block;
use create_engine::kinetics::Dir;
use create_engine::steam::Face;
use create_engine::stock::{
    self as st, GaugeStep, Network, Order, OrderData, Panel, Promises, RequestType, Requester,
    Source, Summary, Trigger,
};
use create_engine::world::dimension_index;
use horizon_client::items::ItemStack;
use horizon_world::nbt::{Compound, Tag};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// A stack as an order or a summary keys it: count 1, compared as Create compares stacks.
#[derive(Clone)]
pub(super) struct Key(pub(super) ItemStack);

impl PartialEq for Key {
    fn eq(&self, o: &Self) -> bool {
        same_stack(&self.0, &o.0)
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.item)
    }
}

impl Key {
    pub(super) fn of(s: &ItemStack) -> Key {
        Key(ItemStack {
            count: 1,
            ..s.clone()
        })
    }
}

fn order_key(o: &Order<ItemStack>) -> Order<Key> {
    let list = |l: &[(ItemStack, u32)]| l.iter().map(|(s, n)| (Key::of(s), *n)).collect();
    Order {
        stacks: list(&o.stacks),
        crafts: o.crafts.iter().map(|(p, n)| (list(p), *n)).collect(),
    }
}

fn order_stack(o: &Order<Key>) -> Order<ItemStack> {
    let list = |l: &[(Key, u32)]| l.iter().map(|(k, n)| (k.0.clone(), *n)).collect();
    Order {
        stacks: list(&o.stacks),
        crafts: o.crafts.iter().map(|(p, n)| (list(p), *n)).collect(),
    }
}

pub(super) fn order_data_key(d: &OrderData<ItemStack>) -> OrderData<Key> {
    OrderData {
        order_id: d.order_id,
        link_index: d.link_index,
        final_link: d.final_link,
        fragment_index: d.fragment_index,
        is_final: d.is_final,
        context: d.context.as_ref().map(order_key),
    }
}

pub(super) fn order_data_stack(d: &OrderData<Key>) -> OrderData<ItemStack> {
    OrderData {
        order_id: d.order_id,
        link_index: d.link_index,
        final_link: d.final_link,
        fragment_index: d.fragment_index,
        is_final: d.is_final,
        context: d.context.as_ref().map(order_stack),
    }
}

/// A stock link: its network (`freqId`, set by `/create_debug network`) and the signal into it
/// (`redstonePower`; at 15 it leaves the network).
#[derive(Clone, Copy, Debug, Default)]
struct Link {
    network: Option<Network>,
    power: i32,
}

#[derive(Clone, Debug, Default)]
struct Ticker {
    network: Option<Network>,
    /// `previouslyUsedAddress`.
    previous: String,
}

#[derive(Clone, Debug, Default)]
struct Req {
    network: Option<Network>,
    r: Requester<Key>,
}

/// A dimension's stock logistics.
#[derive(Default)]
struct Net {
    links: HashMap<[i32; 3], Link>,
    tickers: HashMap<[i32; 3], Ticker>,
    requesters: HashMap<[i32; 3], Req>,
    /// Factory gauges (one panel each; network 0: none set).
    gauges: HashMap<[i32; 3], Panel<Key>>,
    /// Gauges with an item, and the tick each is looked at next; the earliest of them.
    due: HashMap<[i32; 3], u64>,
    next: u64,
    /// `LogisticsNetwork.panelPromises`.
    promises: HashMap<Network, Promises<Key>>,
    /// `LogisticsManager.SUMMARIES` and `ACCURATE_SUMMARIES`: a network's stock and the tick
    /// it was summed.
    recent: HashMap<Network, (u64, Summary<Key>)>,
    accurate: HashMap<Network, (u64, Summary<Key>)>,
    /// Each packager's last stock (`availableItems`), for `submitNewArrivals`.
    last: HashMap<[i32; 3], Summary<Key>>,
    /// Links in columns not loaded, by network (`getUnloadedLinkCount`).
    unloaded: HashMap<Network, Vec<[i32; 3]>>,
    /// Links, requesters and gauges whose neighbours changed.
    neighbours: Vec<[i32; 3]>,
    unsaved: Vec<(i32, i32)>,
}

thread_local! {
    static STOCK: RefCell<Option<Box<[Net; 3]>>> = const { RefCell::new(None) };
    /// `LogisticsManager.r` and friends: order ids and picks.
    static RNG: Cell<u64> = const { Cell::new(0x2545_f491_4f6c_dd1d) };
}

fn with<R>(dim: usize, f: impl FnOnce(&mut Net) -> R) -> R {
    STOCK.with_borrow_mut(|s| f(&mut s.get_or_insert_with(Default::default)[dim]))
}

fn present() -> bool {
    STOCK.with_borrow(Option::is_some)
}

/// The world closed.
pub(super) fn forget() {
    STOCK.with_borrow_mut(|s| *s = None);
}

/// `Random.nextInt()` stand-in.
pub(super) fn next_int(game: &Game<'_>) -> i32 {
    RNG.with(|r| {
        let mut x = r.get() ^ (game.world.game_time as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        x ^= x >> 30;
        x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x ^= x >> 27;
        x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
        x ^= x >> 31;
        r.set(x);
        x as i32
    })
}

pub(super) fn ours(b: Block) -> bool {
    matches!(
        b,
        Block::StockLink | Block::StockTicker | Block::RedstoneRequester | Block::FactoryGauge
    )
}

/// A right click on a stock link or a gauge does nothing here (their screens are commands);
/// true when the block is one (the template's own use is canceled).
pub(super) fn use_block(block: Block, canceled: &mut bool) -> bool {
    if matches!(block, Block::StockLink | Block::FactoryGauge) {
        *canceled = true;
        return true;
    }
    false
}

fn add(p: [i32; 3], o: [i32; 3]) -> [i32; 3] {
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

fn sub(p: [i32; 3], o: [i32; 3]) -> [i32; 3] {
    [p[0] - o[0], p[1] - o[1], p[2] - o[2]]
}

fn block_at(game: &Game<'_>, pos: [i32; 3]) -> Option<Block> {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
}

/// `FaceAttachedHorizontalDirectionalBlock.getConnectedDirection` of a link or gauge: away from
/// what it hangs on.
fn connected(game: &Game<'_>, pos: [i32; 3]) -> Option<(Block, Dir)> {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    with_world(|w| {
        let (b, i) = w.table.get(state)?;
        let d = match i.face {
            Some(Face::Floor) => Dir::Up,
            Some(Face::Ceiling) => Dir::Down,
            _ => i.facing?,
        };
        Some((b, d))
    })
    .flatten()
}

/// `PackagerBlockEntity.getLinkPos`: a stock link hanging on the packager.
pub(super) fn link_of(game: &Game<'_>, pos: [i32; 3]) -> Option<[i32; 3]> {
    Dir::ALL.into_iter().find_map(|d| {
        let at = add(pos, d.offset());
        (connected(game, at) == Some((Block::StockLink, d))).then_some(at)
    })
}

/// `PackagerLinkBlockEntity.getPackager`: the packager the link hangs on (not a re-packager),
/// none at full power.
fn link_packager(game: &Game<'_>, dim: usize, pos: [i32; 3]) -> Option<[i32; 3]> {
    if with(dim, |n| n.links.get(&pos).is_some_and(|l| l.power == 15)) {
        return None;
    }
    let (Block::StockLink, d) = connected(game, pos)? else {
        return None;
    };
    let at = sub(pos, d.offset());
    (block_at(game, at) == Some(Block::Packager)).then_some(at)
}

/// `FactoryPanelBlockEntity.getRestockedPackager`.
fn restocked(game: &Game<'_>, pos: [i32; 3]) -> Option<[i32; 3]> {
    let (Block::FactoryGauge, d) = connected(game, pos)? else {
        return None;
    };
    let at = sub(pos, d.offset());
    (block_at(game, at) == Some(Block::Packager)).then_some(at)
}

/// `recheckIfLinksPresent`: a packager's `linked` follows a link on it.
pub(super) fn recheck_linked(game: &mut Game<'_>, pos: [i32; 3]) {
    if block_at(game, pos) != Some(Block::Packager) {
        return;
    }
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let linked = game
        .pack
        .assets
        .registry
        .state(state)
        .and_then(|s| s.properties.get("linked"))
        .is_some_and(|v| v == "true");
    let should = link_of(game, pos).is_some();
    if linked != should {
        game.native_set_block_property(pos, "linked", if should { "true" } else { "false" });
    }
}

/// One of ours placed or removed (from `create_packages`' changes).
pub(super) fn block_moved(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    was: Option<Block>,
    is: Option<Block>,
) {
    match was {
        Some(Block::StockLink) => with(dim, |n| {
            n.links.remove(&pos);
        }),
        Some(Block::StockTicker) => with(dim, |n| {
            n.tickers.remove(&pos);
        }),
        Some(Block::RedstoneRequester) => with(dim, |n| {
            n.requesters.remove(&pos);
        }),
        // `FactoryPanelBehaviour.disconnectAll`.
        Some(Block::FactoryGauge) => with(dim, |n| {
            n.gauges.remove(&pos);
            n.due.remove(&pos);
            for g in n.gauges.values_mut() {
                g.inputs.retain(|i| i.0 != pos);
                g.outputs.retain(|o| *o != pos);
            }
        }),
        _ => {}
    }
    match is {
        Some(Block::StockLink) => with(dim, |n| {
            n.links.insert(pos, Link::default());
            n.neighbours.push(pos);
        }),
        Some(Block::StockTicker) => with(dim, |n| {
            n.tickers.insert(pos, Ticker::default());
        }),
        // A requester reads its signal at its first neighbour change (its block entity starts
        // unpowered).
        Some(Block::RedstoneRequester) => with(dim, |n| {
            n.requesters.insert(pos, Req::default());
        }),
        Some(Block::FactoryGauge) => with(dim, |n| {
            n.gauges.insert(pos, Panel::new(0));
            n.neighbours.push(pos);
        }),
        _ => {}
    }
    if was == Some(Block::StockLink) || is == Some(Block::StockLink) {
        for d in Dir::ALL {
            recheck_linked(game, add(pos, d.offset()));
        }
    }
    with(dim, |n| n.unsaved.push((pos[0] >> 4, pos[2] >> 4)));
}

/// `getStateForPlacement`: a link on a ceiling turns round (`PackagerLinkBlock`), a gauge on a
/// floor too (`FactoryPanelBlock`); a link reads its power, a requester its signal.
pub(super) fn place_block(game: &mut Game<'_>, pos: [i32; 3], state: u32) {
    let Some((b, info)) = with_world(|w| w.table.get(state).map(|(b, i)| (b, *i))).flatten() else {
        return;
    };
    let turn = match b {
        Block::StockLink => info.face == Some(Face::Ceiling),
        Block::FactoryGauge => info.face == Some(Face::Floor),
        Block::RedstoneRequester => {
            let at = horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]);
            let signal = game.with_level(|b, level, ticks| b.has_neighbor_signal(level, ticks, at));
            if signal != info.powered {
                game.native_set_block_property(
                    pos,
                    "powered",
                    if signal { "true" } else { "false" },
                );
            }
            return;
        }
        _ => return,
    };
    if turn && let Some(f) = info.facing {
        game.native_set_block_property(pos, "facing", dir_name(f.opposite()));
    }
}

/// A neighbour of a link, requester or gauge changed: looked at in the level's tick.
pub(super) fn neighbor_changed(dim: usize, pos: [i32; 3]) {
    with(dim, |n| {
        if !n.neighbours.contains(&pos) {
            n.neighbours.push(pos);
        }
    });
}

/// `PackagerLinkBlock.getPower`: the strongest signal from every side but the one it hangs on.
fn link_power(game: &mut Game<'_>, pos: [i32; 3]) -> i32 {
    let Some((_, c)) = connected(game, pos) else {
        return 0;
    };
    let mut power = 0;
    for d in Dir::ALL {
        if d.opposite() == c {
            continue;
        }
        let o = d.offset();
        let at = horizon_core::position::BlockPos::new(pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]);
        let dir = horizon_core::geometry::Direction::ALL[d as usize];
        power = power.max(game.with_level(|b, level, ticks| b.signal(level, ticks, at, dir)));
    }
    power
}

/// The level's tick: changed neighbours, then the gauges due. Nothing in a dimension without
/// any of ours.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str) {
    if !present() {
        return;
    }
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    for pos in with(dim, |n| std::mem::take(&mut n.neighbours)) {
        match block_at(game, pos) {
            // `PackagerLinkBlock.neighborChanged`: `powered` follows the signal;
            // `redstonePowerChanged`.
            Some(Block::StockLink) => {
                let power = link_power(game, pos);
                let state = game.world.block(pos[0], pos[1], pos[2]).0;
                let was = with_world(|w| w.table.get(state).map(|(_, i)| i.powered)).flatten();
                if was.is_some_and(|w| w != (power > 0)) {
                    game.native_set_block_property_with(
                        pos,
                        "powered",
                        if power > 0 { "true" } else { "false" },
                        super::native_hooks::flags::UPDATE_CLIENTS,
                    );
                }
                with(dim, |n| {
                    if let Some(l) = n.links.get_mut(&pos)
                        && l.power != power
                    {
                        l.power = power;
                        n.unsaved.push((pos[0] >> 4, pos[2] >> 4));
                    }
                });
            }
            // `RedstoneRequesterBlock.neighborChanged`, `onRedstonePowerChanged`.
            Some(Block::RedstoneRequester) => {
                let at = horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]);
                let signal =
                    game.with_level(|b, level, ticks| b.has_neighbor_signal(level, ticks, at));
                let state = game.world.block(pos[0], pos[1], pos[2]).0;
                let was = with_world(|w| w.table.get(state).map(|(_, i)| i.powered)).flatten();
                if was.is_some_and(|w| w != signal) {
                    game.native_set_block_property(
                        pos,
                        "powered",
                        if signal { "true" } else { "false" },
                    );
                }
                let rising = with(dim, |n| {
                    let r = n.requesters.get_mut(&pos)?;
                    let lit = r.r.last_succeeded;
                    let rising = r.r.power(signal);
                    n.unsaved.push((pos[0] >> 4, pos[2] >> 4));
                    Some((rising, lit))
                });
                if let Some((rising, lit)) = rising {
                    if rising {
                        trigger_requester(game, dim, pos);
                    }
                    if with(dim, |n| n.requesters.get(&pos).map(|r| r.r.last_succeeded))
                        != Some(lit)
                    {
                        analog_touched(dim, pos);
                    }
                }
            }
            // `FactoryPanelBlockEntity.lazyTick`: a restocker while a packager is behind it.
            Some(Block::FactoryGauge) => {
                let restocker = game_restocker(game, pos);
                with(dim, |n| {
                    if let Some(g) = n.gauges.get_mut(&pos) {
                        g.restocker = restocker;
                    }
                });
            }
            _ => {}
        }
    }
    let now = game.world.game_time.max(0) as u64;
    if with(dim, |n| !n.due.is_empty() && now >= n.next) {
        let due: Vec<[i32; 3]> = with(dim, |n| {
            let mut v: Vec<[i32; 3]> = n
                .due
                .iter()
                .filter(|(_, t)| **t <= now)
                .map(|(p, _)| *p)
                .collect();
            v.sort_unstable();
            v
        });
        for pos in due {
            with(dim, |n| {
                n.due.insert(pos, now + u64::from(st::GAUGE_INTERVAL))
            });
            look_gauge(game, dim, pos, now);
        }
        with(dim, |n| {
            n.next = n.due.values().copied().min().unwrap_or(u64::MAX)
        });
    }
    let unsaved = with(dim, |n| {
        let mut u = std::mem::take(&mut n.unsaved);
        u.sort_unstable();
        u.dedup();
        u
    });
    for (cx, cz) in unsaved {
        game.native_mark_chunk_unsaved(dimension, [cx, cz]);
    }
}

fn game_restocker(game: &Game<'_>, pos: [i32; 3]) -> bool {
    restocked(game, pos).is_some()
}

/// A packager's stock (`getAvailableItems`), and what arrived since it was last summed handed
/// to the promises waiting for it (`submitNewArrivals`: restocking gauges on the packager and
/// its link's network).
fn packager_stock(
    game: &Game<'_>,
    dim: usize,
    packager: [i32; 3],
) -> (Summary<Key>, Option<[i32; 3]>) {
    let (s, id) = packages::available(game, dim, packager);
    let before = with(dim, |n| n.last.insert(packager, s.clone()));
    let Some(before) = before else {
        return (s, id);
    };
    let arrived = st::arrivals(&before, &s);
    if arrived.is_empty() {
        return (s, id);
    }
    for d in Dir::ALL {
        let at = add(packager, d.offset());
        match connected(game, at) {
            Some((Block::FactoryGauge, c)) if c == d => with(dim, |n| {
                if let Some(g) = n.gauges.get_mut(&at)
                    && g.restocker
                    && g.filter.is_some()
                {
                    for (k, a) in &arrived {
                        g.restocker_promises.arrived(k, *a);
                    }
                }
            }),
            Some((Block::StockLink, c)) if c == d => with(dim, |n| {
                let Some(net) = n.links.get(&at).and_then(|l| l.network) else {
                    return;
                };
                if let Some(p) = n.promises.get_mut(&net) {
                    for (k, a) in &arrived {
                        p.arrived(k, *a);
                    }
                }
            }),
            _ => {}
        }
    }
    (s, id)
}

/// `triggerStockCheck`.
pub(super) fn stock_check(game: &Game<'_>, dim: usize, packager: [i32; 3]) {
    if present() {
        packager_stock(game, dim, packager);
    }
}

/// The links of a network (`getAllPresent`): loaded, below full power, by position.
fn links_of(dim: usize, net: Network) -> Vec<([i32; 3], Link)> {
    with(dim, |n| {
        let mut v: Vec<([i32; 3], Link)> = n
            .links
            .iter()
            .filter(|(_, l)| l.network == Some(net) && l.power < 15)
            .map(|(p, l)| (*p, *l))
            .collect();
        v.sort_unstable_by_key(|x| x.0);
        v
    })
}

/// `LogisticsManager.getSummaryOfNetwork`: each inventory once, from Create's caches (`accurate`:
/// summed again each tick; else every 20).
fn network_stock(game: &Game<'_>, dim: usize, net: Network, accurate: bool) -> Summary<Key> {
    let now = game.world.game_time.max(0) as u64;
    let cached = with(dim, |n| {
        let (map, ticks) = if accurate {
            (&n.accurate, st::ACCURATE_TICKS)
        } else {
            (&n.recent, st::SUMMARY_TICKS)
        };
        map.get(&net)
            .filter(|(at, _)| now.saturating_sub(*at) < ticks)
            .map(|(_, s)| s.clone())
    });
    if let Some(s) = cached {
        return s;
    }
    let mut out = Summary::default();
    let mut seen: Vec<[i32; 3]> = Vec::new();
    for (pos, _) in links_of(dim, net) {
        let Some(packager) = link_packager(game, dim, pos) else {
            continue;
        };
        let (s, id) = packager_stock(game, dim, packager);
        if let Some(id) = id {
            if seen.contains(&id) {
                continue;
            }
            seen.push(id);
        }
        out.contributing_links += 1;
        out.add_all(&s);
    }
    with(dim, |n| {
        let map = if accurate {
            &mut n.accurate
        } else {
            &mut n.recent
        };
        map.insert(net, (now, out.clone()));
    });
    out
}

/// The network's links as an order sees them, by priority (`getAllPresent(freq, true)`: least
/// power first); a link whose packager reaches `ignored` gives nothing.
fn sources(
    game: &Game<'_>,
    dim: usize,
    net: Network,
    ignored: Option<[i32; 3]>,
) -> Vec<Source<Key>> {
    let mut links = links_of(dim, net);
    links.sort_by_key(|l| l.1.power);
    links
        .into_iter()
        .map(|(pos, _)| {
            let packager = link_packager(game, dim, pos);
            let (stock, inventory) = match packager {
                Some(p) => packager_stock(game, dim, p),
                None => (Summary::default(), None),
            };
            let same = ignored.is_some() && inventory == ignored;
            Source {
                packager: packager.filter(|_| !same),
                inventory,
                stock: if same { Summary::default() } else { stock },
            }
        })
        .collect()
}

/// `LogisticsManager.broadcastPackageRequest`: the order split over the network's packagers
/// (none if any of them is too busy), each packing its part.
fn broadcast(
    game: &mut Game<'_>,
    dim: usize,
    net: Network,
    kind: RequestType,
    order: &Order<Key>,
    ignored: Option<[i32; 3]>,
    address: &str,
) -> bool {
    if order.is_empty() {
        return false;
    }
    let srcs = sources(game, dim, net, ignored);
    let order_id = next_int(game);
    let requests = st::find_packagers(&srcs, order, address, order_id, |n| {
        (next_int(game) as u32 % n.max(1) as u32) as usize
    });
    let grouped = st::by_packager(requests);
    if grouped
        .iter()
        .any(|(p, _)| packages::too_busy(dim, *p, kind))
    {
        return false;
    }
    perform(game, dim, grouped);
    true
}

/// `performPackageRequests`: each packager packs its requests; what went into boxes leaves the
/// network's accurate stock (`deductFromAccurateSummary`); the packagers' stock is looked at
/// again (`triggerStockCheck`).
fn perform(
    game: &mut Game<'_>,
    dim: usize,
    grouped: Vec<([i32; 3], std::collections::VecDeque<st::Request<Key>>)>,
) {
    for (p, mut queue) in grouped {
        let boxed = packages::perform_requests(game, dim, p, &mut queue);
        let net =
            link_of(game, p).and_then(|l| with(dim, |n| n.links.get(&l).and_then(|l| l.network)));
        if let Some(net) = net {
            with(dim, |n| {
                if let Some((_, s)) = n.accurate.get_mut(&net) {
                    for b in &boxed {
                        let k = Key::of(b);
                        let take = s.count_of(&k).min(b.count as i32);
                        s.add(&k, -take);
                    }
                }
            });
        }
        packager_stock(game, dim, p);
    }
}

/// `RedstoneRequesterBlockEntity.triggerRequest`.
fn trigger_requester(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    let Some(net) = with(dim, |n| n.requesters.get(&pos).map(|r| r.network)) else {
        return;
    };
    let stock = net.map_or_else(Summary::default, |net| network_stock(game, dim, net, true));
    let Some((trigger, order, address)) = with(dim, |n| {
        let r = n.requesters.get_mut(&pos)?;
        Some((r.r.trigger(&stock), r.r.order.clone(), r.r.address.clone()))
    }) else {
        return;
    };
    if let (Trigger::Send { .. }, Some(net)) = (&trigger, net) {
        broadcast(
            game,
            dim,
            net,
            RequestType::Redstone,
            &order,
            None,
            &address,
        );
    }
}

/// A gauge looked at (`tickStorageMonitor`, `tickRequests`).
fn look_gauge(game: &mut Game<'_>, dim: usize, pos: [i32; 3], now: u64) {
    let Some(mut panel) = with(dim, |n| n.gauges.get(&pos).cloned()) else {
        return;
    };
    let Some(item) = panel.filter.clone() else {
        return;
    };
    let max = u32::from(stack_limit(game, &item.0.item));
    let packager = if panel.restocker {
        restocked(game, pos)
    } else {
        None
    };
    let unloaded = if panel.restocker {
        u32::from(packager.is_none())
    } else {
        with(dim, |n| n.unloaded.get(&panel.network).map_or(0, Vec::len)) as u32
    };
    let in_storage = if panel.restocker {
        packager.map_or(0, |p| packager_stock(game, dim, p).0.count_of(&item))
    } else {
        network_stock(game, dim, panel.network, false).count_of(&item)
    };
    let expiry = panel.promise_expiry();
    let promised = if panel.restocker {
        panel.restocker_promises.total(&item, expiry, now)
    } else {
        with(dim, |n| {
            n.promises
                .get_mut(&panel.network)
                .map_or(0, |p| p.total(&item, expiry, now))
        })
    };
    let step = panel.look(max, in_storage, promised, unloaded);
    let (net, address) = (panel.network, panel.recipe_address.clone());
    with(dim, |n| {
        if let Some(g) = n.gauges.get_mut(&pos) {
            *g = panel.clone();
        }
    });
    match step {
        GaugeStep::Idle => {}
        // `tryRestock`.
        GaugeStep::Restock { item, demand } => {
            let Some(p) = packager else { return };
            let Some(inv) = packages::available(game, dim, p).1 else {
                return;
            };
            let available: i32 = sources(game, dim, net, Some(inv))
                .iter()
                .filter(|s| s.packager.is_some())
                .map(|s| s.stock.count_of(&item))
                .sum();
            if available == 0 {
                return;
            }
            let amount = st::restock_amount(demand, promised, in_storage, max, available);
            let order = Order::simple(vec![(item.clone(), amount)]);
            if broadcast(
                game,
                dim,
                net,
                RequestType::Restock,
                &order,
                Some(inv),
                &address,
            ) {
                with(dim, |n| {
                    if let Some(g) = n.gauges.get_mut(&pos) {
                        g.restocker_promises.add(item, amount, now);
                    }
                });
            }
        }
        GaugeStep::Request => request_inputs(game, dim, pos, &panel, now),
    }
}

/// `tickRequests` of a gauge fed by others: what each asks for, from its network, all or none.
fn request_inputs(game: &mut Game<'_>, dim: usize, pos: [i32; 3], panel: &Panel<Key>, now: u64) {
    let groups = with(dim, |n| {
        st::consolidate(&panel.inputs, |p| {
            n.gauges.get(&p).map(|g| (g.network, g.filter.clone()))
        })
    });
    let Some(groups) = groups else { return };
    let mut failed = false;
    let mut wanted: Vec<(Network, Vec<(Key, u32)>)> = Vec::new();
    for (net, items) in groups {
        let stock = network_stock(game, dim, net, true);
        let mut list = Vec::new();
        for (item, total) in items {
            match item {
                Some(k) if total > 0 && stock.count_of(&k) >= total as i32 => list.push((k, total)),
                _ => failed = true,
            }
        }
        wanted.push((net, list));
    }
    if failed {
        return;
    }
    let crafts = if panel.craft.is_empty() {
        Vec::new()
    } else {
        vec![(panel.craft.iter().map(|k| (k.clone(), 1)).collect(), 1)]
    };
    let mut all = Vec::new();
    for (net, stacks) in wanted {
        let order = Order {
            stacks,
            crafts: crafts.clone(),
        };
        let srcs = sources(game, dim, net, None);
        let order_id = next_int(game);
        let r = st::find_packagers(&srcs, &order, &panel.recipe_address, order_id, |n| {
            (next_int(game) as u32 % n.max(1) as u32) as usize
        });
        all.push(st::by_packager(r));
    }
    if all
        .iter()
        .flatten()
        .any(|(p, _)| packages::too_busy(dim, *p, RequestType::Restock))
    {
        return;
    }
    for g in all {
        perform(game, dim, g);
    }
    // The output promised to the gauge's network (`getQueuedPromises`: a network with links).
    let Some(item) = panel.filter.clone() else {
        return;
    };
    let known = with(dim, |n| {
        n.links.values().any(|l| l.network == Some(panel.network))
    });
    if known {
        with(dim, |n| {
            n.promises
                .entry(panel.network)
                .or_default()
                .add(item, panel.recipe_output, now)
        });
    }
    let _ = pos;
}

/// `getAnalogOutputSignal` of a redstone requester: whether its last request went out. None
/// for anything else.
pub(super) fn requester_succeeded(dim: usize, pos: [i32; 3]) -> Option<bool> {
    STOCK.with(|s| {
        let s = s.try_borrow().ok()?;
        Some(s.as_ref()?[dim].requesters.get(&pos)?.r.last_succeeded)
    })
}

/// Our debug commands, standing in for the linking item's click (`network`), the ticker's screen
/// (`stock`, `order`), the requester's screen (`requester`) and the gauge's screens and
/// connections (`gauge`, `gauge_connect`, `gauge_craft`).
pub(super) fn register_commands(commands: &mut Vec<super::native_server::NativeCommand>) {
    for usage in [
        "create_debug network <pos:block_pos> <id:text>",
        "create_debug stock <pos:block_pos>",
        "create_debug order <pos:block_pos> <order:text>",
        "create_debug requester <pos:block_pos> <order:text>",
        "create_debug gauge <pos:block_pos> <settings:text>",
        "create_debug gauge_connect <from:block_pos> <to:text>",
        "create_debug gauge_craft <pos:block_pos> <recipe:text>",
    ] {
        commands.push(super::native_server::NativeCommand::op(usage, 2));
    }
}

/// An item id from a command (`minecraft:` when it names none).
fn item_of(game: &Game<'_>, word: &str) -> Option<Key> {
    let id = if word.contains(':') {
        word.to_owned()
    } else {
        format!("minecraft:{word}")
    };
    game.items().get(&id)?;
    Some(Key(ItemStack::new(&id, 1)))
}

/// `<item> <count>` pairs.
fn stacks_of(game: &Game<'_>, words: &[&str]) -> Result<Vec<(Key, u32)>, String> {
    if words.len() % 2 != 0 {
        return Err("expected <item> <count> pairs".into());
    }
    words
        .chunks(2)
        .map(|w| {
            let item = item_of(game, w[0]).ok_or_else(|| format!("no item {}", w[0]))?;
            let n = w[1]
                .parse::<u32>()
                .map_err(|_| format!("not a count: {}", w[1]))?;
            Ok((item, n))
        })
        .collect()
}

pub(super) fn perform_command(game: &mut Game<'_>, command: &str) -> bool {
    let Some(rest) = command.strip_prefix("create_debug ") else {
        return false;
    };
    let (verb, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    if !matches!(
        verb,
        "network" | "stock" | "order" | "requester" | "gauge" | "gauge_connect" | "gauge_craft"
    ) {
        return false;
    }
    let player = game.native_acting_player();
    let here = player.as_ref().map(|p| p.position);
    let dim = player
        .as_ref()
        .and_then(|p| dimension_index(&p.dimension))
        .unwrap_or(0);
    let words: Vec<&str> = rest.split_whitespace().collect();
    let line = match packages::coords(&words, here) {
        None => format!("usage: create_debug {verb} <x y z> ..."),
        Some(pos) => {
            let args = &words[3..];
            let result = match verb {
                "network" => set_network(game, dim, pos, args),
                "stock" => Ok(describe(game, dim, pos)),
                "order" => order(game, dim, pos, args),
                "requester" => set_requester(game, dim, pos, args),
                "gauge" => set_gauge(game, dim, pos, args),
                "gauge_connect" => connect(dim, pos, args, here),
                _ => set_craft(game, dim, pos, args),
            };
            result.unwrap_or_else(|e| e)
        }
    };
    game.native_feedback(horizon_client::text::Text::literal(line));
    true
}

/// The linking item's click (`LogisticallyLinkedBlockItem`): the block joins network `id`.
fn set_network(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    args: &[&str],
) -> Result<String, String> {
    let text = args.join(" ");
    if text.is_empty() {
        return Err("usage: create_debug network <x y z> <id>".into());
    }
    let net = st::network_id(&text);
    let block = block_at(game, pos);
    let set = with(dim, |n| {
        let set = match block {
            Some(Block::StockLink) => n.links.get_mut(&pos).map(|l| l.network = Some(net)),
            Some(Block::StockTicker) => n.tickers.get_mut(&pos).map(|t| t.network = Some(net)),
            Some(Block::RedstoneRequester) => {
                n.requesters.get_mut(&pos).map(|r| r.network = Some(net))
            }
            Some(Block::FactoryGauge) => n.gauges.get_mut(&pos).map(|g| g.network = net),
            _ => None,
        };
        n.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        n.recent.remove(&net);
        n.accurate.remove(&net);
        set
    });
    match set {
        Some(()) => Ok(format!("network {} for {pos:?}", st::network_text(net))),
        None => Err("no stock link, ticker, requester or gauge there".into()),
    }
}

/// The network of the block at `pos`.
fn network_at(dim: usize, pos: [i32; 3]) -> Option<Network> {
    with(dim, |n| {
        n.links
            .get(&pos)
            .and_then(|l| l.network)
            .or_else(|| n.tickers.get(&pos).and_then(|t| t.network))
            .or_else(|| n.requesters.get(&pos).and_then(|r| r.network))
            .or_else(|| n.gauges.get(&pos).map(|g| g.network).filter(|n| *n != 0))
    })
}

fn stock_line(s: &Summary<Key>) -> String {
    s.by_count()
        .iter()
        .map(|(k, n)| format!("{n} {}", k.0.item))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The ticker's screen (`LogisticalStockRequestPacket`: `getRecentSummary`), or a gauge's state.
fn describe(game: &Game<'_>, dim: usize, pos: [i32; 3]) -> String {
    if let Some(g) = with(dim, |n| n.gauges.get(&pos).cloned()) {
        let inputs: Vec<String> = g.inputs.iter().map(|(p, a)| format!("{p:?}x{a}")).collect();
        return format!(
            "gauge {} x{}{} address=\"{}\" network={} restocker={} satisfied={} promised_satisfied={} waiting={} level={} promised={} inputs=[{}] output={}",
            g.filter.as_ref().map_or("-", |k| &*k.0.item),
            g.count,
            if g.up_to { "" } else { " stacks" },
            g.recipe_address,
            if g.network == 0 {
                "-".to_owned()
            } else {
                st::network_text(g.network)
            },
            g.restocker,
            g.satisfied,
            g.promised_satisfied,
            g.waiting,
            g.last_level,
            g.last_promised,
            inputs.join(", "),
            g.recipe_output,
        );
    }
    if let Some(r) = with(dim, |n| n.requesters.get(&pos).cloned()) {
        return format!(
            "requester to \"{}\" partial={} powered={} succeeded={} order=[{}]",
            r.r.address,
            r.r.allow_partial,
            r.r.powered,
            r.r.last_succeeded,
            r.r.order
                .stacks
                .iter()
                .map(|(k, n)| format!("{n} {}", k.0.item))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let Some(net) = network_at(dim, pos) else {
        return "no network there".into();
    };
    let s = network_stock(game, dim, net, false);
    format!(
        "stock of {} from {} links: {}",
        st::network_text(net),
        s.contributing_links,
        stock_line(&s)
    )
}

/// The ticker's order (`PackageOrderRequestPacket`): `<address> <item> <count>...`.
fn order(game: &mut Game<'_>, dim: usize, pos: [i32; 3], args: &[&str]) -> Result<String, String> {
    let usage = "usage: create_debug order <x y z> <address> <item> <count>...";
    let Some((address, items)) = args.split_first() else {
        return Err(usage.into());
    };
    let Some(net) = with(dim, |n| n.tickers.get(&pos).map(|t| t.network)) else {
        return Err("no stock ticker there".into());
    };
    let Some(net) = net else {
        return Err("the ticker is on no network".into());
    };
    let order = Order::simple(stacks_of(game, items)?);
    let sent = broadcast(game, dim, net, RequestType::Player, &order, None, address);
    with(dim, |n| {
        if let Some(t) = n.tickers.get_mut(&pos) {
            t.previous = (*address).to_owned();
        }
        n.unsaved.push((pos[0] >> 4, pos[2] >> 4));
    });
    Ok(if sent {
        format!("ordered {} stacks to \"{address}\"", order.stacks.len())
    } else {
        "nothing ordered: the packagers are busy or the order is empty".into()
    })
}

/// The requester's screen: `<address> <all|partial> <item> <count>...`.
fn set_requester(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    args: &[&str],
) -> Result<String, String> {
    let usage = "usage: create_debug requester <x y z> <address> <all|partial> <item> <count>...";
    let [address, mode, items @ ..] = args else {
        return Err(usage.into());
    };
    let partial = match *mode {
        "all" => false,
        "partial" => true,
        _ => return Err(usage.into()),
    };
    let stacks = stacks_of(game, items)?;
    with(dim, |n| {
        let r = n.requesters.get_mut(&pos)?;
        r.r.order = Order::simple(stacks);
        r.r.address = (*address).to_owned();
        r.r.allow_partial = partial;
        n.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        Some(format!("requester to \"{address}\""))
    })
    .ok_or_else(|| "no redstone requester there".into())
}

/// The gauge's item, amount and address: `<item|-> <count> <items|stacks> [address]`.
fn set_gauge(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    args: &[&str],
) -> Result<String, String> {
    let usage = "usage: create_debug gauge <x y z> <item|-> <count> <items|stacks> [address]";
    let [item, count, mode, address @ ..] = args else {
        return Err(usage.into());
    };
    let filter = if *item == "-" {
        None
    } else {
        Some(item_of(game, item).ok_or_else(|| format!("no item {item}"))?)
    };
    let count: u32 = count.parse().map_err(|_| usage.to_owned())?;
    let up_to = match *mode {
        "items" => true,
        "stacks" => false,
        _ => return Err(usage.into()),
    };
    let address = address.join(" ");
    let now = game.world.game_time.max(0) as u64;
    let restocker = game_restocker(game, pos);
    with(dim, |n| {
        let g = n.gauges.get_mut(&pos)?;
        g.filter = filter;
        g.count = count;
        g.up_to = up_to;
        g.recipe_address = address.clone();
        g.restocker = restocker;
        if g.filter.is_some() {
            // Its timer starts at 0: looked at the next tick.
            n.due.insert(pos, now);
            n.next = n.next.min(now);
        } else {
            n.due.remove(&pos);
        }
        n.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        Some(format!("gauge {item} x{count} {mode} to \"{address}\""))
    })
    .ok_or_else(|| "no factory gauge there".into())
}

/// `FactoryPanelBehaviour.addConnection`: the gauge at `from` feeds the one at `to` (at most
/// nine; never a restocker), asking `amount`.
fn connect(
    dim: usize,
    from: [i32; 3],
    args: &[&str],
    here: Option<[f64; 3]>,
) -> Result<String, String> {
    let usage = "usage: create_debug gauge_connect <from x y z> <to x y z> [amount]";
    let to = packages::coords(args, here).ok_or_else(|| usage.to_owned())?;
    let amount = match args.get(3) {
        Some(a) => a.parse::<u32>().map_err(|_| usage.to_owned())?,
        None => 1,
    };
    with(dim, |n| {
        if !n.gauges.contains_key(&from) || from == to {
            return Err("no factory gauge to connect from".into());
        }
        let g = n
            .gauges
            .get_mut(&to)
            .ok_or("no factory gauge to connect to")?;
        if g.restocker {
            return Err("a restocking gauge takes no inputs".into());
        }
        match g.inputs.iter().position(|i| i.0 == from) {
            Some(i) => g.inputs[i].1 = amount,
            None if g.inputs.len() >= st::MAX_INPUTS => return Err("nine inputs already".into()),
            None => g.inputs.push((from, amount)),
        }
        let f = n.gauges.get_mut(&from).unwrap();
        if !f.outputs.contains(&to) {
            f.outputs.push(to);
        }
        n.unsaved.push((to[0] >> 4, to[2] >> 4));
        n.unsaved.push((from[0] >> 4, from[2] >> 4));
        Ok(format!("gauge {from:?} feeds {to:?} x{amount}"))
    })
}

/// The gauge's recipe: `<output count> [item...]` (`recipeOutput`, `activeCraftingArrangement`).
fn set_craft(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    args: &[&str],
) -> Result<String, String> {
    let usage = "usage: create_debug gauge_craft <x y z> <output count> [item...]";
    let Some((count, items)) = args.split_first() else {
        return Err(usage.into());
    };
    let output: u32 = count.parse().map_err(|_| usage.to_owned())?;
    if items.len() > package_slots() {
        return Err("nine items at most".into());
    }
    let craft = items
        .iter()
        .map(|w| item_of(game, w).ok_or_else(|| format!("no item {w}")))
        .collect::<Result<Vec<_>, _>>()?;
    with(dim, |n| {
        let g = n.gauges.get_mut(&pos)?;
        g.recipe_output = output.max(1);
        g.craft = craft;
        n.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        Some(format!("gauge makes {output} from {} items", items.len()))
    })
    .ok_or_else(|| "no factory gauge there".into())
}

fn package_slots() -> usize {
    create_engine::package::SLOTS
}

const KEY: &str = "create_stock";

fn ints(t: Option<&Tag>) -> Vec<i32> {
    match t {
        Some(Tag::IntArray(v)) => v.clone(),
        _ => Vec::new(),
    }
}

fn text(t: Option<&Tag>) -> String {
    match t {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    }
}

fn freq(net: Option<Network>) -> Tag {
    Tag::IntArray(net.map_or_else(Vec::new, |n| st::network_ints(n).to_vec()))
}

/// Links, tickers, requesters and gauges of a column saved (their block entities' `write`).
pub(super) fn chunk_data_save(
    game: &Game<'_>,
    dimension: &str,
    chunk: [i32; 2],
    data: &mut Compound,
) {
    if !present() {
        return;
    }
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let now = game.world.game_time.max(0) as u64;
    let inside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]);
    let pos_tag = |p: [i32; 3]| Tag::IntArray(p.to_vec());
    let lists = with(dim, |n| {
        let sorted = |mut v: Vec<[i32; 3]>| {
            v.sort_unstable();
            v
        };
        let links: Vec<Tag> = sorted(n.links.keys().filter(|p| inside(p)).copied().collect())
            .into_iter()
            .map(|p| {
                let l = n.links[&p];
                Tag::compound([
                    ("Pos", pos_tag(p)),
                    ("Freq", freq(l.network)),
                    ("Power", Tag::IntArray(vec![l.power])),
                ])
            })
            .collect();
        let tickers: Vec<Tag> = sorted(n.tickers.keys().filter(|p| inside(p)).copied().collect())
            .into_iter()
            .map(|p| {
                let t = &n.tickers[&p];
                Tag::compound([
                    ("Pos", pos_tag(p)),
                    ("Freq", freq(t.network)),
                    ("PreviousAddress", Tag::String(t.previous.as_str().into())),
                ])
            })
            .collect();
        let requesters: Vec<Tag> =
            sorted(n.requesters.keys().filter(|p| inside(p)).copied().collect())
                .into_iter()
                .map(|p| {
                    let r = &n.requesters[&p];
                    Tag::compound([
                        ("Pos", pos_tag(p)),
                        ("Freq", freq(r.network)),
                        (
                            "State",
                            Tag::IntArray(vec![
                                i32::from(r.r.powered),
                                i32::from(r.r.last_succeeded),
                                i32::from(r.r.allow_partial),
                            ]),
                        ),
                        ("EncodedAddress", Tag::String(r.r.address.as_str().into())),
                        ("EncodedRequest", body::order_nbt(&order_stack(&r.r.order))),
                    ])
                })
                .collect();
        let gauges: Vec<Tag> = sorted(n.gauges.keys().filter(|p| inside(p)).copied().collect())
            .into_iter()
            .map(|p| {
                let g = &n.gauges[&p];
                let mut c = vec![
                    ("Pos", pos_tag(p)),
                    ("Freq", freq((g.network != 0).then_some(g.network))),
                    (
                        "Settings",
                        Tag::IntArray(vec![
                            g.count as i32,
                            i32::from(g.up_to),
                            g.recipe_output as i32,
                            g.promise_clearing,
                            i32::from(g.restocker),
                            i32::from(g.satisfied),
                            i32::from(g.promised_satisfied),
                            i32::from(g.waiting),
                        ]),
                    ),
                    (
                        "RecipeAddress",
                        Tag::String(g.recipe_address.as_str().into()),
                    ),
                    (
                        "TargetedBy",
                        Tag::IntArray(
                            g.inputs
                                .iter()
                                .flat_map(|(p, a)| [p[0], p[1], p[2], *a as i32])
                                .collect(),
                        ),
                    ),
                    (
                        "Targeting",
                        Tag::IntArray(g.outputs.iter().flatten().copied().collect()),
                    ),
                    (
                        "Craft",
                        Tag::List(g.craft.iter().map(|k| body::to_nbt(&k.0)).collect()),
                    ),
                ];
                if let Some(f) = &g.filter {
                    c.push(("Filter", body::to_nbt(&f.0)));
                }
                if !g.restocker_promises.is_empty() {
                    // `RequestPromise`: the stack, its count and age.
                    let list = g
                        .restocker_promises
                        .list
                        .iter()
                        .map(|(k, n, at)| {
                            Tag::compound([
                                ("Item", body::to_nbt(&k.0)),
                                (
                                    "Count",
                                    Tag::IntArray(vec![*n as i32, now.saturating_sub(*at) as i32]),
                                ),
                            ])
                        })
                        .collect();
                    c.push(("Promises", Tag::List(list)));
                }
                Tag::compound(c)
            })
            .collect();
        (links, tickers, requesters, gauges)
    });
    let (links, tickers, requesters, gauges) = lists;
    if links.is_empty() && tickers.is_empty() && requesters.is_empty() && gauges.is_empty() {
        data.remove(&KEY.into());
        return;
    }
    data.insert(
        KEY.into(),
        Tag::compound([
            ("links", Tag::List(links)),
            ("tickers", Tag::List(tickers)),
            ("requesters", Tag::List(requesters)),
            ("gauges", Tag::List(gauges)),
        ]),
    );
}

/// A column joined a level: its links, tickers, requesters and gauges back (`read`).
pub(super) fn chunk_load(game: &Game<'_>, dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let Some(data) = Game::native_chunk_data(chunk, KEY) else {
        return;
    };
    let now = game.world.game_time.max(0) as u64;
    let list = |key: &str| -> Vec<Compound> {
        match data.get(&key.into()) {
            Some(Tag::List(l)) => l
                .iter()
                .filter_map(|t| match t {
                    Tag::Compound(c) => Some(c.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    let pos_of = |c: &Compound| -> Option<[i32; 3]> {
        let v = ints(c.get(&"Pos".into()));
        (v.len() == 3).then(|| [v[0], v[1], v[2]])
    };
    let net_of = |c: &Compound| st::network_from_ints(&ints(c.get(&"Freq".into())));
    with(dim, |n| {
        for c in list("links") {
            let Some(pos) = pos_of(&c) else { continue };
            let network = net_of(&c);
            if let Some(net) = network
                && let Some(u) = n.unloaded.get_mut(&net)
            {
                u.retain(|p| *p != pos);
            }
            n.links.insert(
                pos,
                Link {
                    network,
                    power: ints(c.get(&"Power".into())).first().copied().unwrap_or(0),
                },
            );
        }
        for c in list("tickers") {
            let Some(pos) = pos_of(&c) else { continue };
            n.tickers.insert(
                pos,
                Ticker {
                    network: net_of(&c),
                    previous: text(c.get(&"PreviousAddress".into())),
                },
            );
        }
        for c in list("requesters") {
            let Some(pos) = pos_of(&c) else { continue };
            let s = ints(c.get(&"State".into()));
            let flag = |i: usize| s.get(i).is_some_and(|v| *v != 0);
            let order = c
                .get(&"EncodedRequest".into())
                .and_then(body::order_from_nbt)
                .map(|o| order_key(&o))
                .unwrap_or_default();
            n.requesters.insert(
                pos,
                Req {
                    network: net_of(&c),
                    r: Requester {
                        order,
                        address: text(c.get(&"EncodedAddress".into())),
                        allow_partial: flag(2),
                        powered: flag(0),
                        last_succeeded: flag(1),
                    },
                },
            );
        }
        for c in list("gauges") {
            let Some(pos) = pos_of(&c) else { continue };
            let mut g = Panel::new(net_of(&c).unwrap_or(0));
            let s = ints(c.get(&"Settings".into()));
            let at = |i: usize| s.get(i).copied().unwrap_or(0);
            g.count = at(0).max(0) as u32;
            g.up_to = at(1) != 0;
            g.recipe_output = at(2).max(1) as u32;
            g.promise_clearing = if s.len() > 3 { at(3) } else { -1 };
            g.restocker = at(4) != 0;
            g.satisfied = at(5) != 0;
            g.promised_satisfied = at(6) != 0;
            g.waiting = at(7) != 0;
            g.recipe_address = text(c.get(&"RecipeAddress".into()));
            g.inputs = ints(c.get(&"TargetedBy".into()))
                .chunks_exact(4)
                .map(|v| ([v[0], v[1], v[2]], v[3].max(0) as u32))
                .collect();
            g.outputs = ints(c.get(&"Targeting".into()))
                .chunks_exact(3)
                .map(|v| [v[0], v[1], v[2]])
                .collect();
            if let Some(Tag::List(l)) = c.get(&"Craft".into()) {
                g.craft = l
                    .iter()
                    .filter_map(body::from_nbt)
                    .map(|s| Key::of(&s))
                    .collect();
            }
            g.filter = c
                .get(&"Filter".into())
                .and_then(body::from_nbt)
                .map(|s| Key::of(&s));
            if let Some(Tag::List(l)) = c.get(&"Promises".into()) {
                for t in l {
                    let Tag::Compound(e) = t else { continue };
                    let Some(item) = e.get(&"Item".into()).and_then(body::from_nbt) else {
                        continue;
                    };
                    let v = ints(e.get(&"Count".into()));
                    let count = v.first().copied().unwrap_or(0).max(0) as u32;
                    let age = v.get(1).copied().unwrap_or(0).max(0) as u64;
                    g.restocker_promises
                        .add(Key::of(&item), count, now.saturating_sub(age));
                }
            }
            if g.filter.is_some() {
                n.due.insert(pos, now);
                n.next = n.next.min(now);
            }
            n.gauges.insert(pos, g);
        }
    });
}

/// A column left a level: what was in it is forgotten (kept in its save); its links count as
/// unloaded in their networks.
pub(super) fn chunk_unload(dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    if !present() {
        return;
    }
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let inside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk.x, chunk.z);
    with(dim, |n| {
        let gone: Vec<([i32; 3], Option<Network>)> = n
            .links
            .iter()
            .filter(|(p, _)| inside(p))
            .map(|(p, l)| (*p, l.network))
            .collect();
        for (p, net) in gone {
            n.links.remove(&p);
            if let Some(net) = net {
                n.unloaded.entry(net).or_default().push(p);
            }
        }
        n.tickers.retain(|p, _| !inside(p));
        n.requesters.retain(|p, _| !inside(p));
        n.gauges.retain(|p, _| !inside(p));
        n.due.retain(|p, _| !inside(p));
        n.last.retain(|p, _| !inside(p));
    });
}
