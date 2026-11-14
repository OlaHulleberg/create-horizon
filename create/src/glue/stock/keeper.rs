//! The stock keeper's request screen (`StockKeeperRequestScreen`): a mob seated beside a stock
//! ticker, or a blaze burner beside it, is clicked (`StockTickerInteractionHandler`) and the
//! screen lists what the ticker's network holds (`getRecentSummary`, `LogisticalStockResponse`),
//! asked again every second while open. Clicks put up to nine stacks on the order (shift a
//! stack, control ten; right click takes off), the address box names where they go, and Send
//! (`PackageOrderRequestPacket`) broadcasts the order. Holding a redstone requester, the screen
//! programs it instead (`encodeRequester`: Configure).
//!
//! Left out: the keeper drawn beside the window, categories (the ticker's category editor is not
//! ported), crafting recipes from JEI, the network lock and the search sync button.

use super::super::net;
use super::super::server::ServerLevel;
use super::{Key, broadcast, network_at, network_stock, with};
use create_engine::Block;
use create_engine::stock::{Order, RequestType};
use mod_api::{
    ClientHost, DrawContext, Flow, Host, ItemStack, PlayerRef, ScreenCharTypedPreEvent,
    ScreenClosingEvent, ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent,
    ScreenMouseScrolledPreEvent, ScreenRenderPostEvent, ServerHost, SoundPlay, Tag, Text,
};
use std::cell::RefCell;

/// Server to client: the screen's contents (`0` opens it, `1` adds stacks, `2` starts a fresh
/// list); client to server: `R` asks for the stock again, `O` sends the order.
pub(super) const CHANNEL: &str = "create:stock_keeper";
const MOD: &str = "create";
/// The order's slots (`cols`).
const COLS: usize = 9;

// ---------------------------------------------------------------------------------------------
// Wire

pub(crate) fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = &s.as_bytes()[..s.len().min(255)];
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
    for dy in [0, 1] {
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let p = [pos[0] + dx, pos[1] - dy, pos[2] + dz];
            if dy == 0
                && super::super::logistics::info_at(lv, p)
                    .is_some_and(|(b, _)| b == Block::BlazeBurner)
            {
                return true;
            }
            let lo = p.map(f64::from);
            let hi = p.map(|v| f64::from(v) + 1.0);
            let seated = lv
                .level
                .entities_in(&*lv.host, lo, hi)
                .unwrap_or_default()
                .into_iter()
                .any(|e| {
                    e.tags(&*lv.host)
                        .is_some_and(|t| t.iter().any(|t| t == "create.seat"))
                        && e.passengers(&*lv.host).is_some_and(|p| !p.is_empty())
                });
            if seated {
                return true;
            }
        }
    }
    false
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

/// `interactWithLogisticsManagerAt`: the ticker's request screen opens for `player`, with the
/// address it last sent to and its network's stock.
pub(super) fn open(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) {
    let dim = lv.dim;
    let previous =
        with(dim, |n| n.tickers.get(&pos).map(|t| t.previous.clone())).unwrap_or_default();
    let held = super::super::items::held(&*lv.host, player, mod_api::Hand::Main)
        .map(|s| s.item().id(&*lv.host));
    let encode = held.as_deref() == Some("create:redstone_requester");
    let mut head = vec![0u8];
    for v in pos {
        put_i32(&mut head, v);
    }
    head.push(u8::from(encode));
    put_str(&mut head, &previous);
    send_stock(lv, player, pos, head);
}

/// The ticker's network's stock to `player`, after `head` (an opening or a refresh): the
/// number of links, then the stacks, in messages of at most 20,000 bytes (the ones after the
/// first `1`, its position and more stacks).
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
    let blobs: Vec<Vec<u8>> = entries
        .iter()
        .filter_map(|(k, count)| {
            let mut b = Vec::new();
            put_stack(host, &mut b, &k.0, (*count).max(0) as u32).then_some(b)
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

/// What the client sent: a refresh, or the order (`PackageOrderRequestPacket.applySettings`),
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
    match kind {
        b'R' => {
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
            for _ in 0..n.min(COLS as u8) {
                let Some((s, count)) = r.stack(&*lv.host) else {
                    return;
                };
                if count > 0 {
                    stacks.push((Key::of(&s), count));
                }
            }
            order(&mut lv, player, pos, &address, stacks, encode != 0);
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
    with(dim, |n| {
        if let Some(t) = n.tickers.get_mut(&pos) {
            t.previous = address.to_owned();
        }
        n.unsaved.push((pos[0] >> 4, pos[2] >> 4));
    });
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

struct Open {
    pos: [i32; 3],
    encode: bool,
    address: String,
    search: String,
    focus: Focus,
    /// What the network holds (`None` until the server answered: `checking_stocks`).
    stock: Option<Vec<(ItemStack, u32)>>,
    links: i32,
    order: Vec<(ItemStack, u32)>,
    scroll: i32,
    ticks: u32,
    /// `successTicks`.
    sent: u32,
    size: [f32; 2],
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    /// Messages to send at the next client tick, and whether to close the screen.
    static LATER: RefCell<(Vec<Vec<u8>>, bool)> = const { RefCell::new((Vec::new(), false)) };
}

pub(super) fn reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = (Vec::new(), false));
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

/// The server's stock (and the opening of the screen).
pub(super) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let mut r = Reader(data);
    let (Some(kind), Some(pos)) = (r.u8(), r.pos()) else {
        return true;
    };
    let opening = kind == 0;
    let (encode, address) = if opening {
        let encode = r.u8().unwrap_or(0) != 0;
        (encode, r.str().unwrap_or_default())
    } else {
        (false, String::new())
    };
    let links = if kind == 1 { None } else { r.i32() };
    let n = r.bytes(2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
    let mut stacks = Vec::with_capacity(usize::from(n));
    for _ in 0..n {
        let Some(s) = r.stack(&*host) else {
            break;
        };
        stacks.push(s);
    }
    if opening {
        OPEN.with_borrow_mut(|o| {
            *o = Some(Open {
                pos,
                encode,
                address,
                search: String::new(),
                focus: Focus::None,
                stock: Some(stacks),
                links: links.unwrap_or(0),
                order: Vec::new(),
                scroll: 0,
                ticks: 0,
                sent: 0,
                size: [0.0, 0.0],
            })
        });
        if !host.mod_screen_open(MOD) {
            OPEN.with_borrow_mut(|o| *o = None);
        }
        return true;
    }
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut().filter(|o| o.pos == pos) else {
            return;
        };
        if kind == 2 {
            o.stock = Some(stacks);
            o.links = links.unwrap_or(o.links);
        } else {
            o.stock.get_or_insert_with(Vec::new).extend(stacks);
        }
    });
    true
}

pub(super) fn tick(host: &mut dyn ClientHost) {
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    if close {
        host.mod_screen_close(MOD);
        OPEN.with_borrow_mut(|o| *o = None);
    }
    for m in send {
        net::send_to_server(host, CHANNEL, &m);
    }
    if host.mod_screen() != Some(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
        return;
    }
    // `LogisticalStockRequestPacket` every 20 ticks (sooner just after an order).
    let ask = OPEN.with_borrow_mut(|o| {
        let o = o.as_mut()?;
        o.ticks += 1;
        if o.sent > 0 {
            o.sent += 1;
            if o.sent > 30 {
                o.sent = 0;
            }
        }
        o.ticks.is_multiple_of(20).then_some(o.pos)
    });
    if let Some(pos) = ask {
        let mut m = vec![b'R'];
        for v in pos {
            put_i32(&mut m, v);
        }
        net::send_to_server(host, CHANNEL, &m);
    }
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
    }
}

/// The stacks shown: the stock matching the search (by name, `@` a mod's).
fn displayed(host: &dyn ClientHost, o: &Open) -> Vec<(ItemStack, u32)> {
    let Some(stock) = &o.stock else {
        return Vec::new();
    };
    let q = o.search.trim().to_lowercase();
    stock
        .iter()
        .filter(|(s, _)| {
            if q.is_empty() {
                return true;
            }
            if let Some(m) = q.strip_prefix('@') {
                return s
                    .item()
                    .id(host)
                    .split(':')
                    .next()
                    .is_some_and(|ns| ns.contains(m));
            }
            host.hover_name(s).to_lowercase().contains(&q)
        })
        .cloned()
        .collect()
}

/// `getMaxScroll` without categories.
fn max_scroll(n: usize, h: f32) -> i32 {
    let rows = 2 + n.div_ceil(COLS);
    (((rows as f32 * CELL - (h - 84.0) + 50.0) / CELL) as i32).max(0)
}

/// What the cursor is over: an item of the list (its index) or of the order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hover {
    Item(usize),
    Order(usize),
}

fn hovered(l: &Layout, o: &Open, n: usize, mouse: [f32; 2]) -> Option<Hover> {
    let [mx, my] = mouse;
    if mx < l.items_x || mx >= l.items_x + COLS as f32 * CELL {
        return None;
    }
    let col = ((mx - l.items_x) / CELL) as usize;
    if my >= l.order_y && my < l.order_y + CELL {
        return (col < o.order.len()).then_some(Hover::Order(col));
    }
    if my < l.y + 16.0 || my > l.y + l.h - 80.0 {
        return None;
    }
    let local = my - l.items_y - 4.0 + o.scroll as f32 * CELL;
    if local < 0.0 {
        return None;
    }
    let i = (local / CELL) as usize * COLS + col;
    (i < n).then_some(Hover::Item(i))
}

/// `drawItemCount`'s text.
fn count_text(count: u32) -> String {
    if count >= 1_000_000 {
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

pub(super) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let size = event.draw.size();
    let mouse = event.screen.mouse;
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.as_mut() {
            o.size = size;
            render(host, event.draw, o, mouse);
        }
    });
}

fn render(host: &dyn ClientHost, draw: &mut dyn DrawContext, o: &Open, mouse: [f32; 2]) {
    let l = layout(draw.size());
    let (x, y) = (l.x, l.y);
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
    let shown = displayed(host, o);
    let hover = hovered(&l, o, shown.len(), mouse);
    // The items, a row at a time within the window.
    for (i, (s, count)) in shown.iter().enumerate() {
        let row = (i / COLS) as i32 - o.scroll;
        let at = [
            l.items_x + (i % COLS) as f32 * CELL,
            l.items_y + 4.0 + row as f32 * CELL,
        ];
        if at[1] < y + 14.0 || at[1] > y + l.h - 92.0 {
            continue;
        }
        draw.sprite(
            TEX,
            [32.0, 200.0, 18.0, 18.0],
            [at[0], at[1], 18.0, 18.0],
            None,
        );
        let left = count.saturating_sub(order_count(o, s));
        if left > 0 {
            let grow = if hover == Some(Hover::Item(i)) {
                1.0
            } else {
                0.0
            };
            draw.item_stack(
                s,
                [at[0] + 1.0 - grow / 2.0, at[1] + 1.0 - grow / 2.0],
                16.0 + grow,
            );
        }
        if left > 1 {
            draw_count(draw, at, left);
        }
    }
    // The search box (`searchBox`) and its hint.
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
    draw.text(
        &search,
        [x + WIDTH / 2.0 - w / 2.0, y + 22.0],
        0xFF4A_2D31,
        false,
    );
    if o.stock.as_ref().is_none_or(|_| shown.is_empty()) {
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
        } else {
            tr(
                host,
                "create.gui.stock_keeper.no_search_results",
                "No matching items found",
            )
        };
        let w = draw.text_width(&msg);
        let at = [x + WIDTH / 2.0 - w / 2.0, l.items_y + 20.0];
        draw.text(&msg, [at[0] + 1.0, at[1] + 1.0], 0xFF4A_2D31, false);
        draw.text(&msg, at, 0xFFF8_F8EC, false);
    }
    // The order.
    for (i, (s, count)) in o.order.iter().enumerate() {
        let at = [l.items_x + i as f32 * CELL, l.order_y];
        let grow = if hover == Some(Hover::Order(i)) {
            1.0
        } else {
            0.0
        };
        draw.item_stack(
            s,
            [at[0] + 1.0 - grow / 2.0, at[1] + 1.0 - grow / 2.0],
            16.0 + grow,
        );
        if *count > 1 {
            draw_count(draw, at, *count);
        }
    }
    // The address box.
    let ax = x + 27.0;
    let ay = y + l.h - 36.0;
    if o.address.is_empty() && o.focus != Focus::Address {
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
    // Send (Configure when programming a requester), the button lit when hovered.
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
        [x + WIDTH / 2.0 - w / 2.0, y + 4.0],
        0xFF71_4A40,
        false,
    );
    let send = if o.encode {
        tr(host, "create.gui.stock_keeper.configure", "Configure")
    } else {
        tr(host, "create.gui.stock_keeper.send", "Send")
    };
    if !just_sent || o.sent < 10 {
        let w = draw.text_width(&send);
        draw.text(
            &send,
            [x + WIDTH - 42.0 - w / 2.0, y + l.h - 35.0],
            0xFF25_2525,
            false,
        );
    }
    if just_sent {
        let msg = tr(
            host,
            "create.gui.stock_keeper.request_sent",
            "Request Sent!",
        );
        let w = draw.text_width(&msg);
        let mx = x + WIDTH / 2.0 - (w + 10.0) / 2.0;
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
            [mx, my - 4.0, w + 10.0, 16.0],
            None,
        );
        draw.sprite(
            TEX,
            [75.0, 228.0, 8.0, 16.0],
            [mx + w + 10.0, my - 4.0, 8.0, 16.0],
            None,
        );
        draw.text(&msg, [mx + 5.0, my], 0xFF4A_2D31, false);
    }
    // The hovered stack's name and how many there are (Create shows the item's tooltip).
    let tip = match hover {
        Some(Hover::Item(i)) => shown.get(i).cloned(),
        Some(Hover::Order(i)) => o.order.get(i).cloned(),
        None => None,
    };
    if let Some((s, n)) = tip {
        let lines = [
            Text::literal(host.hover_name(&s)),
            Text::literal(format!("x{n}")),
        ];
        let _ = draw.gui_tooltip(&lines, mouse);
    }
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
            o.order.push((stack.clone(), 0));
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
    let room = (i64::from(available) - now).max(0);
    o.order[i].1 = (now + delta.min(room)) as u32;
    if o.order[i].1 == 0 {
        o.order.remove(i);
    }
}

fn available(o: &Open, s: &ItemStack) -> u32 {
    o.stock
        .as_ref()
        .and_then(|st| st.iter().find(|(x, _)| x.is_stackable_with(s)))
        .map_or(0, |e| e.1)
}

/// `sendIt`: the order to the server; the screen shows "Request Sent!".
fn send(host: &dyn ClientHost, o: &mut Open) {
    if o.order.is_empty() {
        return;
    }
    let mut m = vec![b'O'];
    for v in o.pos {
        put_i32(&mut m, v);
    }
    put_str(&mut m, &o.address);
    m.push(u8::from(o.encode));
    m.push(o.order.len() as u8);
    for (s, n) in &o.order {
        put_stack(host, &mut m, s, *n);
    }
    o.order.clear();
    o.sent = 1;
    o.ticks = 10;
    let close = o.encode;
    LATER.with_borrow_mut(|l| {
        l.0.push(m);
        l.1 |= close;
    });
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
        // The search box (right click clears it) and the address box take the focus.
        let in_search = mouse[0] >= l.x + 71.0
            && mouse[0] < l.x + 171.0
            && mouse[1] >= l.y + 20.0
            && mouse[1] < l.y + 32.0;
        let in_address = mouse[0] >= l.x + 27.0
            && mouse[0] < l.x + 119.0
            && mouse[1] >= l.y + l.h - 37.0
            && mouse[1] < l.y + l.h - 25.0;
        if in_search {
            if rmb {
                o.search.clear();
                o.scroll = 0;
            }
            o.focus = Focus::Search;
            return;
        }
        if in_address {
            o.focus = Focus::Address;
            return;
        }
        o.focus = Focus::None;
        if lmb && confirm_hovered(&l, mouse) {
            ui_sound(host, "minecraft:ui.button.click", 1.0, 1.0);
            send(host, o);
            return;
        }
        if !lmb && !rmb {
            return;
        }
        let shown = displayed(host, o);
        let Some(h) = hovered(&l, o, shown.len(), mouse) else {
            return;
        };
        let (stack, order_clicked) = match h {
            Hover::Item(i) => (shown[i].0.clone(), false),
            Hover::Order(i) => (o.order[i].0.clone(), true),
        };
        let n = i64::from(transfer(host, &stack, event.modifiers));
        let avail = available(o, &stack);
        let delta = if rmb || order_clicked { -n } else { n };
        change(host, o, &stack, delta, avail);
    });
    Flow::Cancel
}

pub(super) fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) || event.scroll[1] == 0.0 {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    let up = event.scroll[1] > 0.0;
    let shift = event.modifiers & 1 != 0;
    let control = event.modifiers & 2 != 0;
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        let l = layout(o.size);
        let shown = displayed(host, o);
        let max = max_scroll(shown.len(), l.h);
        let h = hovered(&l, o, shown.len(), mouse);
        let list = matches!(h, Some(Hover::Item(_))) && !shift && max != 0;
        if h.is_none() || list {
            o.scroll = (o.scroll + if up { -1 } else { 1 }).clamp(0, max);
            return;
        }
        let stack = match h {
            Some(Hover::Item(i)) => shown[i].0.clone(),
            Some(Hover::Order(i)) => o.order[i].0.clone(),
            None => return,
        };
        let n = if control { 10 } else { 1 };
        let avail = available(o, &stack);
        let before = order_count(o, &stack);
        change(host, o, &stack, if up { n } else { -n }, avail);
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
    let focus = OPEN.with_borrow(|o| o.as_ref().map_or(Focus::None, |o| o.focus));
    if focus == Focus::None {
        // `E` (inventory) closes it as a container screen would; Escape as any screen.
        if event.key == "key.keyboard.e" {
            LATER.with_borrow_mut(|l| l.1 = true);
            return Flow::Cancel;
        }
        return Flow::Continue;
    }
    match event.key {
        "key.keyboard.backspace" => OPEN.with_borrow_mut(|o| {
            if let Some(o) = o.as_mut() {
                match o.focus {
                    Focus::Search => {
                        o.search.pop();
                        o.scroll = 0;
                    }
                    Focus::Address => {
                        o.address.pop();
                    }
                    Focus::None => {}
                }
            }
        }),
        "key.keyboard.enter" | "key.keyboard.keypad.enter" | "key.keyboard.tab" => OPEN
            .with_borrow_mut(|o| {
                if let Some(o) = o.as_mut() {
                    o.focus = Focus::None;
                }
            }),
        "key.keyboard.escape" => return Flow::Continue,
        _ => {}
    }
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
    if c == '\u{a7}' || c < ' ' || c == '\u{7f}' {
        return Flow::Cancel;
    }
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        match o.focus {
            Focus::Search if o.search.chars().count() < 50 => {
                o.search.push(c);
                o.scroll = 0;
            }
            Focus::Address if o.address.chars().count() < 50 => o.address.push(c),
            _ => {}
        }
    });
    Flow::Cancel
}

pub(super) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    OPEN.with_borrow_mut(|o| *o = None);
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
    }

    #[test]
    fn the_window_fits_whole_body_rows() {
        let h = window_height(300.0);
        assert_eq!((h - HEADER_H - FOOTER_H) % BODY_H, 0.0);
        assert!(h <= 290.0);
        assert_eq!(window_height(1000.0), HEADER_H + FOOTER_H + BODY_H * 17.0);
    }
}
