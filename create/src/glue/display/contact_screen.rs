//! The elevator contact's screen (`ElevatorContactScreen`): its floor's short name (at most 4
//! characters) and description (30), and its doors' setting (`DoorControl.createWidget`). A
//! click on the contact (anything but the wrench) opens it from the server with what the
//! contact holds (Create's client reads its own block entity); Enter or the confirm button sends
//! the names and setting back (`ElevatorContactEditPacket`, from within 20 blocks) and closes it,
//! right after the key or click (`Subsystem::input_handled`: the screen hooks get a read-only
//! client). Escape closes it without
//! sending.

use crate::glue::net;
use crate::glue::server::ServerLevel;
use crate::glue::trains::gui::{
    Reader, allowed, click_sound, icon_button, inside, millis, put_pos, put_str, tr,
};
use crate::glue::world::with_world;
use create_engine::Block;
use create_engine::lifts::DOOR_CONTROLS;
use mod_api::text::TextStyle;
use mod_api::{
    ClientHost, Flow, PlayerRef, ScreenCharTypedPreEvent, ScreenClosingEvent,
    ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent, ScreenMouseScrolledPreEvent,
    ScreenRenderPostEvent, ServerHost, Text,
};
use std::cell::RefCell;

/// Server to client: the screen opened; client to server: the edit.
pub(super) const CHANNEL: &str = "create:elevator_contact";
const MOD: &str = "create";

/// `AllGuiTextures.ELEVATOR_CONTACT` in `display_link.png`; the window is 30 wider
/// (`setWindowSize(background.getWidth() + 30, ..)`).
const SHEET: &str = "create:gui/display_link";
const BG: [f32; 4] = [20.0, 172.0, 233.0, 82.0];
const WINDOW: [f32; 2] = [263.0, 82.0];
/// `AllIcons.I_CONFIRM`.
const I_CONFIRM: [f32; 2] = [0.0, 16.0];
/// `editBox(33, 30, 4)` and `editBox(63, 140, 30)`: the boxes' max lengths.
const SHORT_MAX: usize = 4;
const LONG_MAX: usize = 30;

/// What the screen edits.
#[derive(Clone, Debug, Default, PartialEq)]
struct View {
    pos: [i32; 3],
    short: String,
    long: String,
    door: u8,
}

impl View {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_pos(&mut out, self.pos);
        put_str(&mut out, &self.short);
        put_str(&mut out, &self.long);
        out.push(self.door);
        out
    }

    fn decode(data: &[u8]) -> Option<View> {
        let mut r = Reader(data);
        Some(View {
            pos: r.pos()?,
            short: r.str()?,
            long: r.str()?,
            door: r.u8()?,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Server

/// `ElevatorContactBlock.useItemOn` -> `displayScreen`: the screen opens for `player`.
pub(crate) fn open(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) {
    let dim = lv.dim;
    let c = with_world(|w| w.dims[dim].lifts.contacts.get(&pos).cloned())
        .flatten()
        .unwrap_or_default();
    let v = View {
        pos,
        short: c.short,
        long: c.long,
        door: c.door,
    };
    net::send_to_player(lv.host, player, CHANNEL, &v.encode());
}

/// `ElevatorContactEditPacket.applySettings`: the names and the doors' setting, from within 20
/// blocks of an elevator contact (`BlockEntityConfigurationPacket`); its column hears of the
/// new names (`updateName` → `namesChanged`).
pub(super) fn client_message(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let Some(v) = View::decode(data) else {
        return;
    };
    let near = player.entity().position(&*host).is_some_and(|at| {
        (0..3)
            .map(|i| (at[i] - (f64::from(v.pos[i]) + 0.5)).powi(2))
            .sum::<f64>()
            <= 20.0 * 20.0
    });
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return;
    };
    let Some(lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    if !near
        || !matches!(
            super::block_at(&lv, v.pos),
            Some((Block::ElevatorContact, _))
        )
    {
        return;
    }
    let dim = lv.dim;
    let short: String = v.short.chars().take(SHORT_MAX).collect();
    let long: String = v.long.chars().take(LONG_MAX).collect();
    with_world(|w| {
        let l = &mut w.dims[dim].lifts;
        let c = l.contacts.entry(v.pos).or_default();
        c.short = short;
        c.long = long;
        c.door = v.door.min(DOOR_CONTROLS.len() as u8 - 1);
        l.contacts_changed.push(v.pos);
    });
}

// ---------------------------------------------------------------------------------------------
// Client

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Short,
    Long,
    None,
}

struct Open {
    view: View,
    focus: Focus,
    /// The whole short name is selected (`setHighlightPos(0)`), typing replaces it.
    selected: bool,
    size: [f32; 2],
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    /// The edit to send and whether to close, at the next client tick.
    static LATER: RefCell<(Option<View>, bool)> = const { RefCell::new((None, false)) };
}

pub(super) fn reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = (None, false));
}

/// The server opened the screen: the short name has the focus, all of it selected.
pub(super) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != CHANNEL {
        return false;
    }
    let Some(view) = View::decode(data) else {
        return true;
    };
    OPEN.with_borrow_mut(|o| {
        *o = Some(Open {
            view,
            focus: Focus::Short,
            selected: true,
            size: [0.0; 2],
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
    true
}

/// What the hooks left: the edit to send, and closing.
pub(super) fn tick(host: &mut dyn ClientHost) {
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    if let Some(v) = send {
        net::send_to_server(host, CHANNEL, &v.encode());
    }
    if close {
        OPEN.with_borrow_mut(|o| *o = None);
        host.mod_screen_close(MOD);
    }
    if host.mod_screen() != Some(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

/// `AbstractSimiScreen.init`: the window's corner.
fn corner(size: [f32; 2]) -> (f32, f32) {
    (
        ((size[0] - WINDOW[0]) / 2.0).floor(),
        ((size[1] - WINDOW[1]) / 2.0).floor(),
    )
}

fn confirm_button(x: f32, y: f32) -> [f32; 4] {
    [x + 200.0, y + 58.0, 18.0, 18.0]
}
/// The short name's box (`centerInput`), the description's, their tooltip areas and the doors'
/// input (`SelectionScrollInput(x + 58, y + 57, 53, 16)`).
fn short_box(draw_width: f32, x: f32, y: f32) -> [f32; 4] {
    let bx = if draw_width == 0.0 {
        x + 34.0
    } else {
        x + 36.0 - (draw_width / 2.0).floor()
    };
    [bx, y + 30.0, 30.0, 10.0]
}
fn long_box(x: f32, y: f32) -> [f32; 4] {
    [x + 63.0, y + 30.0, 140.0, 10.0]
}
fn short_area(x: f32, y: f32) -> [f32; 4] {
    [x + 21.0, y + 23.0, 30.0, 18.0]
}
fn long_area(x: f32, y: f32) -> [f32; 4] {
    [x + 57.0, y + 23.0, 147.0, 18.0]
}
fn door_input(x: f32, y: f32) -> [f32; 4] {
    [x + 58.0, y + 57.0, 53.0, 16.0]
}

fn door_key(i: u8) -> &'static str {
    DOOR_CONTROLS.get(usize::from(i)).copied().unwrap_or("all")
}

pub(super) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let size = event.draw.size();
    let Some((view, focus, selected)) = OPEN.with_borrow_mut(|o| {
        o.as_mut().map(|o| {
            o.size = size;
            (o.view.clone(), o.focus, o.selected)
        })
    }) else {
        return;
    };
    let mouse = event.screen.mouse;
    let draw = &mut *event.draw;
    let (x, y) = corner(size);
    draw.sprite(SHEET, BG, [x, y, BG[2], BG[3]], None);
    // The title, centred over the background less 8.
    let title = tr(host, "create.elevator_contact.title", &[]);
    let tw = draw.text_width(&title);
    let tx = x + ((BG[2] - 8.0) / 2.0).floor() - (tw / 2.0).floor();
    draw.text(&title, [tx, y + 6.0], 0xFF2F_3738, false);
    // The contact's item, five times its size, right of the window; the train door's.
    let item = |id: &str| {
        mod_api::Identifier::parse(id)
            .ok()
            .and_then(|i| host.item_key(&i))
    };
    if let Some(contact) = item("create:elevator_contact") {
        draw.item(contact, [x + BG[2] + 6.0, y + BG[3] - 56.0], 80.0);
    }
    if let Some(door) = item("create:train_door") {
        draw.item(door, [x + 37.0, y + 58.0], 16.0);
    }
    // The two edit boxes: white, unbordered; the cursor blinks at the end of the focused one.
    let blink = (millis() / 300).is_multiple_of(2);
    let sw = draw.text_width(&view.short);
    let sb = short_box(sw, x, y);
    if focus == Focus::Short && selected && !view.short.is_empty() {
        draw.fill([sb[0], sb[1] - 1.0, sw, 10.0], 0xFF00_00FF);
    }
    let mut short = view.short.clone();
    if focus == Focus::Short && blink && !selected {
        short.push('_');
    }
    draw.text(&short, [sb[0], sb[1]], 0xFFFF_FFFF, true);
    let lb = long_box(x, y);
    let mut long = view.long.clone();
    if focus == Focus::Long && blink {
        long.push('_');
    }
    draw.text(&long, [lb[0], lb[1]], 0xFFFF_FFFF, true);
    // The doors' label (`Label(x + 4, y + 6)` with a shadow) and the confirm button.
    let d = door_input(x, y);
    let label = tr(
        host,
        &format!(
            "create.contraption.door_control.{}.short",
            door_key(view.door)
        ),
        &[],
    );
    draw.text(&label, [d[0] + 4.0, d[1] + 6.0], 0xFFFF_FFFF, true);
    let c = confirm_button(x, y);
    icon_button(draw, [c[0], c[1]], I_CONFIRM, true, inside(c, mouse));
    // Tooltips (`TooltipArea`s, the doors' `SelectionScrollInput`).
    let lmb = || {
        styled(
            tr(host, "create.gui.schedule.lmb_edit", &[]),
            "dark_gray",
            true,
        )
    };
    let header = |k: &str| styled(tr(host, k, &[]), HEADER, false);
    let tips: Vec<Text> = if inside(short_area(x, y), mouse) {
        vec![header("create.elevator_contact.floor_identifier"), lmb()]
    } else if inside(long_area(x, y), mouse) {
        vec![
            header("create.elevator_contact.floor_description"),
            styled(
                tr(host, "create.crafting_blueprint.optional", &[]),
                "gray",
                false,
            ),
            lmb(),
        ]
    } else if inside(d, mouse) {
        door_tooltip(host, view.door)
    } else {
        Vec::new()
    };
    if !tips.is_empty() {
        let _ = draw.gui_tooltip(&tips, mouse);
    }
}

/// `AbstractSimiWidget.HEADER_RGB` and `HINT_RGB`.
const HEADER: &str = "#5391E1";
const HINT: &str = "#96B7E0";

fn styled(text: String, color: &str, italic: bool) -> Text {
    let mut style = TextStyle::default();
    style.color = Some(color.to_owned());
    style.italic = italic.then_some(true);
    Text::literal(text).with_style(style)
}

/// `SelectionScrollInput.updateTooltip`: its title, the options with the current one marked,
/// the hint of the way the player faces (`DoorControl.createWidget`), and how to scroll.
fn door_tooltip(host: &dyn ClientHost, current: u8) -> Vec<Text> {
    let mut lines = vec![styled(
        tr(host, "create.contraption.door_control", &[]),
        HEADER,
        false,
    )];
    for (i, k) in DOOR_CONTROLS.iter().enumerate() {
        let name = tr(host, &format!("create.contraption.door_control.{k}"), &[]);
        if i == usize::from(current) {
            lines.push(styled(format!("-> {name}"), "white", false));
        } else {
            lines.push(styled(format!("> {name}"), "gray", false));
        }
    }
    let facing = host
        .local_player()
        .and_then(|p| p.entity().rotation(host))
        .map_or(
            "none",
            |r| match create_engine::place::horizontal_direction(r[0]) {
                create_engine::kinetics::Dir::North => "north",
                create_engine::kinetics::Dir::South => "south",
                create_engine::kinetics::Dir::West => "west",
                create_engine::kinetics::Dir::East => "east",
                _ => "none",
            },
        );
    let facing = tr(
        host,
        &format!("create.contraption.door_control.{facing}.short"),
        &[],
    );
    lines.push(styled(
        tr(
            host,
            "create.contraption.door_control.player_facing",
            &[facing],
        ),
        HINT,
        false,
    ));
    lines.push(styled(
        tr(host, "create.gui.scrollInput.scrollToSelect", &[]),
        "dark_gray",
        true,
    ));
    lines
}

pub(super) fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    let mut close = None;
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        let (x, y) = corner(o.size);
        if event.button == 0 && inside(confirm_button(x, y), mouse) {
            click_sound(host);
            close = Some(o.view.clone());
            return;
        }
        // `mouseClicked`: the short name's area takes the focus; the description's box too.
        if inside(short_area(x, y), mouse) {
            o.focus = Focus::Short;
            o.selected = false;
        } else if inside(long_box(x, y), mouse) {
            o.focus = Focus::Long;
            o.selected = false;
        } else {
            o.focus = Focus::None;
            o.selected = false;
        }
    });
    if let Some(v) = close {
        LATER.with_borrow_mut(|l| *l = (Some(v), true));
    }
    Flow::Cancel
}

/// The doors' input: a step a notch, within its options.
pub(super) fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    // A `SelectionScrollInput`: the wheel down moves on.
    let step = if event.scroll[1] > 0.0 {
        -1
    } else if event.scroll[1] < 0.0 {
        1
    } else {
        0
    };
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        let (x, y) = corner(o.size);
        if step != 0 && inside(door_input(x, y), mouse) {
            let n = (i32::from(o.view.door) + step).clamp(0, DOOR_CONTROLS.len() as i32 - 1);
            if n != i32::from(o.view.door) {
                o.view.door = n as u8;
                let max = DOOR_CONTROLS.len() as f32;
                let pitch = 1.5 + 0.1 * n as f32 / max;
                host.play_local_sound(
                    &mod_api::SoundPlay::ui("create:scroll_value").with_pitch(pitch),
                    0,
                );
            }
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
    match event.key {
        "key.keyboard.backspace" => {
            OPEN.with_borrow_mut(|o| {
                let Some(o) = o.as_mut() else {
                    return;
                };
                let all = std::mem::take(&mut o.selected);
                let field = match o.focus {
                    Focus::Short => &mut o.view.short,
                    Focus::Long => &mut o.view.long,
                    Focus::None => return,
                };
                if all {
                    field.clear();
                } else {
                    field.pop();
                }
            });
            Flow::Cancel
        }
        // `keyPressed`: Enter confirms.
        "key.keyboard.enter" | "key.keyboard.keypad.enter" => {
            let v = OPEN.with_borrow(|o| o.as_ref().map(|o| o.view.clone()));
            if let Some(v) = v {
                LATER.with_borrow_mut(|l| *l = (Some(v), true));
            }
            Flow::Cancel
        }
        "key.keyboard.escape" => Flow::Continue,
        _ => {
            let typing = OPEN.with_borrow(|o| o.as_ref().is_some_and(|o| o.focus != Focus::None));
            if typing { Flow::Cancel } else { Flow::Continue }
        }
    }
}

pub(super) fn screen_char_typed(
    host: &dyn ClientHost,
    event: &mut ScreenCharTypedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let c = event.c;
    if !allowed(c) {
        return Flow::Cancel;
    }
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        let all = std::mem::take(&mut o.selected);
        let (field, max) = match o.focus {
            Focus::Short => (&mut o.view.short, SHORT_MAX),
            Focus::Long => (&mut o.view.long, LONG_MAX),
            Focus::None => return,
        };
        if all {
            field.clear();
        }
        if field.chars().count() < max {
            field.push(c);
        }
    });
    Flow::Cancel
}

/// Closing by Escape sends nothing (`onClose` without `confirm`).
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
    fn the_edit_crosses_the_wire() {
        let v = View {
            pos: [1, 64, -3],
            short: "B2".into(),
            long: "Basement".into(),
            door: 3,
        };
        assert_eq!(View::decode(&v.encode()), Some(v));
    }
}
