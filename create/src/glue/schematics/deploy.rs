//! A written schematic in a player's hand (`SchematicHandler`, `DeployTool`,
//! `SchematicEditScreen`): its bounds are outlined where it is deployed, or where a right click
//! would deploy it (centred on the block looked at, on its face); a right click deploys it
//! there (moving it when it already is), and sneaking, the right click opens Create's screen to
//! type its anchor and choose its rotation and mirror. The server writes the held schematic
//! (`super::deploy`).
//!
//! Left out: the ghost of the schematic's blocks (`SchematicRenderer`) and the other tools
//! (move, rotate, flip, print), whose selection screen this port has not; the edit screen does
//! what they do.

use super::super::net;
use super::body;
use super::quill::CHANNEL;
use create_engine::schematic::{Mirror, Rotation, SchematicData, transform};
use mod_api::{
    ClientHost, DrawContext, Flow, Hand, Identifier, InteractionKey,
    InteractionKeyMappingTriggeredEvent, ItemStack, PlayerRef, RenderLevelStage,
    RenderLevelStageEvent, ScreenCharTypedPreEvent, ScreenClosingEvent, ScreenKeyPressedPreEvent,
    ScreenMouseButtonPressedPreEvent, ScreenMouseScrolledPreEvent, ScreenRenderPostEvent,
    ServerHost, SoundPlay, WorldDepth, WorldLine,
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

/// The written schematic in the local player's main hand, and its data.
fn held(host: &dyn ClientHost) -> Option<(ItemStack, SchematicData)> {
    let player = host.local_player()?;
    let stack = host.living(player.entity())?.hand_item(host, Hand::Main)?;
    if stack.item().id(host) != super::SCHEMATIC {
        return None;
    }
    let data = body::schematic_data(&stack);
    data.file.as_ref()?;
    Some((stack, data))
}

/// The world box of the schematic deployed at `anchor` (`SchematicTransformation`'s bounds:
/// the template's corners turned as the printer turns them).
fn world_box(data: &SchematicData, anchor: [i32; 3]) -> Option<([f64; 3], [f64; 3])> {
    let size = data.bounds?;
    let far = [size[0] - 1, size[1] - 1, size[2] - 1];
    let a = transform([0; 3], data.mirror, data.rotation, [0; 3]);
    let b = transform(far, data.mirror, data.rotation, [0; 3]);
    Some((
        [0, 1, 2].map(|i| f64::from(anchor[i] + a[i].min(b[i]))),
        [0, 1, 2].map(|i| f64::from(anchor[i] + a[i].max(b[i]) + 1)),
    ))
}

/// `DeployTool`'s target: the block looked at (75 blocks), on its face unless replaceable,
/// less the schematic's centre across.
fn target(host: &dyn ClientHost, data: &SchematicData) -> Option<[i32; 3]> {
    let hit = host.raycast(75.0)?;
    let pos = [hit.block_x, hit.block_y, hit.block_z];
    let replaceable = host
        .level(host.dimension())
        .and_then(|l| l.block_state(host, pos))
        .and_then(|s| mod_api::HostBlockStates::block_states(host).replaceable(s))
        .unwrap_or(false);
    let at = if replaceable {
        pos
    } else {
        let face = super::quill::face_of(pos, [hit.x, hit.y, hit.z]).offset();
        [pos[0] + face[0], pos[1] + face[1], pos[2] + face[2]]
    };
    let (min, max) = world_box(data, [0; 3])?;
    let center = [0, 2].map(|i| ((min[i] + max[i]) / 2.0) as i32);
    Some([at[0] - center[0], at[1], at[2] - center[1]])
}

fn outline(event: &mut RenderLevelStageEvent<'_>, (min, max): ([f64; 3], [f64; 3]), color: u32) {
    let (min, max) = (min.map(|v| v - 0.005), max.map(|v| v + 0.005));
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
}

/// The deployed schematic's bounds, or where it would go.
pub(super) fn render_level_stage(host: &dyn ClientHost, event: &mut RenderLevelStageEvent<'_>) {
    if event.stage != RenderLevelStage::AfterTranslucentBlocks {
        return;
    }
    let Some((_, data)) = held(host) else {
        return;
    };
    let anchor = match (data.deployed, data.anchor) {
        (Some(true), Some(a)) => a,
        _ => match target(host, &data) {
            Some(t) => t,
            None => return,
        },
    };
    if let Some(b) = world_box(&data, anchor) {
        let color = if data.deployed == Some(true) {
            0xFFFF_FFFF
        } else {
            0xFF68_86C5
        };
        outline(event, b, color);
    }
}

/// `D`, the anchor, rotation and mirror: the held schematic deployed so.
fn send(host: &mut dyn ClientHost, anchor: [i32; 3], rotation: usize, mirror: usize) {
    let mut data = vec![b'D', rotation as u8, mirror as u8];
    for v in anchor {
        data.extend_from_slice(&v.to_le_bytes());
    }
    net::send_to_server(host, CHANNEL, &data);
}

/// A right click with the schematic: deployed where looked at, or, sneaking, its screen.
pub(super) fn interaction_key(
    host: &mut dyn ClientHost,
    event: &mut InteractionKeyMappingTriggeredEvent,
) -> Flow {
    if event.key != InteractionKey::Use || event.hand != Hand::Main || !host.playing() {
        return Flow::Continue;
    }
    let Some((_, data)) = held(&*host) else {
        return Flow::Continue;
    };
    event.swing_hand = false;
    let rotation = Rotation::ALL
        .iter()
        .position(|r| *r == data.rotation)
        .unwrap_or(0);
    let mirror = Mirror::ALL
        .iter()
        .position(|m| *m == data.mirror)
        .unwrap_or(0);
    let sneaking = host
        .local_player()
        .is_some_and(|p| p.entity().is_shift_key_down(&*host) == Some(true));
    if sneaking {
        // `SchematicEditScreen.init`: the anchor, else the player's own block.
        let anchor = match (data.deployed, data.anchor) {
            (Some(true), Some(a)) => Some(a),
            _ => host
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
                rotation,
                mirror,
                focus: None,
                opened: std::time::Instant::now(),
            })
        });
        if !host.mod_screen_open(MOD) {
            OPEN.with_borrow_mut(|o| *o = None);
        }
        return Flow::Cancel;
    }
    if let Some(t) = target(&*host, &data) {
        send(host, t, rotation, mirror);
    }
    Flow::Cancel
}

pub(super) fn tick(host: &mut dyn ClientHost) {
    let (edit, close) = LATER.with_borrow_mut(std::mem::take);
    if let Some((anchor, r, m)) = edit {
        send(host, anchor, r, m);
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
    let title = held(host).and_then(|(_, d)| d.file).unwrap_or_default();
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

/// `onClose`: the edit sent (`SchematicSyncPacket`, `deployed` and the anchor typed).
fn confirm() {
    let edit = OPEN.with_borrow_mut(Option::take);
    let Some(e) = edit else {
        return;
    };
    let parse = |s: &str| s.parse::<i32>().unwrap_or(0);
    let anchor = [
        parse(&e.fields[0]),
        parse(&e.fields[1]),
        parse(&e.fields[2]),
    ];
    LATER.with_borrow_mut(|l| *l = (Some((anchor, e.rotation, e.mirror)), true));
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

/// The server's half: the held schematic deployed as the client asked.
pub(super) fn client_message(host: &mut dyn ServerHost, player: PlayerRef, rest: &[u8]) {
    if rest.len() < 14 {
        return;
    }
    let rotation = Rotation::ALL[usize::from(rest[0]) % 4];
    let mirror = Mirror::ALL[usize::from(rest[1]) % 3];
    let int = |i: usize| {
        let at = 2 + i * 4;
        i32::from_le_bytes([rest[at], rest[at + 1], rest[at + 2], rest[at + 3]])
    };
    let anchor = [int(0), int(1), int(2)];
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return;
    };
    let Some(mut lv) = super::ServerLevel::new(host, &dimension) else {
        return;
    };
    let Some((_, stack)) = super::held(&lv, player) else {
        return;
    };
    let Some(file) = body::schematic_data(&stack).file else {
        return;
    };
    super::deploy(
        &mut lv,
        Some(player),
        &file,
        anchor,
        Some(rotation.name()),
        Some(mirror.name()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turned_schematic_keeps_its_box_at_the_anchor() {
        let mut data = SchematicData::written("a.nbt", "me", [3, 2, 1]);
        assert_eq!(
            world_box(&data, [10, 64, 10]),
            Some(([10.0, 64.0, 10.0], [13.0, 66.0, 11.0]))
        );
        // Clockwise a quarter: x runs to -z... z of 1 and x of 3 swap round the anchor.
        data.rotation = Rotation::Clockwise90;
        let (min, max) = world_box(&data, [0, 0, 0]).unwrap();
        assert_eq!([max[0] - min[0], max[2] - min[2]], [1.0, 3.0]);
    }
}
