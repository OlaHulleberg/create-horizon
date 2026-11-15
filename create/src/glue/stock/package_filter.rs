//! The package filter's screen (`PackageFilterScreen`, `PackageFilterMenu`): the package filter in
//! the main hand, used (not sneaking), opens it. Its address box (suggested from `#` entries on
//! clipboards in the inventory, as the stock keeper's) names the addresses the filter lets
//! through (`PackageItem.matchAddress`, `*` a wildcard); the reset button clears it, confirm
//! closes. Closing writes the address into the held filter (`saveData`); the screen closes when
//! the main hand no longer holds the filter it opened for (`containerTick`).
//!
//! As the other filters' screens, it is the mod's own screen: the player's inventory under the
//! panel is drawn, not clicked (Create's menu moves the real stacks).

use super::super::net;
use super::keeper::{Reader, put_str};
use mod_api::{
    ClientHost, Flow, Hand, HostSide, InteractionResult, ItemPart, ItemStack, PlayerRef,
    ScreenCharTypedPreEvent, ScreenClosingEvent, ScreenKeyPressedPreEvent,
    ScreenMouseButtonPressedPreEvent, ScreenMouseScrolledPreEvent, ScreenRenderPostEvent,
    ServerHost, SoundPlay, Tag, Text, UseItemEvent, UseOnEvent,
};
use std::cell::RefCell;

/// Server to client: open the screen (the address, the selected hotbar slot and the inventory);
/// client to server: the address chosen.
pub(super) const CHANNEL: &str = "create:package_filter";
const MOD: &str = "create";
const ITEM: &str = "create:package_filter";
const ADDRESS: &str = create_engine::package::ADDRESS_KEY;

// ---------------------------------------------------------------------------------------------
// Server

/// `PackageFilterItem` (`FilterItem.use` / `useOn`): the main hand's, not sneaking, opens the
/// screen.
pub(super) struct PackageFilterPart;

#[mod_api::horizon_part]
impl ItemPart for PackageFilterPart {
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        used(host, event.player, event.hand)
    }

    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        used(host, event.player, event.hand)
    }
}

fn used(host: HostSide, player: u128, hand: Hand) -> Option<InteractionResult> {
    if hand != Hand::Main {
        return None;
    }
    match host {
        HostSide::Server(host) => {
            let player = host.player(player)?;
            if super::super::items::sneaking(&*host, player) {
                return None;
            }
            open(host, player);
        }
        HostSide::Client(host) => {
            let player = host.local_player()?;
            if super::super::items::sneaking(&*host, player) {
                return None;
            }
        }
    }
    Some(InteractionResult::Success)
}

fn open(host: &mut dyn ServerHost, player: PlayerRef) {
    let Some(stack) = super::super::items::held(&*host, player, Hand::Main) else {
        return;
    };
    let address = super::super::items::filters::address(&stack);
    let mut m = Vec::new();
    put_str(&mut m, &address);
    m.push(player.selected_slot(&*host).unwrap_or(0) as u8);
    let inventory: Vec<(u32, ItemStack)> = player
        .inventory_slots(&*host)
        .unwrap_or_default()
        .into_iter()
        .filter(|(i, _)| *i < 36)
        .collect();
    m.push(inventory.len() as u8);
    for (i, s) in &inventory {
        m.push(*i as u8);
        super::keeper::put_stack(&*host, &mut m, s, s.count());
    }
    net::send_to_player(host, player, CHANNEL, &m);
}

/// `PackageFilterMenu.saveData`: the address into the filter still in the main hand.
pub(super) fn client_message(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let Some(address) = Reader(data).str() else {
        return;
    };
    let Some(mut stack) = super::super::items::held(&*host, player, Hand::Main) else {
        return;
    };
    if stack.item().id(&*host) != ITEM {
        return;
    }
    let address: String = address.chars().take(25).collect();
    super::super::items::set_custom(
        &mut stack,
        &[(ADDRESS, Some(Tag::String(address.as_str().into())))],
    );
    super::super::items::set_held(host, player, Hand::Main, Some(stack));
}

// ---------------------------------------------------------------------------------------------
// Client

struct Open {
    address: String,
    focus: bool,
    suggestion: usize,
    inventory: Vec<(u32, ItemStack)>,
    /// The filter it opened for, to close when the hand no longer holds it.
    filter: Option<ItemStack>,
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

fn main_hand(host: &dyn ClientHost) -> Option<ItemStack> {
    let player = host.local_player()?;
    host.living(player.entity())?.hand_item(host, Hand::Main)
}

pub(super) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let mut r = Reader(data);
    let address = r.str().unwrap_or_default();
    let _selected = r.u8();
    let mut inventory = Vec::new();
    for _ in 0..r.u8().unwrap_or(0) {
        let (Some(i), Some((s, n))) = (r.u8(), r.stack(&*host)) else {
            break;
        };
        if let Some(s) = super::body::with_count(&s, n) {
            inventory.push((u32::from(i), s));
        }
    }
    let filter = main_hand(&*host);
    OPEN.with_borrow_mut(|o| {
        *o = Some(Open {
            address,
            // `init`: the box has the focus.
            focus: true,
            suggestion: 0,
            inventory,
            filter,
            size: [0.0, 0.0],
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
    true
}

fn settings(o: &Open) -> Vec<u8> {
    let mut m = Vec::new();
    put_str(&mut m, &o.address);
    m
}

pub(super) fn tick(host: &mut dyn ClientHost) {
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    for m in send {
        net::send_to_server(host, CHANNEL, &m);
    }
    if close {
        if let Some(o) = OPEN.with_borrow_mut(Option::take) {
            net::send_to_server(host, CHANNEL, &settings(&o));
        }
        host.mod_screen_close(MOD);
    }
    if host.mod_screen() != Some(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
        return;
    }
    // `containerTick`: the filter left the hand, the screen closes.
    let hand = main_hand(&*host);
    let gone = OPEN.with_borrow(|o| {
        o.as_ref().is_some_and(|o| match (&o.filter, &hand) {
            (Some(a), Some(b)) => !a.is_stackable_with(b),
            _ => true,
        })
    });
    if gone {
        OPEN.with_borrow_mut(|o| *o = None);
        host.mod_screen_close(MOD);
    }
}

fn tr(host: &dyn ClientHost, key: &str, fallback: &str) -> String {
    host.translate(key).unwrap_or_else(|| fallback.to_owned())
}

/// `PACKAGE_FILTER` (218 × 79) over `PLAYER_INVENTORY`, offset by (-11, 7).
const BG: [f32; 2] = [218.0, 79.0];
const H: f32 = 79.0 + 4.0 + 108.0;

fn corner(size: [f32; 2]) -> (f32, f32) {
    (
        ((size[0] - BG[0]) / 2.0).floor() - 11.0,
        ((size[1] - H) / 2.0).floor() + 7.0,
    )
}

fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
}

fn address_rect(x: f32, y: f32) -> [f32; 4] {
    [x + 44.0, y + 28.0, 129.0, 9.0]
}

fn reset_rect(x: f32, y: f32) -> [f32; 4] {
    [x + BG[0] - 62.0, y + BG[1] - 24.0, 18.0, 18.0]
}

fn confirm_rect(x: f32, y: f32) -> [f32; 4] {
    [x + BG[0] - 33.0, y + BG[1] - 24.0, 18.0, 18.0]
}

/// The suggested addresses: those on clipboards starting with what is typed, while focused.
fn suggestions(host: &dyn ClientHost, o: &Open) -> Vec<String> {
    if !o.focus {
        return Vec::new();
    }
    let typed = o.address.to_lowercase();
    super::keeper::addresses(host)
        .into_iter()
        .filter(|a| *a != o.address && a.to_lowercase().starts_with(&typed))
        .collect()
}

fn icon_button(draw: &mut dyn mod_api::DrawContext, r: [f32; 4], icon: [f32; 2], hovered: bool) {
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
        super::super::items::gadget_gui::dim(host, draw);
        let (x, y) = corner(size);
        // `renderBg`: the inventory centred under the panel, the panel and its title.
        let ix = x + ((BG[0] - 176.0) / 2.0).floor();
        let iy = y + BG[1] + 4.0;
        draw.sprite(
            "create:gui/player_inventory",
            [0.0, 0.0, 176.0, 108.0],
            [ix, iy, 176.0, 108.0],
            None,
        );
        let inv = tr(host, "container.inventory", "Inventory");
        draw.text(&inv, [ix + 8.0, iy + 6.0], 0xFF40_4040, false);
        for (i, s) in &o.inventory {
            let (col, row) = (*i % 9, *i / 9);
            let sy = if row == 0 {
                iy + 76.0
            } else {
                iy + 18.0 + (row - 1) as f32 * 18.0
            };
            draw.gui_draw_item_with(s, [ix + 8.0 + col as f32 * 18.0, sy], None);
        }
        draw.sprite(
            "create:gui/filters_2",
            [0.0, 0.0, BG[0], BG[1]],
            [x, y, BG[0], BG[1]],
            None,
        );
        let title = tr(host, "item.create.package_filter", "Package Filter");
        let w = draw.text_width(&title);
        draw.text(
            &title,
            [(x + (BG[0] - 8.0) / 2.0 - w / 2.0).floor(), y + 4.0],
            0xFF3D_3C48,
            false,
        );
        // The held filter four times its size beside the panel, the default box in it.
        if let Some(f) = &o.filter {
            draw.item_stack(f, [x + BG[0] + 8.0, y + BG[1] - 52.0], 64.0);
        }
        if let Some(item) = mod_api::Identifier::parse("create:cardboard_package_12x12")
            .ok()
            .and_then(|i| host.item_key(&i))
        {
            draw.item(item, [x + 16.0, y + 23.0], 16.0);
        }
        // The address box, white, and its suggestions under it.
        let a = address_rect(x, y);
        let mut text = o.address.clone();
        if o.focus && blink() {
            text.push('_');
        }
        draw.text(&text, [a[0], a[1]], 0xFFFF_FFFF, true);
        let suggested = suggestions(host, o);
        if !suggested.is_empty() {
            let rows = suggested.len().min(7);
            let w = suggested
                .iter()
                .map(|s| draw.text_width(s))
                .fold(0.0_f32, f32::max);
            let sy = a[1] + a[3] + 3.0;
            draw.fill([a[0] - 1.0, sy, w + 2.0, rows as f32 * 12.0], 0xEE30_3030);
            for (i, s) in suggested.iter().take(rows).enumerate() {
                let color = if i == o.suggestion % suggested.len() {
                    0xFFFF_FF00
                } else {
                    0xFFAA_AAAA
                };
                draw.text(s, [a[0], sy + 2.0 + i as f32 * 12.0], color, true);
            }
        } else if super::keeper::addresses(host).is_empty()
            && let Some(item) = mod_api::Identifier::parse("create:clipboard")
                .ok()
                .and_then(|i| host.item_key(&i))
        {
            draw.item(item, [a[0] + a[2] + 4.0, a[1] - 4.0], 16.0);
        }
        let rr = reset_rect(x, y);
        let cr = confirm_rect(x, y);
        icon_button(draw, rr, [16.0, 0.0], inside(rr, mouse));
        icon_button(draw, cr, [0.0, 16.0], inside(cr, mouse));
        let tip = if inside(rr, mouse) {
            Some(vec![Text::literal(tr(
                host,
                "create.gui.filter.reset",
                "Reset",
            ))])
        } else if inside(cr, mouse) {
            Some(vec![Text::literal(tr(
                host,
                "create.gui.filter.confirm",
                "Confirm",
            ))])
        } else {
            None
        };
        if let Some(lines) = tip {
            let _ = draw.gui_tooltip(&lines, mouse);
        }
    });
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

pub(super) fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else { return };
        let (x, y) = corner(o.size);
        let a = address_rect(x, y);
        // A suggestion clicked.
        let suggested = suggestions(host, o);
        let sy = a[1] + a[3] + 3.0;
        if !suggested.is_empty()
            && mouse[1] >= sy
            && mouse[1] < sy + suggested.len().min(7) as f32 * 12.0
            && mouse[0] >= a[0] - 1.0
        {
            let i = ((mouse[1] - sy) / 12.0) as usize;
            if let Some(s) = suggested.get(i) {
                o.address = s.clone();
            }
            return;
        }
        if inside(a, mouse) {
            if event.button == 1 {
                o.address.clear();
            }
            o.focus = true;
            o.suggestion = 0;
            return;
        }
        o.focus = false;
        if event.button != 0 {
            return;
        }
        if inside(reset_rect(x, y), mouse) {
            // `clearContents`, `contentsCleared`: the address emptied and focused again.
            click_sound(host);
            o.address.clear();
            o.focus = true;
        } else if inside(confirm_rect(x, y), mouse) {
            click_sound(host);
            LATER.with_borrow_mut(|l| l.1 = true);
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
        let n = suggestions(host, o).len();
        if n > 0 {
            o.suggestion = if event.scroll[1] > 0.0 {
                (o.suggestion + n - 1) % n
            } else {
                (o.suggestion + 1) % n
            };
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
    let focus = OPEN.with_borrow(|o| o.as_ref().is_some_and(|o| o.focus));
    if !focus {
        if matches!(event.key, "key.keyboard.e" | "key.keyboard.escape") {
            LATER.with_borrow_mut(|l| l.1 = true);
            return Flow::Cancel;
        }
        return Flow::Continue;
    }
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else { return };
        match event.key {
            "key.keyboard.backspace" => {
                o.address.pop();
                o.suggestion = 0;
            }
            "key.keyboard.tab" => {
                let s = suggestions(host, o);
                if let Some(s) = s.get(o.suggestion % s.len().max(1)) {
                    o.address = s.clone();
                }
            }
            "key.keyboard.up" | "key.keyboard.down" => {
                let n = suggestions(host, o).len().max(1);
                o.suggestion = if event.key == "key.keyboard.up" {
                    (o.suggestion + n - 1) % n
                } else {
                    (o.suggestion + 1) % n
                };
            }
            // `keyPressed`: Enter takes the focus away.
            "key.keyboard.enter" | "key.keyboard.keypad.enter" => o.focus = false,
            "key.keyboard.escape" => LATER.with_borrow_mut(|l| l.1 = true),
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
    if c == '\u{a7}' || c < ' ' || c == '\u{7f}' {
        return Flow::Cancel;
    }
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.as_mut()
            && o.focus
            && o.address.chars().count() < 25
        {
            o.address.push(c);
            o.suggestion = 0;
        }
    });
    Flow::Cancel
}

/// `removed`: the address goes to the server at the next tick.
pub(super) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(o) = OPEN.with_borrow_mut(Option::take) {
        let m = settings(&o);
        LATER.with_borrow_mut(|l| l.0.push(m));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_window_sits_where_create_offsets_it() {
        // A 427×240 screen: the 218-wide panel 11 left of centre, 7 below.
        assert_eq!(super::corner([427.0, 240.0]), (93.0, 31.0));
    }
}
