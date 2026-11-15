//! The stock ticker's category editor (`StockKeeperCategoryScreen`, `StockKeeperCategoryMenu`):
//! a click on a ticker with a keeper opens it. Its cards are the ticker's categories, each a
//! list or attribute filter named by its stack's custom name; a card is edited (its filter put
//! in or taken out of the editor's slot, its name typed), deleted (the filter given back,
//! `StockKeeperCategoryRefundPacket`), moved up or down (shift: to the top or bottom), and a new
//! one added. The request screen groups the network's stock by them (`keeper`).
//!
//! The server keeps the list (`StockKeeperCategoryEditPacket`): each change is sent as it is
//! made and written through at once, so the filters are always either the ticker's or the
//! player's. The editor's slot and the player's inventory are drawn by the screen: a click on a
//! filter in the inventory puts it in the slot (the one there goes back where it came from), a
//! click on the slot takes its filter back.

use super::super::net;
use super::super::server::ServerLevel;
use super::keeper::{Reader, put_i32, put_stack, put_str};
use super::{may_interact_message, with};
use mod_api::{
    ClientHost, DrawContext, Flow, Host, ItemStack, PlayerRef, ScreenCharTypedPreEvent,
    ScreenClosingEvent, ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent,
    ScreenMouseScrolledPreEvent, ScreenRenderPostEvent, ServerHost, SoundPlay, Tag, Text,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// Server to client: the editor (`0` opens it, `1` its contents again); client to server: `S`
/// start editing a card, `P` put an inventory slot's filter in, `T` take the filter out, `E`
/// end editing with a name, `D` delete a card, `M` move one.
pub(super) const CHANNEL: &str = "create:stock_categories";
const MOD: &str = "create";
const NAME: &str = "minecraft:custom_name";

/// `FilterItem`s: list, attribute and package filters.
pub(crate) fn is_filter(host: &dyn Host, stack: &ItemStack) -> bool {
    matches!(
        stack.item().id(host).as_str(),
        "create:filter" | "create:attribute_filter" | "create:package_filter"
    )
}

/// The stack with `name` as its custom name, or none for a blank one (`stopEditing`).
fn named(stack: &ItemStack, name: &str) -> ItemStack {
    let mut out = stack.clone();
    let mut patch = out
        .components()
        .map(|c| c.as_nbt().clone())
        .unwrap_or_default();
    if name.trim().is_empty() {
        patch.remove(&NAME.into());
    } else {
        patch.insert(NAME.into(), Tag::String(name.into()));
    }
    let _ = out.set_components(patch);
    out
}

fn has_name(stack: &ItemStack) -> bool {
    stack
        .components()
        .is_some_and(|c| c.as_nbt().contains_key(&NAME.into()))
}

// ---------------------------------------------------------------------------------------------
// Server

/// The card a player edits: level, ticker and index (`-1` a new one not yet given a filter).
type Editing = (usize, [i32; 3], i32);

thread_local! {
    static EDITING: RefCell<HashMap<u128, Editing>> = RefCell::new(HashMap::new());
}

/// `StockTickerBlock.useItemOn` with a keeper: the editor opens (`CategoryMenuProvider`).
pub(super) fn open(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) {
    EDITING.with_borrow_mut(|e| e.remove(&player.uuid()));
    send(lv, player, pos, true);
}

fn send(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3], opening: bool) {
    let dim = lv.dim;
    let Some(cats) = with(dim, |n| n.tickers.get(&pos).map(|t| t.categories.clone())) else {
        return;
    };
    let editing = EDITING.with_borrow(|e| {
        e.get(&player.uuid())
            .filter(|(d, p, _)| *d == dim && *p == pos)
            .map(|e| e.2)
    });
    let host: &dyn Host = &*lv.host;
    let mut m = vec![u8::from(!opening)];
    for v in pos {
        put_i32(&mut m, v);
    }
    m.push(cats.len().min(255) as u8);
    for c in cats.iter().take(255) {
        match c {
            Some(s) if put_stack(host, &mut Vec::new(), s, 1) => {
                m.push(1);
                put_stack(host, &mut m, s, 1);
            }
            _ => m.push(0),
        }
    }
    put_i32(&mut m, editing.unwrap_or(-2));
    let inventory: Vec<(u32, ItemStack)> = player
        .inventory_slots(host)
        .unwrap_or_default()
        .into_iter()
        .filter(|(i, _)| *i < 36)
        .collect();
    m.push(inventory.len() as u8);
    for (i, s) in &inventory {
        m.push(*i as u8);
        put_stack(host, &mut m, s, s.count());
    }
    net::send_to_player(lv.host, player, CHANNEL, &m);
}

/// What the editor did (`StockKeeperCategoryEditPacket`, `RefundPacket`, the menu's slot), from
/// a player within reach of the ticker (`stillValid`).
pub(super) fn client_message(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let mut r = Reader(data);
    let (Some(kind), Some(pos)) = (r.u8(), r.pos()) else {
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
    if d2 > 8.0 * 8.0 {
        return;
    }
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    let dim = lv.dim;
    if !with(dim, |n| n.tickers.contains_key(&pos)) || !may_interact_message(&mut lv, pos, player) {
        return;
    }
    let uuid = player.uuid();
    let editing = EDITING.with_borrow(|e| {
        e.get(&uuid)
            .filter(|(d, p, _)| *d == dim && *p == pos)
            .map(|e| e.2)
    });
    let changed = |n: &mut super::Net| n.unsaved.push((pos[0] >> 4, pos[2] >> 4));
    match kind {
        b'S' => {
            let Some(i) = r.i32() else { return };
            let len = with(dim, |n| {
                n.tickers.get(&pos).map_or(0, |t| t.categories.len())
            });
            if i >= -1 && i < len as i32 {
                EDITING.with_borrow_mut(|e| e.insert(uuid, (dim, pos, i)));
            }
        }
        b'P' => {
            let (Some(i), Some(slot)) = (editing, r.u8()) else {
                return;
            };
            let slot = u32::from(slot);
            let Some(stack) = player
                .inventory_slots(&*lv.host)
                .unwrap_or_default()
                .into_iter()
                .find(|(k, _)| *k == slot)
                .map(|(_, s)| s)
            else {
                return;
            };
            // `InactiveItemHandlerSlot.mayPlace`: filters only, one at a time.
            if !is_filter(&*lv.host, &stack) {
                return;
            }
            let one = super::body::with_count(&stack, 1).unwrap_or_else(|| stack.clone());
            let rest = stack.shrink(1);
            let old = with(dim, |n| {
                let t = n.tickers.get_mut(&pos)?;
                let old = if i < 0 {
                    t.categories.push(Some(one));
                    let at = t.categories.len() as i32 - 1;
                    EDITING.with_borrow_mut(|e| e.insert(uuid, (dim, pos, at)));
                    None
                } else {
                    t.categories
                        .get_mut(i as usize)
                        .and_then(|c| c.replace(one))
                };
                changed(n);
                Some(old)
            });
            let Some(old) = old else { return };
            let _ = player.set_inventory_slot(&mut *lv.host, slot, rest);
            if let Some(old) = old {
                super::super::items::give_back(&mut *lv.host, player, old);
            }
        }
        b'T' => {
            let Some(i) = editing.filter(|i| *i >= 0) else {
                return;
            };
            let taken = with(dim, |n| {
                let taken = n
                    .tickers
                    .get_mut(&pos)?
                    .categories
                    .get_mut(i as usize)?
                    .take();
                changed(n);
                taken
            });
            if let Some(s) = taken {
                super::super::items::give_back(&mut *lv.host, player, s);
            }
        }
        b'E' => {
            let name = r.str().unwrap_or_default();
            EDITING.with_borrow_mut(|e| e.remove(&uuid));
            let Some(i) = editing.filter(|i| *i >= 0) else {
                send(&mut lv, player, pos, false);
                return;
            };
            with(dim, |n| {
                let Some(t) = n.tickers.get_mut(&pos) else {
                    return;
                };
                let i = i as usize;
                match t.categories.get(i) {
                    Some(Some(s)) => t.categories[i] = Some(named(s, &name)),
                    Some(None) => {
                        t.categories.remove(i);
                    }
                    None => {}
                }
                changed(n);
            });
        }
        b'D' => {
            let Some(i) = r.i32() else { return };
            let refund = with(dim, |n| {
                let t = n.tickers.get_mut(&pos)?;
                if i < 0 || i as usize >= t.categories.len() {
                    return None;
                }
                let gone = t.categories.remove(i as usize);
                changed(n);
                gone
            });
            if let Some(s) = refund.filter(|s| is_filter(&*lv.host, s)) {
                super::super::items::give_back(&mut *lv.host, player, s);
            }
        }
        b'M' => {
            let (Some(i), Some(to)) = (r.i32(), r.i32()) else {
                return;
            };
            with(dim, |n| {
                let Some(t) = n.tickers.get_mut(&pos) else {
                    return;
                };
                let len = t.categories.len() as i32;
                if i < 0 || i >= len {
                    return;
                }
                let c = t.categories.remove(i as usize);
                let to = to.clamp(0, len - 1) as usize;
                t.categories.insert(to, c);
                changed(n);
            });
        }
        _ => return,
    }
    send(&mut lv, player, pos, false);
}

// ---------------------------------------------------------------------------------------------
// Client

struct Open {
    pos: [i32; 3],
    cards: Vec<Option<ItemStack>>,
    /// The card being edited (`-1` a new one), as the server last said.
    editing: Option<i32>,
    /// The name box's text and whether it has the focus.
    name: String,
    focus: bool,
    inventory: Vec<(u32, ItemStack)>,
    /// `scroll`: its value, last tick's and where it is chasing to.
    scroll: f32,
    scroll_prev: f32,
    scroll_to: f32,
    size: [f32; 2],
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static LATER: RefCell<(Vec<Vec<u8>>, bool)> = const { RefCell::new((Vec::new(), false)) };
}

pub(super) fn reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = (Vec::new(), false));
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

fn tr(host: &dyn ClientHost, key: &str, fallback: &str) -> String {
    host.translate(key).unwrap_or_else(|| fallback.to_owned())
}

fn new_category(host: &dyn ClientHost) -> String {
    tr(host, "create.gui.stock_ticker.new_category", "New Category")
}

pub(super) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let mut r = Reader(data);
    let (Some(kind), Some(pos), Some(n)) = (r.u8(), r.pos(), r.u8()) else {
        return true;
    };
    let mut cards = Vec::new();
    for _ in 0..n {
        match r.u8() {
            Some(1) => match r.stack(&*host) {
                Some((s, _)) => cards.push(Some(s)),
                None => return true,
            },
            Some(_) => cards.push(None),
            None => return true,
        }
    }
    let editing = r.i32().filter(|i| *i >= -1);
    let mut inventory = Vec::new();
    for _ in 0..r.u8().unwrap_or(0) {
        let (Some(i), Some((s, n))) = (r.u8(), r.stack(&*host)) else {
            break;
        };
        if let Some(s) = super::body::with_count(&s, n) {
            inventory.push((u32::from(i), s));
        }
    }
    if kind == 0 {
        OPEN.with_borrow_mut(|o| {
            *o = Some(Open {
                pos,
                cards,
                editing: None,
                name: String::new(),
                focus: false,
                inventory,
                scroll: 0.0,
                scroll_prev: 0.0,
                scroll_to: 0.0,
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
        o.cards = cards;
        o.inventory = inventory;
        // A new card got its filter: it is now that card.
        if o.editing.is_some() {
            o.editing = editing.or(o.editing);
        }
    });
    true
}

fn message(kind: u8, pos: [i32; 3]) -> Vec<u8> {
    let mut m = vec![kind];
    for v in pos {
        put_i32(&mut m, v);
    }
    m
}

fn queue(m: Vec<u8>) {
    LATER.with_borrow_mut(|l| l.0.push(m));
}

pub(super) fn tick(host: &mut dyn ClientHost) {
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    for m in send {
        net::send_to_server(host, CHANNEL, &m);
    }
    if close {
        host.mod_screen_close(MOD);
        OPEN.with_borrow_mut(|o| *o = None);
    }
    if host.mod_screen() != Some(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
        return;
    }
    let default = new_category(&*host);
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else { return };
        // `LerpedFloat.tickChaser`, `Chaser.EXP` at 0.7.
        o.scroll_prev = o.scroll;
        o.scroll += (o.scroll_to - o.scroll) * 0.7;
        if (o.scroll - o.scroll_to).abs() < 1.0 / 16.0 {
            o.scroll = o.scroll_to;
        }
        // `containerTick`: a named filter names a new card.
        if let Some(i) = o.editing
            && o.name == default
            && let Some(Some(s)) = usize::try_from(i).ok().and_then(|i| o.cards.get(i))
            && has_name(s)
        {
            o.name = host.hover_name(s);
        }
    });
}

const W: f32 = 192.0;
const H: f32 = 131.0;
const TEX: &str = "create:gui/stock_keeper_categories";
const CARD: f32 = 20.0;

fn corner(size: [f32; 2]) -> (f32, f32) {
    (((size[0] - W) / 2.0).floor(), ((size[1] - H) / 2.0).floor())
}

fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
}

fn confirm_rect(x: f32, y: f32, editing: bool) -> [f32; 4] {
    if editing {
        [x + 36.0 + 131.0, y + 59.0, 18.0, 18.0]
    } else {
        [x + W - 25.0, y + H - 25.0, 18.0, 18.0]
    }
}

fn slot_rect(x: f32, y: f32) -> [f32; 4] {
    [x + 16.0, y + 24.0, 16.0, 16.0]
}

fn name_rect(x: f32, y: f32) -> [f32; 4] {
    [x + 47.0, y + 28.0, 124.0, 10.0]
}

/// `addPlayerSlots(18, 106)`: inventory index and rectangle.
fn inventory_slots(x: f32, y: f32) -> Vec<(u32, [f32; 4])> {
    let mut out = Vec::new();
    for row in 0..3 {
        for col in 0..9 {
            let at = [x + 18.0 + col as f32 * 18.0, y + 106.0 + row as f32 * 18.0];
            out.push((9 + row * 9 + col, [at[0], at[1], 16.0, 16.0]));
        }
    }
    for col in 0..9 {
        out.push((col, [x + 18.0 + col as f32 * 18.0, y + 164.0, 16.0, 16.0]));
    }
    out
}

/// What a click at `mouse` on the card list does (`action`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Edit(usize),
    Delete(usize),
    Up(usize),
    Down(usize),
    New,
}

fn action(o: &Open, x: f32, y: f32, mouse: [f32; 2]) -> Option<Action> {
    let [mx, my] = mouse.map(|v| v.floor());
    if mx < x || mx >= x + W || my < y + 15.0 || my >= y + 99.0 || o.editing.is_some() {
        return None;
    }
    let mut ax = mx - x - 20.0;
    let mut ay = my - y - 24.0;
    if !(0.0..196.0).contains(&ax) || !(0.0..143.0).contains(&ay) {
        return None;
    }
    ay += o.scroll_to.round();
    let n = o.cards.len();
    for i in 0..n {
        if ay >= CARD {
            ay -= CARD;
            continue;
        }
        if ax > 0.0 && ax <= 140.0 && ay > 0.0 && ay <= 16.0 {
            return Some(Action::Edit(i));
        }
        if ax > 140.0 && ax <= 156.0 && ay > 0.0 && ay <= 16.0 {
            return Some(Action::Delete(i));
        }
        if ax > 158.0 && ax < 170.0 {
            if ay > 2.0 && ay <= 10.0 && i > 0 {
                return Some(Action::Up(i));
            }
            if ay > 10.0 && ay <= 22.0 && i + 1 < n {
                return Some(Action::Down(i));
            }
        }
        ax -= 18.0;
        ay -= 28.0;
        if ax < 0.0 || ay < 0.0 || ax > 160.0 {
            return None;
        }
    }
    (ax > 0.0 && ax <= 16.0 && ay > 0.0 && ay <= 16.0).then_some(Action::New)
}

fn card_name(host: &dyn ClientHost, card: &Option<ItemStack>) -> String {
    match card {
        Some(s) => host.hover_name(s),
        None => tr(
            host,
            "create.gui.stock_ticker.empty_category_name_placeholder",
            "(Empty)",
        ),
    }
}

fn styled(text: String, color: &str, italic: bool) -> Text {
    Text::literal(text).with_style(super::keeper::style(color, italic))
}

fn icon_button(draw: &mut dyn DrawContext, r: [f32; 4], icon: [f32; 2], hovered: bool) {
    let u = if hovered { 18.0 } else { 0.0 };
    draw.sprite("create:gui/widgets", [u, 0.0, 18.0, 18.0], r, None);
    draw.sprite(
        "create:gui/icons",
        [icon[0], icon[1], 16.0, 16.0],
        [r[0] + 1.0, r[1] + 1.0, 16.0, 16.0],
        None,
    );
}

pub(super) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let size = event.draw.size();
    let mouse = event.screen.mouse;
    let draw = &mut *event.draw;
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else { return };
        o.size = size;
        render(host, draw, o, mouse);
    });
}

fn render(host: &dyn ClientHost, draw: &mut dyn DrawContext, o: &Open, mouse: [f32; 2]) {
    super::super::items::gadget_gui::dim(host, draw);
    let (x, y) = corner(o.size);
    // `renderBg`: header, four slices, footer and the keeper's speech tail.
    draw.sprite(TEX, [32.0, 0.0, 192.0, 18.0], [x, y, 192.0, 18.0], None);
    for i in 0..4 {
        let at = y + 18.0 + i as f32 * 20.0;
        draw.sprite(TEX, [32.0, 32.0, 192.0, 20.0], [x, at, 192.0, 20.0], None);
    }
    draw.sprite(
        TEX,
        [32.0, 79.0, 200.0, 33.0],
        [x, y + 98.0, 200.0, 33.0],
        None,
    );
    draw.sprite(
        TEX,
        [238.0, 86.0, 14.0, 20.0],
        [x + W - 6.0, y + 105.0, 14.0, 20.0],
        None,
    );
    let title = tr(host, "block.create.stock_ticker", "Stock Ticker");
    let center = x + W / 2.0;
    let w = draw.text_width(&title);
    draw.text(
        &title,
        [(center - w / 2.0).floor(), y + 4.0],
        0xFF3D_3C48,
        false,
    );
    // The ticker itself, three times its size, beside the window (`renderForeground`).
    if let Some(item) = mod_api::Identifier::parse("create:stock_ticker")
        .ok()
        .and_then(|i| host.item_key(&i))
    {
        draw.item(item, [x + W + 12.0, y + H - 39.0], 48.0);
    }
    let partial = super::keeper::partial_tick();
    let scroll = o.scroll_prev + (o.scroll - o.scroll_prev) * partial;
    if o.editing.is_none() {
        // The cards, scrolled, cut to the list
        // (`enableScissor(leftPos + 3, topPos + 16, leftPos + 187, topPos + 19 + 20 * slices)`).
        let top = y + 16.0;
        let bottom = y + 99.0;
        let _ = draw.push_clip([x + 3.0, top, 184.0, bottom - top]);
        let mut at = 25.0;
        for (i, card) in o.cards.iter().enumerate() {
            let cy = y + at - scroll;
            at += CARD;
            if cy + 18.0 < top || cy > bottom {
                continue;
            }
            let cx = x + 7.0;
            draw.sprite(TEX, [38.0, 159.0, 171.0, 18.0], [cx, cy, 171.0, 18.0], None);
            if i > 0 {
                draw.sprite(
                    TEX,
                    [211.0, 160.0, 8.0, 8.0],
                    [cx + 172.0, cy + 2.0, 8.0, 8.0],
                    None,
                );
            }
            if i + 1 < o.cards.len() {
                draw.sprite(
                    TEX,
                    [211.0, 169.0, 8.0, 8.0],
                    [cx + 172.0, cy + 11.0, 8.0, 8.0],
                    None,
                );
            }
            if let Some(s) = card {
                draw.item_stack(s, [cx + 14.0, cy + 1.0], 16.0);
            }
            let name = card_name(host, card);
            let mut shown: String = name.chars().take(20).collect::<String>().trim_end().into();
            if name.chars().count() > 20 {
                shown.push_str("...");
            }
            draw.text(&shown, [cx + 35.0, cy + 5.0], 0xFF65_6565, false);
        }
        let ny = y + at - scroll;
        if ny + 18.0 >= top && ny <= bottom {
            draw.sprite(
                TEX,
                [38.0, 127.0, 27.0, 18.0],
                [x + 7.0, ny, 27.0, 18.0],
                None,
            );
        }
        let _ = draw.pop_clip();
        let cr = confirm_rect(x, y, false);
        icon_button(draw, cr, [0.0, 16.0], inside(cr, mouse));
        // `action(graphics, ..)`: the hovered control's tooltip.
        let lmb = || {
            styled(
                tr(host, "create.gui.schedule.lmb_edit", "Left-Click to Edit"),
                "dark_gray",
                true,
            )
        };
        let tip = match action(o, x, y, mouse) {
            Some(Action::Edit(i)) => Some(vec![Text::literal(card_name(host, &o.cards[i])), lmb()]),
            Some(Action::Delete(_)) => Some(vec![Text::literal(tr(
                host,
                "create.gui.stock_ticker.delete_category",
                "Delete Category",
            ))]),
            Some(Action::Up(_)) => Some(vec![
                Text::literal(tr(host, "create.gui.schedule.move_up", "Move up")),
                styled(
                    tr(
                        host,
                        "create.gui.stock_ticker.shift_moves_top",
                        "Shift moves to top",
                    ),
                    "dark_gray",
                    true,
                ),
            ]),
            Some(Action::Down(_)) => Some(vec![
                Text::literal(tr(host, "create.gui.schedule.move_down", "Move down")),
                styled(
                    tr(
                        host,
                        "create.gui.stock_ticker.shift_moves_bottom",
                        "Shift moves to bottom",
                    ),
                    "dark_gray",
                    true,
                ),
            ]),
            Some(Action::New) => Some(vec![Text::literal(new_category(host))]),
            None => None,
        };
        if let Some(lines) = tip {
            let _ = draw.gui_tooltip(&lines, mouse);
        }
        return;
    }
    // Editing: the list dimmed, the editor over it with the player's inventory.
    draw.fill([0.0, 0.0, o.size[0], o.size[1]], 0xC810_1010);
    draw.sprite(
        TEX,
        [32.0, 0.0, 192.0, 18.0],
        [x, y - 5.0, 192.0, 18.0],
        None,
    );
    draw.sprite(
        TEX,
        [32.0, 208.0, 192.0, 38.0],
        [x, y + 13.0, 192.0, 38.0],
        None,
    );
    draw.sprite(
        TEX,
        [32.0, 79.0, 200.0, 33.0],
        [x, y + 51.0, 200.0, 33.0],
        None,
    );
    draw.sprite(
        "create:gui/player_inventory",
        [0.0, 0.0, 176.0, 108.0],
        [x + 10.0, y + 88.0, 176.0, 108.0],
        None,
    );
    let inv = tr(host, "container.inventory", "Inventory");
    draw.text(&inv, [x + 18.0, y + 94.0], 0xFF40_4040, false);
    let title = tr(
        host,
        "create.gui.stock_ticker.category_editor",
        "Category Editor",
    );
    let w = draw.text_width(&title);
    draw.text(
        &title,
        [(center - w / 2.0).floor(), y - 1.0],
        0xFF3D_3C48,
        false,
    );
    let slot = slot_rect(x, y);
    let proxy = o
        .editing
        .and_then(|i| usize::try_from(i).ok())
        .and_then(|i| o.cards.get(i).cloned())
        .flatten();
    if let Some(s) = &proxy {
        draw.item_stack(s, [slot[0], slot[1]], 16.0);
    }
    if inside(slot, mouse) {
        draw.fill(slot, 0x80FF_FFFF);
    }
    // The name box.
    let nr = name_rect(x, y);
    let mut text = o.name.clone();
    if o.focus && blink() {
        text.push('_');
    }
    draw.text(&text, [nr[0], nr[1]], 0xFFEE_EEEE, true);
    let cr = confirm_rect(x, y, true);
    icon_button(draw, cr, [0.0, 16.0], inside(cr, mouse));
    for (i, r) in inventory_slots(x, y) {
        if let Some((_, s)) = o.inventory.iter().find(|(k, _)| *k == i) {
            draw.gui_draw_item_with(s, [r[0], r[1]], None);
        }
        if inside(r, mouse) {
            draw.fill(r, 0x80FF_FFFF);
        }
    }
    let header = |k: &str, f: &str| styled(tr(host, k, f), "#5391E1", false);
    let gray = |k: &str, f: &str| styled(tr(host, k, f), "gray", false);
    let tip = if inside(slot, mouse) {
        Some(match &proxy {
            // `getTooltipFromContainerItem`: the slot's stack under "Category Filter".
            Some(s) => vec![
                header("create.gui.stock_ticker.category_filter", "Category Filter"),
                Text::literal(host.hover_name(s)),
            ],
            None => vec![
                header("create.gui.stock_ticker.category_filter", "Category Filter"),
                gray(
                    "create.gui.stock_ticker.category_filter_tip",
                    "Place a List or Attribute Filter",
                ),
                gray(
                    "create.gui.stock_ticker.category_filter_tip_1",
                    "to specify which items are included",
                ),
            ],
        })
    } else if inside(nr, mouse) && !o.focus {
        Some(vec![
            header("create.gui.stock_ticker.category_name", "Category Name"),
            styled(
                tr(host, "create.gui.schedule.lmb_edit", "Left-Click to Edit"),
                "dark_gray",
                true,
            ),
        ])
    } else {
        inventory_slots(x, y)
            .into_iter()
            .find(|(_, r)| inside(*r, mouse))
            .and_then(|(i, _)| o.inventory.iter().find(|(k, _)| *k == i))
            .map(|(_, s)| vec![Text::literal(host.hover_name(s))])
    };
    if let Some(lines) = tip {
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

fn click_sound(host: &dyn ClientHost) {
    host.play_local_sound(&SoundPlay::ui("minecraft:ui.button.click"), 0);
}

/// `startEditing`: the card's name in the box (a new card's "New Category").
fn start_editing(host: &dyn ClientHost, o: &mut Open, i: i32) {
    o.editing = Some(i);
    o.focus = false;
    o.name = match usize::try_from(i).ok().and_then(|i| o.cards.get(i)) {
        Some(Some(s)) => host.hover_name(s),
        _ => new_category(host),
    };
    let mut m = message(b'S', o.pos);
    put_i32(&mut m, i);
    queue(m);
}

/// `stopEditing`: the name goes with the card (a card left without a filter goes).
fn stop_editing(host: &dyn ClientHost, o: &mut Open) {
    if o.editing.take().is_none() {
        return;
    }
    click_sound(host);
    o.focus = false;
    let mut m = message(b'E', o.pos);
    put_str(&mut m, &o.name);
    queue(m);
}

pub(super) fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    let shift = event.modifiers & 1 != 0;
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else { return };
        let (x, y) = corner(o.size);
        if o.editing.is_some() {
            if inside(confirm_rect(x, y, true), mouse) {
                stop_editing(host, o);
                return;
            }
            o.focus = inside(name_rect(x, y), mouse);
            if inside(slot_rect(x, y), mouse) {
                queue(message(b'T', o.pos));
                return;
            }
            if let Some((i, _)) = inventory_slots(x, y)
                .into_iter()
                .find(|(_, r)| inside(*r, mouse))
                && o.inventory.iter().any(|(k, _)| *k == i)
            {
                let mut m = message(b'P', o.pos);
                m.push(i as u8);
                queue(m);
            }
            return;
        }
        if event.button == 0 && inside(confirm_rect(x, y, false), mouse) {
            click_sound(host);
            LATER.with_borrow_mut(|l| l.1 = true);
            return;
        }
        let Some(a) = action(o, x, y, mouse) else {
            return;
        };
        if event.button != 0 {
            return;
        }
        click_sound(host);
        match a {
            Action::Edit(i) => start_editing(host, o, i as i32),
            Action::New => start_editing(host, o, -1),
            Action::Delete(i) => {
                let mut m = message(b'D', o.pos);
                put_i32(&mut m, i as i32);
                queue(m);
                o.cards.remove(i);
            }
            Action::Up(i) | Action::Down(i) => {
                let to = match (a, shift) {
                    (Action::Up(_), true) => 0,
                    (Action::Up(_), false) => i - 1,
                    (_, true) => o.cards.len() - 1,
                    _ => i + 1,
                };
                let mut m = message(b'M', o.pos);
                put_i32(&mut m, i as i32);
                put_i32(&mut m, to as i32);
                queue(m);
                let c = o.cards.remove(i);
                o.cards.insert(to, c);
            }
        }
    });
    Flow::Cancel
}

pub(super) fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else { return };
        if o.editing.is_some() {
            return;
        }
        // `mouseScrolled`: 12 a notch, within the cards and the new card's button.
        let mut max = 40.0 - (3.0 + 20.0 * 4.0);
        max += o.cards.len() as f32 * CARD + 24.0;
        o.scroll_to = if max > 0.0 {
            ((o.scroll_to - event.scroll[1] as f32 * 12.0).clamp(0.0, max)).floor()
        } else {
            0.0
        };
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
    let (editing, focus) = OPEN.with_borrow(|o| {
        o.as_ref()
            .map_or((false, false), |o| (o.editing.is_some(), o.focus))
    });
    if !editing {
        return if event.key == "key.keyboard.e" {
            LATER.with_borrow_mut(|l| l.1 = true);
            Flow::Cancel
        } else {
            Flow::Continue
        };
    }
    // `keyPressed` while editing: Escape, Enter in the box or E outside it end the editing.
    let enter = matches!(
        event.key,
        "key.keyboard.enter" | "key.keyboard.keypad.enter"
    );
    let end = event.key == "key.keyboard.escape"
        || (enter && focus)
        || (event.key == "key.keyboard.e" && !focus);
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else { return };
        if end {
            stop_editing(host, o);
        } else if o.focus && event.key == "key.keyboard.backspace" {
            o.name.pop();
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
    if c == '\u{a7}' || c < ' ' || c == '\u{7f}' {
        return Flow::Cancel;
    }
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.as_mut()
            && o.editing.is_some()
            && o.focus
            && o.name.chars().count() < 28
        {
            o.name.push(c);
        }
    });
    Flow::Cancel
}

/// `removed`: an edit still open ends with the screen.
pub(super) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.as_mut()
            && o.editing.is_some()
        {
            o.editing = None;
            let mut m = message(b'E', o.pos);
            put_str(&mut m, &o.name);
            queue(m);
        }
        *o = None;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(cards: usize) -> Open {
        Open {
            pos: [0; 3],
            cards: vec![None; cards],
            editing: None,
            name: String::new(),
            focus: false,
            inventory: Vec::new(),
            scroll: 0.0,
            scroll_prev: 0.0,
            scroll_to: 0.0,
            size: [400.0, 300.0],
        }
    }

    #[test]
    fn cards_answer_their_controls_as_create_lays_them_out() {
        let o = open(2);
        let (x, y) = (0.0, 0.0);
        // The first card's name field, its delete button, the second's up arrow, the new button.
        assert_eq!(action(&o, x, y, [30.0, 30.0]), Some(Action::Edit(0)));
        assert_eq!(action(&o, x, y, [165.0, 30.0]), Some(Action::Delete(0)));
        assert_eq!(action(&o, x, y, [180.0, 50.0]), Some(Action::Up(1)));
        assert_eq!(action(&o, x, y, [180.0, 37.0]), Some(Action::Down(0)));
        assert_eq!(action(&o, x, y, [180.0, 30.0]), None);
        assert_eq!(action(&o, x, y, [25.0, 70.0]), Some(Action::New));
    }
}
