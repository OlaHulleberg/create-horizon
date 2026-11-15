//! The stock keeper's request screen (`StockKeeperRequestScreen`): a mob seated beside a stock
//! ticker, or a blaze burner beside it, is clicked (`StockTickerInteractionHandler`) and the
//! screen lists what the ticker's network holds (`getRecentSummary`, `LogisticalStockResponse`),
//! asked again while open (`refreshClientStockSnapshot`, every 16 ticks). The stock is grouped
//! by the ticker's categories (`receiveStockPacket`; each player folds them, `hiddenCategories`,
//! kept by the ticker), searched by name or id (`@` a mod's, `#` a tag's), and scrolled by the
//! wheel or its bar. Clicks put up to nine stacks on the order (shift a stack, control ten; right
//! click and clicks on the order take off; the wheel one), the address box names where they go
//! (remembered by the ticker, suggested from `#` entries on clipboards in the inventory), and
//! Send or shift-Enter (`PackageOrderRequestPacket`) broadcasts the order. The network's owner
//! may lock it (`StockKeeperLockPacket`); a locked network's keeper serves its owner only.
//! Holding a redstone requester, the screen programs it instead (`encodeRequester`: Configure);
//! holding a clipboard with items on it, it requests what of them is in stock
//! (`isSchematicListMode`). Holding a table cloth, it programs the cloth as a shop of what is
//! ordered (`shop`). The screen closes when its keeper is gone.
//!
//! The address box also offers the `#` addresses on clipboards placed within 32 blocks, which
//! the server tells each player of every second (`advertise_clipboards`).
//!
//! The seated keeper is drawn beside the window turned toward the cursor, and a blaze keeper in
//! its burner.
//!
//! Left out: crafting recipes and the search sync button, which come from JEI
//! (`StockKeeperTransferHandler`, `SearchSyncMode`), a mod the game does not have.

use super::super::net;
use super::super::server::ServerLevel;
use super::{Key, broadcast, network_at, network_stock, with};
use create_engine::Block;
use create_engine::stock::{INF, Order, RequestType};
use mod_api::text::TextStyle;
use mod_api::{
    ClientHost, DrawContext, Flow, Host, ItemStack, PlayerRef, ScreenCharTypedPreEvent,
    ScreenClosingEvent, ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent,
    ScreenMouseButtonReleasedPreEvent, ScreenMouseScrolledPreEvent, ScreenRenderPostEvent,
    ServerHost, SoundPlay, Tag, Text,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// Server to client: the screen's contents (`0` opens it, `1` adds stacks, `2` starts a fresh
/// list, `3` closes it); client to server: `R` asks for the stock again, `O` sends the order (an
/// empty one keeps the address), `H` the folded categories, `L` the lock.
pub(super) const CHANNEL: &str = "create:stock_keeper";
const MOD: &str = "create";
/// The order's slots (`cols`).
const COLS: usize = 9;
/// The unsorted category's index on the wire.
const UNSORTED: u8 = 255;

// ---------------------------------------------------------------------------------------------
// Wire

pub(crate) fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_str(out: &mut Vec<u8>, s: &str) {
    let mut end = s.len().min(255);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let b = &s.as_bytes()[..end];
    out.push(b.len() as u8);
    out.extend_from_slice(b);
}

/// A stack's item NBT (one of it) as network bytes.
fn stack_bytes(host: &dyn Host, stack: &ItemStack) -> Option<Vec<u8>> {
    let one = super::body::with_count(stack, 1)?;
    Tag::Compound(one.to_nbt(host)?).to_network_bytes().ok()
}

fn stack_from_bytes(host: &dyn Host, bytes: &[u8]) -> Option<ItemStack> {
    let (Tag::Compound(nbt), _) = Tag::from_network_bytes(bytes, None).ok()? else {
        return None;
    };
    ItemStack::from_nbt(host, &nbt)
}

pub(crate) fn put_stack(host: &dyn Host, out: &mut Vec<u8>, stack: &ItemStack, count: u32) -> bool {
    let Some(nbt) = stack_bytes(host, stack) else {
        return false;
    };
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&(nbt.len() as u16).to_le_bytes());
    out.extend_from_slice(&nbt);
    true
}

pub(crate) struct Reader<'a>(pub(crate) &'a [u8]);

impl Reader<'_> {
    pub(crate) fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    pub(crate) fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }
    pub(crate) fn i32(&mut self) -> Option<i32> {
        self.bytes(4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub(crate) fn str(&mut self) -> Option<String> {
        let n = usize::from(self.u8()?);
        String::from_utf8(self.bytes(n)?.to_vec()).ok()
    }
    pub(crate) fn pos(&mut self) -> Option<[i32; 3]> {
        Some([self.i32()?, self.i32()?, self.i32()?])
    }
    pub(crate) fn stack(&mut self, host: &dyn Host) -> Option<(ItemStack, u32)> {
        let count = self.i32()? as u32;
        let n = self.bytes(2).map(|b| u16::from_le_bytes([b[0], b[1]]))?;
        let nbt = self.bytes(usize::from(n))?;
        Some((stack_from_bytes(host, nbt)?, count))
    }
}

// ---------------------------------------------------------------------------------------------
// Server

/// `StockTickerBlockEntity.isKeeperPresent`: a seat with a rider beside the ticker or below that,
/// or a blaze burner beside it.
pub(super) fn keeper_present(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    keeper_entity(lv, pos).is_some()
        || [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dz)| {
            super::super::logistics::info_at(lv, [pos[0] + dx, pos[1], pos[2] + dz])
                .is_some_and(|(b, _)| b == Block::BlazeBurner)
        })
}

/// The screen's `stockKeeper` (`init`): the rider of a seat beside the ticker or below that, the
/// last one found, as Create's search keeps it; None when a blaze burner beside it is found
/// first.
fn keeper_entity(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<mod_api::EntityRef> {
    let mut found = None;
    for dy in [0, 1] {
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let p = [pos[0] + dx, pos[1] - dy, pos[2] + dz];
            let lo = p.map(f64::from);
            let hi = p.map(|v| f64::from(v) + 1.0);
            let seat = |e: mod_api::EntityRef| {
                e.tags(&*lv.host)
                    .is_some_and(|t| t.iter().any(|t| t == "create.seat"))
            };
            for e in lv.level.entities_in(&*lv.host, lo, hi).unwrap_or_default() {
                // The seat stand-in's rider, or a rider on one (a stand-in that came back
                // from a save is not listed, its rider is).
                let rider = if seat(e) {
                    e.passengers(&*lv.host).and_then(|p| p.first().copied())
                } else {
                    e.vehicle(&*lv.host).is_some_and(seat).then_some(e)
                };
                if rider.is_some() {
                    found = rider;
                }
            }
            if dy == 0
                && super::super::logistics::info_at(lv, p)
                    .is_some_and(|(b, _)| b == Block::BlazeBurner)
            {
                return found;
            }
        }
    }
    found
}

/// `getStockTickerPosition`: the one stock ticker beside a seated entity, or above that.
pub(super) fn ticker_beside(lv: &ServerLevel<'_>, entity: mod_api::EntityRef) -> Option<[i32; 3]> {
    let root = entity.root_vehicle(&*lv.host)?;
    if root == entity
        || !root
            .tags(&*lv.host)
            .is_some_and(|t| t.iter().any(|t| t == "create.seat"))
    {
        return None;
    }
    let at = entity.block_position(&*lv.host)?;
    let mut found = Vec::new();
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        for dy in [0, 1] {
            let p = [at[0] + dx, at[1] + dy, at[2] + dz];
            if super::super::logistics::info_at(lv, p).is_some_and(|(b, _)| b == Block::StockTicker)
            {
                found.push(p);
            }
        }
    }
    (found.len() == 1).then(|| found[0])
}

/// `interactWithLogisticsManagerAt`: a shopping list in the main hand is handed over
/// (`interactWithShop`); else the request screen opens.
pub(super) fn interact(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) {
    let held = super::super::items::held(&*lv.host, player, mod_api::Hand::Main);
    if held.is_some_and(|s| s.item().id(&*lv.host) == "create:shopping_list") {
        super::shop::checkout(lv, player, pos);
        return;
    }
    open(lv, player, pos);
}

/// `interactWithLogisticsManagerAt`: the ticker's request screen opens for `player` (a locked
/// network's for its owner only), with the address it last sent to, the player's folded
/// categories and its network's stock.
pub(super) fn open(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) {
    let dim = lv.dim;
    if !super::may_interact(dim, pos, player) {
        let line = Text::translate("create.stock_keeper.locked", Vec::new())
            .with_style(style("red", false));
        let _ = player.action_bar(lv.host, line);
        return;
    }
    let Some((previous, cats, hidden)) = with(dim, |n| {
        n.tickers.get(&pos).map(|t| {
            (
                t.previous.clone(),
                t.categories.clone(),
                t.hidden.get(&player.uuid()).cloned().unwrap_or_default(),
            )
        })
    }) else {
        return;
    };
    let held = super::super::items::held(&*lv.host, player, mod_api::Hand::Main);
    let held_id = held.as_ref().map(|s| s.item().id(&*lv.host));
    // `encodeRequester`: a redstone requester or a table cloth in hand is programmed.
    let encode = held_id.as_deref().is_some_and(|i| {
        i == "create:redstone_requester"
            || (i.starts_with("create:") && i.ends_with("_table_cloth"))
    });
    let (admin, lockable) = super::may_administrate(dim, pos, player);
    let locked = super::is_locked(dim, pos);
    let host: &dyn Host = &*lv.host;
    let mut head = vec![0u8];
    for v in pos {
        put_i32(&mut head, v);
    }
    head.push(u8::from(encode));
    head.push(u8::from(admin && lockable));
    head.push(u8::from(locked));
    put_str(&mut head, &previous);
    head.push(hidden.len().min(255) as u8);
    for h in hidden.iter().take(255) {
        put_i32(&mut head, *h);
    }
    // The categories' stacks, named on the client (`getHoverName`).
    head.push(cats.len().min(254) as u8);
    for c in cats.iter().take(254) {
        match c {
            Some(s) if put_stack(host, &mut Vec::new(), s, 1) => {
                head.push(1);
                put_stack(host, &mut head, s, 1);
            }
            _ => head.push(0),
        }
    }
    // `clipboardItem`: the clipboard's entries with an item (`ClipboardEntry.icon`, `itemAmount`).
    let mut list = Vec::new();
    if held_id.as_deref() == Some("create:clipboard")
        && let Some(stack) = &held
    {
        for page in super::super::display::clipboard::content(stack).pages {
            for e in page {
                if let Some((item, amount)) = e.icon
                    && let Some(s) = super::body::new_stack(host, &item, 1)
                {
                    list.push((s, amount.max(0) as u32));
                }
            }
        }
    }
    head.push(list.len().min(255) as u8);
    for (s, n) in list.iter().take(255) {
        put_stack(host, &mut head, s, *n);
    }
    // The seated keeper, drawn beside the window: its entity id, -1 for none.
    put_i32(
        &mut head,
        keeper_entity(lv, pos).map_or(-1, |e| e.id() as i32),
    );
    send_stock(lv, player, pos, head);
}

/// The ticker's network's stock to `player`, after `head` (an opening or a refresh): the
/// number of links, then the stacks, each with its category (the first of the ticker's
/// filters it passes, `receiveStockPacket`) and its item's tags, in messages of at most 20,000
/// bytes (the ones after the first `1`, its position and more stacks).
fn send_stock(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3], head: Vec<u8>) {
    let dim = lv.dim;
    let (entries, links) = match network_at(dim, pos) {
        Some(net) => {
            let s = network_stock(lv, dim, net, false);
            (s.by_count(), s.contributing_links)
        }
        None => (Vec::new(), 0),
    };
    let host: &dyn Host = &*lv.host;
    let filters: Vec<Option<create_engine::filter::Filter>> = with(dim, |n| {
        n.tickers
            .get(&pos)
            .map(|t| t.categories.clone())
            .unwrap_or_default()
    })
    .iter()
    .map(|c| {
        c.as_ref()
            .map(|s| super::super::items::filters::filter_of(host, s))
    })
    .collect();
    let blobs: Vec<Vec<u8>> = entries
        .iter()
        .filter_map(|(k, count)| {
            let mut b = Vec::new();
            if !put_stack(host, &mut b, &k.0, (*count).max(0) as u32) {
                return None;
            }
            let cat = filters
                .iter()
                .take(usize::from(UNSORTED))
                .position(|f| {
                    f.as_ref()
                        .is_some_and(|f| super::super::items::filters::passes(host, Some(f), &k.0))
                })
                .map_or(UNSORTED, |i| i as u8);
            b.push(cat);
            let tags = super::super::items::filters::item_tags(host, &k.0);
            b.push(tags.len().min(255) as u8);
            for t in tags.iter().take(255) {
                put_str(&mut b, t);
            }
            Some(b)
        })
        .collect();
    let mut msgs = Vec::new();
    let mut msg = head;
    put_i32(&mut msg, links as i32);
    let mut part: Vec<&Vec<u8>> = Vec::new();
    let mut size = 0;
    let flush = |msg: &mut Vec<u8>, part: &mut Vec<&Vec<u8>>| {
        msg.extend_from_slice(&(part.len() as u16).to_le_bytes());
        for b in part.drain(..) {
            msg.extend_from_slice(b);
        }
    };
    for b in &blobs {
        if size + b.len() > 20_000 && !part.is_empty() {
            flush(&mut msg, &mut part);
            msgs.push(std::mem::take(&mut msg));
            msg.push(1);
            for v in pos {
                put_i32(&mut msg, v);
            }
            size = 0;
        }
        size += b.len();
        part.push(b);
    }
    flush(&mut msg, &mut part);
    msgs.push(msg);
    for m in msgs {
        net::send_to_player(lv.host, player, CHANNEL, &m);
    }
}

/// What the client sent: a refresh, the order (`PackageOrderRequestPacket.applySettings`), the
/// folded categories (`StockKeeperCategoryHidingPacket`) or the lock (`StockKeeperLockPacket`),
/// from a player within reach of the ticker.
pub(super) fn client_message(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let mut r = Reader(data);
    let Some(kind) = r.u8() else {
        return;
    };
    let Some(pos) = r.pos() else {
        return;
    };
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return;
    };
    let Some(at) = player.entity().position(&*host) else {
        return;
    };
    let d2: f64 = (0..3)
        .map(|i| (at[i] - f64::from(pos[i]) - 0.5).powi(2))
        .sum();
    if d2 > 10.0 * 10.0 {
        return;
    }
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    if super::super::logistics::info_at(&lv, pos).map(|(b, _)| b) != Some(Block::StockTicker) {
        return;
    }
    let dim = lv.dim;
    match kind {
        b'R' => {
            // The screen's `containerTick`: its keeper gone, it closes.
            if !keeper_present(&lv, pos) {
                let mut m = vec![3u8];
                for v in pos {
                    put_i32(&mut m, v);
                }
                net::send_to_player(lv.host, player, CHANNEL, &m);
                return;
            }
            let mut head = vec![2u8];
            for v in pos {
                put_i32(&mut head, v);
            }
            send_stock(&mut lv, player, pos, head);
        }
        b'O' => {
            let Some(address) = r.str() else {
                return;
            };
            let Some(encode) = r.u8() else {
                return;
            };
            let Some(n) = r.u8() else {
                return;
            };
            let mut stacks = Vec::new();
            for _ in 0..n.min(COLS as u8 * 4) {
                let Some((s, count)) = r.stack(&*lv.host) else {
                    return;
                };
                if count > 0 {
                    stacks.push((Key::of(&s), count));
                }
            }
            if !super::may_interact(dim, pos, player) {
                return;
            }
            order(&mut lv, player, pos, &address, stacks, encode != 0);
        }
        b'H' => {
            let n = r.u8().unwrap_or(0);
            let mut hidden = Vec::new();
            for _ in 0..n {
                let Some(i) = r.i32() else { return };
                hidden.push(i);
            }
            with(dim, |n| {
                if let Some(t) = n.tickers.get_mut(&pos) {
                    if hidden.is_empty() {
                        t.hidden.remove(&player.uuid());
                    } else {
                        t.hidden.insert(player.uuid(), hidden);
                    }
                    n.unsaved.push((pos[0] >> 4, pos[2] >> 4));
                }
            });
        }
        b'L' => {
            let lock = r.u8().unwrap_or(0) != 0;
            super::set_locked(dim, pos, player, lock);
        }
        _ => {}
    }
}

fn order(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    pos: [i32; 3],
    address: &str,
    stacks: Vec<(Key, u32)>,
    encode: bool,
) {
    let dim = lv.dim;
    let order = Order::simple(stacks);
    let center = pos.map(|v| f64::from(v) + 0.5);
    if encode {
        // `RedstoneRequesterBlock.programRequester`: the requester in hand keeps the order.
        if !order.is_empty() {
            let _ = lv.level.play_sound(
                &mut *lv.host,
                "create:confirm",
                center,
                mod_api::SoundSource::Blocks,
                1.0,
                1.0,
            );
        }
        super::for_ticker(pos, || {
            super::program_requester(lv, player, &order, address)
        });
        return;
    }
    // `StockTickerBlockEntity.broadcastPackageRequest`: the address is kept, even for an empty
    // order (the screen closing).
    with(dim, |n| {
        if let Some(t) = n.tickers.get_mut(&pos) {
            t.previous = address.to_owned();
        }
        n.unsaved.push((pos[0] >> 4, pos[2] >> 4));
    });
    if order.is_empty() {
        return;
    }
    let _ = lv.level.play_sound(
        &mut *lv.host,
        "create:stock_ticker_request",
        center,
        mod_api::SoundSource::Blocks,
        1.0,
        1.0,
    );
    crate::glue::advancements::award(lv.host, player, "stock_ticker");
    let Some(net) = network_at(dim, pos) else {
        return;
    };
    broadcast(lv, dim, net, RequestType::Player, &order, None, address);
}

// ---------------------------------------------------------------------------------------------
// Client

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    None,
    Search,
    Address,
}

/// A stack of the network's stock: its count, its category (`UNSORTED` for none) and its item's
/// tags.
#[derive(Clone)]
struct Entry {
    stack: ItemStack,
    count: u32,
    cat: u8,
    tags: Vec<String>,
}

/// `CategoryEntry`: a category as the list shows it.
#[derive(Clone, Debug, PartialEq)]
struct Category {
    /// `targetBECategory`: the ticker's category, `-1` the unsorted one.
    target: i32,
    name: String,
    hidden: bool,
    /// Its header's offset from the top of the list.
    y: f32,
}

struct Open {
    pos: [i32; 3],
    /// The seated keeper's entity id (`stockKeeper`).
    keeper: Option<u32>,
    encode: bool,
    /// `isAdmin`: the lock is shown; `isLocked`.
    admin: bool,
    locked: bool,
    address: String,
    search: String,
    focus: Focus,
    /// The ticker's category names, and those this player folded (`hiddenCategories`).
    cat_names: Vec<String>,
    hidden: Vec<i32>,
    /// What the network holds (`None` until the server answered: `checking_stocks`), and a
    /// list still arriving in parts.
    stock: Option<Vec<Entry>>,
    incoming: Option<Vec<Entry>>,
    links: i32,
    order: Vec<(ItemStack, u32)>,
    /// `forcedEntries`: stacks just ordered, shown as at most what was left of them until the
    /// stock says so (`-1 - left`).
    forced: Vec<(ItemStack, i64)>,
    /// `clipboardItem`: the clipboard's items, for `isSchematicListMode`.
    list: Option<Vec<(ItemStack, u32)>>,
    /// `itemScroll` in rows: its value, last tick's and where it chases to (and how fast).
    scroll: f32,
    scroll_prev: f32,
    scroll_to: f32,
    chase: f32,
    /// `scrollHandleActive`.
    dragging: bool,
    /// `ticksSinceLastUpdate`, `emptyTicks`, `successTicks`.
    since_update: u32,
    empty_ticks: u32,
    sent: u32,
    /// The address suggestion picked (`DestinationSuggestions`).
    suggestion: usize,
    size: [f32; 2],
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    /// Messages to send at the next client tick, and whether to close the screen.
    static LATER: RefCell<(Vec<Vec<u8>>, bool)> = const { RefCell::new((Vec::new(), false)) };
    /// `ignoreTextInput`: the chat key that focused the search types nothing.
    static IGNORE_TEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(super) fn reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = (Vec::new(), false));
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

fn read_entries(host: &dyn ClientHost, r: &mut Reader<'_>) -> Vec<Entry> {
    let n = r.bytes(2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
    let mut out = Vec::with_capacity(usize::from(n));
    for _ in 0..n {
        let (Some((stack, count)), Some(cat), Some(nt)) = (r.stack(host), r.u8(), r.u8()) else {
            break;
        };
        let tags = (0..nt).filter_map(|_| r.str()).collect();
        out.push(Entry {
            stack,
            count,
            cat,
            tags,
        });
    }
    out
}

/// The server's stock (and the opening and closing of the screen).
pub(super) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let mut r = Reader(data);
    let (Some(kind), Some(pos)) = (r.u8(), r.pos()) else {
        return true;
    };
    if kind == 3 {
        if OPEN.with_borrow(|o| o.as_ref().is_some_and(|o| o.pos == pos)) {
            LATER.with_borrow_mut(|l| l.1 = true);
        }
        return true;
    }
    if kind == 0 {
        let encode = r.u8().unwrap_or(0) != 0;
        let admin = r.u8().unwrap_or(0) != 0;
        let locked = r.u8().unwrap_or(0) != 0;
        let address = r.str().unwrap_or_default();
        let hidden = (0..r.u8().unwrap_or(0)).filter_map(|_| r.i32()).collect();
        let mut cat_names = Vec::new();
        for _ in 0..r.u8().unwrap_or(0) {
            match r.u8() {
                Some(1) => match r.stack(&*host) {
                    Some((s, _)) => cat_names.push(host.hover_name(&s)),
                    None => break,
                },
                Some(_) => cat_names.push(String::new()),
                None => break,
            }
        }
        let list: Vec<(ItemStack, u32)> = (0..r.u8().unwrap_or(0))
            .filter_map(|_| r.stack(&*host))
            .collect();
        let keeper = r.i32().and_then(|id| u32::try_from(id).ok());
        let links = r.i32().unwrap_or(0);
        let entries = read_entries(&*host, &mut r);
        OPEN.with_borrow_mut(|o| {
            *o = Some(Open {
                pos,
                encode,
                admin,
                locked,
                address: address.chars().take(25).collect(),
                search: String::new(),
                focus: Focus::None,
                cat_names,
                hidden,
                stock: None,
                incoming: Some(entries),
                links,
                order: Vec::new(),
                forced: Vec::new(),
                list: (!list.is_empty()).then_some(list),
                keeper,
                scroll: 0.0,
                scroll_prev: 0.0,
                scroll_to: 0.0,
                chase: 0.5,
                dragging: false,
                since_update: 15,
                empty_ticks: 0,
                sent: 0,
                suggestion: 0,
                size: [0.0, 0.0],
            })
        });
        if !host.mod_screen_open(MOD) {
            OPEN.with_borrow_mut(|o| *o = None);
            return true;
        }
        // `init`'s first opening.
        ui_sound(&*host, "minecraft:block.wood.hit", 0.5, 1.5);
        ui_sound(&*host, "minecraft:item.book.page_turn", 1.0, 1.0);
        return true;
    }
    let links = if kind == 2 { r.i32() } else { None };
    let entries = read_entries(&*host, &mut r);
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut().filter(|o| o.pos == pos) else {
            return;
        };
        // A list is taken whole at the next tick; a part arriving after that joins the list.
        if kind == 2 {
            o.incoming = Some(entries);
            o.links = links.unwrap_or(o.links);
        } else if let Some(i) = o.incoming.as_mut() {
            i.extend(entries);
        } else if let Some(s) = o.stock.as_mut() {
            s.extend(entries);
        }
    });
    true
}

/// A new stock list: orders kept within it (`revalidateOrders`), the clipboard's request made
/// again (`requestSchematicList`), the scroll kept within the list.
fn stock_changed(host: &dyn ClientHost, o: &mut Open) {
    let stock = o.stock.clone().unwrap_or_default();
    let count = |s: &ItemStack| {
        stock
            .iter()
            .find(|e| e.stack.is_stackable_with(s))
            .map_or(0, |e| e.count)
    };
    if let Some(list) = &o.list {
        o.order = list
            .iter()
            .filter_map(|(s, n)| {
                let n = (*n).min(count(s));
                (n > 0).then(|| (s.clone(), n))
            })
            .collect();
    } else {
        for e in &mut o.order {
            e.1 = e.1.min(count(&e.0));
        }
        o.order.retain(|e| e.1 > 0);
    }
    clamp_scroll(host, o);
}

pub(super) fn tick(host: &mut dyn ClientHost) {
    ticked();
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    for m in send {
        net::send_to_server(host, CHANNEL, &m);
    }
    if close {
        if let Some(o) = OPEN.with_borrow_mut(Option::take) {
            for m in closing_messages(&o) {
                net::send_to_server(host, CHANNEL, &m);
            }
        }
        host.mod_screen_close(MOD);
    }
    if host.mod_screen() != Some(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
        return;
    }
    let ask = OPEN.with_borrow_mut(|o| {
        let o = o.as_mut()?;
        // The answer that arrived since the last tick.
        if let Some(fresh) = o.incoming.take() {
            o.stock = Some(fresh);
            stock_changed(host, o);
        }
        // `forcedEntries` go once the stock is down to them.
        if !o.forced.is_empty() {
            let stock = o.stock.clone().unwrap_or_default();
            o.forced.retain(|(s, f)| {
                let limited = -f - 1;
                let actual = stock
                    .iter()
                    .find(|e| e.stack.is_stackable_with(s))
                    .map_or(0, |e| i64::from(e.count));
                actual > limited
            });
        }
        let empty = o.list.is_some() || shown(host, o).0.iter().all(Vec::is_empty);
        o.empty_ticks = if empty { o.empty_ticks + 1 } else { 0 };
        o.sent = if o.sent > 0 && o.order.is_empty() {
            o.sent + 1
        } else {
            0
        };
        // `itemScroll.tickChaser`, settled within a sixteenth.
        o.scroll_prev = o.scroll;
        o.scroll += (o.scroll_to - o.scroll) * o.chase;
        if (o.scroll - o.scroll_to).abs() < 1.0 / 16.0 {
            o.scroll = o.scroll_to;
        }
        o.since_update = (o.since_update + 1).min(100);
        if o.since_update > 15 {
            o.since_update = 0;
            return Some(o.pos);
        }
        None
    });
    if let Some(pos) = ask {
        let mut m = vec![b'R'];
        for v in pos {
            put_i32(&mut m, v);
        }
        net::send_to_server(host, CHANNEL, &m);
    }
}

/// `removed`: the address is kept (an empty order) and the folded categories saved.
fn closing_messages(o: &Open) -> Vec<Vec<u8>> {
    let mut a = vec![b'O'];
    for v in o.pos {
        put_i32(&mut a, v);
    }
    put_str(&mut a, &o.address);
    a.push(0);
    a.push(0);
    let mut h = vec![b'H'];
    for v in o.pos {
        put_i32(&mut h, v);
    }
    h.push(o.hidden.len().min(255) as u8);
    for i in o.hidden.iter().take(255) {
        put_i32(&mut h, *i);
    }
    vec![a, h]
}

thread_local! {
    /// When the last client tick ran, for the frame's partial tick (screens are not told it).
    static TICKED: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

/// A text style of `color` (a name or `#RRGGBB`), italic or not.
pub(super) fn style(color: &str, italic: bool) -> TextStyle {
    let mut s = TextStyle::default();
    s.color = Some(color.into());
    s.italic = italic.then_some(true);
    s
}

/// The fraction of a tick since the last client tick (`partialTicks`).
pub(crate) fn partial_tick() -> f32 {
    TICKED
        .get()
        .map_or(1.0, |t| (t.elapsed().as_secs_f32() * 20.0).min(1.0))
}

/// Notes a client tick for `partial_tick`.
pub(super) fn ticked() {
    TICKED.set(Some(std::time::Instant::now()));
}

fn tr(host: &dyn ClientHost, key: &str, fallback: &str) -> String {
    host.translate(key).unwrap_or_else(|| fallback.to_owned())
}

const TEX: &str = "create:gui/stock_keeper";
const WIDTH: f32 = 226.0;
const HEADER_H: f32 = 36.0;
const BODY_H: f32 = 20.0;
const FOOTER_H: f32 = 80.0;
const CELL: f32 = 20.0;

/// The window's height for a screen `screen_h` high (`init`).
fn window_height(screen_h: f32) -> f32 {
    let mut h = (screen_h - 10.0).floor();
    h -= (h - HEADER_H - FOOTER_H).rem_euclid(BODY_H);
    h.min(HEADER_H + FOOTER_H + BODY_H * 17.0)
}

struct Layout {
    x: f32,
    y: f32,
    h: f32,
    items_x: f32,
    items_y: f32,
    order_y: f32,
    lock_x: f32,
    beside_search_y: f32,
}

fn layout(size: [f32; 2]) -> Layout {
    let h = window_height(size[1]);
    let x = ((size[0] - WIDTH) / 2.0).floor();
    let y = ((size[1] - h) / 2.0).floor();
    Layout {
        x,
        y,
        h,
        items_x: x + ((WIDTH - COLS as f32 * CELL) / 2.0).floor() + 1.0,
        items_y: y + 33.0,
        order_y: y + h - 72.0,
        lock_x: x + 186.0,
        beside_search_y: y + 18.0,
    }
}

/// Whether `e` matches the search (`refreshSearchResults`): a mod's id after `@`, a tag after
/// `#`, else the name or the item's id path.
fn matches(host: &dyn ClientHost, e: &Entry, search: &str) -> bool {
    if search.trim().is_empty() {
        return true;
    }
    let (mods, tags, value) = if let Some(v) = search.strip_prefix('@') {
        (true, false, v)
    } else if let Some(v) = search.strip_prefix('#') {
        (false, true, v)
    } else {
        (false, false, search)
    };
    let value = value.to_lowercase();
    let id = e.stack.item().id(host);
    let (namespace, path) = id.split_once(':').unwrap_or(("minecraft", id.as_str()));
    if mods {
        return namespace.contains(&value);
    }
    if tags {
        return e.tags.iter().any(|t| t.contains(&value));
    }
    host.hover_name(&e.stack).to_lowercase().contains(&value) || path.contains(&value)
}

/// `displayedItems` (indices into the stock, by category: the ticker's, then the unsorted) and
/// `categories` (empty when no item is in any of the ticker's, so no headers are shown).
fn shown(host: &dyn ClientHost, o: &Open) -> (Vec<Vec<usize>>, Vec<Category>) {
    let Some(stock) = &o.stock else {
        return (Vec::new(), Vec::new());
    };
    if o.list.is_some() {
        return (Vec::new(), Vec::new());
    }
    let n = o.cat_names.len();
    let mut groups = vec![Vec::new(); n + 1];
    for (i, e) in stock.iter().enumerate() {
        let g = if e.cat == UNSORTED || usize::from(e.cat) >= n {
            n
        } else {
            usize::from(e.cat)
        };
        if matches(host, e, &o.search) {
            groups[g].push(i);
        }
    }
    let mut cats: Vec<Category> = o
        .cat_names
        .iter()
        .enumerate()
        .map(|(i, name)| Category {
            target: i as i32,
            name: name.clone(),
            hidden: o.hidden.contains(&(i as i32)),
            y: 0.0,
        })
        .collect();
    cats.push(Category {
        target: -1,
        name: tr(
            host,
            "create.gui.stock_keeper.unsorted_category",
            "Unsorted",
        ),
        hidden: o.hidden.contains(&-1),
        y: 0.0,
    });
    let any = place_categories(&groups, &mut cats);
    if !any {
        cats.clear();
    }
    (groups, cats)
}

/// Each category's header offset, and whether any of the ticker's categories has items.
fn place_categories(groups: &[Vec<usize>], cats: &mut [Category]) -> bool {
    let mut y = 0.0;
    let mut any = false;
    for (i, g) in groups.iter().enumerate() {
        cats[i].y = y;
        if g.is_empty() {
            continue;
        }
        if i + 1 < groups.len() {
            any = true;
        }
        y += CELL;
        if !cats[i].hidden {
            y += g.len().div_ceil(COLS) as f32 * CELL;
        }
    }
    any
}

/// `getMaxScroll`.
fn max_scroll(groups: &[Vec<usize>], cats: &[Category], h: f32) -> i32 {
    let visible = h - 84.0;
    let mut rows = 2;
    for (i, g) in groups.iter().enumerate() {
        if g.is_empty() {
            continue;
        }
        rows += 1;
        if cats.get(i).is_some_and(|c| c.hidden) {
            continue;
        }
        rows += g.len().div_ceil(COLS);
    }
    (((rows as f32 * CELL - visible + 50.0) / CELL) as i32).max(0)
}

/// `clampScrollBar`.
fn clamp_scroll(host: &dyn ClientHost, o: &mut Open) {
    let (groups, cats) = shown(host, o);
    let max = max_scroll(&groups, &cats, layout(o.size).h) as f32;
    if o.scroll_to > max {
        o.scroll_to = max;
        o.scroll = max;
        o.scroll_prev = max;
    }
}

/// What the cursor is over: a stack of the list (its category and index there) or of the order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hover {
    Item(usize, usize),
    Order(usize),
}

/// `getHoveredSlot`.
fn hovered(
    l: &Layout,
    o: &Open,
    groups: &[Vec<usize>],
    cats: &[Category],
    mouse: [f32; 2],
) -> Option<Hover> {
    let [mx, my] = mouse.map(f32::floor);
    let x = mx + 1.0;
    if x < l.items_x || x >= l.items_x + COLS as f32 * CELL || o.list.is_some() {
        return None;
    }
    let col = ((x - l.items_x) / CELL) as usize;
    if my >= l.order_y && my < l.order_y + CELL {
        return (col < o.order.len()).then_some(Hover::Order(col));
    }
    if my < l.y + 16.0 || my > l.y + l.h - 80.0 || o.scroll != o.scroll_to {
        return None;
    }
    let local = my - l.items_y;
    for (ci, g) in groups.iter().enumerate() {
        let (hidden, cy) = cats.get(ci).map_or((false, 0.0), |c| (c.hidden, c.y));
        if hidden {
            continue;
        }
        let first = if cats.is_empty() { 4.0 } else { CELL };
        let row = ((local - first - cy) / CELL + o.scroll_to).floor();
        let slot = row as i64 * COLS as i64 + col as i64;
        if slot < 0 {
            return None;
        }
        if g.len() as i64 <= slot {
            continue;
        }
        return Some(Hover::Item(ci, slot as usize));
    }
    None
}

/// `drawItemCount`'s text.
fn count_text(count: u32) -> String {
    if count >= INF as u32 {
        "+".into()
    } else if count >= 1_000_000 {
        format!("{}m", count / 1_000_000)
    } else if count >= 10_000 {
        format!("{}k", count / 1000)
    } else if count >= 1000 {
        format!("{}k", ((count * 10) / 1000) as f32 / 10.0)
    } else if count >= 100 {
        count.to_string()
    } else {
        format!(" {count}")
    }
}

/// `drawItemCount`: the count in the screen's small digits (`NUMBERS`), right of the slot.
fn draw_count(draw: &mut dyn DrawContext, at: [f32; 2], count: u32) {
    let text = count_text(count);
    let mut x = (-(text.len() as f32) * 2.5).floor();
    for c in text.chars() {
        let (offset, width) = match c {
            ' ' => {
                x += 4.0;
                continue;
            }
            '.' => (60.0, 3.0),
            'k' => (64.0, 5.0),
            'm' => (70.0, 7.0),
            '+' => (84.0, 9.0),
            d => (f32::from(d as u8 - b'0') * 6.0, 5.0),
        };
        draw.sprite(
            TEX,
            [48.0 + offset, 176.0, width, 8.0],
            [at[0] + 14.0 + x, at[1] + 10.0, width, 8.0],
            None,
        );
        x += width - 1.0;
    }
}

fn order_count(o: &Open, s: &ItemStack) -> u32 {
    o.order
        .iter()
        .find(|(x, _)| x.is_stackable_with(s))
        .map_or(0, |e| e.1)
}

/// `renderItemEntry`'s count for a listed stack: less what is ordered, and at most what was
/// left after the last order (`forcedEntries`).
fn listed_count(o: &Open, e: &Entry) -> u32 {
    if e.count >= INF as u32 {
        return e.count;
    }
    let mut n = i64::from(e.count);
    if let Some((_, f)) = o.forced.iter().find(|(s, _)| s.is_stackable_with(&e.stack)) {
        n = n.min(-f - 1);
    }
    n -= i64::from(order_count(o, &e.stack));
    n.max(0) as u32
}

pub(super) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let size = event.draw.size();
    let mouse = event.screen.mouse;
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.as_mut() {
            o.size = size;
            if o.dragging {
                drag(host, o, mouse);
            }
            render(host, event.draw, o, mouse);
        }
    });
}

/// One stack at `at` (`renderItemEntry`): its slot (in the list), the item a little larger when
/// hovered, its count.
fn item_entry(
    draw: &mut dyn DrawContext,
    at: [f32; 2],
    stack: &ItemStack,
    count: u32,
    hover: bool,
    slot: bool,
) {
    if slot {
        draw.sprite(
            TEX,
            [32.0, 200.0, 18.0, 18.0],
            [at[0], at[1], 18.0, 18.0],
            None,
        );
    }
    if count == 0 {
        return;
    }
    let size = if hover { 16.0 * 1.075 } else { 16.0 };
    let off = 1.0 + (16.0 - size) / 2.0;
    draw.item_stack(stack, [at[0] + off, at[1] + off], size);
    if count > 1 {
        draw_count(draw, at, count);
    }
}

fn render(host: &dyn ClientHost, draw: &mut dyn DrawContext, o: &Open, mouse: [f32; 2]) {
    super::super::items::gadget_gui::dim(host, draw);
    let l = layout(draw.size());
    let (x, y) = (l.x, l.y);
    let partial = partial_tick();
    let scroll = o.scroll_prev + (o.scroll - o.scroll_prev) * partial;
    // The window: its header, as many body rows as fit, its footer (`renderBg`).
    draw.sprite(
        TEX,
        [0.0, 0.0, 256.0, HEADER_H],
        [x - 15.0, y, 256.0, HEADER_H],
        None,
    );
    let rows = ((l.h - HEADER_H - FOOTER_H) / BODY_H) as i32;
    for i in 0..rows {
        let at = y + HEADER_H + i as f32 * BODY_H;
        draw.sprite(
            TEX,
            [0.0, 48.0, 256.0, BODY_H],
            [x - 15.0, at, 256.0, BODY_H],
            None,
        );
    }
    let foot = y + HEADER_H + rows as f32 * BODY_H;
    draw.sprite(
        TEX,
        [0.0, 80.0, 256.0, FOOTER_H],
        [x - 15.0, foot, 256.0, FOOTER_H],
        None,
    );
    let shift = scroll * CELL;
    let (groups, cats) = shown(host, o);
    let stock = o.stock.as_deref().unwrap_or_default();
    let hover = hovered(&l, o, &groups, &cats, mouse);
    // The list window, cut to `itemWindowX - 5, itemWindowY, itemWindowX2 + 10, itemWindowY2`;
    // its background, the search bar, the lock and the stacks scroll with the list.
    let top = y + 17.0;
    let bottom = y + l.h - 80.0;
    let _ = draw.push_clip([x + 16.0, top, 199.0, bottom - top]);
    let reach = max_scroll(&groups, &cats, l.h) as f32 * CELL + l.h - 72.0;
    let mut slice = -2.0;
    while slice < reach {
        let at = slice - shift;
        if (-20.0..=l.h - 72.0).contains(&at) {
            draw.sprite(
                TEX,
                [37.0, 48.0, 182.0, 20.0],
                [x + 22.0, y + at + 18.0, 182.0, 20.0],
                None,
            );
        }
        slice += 20.0;
    }
    draw.sprite(
        TEX,
        [57.0, 17.0, 142.0, 18.0],
        [x + 42.0, y + 17.0 - shift, 142.0, 18.0],
        None,
    );
    {
        // The search text and hint.
        let search = if o.search.is_empty() && o.focus != Focus::Search {
            tr(host, "create.gui.stock_keeper.search_items", "Search Items")
        } else {
            let mut s = o.search.clone();
            if o.focus == Focus::Search && blink() {
                s.push('_');
            }
            s
        };
        let w = draw.text_width(&search);
        let sx = if o.search.is_empty() && o.focus != Focus::Search {
            (x + WIDTH / 2.0 - w / 2.0).floor()
        } else {
            x + 71.0
        };
        draw.text(&search, [sx, y + 22.0 - shift], 0xFF4A_2D31, false);
    }
    for (ci, g) in groups.iter().enumerate() {
        if g.is_empty() {
            continue;
        }
        let cy = cats.get(ci).map_or(0.0, |c| c.y);
        if let Some(c) = cats.get(ci) {
            let hy = l.items_y + cy - shift;
            let u = if c.hidden { 143.0 } else { 151.0 };
            draw.sprite(
                TEX,
                [u, 176.0, 8.0, 8.0],
                [l.items_x, hy + 6.0, 8.0, 8.0],
                None,
            );
            draw.text(&c.name, [l.items_x + 10.0, hy + 8.0], 0xFF4A_2D31, false);
            draw.text(&c.name, [l.items_x + 9.0, hy + 7.0], 0xFFF8_F8EC, false);
            if c.hidden {
                continue;
            }
        }
        let first = if cats.is_empty() { 4.0 } else { CELL };
        for (i, &si) in g.iter().enumerate() {
            let py = l.items_y + cy + first + (i / COLS) as f32 * CELL - shift;
            if py < y {
                continue;
            }
            if py > y + l.h - 72.0 {
                break;
            }
            let e = &stock[si];
            let at = [l.items_x + (i % COLS) as f32 * CELL, py];
            item_entry(
                draw,
                at,
                &e.stack,
                listed_count(o, e),
                hover == Some(Hover::Item(ci, i)),
                true,
            );
        }
    }
    // The lock, for the network's owner.
    if o.admin {
        let u = if o.locked { 16.0 } else { 32.0 };
        draw.sprite(
            TEX,
            [u, 176.0, 15.0, 16.0],
            [l.lock_x, l.beside_search_y - shift, 15.0, 16.0],
            None,
        );
    }
    // `getTroubleshootingMessage`, faded in once the list stayed empty.
    if groups.iter().all(Vec::is_empty) {
        let msg = if o.stock.is_none() {
            tr(
                host,
                "create.gui.stock_keeper.checking_stocks",
                "Checking stocks...",
            )
        } else if o.links == 0 {
            tr(
                host,
                "create.gui.stock_keeper.no_packagers_linked",
                "No Packagers Linked",
            )
        } else if o.stock.as_ref().is_some_and(Vec::is_empty) {
            tr(
                host,
                "create.gui.stock_keeper.inventories_empty",
                "Linked Inventories are Empty",
            )
        } else if o.list.is_some() {
            if o.order.is_empty() {
                tr(
                    host,
                    "create.gui.stock_keeper.schematic_list.no_results",
                    "No items from your checklist are in stock",
                )
            } else {
                tr(
                    host,
                    "create.gui.stock_keeper.schematic_list.requesting",
                    "Click 'Send' to request all available items from your checklist",
                )
            }
        } else {
            tr(
                host,
                "create.gui.stock_keeper.no_search_results",
                "No matching items found",
            )
        };
        let alpha = ((o.empty_ticks as f32 + partial - 10.0) / 5.0).clamp(0.0, 1.0);
        if alpha > 0.0 {
            let a = ((alpha * 255.0) as u32).max(5) << 24;
            for (i, line) in wrap(draw, &msg, 160.0).iter().enumerate() {
                let w = draw.text_width(line);
                let at = [
                    (x + WIDTH / 2.0 - w / 2.0).floor(),
                    l.items_y + 20.0 + i as f32 * 10.0,
                ];
                draw.text(line, [at[0] + 1.0, at[1] + 1.0], a | 0x4A_2D31, false);
                draw.text(line, at, a | 0xF8_F8EC, false);
            }
        }
    }
    let _ = draw.pop_clip();
    // The scroll bar.
    let max = max_scroll(&groups, &cats, l.h) as f32;
    let window_h = l.h - 92.0;
    let total_h = max * CELL + window_h;
    let bar = (window_h / total_h * (window_h - 2.0)).floor().max(5.0);
    if bar < window_h - 2.0 {
        let bx = l.items_x + COLS as f32 * CELL;
        let by = (y + 15.0 + scroll * CELL / total_h * (window_h - 2.0)).floor();
        draw.sprite(TEX, [219.0, 196.0, 5.0, 1.0], [bx, by, 5.0, bar], None);
        draw.sprite(TEX, [219.0, 192.0, 5.0, 4.0], [bx, by, 5.0, 4.0], None);
        if bar > 16.0 {
            draw.sprite(
                TEX,
                [219.0, 197.0, 5.0, 9.0],
                [bx, (by + bar / 2.0 - 4.0).floor(), 5.0, 9.0],
                None,
            );
        }
        draw.sprite(
            TEX,
            [219.0, 207.0, 5.0, 5.0],
            [bx, by + bar - 5.0, 5.0, 5.0],
            None,
        );
    }
    // The seated keeper beside the window (`renderBg`'s `stockKeeper`), turned toward the
    // cursor (`renderEntityInInventoryFollowsMouse`, its height clamped round its head).
    if let Some((keeper, bb)) = keeper_mob(host, o.pos, o.keeper) {
        let wide = ((bb[1][0] - bb[0][0]).max(0.0) - 1.0).max(0.0) * 50.0;
        let tall = ((bb[1][1] - bb[0][1]).max(0.0) - 1.0).max(0.0) * 25.0;
        let ex = x - 35.0 - (wide as i32) as f32;
        let ey = y + l.h - 47.0 - (tall as i32) as f32;
        let _ = draw.entity(
            &mod_api::GuiEntity::existing(keeper),
            mod_api::GuiEntityView::new(
                [ex - 100.0, ey - 100.0, ex + 100.0, ey + 100.0],
                50.0,
                0.0,
                mod_api::GuiEntityLook::At([mouse[0], mouse[1].clamp(ey - 50.0, ey + 10.0)]),
            ),
        );
    }
    // The blaze keeper beside the window (`renderBg`'s `blaze`): its burner drawn as a scene
    // block, the blaze in it lit and wearing the logistics hat.
    if let Some((state, mut nbt)) = blaze_keeper(host, o.pos) {
        nbt.insert(KEEPER_KEY.into(), Tag::Byte(1));
        let fields = mod_api::item::containers::ModBlockEntityFields {
            nbt,
            ..Default::default()
        };
        let entity = mod_api::Identifier::parse(super::super::kinetics::TYPE)
            .ok()
            .map(|id| mod_api::SceneBlockEntity::new(id, fields));
        let (ex, ey) = (x - 35.0, y + l.h - 43.0);
        let _ = draw.draw_scene(
            [ex - 50.0, ey - 70.0, ex + 50.0, ey + 30.0],
            &mod_api::Scene::new(vec![mod_api::SceneBlock::new([0, 0, 0], state, entity)]),
            mod_api::SceneView::new([0.5, 0.5, 0.5], -45.0, 22.5, 48.0),
        );
    }
    // The requester or table cloth being programmed (`itemToProgram`, the main hand's),
    // large beside the window.
    if o.encode
        && let Some(item) = host
            .local_player()
            .and_then(|p| super::super::items::held(host, p, mod_api::Hand::Main))
    {
        draw.item(item.item(), [x + WIDTH + 5.0, y + l.h - 70.0], 56.0);
    }
    // The order.
    for (i, (s, count)) in o.order.iter().enumerate().take(COLS) {
        let at = [l.items_x + i as f32 * CELL, l.order_y];
        item_entry(draw, at, s, *count, hover == Some(Hover::Order(i)), false);
    }
    if o.order.len() > COLS {
        let more = format!("[+{}]", o.order.len() - COLS);
        draw.text(
            &more,
            [x + WIDTH - 40.0, l.order_y + 21.0],
            0xFFF8_F8EC,
            true,
        );
    }
    // The address box, its hint, its suggestions and the clipboard beside it.
    let ax = x + 27.0;
    let ay = y + l.h - 36.0;
    if o.address.trim().is_empty() && o.focus != Focus::Address {
        let hint = tr(
            host,
            "create.gui.stock_keeper.package_address",
            "Package Address",
        );
        draw.text(&hint, [ax, ay], 0xFFCD_BCA8, false);
    } else {
        let mut s = o.address.clone();
        if o.focus == Focus::Address && blink() {
            s.push('_');
        }
        draw.text(&s, [ax, ay], 0xFF71_4A40, false);
    }
    let options = addresses(host);
    let suggested = suggestions(o, &options);
    if !suggested.is_empty() {
        let rows = suggested.len().min(7);
        let w = suggested
            .iter()
            .map(|s| draw.text_width(s))
            .fold(0.0_f32, f32::max);
        let sy = ay - 3.0 - rows as f32 * 12.0;
        draw.fill([ax - 1.0, sy, w + 2.0, rows as f32 * 12.0], 0xEE30_3030);
        for (i, s) in suggested.iter().take(rows).enumerate() {
            let color = if i == o.suggestion % suggested.len() {
                0xFFFF_FF00
            } else {
                0xFFAA_AAAA
            };
            draw.text(s, [ax, sy + 2.0 + i as f32 * 12.0], color, true);
        }
    } else if options.is_empty()
        && let Some(item) = mod_api::Identifier::parse("create:clipboard")
            .ok()
            .and_then(|i| host.item_key(&i))
    {
        draw.item(item, [ax + 92.0 + 4.0, ay - 4.0], 16.0);
    }
    // Send (Configure when programming a requester), the button lit when hovered; once sent it
    // slides away and "Request Sent!" shows.
    let just_sent = o.order.is_empty() && o.sent > 0;
    if confirm_hovered(&l, mouse) && !just_sent {
        draw.sprite(
            TEX,
            [55.0, 200.0, 80.0, 20.0],
            [x + WIDTH - 81.0, y + l.h - 41.0, 80.0, 20.0],
            None,
        );
    }
    let title = tr(host, "create.gui.stock_keeper.title", "Stock Keeper");
    let w = draw.text_width(&title);
    draw.text(
        &title,
        [(x + WIDTH / 2.0 - w / 2.0).floor(), y + 4.0],
        0xFF71_4A40,
        false,
    );
    let send = if o.encode {
        tr(host, "create.gui.stock_keeper.configure", "Configure")
    } else {
        tr(host, "create.gui.stock_keeper.send", "Send")
    };
    let w = draw.text_width(&send);
    let sx = (x + WIDTH - 42.0 - w / 2.0).floor();
    if just_sent {
        let alpha = ((o.sent as f32 + partial - 5.0) / 5.0).clamp(0.0, 1.0);
        if o.sent < 10 {
            let a = (((1.0 - alpha * alpha) * 255.0) as u32).max(5) << 24;
            draw.text(
                &send,
                [sx + alpha * alpha * 50.0, y + l.h - 35.0],
                a | 0x25_2525,
                false,
            );
        }
        let msg = tr(
            host,
            "create.gui.stock_keeper.request_sent",
            "Request Sent!",
        );
        let alpha = ((o.sent as f32 + partial - 10.0) / 5.0).clamp(0.0, 1.0);
        if alpha > 0.0 {
            let w = draw.text_width(&msg);
            let mx = (x + WIDTH / 2.0 - (w + 10.0) / 2.0).floor();
            let my = l.order_y + 5.0;
            draw.sprite(
                TEX,
                [64.0, 228.0, 8.0, 16.0],
                [mx - 8.0, my - 4.0, 8.0, 16.0],
                None,
            );
            draw.sprite(
                TEX,
                [73.0, 228.0, 1.0, 16.0],
                [mx, my - 4.0, w + 14.0, 16.0],
                None,
            );
            draw.sprite(
                TEX,
                [75.0, 228.0, 8.0, 16.0],
                [mx + w + 10.0, my - 4.0, 8.0, 16.0],
                None,
            );
            let a = ((alpha * 255.0) as u32).max(5) << 24;
            draw.text(&msg, [mx + 5.0, my], a | 0x8C_5D4B, false);
        }
    } else {
        draw.text(&send, [sx, y + l.h - 35.0], 0xFF25_2525, false);
    }
    // `renderForeground`: the hovered stack's tooltip (`renderTooltip`), the lock's and the
    // address box's tooltips.
    let tip_stack = match hover {
        Some(Hover::Item(ci, i)) => groups
            .get(ci)
            .and_then(|g| g.get(i))
            .map(|&si| stock[si].stack.clone()),
        Some(Hover::Order(i)) => o.order.get(i).map(|e| e.0.clone()),
        None => None,
    };
    let line = |k: &str, f: &str| Text::literal(tr(host, k, f));
    let gray = |k: &str, f: &str| Text::literal(tr(host, k, f)).with_style(style("gray", false));
    let dark =
        |k: &str, f: &str| Text::literal(tr(host, k, f)).with_style(style("dark_gray", true));
    let [mx, my] = mouse;
    let tip: Option<Vec<Text>> = if let Some(s) = tip_stack {
        let lines = crate::glue::items::tooltip_lines(host, &s);
        Some(if lines.is_empty() {
            vec![Text::literal(host.hover_name(&s))]
        } else {
            lines
        })
    } else if scroll < 1.0
        && o.admin
        && my > l.beside_search_y
        && my <= l.beside_search_y + 15.0
        && mx > l.lock_x
        && mx <= l.lock_x + 15.0
    {
        Some(vec![
            if o.locked {
                line(
                    "create.gui.stock_keeper.network_locked",
                    "Network is locked",
                )
            } else {
                line("create.gui.stock_keeper.network_open", "Network is open")
            },
            gray(
                "create.gui.stock_keeper.network_lock_tip",
                "Locking prevents other Players",
            ),
            gray(
                "create.gui.stock_keeper.network_lock_tip_1",
                "from ordering items directly",
            ),
            dark(
                "create.gui.stock_keeper.network_lock_tip_2",
                "Click to toggle",
            ),
        ])
    } else if o.address.trim().is_empty() && o.focus != Focus::Address && in_address(&l, mouse) {
        Some(vec![
            Text::literal(tr(
                host,
                "create.gui.factory_panel.restocker_address",
                "Send items to...",
            ))
            .with_style(style("#5391E1", false)),
            dark("create.gui.schedule.lmb_edit", "Left-Click to Edit"),
        ])
    } else if options.is_empty()
        && suggested.is_empty()
        && mx >= ax + 96.0
        && mx < ax + 112.0
        && my >= ay - 4.0
        && my < ay + 12.0
    {
        Some(vec![
            Text::literal(tr(
                host,
                "create.gui.address_box.clipboard_tip",
                "Keeping Track",
            ))
            .with_style(style("#5391E1", false)),
            gray(
                "create.gui.address_box.clipboard_tip_1",
                "Entries starting with '#' on",
            ),
            gray(
                "create.gui.address_box.clipboard_tip_2",
                "held or nearby clipboards will",
            ),
            gray(
                "create.gui.address_box.clipboard_tip_3",
                "help auto-complete this input.",
            ),
            Text::literal(tr(
                host,
                "create.gui.address_box.clipboard_tip_4",
                "(e.g. #storage, #factory)",
            ))
            .with_style(style("dark_gray", false)),
        ])
    } else {
        None
    };
    if let Some(lines) = tip {
        let _ = draw.gui_tooltip(&lines, mouse);
    }
}

/// The key a scene's burner carries to be drawn as a stock keeper.
pub(crate) const KEEPER_KEY: &str = "StockKeeper";

/// The seated keeper (the screen's `stockKeeper`, found by the server as the screen opened),
/// while it is alive beside the ticker at `pos`, and its box.
fn keeper_mob(
    host: &dyn ClientHost,
    pos: [i32; 3],
    id: Option<u32>,
) -> Option<(mod_api::EntityRef, [[f64; 3]; 2])> {
    let id = id?;
    let level = host.level(host.dimension())?;
    let lo = [pos[0] - 1, pos[1] - 1, pos[2] - 1].map(f64::from);
    let hi = [pos[0] + 2, pos[1] + 1, pos[2] + 2].map(f64::from);
    let e = level
        .entities_in(host, lo, hi)
        .unwrap_or_default()
        .into_iter()
        .find(|e| e.id() == id)?;
    Some((e, e.bounding_box(host)?))
}

/// The blaze burner beside the ticker at `pos` (the screen's `blaze`), its state and data.
fn blaze_keeper(
    host: &dyn ClientHost,
    pos: [i32; 3],
) -> Option<(mod_api::BlockState, mod_api::Compound)> {
    let level = host.level(host.dimension())?;
    [[1, 0], [-1, 0], [0, 1], [0, -1]]
        .iter()
        .find_map(|[dx, dz]| {
            let at = [pos[0] + dx, pos[1], pos[2] + dz];
            let state = level.block_state(host, at)?;
            let block = super::super::states::id(state).and_then(|id| {
                crate::glue::client_table(host, |t| t.get(id).map(|(b, _)| b)).flatten()
            })?;
            if block != Block::BlazeBurner {
                return None;
            }
            let nbt = host
                .block_entity(level, at)
                .and_then(|e| e.data(host))
                .unwrap_or_default();
            Some((state, nbt))
        })
}

/// `Font.split(msg, width)`: words to lines no wider than `width`.
fn wrap(draw: &dyn DrawContext, msg: &str, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in msg.split(' ') {
        let next = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if !line.is_empty() && draw.text_width(&next) > width {
            lines.push(std::mem::take(&mut line));
            line = word.to_owned();
        } else {
            line = next;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// `AddressEditBoxHelper.createSuggestions`: the addresses (`#` entries not ticked) on the
/// clipboards in the player's inventory, then on those placed nearby (`NEARBY_CLIPBOARDS`, as
/// the server last told), each once.
pub(super) fn addresses(host: &dyn ClientHost) -> Vec<String> {
    let Some(player) = host.local_player() else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for stack in player.inventory(host).unwrap_or_default() {
        if stack.item().id(host) != "create:clipboard" {
            continue;
        }
        for a in clipboard_addresses(&stack) {
            if !out.contains(&a) {
                out.push(a);
            }
        }
    }
    for a in NEARBY.with_borrow(Clone::clone) {
        if !out.contains(&a) {
            out.push(a);
        }
    }
    out
}

/// `appendAddresses`: a clipboard's `#` entries not ticked, the `#` taken off.
pub(crate) fn clipboard_addresses(stack: &ItemStack) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for page in super::super::display::clipboard::content(stack).pages {
        for e in page {
            if e.checked || !e.text.starts_with('#') || e.text.len() == 1 {
                continue;
            }
            let address = e.text[1..].trim().to_owned();
            if !address.is_empty() && !out.contains(&address) {
                out.push(address);
            }
        }
    }
    out
}

/// The channel of the addresses on clipboards placed near a player, server to client.
pub(super) const NEARBY_CHANNEL: &str = "create:nearby_addresses";

thread_local! {
    /// The client's: the addresses on clipboards placed nearby.
    static NEARBY: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// The server's: what each player was last told.
    static TOLD: RefCell<HashMap<u128, Vec<String>>> = RefCell::new(HashMap::new());
}

/// `ClipboardBlockEntity`'s client tick advertising itself (`advertiseClipboard`, within 32
/// blocks, kept 20 ticks): every second the server tells each player of `lv` the addresses on
/// the clipboards placed within 32 blocks, when they changed.
pub(super) fn advertise_clipboards(lv: &mut ServerLevel<'_>) {
    if lv.game_time() % 20 != 0 {
        return;
    }
    for player in lv.players() {
        let Some(at) = player.entity().position(&*lv.host) else {
            continue;
        };
        let mut addresses: Vec<String> = Vec::new();
        for stack in crate::glue::decor::clipboards_near(lv.dim, at, 32.0) {
            for a in clipboard_addresses(&stack) {
                if !addresses.contains(&a) {
                    addresses.push(a);
                }
            }
        }
        let uuid = player.uuid();
        if TOLD.with_borrow(|t| t.get(&uuid) == Some(&addresses)) {
            continue;
        }
        let mut m = Vec::new();
        for a in &addresses {
            put_str(&mut m, a);
        }
        TOLD.with_borrow_mut(|t| t.insert(uuid, addresses));
        net::send_to_player(lv.host, player, NEARBY_CHANNEL, &m);
    }
}

/// The nearby clipboards' addresses, from the server.
pub(super) fn nearby_from_server(data: &[u8]) {
    let mut r = Reader(data);
    let mut list = Vec::new();
    while let Some(s) = r.str() {
        list.push(s);
    }
    NEARBY.with_borrow_mut(|n| *n = list);
}

pub(super) fn forget_nearby() {
    NEARBY.with_borrow_mut(Vec::clear);
    TOLD.with_borrow_mut(HashMap::clear);
}

/// `DestinationSuggestions.updateCommandInfo`: while the box has the focus, the addresses
/// starting with what is typed (not it itself).
fn suggestions(o: &Open, options: &[String]) -> Vec<String> {
    if o.focus != Focus::Address {
        return Vec::new();
    }
    let typed = o.address.to_lowercase();
    options
        .iter()
        .filter(|a| **a != o.address && a.to_lowercase().starts_with(&typed))
        .cloned()
        .collect()
}

fn blink() -> bool {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
        / 300)
        .is_multiple_of(2)
}

/// `isConfirmHovered`.
fn confirm_hovered(l: &Layout, mouse: [f32; 2]) -> bool {
    let (cx, cy) = (l.x + 143.0, l.y + l.h - 39.0);
    mouse[0] >= cx && mouse[0] < cx + 78.0 && mouse[1] >= cy && mouse[1] < cy + 18.0
}

fn in_search(l: &Layout, mouse: [f32; 2]) -> bool {
    mouse[0] >= l.x + 71.0
        && mouse[0] < l.x + 171.0
        && mouse[1] >= l.y + 22.0
        && mouse[1] < l.y + 31.0
}

fn in_address(l: &Layout, mouse: [f32; 2]) -> bool {
    mouse[0] >= l.x + 27.0
        && mouse[0] < l.x + 119.0
        && mouse[1] >= l.y + l.h - 36.0
        && mouse[1] < l.y + l.h - 26.0
}

fn ui_sound(host: &dyn ClientHost, id: &str, volume: f32, pitch: f32) {
    host.play_local_sound(&SoundPlay::ui(id).with_volume(volume).with_pitch(pitch), 0);
}

/// One stack's worth (shift), ten (control) or one.
fn transfer(host: &dyn ClientHost, s: &ItemStack, modifiers: u32) -> u32 {
    if modifiers & 1 != 0 {
        host.max_stack_size(s.item()).unwrap_or(64)
    } else if modifiers & 2 != 0 {
        10
    } else {
        1
    }
}

/// Adds `delta` of `stack` to the order (`mouseClicked`'s items), within what is in stock.
fn change(host: &dyn ClientHost, o: &mut Open, stack: &ItemStack, delta: i64, available: u32) {
    let i = match o.order.iter().position(|(s, _)| s.is_stackable_with(stack)) {
        Some(i) => i,
        None => {
            if o.order.len() >= COLS || delta < 0 {
                return;
            }
            o.order.push((
                super::body::with_count(stack, 1).unwrap_or_else(|| stack.clone()),
                0,
            ));
            ui_sound(host, "minecraft:block.wool.step", 0.75, 1.2);
            ui_sound(host, "minecraft:block.bamboo_wood.step", 0.75, 0.8);
            o.order.len() - 1
        }
    };
    let now = i64::from(o.order[i].1);
    if delta < 0 {
        let next = now + delta;
        if next <= 0 {
            o.order.remove(i);
            ui_sound(host, "minecraft:block.wool.step", 0.75, 1.8);
            ui_sound(host, "minecraft:block.bamboo_wood.step", 0.75, 1.8);
        } else {
            o.order[i].1 = next as u32;
        }
        return;
    }
    o.order[i].1 = (now + delta.min(i64::from(available) - now)).max(0) as u32;
}

fn available(o: &Open, s: &ItemStack) -> u32 {
    o.stock
        .as_ref()
        .and_then(|st| st.iter().find(|e| e.stack.is_stackable_with(s)))
        .map_or(0, |e| e.count)
}

/// `sendIt`: the order to the server; the stacks ordered show as what is left of them until the
/// stock comes back; the screen shows "Request Sent!" (and closes in clipboard mode).
fn send(host: &dyn ClientHost, o: &mut Open) {
    stock_changed_orders_only(o);
    if o.order.is_empty() {
        return;
    }
    o.forced = o
        .order
        .iter()
        .filter_map(|(s, n)| {
            let have = available(o, s);
            (have < INF as u32).then(|| (s.clone(), -1 - (i64::from(have) - i64::from(*n)).max(0)))
        })
        .collect();
    let mut m = vec![b'O'];
    for v in o.pos {
        put_i32(&mut m, v);
    }
    put_str(&mut m, &o.address);
    m.push(u8::from(o.encode));
    m.push(o.order.len().min(255) as u8);
    for (s, n) in &o.order {
        put_stack(host, &mut m, s, *n);
    }
    o.order.clear();
    o.sent = 1;
    o.since_update = 10;
    let close = o.encode || o.list.is_some();
    LATER.with_borrow_mut(|l| {
        l.0.push(m);
        l.1 |= close;
    });
}

/// `revalidateOrders` without the rest of a new stock's work.
fn stock_changed_orders_only(o: &mut Open) {
    let counts: Vec<u32> = o.order.iter().map(|(s, _)| available(o, s)).collect();
    for (e, have) in o.order.iter_mut().zip(counts) {
        e.1 = e.1.min(have);
    }
    o.order.retain(|e| e.1 > 0);
}

/// `scrollHandleActive` while the button is held: the list follows the bar (`mouseDragged`).
fn drag(host: &dyn ClientHost, o: &mut Open, mouse: [f32; 2]) {
    let l = layout(o.size);
    let (groups, cats) = shown(host, o);
    let max = max_scroll(&groups, &cats, l.h) as f32;
    let window_h = l.h - 92.0;
    let total_h = max * CELL + window_h;
    let bar = (window_h / total_h * (window_h - 2.0)).floor().max(5.0);
    if bar >= window_h - 2.0 {
        return;
    }
    let target = (mouse[1] - l.y - 15.0 - bar / 2.0) * total_h / (window_h - 2.0) / CELL;
    o.scroll_to = target.clamp(0.0, max);
    o.chase = 0.8;
}

pub(super) fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    let (lmb, rmb) = (event.button == 0, event.button == 1);
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        let l = layout(o.size);
        // Search: right click clears it.
        if rmb && in_search(&l, mouse) && o.scroll_to == 0.0 {
            o.search.clear();
            o.focus = Focus::Search;
            o.scroll_to = 0.0;
            o.scroll = 0.0;
            return;
        }
        // The address box: a suggestion clicked, right click clears, a click focuses.
        if o.focus == Focus::Address {
            let options = addresses(host);
            let suggested = suggestions(o, &options);
            let rows = suggested.len().min(7);
            let ay = l.y + l.h - 36.0;
            let sy = ay - 3.0 - rows as f32 * 12.0;
            if rows > 0 && mouse[1] >= sy && mouse[1] < ay - 3.0 && mouse[0] >= l.x + 26.0 {
                let i = ((mouse[1] - sy) / 12.0) as usize;
                if let Some(s) = suggested.get(i) {
                    o.address = s.clone();
                }
                return;
            }
        }
        if in_address(&l, mouse) {
            if rmb {
                o.address.clear();
            }
            o.focus = Focus::Address;
            o.suggestion = 0;
            return;
        }
        if in_search(&l, mouse) && o.scroll_to == 0.0 && o.list.is_none() {
            o.focus = Focus::Search;
            return;
        }
        o.focus = Focus::None;
        let (groups, cats) = shown(host, o);
        // The scroll bar.
        let bar_x = l.items_x + COLS as f32 * CELL - 1.0;
        if max_scroll(&groups, &cats, l.h) > 0
            && lmb
            && mouse[0] > bar_x
            && mouse[0] <= bar_x + 8.0
            && mouse[1] > l.y + 15.0
            && mouse[1] < l.y + l.h - 82.0
        {
            o.dragging = true;
            return;
        }
        // The lock.
        if o.scroll_to == 0.0
            && lmb
            && o.admin
            && mouse[1] > l.beside_search_y
            && mouse[1] <= l.beside_search_y + 15.0
            && mouse[0] > l.lock_x
            && mouse[0] <= l.lock_x + 15.0
        {
            o.locked = !o.locked;
            let mut m = vec![b'L'];
            for v in o.pos {
                put_i32(&mut m, v);
            }
            m.push(u8::from(o.locked));
            LATER.with_borrow_mut(|later| later.0.push(m));
            ui_sound(host, "minecraft:ui.button.click", 1.0, 1.0);
            return;
        }
        if lmb && confirm_hovered(&l, mouse) {
            send(host, o);
            ui_sound(host, "minecraft:ui.button.click", 1.0, 1.0);
            return;
        }
        // A category's header folds or unfolds it.
        let local = mouse[1].floor() - l.items_y;
        if o.scroll == o.scroll_to
            && lmb
            && !cats.is_empty()
            && mouse[0] >= l.items_x
            && mouse[0] < l.items_x + COLS as f32 * CELL
            && mouse[1] >= l.y + 16.0
            && mouse[1] <= l.y + l.h - 80.0
        {
            for (ci, c) in cats.iter().enumerate() {
                if ((local - c.y) / CELL + o.scroll_to).floor() != 0.0 {
                    continue;
                }
                if groups.get(ci).is_none_or(Vec::is_empty) {
                    continue;
                }
                if c.hidden {
                    o.hidden.retain(|h| *h != c.target);
                    ui_sound(host, "minecraft:entity.item_frame.rotate_item", 1.0, 0.675);
                } else {
                    o.hidden.push(c.target);
                    ui_sound(host, "minecraft:entity.item_frame.rotate_item", 1.0, 1.5);
                }
                let (g, c) = shown(host, o);
                let max = max_scroll(&g, &c, l.h) as f32;
                o.scroll_to = o.scroll_to.min(max);
                o.scroll = o.scroll.min(max);
                return;
            }
        }
        if !lmb && !rmb {
            return;
        }
        let Some(h) = hovered(&l, o, &groups, &cats, mouse) else {
            return;
        };
        let stock = o.stock.clone().unwrap_or_default();
        let (stack, order_clicked) = match h {
            Hover::Item(ci, i) => (stock[groups[ci][i]].stack.clone(), false),
            Hover::Order(i) => (o.order[i].0.clone(), true),
        };
        let n = i64::from(transfer(host, &stack, event.modifiers));
        let avail = available(o, &stack);
        let delta = if rmb || order_clicked { -n } else { n };
        change(host, o, &stack, delta, avail);
    });
    Flow::Cancel
}

pub(super) fn screen_mouse_released(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
) -> Flow {
    if !ours(host) || event.button != 0 {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.as_mut()
            && o.dragging
        {
            // The bar follows the cursor to where it was let go.
            drag(host, o, mouse);
            o.dragging = false;
        }
    });
    Flow::Continue
}

pub(super) fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) || event.scroll[1] == 0.0 {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    let notches = event.scroll[1];
    let shift = event.modifiers & 1 != 0;
    let control = event.modifiers & 2 != 0;
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        let l = layout(o.size);
        // The address box's suggestions go round with the wheel.
        if o.focus == Focus::Address {
            let n = suggestions(o, &addresses(host)).len();
            if n > 0 {
                o.suggestion = if notches > 0.0 {
                    (o.suggestion + n - 1) % n
                } else {
                    (o.suggestion + 1) % n
                };
                return;
            }
        }
        let (groups, cats) = shown(host, o);
        let max = max_scroll(&groups, &cats, l.h);
        let h = hovered(&l, o, &groups, &cats, mouse);
        let list = matches!(h, Some(Hover::Item(..))) && !shift && max != 0;
        if h.is_none() || list {
            let dir = (notches.abs().ceil() * -notches.signum()) as f32;
            o.scroll_to = (o.scroll_to + dir).round().clamp(0.0, max as f32);
            o.chase = 0.5;
            return;
        }
        let stock = o.stock.clone().unwrap_or_default();
        let stack = match h {
            Some(Hover::Item(ci, i)) => stock[groups[ci][i]].stack.clone(),
            Some(Hover::Order(i)) => o.order[i].0.clone(),
            None => return,
        };
        let n = notches.abs().ceil() as i64 * if control { 10 } else { 1 };
        let avail = available(o, &stack);
        let before = order_count(o, &stack);
        change(host, o, &stack, if notches > 0.0 { n } else { -n }, avail);
        let after = order_count(o, &stack);
        if after != before && after != 0 && before != 0 {
            ui_sound(host, "create:scroll_value", 0.25, 1.2);
        }
    });
    Flow::Cancel
}

pub(super) fn screen_key_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenKeyPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    IGNORE_TEXT.set(false);
    let focus = OPEN.with_borrow(|o| o.as_ref().map_or(Focus::None, |o| o.focus));
    let enter = matches!(
        event.key,
        "key.keyboard.enter" | "key.keyboard.keypad.enter"
    );
    // Shift-Enter sends; Enter leaves the search box.
    if enter && focus == Focus::Search {
        OPEN.with_borrow_mut(|o| {
            if let Some(o) = o.as_mut() {
                o.focus = Focus::None;
            }
        });
        return Flow::Cancel;
    }
    if enter && event.modifiers & 1 != 0 {
        OPEN.with_borrow_mut(|o| {
            if let Some(o) = o.as_mut() {
                send(host, o);
            }
        });
        return Flow::Cancel;
    }
    if focus == Focus::None {
        // The chat key focuses the search; `E` (inventory) closes as a container screen would.
        if event.key == "key.keyboard.t" {
            OPEN.with_borrow_mut(|o| {
                if let Some(o) = o.as_mut()
                    && o.list.is_none()
                {
                    o.focus = Focus::Search;
                    IGNORE_TEXT.set(true);
                }
            });
            return Flow::Cancel;
        }
        if event.key == "key.keyboard.e" {
            LATER.with_borrow_mut(|l| l.1 = true);
            return Flow::Cancel;
        }
        if event.key == "key.keyboard.escape" {
            LATER.with_borrow_mut(|l| l.1 = true);
            return Flow::Cancel;
        }
        return Flow::Continue;
    }
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else { return };
        match event.key {
            "key.keyboard.backspace" => match o.focus {
                Focus::Search => {
                    o.search.pop();
                    o.scroll = 0.0;
                    o.scroll_to = 0.0;
                }
                Focus::Address => {
                    o.address.pop();
                    o.suggestion = 0;
                }
                Focus::None => {}
            },
            // A suggestion taken (`CommandSuggestions`: Tab), or the box left.
            "key.keyboard.tab" if o.focus == Focus::Address => {
                let s = suggestions(o, &addresses(host));
                if let Some(s) = s.get(o.suggestion % s.len().max(1)) {
                    o.address = s.clone();
                }
            }
            "key.keyboard.up" | "key.keyboard.down" if o.focus == Focus::Address => {
                let n = suggestions(o, &addresses(host)).len().max(1);
                o.suggestion = if event.key == "key.keyboard.up" {
                    (o.suggestion + n - 1) % n
                } else {
                    (o.suggestion + 1) % n
                };
            }
            "key.keyboard.enter" | "key.keyboard.keypad.enter" | "key.keyboard.tab" => {
                o.focus = Focus::None;
            }
            "key.keyboard.escape" => {
                LATER.with_borrow_mut(|l| l.1 = true);
            }
            _ => {}
        }
    });
    Flow::Cancel
}

pub(super) fn screen_char_typed(
    host: &dyn ClientHost,
    event: &mut ScreenCharTypedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let c = event.c;
    if c == '\u{a7}' || c < ' ' || c == '\u{7f}' || IGNORE_TEXT.replace(false) {
        return Flow::Cancel;
    }
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        match o.focus {
            Focus::Search if o.search.chars().count() < 50 => {
                o.search.push(c);
                o.scroll = 0.0;
                o.scroll_to = 0.0;
            }
            Focus::Address if o.address.chars().count() < 25 => {
                o.address.push(c);
                o.suggestion = 0;
            }
            _ => {}
        }
    });
    Flow::Cancel
}

/// `removed`: the address and folded categories go to the server at the next tick.
pub(super) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(o) = OPEN.with_borrow_mut(Option::take) {
        let msgs = closing_messages(&o);
        LATER.with_borrow_mut(|l| l.0.extend(msgs));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_read_as_create_writes_them() {
        assert_eq!(count_text(5), " 5");
        assert_eq!(count_text(150), "150");
        assert_eq!(count_text(1500), "1.5k");
        assert_eq!(count_text(25_000), "25k");
        assert_eq!(count_text(3_000_000), "3m");
        assert_eq!(count_text(INF as u32), "+");
    }

    #[test]
    fn the_window_fits_whole_body_rows() {
        let h = window_height(300.0);
        assert_eq!((h - HEADER_H - FOOTER_H) % BODY_H, 0.0);
        assert!(h <= 290.0);
        assert_eq!(window_height(1000.0), HEADER_H + FOOTER_H + BODY_H * 17.0);
    }

    fn cat(target: i32, hidden: bool) -> Category {
        Category {
            target,
            name: String::new(),
            hidden,
            y: 0.0,
        }
    }

    #[test]
    fn categories_stack_their_headers_and_rows() {
        // Ten items in the first category, none in the second, three unsorted.
        let groups = vec![(0..10).collect(), Vec::new(), (10..13).collect()];
        let mut cats = vec![cat(0, false), cat(1, false), cat(-1, false)];
        assert!(place_categories(&groups, &mut cats));
        assert_eq!(
            cats.iter().map(|c| c.y).collect::<Vec<_>>(),
            [0.0, 60.0, 60.0]
        );
        // Folded, the first is its header only.
        cats[0].hidden = true;
        place_categories(&groups, &mut cats);
        assert_eq!(cats[2].y, 20.0);
        // Only unsorted items: no headers.
        let groups = vec![Vec::new(), (0..3).collect()];
        let mut cats = vec![cat(0, false), cat(-1, false)];
        assert!(!place_categories(&groups, &mut cats));
    }

    #[test]
    fn the_scroll_reaches_the_last_row() {
        let groups = vec![(0..40).collect::<Vec<usize>>()];
        // 5 rows of items, one for the list and 2 more: 160 px against a 136 px window, plus 50.
        assert_eq!(max_scroll(&groups, &[], 220.0), 3);
        assert_eq!(max_scroll(&[(0..9).collect()], &[], 220.0), 0);
    }
}
