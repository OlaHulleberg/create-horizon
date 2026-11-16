//! The written schematic in a player's hand on the client (`SchematicHandler` and the tools of
//! `client/tools`): its outline, where a right click would deploy it or where it is deployed,
//! and the tool selection overlay above the hotbar (`ToolSelectionScreen`), focused while the
//! tool menu key (left Alt) is held, the wheel then choosing a tool, which is equipped as the key
//! is let go. Until the schematic is deployed the only tool is Position (`DeployTool`); then
//! Move XZ, Move Y, Position, Rotate, Mirror, and in creative Print. Ctrl and the wheel work the
//! tool; a right click deploys, mirrors or prints. The schematic's place, turn and mirror are
//! the client's (`SchematicTransformation`, `transform.rs`), chased smoothly, and sent to the
//! server's stack ten ticks after the last change (`SchematicSyncPacket`); Print places it at
//! once in creative (`SchematicPlacePacket`, `super::print_instantly`).
//!
//! The deployed schematic's blocks are not drawn (`SchematicRenderer`): no world draw takes a
//! block state's model outside a block entity (`API_GAPS.md`). Its outline, checkered faces,
//! the face looked at, the rotation axis and the mirror plane are drawn as Create's
//! (`AABBOutline`, `LineOutline`); the checkered faces sample Create's checkerboard
//! (`AllSpecialTextures.CHECKERED`) through the glue's `Checkered` pipeline (`pipelines`).

use super::super::net;
use super::super::pipelines::Textured;
use super::body;
use super::quill::CHANNEL;
use super::transform::{Placing, trace_until};
use create_engine::kinetics::Dir;
use create_engine::schematic::{Mirror, Rotation, SchematicData};
use mod_api::{
    ClientHost, DrawContext, Flow, GameMode, Hand, InputKeyEvent, InteractionAttempt,
    InteractionKey, InteractionKeyMappingTriggeredEvent, ItemStack, KeyAction, MouseScrollingEvent,
    PipelineQuad, RenderLevelStage, RenderLevelStageEvent, WorldDepth, WorldQuad,
};
use std::cell::RefCell;

const MOD: &str = "create";
/// `AllKeys.TOOL_MENU`'s id.
pub(super) const TOOL_MENU: &str = "toolmenu";
const ICONS: &str = "create:gui/icons";
const WIDGETS: &str = "create:gui/widgets";
/// `AllGuiTextures.HUD_BACKGROUND`: `overlay`, 16 × 16, repeated.
const OVERLAY: &str = "create:gui/overlay";
/// `SchematicHandler.SYNC_DELAY`.
const SYNC_DELAY: i32 = 10;
/// The outline's colour (`outline.getParams().colored(0x6886c5)`).
const BLUE: u32 = 0x6886C5;
/// The rotate and mirror tools' guides (`colored(0xdddddd)`).
const GREY: u32 = 0xDDDDDD;
/// `lineWidth(1 / 16f)`.
const LINE: f64 = 1.0 / 16.0;

/// `ToolType`, in its declared order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tool {
    Deploy,
    Move,
    MoveY,
    Rotate,
    Flip,
    Print,
}

impl Tool {
    /// `Lang.asId(name())`.
    fn id(self) -> &'static str {
        match self {
            Tool::Deploy => "deploy",
            Tool::Move => "move",
            Tool::MoveY => "move_y",
            Tool::Rotate => "rotate",
            Tool::Flip => "flip",
            Tool::Print => "print",
        }
    }

    /// Its icon in `icons.png` (`AllIcons.I_TOOL_DEPLOY`, `I_TOOL_MOVE_XZ`, `I_TOOL_MOVE_Y`,
    /// `I_TOOL_ROTATE`, `I_TOOL_MIRROR`, `I_CONFIRM`).
    fn icon(self) -> [f32; 2] {
        match self {
            Tool::Deploy => [0.0, 48.0],
            Tool::Move => [0.0, 64.0],
            Tool::MoveY => [16.0, 64.0],
            Tool::Rotate => [32.0, 64.0],
            Tool::Flip => [48.0, 64.0],
            Tool::Print => [0.0, 16.0],
        }
    }

    /// `ToolType.getTools`.
    fn all(creative: bool) -> Vec<Tool> {
        let mut tools = vec![
            Tool::Move,
            Tool::MoveY,
            Tool::Deploy,
            Tool::Rotate,
            Tool::Flip,
        ];
        if creative {
            tools.push(Tool::Print);
        }
        tools
    }
}

/// `SchematicToolBase`'s fields (one set: only the equipped tool's are ever read, and
/// equipping one starts them over, `init`).
#[derive(Clone, Debug, Default)]
struct Aim {
    selected: Option<[i32; 3]>,
    chasing: [f64; 3],
    last_chasing: [f64; 3],
    ignore_blocks: bool,
    range: i32,
    schematic_selected: bool,
    face: Option<Dir>,
}

/// `ToolSelectionScreen`.
#[derive(Clone, Debug)]
struct Selection {
    tools: Vec<Tool>,
    selection: usize,
    focused: bool,
    y_offset: f32,
}

impl Selection {
    fn new(tools: Vec<Tool>) -> Selection {
        Selection {
            tools,
            selection: 0,
            focused: false,
            y_offset: 0.0,
        }
    }

    /// `cycle`: the wheel up goes left.
    fn cycle(&mut self, direction: i32) {
        let n = self.tools.len() as i32;
        let s = self.selection as i32 + if direction < 0 { 1 } else { -1 };
        self.selection = (s + n).rem_euclid(n) as usize;
    }

    /// `update`.
    fn update(&mut self) {
        if self.focused {
            self.y_offset += (10.0 - self.y_offset) * 0.1;
        } else {
            self.y_offset *= 0.9;
        }
    }
}

/// `SchematicHandler`.
struct Handler {
    displayed: Option<String>,
    active: bool,
    deployed: bool,
    size: Option<[i32; 3]>,
    transformation: Placing,
    tool: Tool,
    aim: Aim,
    screen: Selection,
    sync_cooldown: i32,
    slot: u8,
    /// The last press of the use key was a tool's (the held repeats are dropped with it).
    took_press: bool,
}

impl Handler {
    fn new() -> Handler {
        let mut h = Handler {
            displayed: None,
            active: false,
            deployed: false,
            size: None,
            transformation: Placing::default(),
            tool: Tool::Deploy,
            aim: Aim::default(),
            screen: Selection::new(vec![Tool::Deploy]),
            sync_cooldown: 0,
            slot: 0,
            took_press: false,
        };
        h.equip(Tool::Deploy);
        h
    }

    /// A new `ToolSelectionScreen`, whose constructor equips its first tool.
    fn select_from(&mut self, tools: Vec<Tool>) {
        let first = tools[0];
        self.screen = Selection::new(tools);
        self.equip(first);
    }

    /// `equip`: the tool's `init`.
    fn equip(&mut self, tool: Tool) {
        self.tool = tool;
        let ignore_blocks = tool == Tool::Deploy && self.aim.ignore_blocks;
        self.aim = Aim {
            range: if tool == Tool::Deploy { -1 } else { 0 },
            ignore_blocks,
            ..Aim::default()
        };
    }

    /// `loadSettings` and `init`.
    fn init(&mut self, data: &SchematicData, creative: bool) {
        self.transformation = Placing::default();
        self.deployed = data.deployed.unwrap_or(false);
        let anchor = data.anchor.unwrap_or([0; 3]);
        if let Some(size) = data.bounds {
            self.size = Some(size);
            self.transformation
                .init(anchor, data.rotation, data.mirror, size);
        }
        self.displayed = data.file.clone();
        self.active = true;
        if self.deployed {
            let before = self.tool;
            self.select_from(Tool::all(creative));
            if let Some(i) = self.screen.tools.iter().position(|t| *t == before) {
                self.screen.selection = i;
            }
            self.equip(before);
        } else {
            self.select_from(vec![Tool::Deploy]);
        }
    }

    /// `deploy`.
    fn deploy(&mut self, creative: bool) {
        if !self.deployed {
            self.select_from(Tool::all(creative));
        }
        self.deployed = true;
    }

    fn mark_dirty(&mut self) {
        self.sync_cooldown = SYNC_DELAY;
    }

    /// `bounds.getCenter()`.
    fn center(&self) -> [f64; 3] {
        self.size.unwrap_or([0; 3]).map(|v| f64::from(v) / 2.0)
    }
}

thread_local! {
    static HANDLER: RefCell<Handler> = RefCell::new(Handler::new());
    /// Messages for the server, from hooks without a mutable client.
    static OUT: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn reset() {
    HANDLER.with_borrow_mut(|h| *h = Handler::new());
    OUT.with_borrow_mut(Vec::clear);
}

// ---------------------------------------------------------------------------------------------
// The handler's tick

fn game_mode(host: &dyn ClientHost) -> Option<GameMode> {
    host.local_player().and_then(|p| p.game_mode(host))
}

fn creative(host: &dyn ClientHost) -> bool {
    game_mode(host) == Some(GameMode::Creative)
}

/// `findBlueprintInHand`: a written schematic in the main hand, its slot and data.
fn held(host: &dyn ClientHost) -> Option<(u8, ItemStack, SchematicData)> {
    let player = host.local_player()?;
    let stack = host.living(player.entity())?.hand_item(host, Hand::Main)?;
    if stack.item().id(host) != super::SCHEMATIC {
        return None;
    }
    let data = body::schematic_data(&stack);
    data.file.as_ref()?;
    let slot = player.selected_slot(host).unwrap_or(0) as u8;
    Some((slot, stack, data))
}

/// The view: the eye and the unit look vector (`getLookAngle`).
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

/// `AllKeys.ctrlDown()` and `ACTIVATE_TOOL.isPressed()`: left Ctrl.
fn ctrl(host: &dyn ClientHost) -> bool {
    host.is_key_down("key.keyboard.left.control") || host.modifiers().control()
}

/// `SchematicHandler.tick`.
pub(super) fn tick(host: &mut dyn ClientHost) {
    for m in OUT.with_borrow_mut(std::mem::take) {
        net::send_to_server(host, CHANNEL, &m);
    }
    let host: &dyn ClientHost = host;
    if game_mode(host) == Some(GameMode::Spectator) {
        HANDLER.with_borrow_mut(|h| {
            if h.active {
                h.active = false;
                h.sync_cooldown = 0;
                h.slot = 0;
            }
        });
        return;
    }
    HANDLER.with_borrow_mut(|h| h.transformation.tick());
    let Some((slot, _, data)) = held(host) else {
        HANDLER.with_borrow_mut(|h| {
            h.active = false;
            h.sync_cooldown = 0;
        });
        return;
    };
    let creative = creative(host);
    let sync = HANDLER.with_borrow_mut(|h| {
        h.slot = slot;
        if !h.active || data.file != h.displayed {
            h.init(&data, creative);
        }
        if h.sync_cooldown > 0 {
            h.sync_cooldown -= 1;
        }
        h.screen.update();
        h.sync_cooldown == 1
    });
    if sync {
        HANDLER.with_borrow(sync_message);
    }
    update_selection(host);
}

/// `SchematicSyncPacket`: `Y`, the slot, deployed, rotation, mirror and anchor.
fn sync_message(h: &Handler) {
    let (rotation, mirror) = h.transformation.to_settings();
    let mut m = vec![
        b'Y',
        h.slot,
        u8::from(h.deployed),
        rotation as u8,
        mirror as u8,
    ];
    for v in h.transformation.anchor() {
        m.extend_from_slice(&v.to_le_bytes());
    }
    OUT.with_borrow_mut(|o| o.push(m));
}

/// The tool's `updateSelection`.
fn update_selection(host: &dyn ClientHost) {
    let ctrl = ctrl(host);
    let view = view(host);
    let hit = host.raycast(75.0).map(|hit| {
        let at = [hit.x, hit.y, hit.z];
        // `BlockPos.containing(trace.getLocation())`, then a step out through a top or bottom
        // face unless the block there is replaceable.
        let mut pos = at.map(|v| v.floor() as i32);
        let face = super::quill::face_of([hit.block_x, hit.block_y, hit.block_z], at);
        let replaceable = host
            .level(host.dimension())
            .and_then(|l| l.block_state(host, pos))
            .and_then(|s| mod_api::HostBlockStates::block_states(host).replaceable(s))
            .unwrap_or(false);
        if matches!(face, Dir::Up | Dir::Down) && !replaceable {
            let o = face.offset();
            pos = [0, 1, 2].map(|i| pos[i] + o[i]);
        }
        pos
    });
    HANDLER.with_borrow_mut(|h| {
        if !h.active {
            return;
        }
        if h.tool == Tool::Deploy {
            if h.aim.range == -1 {
                let c = h.center();
                let length = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
                h.aim.range = ((length / 2.0) as i32).clamp(1, 100);
            }
            h.aim.ignore_blocks = ctrl;
        }
        update_target(h, view, hit);
        let Some(selected) = h.aim.selected else {
            return;
        };
        h.aim.last_chasing = h.aim.chasing;
        let target = selected.map(f64::from);
        let d = [0, 1, 2].map(|i| target[i] - h.aim.chasing[i]);
        if (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() < 1.0 / 512.0 {
            h.aim.chasing = target;
            return;
        }
        h.aim.chasing = [0, 1, 2].map(|i| h.aim.chasing[i] + d[i] * 0.5);
    });
}

/// `updateTargetPos`: the face of the deployed schematic looked at, and the block a click would
/// pick (or, holding Ctrl with Position, the point `range` blocks ahead).
fn update_target(h: &mut Handler, view: Option<([f64; 3], [f64; 3])>, hit: Option<[i32; 3]>) {
    let Some((eye, look)) = view else {
        h.aim.selected = None;
        return;
    };
    if h.deployed {
        let size = h.size.unwrap_or([0; 3]).map(f64::from);
        let start = h.transformation.to_local(eye, 1.0);
        let end = h
            .transformation
            .to_local([0, 1, 2].map(|i| eye[i] + look[i] * 70.0), 1.0);
        let found = trace_until(start, end, |p| {
            (0..3).all(|i| {
                let c = f64::from(p[i]) + 0.5;
                c >= 0.0 && c < size[i]
            })
        });
        h.aim.schematic_selected = found.is_some();
        h.aim.face = found.map(|(_, f)| f);
    }
    let snap = h.aim.selected.is_none();
    if h.aim.ignore_blocks {
        let range = f64::from(h.aim.range);
        let at = [0, 1, 2].map(|i| (eye[i] + look[i] * range).floor() as i32);
        h.aim.selected = Some(at);
        if snap {
            h.aim.chasing = at.map(f64::from);
            h.aim.last_chasing = h.aim.chasing;
        }
        return;
    }
    h.aim.selected = hit;
    if snap && let Some(at) = hit {
        h.aim.chasing = at.map(f64::from);
        h.aim.last_chasing = h.aim.chasing;
    }
}

// ---------------------------------------------------------------------------------------------
// Input

/// `onKeyInput`: holding the tool menu key focuses the overlay; letting it go equips the tool
/// chosen (`onClose`). Only with no screen open.
pub(super) fn input_key(host: &mut dyn ClientHost, event: &mut InputKeyEvent<'_>) {
    if !host.playing() {
        return;
    }
    let bound = host
        .mod_key(MOD, TOOL_MENU)
        .map_or_else(|| "key.keyboard.left.alt".to_owned(), |(k, _)| k);
    if event.key != bound {
        return;
    }
    let pressed = event.action != KeyAction::Release;
    HANDLER.with_borrow_mut(|h| {
        if !h.active {
            return;
        }
        if pressed && !h.screen.focused {
            h.screen.focused = true;
        }
        if !pressed && h.screen.focused {
            h.screen.focused = false;
            let tool = h.screen.tools[h.screen.selection];
            h.equip(tool);
        }
    });
}

/// `mouseScrolled`: the focused overlay cycles; with Ctrl the tool takes the wheel.
pub(super) fn mouse_scrolling(host: &mut dyn ClientHost, event: &mut MouseScrollingEvent) -> Flow {
    let delta = event.scroll[1];
    let ctrl = ctrl(&*host);
    let taken = HANDLER.with_borrow_mut(|h| {
        if !h.active {
            return false;
        }
        if h.screen.focused {
            h.screen
                .cycle(delta.signum() as i32 * i32::from(delta != 0.0));
            return true;
        }
        ctrl && mouse_wheel(h, delta)
    });
    if taken { Flow::Cancel } else { Flow::Continue }
}

/// The tool's `handleMouseWheel`.
fn mouse_wheel(h: &mut Handler, delta: f64) -> bool {
    match h.tool {
        Tool::Deploy => {
            if !h.aim.ignore_blocks {
                return false;
            }
            h.aim.range = ((f64::from(h.aim.range) + delta) as i32).clamp(1, 100);
            true
        }
        Tool::Move => {
            let Some(face) = h.aim.face.filter(|_| h.aim.schematic_selected) else {
                return true;
            };
            if matches!(face, Dir::Up | Dir::Down) {
                return true;
            }
            let t = &h.transformation;
            let o = face.offset().map(f64::from);
            let s = -delta.signum();
            let v = [
                o[0] * s * f64::from(t.mirror_modifier_x()),
                0.0,
                o[2] * s * f64::from(t.mirror_modifier_z()),
            ];
            let v = super::transform::rotate_y(v, f64::from(t.rotation_target()));
            h.transformation.shift([v[0] as i32, 0, v[2] as i32]);
            h.mark_dirty();
            true
        }
        Tool::MoveY => {
            if h.deployed {
                let sign = if delta > 0.0 {
                    1
                } else if delta < 0.0 {
                    -1
                } else {
                    0
                };
                h.transformation.shift([0, sign, 0]);
                h.mark_dirty();
            }
            true
        }
        Tool::Rotate => {
            h.transformation.rotate90(delta > 0.0);
            h.mark_dirty();
            true
        }
        Tool::Flip => {
            mirror(h);
            true
        }
        Tool::Print => false,
    }
}

/// `FlipTool.mirror`: across the face looked at, a side of the schematic.
fn mirror(h: &mut Handler) {
    let Some(face) = h.aim.face.filter(|_| h.aim.schematic_selected) else {
        return;
    };
    match face {
        Dir::East | Dir::West => h.transformation.flip_x(),
        Dir::North | Dir::South => h.transformation.flip_z(),
        _ => return,
    }
    h.mark_dirty();
}

/// The tool's `handleRightClick`.
fn right_click(h: &mut Handler, creative: bool) -> bool {
    match h.tool {
        Tool::Deploy => {
            let Some(selected) = h.aim.selected else {
                return false;
            };
            let c = h.center();
            let target = [
                selected[0] - c[0] as i32,
                selected[1],
                selected[2] - c[2] as i32,
            ];
            h.transformation.start_at(target);
            h.transformation.move_to(target);
            h.mark_dirty();
            h.deploy(creative);
            true
        }
        Tool::Flip => {
            mirror(h);
            true
        }
        Tool::Print => {
            print_instantly(h);
            true
        }
        Tool::Move | Tool::MoveY | Tool::Rotate => false,
    }
}

/// `printInstantly`: the server places it (with the place it has now, sent first), and the
/// schematic is no longer deployed.
fn print_instantly(h: &mut Handler) {
    sync_message(h);
    OUT.with_borrow_mut(|o| o.push(vec![b'P', h.slot]));
    h.deployed = false;
    h.select_from(vec![Tool::Deploy]);
    h.mark_dirty();
}

/// `onMouseInput` for the right button: the tool's click, unless sneaking (the edit screen) or
/// at a schematicannon or deployer.
pub(super) fn interaction_key(
    host: &mut dyn ClientHost,
    event: &mut InteractionKeyMappingTriggeredEvent,
) -> Flow {
    if event.key != InteractionKey::Use || event.hand != Hand::Main {
        return Flow::Continue;
    }
    if !HANDLER.with_borrow(|h| h.active) {
        return Flow::Continue;
    }
    if event.attempt == InteractionAttempt::Held {
        if HANDLER.with_borrow(|h| h.took_press) {
            event.swing_hand = false;
            return Flow::Cancel;
        }
        return Flow::Continue;
    }
    HANDLER.with_borrow_mut(|h| h.took_press = false);
    let sneaking = host
        .local_player()
        .is_some_and(|p| p.entity().is_shift_key_down(&*host) == Some(true));
    if sneaking {
        return Flow::Continue;
    }
    if let Some(hit) = host.raycast(reach(&*host)) {
        let at = |id: &str| host.block_key(id) == Some(hit.block);
        if at("create:schematicannon") || at("create:deployer") {
            return Flow::Continue;
        }
    }
    let creative = creative(&*host);
    let taken = HANDLER.with_borrow_mut(|h| {
        let taken = right_click(h, creative);
        h.took_press = taken;
        taken
    });
    if !taken {
        return Flow::Continue;
    }
    event.swing_hand = false;
    for m in OUT.with_borrow_mut(std::mem::take) {
        net::send_to_server(host, CHANNEL, &m);
    }
    Flow::Cancel
}

/// How far the block the click is at may be (`mc.hitResult`): the player's block interaction
/// range, 4.5, and half a block more in creative.
fn reach(host: &dyn ClientHost) -> f32 {
    if creative(host) { 5.0 } else { 4.5 }
}

// ---------------------------------------------------------------------------------------------
// The edit screen's side (`SchematicEditScreen`)

/// What the edit screen starts from: the schematic's name, its anchor (or none when not
/// deployed), and its rotation and mirror (`toSettings`).
pub(super) fn edit_start() -> Option<(String, Option<[i32; 3]>, Rotation, Mirror)> {
    HANDLER.with_borrow(|h| {
        if !h.active {
            return None;
        }
        let (r, m) = h.transformation.to_settings();
        let anchor = h.deployed.then(|| h.transformation.anchor());
        Some((
            h.displayed.clone().unwrap_or_else(|| "-".into()),
            anchor,
            r,
            m,
        ))
    })
}

/// `SchematicHandler.getCurrentSchematicName`.
pub(super) fn current_name() -> String {
    HANDLER.with_borrow(|h| h.displayed.clone().unwrap_or_else(|| "-".into()))
}

/// `SchematicEditScreen.removed` with valid coordinates: deployed there, turned and mirrored.
pub(super) fn edited(host: &dyn ClientHost, anchor: [i32; 3], rotation: Rotation, mirror: Mirror) {
    let creative = creative(host);
    HANDLER.with_borrow_mut(|h| {
        if let Some(size) = h.size {
            h.transformation.init(anchor, rotation, mirror, size);
        }
        h.mark_dirty();
        h.deploy(creative);
    });
}

// ---------------------------------------------------------------------------------------------
// Drawing: the outlines

/// A point of an outline's own space where it is drawn.
type Place<'a> = &'a dyn Fn([f64; 3]) -> [f64; 3];

/// A checkered face: its corners, their uv in blocks, its colour.
type Checkered = ([[f64; 3]; 4], [[f32; 2]; 4], u32);

/// Geometry for one frame.
#[derive(Default)]
struct Geometry {
    solid: Vec<WorldQuad>,
    /// Checkered faces: corners, their uv in blocks, colour.
    checkered: Vec<Checkered>,
}

fn argb(rgb: u32, alpha: f32) -> u32 {
    (((alpha.clamp(0.0, 1.0) * 255.0) as u32) << 24) | (rgb & 0xFF_FFFF)
}

/// `bufferCuboid`: a box of solid colour, its six faces.
fn cuboid(g: &mut Geometry, min: [f64; 3], max: [f64; 3], color: u32, place: Place<'_>) {
    for (axis, side) in [(0, 0), (0, 1), (1, 0), (1, 1), (2, 0), (2, 1)] {
        let corners = face_corners(min, max, axis, side).map(place);
        g.solid
            .push(WorldQuad::new(corners, color, WorldDepth::Tested));
    }
}

/// The four corners of the face of `min..max` across `axis` on `side` (0 the low one).
fn face_corners(min: [f64; 3], max: [f64; 3], axis: usize, side: usize) -> [[f64; 3]; 4] {
    let (u, v) = match axis {
        0 => (2, 1),
        1 => (0, 2),
        _ => (0, 1),
    };
    let at = |a: f64, b: f64| {
        let mut p = [0.0; 3];
        p[axis] = if side == 0 { min[axis] } else { max[axis] };
        p[u] = a;
        p[v] = b;
        p
    };
    [
        at(min[u], max[v]),
        at(min[u], min[v]),
        at(max[u], min[v]),
        at(max[u], max[v]),
    ]
}

/// `bufferCuboidLine` along one axis: from `origin`, `length` along `axis`, `width` across.
fn cuboid_line(
    g: &mut Geometry,
    origin: [f64; 3],
    axis: usize,
    length: f64,
    color: u32,
    place: Place<'_>,
) {
    let half = LINE / 2.0;
    let mut min = origin.map(|v| v - half);
    let mut max = origin.map(|v| v + half);
    if length >= 0.0 {
        max[axis] += length;
    } else {
        min[axis] += length;
    }
    cuboid(g, min, max, color, place);
}

/// `AABBOutline.renderBox`: its twelve edges and, when `faces` gives their alpha, its faces
/// checkered (the highlighted one at full alpha). `camera_inside` is `box.contains(camera)`
/// in the outline's own space; with it the box is pulled in a hair and nothing is culled, as
/// Create draws all three of its schematic outlines; without it, only faces turned to the
/// `eye` are drawn (`cull`).
#[allow(clippy::too_many_arguments)]
fn outline(
    g: &mut Geometry,
    min: [f64; 3],
    max: [f64; 3],
    rgb: u32,
    faces: Option<Option<Dir>>,
    camera_inside: bool,
    eye: [f64; 3],
    place: Place<'_>,
) {
    let inflate = if camera_inside {
        -1.0 / 128.0
    } else {
        1.0 / 128.0
    };
    let min = min.map(|v| v - inflate);
    let max = max.map(|v| v + inflate);
    if let Some(highlight) = faces {
        for d in Dir::ALL {
            let o = d.offset();
            let axis = (0..3).find(|&i| o[i] != 0).unwrap_or(1);
            let side = usize::from(o[axis] > 0);
            let local = face_corners(min, max, axis, side);
            let corners = local.map(place);
            if !camera_inside {
                // Cull: the face's outward normal against the eye.
                let c = [0, 1, 2].map(|i| corners.iter().map(|p| p[i]).sum::<f64>() / 4.0);
                let e1 = [0, 1, 2].map(|i| corners[1][i] - corners[0][i]);
                let e2 = [0, 1, 2].map(|i| corners[2][i] - corners[0][i]);
                let n = [
                    e1[1] * e2[2] - e1[2] * e2[1],
                    e1[2] * e2[0] - e1[0] * e2[2],
                    e1[0] * e2[1] - e1[1] * e2[0],
                ];
                let mid = [0, 1, 2].map(|i| (min[i] + max[i]) / 2.0);
                let out =
                    place([0, 1, 2].map(|i| mid[i] + f64::from(o[i]) * (max[i] - min[i]).max(1.0)));
                let centre = place(mid);
                let outward = [0, 1, 2].map(|i| out[i] - centre[i]);
                let sign = if n.iter().zip(outward).map(|(a, b)| a * b).sum::<f64>() < 0.0 {
                    -1.0
                } else {
                    1.0
                };
                let facing: f64 = (0..3).map(|i| n[i] * sign * (eye[i] - c[i])).sum();
                if facing <= 0.0 {
                    continue;
                }
            }
            let (u, v) = match axis {
                0 => (2, 1),
                1 => (0, 2),
                _ => (0, 1),
            };
            let (du, dv) = ((max[u] - min[u]) as f32, (max[v] - min[v]) as f32);
            let uv = [[0.0, 0.0], [0.0, dv], [du, dv], [du, 0.0]];
            let alpha = if highlight == Some(d) { 1.0 } else { 0.5 };
            g.checkered.push((corners, uv, argb(rgb, alpha)));
        }
    }
    let color = argb(rgb, 1.0);
    let len = [0, 1, 2].map(|i| max[i] - min[i]);
    let edges: [([f64; 3], usize); 12] = [
        (min, 0),
        (min, 1),
        (min, 2),
        ([max[0], min[1], min[2]], 1),
        ([max[0], min[1], min[2]], 2),
        ([min[0], max[1], min[2]], 0),
        ([min[0], max[1], min[2]], 2),
        ([min[0], min[1], max[2]], 0),
        ([min[0], min[1], max[2]], 1),
        ([min[0], max[1], max[2]], 0),
        ([max[0], min[1], max[2]], 1),
        ([max[0], max[1], min[2]], 2),
    ];
    for (origin, axis) in edges {
        cuboid_line(g, origin, axis, len[axis], color, place);
    }
}

/// Whether `p` is in `min..max` as `AABB.contains` takes it.
fn contains(min: [f64; 3], max: [f64; 3], p: [f64; 3]) -> bool {
    (0..3).all(|i| p[i] >= min[i] && p[i] < max[i])
}

/// `SchematicHandler.render`: the tool's own drawing, then, carried by the transformation,
/// the deployed schematic's outline and the tool's drawing on it.
pub(super) fn render_level_stage(host: &dyn ClientHost, event: &mut RenderLevelStageEvent<'_>) {
    if event.stage != RenderLevelStage::AfterTranslucentBlocks {
        return;
    }
    let pt = event.view.partial_tick;
    let eye = event.view.camera;
    let mut g = Geometry::default();
    let drew = HANDLER.with_borrow(|h| {
        if !h.active {
            return false;
        }
        let Some(size) = h.size else {
            return false;
        };
        let bounds = ([0.0; 3], size.map(f64::from));
        let t = &h.transformation;
        // `renderTool`: Position's outline where a click would deploy it.
        if h.tool == Tool::Deploy && h.aim.selected.is_some() {
            let a = &h.aim;
            let at = [0, 1, 2]
                .map(|i| a.last_chasing[i] + (a.chasing[i] - a.last_chasing[i]) * f64::from(pt));
            let c = h.center();
            let origin = [bounds.1[0] / 2.0, 0.0, bounds.1[2] / 2.0];
            let ro = t.rotation_offset(true);
            let rot = f64::from(t.current_rotation(pt));
            let shift = [
                at[0] - f64::from(c[0] as i32),
                at[1],
                at[2] - f64::from(c[2] as i32),
            ];
            let place = move |p: [f64; 3]| {
                let q = [0, 1, 2].map(|i| p[i] - origin[i] - ro[i]);
                let q = super::transform::rotate_y(q, rot);
                [0, 1, 2].map(|i| q[i] + origin[i] + ro[i] + shift[i])
            };
            let inside = contains(bounds.0, bounds.1, [0.0; 3]);
            outline(&mut g, bounds.0, bounds.1, BLUE, None, inside, eye, &place);
        }
        let place = |p: [f64; 3]| t.apply(p, pt);
        // `renderOnSchematic`.
        match h.tool {
            Tool::Rotate => {
                let c = h.center();
                let ro = t.rotation_offset(false);
                let height = bounds.1[1] + bounds.1[1].max(20.0);
                let centre = [c[0] + ro[0], c[1] + ro[1], c[2] + ro[2]];
                let start = [centre[0], centre[1] - height / 2.0, centre[2]];
                cuboid_line(&mut g, start, 1, height, argb(GREY, 1.0), &place);
            }
            Tool::Flip => {
                if let Some(face) = h.aim.face.filter(|_| h.aim.schematic_selected)
                    && !matches!(face, Dir::Up | Dir::Down)
                {
                    // The plane the mirror is across: the box squeezed to nothing along the
                    // clockwise of the face, a block wider each other way, at its middle.
                    let axis = match face {
                        Dir::North | Dir::South => 0,
                        _ => 2,
                    };
                    let (mut min, mut max) = bounds;
                    max[axis] = min[axis];
                    for i in 0..3 {
                        if i != axis {
                            min[i] -= 1.0;
                            max[i] += 1.0;
                        }
                    }
                    let half = bounds.1[axis] / 2.0;
                    min[axis] += half;
                    max[axis] += half;
                    let inside = contains(min, max, [0.0; 3]);
                    if h.deployed {
                        outline(&mut g, min, max, GREY, Some(None), inside, eye, &place);
                    }
                }
            }
            _ => {}
        }
        if h.deployed {
            let highlight = if h.tool == Tool::Move {
                h.aim.face
            } else {
                None
            };
            let inside = contains(bounds.0, bounds.1, [0.0; 3]);
            outline(
                &mut g,
                bounds.0,
                bounds.1,
                BLUE,
                Some(highlight),
                inside,
                eye,
                &place,
            );
        }
        true
    });
    if !drew {
        return;
    }
    let _ = event.draw.quads(&g.solid);
    if g.checkered.is_empty() {
        return;
    }
    // `AllSpecialTextures.CHECKERED` from the block atlas; without the pipeline the edges alone.
    if let Some(p) = super::super::pipelines::resolve(host, Textured::Checkered) {
        let quads: Vec<PipelineQuad> = g
            .checkered
            .iter()
            .map(|(c, uv, color)| PipelineQuad::new(*c, *uv, *color))
            .collect();
        if let Err(e) = event.draw.pipeline_quads(p, &quads) {
            crate::create_log!("[create] the schematic's checkered faces were refused: {e:?}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Drawing: the overlay

fn tr(host: &dyn ClientHost, key: &str) -> String {
    host.translate(key).unwrap_or_else(|| key.to_owned())
}

/// `AllGuiTextures.HUD_BACKGROUND` across `rect`, as `blit` repeats its 16 pixels.
fn hud_background(draw: &mut dyn DrawContext, rect: [f32; 4], tint: u32) {
    let [x, y, w, h] = rect;
    let mut dy = 0.0;
    while dy < h {
        let th = (h - dy).min(16.0);
        let mut dx = 0.0;
        while dx < w {
            let tw = (w - dx).min(16.0);
            draw.sprite(
                OVERLAY,
                [0.0, 0.0, tw, th],
                [x + dx, y + dy, tw, th],
                Some(tint),
            );
            dx += 16.0;
        }
        dy += 16.0;
    }
}

/// `drawCenteredString`: centred on `x`, with a shadow.
fn centered(draw: &mut dyn DrawContext, text: &str, x: f32, y: f32, argb: u32) {
    let w = draw.text_width(text);
    draw.text(text, [x - (w / 2.0).floor(), y], argb, true);
}

/// `SchematicHandler.render(GuiGraphics)`: the schematic's hotbar slot marked
/// (`SchematicHotbarSlotOverlay`) and the tool overlay (`ToolSelectionScreen.renderPassive`).
pub(super) fn render_gui_post(host: &dyn ClientHost, event: &mut mod_api::RenderGuiPostEvent<'_>) {
    let Some((tools, selection, focused, y_offset, slot)) = HANDLER.with_borrow(|h| {
        h.active.then(|| {
            (
                h.screen.tools.clone(),
                h.screen.selection,
                h.screen.focused,
                h.screen.y_offset,
                h.slot,
            )
        })
    }) else {
        return;
    };
    let draw = &mut *event.draw;
    let [gw, gh] = draw.size();
    // `SchematicHotbarSlotOverlay.renderOn`: under the slot's item, so the item drawn again.
    let at = [
        (gw / 2.0).floor() - 88.0 + 20.0 * f32::from(slot),
        gh - 19.0,
    ];
    draw.sprite(
        WIDGETS,
        [54.0, 0.0, 16.0, 16.0],
        [at[0], at[1], 16.0, 16.0],
        None,
    );
    if let Some((_, stack, _)) = held(host) {
        draw.item_stack(&stack, at, 16.0);
    }

    // `ToolSelectionScreen.draw`.
    let w = (tools.len() as f32 * 50.0 + 30.0).max(220.0);
    let h = 30.0;
    let mut x = ((gw - w) / 2.0).floor() + 15.0;
    let y = gh - h - 75.0 - y_offset;
    let bg_alpha = if focused { 7.0 / 8.0 } else { 1.0 / 2.0 };
    hud_background(draw, [x - 15.0, y, w, h], argb(0xFFFFFF, bg_alpha));

    let tip_alpha = y_offset / 10.0;
    if tip_alpha > 0.25 {
        let tint = (((tip_alpha * 255.0) as u32) << 24) | 0xB3_B3CC;
        hud_background(draw, [x - 15.0, y + 33.0, w, h + 22.0], tint);
        let a = ((tip_alpha * 255.0) as u32) << 24;
        let id = tools[selection].id();
        let lines: Vec<String> = (0..4)
            .map(|i| tr(host, &format!("create.schematic.tool.{id}.description.{i}")))
            .collect();
        for (i, (dy, rgb)) in [
            (38.0, 0xEEEEEE),
            (50.0, 0xCCDDFF),
            (60.0, 0xCCDDFF),
            (72.0, 0xCCCCDD),
        ]
        .into_iter()
        .enumerate()
        {
            draw.text(&lines[i], [x - 10.0, y + dy], a | rgb, false);
        }
    }

    if tools.len() > 1 {
        let hint = if focused {
            tr(host, "create.gui.toolmenu.cycle")
        } else {
            let key = host
                .mod_key(MOD, TOOL_MENU)
                .map_or_else(|| "key.keyboard.left.alt".to_owned(), |(k, _)| k);
            let name = tr(host, &key).to_uppercase();
            tr(host, "create.gui.toolmenu.focusKey")
                .replace("%1$s", &name)
                .replace("%s", &name)
        };
        centered(draw, &hint, (gw / 2.0).floor(), y - 10.0, 0xFFCC_DDFF);
    } else {
        x += 65.0;
    }

    for (i, tool) in tools.iter().enumerate() {
        let mut alpha = if focused { 1.0 } else { 0.2 };
        let mut dy = 0.0;
        let ix = x + i as f32 * 50.0;
        if i == selection {
            dy = -10.0;
            let name = tr(host, &format!("create.schematic.tool.{}", tool.id()));
            centered(draw, &name, ix + 24.0, y + 28.0 + dy, 0xFFCC_DDFF);
            alpha = 1.0;
        }
        let [u, v] = tool.icon();
        draw.sprite(
            ICONS,
            [u, v, 16.0, 16.0],
            [ix + 16.0, y + 12.0 + dy, 16.0, 16.0],
            Some(argb(0x000000, alpha)),
        );
        draw.sprite(
            ICONS,
            [u, v, 16.0, 16.0],
            [ix + 16.0, y + 11.0 + dy, 16.0, 16.0],
            Some(argb(0xFFFFFF, alpha)),
        );
    }
}

/// The tool menu key: left Alt (`AllKeys.TOOL_MENU`, "Focus Schematic Overlay").
pub(super) fn register(r: &mut dyn mod_api::Registrar) {
    let mut k = mod_api::KeyRegistration::new(TOOL_MENU, "key.keyboard.left.alt");
    k.label = "create.keyinfo.toolmenu".into();
    r.register_key_mapping(k);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wheel_up_cycles_left_round_the_tools() {
        let mut s = Selection::new(Tool::all(false));
        s.cycle(1);
        assert_eq!(s.tools[s.selection], Tool::Flip);
        s.cycle(-1);
        s.cycle(-1);
        assert_eq!(s.tools[s.selection], Tool::MoveY);
    }

    #[test]
    fn deploying_equips_move_and_reloading_keeps_the_tool() {
        let mut h = Handler::new();
        let mut data = SchematicData::written("a.nbt", "me", [3, 2, 5]);
        h.init(&data, true);
        assert_eq!(h.screen.tools, vec![Tool::Deploy]);
        h.aim.selected = Some([10, 64, 10]);
        assert!(right_click(&mut h, true));
        assert_eq!(h.tool, Tool::Move);
        assert_eq!(h.screen.tools.len(), 6);
        assert_eq!(h.transformation.anchor(), [9, 64, 8]);
        h.equip(Tool::Rotate);
        data.deployed = Some(true);
        h.init(&data, false);
        assert_eq!(h.tool, Tool::Rotate);
        assert_eq!(h.screen.tools[h.screen.selection], Tool::Rotate);
    }

    #[test]
    fn a_sync_waits_ten_ticks_after_the_last_change() {
        let mut h = Handler::new();
        h.mark_dirty();
        let mut synced = None;
        for t in 1..=12 {
            if h.sync_cooldown > 0 {
                h.sync_cooldown -= 1;
            }
            if h.sync_cooldown == 1 && synced.is_none() {
                synced = Some(t);
            }
        }
        assert_eq!(synced, Some(9));
    }
}
