//! The frogport's and postbox's screen (`PackagePortScreen`): the address it catches packages
//! for (`addressFilter`, written in its header), whether it sends and receives or only sends
//! (shown while it has a target), and its 18 slots of packages beside the player's inventory.
//! Closing sends the address and the mode (`PackagePortConfigurationPacket`).
//!
//! The menu's slots are drawn by the screen, not the game's container: a click on a package in
//! the port gives it to the player, a click on a package in the player's inventory puts it into
//! the port (as a shift-click would move it); nothing is carried on the cursor.

use super::super::net;
use super::super::server::ServerLevel;
use super::super::stock::keeper::{Reader, put_i32, put_stack, put_str};
use super::{body, mark_unsaved, parcel, with};
use create_engine::package;
use create_engine::port::SLOTS;
use mod_api::{
    ClientHost, DrawContext, Flow, ItemStack, PlayerRef, ScreenCharTypedPreEvent,
    ScreenClosingEvent, ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent,
    ScreenRenderPostEvent, ServerHost, SoundPlay, Text,
};
use std::cell::RefCell;

/// Server to client: the screen and its contents; client to server: `C` the settings, `T` a
/// port slot taken, `P` an inventory slot put in.
pub(super) const CHANNEL: &str = "create:package_port";
const MOD: &str = "create";

// ---------------------------------------------------------------------------------------------
// Server

/// `PackagePortBlockEntity.use` (`openMenu`), or the contents again after a click (`again`).
pub(crate) fn open(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3], again: bool) {
    let dim = lv.dim;
    let Some(port) = with(dim, |pk| pk.ports.get(&pos).cloned()) else {
        return;
    };
    if !again && port.frog {
        super::frog_draw::viewer(dim, pos, player.uuid(), true);
    }
    let mut m = vec![u8::from(again)];
    for v in pos {
        put_i32(&mut m, v);
    }
    m.push(u8::from(port.frog));
    put_str(&mut m, &port.filter);
    m.push(u8::from(port.accepts));
    m.push(u8::from(port.target.is_some()));
    for slot in &port.items {
        match slot {
            Some(b) => {
                m.push(1);
                put_stack(&*lv.host, &mut m, &b.stack, b.stack.count());
            }
            None => m.push(0),
        }
    }
    let inventory: Vec<(u32, ItemStack)> = player
        .inventory_slots(&*lv.host)
        .unwrap_or_default()
        .into_iter()
        .filter(|(i, _)| *i < 36)
        .collect();
    m.push(inventory.len() as u8);
    for (i, s) in &inventory {
        m.push(*i as u8);
        put_stack(&*lv.host, &mut m, s, s.count());
    }
    net::send_to_player(lv.host, player, CHANNEL, &m);
}

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
    if d2 > 10.0 * 10.0 {
        return;
    }
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    let dim = lv.dim;
    if with(dim, |pk| !pk.ports.contains_key(&pos)) {
        return;
    }
    match kind {
        // `PackagePortConfigurationPacket.applySettings`.
        b'C' => {
            let (Some(filter), Some(accepts)) = (r.str(), r.u8()) else {
                return;
            };
            let filter: String = filter.chars().take(25).collect();
            with(dim, |pk| {
                if let Some(p) = pk.ports.get_mut(&pos) {
                    p.filter = filter;
                    p.accepts = accepts != 0;
                }
                pk.register(pos);
                pk.wake(pos);
                pk.touch(pos);
            });
            mark_unsaved(dim, &[pos]);
            super::frog_draw::viewer(dim, pos, player.uuid(), false);
            super::station_ports::filter_changed(dim, pos);
            super::super::blocks::postbox_closed(&mut lv, pos);
        }
        b'T' => {
            let Some(slot) = r.u8().map(usize::from).filter(|s| *s < SLOTS) else {
                return;
            };
            let taken = with(dim, |pk| {
                let b = pk.ports.get_mut(&pos)?.items[slot].take();
                pk.wake(pos);
                pk.touch(pos);
                b
            });
            if let Some(b) = taken {
                let _ = player.give_stack(lv.host, b.stack);
                mark_unsaved(dim, &[pos]);
            }
            super::automation::sync(&mut lv);
            open(&mut lv, player, pos, true);
        }
        b'P' => {
            let Some(slot) = r.u8().map(u32::from).filter(|s| *s < 36) else {
                return;
            };
            let stack = player
                .inventory_slots(&*lv.host)
                .unwrap_or_default()
                .into_iter()
                .find(|(i, _)| *i == slot)
                .map(|(_, s)| s);
            // `PackagePortMenu`'s slots take packages only (`PackageItem.isPackage`).
            if let Some(stack) = stack.filter(|s| package::is_package(&body::item_id(&*lv.host, s)))
            {
                let rest = with(dim, |pk| {
                    let left = pk.ports.get_mut(&pos)?.insert(parcel(stack.clone()));
                    pk.wake(pos);
                    pk.touch(pos);
                    Some(left)
                });
                if let Some(None) = rest {
                    let _ = player.set_inventory_slot(lv.host, slot, None);
                    mark_unsaved(dim, &[pos]);
                }
            }
            super::automation::sync(&mut lv);
            open(&mut lv, player, pos, true);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------
// Client

struct Open {
    pos: [i32; 3],
    frog: bool,
    filter: String,
    editing: bool,
    accepts: bool,
    target: bool,
    items: Vec<Option<ItemStack>>,
    inventory: Vec<(u32, ItemStack)>,
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

pub(super) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let mut r = Reader(data);
    let (Some(again), Some(pos), Some(frog), Some(filter), Some(accepts), Some(target)) =
        (r.u8(), r.pos(), r.u8(), r.str(), r.u8(), r.u8())
    else {
        return true;
    };
    let mut items = Vec::with_capacity(SLOTS);
    for _ in 0..SLOTS {
        let s = match r.u8() {
            Some(1) => r.stack(&*host).and_then(|(s, n)| body::with_count(&s, n)),
            _ => None,
        };
        items.push(s);
    }
    let mut inventory = Vec::new();
    for _ in 0..r.u8().unwrap_or(0) {
        let (Some(i), Some((s, n))) = (r.u8(), r.stack(&*host)) else {
            break;
        };
        if let Some(s) = body::with_count(&s, n) {
            inventory.push((u32::from(i), s));
        }
    }
    if again != 0 {
        OPEN.with_borrow_mut(|o| {
            if let Some(o) = o.as_mut().filter(|o| o.pos == pos) {
                o.items = items;
                o.inventory = inventory;
            }
        });
        return true;
    }
    OPEN.with_borrow_mut(|o| {
        *o = Some(Open {
            pos,
            frog: frog != 0,
            filter,
            editing: false,
            accepts: accepts != 0,
            target: target != 0,
            items,
            inventory,
            size: [0.0, 0.0],
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
    true
}

fn settings(o: &Open) -> Vec<u8> {
    let mut m = vec![b'C'];
    for v in o.pos {
        put_i32(&mut m, v);
    }
    put_str(&mut m, &o.filter);
    m.push(u8::from(o.accepts));
    m
}

pub(super) fn tick(host: &mut dyn ClientHost) {
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    if close {
        host.mod_screen_close(MOD);
    }
    for m in send {
        net::send_to_server(host, CHANNEL, &m);
    }
    if host.mod_screen() != Some(MOD)
        && let Some(o) = OPEN.with_borrow_mut(Option::take)
    {
        net::send_to_server(host, CHANNEL, &settings(&o));
    }
}

fn tr(host: &dyn ClientHost, key: &str, fallback: &str) -> String {
    host.translate(key).unwrap_or_else(|| fallback.to_owned())
}

const TEX: &str = "create:gui/frogport_and_mailbox";
/// `FROGPORT_BG` over `PLAYER_INVENTORY`.
const W: f32 = 220.0;
const H: f32 = 82.0 + 108.0;

fn corner(size: [f32; 2]) -> (f32, f32) {
    (((size[0] - W) / 2.0).floor(), ((size[1] - H) / 2.0).floor())
}

fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
}

/// `PackagePortMenu.addSlots`: the port's 18 at (27, 9), the player's at (38, 108).
fn port_slot(x: f32, y: f32, i: usize) -> [f32; 4] {
    [
        x + 27.0 + (i % 9) as f32 * 18.0,
        y + 9.0 + (i / 9) as f32 * 18.0,
        16.0,
        16.0,
    ]
}

fn inventory_slots(x: f32, y: f32) -> Vec<(u32, [f32; 4])> {
    let mut out = Vec::new();
    for row in 0..3 {
        for col in 0..9 {
            let at = [x + 38.0 + col as f32 * 18.0, y + 108.0 + row as f32 * 18.0];
            out.push((9 + row * 9 + col, [at[0], at[1], 16.0, 16.0]));
        }
    }
    for col in 0..9 {
        out.push((col, [x + 38.0 + col as f32 * 18.0, y + 166.0, 16.0, 16.0]));
    }
    out
}

fn confirm_rect(x: f32, y: f32) -> [f32; 4] {
    [x + W - 33.0, y + 82.0 - 24.0, 18.0, 18.0]
}

fn mode_rect(x: f32, y: f32, send_only: bool) -> [f32; 4] {
    [
        x + 37.0 + if send_only { 18.0 } else { 0.0 },
        y + 82.0 - 24.0,
        18.0,
        18.0,
    ]
}

fn address_rect(x: f32, y: f32) -> [f32; 4] {
    [x + 23.0, y - 13.0, W - 20.0, 12.0]
}

fn icon_button(
    draw: &mut dyn DrawContext,
    r: [f32; 4],
    icon: [f32; 2],
    green: bool,
    hovered: bool,
) {
    let u = if hovered {
        18.0
    } else if green {
        72.0
    } else {
        0.0
    };
    draw.sprite("create:gui/widgets", [u, 0.0, 18.0, 18.0], r, None);
    draw.sprite(
        "create:gui/icons",
        [icon[0], icon[1], 16.0, 16.0],
        [r[0] + 1.0, r[1] + 1.0, 16.0, 16.0],
        None,
    );
}

fn item_key(host: &dyn ClientHost, id: &str) -> Option<mod_api::ItemKey> {
    mod_api::Identifier::parse(id)
        .ok()
        .and_then(|i| host.item_key(&i))
}

fn block_item(host: &dyn ClientHost, pos: [i32; 3]) -> Option<mod_api::ItemKey> {
    let state = host.level(host.dimension())?.block_state(host, pos)?;
    let text = host.block_state_string(state)?;
    item_key(host, text.split('[').next()?)
}

pub(super) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let size = event.draw.size();
    let mouse = event.screen.mouse;
    let draw = &mut *event.draw;
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        super::super::items::gadget_gui::dim(host, draw);
        o.size = size;
        let (x, y) = corner(size);
        // The header (the frog's, or the postbox's flag) above the window, then the window.
        let (hy, hh) = if o.frog { (0.0, 17.0) } else { (23.0, 24.0) };
        draw.sprite(TEX, [0.0, hy, 214.0, hh], [x, y - hh, 214.0, hh], None);
        draw.sprite(TEX, [0.0, 47.0, 220.0, 82.0], [x, y, 220.0, 82.0], None);
        // The address, or the block's name while empty, centred, with the pencil.
        let block = block_item(host, o.pos);
        let mut text = o.filter.clone();
        let color = 0xFF3D_3C48;
        if text.is_empty() && !o.editing {
            text = block.map_or_else(String::new, |k| {
                ItemStack::new(k, 1).map_or_else(String::new, |s| host.hover_name(&s))
            });
        }
        if o.editing && blink() {
            text.push('_');
        }
        let w = draw.text_width(&text).min(W - 20.0);
        let tx = (x + W / 2.0 - (w + 10.0) / 2.0).floor();
        draw.text(&text, [tx, y - 11.0], color, false);
        if !o.editing {
            draw.sprite(
                TEX,
                [230.0, 3.0, 13.0, 13.0],
                [tx + w + 5.0, y - 14.0, 13.0, 13.0],
                None,
            );
        }
        if let Some(k) = block {
            draw.item(k, [x + W + 6.0, y + 82.0 - 56.0], 64.0);
        }
        // The player's inventory below.
        let ix = x + 30.0;
        let iy = y + 8.0 + H - 108.0;
        draw.sprite(
            "create:gui/player_inventory",
            [0.0, 0.0, 176.0, 108.0],
            [ix, iy, 176.0, 108.0],
            None,
        );
        let inv = tr(host, "container.inventory", "Inventory");
        draw.text(&inv, [ix + 8.0, iy + 6.0], 0xFF40_4040, false);
        for (i, s) in o.items.iter().enumerate() {
            let r = port_slot(x, y, i);
            if let Some(s) = s {
                draw.gui_draw_item_with(s, [r[0], r[1]], None);
            }
            if inside(r, mouse) {
                draw.fill(r, 0x80FF_FFFF);
            }
        }
        for (i, r) in inventory_slots(x, y) {
            if let Some((_, s)) = o.inventory.iter().find(|(k, _)| *k == i) {
                draw.gui_draw_item_with(s, [r[0], r[1]], None);
            }
            if inside(r, mouse) {
                draw.fill(r, 0x80FF_FFFF);
            }
        }
        // Its target, and whether it receives (only with a target).
        let accept = mode_rect(x, y, false);
        let send_only = mode_rect(x, y, true);
        if o.target {
            draw.sprite(
                TEX,
                [26.0, 55.0, 18.0, 18.0],
                [x + 13.0, y + 58.0, 18.0, 18.0],
                None,
            );
            let icon = if o.frog {
                "create:chain_conveyor"
            } else {
                "create:track_station"
            };
            if let Some(k) = item_key(host, icon) {
                draw.item(k, [x + 14.0, y + 59.0], 16.0);
            }
            icon_button(
                draw,
                accept,
                [16.0, 192.0],
                o.accepts,
                inside(accept, mouse),
            );
            icon_button(
                draw,
                send_only,
                [0.0, 192.0],
                !o.accepts,
                inside(send_only, mouse),
            );
        }
        let cr = confirm_rect(x, y);
        icon_button(draw, cr, [0.0, 16.0], false, inside(cr, mouse));
        let line = |k: &str, f: &str| Text::literal(tr(host, k, f));
        let tip = if inside(address_rect(x, y), mouse) {
            Some(vec![
                line(
                    "create.gui.package_port.catch_packages",
                    "Catch packages addressed to...",
                ),
                line(
                    "create.gui.package_port.catch_packages_empty",
                    "Leave empty to match non-addressed",
                ),
                line(
                    "create.gui.package_port.catch_packages_wildcard",
                    "Use * as a text wildcard",
                ),
            ])
        } else if o.target && inside(accept, mouse) {
            Some(vec![line(
                "create.gui.package_port.send_and_receive",
                "Send and receive packages",
            )])
        } else if o.target && inside(send_only, mouse) {
            Some(vec![line(
                "create.gui.package_port.send_only",
                "Only send packages",
            )])
        } else {
            let hovered = (0..SLOTS)
                .find(|&i| inside(port_slot(x, y, i), mouse))
                .and_then(|i| o.items[i].clone())
                .or_else(|| {
                    inventory_slots(x, y)
                        .into_iter()
                        .find(|(_, r)| inside(*r, mouse))
                        .and_then(|(i, _)| {
                            o.inventory
                                .iter()
                                .find(|(k, _)| *k == i)
                                .map(|e| e.1.clone())
                        })
                });
            hovered.map(|s| {
                let mut lines = vec![Text::literal(host.hover_name(&s))];
                let to = body::address(&s);
                if !to.is_empty() {
                    lines.push(Text::literal(format!("\u{2192} {to}")));
                }
                lines
            })
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

fn click(host: &dyn ClientHost) {
    host.play_local_sound(
        &SoundPlay::ui("minecraft:ui.button.click").with_volume(0.25),
        0,
    );
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
        let Some(o) = o.as_mut() else {
            return;
        };
        let (x, y) = corner(o.size);
        o.editing = inside(address_rect(x, y), mouse);
        if o.editing {
            return;
        }
        if inside(confirm_rect(x, y), mouse) {
            click(host);
            LATER.with_borrow_mut(|l| l.1 = true);
            return;
        }
        if o.target && inside(mode_rect(x, y, false), mouse) {
            click(host);
            o.accepts = true;
            return;
        }
        if o.target && inside(mode_rect(x, y, true), mouse) {
            click(host);
            o.accepts = false;
            return;
        }
        let mut msg = None;
        if let Some(i) = (0..SLOTS).find(|&i| inside(port_slot(x, y, i), mouse))
            && o.items[i].is_some()
        {
            msg = Some((b'T', i as u8));
        } else if let Some((i, _)) = inventory_slots(x, y)
            .into_iter()
            .find(|(_, r)| inside(*r, mouse))
            && o.inventory.iter().any(|(k, _)| *k == i)
        {
            msg = Some((b'P', i as u8));
        }
        if let Some((kind, slot)) = msg {
            let mut m = vec![kind];
            for v in o.pos {
                put_i32(&mut m, v);
            }
            m.push(slot);
            LATER.with_borrow_mut(|l| l.0.push(m));
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
    let editing = OPEN.with_borrow(|o| o.as_ref().is_some_and(|o| o.editing));
    if !editing {
        return if event.key == "key.keyboard.e" {
            LATER.with_borrow_mut(|l| l.1 = true);
            Flow::Cancel
        } else {
            Flow::Continue
        };
    }
    match event.key {
        "key.keyboard.backspace" => OPEN.with_borrow_mut(|o| {
            if let Some(o) = o.as_mut() {
                o.filter.pop();
            }
        }),
        "key.keyboard.enter" | "key.keyboard.keypad.enter" => OPEN.with_borrow_mut(|o| {
            if let Some(o) = o.as_mut() {
                o.editing = false;
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
        if let Some(o) = o.as_mut()
            && o.editing
            && o.filter.chars().count() < 25
        {
            o.filter.push(c);
        }
    });
    Flow::Cancel
}

/// `removed`: the address and mode go to the server at the next tick.
pub(super) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(o) = OPEN.with_borrow_mut(Option::take) {
        LATER.with_borrow_mut(|l| l.0.push(settings(&o)));
    }
}
