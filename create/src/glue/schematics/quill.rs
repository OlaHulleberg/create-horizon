//! The schematic and quill in a player's hands (`SchematicAndQuillHandler`,
//! `SchematicPromptScreen`): on the client, right clicks pick the two corners (the block looked
//! at within 75 blocks, or, holding Ctrl, a point `range` blocks ahead), Ctrl and the wheel
//! move the face looked at (or the reach before the second corner), sneaking discards; the
//! selection is outlined in Create's blue with the face looked at shaded; a right click with both
//! corners opens the prompt for the file's name, whose confirm saves it (and whose convert
//! button also writes it to the held quill's schematic). The server saves (`super::save`) and
//! tells the player, as Create's status messages, what happened.
//!
//! The client has no action bar of its own here: its status messages go by the server.

use super::super::net;
use create_engine::kinetics::Dir;
use mod_api::{
    ClientHost, DrawContext, Flow, Hand, Identifier, InteractionKey,
    InteractionKeyMappingTriggeredEvent, MouseScrollingEvent, PlayerRef, RenderLevelStage,
    RenderLevelStageEvent, ScreenCharTypedPreEvent, ScreenClosingEvent, ScreenKeyPressedPreEvent,
    ScreenMouseButtonPressedPreEvent, ScreenRenderPostEvent, ServerHost, SoundPlay, Text,
    WorldDepth, WorldLine, WorldQuad,
};
use std::cell::RefCell;

/// Client to server: `S` and a selection to save, `M` and a status message to show, `Y` a held
/// schematic's place (`SchematicSyncPacket`) and `P` its print (`SchematicPlacePacket`).
pub(super) const CHANNEL: &str = "create:schematic_quill";
const MOD: &str = "create";
/// `AllGuiTextures.SCHEMATIC_PROMPT`: `schematics_2`, 213 × 79.
const PROMPT: &str = "create:gui/schematics_2";
const WIDGETS: &str = "create:gui/widgets";
const ICONS: &str = "create:gui/icons";
const WIDTH: f32 = 213.0;
const HEIGHT: f32 = 79.0;

/// `SchematicAndQuillHandler`'s state on this client.
struct Selection {
    first: Option<[i32; 3]>,
    second: Option<[i32; 3]>,
    selected: Option<[i32; 3]>,
    face: Option<Dir>,
    range: i32,
}

/// The prompt's name field, and when it opened (for the cursor's blink).
struct Prompt {
    name: String,
    opened: std::time::Instant,
}

/// What waits for the client's next tick, the screen hooks having a read-only client.
#[derive(Default)]
struct Later {
    close: bool,
    save: Option<bool>,
}

thread_local! {
    static SELECTION: RefCell<Selection> = const { RefCell::new(Selection {
        first: None,
        second: None,
        selected: None,
        face: None,
        range: 10,
    }) };
    static OPEN: RefCell<Option<Prompt>> = const { RefCell::new(None) };
    static LATER: RefCell<Later> = RefCell::new(Later::default());
    /// The screen's size in GUI pixels when last drawn, for clicks.
    static SIZE: std::cell::Cell<[f32; 2]> = const { std::cell::Cell::new([0.0, 0.0]) };
}

pub(super) fn reset() {
    SELECTION.with_borrow_mut(|s| {
        s.first = None;
        s.second = None;
        s.selected = None;
        s.face = None;
    });
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = Later::default());
}

// ---------------------------------------------------------------------------------------------
// Client: the selection

/// `isActive`: the quill in the main hand and no screen open.
fn active(host: &dyn ClientHost) -> bool {
    host.playing() && holding(host)
}

fn holding(host: &dyn ClientHost) -> bool {
    let Some(player) = host.local_player() else {
        return false;
    };
    host.living(player.entity())
        .and_then(|l| l.hand_item(host, Hand::Main))
        .is_some_and(|s| s.item().id(host) == super::QUILL)
}

/// The eye and the unit vector of the view.
fn view(host: &dyn ClientHost) -> Option<([f64; 3], [f64; 3])> {
    let camera = host.camera()?;
    let (yaw, pitch) = (
        f64::from(camera.yaw).to_radians(),
        f64::from(camera.pitch).to_radians(),
    );
    let look = [
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    ];
    Some((camera.position, look))
}

/// The face of block `pos` that `point` lies on.
pub(crate) fn face_of(pos: [i32; 3], point: [f64; 3]) -> Dir {
    let local = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]));
    let mut best = (Dir::Up, f64::MAX);
    for d in Dir::ALL {
        let o = d.offset();
        let axis = (0..3).find(|&i| o[i] != 0).unwrap_or(1);
        let plane = if o[axis] > 0 { 1.0 } else { 0.0 };
        let dist = (local[axis] - plane).abs();
        if dist < best.1 {
            best = (d, dist);
        }
    }
    best.0
}

/// `tick`: the block a click would pick, and the face of the selection looked at.
pub(super) fn tick(host: &mut dyn ClientHost) {
    let later = LATER.with_borrow_mut(std::mem::take);
    if let Some(convert) = later.save {
        save(host, convert);
    }
    if later.close {
        host.mod_screen_close(MOD);
    }
    if host.mod_screen() != Some(MOD) && OPEN.with_borrow(Option::is_some) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
    if !active(&*host) {
        return;
    }
    let Some((eye, look)) = view(&*host) else {
        return;
    };
    let ctrl = host.modifiers().control();
    let range = SELECTION.with_borrow(|s| s.range);
    let selected = if ctrl {
        Some([0, 1, 2].map(|i| (eye[i] + look[i] * f64::from(range)).floor() as i32))
    } else {
        host.raycast(75.0).map(|hit| {
            let pos = [hit.block_x, hit.block_y, hit.block_z];
            let face = face_of(pos, [hit.x, hit.y, hit.z]);
            let replaceable = host
                .level(host.dimension())
                .and_then(|l| l.block_state(&*host, pos))
                .and_then(|s| mod_api::HostBlockStates::block_states(&*host).replaceable(s))
                .unwrap_or(false);
            if matches!(face, Dir::Up | Dir::Down) && !replaceable {
                let o = face.offset();
                [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]
            } else {
                pos
            }
        })
    };
    SELECTION.with_borrow_mut(|s| {
        s.selected = selected;
        s.face = match (s.first, s.second) {
            (Some(a), Some(b)) => crossed_face(bounds(a, b), eye, look),
            _ => None,
        };
    });
}

/// The box of the blocks from `a` to `b`, both included.
fn bounds(a: [i32; 3], b: [i32; 3]) -> ([f64; 3], [f64; 3]) {
    (
        [0, 1, 2].map(|i| f64::from(a[i].min(b[i]))),
        [0, 1, 2].map(|i| f64::from(a[i].max(b[i]) + 1)),
    )
}

/// The face of `bb` the view crosses (entering it from outside, leaving it from inside), as
/// its outward direction (`rayTraceUntil` with `inside ^ bb.contains`).
fn crossed_face(bb: ([f64; 3], [f64; 3]), eye: [f64; 3], look: [f64; 3]) -> Option<Dir> {
    let (min, max) = bb;
    let inside = (0..3).all(|i| eye[i] > min[i] && eye[i] < max[i]);
    let (mut near, mut far) = (f64::MIN, f64::MAX);
    let (mut near_face, mut far_face) = (None, None);
    for i in 0..3 {
        let neg = Dir::ALL.into_iter().find(|d| d.offset()[i] < 0)?;
        let pos = Dir::ALL.into_iter().find(|d| d.offset()[i] > 0)?;
        if look[i].abs() < 1e-9 {
            if eye[i] < min[i] || eye[i] > max[i] {
                return None;
            }
            continue;
        }
        let (t1, t2) = ((min[i] - eye[i]) / look[i], (max[i] - eye[i]) / look[i]);
        let (enter, exit, enter_face, exit_face) = if t1 < t2 {
            (t1, t2, neg, pos)
        } else {
            (t2, t1, pos, neg)
        };
        if enter > near {
            near = enter;
            near_face = Some(enter_face);
        }
        if exit < far {
            far = exit;
            far_face = Some(exit_face);
        }
    }
    if near > far || far < 0.0 || far > 70.0 {
        return None;
    }
    if inside { far_face } else { near_face }
}

/// A status message, shown by the server in the player's action bar (`sendStatus`).
fn status(host: &mut dyn ClientHost, key: &str, args: &[String]) {
    let mut data = vec![b'M'];
    data.extend_from_slice(key.as_bytes());
    for a in args {
        data.push(0);
        data.extend_from_slice(a.as_bytes());
    }
    net::send_to_server(host, CHANNEL, &data);
}

/// `onMouseInput` for the use key: Create's clicks of the quill.
pub(super) fn interaction_key(
    host: &mut dyn ClientHost,
    event: &mut InteractionKeyMappingTriggeredEvent,
) -> Flow {
    if event.key != InteractionKey::Use || event.hand != Hand::Main || !active(&*host) {
        return Flow::Continue;
    }
    event.swing_hand = false;
    let sneaking = host
        .local_player()
        .is_some_and(|p| p.entity().is_shift_key_down(&*host) == Some(true));
    if sneaking {
        SELECTION.with_borrow_mut(|s| {
            s.first = None;
            s.second = None;
        });
        status(host, "create.schematicAndQuill.abort", &[]);
        return Flow::Cancel;
    }
    let (first, second, selected) = SELECTION.with_borrow(|s| (s.first, s.second, s.selected));
    if second.is_some() {
        OPEN.with_borrow_mut(|o| {
            *o = Some(Prompt {
                name: String::new(),
                opened: std::time::Instant::now(),
            })
        });
        if !host.mod_screen_open(MOD) {
            OPEN.with_borrow_mut(|o| *o = None);
        }
        return Flow::Cancel;
    }
    let Some(selected) = selected else {
        status(host, "create.schematicAndQuill.noTarget", &[]);
        return Flow::Cancel;
    };
    if first.is_some() {
        SELECTION.with_borrow_mut(|s| s.second = Some(selected));
        status(host, "create.schematicAndQuill.secondPos", &[]);
    } else {
        SELECTION.with_borrow_mut(|s| s.first = Some(selected));
        status(host, "create.schematicAndQuill.firstPos", &[]);
    }
    Flow::Cancel
}

/// `mouseScrolled` with Ctrl held: the reach before the second corner, then the face looked at
/// pushed out or pulled in.
pub(super) fn mouse_scrolling(host: &mut dyn ClientHost, event: &mut MouseScrollingEvent) -> Flow {
    if !active(&*host) || !host.modifiers().control() || event.scroll[1] == 0.0 {
        return Flow::Continue;
    }
    let mut delta = event.scroll[1];
    let eye = view(&*host).map(|(e, _)| e);
    let resized = SELECTION.with_borrow_mut(|s| {
        if s.second.is_none() {
            s.range = (f64::from(s.range) + delta).clamp(1.0, 100.0) as i32;
        }
        let (Some(face), Some(a), Some(b)) = (s.face, s.first, s.second) else {
            return None;
        };
        let (mut min, mut max) = bounds(a, b);
        if let Some(eye) = eye
            && (0..3).all(|i| eye[i] > min[i] && eye[i] < max[i])
        {
            delta = -delta;
        }
        let step = if delta > 0.0 {
            delta.ceil()
        } else {
            delta.floor()
        };
        let o = face.offset().map(f64::from);
        // `bb.move` toward a negative face, then the far corner pulled back: the face moves out
        // with the wheel up.
        let negative = o.iter().any(|v| *v < 0.0);
        if negative {
            for i in 0..3 {
                min[i] -= o[i] * step;
                max[i] -= o[i] * step;
            }
        }
        let sign = if negative { -1.0 } else { 1.0 };
        for i in 0..3 {
            max[i] = (max[i] - o[i] * step * sign).max(min[i] + 1.0);
        }
        let lo = min.map(|v| v as i32);
        let hi = max.map(|v| v as i32 - 1);
        s.first = Some(lo);
        s.second = Some(hi);
        Some([0, 1, 2].map(|i| hi[i] - lo[i] + 1))
    });
    if let Some(size) = resized {
        let args = size.map(|v| v.to_string());
        status(host, "create.schematicAndQuill.dimensions", &args);
    }
    Flow::Cancel
}

/// The outline: Create's `0x6886c5` box round the selection (or the corner and the block
/// looked at), its face looked at shaded.
pub(super) fn render_level_stage(host: &dyn ClientHost, event: &mut RenderLevelStageEvent<'_>) {
    if event.stage != RenderLevelStage::AfterTranslucentBlocks || !holding(host) {
        return;
    }
    let (first, second, selected, face) =
        SELECTION.with_borrow(|s| (s.first, s.second, s.selected, s.face));
    let (a, b) = match (first, second, selected) {
        (Some(a), Some(b), _) => (a, b),
        (Some(a), None, Some(c)) => (a, c),
        (Some(a), None, None) => (a, a),
        (None, _, Some(c)) => (c, c),
        _ => return,
    };
    // A hair outside the blocks, so their faces do not hide it.
    let (min, max) = bounds(a, b);
    let (min, max) = (min.map(|v| v - 0.005), max.map(|v| v + 0.005));
    let color = 0xFF68_86C5;
    let c = |x: usize, y: usize, z: usize| {
        [
            if x == 0 { min[0] } else { max[0] },
            if y == 0 { min[1] } else { max[1] },
            if z == 0 { min[2] } else { max[2] },
        ]
    };
    let mut lines = Vec::with_capacity(12);
    for i in 0..2 {
        for j in 0..2 {
            lines.push(WorldLine::new(
                c(0, i, j),
                c(1, i, j),
                color,
                3.0,
                WorldDepth::Tested,
            ));
            lines.push(WorldLine::new(
                c(i, 0, j),
                c(i, 1, j),
                color,
                3.0,
                WorldDepth::Tested,
            ));
            lines.push(WorldLine::new(
                c(i, j, 0),
                c(i, j, 1),
                color,
                3.0,
                WorldDepth::Tested,
            ));
        }
    }
    let _ = event.draw.lines(&lines);
    if let Some(face) = face {
        let o = face.offset();
        let axis = (0..3).find(|&i| o[i] != 0).unwrap_or(1);
        let side = usize::from(o[axis] > 0);
        let corner = |u: usize, v: usize| {
            let mut idx = [0usize; 3];
            idx[axis] = side;
            let (p, q) = match axis {
                0 => (1, 2),
                1 => (0, 2),
                _ => (0, 1),
            };
            idx[p] = u;
            idx[q] = v;
            c(idx[0], idx[1], idx[2])
        };
        // Pushed a hair out of the box so it does not fight the blocks' faces.
        let nudge = |p: [f64; 3]| [0, 1, 2].map(|i| p[i] + f64::from(o[i]) * 0.01);
        let quad = WorldQuad::new(
            [
                nudge(corner(0, 0)),
                nudge(corner(1, 0)),
                nudge(corner(1, 1)),
                nudge(corner(0, 1)),
            ],
            0x4068_86C5,
            WorldDepth::Tested,
        );
        let _ = event.draw.quads(&[quad]);
    }
}

// ---------------------------------------------------------------------------------------------
// Client: the prompt

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

fn corner(size: [f32; 2]) -> (f32, f32) {
    (
        ((size[0] - WIDTH) / 2.0).floor(),
        ((size[1] - HEIGHT) / 2.0).floor(),
    )
}

/// The buttons, as `[x, y, w, h]`: abort, confirm, convert (`IconButton`s, `guiTop + 2` down).
fn buttons(left: f32, top: f32) -> [[f32; 4]; 3] {
    let y = top + 2.0 + 53.0;
    [
        [left + 7.0, y, 18.0, 18.0],
        [left + 158.0, y, 18.0, 18.0],
        [left + 180.0, y, 18.0, 18.0],
    ]
}

fn inside(r: [f32; 4], at: [f32; 2]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

fn item_key(host: &dyn ClientHost, id: &str) -> Option<mod_api::ItemKey> {
    Identifier::parse(id).ok().and_then(|i| host.item_key(&i))
}

pub(super) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let draw: &mut dyn DrawContext = event.draw;
    let cursor = event.screen.mouse;
    SIZE.set(draw.size());
    let (x, y) = corner(draw.size());
    draw.sprite(
        PROMPT,
        [0.0, 0.0, WIDTH, HEIGHT],
        [x, y, WIDTH, HEIGHT],
        None,
    );
    let title = host
        .translate("create.schematicAndQuill.title")
        .unwrap_or_else(|| "Schematic Name:".into());
    let tw = draw.text_width(&title);
    draw.text(
        &title,
        [x + ((WIDTH - 8.0 - tw) / 2.0).floor(), y + 4.0],
        0xFF50_5050,
        false,
    );
    if let Some(k) = item_key(host, super::SCHEMATIC) {
        draw.item(k, [x + 22.0, y + 24.0], 16.0);
    }
    if let Some(k) = item_key(host, super::QUILL) {
        draw.item(k, [x + WIDTH + 6.0, y + HEIGHT - 38.0], 48.0);
    }
    let (name, blink) = OPEN.with_borrow(|o| {
        o.as_ref().map_or((String::new(), false), |p| {
            (
                p.name.clone(),
                (p.opened.elapsed().as_millis() / 300) % 2 == 0,
            )
        })
    });
    let at = [x + 49.0, y + 2.0 + 26.0];
    draw.text(&name, at, 0xFFFF_FFFF, true);
    if blink {
        let w = draw.text_width(&name);
        draw.text("_", [at[0] + w, at[1]], 0xFFFF_FFFF, true);
    }
    for (r, icon) in buttons(x, y)
        .into_iter()
        .zip([[16.0, 0.0], [0.0, 16.0], [0.0, 128.0]])
    {
        let bg = if inside(r, cursor) { 18.0 } else { 0.0 };
        draw.sprite(WIDGETS, [bg, 0.0, 18.0, 18.0], r, None);
        draw.sprite(
            ICONS,
            [icon[0], icon[1], 16.0, 16.0],
            [r[0] + 1.0, r[1] + 1.0, 16.0, 16.0],
            None,
        );
    }
    // The buttons' tooltips.
    let tips = [
        "create.action.discard",
        "create.action.saveToFile",
        "create.schematicAndQuill.convert",
    ];
    for (r, key) in buttons(x, y).into_iter().zip(tips) {
        if inside(r, cursor) {
            let tip = host.translate(key).unwrap_or_else(|| key.to_owned());
            let _ = draw.gui_tooltip(&[Text::literal(tip)], cursor);
        }
    }
}

pub(super) fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    if event.button != 0 {
        return Flow::Cancel;
    }
    let size = SIZE.get();
    let (x, y) = corner(size);
    let [abort, confirm, convert] = buttons(x, y);
    let at = event.screen.mouse;
    let click = || {
        host.play_local_sound(&SoundPlay::ui("minecraft:ui.button.click"), 0);
    };
    if inside(abort, at) {
        click();
        SELECTION.with_borrow_mut(|s| {
            s.first = None;
            s.second = None;
        });
        LATER.with_borrow_mut(|l| l.close = true);
        OPEN.with_borrow_mut(|o| *o = None);
    } else if inside(confirm, at) {
        click();
        LATER.with_borrow_mut(|l| {
            l.save = Some(false);
            l.close = true;
        });
    } else if inside(convert, at) {
        click();
        LATER.with_borrow_mut(|l| {
            l.save = Some(true);
            l.close = true;
        });
    }
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
        "key.keyboard.enter" | "key.keyboard.keypad.enter" => {
            LATER.with_borrow_mut(|l| {
                l.save = Some(false);
                l.close = true;
            });
            Flow::Cancel
        }
        "key.keyboard.backspace" => {
            OPEN.with_borrow_mut(|o| {
                if let Some(p) = o.as_mut() {
                    p.name.pop();
                }
            });
            Flow::Cancel
        }
        "key.keyboard.escape" => Flow::Continue,
        // Typing goes to the field, not to the game's keys (the inventory key closing it).
        _ => Flow::Cancel,
    }
}

/// `EditBox.charTyped`, at most 35 characters.
pub(super) fn screen_char_typed(
    host: &dyn ClientHost,
    event: &mut ScreenCharTypedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let c = event.c;
    if c != '\u{a7}' && c >= ' ' && c != '\u{7f}' {
        OPEN.with_borrow_mut(|o| {
            if let Some(p) = o.as_mut()
                && p.name.chars().count() < 35
            {
                p.name.push(c);
            }
        });
    }
    Flow::Cancel
}

/// The prompt closed (Escape, or a button): forgotten unless a save waits.
pub(super) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || !ours(host) {
        return;
    }
    if LATER.with_borrow(|l| l.save.is_none()) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
}

/// `saveSchematic`: the selection and the name to the server, which saves and answers.
fn save(host: &mut dyn ClientHost, convert: bool) {
    let name = OPEN
        .with_borrow_mut(Option::take)
        .map(|p| p.name)
        .unwrap_or_default();
    let Some((a, b)) = SELECTION.with_borrow(|s| Some((s.first?, s.second?))) else {
        return;
    };
    let mut data = vec![b'S', u8::from(convert)];
    for v in a.iter().chain(b.iter()) {
        data.extend_from_slice(&v.to_le_bytes());
    }
    data.extend_from_slice(name.as_bytes());
    net::send_to_server(host, CHANNEL, &data);
    SELECTION.with_borrow_mut(|s| {
        s.first = None;
        s.second = None;
    });
}

// ---------------------------------------------------------------------------------------------
// Server

/// A selection to save, or a status message to show.
pub(super) fn client_message(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    match data.split_first() {
        Some((b'M', rest)) => {
            let text = String::from_utf8_lossy(rest);
            let mut parts = text.split('\0');
            let key = parts.next().unwrap_or_default().to_owned();
            let args: Vec<Text> = parts.map(|a| Text::literal(a.to_owned())).collect();
            let _ = player.action_bar(host, Text::translate(key, args));
        }
        Some((b'Y', rest)) => super::synced(host, player, rest),
        Some((b'P', rest)) => super::printed(host, player, rest),
        Some((b'S', rest)) if rest.len() >= 25 => {
            let convert = rest[0] != 0;
            let int = |i: usize| {
                let at = 1 + i * 4;
                i32::from_le_bytes([rest[at], rest[at + 1], rest[at + 2], rest[at + 3]])
            };
            let (a, b) = ([int(0), int(1), int(2)], [int(3), int(4), int(5)]);
            let file = String::from_utf8_lossy(&rest[25..]).into_owned();
            // `SchematicAndQuillHandler` saves on the client; here the server saves for it,
            // as long as the selection is near the player (the 75 blocks it can pick in).
            let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
                return;
            };
            let Some(pos) = player.entity().position(&*host) else {
                return;
            };
            let near = |p: [i32; 3]| (0..3).all(|i| (f64::from(p[i]) - pos[i]).abs() <= 128.0);
            if !near(a) || !near(b) {
                return;
            }
            let Some(mut lv) = super::ServerLevel::new(host, &dimension) else {
                return;
            };
            let who = player.name(&*lv.host).unwrap_or_default();
            let lines = super::save(&mut lv, Some(player), &who, a, b, &file, convert);
            // `schematicAndQuill.saved` (or `failed`), as the status Create shows.
            let line = match lines.first() {
                Some(l) if l.starts_with("Saved as ") => Text::translate(
                    "create.schematicAndQuill.saved",
                    vec![Text::literal(l.trim_start_matches("Saved as ").to_owned())],
                ),
                _ => Text::translate("create.schematicAndQuill.failed", Vec::new()),
            };
            let _ = player.action_bar(&mut *lv.host, line);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_face_looked_at_is_the_one_crossed() {
        let bb = ([0.0, 0.0, 0.0], [3.0, 3.0, 3.0]);
        // From the north looking south: the north face.
        assert_eq!(
            crossed_face(bb, [1.5, 1.5, -5.0], [0.0, 0.0, 1.0]),
            Some(Dir::North)
        );
        // From inside looking up: the top.
        assert_eq!(
            crossed_face(bb, [1.5, 1.5, 1.5], [0.0, 1.0, 0.0]),
            Some(Dir::Up)
        );
        // Looking away: none.
        assert_eq!(crossed_face(bb, [1.5, 1.5, -5.0], [0.0, 0.0, -1.0]), None);
    }

    #[test]
    fn a_hit_point_names_its_face() {
        assert_eq!(face_of([0, 64, 0], [0.5, 65.0, 0.5]), Dir::Up);
        assert_eq!(face_of([0, 64, 0], [0.0, 64.5, 0.5]), Dir::West);
        assert_eq!(face_of([0, 64, 0], [0.5, 64.5, 1.0]), Dir::South);
    }
}
