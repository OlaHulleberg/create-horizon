//! The schematic's edit screen (`SchematicEditScreen`), opened by a right click with the
//! written schematic while sneaking (`SchematicItem.use`): its anchor typed, its rotation and
//! mirror chosen; closing it with valid coordinates deploys it there (`super::tools::edited`).

use create_engine::schematic::{Mirror, Rotation};
use mod_api::{
    ClientHost, DrawContext, Flow, Hand, Identifier, InteractionAttempt, InteractionKey,
    InteractionKeyMappingTriggeredEvent, ScreenCharTypedPreEvent, ScreenClosingEvent,
    ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent, ScreenMouseScrolledPreEvent,
    ScreenRenderPostEvent, SoundPlay,
};
use std::cell::{Cell, RefCell};

const MOD: &str = "create";
/// `AllGuiTextures.SCHEMATIC`: `schematics` from (10, 8), 192 × 123.
const TEX: &str = "create:gui/schematics";
const WIDGETS: &str = "create:gui/widgets";
const ICONS: &str = "create:gui/icons";
const WIDTH: f32 = 192.0;
const HEIGHT: f32 = 123.0;

/// `SchematicEditScreen`'s fields: the anchor's text, rotation, mirror, the field typed in.
struct Edit {
    fields: [String; 3],
    rotation: usize,
    mirror: usize,
    focus: Option<usize>,
    opened: std::time::Instant,
}

/// An edit to send: the anchor, rotation and mirror.
type Edited = ([i32; 3], usize, usize);

thread_local! {
    static OPEN: RefCell<Option<Edit>> = const { RefCell::new(None) };
    static SIZE: Cell<[f32; 2]> = const { Cell::new([0.0, 0.0]) };
    /// The edit to send and the close, at the next client tick.
    static LATER: RefCell<(Option<Edited>, bool)> = const { RefCell::new((None, false)) };
}

pub(super) fn reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = (None, false));
}

/// A right click with the schematic while sneaking: its screen (`SchematicItem.use`).
pub(super) fn interaction_key(
    host: &mut dyn ClientHost,
    event: &mut InteractionKeyMappingTriggeredEvent,
) -> Flow {
    if event.key != InteractionKey::Use || event.hand != Hand::Main || !host.playing() {
        return Flow::Continue;
    }
    let sneaking = host
        .local_player()
        .is_some_and(|p| p.entity().is_shift_key_down(&*host) == Some(true));
    if !sneaking {
        return Flow::Continue;
    }
    let Some((_, anchor, rotation, mirror)) = super::tools::edit_start() else {
        return Flow::Continue;
    };
    event.swing_hand = false;
    if event.attempt == InteractionAttempt::Held {
        return Flow::Cancel;
    }
    // `SchematicEditScreen.init`: the anchor, else the player's own block.
    let anchor = match anchor {
        Some(a) => Some(a),
        None => host
            .local_player()
            .and_then(|p| p.entity().position(&*host))
            .map(|p| p.map(|v| v.floor() as i32)),
    };
    let Some(anchor) = anchor else {
        return Flow::Cancel;
    };
    OPEN.with_borrow_mut(|o| {
        *o = Some(Edit {
            fields: anchor.map(|v| v.to_string()),
            rotation: rotation as usize,
            mirror: mirror as usize,
            focus: None,
            opened: std::time::Instant::now(),
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
    Flow::Cancel
}

pub(super) fn tick(host: &mut dyn ClientHost) {
    let (edit, close) = LATER.with_borrow_mut(std::mem::take);
    if let Some((anchor, r, m)) = edit {
        super::tools::edited(&*host, anchor, Rotation::ALL[r % 4], Mirror::ALL[m % 3]);
    }
    if close {
        host.mod_screen_close(MOD);
    }
    if host.mod_screen() != Some(MOD) && OPEN.with_borrow(Option::is_some) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

fn corner(size: [f32; 2]) -> (f32, f32) {
    (
        ((size[0] - WIDTH) / 2.0).floor(),
        ((size[1] - HEIGHT) / 2.0).floor(),
    )
}

fn inside(r: [f32; 4], at: [f32; 2]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

/// The three fields, the rotation and mirror areas and the confirm button, from `guiTop + 2`.
fn areas(x: f32, y: f32) -> ([[f32; 4]; 3], [f32; 4], [f32; 4], [f32; 4]) {
    let y = y + 2.0;
    (
        [
            [x + 50.0, y + 26.0, 34.0, 10.0],
            [x + 90.0, y + 26.0, 34.0, 10.0],
            [x + 130.0, y + 26.0, 34.0, 10.0],
        ],
        [x + 45.0, y + 43.0, 118.0, 18.0],
        [x + 45.0, y + 65.0, 118.0, 18.0],
        [x + WIDTH - 33.0, y + HEIGHT - 26.0, 18.0, 18.0],
    )
}

fn tr(host: &dyn ClientHost, key: &str, fallback: &str) -> String {
    host.translate(key).unwrap_or_else(|| fallback.to_owned())
}

/// The options as Create names them (`schematic.rotation.*`, `schematic.mirror.*`), in
/// `Rotation.values()` and `Mirror.values()` order.
const ROTATIONS: [(&str, &str); 4] = [
    ("create.schematic.rotation.none", "None"),
    ("create.schematic.rotation.cw90", "Clockwise 90"),
    ("create.schematic.rotation.cw180", "Clockwise 180"),
    ("create.schematic.rotation.cw270", "Clockwise 270"),
];
const MIRRORS: [(&str, &str); 3] = [
    ("create.schematic.mirror.none", "None"),
    ("create.schematic.mirror.leftRight", "Left-Right"),
    ("create.schematic.mirror.frontBack", "Front-Back"),
];

pub(super) fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let draw: &mut dyn DrawContext = event.draw;
    SIZE.set(draw.size());
    let cursor = event.screen.mouse;
    let (x, y) = corner(draw.size());
    draw.sprite(TEX, [10.0, 8.0, WIDTH, HEIGHT], [x, y, WIDTH, HEIGHT], None);
    // `getCurrentSchematicName`: the file.
    let title = super::tools::current_name();
    let tw = draw.text_width(&title);
    draw.text(
        &title,
        [x + ((WIDTH - 8.0 - tw) / 2.0).floor(), y + 4.0],
        0xFF50_5050,
        false,
    );
    let (fields, rot, mir, confirm) = areas(x, y);
    let (texts, r, m, focus, blink) = OPEN.with_borrow(|o| {
        o.as_ref().map_or(Default::default(), |e| {
            (
                e.fields.clone(),
                e.rotation,
                e.mirror,
                e.focus,
                (e.opened.elapsed().as_millis() / 300) % 2 == 0,
            )
        })
    });
    for (i, f) in fields.iter().enumerate() {
        draw.text(&texts[i], [f[0], f[1]], 0xFFFF_FFFF, true);
        if focus == Some(i) && blink {
            let w = draw.text_width(&texts[i]);
            draw.text("_", [f[0] + w, f[1]], 0xFFFF_FFFF, true);
        }
    }
    let (rk, rf) = ROTATIONS[r % 4];
    draw.text(
        &tr(host, rk, rf),
        [rot[0] + 5.0, rot[1] + 5.0],
        0xFFFF_FFFF,
        true,
    );
    let (mk, mf) = MIRRORS[m % 3];
    draw.text(
        &tr(host, mk, mf),
        [mir[0] + 5.0, mir[1] + 5.0],
        0xFFFF_FFFF,
        true,
    );
    let bg = if inside(confirm, cursor) { 18.0 } else { 0.0 };
    draw.sprite(WIDGETS, [bg, 0.0, 18.0, 18.0], confirm, None);
    draw.sprite(
        ICONS,
        [0.0, 16.0, 16.0, 16.0],
        [confirm[0] + 1.0, confirm[1] + 1.0, 16.0, 16.0],
        None,
    );
    if let Some(k) = Identifier::parse(super::SCHEMATIC)
        .ok()
        .and_then(|i| host.item_key(&i))
    {
        draw.item(k, [x + WIDTH + 6.0, y + HEIGHT - 40.0], 48.0);
    }
}

/// `removed`: with valid coordinates, the schematic deployed there, turned and mirrored as
/// chosen; the screen closed.
fn confirm() {
    let edit = OPEN.with_borrow_mut(Option::take);
    let Some(e) = edit else {
        return;
    };
    let parse = |s: &str| s.parse::<i32>().ok();
    let anchor = match (
        parse(&e.fields[0]),
        parse(&e.fields[1]),
        parse(&e.fields[2]),
    ) {
        (Some(x), Some(y), Some(z)) => Some([x, y, z]),
        _ => None,
    };
    LATER.with_borrow_mut(|l| *l = (anchor.map(|a| (a, e.rotation, e.mirror)), true));
}

pub(super) fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let (x, y) = corner(SIZE.get());
    let (fields, rot, mir, ok) = areas(x, y);
    let at = event.screen.mouse;
    let step = if event.button == 1 { -1 } else { 1 };
    if inside(ok, at) {
        host.play_local_sound(&SoundPlay::ui("minecraft:ui.button.click"), 0);
        confirm();
        return Flow::Cancel;
    }
    OPEN.with_borrow_mut(|o| {
        let Some(e) = o.as_mut() else {
            return;
        };
        e.focus = fields.iter().position(|f| inside(*f, at));
        if inside(rot, at) {
            e.rotation = (e.rotation as i32 + step).rem_euclid(4) as usize;
        }
        if inside(mir, at) {
            e.mirror = (e.mirror as i32 + step).rem_euclid(3) as usize;
        }
    });
    Flow::Cancel
}

/// `SelectionScrollInput`: the wheel over the rotation or mirror changes it.
pub(super) fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) || event.scroll[1] == 0.0 {
        return Flow::Continue;
    }
    let (x, y) = corner(SIZE.get());
    let (_, rot, mir, _) = areas(x, y);
    let at = event.screen.mouse;
    let step = if event.scroll[1] > 0.0 { -1 } else { 1 };
    OPEN.with_borrow_mut(|o| {
        if let Some(e) = o.as_mut() {
            if inside(rot, at) {
                e.rotation = (e.rotation as i32 + step).rem_euclid(4) as usize;
            }
            if inside(mir, at) {
                e.mirror = (e.mirror as i32 + step).rem_euclid(3) as usize;
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
    // `keyPressed`'s paste: "x,y,z" from the clipboard into the three fields (Create's
    // `replaceAll(" ", "")` is thrown away, so spaced numbers are not taken).
    if event.key == "key.keyboard.v"
        && mod_api::ClientModifiers::new(event.modifiers).control()
        && let Some(text) = host.clipboard_text()
    {
        let split: Vec<&str> = text.split(',').collect();
        if split.len() == 3 && split.iter().all(|s| s.parse::<i32>().is_ok()) {
            OPEN.with_borrow_mut(|o| {
                if let Some(e) = o.as_mut() {
                    for (field, v) in e.fields.iter_mut().zip(&split) {
                        *field = (*v).to_owned();
                    }
                }
            });
            return Flow::Cancel;
        }
    }
    match event.key {
        "key.keyboard.enter" | "key.keyboard.keypad.enter" => {
            confirm();
            Flow::Cancel
        }
        "key.keyboard.backspace" => {
            OPEN.with_borrow_mut(|o| {
                if let Some(e) = o.as_mut()
                    && let Some(f) = e.focus
                {
                    e.fields[f].pop();
                }
            });
            Flow::Cancel
        }
        "key.keyboard.tab" => {
            OPEN.with_borrow_mut(|o| {
                if let Some(e) = o.as_mut() {
                    e.focus = Some(e.focus.map_or(0, |f| (f + 1) % 3));
                }
            });
            Flow::Cancel
        }
        "key.keyboard.escape" => Flow::Continue,
        _ => Flow::Cancel,
    }
}

/// The fields take a number (`setFilter`: an integer or a lone minus), six characters at most.
pub(super) fn screen_char_typed(
    host: &dyn ClientHost,
    event: &mut ScreenCharTypedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    OPEN.with_borrow_mut(|o| {
        if let Some(e) = o.as_mut()
            && let Some(f) = e.focus
        {
            let mut next = e.fields[f].clone();
            next.push(event.c);
            let ok = next == "-" || next.parse::<i32>().is_ok();
            if ok && next.chars().count() <= 6 {
                e.fields[f] = next;
            }
        }
    });
    Flow::Cancel
}

/// Escape closes it as Create's `onClose` does: the edit is kept.
pub(super) fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || !ours(host) {
        return;
    }
    confirm();
    LATER.with_borrow_mut(|l| l.1 = false);
}
