//! The linked controller's frequency screen (`LinkedControllerScreen` over `LinkedControllerMenu`,
//! a `HeldItemGhostItemMenu`): Create's panel with six buttons' two frequency slots each, reset
//! and confirm, the player's inventory under it and the held controller five times its size
//! beside it. A sneaking use with the controller in the main hand opens it
//! (`LinkedControllerItem.use`); what was chosen goes back when it closes (`saveData`), at the
//! client's next tick, and the server writes it on the controller still in that hand.
//!
//! The frequencies are kept as item ids (the port's link frequencies), so a slot shows its item
//! without the components the stack it was copied from had.
//!
//! Left out (no API for it), as for the filters' screens (`filter_screen`): an item-opened mod
//! menu has no screen layout, so this is the mod's own screen, and clicking an inventory slot puts
//! a ghost copy of its stack on the cursor; a slot's tooltip is the item's name, not its tooltip
//! lines.

use super::super::net;
use super::filter_screen::{click_ghost, click_inventory, stack_bytes, stack_of};
use mod_api::{
    ClientHost, DrawContext, Flow, Hand, Host, ItemKey, ItemStack, PlayerRef, ScreenClosingEvent,
    ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent, ScreenMouseButtonReleasedPreEvent,
    ScreenRenderPostEvent, ServerHost, SoundPlay, Text,
};
use std::cell::RefCell;

/// Server to client: open the screen; client to server: the frequencies chosen.
pub(crate) const CHANNEL: &str = "create:controller_screen";
const MOD: &str = "create";
/// Six buttons of two frequencies.
pub(crate) const SLOTS: usize = 12;

// ---------------------------------------------------------------------------------------------
// The wire

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    let n = b.len().min(usize::from(u16::MAX));
    out.extend_from_slice(&(n as u16).to_le_bytes());
    out.extend_from_slice(&b[..n]);
}

fn take<'a>(data: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    let (a, b) = (data.get(..n)?, data.get(n..)?);
    *data = b;
    Some(a)
}

fn take_bytes<'a>(data: &mut &'a [u8]) -> Option<&'a [u8]> {
    let n = take(data, 2).map(|b| u16::from_le_bytes([b[0], b[1]]))?;
    take(data, usize::from(n))
}

/// The twelve frequencies (item ids, empty for none).
fn encode_items(out: &mut Vec<u8>, items: &[String]) {
    for i in 0..SLOTS {
        put_bytes(out, items.get(i).map_or("", String::as_str).as_bytes());
    }
}

fn decode_items(data: &mut &[u8]) -> Option<Vec<String>> {
    (0..SLOTS)
        .map(|_| String::from_utf8(take_bytes(data)?.to_vec()).ok())
        .collect()
}

/// `player.openMenu(controller)`: the screen, with the controller's frequencies and a copy of
/// the player's inventory.
pub(crate) fn open(host: &mut dyn ServerHost, player: PlayerRef, items: &[String]) {
    let h: &dyn Host = &*host;
    let mut out = vec![player.selected_slot(h).unwrap_or(0) as u8];
    let inventory: Vec<(u8, Vec<u8>)> = player
        .inventory_slots(h)
        .unwrap_or_default()
        .into_iter()
        .filter(|(slot, _)| *slot < 36)
        .filter_map(|(slot, s)| Some((slot as u8, stack_bytes(h, &s)?)))
        .collect();
    out.push(inventory.len() as u8);
    for (slot, bytes) in &inventory {
        out.push(*slot);
        put_bytes(&mut out, bytes);
    }
    encode_items(&mut out, items);
    net::send_to_player(host, player, CHANNEL, &out);
}

/// The client's choice: the twelve frequencies.
pub(crate) fn chosen(data: &[u8]) -> Option<Vec<String>> {
    let mut d = data;
    decode_items(&mut d)
}

// ---------------------------------------------------------------------------------------------
// The layout (`AllGuiTextures.LINKED_CONTROLLER`, `LinkedControllerMenu.addSlots`, the screen's
// `init`)

/// `LINKED_CONTROLLER` in `curiosities_2.png`.
const BG: [f32; 4] = [0.0, 0.0, 179.0, 109.0];
const INVENTORY_SIZE: [f32; 2] = [176.0, 108.0];
/// `setWindowOffset(1, 0)`.
const OFFSET: [f32; 2] = [1.0, 0.0];
/// `addPlayerSlots(8, 131)`.
const INVENTORY: [f32; 2] = [8.0, 131.0];

fn window() -> [f32; 2] {
    [
        BG[2].max(INVENTORY_SIZE[0]),
        BG[3] + 4.0 + INVENTORY_SIZE[1],
    ]
}

fn corner(size: [f32; 2]) -> [f32; 2] {
    let w = window();
    [
        ((size[0] - w[0]) / 2.0).floor() + OFFSET[0],
        ((size[1] - w[1]) / 2.0).floor() + OFFSET[1],
    ]
}

/// A ghost slot's top left: six columns of two, 24 apart, 11 more after the fourth.
pub(crate) fn slot_at(i: usize) -> [f32; 2] {
    let (column, row) = (i / 2, i % 2);
    let x = 12.0 + column as f32 * 24.0 + if column > 3 { 11.0 } else { 0.0 };
    [x, 34.0 + row as f32 * 18.0]
}

fn inventory_slot(c: [f32; 2], slot: u8) -> [f32; 2] {
    let [x, y] = [c[0] + INVENTORY[0], c[1] + INVENTORY[1]];
    let s = f32::from(slot);
    if slot < 9 {
        [x + s * 18.0, y + 58.0]
    } else {
        let i = s - 9.0;
        [x + (i % 9.0) * 18.0, y + (i / 9.0).floor() * 18.0]
    }
}

fn reset_button(c: [f32; 2]) -> [f32; 4] {
    [c[0] + BG[2] - 62.0, c[1] + BG[3] - 24.0, 18.0, 18.0]
}

fn confirm_button(c: [f32; 2]) -> [f32; 4] {
    [c[0] + BG[2] - 33.0, c[1] + BG[3] - 24.0, 18.0, 18.0]
}

fn inside(r: [f32; 4], p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
}

fn over_slot(at: [f32; 2], p: [f32; 2]) -> bool {
    inside([at[0] - 1.0, at[1] - 1.0, 18.0, 18.0], p)
}

/// `ControlsUtil.getControls()`' mappings, button by button.
const MAPPINGS: [&str; 6] = [
    "key.forward",
    "key.back",
    "key.left",
    "key.right",
    "key.jump",
    "key.sneak",
];

// ---------------------------------------------------------------------------------------------
// The client

struct Open {
    selected: u8,
    /// The controller's item: the screen closes when the main hand holds another.
    item: ItemKey,
    inventory: Vec<(u8, ItemStack)>,
    /// Each slot's frequency and the stack it shows.
    slots: Vec<Option<(String, ItemStack)>>,
    cursor: Option<u8>,
    size: [f32; 2],
    pressed: bool,
}

impl Open {
    fn stack(&self, slot: u8) -> Option<&ItemStack> {
        self.inventory
            .iter()
            .find(|(s, _)| *s == slot)
            .map(|(_, st)| st)
    }

    fn chosen(&self) -> Vec<u8> {
        let items: Vec<String> = self
            .slots
            .iter()
            .map(|s| s.as_ref().map(|(i, _)| i.clone()).unwrap_or_default())
            .collect();
        let mut out = Vec::new();
        encode_items(&mut out, &items);
        out
    }
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    /// What the screen hooks leave for the next tick: choices to send, and whether to close.
    static LATER: RefCell<(Vec<Vec<u8>>, bool)> = const { RefCell::new((Vec::new(), false)) };
}

pub(crate) fn reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = (Vec::new(), false));
}

fn main_hand(host: &dyn ClientHost) -> Option<ItemStack> {
    let player = host.local_player()?;
    host.living(player.entity())?.hand_item(host, Hand::Main)
}

fn item_stack(host: &dyn Host, id: &str) -> Option<ItemStack> {
    let key = host.item_key(&mod_api::Identifier::parse(id).ok()?)?;
    Some(ItemStack::of(key))
}

/// The server opened the screen.
pub(crate) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let decoded = (|| {
        let mut d = data;
        let selected = take(&mut d, 1)?[0];
        let n = take(&mut d, 1)?[0];
        let mut inventory = Vec::new();
        for _ in 0..n {
            let slot = take(&mut d, 1)?[0];
            let bytes = take_bytes(&mut d)?;
            if let Some(s) = stack_of(&*host, bytes) {
                inventory.push((slot, s));
            }
        }
        Some((selected, inventory, decode_items(&mut d)?))
    })();
    let Some((selected, inventory, items)) = decoded else {
        return true;
    };
    let Some(item) = main_hand(&*host).map(|s| s.item()) else {
        return true;
    };
    let slots = items
        .into_iter()
        .map(|i| {
            let s = item_stack(&*host, &i)?;
            Some((i, s))
        })
        .collect();
    OPEN.with_borrow_mut(|x| {
        *x = Some(Open {
            selected,
            item,
            inventory,
            slots,
            cursor: None,
            size: [0.0, 0.0],
            pressed: false,
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|x| *x = None);
    }
    true
}

fn close_later() {
    if let Some(open) = OPEN.with_borrow_mut(Option::take) {
        LATER.with_borrow_mut(|l| {
            l.0.push(open.chosen());
            l.1 = true;
        });
    }
}

pub(crate) fn tick(host: &mut dyn ClientHost) {
    // `containerTick`: the screen closes when the main hand no longer holds the controller.
    let item = OPEN.with_borrow(|o| o.as_ref().map(|o| o.item));
    if let Some(item) = item
        && host.mod_screen() == Some(MOD)
        && main_hand(&*host).map(|s| s.item()) != Some(item)
    {
        close_later();
    }
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    if close {
        host.mod_screen_close(MOD);
    }
    for c in send {
        net::send_to_server(host, CHANNEL, &c);
    }
    if host.mod_screen() != Some(MOD)
        && let Some(open) = OPEN.with_borrow_mut(Option::take)
    {
        net::send_to_server(host, CHANNEL, &open.chosen());
    }
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

fn tr(host: &dyn ClientHost, key: &str) -> String {
    host.translate(key).unwrap_or_else(|| key.to_owned())
}

pub(crate) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let size = event.draw.size();
    let mouse = event.screen.mouse;
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.as_mut() {
            o.size = size;
            draw(host, event.draw, o, mouse);
        }
    });
}

/// `LinkedControllerScreen.renderBg`, its slots and buttons, the cursor and the tooltips.
fn draw(host: &dyn ClientHost, draw: &mut dyn DrawContext, o: &Open, mouse: [f32; 2]) {
    let c = corner(draw.size());
    super::gadget_gui::dim(host, draw);
    // `renderPlayerInventory` at `getLeftOfCentered`, under the background.
    let p = [
        c[0] - OFFSET[0] + ((window()[0] - INVENTORY_SIZE[0]) / 2.0).floor(),
        c[1] + BG[3] + 4.0,
    ];
    draw.sprite(
        "create:gui/player_inventory",
        [0.0, 0.0, INVENTORY_SIZE[0], INVENTORY_SIZE[1]],
        [p[0], p[1], INVENTORY_SIZE[0], INVENTORY_SIZE[1]],
        None,
    );
    draw.text(
        &tr(host, "container.inventory"),
        [p[0] + 8.0, p[1] + 6.0],
        0xFF40_4040,
        false,
    );
    draw.sprite(
        "create:gui/curiosities_2",
        BG,
        [c[0], c[1], BG[2], BG[3]],
        None,
    );
    // The title (the controller's name) at (15, 4).
    let title = main_hand(host).map_or_else(
        || tr(host, "item.create.linked_controller"),
        |s| host.hover_name(&s),
    );
    draw.text(&title, [c[0] + 15.0, c[1] + 4.0], 0xFF59_2424, false);
    // The held controller, five times its size.
    if let Some(held) = main_hand(host) {
        draw.item_stack(&held, [c[0] + BG[2] - 4.0, c[1] + BG[3] - 56.0], 80.0);
    }
    // Reset (`I_TRASH`) and confirm (`I_CONFIRM`).
    for (r, icon) in [
        (reset_button(c), [16.0, 0.0]),
        (confirm_button(c), [0.0, 16.0]),
    ] {
        let hovered = inside(r, mouse);
        let u = if hovered && o.pressed {
            36.0
        } else if hovered {
            18.0
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
    let mut hover: Option<Vec<Text>> = None;
    for (slot, stack) in &o.inventory {
        let at = inventory_slot(c, *slot);
        let n = stack.count();
        let count = (n > 1).then(|| n.to_string());
        draw.gui_draw_item_with(stack, at, count.as_deref());
    }
    for slot in 0..36u8 {
        let at = inventory_slot(c, slot);
        if over_slot(at, mouse) {
            draw.fill([at[0], at[1], 16.0, 16.0], 0x80FF_FFFF);
            if o.cursor.is_none()
                && let Some(s) = o.stack(slot)
            {
                hover = Some(vec![Text::literal(host.hover_name(s))]);
            }
        }
    }
    for (i, s) in o.slots.iter().enumerate() {
        let r = slot_at(i);
        let at = [c[0] + r[0], c[1] + r[1]];
        if let Some((_, s)) = s {
            draw.item_stack(s, at, 16.0);
        }
        if over_slot(at, mouse) {
            draw.fill([at[0], at[1], 16.0, 16.0], 0x80FF_FFFF);
            if o.cursor.is_none() {
                // `addToTooltip`: the frequency slot and its button's key, in gold.
                let mut lines = Vec::new();
                if let Some((_, s)) = s {
                    lines.push(Text::literal(host.hover_name(s)));
                }
                let key = host
                    .mapping(MAPPINGS[i / 2])
                    .ok()
                    .flatten()
                    .map(|m| m.label)
                    .unwrap_or_else(|| {
                        Text::translate(create_engine::controller::KEYS[i / 2], Vec::new())
                    });
                let mut style = mod_api::text::TextStyle::default();
                style.color = Some("gold".to_owned());
                lines.push(
                    Text::translate(
                        format!("create.linked_controller.frequency_slot_{}", i % 2 + 1),
                        vec![key],
                    )
                    .with_style(style),
                );
                hover = Some(lines);
            }
        }
    }
    if let Some(s) = o.cursor.and_then(|slot| o.stack(slot)) {
        draw.item_stack(s, [mouse[0] - 8.0, mouse[1] - 8.0], 16.0);
    }
    if let Some(lines) = hover {
        let _ = draw.gui_tooltip(&lines, mouse);
    }
}

fn click_sound(host: &dyn ClientHost) {
    host.play_local_sound(
        &SoundPlay::ui("minecraft:ui.button.click").with_volume(0.25),
        0,
    );
}

/// A click: the buttons, then the inventory (a ghost on the cursor, or a shift-click's copy in
/// the first empty slot: repeats are allowed), then the frequency slots. True to close.
fn click(host: &dyn ClientHost, o: &mut Open, mouse: [f32; 2], button: i32, shift: bool) -> bool {
    let c = corner(o.size);
    if button == 0 {
        if inside(reset_button(c), mouse) {
            click_sound(host);
            o.slots.iter_mut().for_each(|s| *s = None);
            return false;
        }
        if inside(confirm_button(c), mouse) {
            click_sound(host);
            return true;
        }
    }
    if button > 1 {
        return false;
    }
    for slot in 0..36u8 {
        if !over_slot(inventory_slot(c, slot), mouse) {
            continue;
        }
        let stack = o.stack(slot).cloned();
        if shift {
            // `quickMoveStack` with repeats allowed: into the first empty slot.
            if let Some(s) = stack
                && slot != o.selected
                && let Some(free) = o.slots.iter_mut().find(|s| s.is_none())
            {
                *free = Some((s.item().id(host), ItemStack::of(s.item())));
            }
        } else {
            o.cursor = click_inventory(o.cursor, slot, stack.is_some(), o.selected);
        }
        return false;
    }
    let carried = o
        .cursor
        .and_then(|slot| o.stack(slot))
        .map(|s| (s.item().id(host), ItemStack::of(s.item())));
    for i in 0..SLOTS {
        let r = slot_at(i);
        if over_slot([c[0] + r[0], c[1] + r[1]], mouse) {
            let carried = if shift { None } else { carried.as_ref() };
            click_ghost(&mut o.slots, i, carried);
            return false;
        }
    }
    let w = window();
    if !inside([c[0], c[1], w[0], w[1]], mouse) {
        o.cursor = None;
    }
    false
}

pub(crate) fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let shift = host.modifiers().shift();
    let mouse = event.screen.mouse;
    let button = event.button;
    let close = OPEN.with_borrow_mut(|o| {
        o.as_mut().is_some_and(|o| {
            o.pressed = button == 0;
            click(host, o, mouse, button, shift)
        })
    });
    if close {
        close_later();
    }
    Flow::Cancel
}

pub(crate) fn screen_mouse_released(
    host: &dyn ClientHost,
    _event: &mut ScreenMouseButtonReleasedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.as_mut() {
            o.pressed = false;
        }
    });
    Flow::Cancel
}

/// The inventory key closes it, as a container's does (Escape is the game's own).
pub(crate) fn screen_key_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenKeyPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    if event.key == "key.keyboard.e" {
        close_later();
        return Flow::Cancel;
    }
    Flow::Continue
}

/// `removed` / `saveData`: the frequencies go to the server at the next tick.
pub(crate) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(open) = OPEN.with_borrow_mut(Option::take) {
        LATER.with_borrow_mut(|l| l.0.push(open.chosen()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_sit_in_six_pairs_with_a_gap_after_the_fourth() {
        // `LinkedControllerMenu.addSlots`: x 12, +24 a column, +11 after column 3; rows 18 apart.
        assert_eq!(slot_at(0), [12.0, 34.0]);
        assert_eq!(slot_at(1), [12.0, 52.0]);
        assert_eq!(slot_at(7), [84.0, 52.0]);
        assert_eq!(slot_at(8), [119.0, 34.0]);
        assert_eq!(slot_at(11), [143.0, 52.0]);
        let mut out = Vec::new();
        let items: Vec<String> = (0..SLOTS)
            .map(|i| {
                if i % 3 == 0 {
                    format!("minecraft:item{i}")
                } else {
                    String::new()
                }
            })
            .collect();
        encode_items(&mut out, &items);
        assert_eq!(chosen(&out), Some(items));
    }
}
