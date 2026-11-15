//! The redstone requester's screen (`RedstoneRequesterScreen`): its order's nine ghost slots
//! with their amounts (the wheel changes one, shift by ten), the address it sends to, and
//! whether partial orders go out. The menu's player inventory is drawn by the screen: a click on
//! one of its stacks picks it up as a ghost (nothing moves), a click on an order slot puts that
//! ghost there (an empty one clears it), as `SorterProofSlot`s take copies. Closing sends the
//! settings (`RedstoneRequesterConfigurationPacket`).

use super::super::net;
use super::super::server::ServerLevel;
use super::keeper::{Reader, put_i32, put_stack, put_str};
use super::{Key, with};
use create_engine::stock::Order;
use mod_api::{
    ClientHost, DrawContext, Flow, ItemStack, PlayerRef, ScreenCharTypedPreEvent,
    ScreenClosingEvent, ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent,
    ScreenMouseScrolledPreEvent, ScreenRenderPostEvent, ServerHost, SoundPlay, Text,
};
use std::cell::RefCell;

/// Server to client: open the screen; client to server: the settings.
pub(super) const CHANNEL: &str = "create:redstone_requester";
const MOD: &str = "create";
const SLOTS: usize = 9;

// ---------------------------------------------------------------------------------------------
// Server

/// `RedstoneRequesterBlockEntity.use`: the screen opens with what the requester holds.
pub(super) fn open(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(r) = with(dim, |n| n.requesters.get(&pos).cloned()) else {
        return;
    };
    let mut m = Vec::new();
    for v in pos {
        put_i32(&mut m, v);
    }
    put_str(&mut m, &r.r.address);
    m.push(u8::from(r.r.allow_partial));
    let stacks = &r.r.order.stacks;
    m.push(stacks.len().min(SLOTS) as u8);
    for (k, n) in stacks.iter().take(SLOTS) {
        put_stack(&*lv.host, &mut m, &k.0, *n);
    }
    // The player's inventory by slot (a client knows its stacks but not their slots), for the
    // screen's inventory to pick ghosts from.
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

/// `RedstoneRequesterConfigurationPacket.applySettings`.
pub(super) fn client_message(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let mut r = Reader(data);
    let Some(pos) = r.pos() else {
        return;
    };
    let (Some(address), Some(partial), Some(n)) = (r.str(), r.u8(), r.u8()) else {
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
    let Some(lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    let mut stacks = Vec::new();
    for _ in 0..n.min(SLOTS as u8) {
        let Some((s, count)) = r.stack(&*lv.host) else {
            return;
        };
        stacks.push((Key::of(&s), count.clamp(1, 256)));
    }
    let dim = lv.dim;
    with(dim, |net| {
        if let Some(q) = net.requesters.get_mut(&pos) {
            q.r.address = address;
            q.r.allow_partial = partial != 0;
            q.r.order = Order::simple(stacks);
            net.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        }
    });
}

// ---------------------------------------------------------------------------------------------
// Client

struct Open {
    pos: [i32; 3],
    address: String,
    editing: bool,
    partial: bool,
    slots: [Option<ItemStack>; SLOTS],
    amounts: [u32; SLOTS],
    /// The ghost picked up from the inventory.
    carried: Option<ItemStack>,
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
    let (Some(pos), Some(address), Some(partial), Some(n)) = (r.pos(), r.str(), r.u8(), r.u8())
    else {
        return true;
    };
    let mut slots: [Option<ItemStack>; SLOTS] = Default::default();
    let mut amounts = [1; SLOTS];
    for i in 0..usize::from(n).min(SLOTS) {
        let Some((s, count)) = r.stack(&*host) else {
            break;
        };
        slots[i] = Some(s);
        amounts[i] = count.max(1);
    }
    let mut inventory = Vec::new();
    for _ in 0..r.u8().unwrap_or(0) {
        let (Some(i), Some((s, n))) = (r.u8(), r.stack(&*host)) else {
            break;
        };
        if let Some(s) = super::body::with_count(&s, n) {
            inventory.push((u32::from(i), s));
        }
    }
    OPEN.with_borrow_mut(|o| {
        *o = Some(Open {
            pos,
            address,
            editing: false,
            partial: partial != 0,
            slots,
            amounts,
            carried: None,
            inventory,
            size: [0.0, 0.0],
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
    true
}

fn settings(host: &dyn ClientHost, o: &Open) -> Vec<u8> {
    let mut m = Vec::new();
    for v in o.pos {
        put_i32(&mut m, v);
    }
    put_str(&mut m, &o.address);
    m.push(u8::from(o.partial));
    let filled: Vec<(usize, &ItemStack)> = o
        .slots
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.as_ref().map(|s| (i, s)))
        .collect();
    m.push(filled.len() as u8);
    for (i, s) in filled {
        put_stack(host, &mut m, s, o.amounts[i]);
    }
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
        let m = settings(&*host, &o);
        net::send_to_server(host, CHANNEL, &m);
    }
}

fn tr(host: &dyn ClientHost, key: &str, fallback: &str) -> String {
    host.translate(key).unwrap_or_else(|| fallback.to_owned())
}

/// `REDSTONE_REQUESTER` (232 × 120) over `PLAYER_INVENTORY` (176 × 108).
const W: f32 = 232.0;
const H: f32 = 228.0;

fn corner(size: [f32; 2]) -> (f32, f32) {
    (((size[0] - W) / 2.0).floor(), ((size[1] - H) / 2.0).floor())
}

fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
}

fn slot_rect(x: f32, y: f32, i: usize) -> [f32; 4] {
    [x + 27.0 + i as f32 * 20.0, y + 28.0, 16.0, 16.0]
}

/// The player inventory's slots as `MenuBase.addPlayerSlots(5, 142)` lays them out: inventory
/// index and rectangle.
fn inventory_slots(x: f32, y: f32) -> Vec<(u32, [f32; 4])> {
    let mut out = Vec::new();
    for row in 0..3 {
        for col in 0..9 {
            let at = [x + 5.0 + col as f32 * 18.0, y + 142.0 + row as f32 * 18.0];
            out.push((9 + row * 9 + col, [at[0], at[1], 16.0, 16.0]));
        }
    }
    for col in 0..9 {
        out.push((col, [x + 5.0 + col as f32 * 18.0, y + 200.0, 16.0, 16.0]));
    }
    out
}

fn confirm_rect(x: f32, y: f32) -> [f32; 4] {
    [x + W - 30.0, y + 120.0 - 25.0, 18.0, 18.0]
}

fn partial_rect(x: f32, y: f32, full: bool) -> [f32; 4] {
    [
        x + 12.0 + if full { 18.0 } else { 0.0 },
        y + 120.0 - 25.0,
        18.0,
        18.0,
    ]
}

fn address_rect(x: f32, y: f32) -> [f32; 4] {
    [x + 55.0, y + 66.0, 110.0, 12.0]
}

fn player_stacks(o: &Open) -> Vec<(u32, ItemStack)> {
    o.inventory.clone()
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
        draw.sprite(
            "create:gui/requester",
            [16.0, 16.0, 232.0, 120.0],
            [x + 3.0, y, 232.0, 120.0],
            None,
        );
        draw.sprite(
            "create:gui/player_inventory",
            [0.0, 0.0, 176.0, 108.0],
            [x - 3.0, y + 124.0, 176.0, 108.0],
            None,
        );
        let inv = tr(host, "container.inventory", "Inventory");
        draw.text(&inv, [x + 5.0, y + 130.0], 0xFF40_4040, false);
        let title = tr(
            host,
            "block.create.redstone_requester",
            "Redstone Requester",
        );
        let w = draw.text_width(&title);
        draw.text(&title, [x + 117.0 - w / 2.0, y + 4.0], 0xFF3D_3C48, false);
        if let Some(item) = mod_api::Identifier::parse("create:redstone_requester")
            .ok()
            .and_then(|i| host.item_key(&i))
        {
            draw.item(item, [x + 245.0, y + 80.0], 48.0);
        }
        // The order's ghosts and their amounts.
        for i in 0..SLOTS {
            let r = slot_rect(x, y, i);
            if let Some(s) = &o.slots[i] {
                draw.gui_draw_item_with(s, [r[0], r[1]], Some(&o.amounts[i].to_string()));
            }
            if inside(r, mouse) {
                draw.fill(r, 0x80FF_FFFF);
            }
        }
        // The address box.
        let a = address_rect(x, y);
        let mut text = o.address.clone();
        if o.editing && blink() {
            text.push('_');
        }
        draw.text(&text, [a[0], a[1] + 2.0], 0xFF55_5555, false);
        // Partial or full orders, and confirm.
        let pr = partial_rect(x, y, false);
        let fr = partial_rect(x, y, true);
        icon_button(draw, pr, [32.0, 192.0], o.partial, inside(pr, mouse));
        icon_button(draw, fr, [48.0, 192.0], !o.partial, inside(fr, mouse));
        let cr = confirm_rect(x, y);
        icon_button(draw, cr, [0.0, 16.0], false, inside(cr, mouse));
        // The player's inventory, and the ghost picked up at the cursor.
        for (i, r) in inventory_slots(x, y) {
            if let Some((_, s)) = player_stacks(o).into_iter().find(|(k, _)| *k == i) {
                draw.gui_draw_item_with(&s, [r[0], r[1]], None);
            }
            if inside(r, mouse) {
                draw.fill(r, 0x80FF_FFFF);
            }
        }
        if let Some(c) = &o.carried {
            draw.item_stack(c, [mouse[0] - 8.0, mouse[1] - 8.0], 16.0);
        }
        // Tooltips: the order's slots (`getTooltipFromContainerItem`), the buttons, the address.
        let line = |k: &str, f: &str| Text::literal(tr(host, k, f));
        let tip: Option<Vec<Text>> = (0..SLOTS)
            .find(|&i| inside(slot_rect(x, y, i), mouse))
            .and_then(|i| o.slots[i].as_ref().map(|s| (i, s)))
            .map(|(i, s)| {
                let name = format!("{} x{}", host.hover_name(s), o.amounts[i]);
                let send = tr(host, "create.gui.factory_panel.send_item", "Send %1$s")
                    .replace("%1$s", &name)
                    .replace("%s", &name);
                vec![
                    Text::literal(send),
                    line(
                        "create.gui.factory_panel.scroll_to_change_amount",
                        "Scroll to change amount",
                    ),
                    line(
                        "create.gui.scrollInput.shiftScrollsFaster",
                        "Shift to Scroll Faster",
                    ),
                ]
            })
            .or_else(|| {
                inside(pr, mouse).then(|| {
                    vec![line(
                        "create.gui.redstone_requester.allow_partial",
                        "Allow partial orders",
                    )]
                })
            })
            .or_else(|| {
                inside(fr, mouse).then(|| {
                    vec![line(
                        "create.gui.redstone_requester.dont_allow_partial",
                        "Must send all items",
                    )]
                })
            })
            .or_else(|| {
                (inside(a, mouse) && !o.editing).then(|| {
                    if o.address.trim().is_empty() {
                        vec![
                            line(
                                "create.gui.redstone_requester.requester_address",
                                "Send order to...",
                            ),
                            line(
                                "create.gui.redstone_requester.requester_address_tip",
                                "Enter the address this",
                            ),
                            line(
                                "create.gui.redstone_requester.requester_address_tip_1",
                                "order should be send to.",
                            ),
                        ]
                    } else {
                        vec![
                            line(
                                "create.gui.redstone_requester.requester_address_given",
                                "Sending order to",
                            ),
                            Text::literal(format!("'{}'", o.address)),
                        ]
                    }
                })
            });
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
        if inside(partial_rect(x, y, false), mouse) {
            click(host);
            o.partial = true;
            return;
        }
        if inside(partial_rect(x, y, true), mouse) {
            click(host);
            o.partial = false;
            return;
        }
        if let Some(i) = (0..SLOTS).find(|&i| inside(slot_rect(x, y, i), mouse)) {
            // A ghost slot takes a copy of what is carried, or empties.
            o.slots[i] = o
                .carried
                .as_ref()
                .and_then(|c| super::body::with_count(c, 1));
            o.amounts[i] = 1;
            return;
        }
        if let Some((i, _)) = inventory_slots(x, y)
            .into_iter()
            .find(|(_, r)| inside(*r, mouse))
        {
            let stack = player_stacks(o)
                .into_iter()
                .find(|(k, _)| *k == i)
                .map(|(_, s)| s);
            o.carried = if o.carried.is_some() { None } else { stack };
            return;
        }
        o.carried = None;
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
    let step = if event.modifiers & 1 != 0 { 10 } else { 1 };
    let up = event.scroll[1] > 0.0;
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        let (x, y) = corner(o.size);
        if let Some(i) = (0..SLOTS).find(|&i| inside(slot_rect(x, y, i), mouse))
            && o.slots[i].is_some()
        {
            let v = i64::from(o.amounts[i]) + if up { step } else { -step };
            o.amounts[i] = v.clamp(1, 256) as u32;
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
                o.address.pop();
            }
        }),
        "key.keyboard.enter" | "key.keyboard.keypad.enter" | "key.keyboard.tab" => OPEN
            .with_borrow_mut(|o| {
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
            && o.address.chars().count() < 50
        {
            o.address.push(c);
        }
    });
    Flow::Cancel
}

/// `removed`: the settings go to the server at the next tick.
pub(super) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(o) = OPEN.with_borrow_mut(Option::take) {
        let m = settings(host, &o);
        LATER.with_borrow_mut(|l| l.0.push(m));
    }
}
