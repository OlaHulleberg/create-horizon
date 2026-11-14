//! Create's stock logistics: stock links joining packagers into logistics networks, stock
//! tickers, redstone requesters and factory gauges. A sibling of `packages` (packagers pack
//! and send what is ordered); the rules are `create_engine::stock`'s. State is typed, per
//! level, on the server thread and saved with its column as `create_stock`; the networks
//! themselves (their links loaded or not, promises and owners: Create's
//! `LogisticsNetworkSavedData`) in an attachment of the overworld (`create:logistics_networks`,
//! the level's saved data), written when they changed. Nothing here ticks: a network's stock is
//! summed when something asks for it (a ticker's dump, an order, a requester's pulse, a gauge's
//! look) and kept for Create's cache times (one tick for orders, 20 for gauges); gauges are
//! looked at on their interval only.
//!
//! Screens: the stock keeper's request screen (`keeper`: a mob seated beside a ticker, or a
//! blaze burner beside it, is clicked) and the redstone requester's (`requester`). The ticker's
//! category editor and the factory gauge's screens and connections are not ported: gauges are
//! still set with `/create_debug gauge`, `gauge_connect` and `gauge_craft`.
//!
//! The overworld's saved networks are read at its first tick (an attachment is reached through
//! a level's own host): a link whose column loaded before then is not counted as unloaded.

pub(crate) mod keeper;
mod requester;

use super::packages::{self as packages, add, block_of, body, command_player, coords, feedback};
use super::packages::{debug_verb, ints, list, pos_of, text};
use super::server::{ServerLevel, UPDATE_CLIENTS, update_all};
use super::states;
use super::subsystem::Subsystem;
use super::world::{level_id, level_index, with_world};
use super::{kinetics, persist};
use create_engine::Block;
use create_engine::kinetics::Dir;
use create_engine::steam::Face;
use create_engine::stock::{
    self as st, GaugeStep, LevelNetworks, Network, Order, OrderData, Ownership, Panel, Promises,
    RequestType, Requester, SavedNetwork, Source, Summary, Trigger,
};
use mod_api::{
    AnalogOutputEvent, AttachmentCodec, AttachmentType, BlockPart, Command, CommandArgumentValue,
    Compound, Flow, HasAnalogOutputEvent, Holder, HostBlockStates, InteractionResult, ItemPart,
    ItemStack, LevelHost, OnBlockPlacedByEvent, RegisterCommandsEvent, Registrar,
    RegistrationContext, RunModCommandEvent, ServerHost, Tag, UseItemOnEvent,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// A stack as an order or a summary keys it: count 1, compared as Create compares stacks.
#[derive(Clone)]
pub struct Key(pub ItemStack);

impl PartialEq for Key {
    fn eq(&self, o: &Self) -> bool {
        self.0.is_stackable_with(&o.0)
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0.item())
    }
}

impl Key {
    pub fn of(s: &ItemStack) -> Key {
        Key(body::with_count(s, 1).unwrap_or_else(|| s.clone()))
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

pub fn order_data_key(d: &OrderData<ItemStack>) -> OrderData<Key> {
    OrderData {
        order_id: d.order_id,
        link_index: d.link_index,
        final_link: d.final_link,
        fragment_index: d.fragment_index,
        is_final: d.is_final,
        context: d.context.as_ref().map(order_key),
    }
}

pub fn order_data_stack(d: &OrderData<Key>) -> OrderData<ItemStack> {
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

/// A level's stock logistics.
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

/// A block of ours placed or removed: level, position, was, is.
type Change = (usize, [i32; 3], Option<Block>, Option<Block>);

/// The overworld's saved networks (`data/create/logistics_networks.dat` in Create; here the
/// level's attachments' saved data).
static NETWORKS: AttachmentType<Compound> = AttachmentType {
    namespace: "create",
    name: "logistics_networks",
    default: Compound::new,
    codec: Some(AttachmentCodec {
        encode: |data| Ok(data.clone()),
        decode: |data| Ok(data.clone()),
    }),
    copy_on_death: false,
    sync: false,
};

thread_local! {
    static STOCK: RefCell<Option<Vec<Net>>> = const { RefCell::new(None) };
    static CHANGES: RefCell<Vec<Change>> = const { RefCell::new(Vec::new()) };
    /// `LogisticsNetwork.owner` and `locked`, by network (`GlobalLogisticsManager`'s, for every
    /// level).
    static OWNERS: RefCell<HashMap<Network, Ownership>> = RefCell::new(HashMap::new());
    /// `LogisticsNetworkSavedData.setDirty`: a network's links, promises or owner changed since
    /// they were last written.
    static DIRTY: Cell<bool> = const { Cell::new(false) };
    /// Whether the saved networks were read for this world.
    static LOADED: Cell<bool> = const { Cell::new(false) };
    /// `LogisticsManager.r` and friends: order ids and picks.
    static RNG: Cell<u64> = const { Cell::new(0x2545_f491_4f6c_dd1d) };
}

fn with<R>(dim: usize, f: impl FnOnce(&mut Net) -> R) -> R {
    STOCK.with_borrow_mut(|s| {
        let levels = s.get_or_insert_with(Vec::new);
        if levels.len() <= dim {
            levels.resize_with(dim + 1, Net::default);
        }
        f(&mut levels[dim])
    })
}

fn present() -> bool {
    STOCK.with_borrow(Option::is_some)
}

/// `GlobalLogisticsManager.markDirty`.
fn mark_dirty() {
    DIRTY.set(true);
}

/// `Random.nextInt()` stand-in, mixed with the level's game time.
pub fn next_int(time: i64) -> i32 {
    RNG.with(|r| {
        let mut x = r.get() ^ (time as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        x ^= x >> 30;
        x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x ^= x >> 27;
        x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
        x ^= x >> 31;
        r.set(x);
        x as i32
    })
}

pub fn ours(b: Block) -> bool {
    matches!(
        b,
        Block::StockLink | Block::StockTicker | Block::RedstoneRequester | Block::FactoryGauge
    )
}

fn sub(p: [i32; 3], o: [i32; 3]) -> [i32; 3] {
    [p[0] - o[0], p[1] - o[1], p[2] - o[2]]
}

fn block_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Block> {
    block_of(lv.block(pos))
}

fn now(lv: &ServerLevel<'_>) -> u64 {
    lv.game_time().max(0) as u64
}

/// `FaceAttachedHorizontalDirectionalBlock.getConnectedDirection` of a link or gauge: away from
/// what it hangs on.
fn connected(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(Block, Dir)> {
    let state = lv.block(pos);
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
pub fn link_of(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<[i32; 3]> {
    Dir::ALL.into_iter().find_map(|d| {
        let at = add(pos, d.offset());
        (connected(lv, at) == Some((Block::StockLink, d))).then_some(at)
    })
}

/// `PackagerLinkBlockEntity.getPackager`: the packager the link hangs on (not a re-packager),
/// none at full power.
fn link_packager(lv: &ServerLevel<'_>, dim: usize, pos: [i32; 3]) -> Option<[i32; 3]> {
    if with(dim, |n| n.links.get(&pos).is_some_and(|l| l.power == 15)) {
        return None;
    }
    let (Block::StockLink, d) = connected(lv, pos)? else {
        return None;
    };
    let at = sub(pos, d.offset());
    (block_at(lv, at) == Some(Block::Packager)).then_some(at)
}

/// `FactoryPanelBlockEntity.getRestockedPackager`.
fn restocked(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<[i32; 3]> {
    let (Block::FactoryGauge, d) = connected(lv, pos)? else {
        return None;
    };
    let at = sub(pos, d.offset());
    (block_at(lv, at) == Some(Block::Packager)).then_some(at)
}

/// `recheckIfLinksPresent`: a packager's `linked` follows a link on it.
pub fn recheck_linked(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    if block_at(lv, pos) != Some(Block::Packager) {
        return;
    }
    let Some(state) = lv.block_state(pos) else {
        return;
    };
    let linked = state
        .property(&lv.host.block_states(), "linked")
        .ok()
        .flatten()
        == Some("true");
    let should = link_of(lv, pos).is_some();
    if linked != should {
        lv.set_property(
            pos,
            "linked",
            if should { "true" } else { "false" },
            update_all(),
        );
    }
}

/// One of ours placed or removed.
fn block_moved(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    pos: [i32; 3],
    was: Option<Block>,
    is: Option<Block>,
) {
    match was {
        // `linkRemoved`.
        Some(Block::StockLink) => with(dim, |n| {
            if n.links.remove(&pos).is_some_and(|l| l.network.is_some()) {
                mark_dirty();
            }
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
            recheck_linked(lv, add(pos, d.offset()));
        }
    }
    with(dim, |n| n.unsaved.push((pos[0] >> 4, pos[2] >> 4)));
}

/// `getStateForPlacement`: a link on a ceiling turns round (`PackagerLinkBlock`), a gauge on a
/// floor too (`FactoryPanelBlock`); a requester reads its signal.
fn place_block(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let state = lv.block(pos);
    let Some((b, info)) = with_world(|w| w.table.get(state).map(|(b, i)| (b, *i))).flatten() else {
        return;
    };
    let turn = match b {
        Block::StockLink => info.face == Some(Face::Ceiling),
        Block::FactoryGauge => info.face == Some(Face::Floor),
        Block::RedstoneRequester => {
            let signal = lv.has_neighbor_signal(pos);
            if signal != info.powered {
                lv.set_property(
                    pos,
                    "powered",
                    if signal { "true" } else { "false" },
                    update_all(),
                );
            }
            return;
        }
        _ => return,
    };
    if turn && let Some(f) = info.facing {
        lv.set_property(
            pos,
            "facing",
            kinetics::dir_name(f.opposite()),
            update_all(),
        );
    }
}

/// `PackagerLinkBlock.getPower`: the strongest signal from every side but the one it hangs on.
fn link_power(lv: &ServerLevel<'_>, pos: [i32; 3]) -> i32 {
    let Some((_, c)) = connected(lv, pos) else {
        return 0;
    };
    let mut power = 0;
    for d in Dir::ALL {
        if d.opposite() == c {
            continue;
        }
        let at = add(pos, d.offset());
        power = power.max(lv.signal(at, packages::direction(d)));
    }
    power
}

pub struct Stock;

impl Subsystem for Stock {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        for b in [
            Block::StockLink,
            Block::FactoryGauge,
            Block::RedstoneRequester,
            Block::StockTicker,
        ] {
            r.block_part(states::key(b), StockBlock);
        }
        for item in [
            "create:stock_link",
            "create:stock_ticker",
            "create:factory_gauge",
            "create:redstone_requester",
        ] {
            let key = r.item_key(item);
            r.item_part(key, StockItem);
        }
    }

    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: mod_api::PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        match channel {
            keeper::CHANNEL => keeper::client_message(host, player, data),
            requester::CHANNEL => requester::client_message(host, player, data),
            _ => return false,
        }
        true
    }

    fn server_message(
        &self,
        host: &mut dyn mod_api::ClientHost,
        channel: &str,
        data: &[u8],
    ) -> bool {
        keeper::server_message(host, channel, data)
            || requester::server_message(host, channel, data)
    }

    fn client_tick(&self, host: &mut dyn mod_api::ClientHost) {
        keeper::tick(host);
        requester::tick(host);
    }

    fn client_reset(&self) {
        keeper::reset();
        requester::reset();
    }

    fn screen_render_post(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenRenderPostEvent<'_>,
    ) {
        keeper::screen_render_post(host, event);
        requester::screen_render_post(host, event);
    }

    fn screen_mouse_pressed(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        if keeper::screen_mouse_pressed(host, event) == Flow::Cancel {
            return Flow::Cancel;
        }
        requester::screen_mouse_pressed(host, event)
    }

    fn screen_mouse_scrolled(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenMouseScrolledPreEvent<'_>,
    ) -> Flow {
        if keeper::screen_mouse_scrolled(host, event) == Flow::Cancel {
            return Flow::Cancel;
        }
        requester::screen_mouse_scrolled(host, event)
    }

    fn screen_key_pressed(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenKeyPressedPreEvent<'_>,
    ) -> Flow {
        if keeper::screen_key_pressed(host, event) == Flow::Cancel {
            return Flow::Cancel;
        }
        requester::screen_key_pressed(host, event)
    }

    fn screen_char_typed(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenCharTypedPreEvent<'_>,
    ) -> Flow {
        if keeper::screen_char_typed(host, event) == Flow::Cancel {
            return Flow::Cancel;
        }
        requester::screen_char_typed(host, event)
    }

    fn screen_closing(
        &self,
        host: &dyn mod_api::ClientHost,
        event: &mut mod_api::ScreenClosingEvent<'_>,
    ) {
        keeper::screen_closing(host, event);
        requester::screen_closing(host, event);
    }

    /// `StockTickerInteractionHandler.interactWithLogisticsManager`: a mob seated beside a
    /// ticker is its keeper; clicking it opens the ticker's request screen.
    fn entity_interact(
        &self,
        host: mod_api::HostSide,
        event: &mut mod_api::EntityInteractEvent<'_>,
    ) -> Option<InteractionResult> {
        let mod_api::HostSide::Server(host) = host else {
            return None;
        };
        if event.hand != mod_api::Hand::Main || event.kind == "minecraft:player" {
            return None;
        }
        let player = host.players().acting_player()?;
        if player.entity().spectator(&*host) == Some(true) {
            return None;
        }
        let dimension = player.level(&*host)?.dimension().id(&*host)?;
        let mut lv = ServerLevel::new(host, &dimension)?;
        let entity = lv.host.entity(event.entity as u32)?;
        let ticker = keeper::ticker_beside(&lv, entity)?;
        keeper::open(&mut lv, player, ticker);
        Some(InteractionResult::Success)
    }

    /// `BlazeBurnerBlock.useItemOn`: a burner beside a stock ticker is its keeper (goggles still
    /// go on it).
    fn right_click_block(
        &self,
        host: mod_api::HostSide,
        event: &mut mod_api::RightClickBlockEvent<'_>,
    ) -> Flow {
        let mod_api::HostSide::Server(host) = host else {
            return Flow::Continue;
        };
        let click = event.click;
        if click.hand != mod_api::Hand::Main
            || click
                .stack
                .is_some_and(|s| s.item().id(&*host) == "create:goggles")
        {
            return Flow::Continue;
        }
        let dimension = click.dimension.key().id().to_owned();
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return Flow::Continue;
        };
        if block_at(&lv, click.pos) != Some(Block::BlazeBurner) {
            return Flow::Continue;
        }
        // `BlazeBurnerBlockEntity.getStockTicker`: a ticker on one of its sides.
        let ticker = Dir::ALL
            .into_iter()
            .filter(|d| d.axis() != create_engine::kinetics::Axis::Y)
            .map(|d| add(click.pos, d.offset()))
            .find(|p| block_at(&lv, *p) == Some(Block::StockTicker));
        let (Some(ticker), Some(player)) = (ticker, lv.host.player(click.player)) else {
            return Flow::Continue;
        };
        keeper::open(&mut lv, player, ticker);
        *event.cancellation_result = InteractionResult::Success;
        Flow::Cancel
    }

    fn world_closed(&self) {
        STOCK.with_borrow_mut(|s| *s = None);
        CHANGES.with_borrow_mut(Vec::clear);
        OWNERS.with_borrow_mut(HashMap::clear);
        DIRTY.set(false);
        LOADED.set(false);
    }

    fn block_changed(&self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        let (o, n) = (
            block_of(old).filter(|b| ours(*b)),
            block_of(new).filter(|b| ours(*b)),
        );
        if o != n {
            CHANGES.with_borrow_mut(|c| c.push((dim, pos, o, n)));
        }
    }

    /// A neighbour of a link, requester or gauge changed: looked at in the level's tick.
    fn neighbor_changed(&self, dim: usize, pos: [i32; 3], state: u32) {
        if !block_of(state).is_some_and(ours) {
            return;
        }
        with(dim, |n| {
            if !n.neighbours.contains(&pos) {
                n.neighbours.push(pos);
            }
        });
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        if lv.dimension == "minecraft:overworld" {
            if !LOADED.get() {
                LOADED.set(true);
                level_load(lv);
            }
            if DIRTY.get() {
                level_save(lv);
            }
        }
        level_tick(lv);
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        if let Some(Tag::Compound(own)) = data.get(&KEY.into()) {
            chunk_load(lv, own);
        }
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        chunk_data_save(lv, chunk, data);
    }

    fn chunk_unload(&self, dim: usize, chunk: [i32; 2]) {
        chunk_unload(dim, chunk);
    }

    /// Our debug commands, for tests: a block's network (`network`), a ticker's stock and an
    /// order (`stock`, `order`; its screen does both), a requester's order (`requester`), and the
    /// gauge's settings and connections (`gauge`, `gauge_connect`, `gauge_craft`: its screens are
    /// not ported).
    fn register_commands(&self, _host: &mut dyn ServerHost, event: &mut RegisterCommandsEvent<'_>) {
        for usage in [
            "create_debug network <pos:block_pos> <id:text>",
            "create_debug stock <pos:block_pos>",
            "create_debug order <pos:block_pos> <order:text>",
            "create_debug requester <pos:block_pos> <order:text>",
            "create_debug gauge <pos:block_pos> <settings:text>",
            "create_debug gauge_connect <from:block_pos> <to:text>",
            "create_debug gauge_craft <pos:block_pos> <recipe:text>",
        ] {
            event.commands.push(Command::op(usage, 2));
        }
    }

    fn run_mod_command(
        &self,
        host: &mut dyn ServerHost,
        event: &mut RunModCommandEvent<'_>,
    ) -> Flow {
        let verbs = [
            "network",
            "stock",
            "order",
            "requester",
            "gauge",
            "gauge_connect",
            "gauge_craft",
        ];
        let Some(verb) = debug_verb(event, &verbs) else {
            return Flow::Continue;
        };
        let here = event.command.position;
        let player = command_player(event.source).map(|p| p.uuid());
        let Some(dimension) = event.command.dimension.id(&*host) else {
            return Flow::Cancel;
        };
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return Flow::Cancel;
        };
        let dim = lv.dim;
        let arg = |name: &str| match event.command.argument(name) {
            Some(CommandArgumentValue::BlockPos(p)) => Some(*p),
            _ => None,
        };
        let rest = ["id", "order", "settings", "to", "recipe"]
            .into_iter()
            .find_map(|name| match event.command.argument(name) {
                Some(CommandArgumentValue::String(s)) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let args: Vec<&str> = rest.split_whitespace().collect();
        let line = match arg("pos").or_else(|| arg("from")) {
            None => format!("usage: create_debug {verb} <x y z> ..."),
            Some(pos) => {
                let result = match verb {
                    "network" => set_network(&mut lv, dim, pos, &args, player),
                    "stock" => Ok(describe(&mut lv, dim, pos)),
                    "order" => order(&mut lv, dim, pos, &args),
                    "requester" => set_requester(&lv, dim, pos, &args),
                    "gauge" => set_gauge(&lv, dim, pos, &args),
                    "gauge_connect" => connect(dim, pos, &args, here),
                    _ => set_craft(&lv, dim, pos, &args),
                };
                result.unwrap_or_else(|e| e)
            }
        };
        feedback(lv.host, line);
        Flow::Cancel
    }
}

/// The level's tick: blocks placed and removed, changed neighbours, then the gauges due.
/// Nothing in a level without any of ours.
fn level_tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let changes: Vec<Change> = CHANGES.with_borrow_mut(|c| {
        if c.is_empty() {
            return Vec::new();
        }
        let mine = c.iter().filter(|x| x.0 == dim).copied().collect();
        c.retain(|x| x.0 != dim);
        mine
    });
    for (_, pos, was, is) in changes {
        block_moved(lv, dim, pos, was, is);
    }
    // Blocks placed from a linking item join their network now they are known.
    let joins: Vec<(Option<Network>, [i32; 3], u128)> = JOINS.with_borrow_mut(|j| {
        let (mine, rest): (Vec<_>, Vec<_>) =
            std::mem::take(j).into_iter().partition(|x| x.0 == dim);
        *j = rest;
        mine.into_iter()
            .map(|(_, net, pos, p)| (net, pos, p))
            .collect()
    });
    for (net, pos, player) in joins {
        let net = net.unwrap_or_else(|| new_network(lv));
        let text = st::network_text(net);
        let _ = set_network(lv, dim, pos, &[text.as_str()], Some(player));
    }
    let programmed: Vec<_> = PROGRAMMED.with_borrow_mut(|p| {
        let (mine, rest): (Vec<_>, Vec<_>) =
            std::mem::take(p).into_iter().partition(|x| x.0 == dim);
        *p = rest;
        mine
    });
    for (_, pos, r) in programmed {
        with(dim, |n| {
            if let Some(q) = n.requesters.get_mut(&pos) {
                q.r.order = r.order;
                q.r.address = r.address;
                n.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
        });
    }
    if !present() {
        return;
    }
    let mut touched = Vec::new();
    for pos in with(dim, |n| std::mem::take(&mut n.neighbours)) {
        match block_at(lv, pos) {
            // `PackagerLinkBlock.neighborChanged`: `powered` follows the signal;
            // `redstonePowerChanged`.
            Some(Block::StockLink) => {
                let power = link_power(lv, pos);
                let state = lv.block(pos);
                let was = with_world(|w| w.table.get(state).map(|(_, i)| i.powered)).flatten();
                if was.is_some_and(|w| w != (power > 0)) {
                    lv.set_property(
                        pos,
                        "powered",
                        if power > 0 { "true" } else { "false" },
                        UPDATE_CLIENTS,
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
                let signal = lv.has_neighbor_signal(pos);
                let state = lv.block(pos);
                let was = with_world(|w| w.table.get(state).map(|(_, i)| i.powered)).flatten();
                if was.is_some_and(|w| w != signal) {
                    lv.set_property(
                        pos,
                        "powered",
                        if signal { "true" } else { "false" },
                        update_all(),
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
                        trigger_requester(lv, dim, pos);
                    }
                    if with(dim, |n| n.requesters.get(&pos).map(|r| r.r.last_succeeded))
                        != Some(lit)
                    {
                        touched.push(pos);
                    }
                }
            }
            // `FactoryPanelBlockEntity.lazyTick`: a restocker while a packager is behind it.
            Some(Block::FactoryGauge) => {
                let restocker = restocked(lv, pos).is_some();
                with(dim, |n| {
                    if let Some(g) = n.gauges.get_mut(&pos) {
                        g.restocker = restocker;
                    }
                });
            }
            _ => {}
        }
    }
    let now = now(lv);
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
            look_gauge(lv, dim, pos, now);
        }
        with(dim, |n| {
            n.next = n.due.values().copied().min().unwrap_or(u64::MAX)
        });
    }
    // Comparators beside requesters whose last request changed read them again.
    for pos in touched {
        if let Some(state) = lv.block_state(pos) {
            let _ = lv.level.update_neighbors(
                &mut *lv.host,
                pos,
                state,
                mod_api::NeighbourUpdate::OutputSignal,
            );
        }
    }
    let unsaved = with(dim, |n| {
        let mut u = std::mem::take(&mut n.unsaved);
        u.sort_unstable();
        u.dedup();
        u
    });
    for (cx, cz) in unsaved {
        persist::mark_unsaved(dim, [cx, cz]);
    }
}

/// A packager's stock (`getAvailableItems`), and what arrived since it was last summed handed
/// to the promises waiting for it (`submitNewArrivals`: restocking gauges on the packager and
/// its link's network).
fn packager_stock(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    packager: [i32; 3],
) -> (Summary<Key>, Option<[i32; 3]>) {
    let (s, id) = packages::available(lv, dim, packager);
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
        match connected(lv, at) {
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
                    let before = p.list.clone();
                    for (k, a) in &arrived {
                        p.arrived(k, *a);
                    }
                    if p.list != before {
                        mark_dirty();
                    }
                }
            }),
            _ => {}
        }
    }
    (s, id)
}

/// `triggerStockCheck`.
pub fn stock_check(lv: &mut ServerLevel<'_>, dim: usize, packager: [i32; 3]) {
    if present() {
        packager_stock(lv, dim, packager);
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

/// `LogisticsManager.getSummaryOfNetwork`: each inventory once, from Create's caches
/// (`accurate`: summed again each tick; else every 20).
fn network_stock(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    net: Network,
    accurate: bool,
) -> Summary<Key> {
    let now = now(lv);
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
        let Some(packager) = link_packager(lv, dim, pos) else {
            continue;
        };
        let (s, id) = packager_stock(lv, dim, packager);
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
    lv: &mut ServerLevel<'_>,
    dim: usize,
    net: Network,
    ignored: Option<[i32; 3]>,
) -> Vec<Source<Key>> {
    let mut links = links_of(dim, net);
    links.sort_by_key(|l| l.1.power);
    links
        .into_iter()
        .map(|(pos, _)| {
            let packager = link_packager(lv, dim, pos);
            let (stock, inventory) = match packager {
                Some(p) => packager_stock(lv, dim, p),
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
    lv: &mut ServerLevel<'_>,
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
    let srcs = sources(lv, dim, net, ignored);
    let time = lv.game_time();
    let order_id = next_int(time);
    let requests = st::find_packagers(&srcs, order, address, order_id, |n| {
        (next_int(time) as u32 % n.max(1) as u32) as usize
    });
    let grouped = st::by_packager(requests);
    if grouped
        .iter()
        .any(|(p, _)| packages::too_busy(dim, *p, kind))
    {
        return false;
    }
    perform(lv, dim, grouped);
    true
}

/// `performPackageRequests`: each packager packs its requests; what went into boxes leaves the
/// network's accurate stock (`deductFromAccurateSummary`); the packagers' stock is looked at
/// again (`triggerStockCheck`).
fn perform(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    grouped: Vec<([i32; 3], std::collections::VecDeque<st::Request<Key>>)>,
) {
    for (p, mut queue) in grouped {
        let boxed = packages::perform_requests(lv, dim, p, &mut queue);
        let net =
            link_of(lv, p).and_then(|l| with(dim, |n| n.links.get(&l).and_then(|l| l.network)));
        if let Some(net) = net {
            with(dim, |n| {
                if let Some((_, s)) = n.accurate.get_mut(&net) {
                    for b in &boxed {
                        let k = Key::of(b);
                        let take = s.count_of(&k).min(b.count() as i32);
                        s.add(&k, -take);
                    }
                }
            });
        }
        packager_stock(lv, dim, p);
    }
}

/// `RedstoneRequesterBlockEntity.triggerRequest`.
fn trigger_requester(lv: &mut ServerLevel<'_>, dim: usize, pos: [i32; 3]) {
    let Some(net) = with(dim, |n| n.requesters.get(&pos).map(|r| r.network)) else {
        return;
    };
    let stock = net.map_or_else(Summary::default, |net| network_stock(lv, dim, net, true));
    let Some((trigger, order, address)) = with(dim, |n| {
        let r = n.requesters.get_mut(&pos)?;
        Some((r.r.trigger(&stock), r.r.order.clone(), r.r.address.clone()))
    }) else {
        return;
    };
    if let (Trigger::Send { .. }, Some(net)) = (&trigger, net) {
        broadcast(lv, dim, net, RequestType::Redstone, &order, None, &address);
    }
}

/// A gauge looked at (`tickStorageMonitor`, `tickRequests`).
fn look_gauge(lv: &mut ServerLevel<'_>, dim: usize, pos: [i32; 3], now: u64) {
    let Some(mut panel) = with(dim, |n| n.gauges.get(&pos).cloned()) else {
        return;
    };
    let Some(item) = panel.filter.clone() else {
        return;
    };
    let max = body::stack_limit(&*lv.host, &item.0);
    let packager = if panel.restocker {
        restocked(lv, pos)
    } else {
        None
    };
    let unloaded = if panel.restocker {
        u32::from(packager.is_none())
    } else {
        with(dim, |n| n.unloaded.get(&panel.network).map_or(0, Vec::len)) as u32
    };
    let in_storage = if panel.restocker {
        packager.map_or(0, |p| packager_stock(lv, dim, p).0.count_of(&item))
    } else {
        network_stock(lv, dim, panel.network, false).count_of(&item)
    };
    let expiry = panel.promise_expiry();
    let promised = if panel.restocker {
        panel.restocker_promises.total(&item, expiry, now)
    } else {
        with(dim, |n| {
            n.promises.get_mut(&panel.network).map_or(0, |p| {
                let before = p.list.len();
                let total = p.total(&item, expiry, now);
                if p.list.len() != before {
                    mark_dirty();
                }
                total
            })
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
            let Some(inv) = packages::available(lv, dim, p).1 else {
                return;
            };
            let available: i32 = sources(lv, dim, net, Some(inv))
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
                lv,
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
        GaugeStep::Request => request_inputs(lv, dim, &panel, now),
    }
}

/// `tickRequests` of a gauge fed by others: what each asks for, from its network, all or none.
fn request_inputs(lv: &mut ServerLevel<'_>, dim: usize, panel: &Panel<Key>, now: u64) {
    let groups = with(dim, |n| {
        st::consolidate(&panel.inputs, |p| {
            n.gauges.get(&p).map(|g| (g.network, g.filter.clone()))
        })
    });
    let Some(groups) = groups else { return };
    let mut failed = false;
    let mut wanted: Vec<(Network, Vec<(Key, u32)>)> = Vec::new();
    for (net, items) in groups {
        let stock = network_stock(lv, dim, net, true);
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
        let srcs = sources(lv, dim, net, None);
        let time = lv.game_time();
        let order_id = next_int(time);
        let r = st::find_packagers(&srcs, &order, &panel.recipe_address, order_id, |n| {
            (next_int(time) as u32 % n.max(1) as u32) as usize
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
        perform(lv, dim, g);
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
        mark_dirty();
    }
}

/// `getAnalogOutputSignal` of a redstone requester: whether its last request went out. None
/// for anything else.
fn requester_succeeded(dim: usize, pos: [i32; 3]) -> Option<bool> {
    STOCK.with(|s| {
        let s = s.try_borrow().ok()?;
        Some(s.as_ref()?.get(dim)?.requesters.get(&pos)?.r.last_succeeded)
    })
}

/// Links', gauges' and requesters' own moments.
struct StockBlock;

#[mod_api::horizon_part]
impl BlockPart for StockBlock {
    /// A right click on a stock link or a gauge does nothing here (their screens are
    /// commands): the click is used, so the block's template does nothing either. A linking
    /// item goes on to its own use (tuning it to the block's network).
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let linking = event.click.stack.is_some_and(|s| {
            matches!(
                s.item().id(&*host).as_str(),
                "create:stock_link"
                    | "create:stock_ticker"
                    | "create:factory_gauge"
                    | "create:redstone_requester"
            )
        });
        if linking {
            // `LogisticallyLinkedBlockItem.useOn` (the block has no use of its own).
            let player = host.player(event.click.player)?;
            if player.entity().is_shift_key_down(&*host) == Some(true) {
                return None;
            }
            let stack = event.click.stack?.clone();
            tune(host, player, event.click.hand, stack, event.click.pos);
            return Some(InteractionResult::Success);
        }
        own_use(host, event.state, event.click)
    }

    fn use_without_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::UseWithoutItemEvent<'_>,
    ) -> Option<InteractionResult> {
        own_use(host, event.state, event.click)
    }

    fn has_analog_output(&self, _level: &dyn LevelHost, event: &mut HasAnalogOutputEvent) {
        if states::id(event.state).and_then(block_of) == Some(Block::RedstoneRequester) {
            event.output = true;
        }
    }

    fn analog_output(&self, _level: &dyn LevelHost, event: &mut AnalogOutputEvent) {
        if states::id(event.state).and_then(block_of) != Some(Block::RedstoneRequester) {
            return;
        }
        let Some(dim) = level_index(event.dimension.key().id()) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        if let Some(lit) = requester_succeeded(dim, pos) {
            event.output = if lit { 15 } else { 0 };
        }
    }
}

/// A click on a link, gauge, requester or ticker, after a linking item's: a requester opens its
/// screen (`RedstoneRequesterBlockEntity.use`); a ticker without a keeper says so
/// (`StockTickerBlock.useItemOn`; with one, Create's category editor would open: not ported);
/// links and gauges take the click (their screens are not ported).
fn own_use(
    host: &mut dyn ServerHost,
    state: mod_api::BlockState,
    click: &mod_api::BlockClick<'_>,
) -> Option<InteractionResult> {
    let b = block_of(states::id(state)?)?;
    let player = host.player(click.player)?;
    if b == Block::RedstoneRequester {
        if player.entity().is_shift_key_down(&*host) == Some(true) {
            return None;
        }
        let dimension = player.dimension(&*host)?.to_string();
        let mut lv = ServerLevel::new(host, &dimension)?;
        requester::open(&mut lv, player, click.pos);
        return Some(InteractionResult::Success);
    }
    if b == Block::StockTicker {
        let dimension = player.dimension(&*host)?.to_string();
        let lv = ServerLevel::new(host, &dimension)?;
        if !keeper::keeper_present(&lv, click.pos) {
            let line = mod_api::Text::translate("create.stock_ticker.keeper_missing", Vec::new());
            let _ = player.action_bar(lv.host, line);
        }
        return Some(InteractionResult::Success);
    }
    matches!(b, Block::StockLink | Block::FactoryGauge).then_some(InteractionResult::Success)
}

/// The items placing links, tickers, gauges and requesters (`LogisticallyLinkedBlockItem`):
/// their placement (`getStateForPlacement`), joining the network the item was tuned to (or a
/// new one); a click on a linked block tunes the item to its network, a click in the air clears
/// it. The network is kept in the stack's custom data (`Freq`), which also makes it glint.
struct StockItem;

/// The key of a tuned item's network in its custom data.
const FREQ: &str = "Freq";

type Join = (usize, Option<Network>, [i32; 3], u128);

thread_local! {
    /// Blocks placed from a linking item, joined at their level's next tick.
    static JOINS: RefCell<Vec<Join>> = const { RefCell::new(Vec::new()) };
}

/// The network a tuned stack is tuned to.
fn tuned(stack: &ItemStack) -> Option<Network> {
    match super::items::custom(stack).get(&FREQ.into()) {
        Some(Tag::String(s)) => Some(st::network_id(&s.to_string_lossy())),
        _ => None,
    }
}

/// `UUID.randomUUID()` for `new_network_started`, from the level's random.
fn new_network(lv: &mut ServerLevel<'_>) -> Network {
    use mod_api::RandomSource;
    let half = |lv: &mut ServerLevel<'_>| {
        lv.level
            .with_random(&mut *lv.host, |r| r.next_long())
            .unwrap_or(0) as u64
    };
    (u128::from(half(lv)) << 64) | u128::from(half(lv))
}

/// The key of a programmed requester's order in its item's custom data (Create's
/// `AUTO_REQUEST_DATA` component: the order and its address).
const AUTO_REQUEST: &str = "AutoRequest";

/// `RedstoneRequesterBlock.programRequester`: the redstone requester in `player`'s main hand
/// keeps `order` and `address`, and is tuned to the ticker's network (its `Freq`).
pub(super) fn program_requester(
    lv: &mut ServerLevel<'_>,
    player: mod_api::PlayerRef,
    order: &Order<Key>,
    address: &str,
) {
    let Some(mut stack) = super::items::held(&*lv.host, player, mod_api::Hand::Main) else {
        return;
    };
    if stack.item().id(&*lv.host) != "create:redstone_requester" {
        return;
    }
    let data = Tag::compound([
        (
            "EncodedRequest",
            body::order_nbt(&*lv.host, &order_stack(order)),
        ),
        ("EncodedAddress", Tag::String(address.into())),
    ]);
    let net = OPENED.get().and_then(|pos| network_at(lv.dim, pos));
    let mut entries = vec![(AUTO_REQUEST, Some(data))];
    let freq = net.map(|n| Tag::String(st::network_text(n).as_str().into()));
    if freq.is_some() {
        entries.push((FREQ, freq));
    }
    super::items::set_custom(&mut stack, &entries);
    if net.is_some() {
        let _ = stack.set(&Glint, true);
    }
    super::items::set_held(lv.host, player, mod_api::Hand::Main, Some(stack));
}

/// A requester placed from a programmed item: its level, position and order.
type Programmed = (usize, [i32; 3], Requester<Key>);

thread_local! {
    /// The ticker whose screen programs a requester (set while its order is handled).
    static OPENED: Cell<Option<[i32; 3]>> = const { Cell::new(None) };
    /// Requesters placed from a programmed item, set up at their level's next tick.
    static PROGRAMMED: RefCell<Vec<Programmed>> = const { RefCell::new(Vec::new()) };
}

/// The ticker `keeper::order` acts for, while `f` runs.
pub(super) fn for_ticker<R>(pos: [i32; 3], f: impl FnOnce() -> R) -> R {
    OPENED.set(Some(pos));
    let r = f();
    OPENED.set(None);
    r
}

/// `AutoRequestData.readFromItem`: the order a programmed requester item carries.
fn programmed(host: &dyn mod_api::Host, stack: &ItemStack) -> Option<Requester<Key>> {
    let Some(Tag::Compound(c)) = super::items::custom(stack)
        .get(&AUTO_REQUEST.into())
        .cloned()
    else {
        return None;
    };
    let order = c
        .get(&"EncodedRequest".into())
        .and_then(|t| body::order_from_nbt(host, t))
        .map(|o| order_key(&o))?;
    Some(Requester {
        order,
        address: text(c.get(&"EncodedAddress".into())),
        ..Requester::default()
    })
}

/// `assignFrequency`: the held linking item tuned to the network of the block at `pos`.
fn tune(
    host: &mut dyn ServerHost,
    player: mod_api::PlayerRef,
    hand: mod_api::Hand,
    mut stack: ItemStack,
    pos: [i32; 3],
) -> Option<()> {
    let dimension = player.dimension(&*host)?.to_string();
    let dim = level_index(&dimension)?;
    let net = network_at(dim, pos)?;
    super::items::set_custom(
        &mut stack,
        &[(
            FREQ,
            Some(Tag::String(st::network_text(net).as_str().into())),
        )],
    );
    let _ = stack.set(&Glint, true);
    super::items::set_held(host, player, hand, Some(stack));
    let _ = player.action_bar(
        host,
        mod_api::Text::translate("create.logistically_linked.tuned", Vec::new()),
    );
    Some(())
}

fn linked_block(b: Block) -> bool {
    matches!(
        b,
        Block::StockLink | Block::StockTicker | Block::RedstoneRequester | Block::FactoryGauge
    )
}

#[mod_api::horizon_part]
impl ItemPart for StockItem {
    fn on_block_placed_by(&self, host: &mut dyn ServerHost, event: &mut OnBlockPlacedByEvent<'_>) {
        let Some(player) = event.placer else {
            return;
        };
        let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
            return;
        };
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        place_block(&mut lv, event.pos);
        // `RedstoneRequesterBlock.setPlacedBy`.
        if let Some(r) = programmed(&*lv.host, event.stack) {
            PROGRAMMED.with_borrow_mut(|p| p.push((lv.dim, event.pos, r)));
        }
        let net = tuned(event.stack);
        // `useOn`: `connected`, or `new_network_started`.
        let key = if net.is_some() {
            "create.logistically_linked.connected"
        } else {
            "create.logistically_linked.new_network_started"
        };
        let _ = player.action_bar(&mut *lv.host, mod_api::Text::translate(key, Vec::new()));
        JOINS.with_borrow_mut(|j| j.push((lv.dim, net, event.pos, player.uuid())));
    }

    /// `LogisticallyLinkedBlockItem.useOn` on a linked block, not sneaking: tuned to its network
    /// (`assignFrequency`).
    fn use_on(
        &self,
        host: mod_api::HostSide,
        event: &mut mod_api::UseOnEvent<'_>,
    ) -> Option<InteractionResult> {
        match host {
            mod_api::HostSide::Client(host) => {
                let player = host.player(event.player)?;
                if player.entity().is_shift_key_down(&*host) == Some(true) {
                    return None;
                }
                let state = host
                    .level(host.dimension())?
                    .block_state(&*host, event.pos)?;
                let b = states::id(state).and_then(block_of)?;
                linked_block(b).then_some(InteractionResult::Success)
            }
            mod_api::HostSide::Server(host) => {
                let player = host.player(event.player)?;
                if player.entity().is_shift_key_down(&*host) == Some(true) {
                    return None;
                }
                let dimension = player.dimension(&*host)?.to_string();
                let lv = ServerLevel::new(host, &dimension)?;
                if !linked_block(block_at(&lv, event.pos)?) {
                    return None;
                }
                tune(host, player, event.hand, event.stack.clone(), event.pos);
                Some(InteractionResult::Success)
            }
        }
    }

    /// `LogisticallyLinkedBlockItem.use`: a tuned item used in the air forgets its network.
    fn use_item(
        &self,
        host: mod_api::HostSide,
        event: &mut mod_api::UseItemEvent<'_>,
    ) -> Option<InteractionResult> {
        tuned(event.stack)?;
        if let mod_api::HostSide::Server(host) = host
            && let Some(player) = host.player(event.player)
        {
            let mut stack = event.stack.clone();
            super::items::set_custom(&mut stack, &[(FREQ, None)]);
            stack.remove(&Glint);
            super::items::set_held(host, player, event.hand, Some(stack));
            let _ = player.action_bar(
                host,
                mod_api::Text::translate("create.logistically_linked.cleared", Vec::new()),
            );
        }
        Some(InteractionResult::Success)
    }

    /// `appendHoverText` of a tuned item.
    fn tooltip(&self, _host: &dyn mod_api::ClientHost, event: &mut mod_api::TooltipEvent<'_>) {
        if tuned(event.stack).is_some() {
            let line = |key: &str| mod_api::Text::translate(key, Vec::new());
            event
                .lines
                .push((line("create.logistically_linked.tooltip"), 0xFFAA00));
            event
                .lines
                .push((line("create.logistically_linked.tooltip_clear"), 0xAAAAAA));
        }
    }
}

/// `minecraft:enchantment_glint_override`: a tuned item glints (`isFoil`).
struct Glint;

impl mod_api::Component for Glint {
    type Value = bool;
    fn id(&self) -> &str {
        "minecraft:enchantment_glint_override"
    }
    fn decode(&self, value: &Tag) -> Option<bool> {
        match value {
            Tag::Byte(b) => Some(*b != 0),
            _ => None,
        }
    }
    fn encode(&self, value: bool) -> Tag {
        Tag::Byte(i8::from(value))
    }
}

/// An item from a command (`minecraft:` when it names none).
fn item_of(lv: &ServerLevel<'_>, word: &str) -> Option<Key> {
    let id = if word.contains(':') {
        word.to_owned()
    } else {
        format!("minecraft:{word}")
    };
    body::new_stack(&*lv.host, &id, 1).map(Key)
}

/// `<item> <count>` pairs.
fn stacks_of(lv: &ServerLevel<'_>, words: &[&str]) -> Result<Vec<(Key, u32)>, String> {
    if !words.len().is_multiple_of(2) {
        return Err("expected <item> <count> pairs".into());
    }
    words
        .chunks(2)
        .map(|w| {
            let item = item_of(lv, w[0]).ok_or_else(|| format!("no item {}", w[0]))?;
            let n = w[1]
                .parse::<u32>()
                .map_err(|_| format!("not a count: {}", w[1]))?;
            Ok((item, n))
        })
        .collect()
}

/// The linking item's click (`LogisticallyLinkedBlockItem`): the block joins network `id`.
fn set_network(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    pos: [i32; 3],
    args: &[&str],
    player: Option<u128>,
) -> Result<String, String> {
    let text = args.join(" ");
    if text.is_empty() {
        return Err("usage: create_debug network <x y z> <id>".into());
    }
    let net = st::network_id(&text);
    let block = block_at(lv, pos);
    // `linkAdded`: a link joining a network; the first player to link one owns it.
    if block == Some(Block::StockLink) && with(dim, |n| n.links.contains_key(&pos)) {
        if let Some(p) = player {
            OWNERS.with_borrow_mut(|o| {
                let o = o.entry(net).or_default();
                if o.owner.is_none() {
                    o.owner = Some(p);
                }
            });
        }
        mark_dirty();
    }
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

/// The ticker's screen (`LogisticalStockRequestPacket`: `getRecentSummary`), or a gauge's state.
fn describe(lv: &mut ServerLevel<'_>, dim: usize, pos: [i32; 3]) -> String {
    let host: &dyn mod_api::Host = &*lv.host;
    let id = |k: &Key| body::item_id(host, &k.0);
    if let Some(g) = with(dim, |n| n.gauges.get(&pos).cloned()) {
        let inputs: Vec<String> = g.inputs.iter().map(|(p, a)| format!("{p:?}x{a}")).collect();
        return format!(
            "gauge {} x{}{} address=\"{}\" network={} restocker={} satisfied={} promised_satisfied={} waiting={} level={} promised={} inputs=[{}] output={}",
            g.filter.as_ref().map_or("-".to_owned(), id),
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
                .map(|(k, n)| format!("{n} {}", id(k)))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let Some(net) = network_at(dim, pos) else {
        return "no network there".into();
    };
    let s = network_stock(lv, dim, net, false);
    let host: &dyn mod_api::Host = &*lv.host;
    let line = s
        .by_count()
        .iter()
        .map(|(k, n)| format!("{n} {}", body::item_id(host, &k.0)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "stock of {} from {} links: {line}",
        st::network_text(net),
        s.contributing_links,
    )
}

/// The ticker's order (`PackageOrderRequestPacket`): `<address> <item> <count>...`.
fn order(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    pos: [i32; 3],
    args: &[&str],
) -> Result<String, String> {
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
    let order = Order::simple(stacks_of(lv, items)?);
    let sent = broadcast(lv, dim, net, RequestType::Player, &order, None, address);
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
    lv: &ServerLevel<'_>,
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
    let stacks = stacks_of(lv, items)?;
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
    lv: &ServerLevel<'_>,
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
        Some(item_of(lv, item).ok_or_else(|| format!("no item {item}"))?)
    };
    let count: u32 = count.parse().map_err(|_| usage.to_owned())?;
    let up_to = match *mode {
        "items" => true,
        "stacks" => false,
        _ => return Err(usage.into()),
    };
    let address = address.join(" ");
    let now = now(lv);
    let restocker = restocked(lv, pos).is_some();
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
fn connect(dim: usize, from: [i32; 3], args: &[&str], here: [f64; 3]) -> Result<String, String> {
    let usage = "usage: create_debug gauge_connect <from x y z> <to x y z> [amount]";
    let to = coords(args, Some(here)).ok_or_else(|| usage.to_owned())?;
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
        if let Some(f) = n.gauges.get_mut(&from)
            && !f.outputs.contains(&to)
        {
            f.outputs.push(to);
        }
        n.unsaved.push((to[0] >> 4, to[2] >> 4));
        n.unsaved.push((from[0] >> 4, from[2] >> 4));
        Ok(format!("gauge {from:?} feeds {to:?} x{amount}"))
    })
}

/// The gauge's recipe: `<output count> [item...]` (`recipeOutput`, `activeCraftingArrangement`).
fn set_craft(
    lv: &ServerLevel<'_>,
    dim: usize,
    pos: [i32; 3],
    args: &[&str],
) -> Result<String, String> {
    let usage = "usage: create_debug gauge_craft <x y z> <output count> [item...]";
    let Some((count, items)) = args.split_first() else {
        return Err(usage.into());
    };
    let output: u32 = count.parse().map_err(|_| usage.to_owned())?;
    if items.len() > create_engine::package::SLOTS {
        return Err("nine items at most".into());
    }
    let craft = items
        .iter()
        .map(|w| item_of(lv, w).ok_or_else(|| format!("no item {w}")))
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

const KEY: &str = "create_stock";

fn freq(net: Option<Network>) -> Tag {
    Tag::IntArray(net.map_or_else(Vec::new, |n| st::network_ints(n).to_vec()))
}

/// Links, tickers, requesters and gauges of a column saved (their block entities' `write`).
fn chunk_data_save(lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
    if !present() {
        return;
    }
    let host: &dyn mod_api::Host = &*lv.host;
    let now = now(lv);
    let inside = |p: &[i32; 3]| [p[0] >> 4, p[2] >> 4] == chunk;
    let pos_tag = |p: [i32; 3]| Tag::IntArray(p.to_vec());
    let lists = with(lv.dim, |n| {
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
                        (
                            "EncodedRequest",
                            body::order_nbt(host, &order_stack(&r.r.order)),
                        ),
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
                        Tag::List(g.craft.iter().map(|k| body::to_nbt(host, &k.0)).collect()),
                    ),
                ];
                if let Some(f) = &g.filter {
                    c.push(("Filter", body::to_nbt(host, &f.0)));
                }
                if !g.restocker_promises.is_empty() {
                    // `RequestPromise`: the stack, its count and age.
                    let list = g
                        .restocker_promises
                        .list
                        .iter()
                        .map(|(k, n, at)| {
                            Tag::compound([
                                ("Item", body::to_nbt(host, &k.0)),
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
fn chunk_load(lv: &ServerLevel<'_>, data: &Compound) {
    let host: &dyn mod_api::Host = &*lv.host;
    let now = now(lv);
    let net_of = |c: &Compound| st::network_from_ints(&ints(c.get(&"Freq".into())));
    with(lv.dim, |n| {
        for c in list(data, "links") {
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
        for c in list(data, "tickers") {
            let Some(pos) = pos_of(&c) else { continue };
            n.tickers.insert(
                pos,
                Ticker {
                    network: net_of(&c),
                    previous: text(c.get(&"PreviousAddress".into())),
                },
            );
        }
        for c in list(data, "requesters") {
            let Some(pos) = pos_of(&c) else { continue };
            let s = ints(c.get(&"State".into()));
            let flag = |i: usize| s.get(i).is_some_and(|v| *v != 0);
            let order = c
                .get(&"EncodedRequest".into())
                .and_then(|t| body::order_from_nbt(host, t))
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
        for c in list(data, "gauges") {
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
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| ([v[0], v[1], v[2]], v[3].max(0) as u32))
                .collect();
            g.outputs = ints(c.get(&"Targeting".into()))
                .as_chunks::<3>()
                .0
                .iter()
                .map(|v| [v[0], v[1], v[2]])
                .collect();
            if let Some(Tag::List(l)) = c.get(&"Craft".into()) {
                g.craft = l
                    .iter()
                    .filter_map(|t| body::from_nbt(host, t))
                    .map(|s| Key::of(&s))
                    .collect();
            }
            g.filter = c
                .get(&"Filter".into())
                .and_then(|t| body::from_nbt(host, t))
                .map(|s| Key::of(&s));
            if let Some(Tag::List(l)) = c.get(&"Promises".into()) {
                for t in l {
                    let Tag::Compound(e) = t else { continue };
                    let Some(item) = e.get(&"Item".into()).and_then(|t| body::from_nbt(host, t))
                    else {
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
fn chunk_unload(dim: usize, chunk: [i32; 2]) {
    if !present() {
        return;
    }
    let inside = |p: &[i32; 3]| [p[0] >> 4, p[2] >> 4] == chunk;
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

fn uuid_tag(id: u128) -> Tag {
    Tag::IntArray(st::network_ints(id).to_vec())
}

/// `GlobalLogisticsManager.levelLoaded`: the networks back from the overworld's saved data.
/// Their links count as unloaded until their columns load (a link already loaded is not).
fn level_load(lv: &mut ServerLevel<'_>) {
    let now = now(lv);
    let data = match NETWORKS.get_existing(&mut *lv.host, Holder::Level) {
        Ok(Some(data)) => data,
        Ok(None) => return,
        Err(e) => {
            crate::create_log!("[create] the logistics networks of {}: {e:?}", lv.dimension);
            return;
        }
    };
    let host: &dyn mod_api::Host = &*lv.host;
    let mut saved: Vec<SavedNetwork<Key>> = Vec::new();
    for c in list(&data, "LogisticsNetworks") {
        let Some(id) = st::network_from_ints(&ints(c.get(&"Id".into()))) else {
            continue;
        };
        let links = list(&c, "Links")
            .iter()
            .filter_map(|l| {
                let p = ints(l.get(&"Pos".into()));
                let pos: [i32; 3] = p.as_slice().try_into().ok()?;
                let dim = match l.get(&"Dim".into()) {
                    Some(Tag::String(d)) => level_index(&d.to_string_lossy())?,
                    _ => 0,
                };
                Some((dim, pos))
            })
            .collect();
        let promises = match c.get(&"Promises".into()) {
            Some(Tag::Compound(q)) => list(q, "List"),
            _ => Vec::new(),
        };
        let promises = promises
            .iter()
            .filter_map(|e| {
                let age = ints(e.get(&"ticks_existed".into())).first().copied()?;
                let Some(Tag::Compound(big)) = e.get(&"promised_stack".into()) else {
                    return None;
                };
                let stack = big
                    .get(&"item_stack".into())
                    .and_then(|t| body::from_nbt(host, t))?;
                let count = ints(big.get(&"count".into())).first().copied()?;
                Some((Key::of(&stack), count.max(0) as u32, age.max(0) as u64))
            })
            .collect();
        let ownership = Ownership {
            owner: st::network_from_ints(&ints(c.get(&"Owner".into()))),
            locked: ints(c.get(&"Locked".into())).first() == Some(&1),
        };
        saved.push(SavedNetwork {
            id,
            links,
            promises,
            ownership,
        });
    }
    let levels = saved
        .iter()
        .flat_map(|n| n.links.iter().map(|l| l.0 + 1))
        .max()
        .unwrap_or(1)
        .max(3);
    let (levels, owners) = st::load_networks(&saved, levels, now);
    for (dim, level) in levels.into_iter().enumerate() {
        with(dim, |n| {
            for (pos, net) in level.links {
                if !n.links.contains_key(&pos) {
                    n.unloaded.entry(net).or_default().push(pos);
                }
            }
            n.promises.extend(level.promises);
        });
    }
    OWNERS.with_borrow_mut(|o| *o = owners);
}

/// The networks as `LogisticsNetworkSavedData.save` writes them, every level's links (loaded
/// or not) and promises.
fn networks_tag(host: &dyn mod_api::Host, now: u64) -> Tag {
    let count = STOCK.with_borrow(|s| s.as_ref().map_or(0, Vec::len));
    let levels: Vec<LevelNetworks<Key>> = (0..count)
        .map(|dim| {
            with(dim, |n| {
                let mut links: Vec<([i32; 3], Network)> = n
                    .links
                    .iter()
                    .filter_map(|(p, l)| Some((*p, l.network?)))
                    .collect();
                for (net, list) in &n.unloaded {
                    links.extend(list.iter().map(|p| (*p, *net)));
                }
                let promises = n.promises.iter().map(|(k, p)| (*k, p.clone())).collect();
                LevelNetworks { links, promises }
            })
        })
        .collect();
    let owners = OWNERS.with_borrow(Clone::clone);
    let list = st::save_networks(&levels, &owners, now)
        .into_iter()
        .map(|n| {
            let links = n
                .links
                .iter()
                .map(|(dim, pos)| {
                    let mut c = vec![("Pos", Tag::IntArray(pos.to_vec()))];
                    if *dim != 0
                        && let Some(name) = level_id(*dim)
                    {
                        c.push(("Dim", Tag::String(name.as_str().into())));
                    }
                    Tag::compound(c)
                })
                .collect();
            let promises = n
                .promises
                .iter()
                .map(|(k, count, age)| {
                    Tag::compound([
                        ("ticks_existed", Tag::IntArray(vec![*age as i32])),
                        (
                            "promised_stack",
                            Tag::compound([
                                ("item_stack", body::to_nbt(host, &k.0)),
                                ("count", Tag::IntArray(vec![*count as i32])),
                            ]),
                        ),
                    ])
                })
                .collect();
            let mut c = vec![
                ("Id", uuid_tag(n.id)),
                ("Promises", Tag::compound([("List", Tag::List(promises))])),
                ("Links", Tag::List(links)),
                ("Locked", Tag::IntArray(vec![i32::from(n.ownership.locked)])),
            ];
            if let Some(owner) = n.ownership.owner {
                c.push(("Owner", uuid_tag(owner)));
            }
            Tag::compound(c)
        })
        .collect();
    Tag::List(list)
}

/// The networks into the overworld's saved data, when they changed (`setDirty`).
fn level_save(lv: &mut ServerLevel<'_>) {
    let now = now(lv);
    let networks = networks_tag(&*lv.host, now);
    let data = Compound::from([("LogisticsNetworks".into(), networks)]);
    match NETWORKS.set(&mut *lv.host, Holder::Level, data) {
        Ok(()) => DIRTY.set(false),
        Err(e) => crate::create_log!("[create] saving the logistics networks: {e:?}"),
    }
}
