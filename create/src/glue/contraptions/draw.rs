//! Moving structures on clients. The legacy glue sent each structure on its own channels
//! (`create:contraption`, `create:piston`, `create:lift`) and drew it as one part model turning
//! or gliding on the GPU. Here a structure travels in its anchor's block entity data
//! (`Contraption`), which the game sends to every client that sees the anchor (when its chunk
//! is sent and whenever it changes), and the anchor's block entity draws it: each block's model
//! (`BlockEntityDrawContext::block`) under a pose turning about the bearing's axis (`spin`, kept
//! between frames like the kinetic parts) or moved along its line each frame from the offset
//! and speed it was sent with. Elevators run their own ticks on the client, as Create's do.

use super::super::server::ServerLevel;
use super::super::states;
use super::super::subsystem::Subsystem;
use super::super::world::{level_index, with_world};
use super::state_text;
use create_engine::contraption::Turn;
use create_engine::elevator::{ColumnKey, Elevator};
use create_engine::kinetics::{Axis, Dir};
use mod_api::{
    BlockEntityDrawContext, BlockEntityOnLoadEvent, BlockEntityPart,
    BlockEntityRenderBoundingBoxEvent, BlockEntitySubmitEvent, BlockEntityViewDistanceEvent,
    BlockKey, BlockPart, ClientHost, Compound, Host, HostSide, Registrar, RegistrationContext, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, Ordering};

/// The key in the anchor's block entity data under which its structure is.
pub const KEY: &str = "Contraption";

const TURN: i32 = 0;
const GLIDE: i32 = 1;
const CLOCK: i32 = 2;
const ELEVATOR: i32 = 3;
/// A structure that is gone: clients merge the data they are sent into their copy, so a key
/// taken away would stay with them; this one says the anchor draws nothing.
const GONE: i32 = -1;

/// How far from the camera anchors draw their structures, in blocks: the legacy parts were
/// drawn at any distance.
const VIEW_DISTANCE: f64 = 256.0;

/// Each structure written gets a new number, so a client knows it changed.
static REVISION: AtomicI32 = AtomicI32::new(1);

/// The drawing of moving structures: its part on the kinetic block entity type, and the
/// client's elevators. After `Kinetics` in `SUBSYSTEMS`, which declares the type.
pub struct Drawing;

impl Subsystem for Drawing {
    fn synced_keys(&self) -> &'static [&'static str] {
        &[KEY]
    }

    fn animated(
        &self,
        _block: create_engine::Block,
        _pos: [i32; 3],
        entity: &mod_api::ModBlockEntity,
    ) -> bool {
        moving(&entity.nbt)
    }

    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        let Some(kind) = super::super::kinetics::kind() else {
            crate::create_log!(
                "[create] contraptions: the kinetic block entity type is not registered yet"
            );
            return;
        };
        if let Err(e) = r.block_entity_part(&kind, StructurePart) {
            crate::create_log!("[create] the contraption drawing part was refused: {e:?}");
        }
    }

    fn world_closed(&self) {
        LOADED.with_borrow_mut(Vec::clear);
    }

    /// Anchors that came back with a structure in their data that is not running (a piston or
    /// gantry is not saved moving): they draw nothing.
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        let dim = lv.dim;
        let loaded: Vec<[i32; 3]> = LOADED.with_borrow_mut(|l| {
            if l.is_empty() {
                return Vec::new();
            }
            let mine = l.iter().filter(|e| e.0 == dim).map(|e| e.1).collect();
            l.retain(|e| e.0 != dim);
            mine
        });
        for pos in loaded {
            if !anchors(dim, pos) {
                hide(lv, pos);
            }
        }
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        client_tick(host);
    }

    fn client_reset(&self) {
        client_reset();
    }
}

thread_local! {
    /// Anchors loaded with a structure in their data (dimension, position), for the level tick.
    static LOADED: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
}

/// Whether the block at `pos` anchors a running structure.
fn anchors(dim: usize, pos: [i32; 3]) -> bool {
    with_world(|w| {
        let d = &w.dims[dim];
        let l = &d.lifts;
        d.windmills.contains_key(&pos)
            || d.pistons.contains_key(&pos)
            || l.clocks.contains_key(&pos)
            || l.pulleys.contains_key(&pos)
            || l.elevators.contains_key(&pos)
            || d.gantries.iter().any(|g| {
                let o = g.carriage_facing.opposite().offset();
                [0, 1, 2].map(|i| g.start[i] + o[i]) == pos
            })
    })
    .unwrap_or(false)
}

/// Whether `data` holds a structure that moves along a line (drawn each frame, so its block
/// entity's geometry is not kept between frames).
pub fn moving(data: &Compound) -> bool {
    match data.get(&KEY.into()) {
        Some(Tag::Compound(c)) => matches!(int(c, "Kind"), Some(GLIDE | ELEVATOR)),
        _ => false,
    }
}

/// Whether `data` holds a structure (not none, nor one that is gone).
fn holds_structure(data: &Compound) -> bool {
    match data.get(&KEY.into()) {
        Some(Tag::Compound(c)) => int(c, "Kind") != Some(GONE),
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Server: writing a structure into its anchor

/// `blocks` (offsets from `place`, in whole blocks from the anchor) with their states, and the
/// light each is drawn with.
fn blocks_tags(
    c: &mut Compound,
    suffix: &str,
    lv: &ServerLevel<'_>,
    anchor: [i32; 3],
    place: [i32; 3],
    blocks: &[([i32; 3], u32)],
) {
    let host = &*lv.host;
    c.insert(
        format!("Lights{suffix}").as_str().into(),
        Tag::IntArray(
            blocks
                .iter()
                .map(|(o, _)| light_near(lv, [0, 1, 2].map(|i| anchor[i] + place[i] + o[i])) as i32)
                .collect(),
        ),
    );
    c.insert(
        format!("Blocks{suffix}").as_str().into(),
        Tag::IntArray(blocks.iter().flat_map(|(o, _)| *o).collect()),
    );
    c.insert(
        format!("States{suffix}").as_str().into(),
        Tag::List(
            blocks
                .iter()
                .map(|(_, s)| Tag::String(state_text(host, *s).unwrap_or_default().as_str().into()))
                .collect(),
        ),
    );
}

/// The light a moving block at `p` is drawn with (`ContraptionRenderer` lights each block with
/// the world's light where it is): the brightest of its cell and the six beside it, since the
/// cell itself may not have been lit again since its block left. The anchor's own light is its
/// block's, dark inside a solid bearing or piston. Packed as the draw context takes it: 16
/// times sky light in bits 0..8, 16 times block light in bits 8..16.
fn light_near(lv: &ServerLevel<'_>, p: [i32; 3]) -> u32 {
    use mod_api::LightLayer;
    let (mut sky, mut block) = (0, 0);
    for o in [
        [0, 0, 0],
        [1, 0, 0],
        [-1, 0, 0],
        [0, 1, 0],
        [0, -1, 0],
        [0, 0, 1],
        [0, 0, -1],
    ] {
        let q = [p[0] + o[0], p[1] + o[1], p[2] + o[2]];
        let at = |layer| lv.level.brightness(&*lv.host, layer, q).unwrap_or(0);
        sky = sky.max(at(LightLayer::Sky));
        block = block.max(at(LightLayer::Block));
    }
    let c = |v: i32| (v.clamp(0, 15) as u32) * 16;
    c(sky) | c(block) << 8
}

fn float(v: f32) -> Tag {
    Tag::Float(v.to_bits())
}

fn axis_index(a: Axis) -> i32 {
    match a {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    }
}

/// Puts `c` (none: takes it away) in the data of the block entity at `anchor`, which sends it
/// to the clients that see it.
fn write(lv: &mut ServerLevel<'_>, anchor: [i32; 3], c: Option<Compound>) {
    let Some(entity) = lv.host.block_entity(lv.level, anchor) else {
        return;
    };
    let Some(mut data) = entity.data(&*lv.host) else {
        return;
    };
    match c {
        Some(mut c) => {
            c.insert(
                "Rev".into(),
                Tag::Int(REVISION.fetch_add(1, Ordering::Relaxed)),
            );
            c.insert("Since".into(), Tag::Long(lv.game_time()));
            data.insert(KEY.into(), Tag::Compound(c));
        }
        None => {
            if !holds_structure(&data) {
                return;
            }
            let gone = Compound::from([
                ("Kind".into(), Tag::Int(GONE)),
                (
                    "Rev".into(),
                    Tag::Int(REVISION.fetch_add(1, Ordering::Relaxed)),
                ),
            ]);
            data.insert(KEY.into(), Tag::Compound(gone));
        }
    }
    let _ = entity.replace_data(&mut *lv.host, data);
}

/// The anchor at `anchor` draws nothing (its structure stopped or came apart).
pub(super) fn hide(lv: &mut ServerLevel<'_>, anchor: [i32; 3]) {
    write(lv, anchor, None);
}

/// A bearing's structure (offsets from the bearing) turning about its axis through the
/// bearing's middle: from `turn`'s angle now at its speed, or (a windmill with no actors) from 0
/// at the bearing's own speed.
pub(super) fn show_turn(
    lv: &mut ServerLevel<'_>,
    anchor: [i32; 3],
    blocks: &[([i32; 3], u32)],
    axis: Axis,
    turn: Option<Turn>,
) {
    let mut c = Compound::new();
    c.insert("Kind".into(), Tag::Int(TURN));
    blocks_tags(&mut c, "", lv, anchor, [0; 3], blocks);
    c.insert("Axis".into(), Tag::Int(axis_index(axis)));
    let now = lv.game_time();
    c.insert("Angle".into(), float(turn.map_or(0.0, |t| t.angle_at(now))));
    if let Some(t) = turn {
        c.insert("Rpm".into(), float(t.rpm));
    }
    write(lv, anchor, Some(c));
}

/// A structure moving along a line: its blocks (offsets from `origin`, itself from the anchor)
/// at `origin + dir × offset` now, moving `per_tick` blocks a tick, within `clamp`; `rope`: a
/// rope pulley's, with rope down to its magnet.
pub(super) struct Glide<'a> {
    pub origin: [i32; 3],
    pub blocks: &'a [([i32; 3], u32)],
    pub dir: [i32; 3],
    pub offset: f32,
    pub per_tick: f32,
    pub clamp: Option<(f32, f32)>,
    pub rope: bool,
}

pub(super) fn show_glide(lv: &mut ServerLevel<'_>, anchor: [i32; 3], g: Glide<'_>) {
    let mut c = Compound::new();
    c.insert("Kind".into(), Tag::Int(GLIDE));
    let place = [0, 1, 2].map(|i| g.origin[i] + (g.dir[i] as f32 * g.offset).round() as i32);
    blocks_tags(&mut c, "", lv, anchor, place, g.blocks);
    c.insert("Origin".into(), Tag::IntArray(g.origin.to_vec()));
    c.insert("Dir".into(), Tag::IntArray(g.dir.to_vec()));
    c.insert("Offset".into(), float(g.offset));
    c.insert("PerTick".into(), float(g.per_tick));
    if let Some((lo, hi)) = g.clamp {
        c.insert("Min".into(), float(lo));
        c.insert("Max".into(), float(hi));
    }
    if g.rope {
        c.insert("Rope".into(), Tag::Byte(1));
    }
    write(lv, anchor, Some(c));
}

/// A clockwork bearing's hands (offsets from the bearing), each turning about its axis from its
/// angle now at its degrees a tick.
pub(super) struct Hands<'a> {
    pub hour: &'a [([i32; 3], u32)],
    pub minute: &'a [([i32; 3], u32)],
    pub axis: Axis,
    pub angles: (f32, f32),
    pub speeds: (f32, f32),
}

pub(super) fn show_clock(lv: &mut ServerLevel<'_>, anchor: [i32; 3], h: Hands<'_>) {
    let mut c = Compound::new();
    c.insert("Kind".into(), Tag::Int(CLOCK));
    blocks_tags(&mut c, "", lv, anchor, [0; 3], h.hour);
    blocks_tags(&mut c, "2", lv, anchor, [0; 3], h.minute);
    c.insert("Axis".into(), Tag::Int(axis_index(h.axis)));
    c.insert("Angle".into(), float(h.angles.0));
    c.insert("Angle2".into(), float(h.angles.1));
    c.insert("Speed".into(), float(h.speeds.0));
    c.insert("Speed2".into(), float(h.speeds.1));
    write(lv, anchor, Some(c));
}

/// An elevator: where it is, its speed, the floor it is called to, its anchor, column and
/// blocks. The client runs its ticks on from there.
pub(super) fn show_elevator(
    lv: &mut ServerLevel<'_>,
    anchor: [i32; 3],
    e: &Elevator,
    rpm: f32,
    target: Option<i32>,
) {
    let mut c = Compound::new();
    c.insert("Kind".into(), Tag::Int(ELEVATOR));
    let place = [
        e.anchor[0] - anchor[0],
        e.y.round() as i32 - anchor[1],
        e.anchor[2] - anchor[2],
    ];
    blocks_tags(&mut c, "", lv, anchor, place, &e.blocks);
    let y = e.y.to_bits();
    let nums = vec![
        e.anchor[0],
        e.anchor[1],
        e.anchor[2],
        e.initial,
        e.offset.to_bits() as i32,
        (y >> 32) as i32,
        y as i32,
        e.prev.to_bits() as i32,
        i32::from(e.arrived),
        e.range,
        e.column.x,
        e.column.z,
        e.column.side as i32,
        e.contact_y,
        e.min_contact,
        e.max_contact,
        i32::from(target.is_some()),
        target.unwrap_or(0),
        rpm.to_bits() as i32,
    ];
    c.insert("Nums".into(), Tag::IntArray(nums));
    write(lv, anchor, Some(c));
}

// ---------------------------------------------------------------------------------------------
// Reading it back

fn int(c: &Compound, key: &str) -> Option<i32> {
    match c.get(&key.into()) {
        Some(Tag::Int(v)) => Some(*v),
        _ => None,
    }
}

fn get_float(c: &Compound, key: &str) -> Option<f32> {
    match c.get(&key.into()) {
        Some(Tag::Float(v)) => Some(f32::from_bits(*v)),
        _ => None,
    }
}

fn ints<'a>(c: &'a Compound, key: &str) -> &'a [i32] {
    match c.get(&key.into()) {
        Some(Tag::IntArray(v)) => v,
        _ => &[],
    }
}

fn vec3(v: &[i32]) -> [i32; 3] {
    [0, 1, 2].map(|i| v.get(i).copied().unwrap_or(0))
}

/// A block of a structure as the client draws it: its offset, block and properties.
struct Drawn {
    at: [f32; 3],
    /// The light it is drawn with (none from an older save: the anchor's).
    light: Option<u32>,
    block: BlockKey,
    props: Vec<(String, String)>,
    /// The Create block and facts of its state.
    create: Option<(create_engine::Block, create_engine::parts::StateInfo)>,
}

fn drawn_blocks(host: &dyn Host, c: &Compound, suffix: &str) -> Vec<Drawn> {
    let offsets = ints(c, &format!("Blocks{suffix}"));
    let lights = ints(c, &format!("Lights{suffix}"));
    let Some(Tag::List(texts)) = c.get(&format!("States{suffix}").as_str().into()) else {
        return Vec::new();
    };
    offsets
        .as_chunks::<3>()
        .0
        .iter()
        .zip(texts)
        .enumerate()
        .filter_map(|(i, (o, t))| {
            let Tag::String(t) = t else { return None };
            let text = t.to_string_lossy();
            let state = host.block_state_id(&text)?;
            let (_, props) = states::split(&text);
            let create = states::id(state).and_then(|id| {
                super::super::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
            });
            Some(Drawn {
                at: o.map(|v| v as f32),
                light: lights.get(i).map(|l| *l as u32),
                block: state.block(),
                create,
                props: props
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect(),
            })
        })
        .collect()
}

enum Shape {
    Turn {
        blocks: Vec<Drawn>,
        axis: [f32; 3],
        /// The angle at `since`, and the speed from then (none: the block's own speed).
        angle: f32,
        since: f64,
        rpm: Option<f32>,
    },
    Glide {
        blocks: Vec<Drawn>,
        origin: [f32; 3],
        dir: [f32; 3],
        offset: f32,
        per_tick: f32,
        since: f64,
        clamp: Option<(f32, f32)>,
        rope: bool,
    },
    Clock {
        hour: Vec<Drawn>,
        minute: Vec<Drawn>,
        axis: [f32; 3],
        angles: (f32, f32),
        speeds: (f32, f32),
        since: f64,
    },
    Elevator {
        blocks: Vec<Drawn>,
        e: Elevator,
        rpm: f32,
        target: Option<i32>,
        /// Its height a tick before, to draw between ticks.
        prev_y: f64,
    },
}

fn axis_vector(i: i32) -> [f32; 3] {
    match i {
        0 => [1.0, 0.0, 0.0],
        2 => [0.0, 0.0, 1.0],
        _ => [0.0, 1.0, 0.0],
    }
}

fn parse(host: &dyn Host, c: &Compound) -> Option<Shape> {
    let since = match c.get(&"Since".into()) {
        Some(Tag::Long(t)) => *t as f64,
        _ => 0.0,
    };
    Some(match int(c, "Kind")? {
        TURN => Shape::Turn {
            blocks: drawn_blocks(host, c, ""),
            axis: axis_vector(int(c, "Axis")?),
            angle: get_float(c, "Angle").unwrap_or(0.0),
            since,
            rpm: get_float(c, "Rpm"),
        },
        GLIDE => Shape::Glide {
            blocks: drawn_blocks(host, c, ""),
            origin: vec3(ints(c, "Origin")).map(|v| v as f32),
            dir: vec3(ints(c, "Dir")).map(|v| v as f32),
            offset: get_float(c, "Offset").unwrap_or(0.0),
            per_tick: get_float(c, "PerTick").unwrap_or(0.0),
            since,
            clamp: get_float(c, "Min").zip(get_float(c, "Max")),
            rope: c.get(&"Rope".into()).is_some(),
        },
        CLOCK => Shape::Clock {
            hour: drawn_blocks(host, c, ""),
            minute: drawn_blocks(host, c, "2"),
            axis: axis_vector(int(c, "Axis")?),
            angles: (get_float(c, "Angle")?, get_float(c, "Angle2")?),
            speeds: (get_float(c, "Speed")?, get_float(c, "Speed2")?),
            since,
        },
        ELEVATOR => {
            let n = ints(c, "Nums");
            let at = |i: usize| n.get(i).copied();
            let f = |i: usize| at(i).map(|v| f32::from_bits(v as u32));
            let y = f64::from_bits((at(5)? as u32 as u64) << 32 | at(6)? as u32 as u64);
            let e = Elevator {
                anchor: [at(0)?, at(1)?, at(2)?],
                initial: at(3)?,
                offset: f(4)?,
                y,
                prev: f(7)?,
                arrived: at(8)? != 0,
                range: at(9)?,
                column: ColumnKey {
                    x: at(10)?,
                    z: at(11)?,
                    side: Dir::ALL[at(12)?.clamp(0, 5) as usize],
                },
                contact_y: at(13)?,
                min_contact: at(14)?,
                max_contact: at(15)?,
                blocks: Vec::new(),
            };
            Shape::Elevator {
                blocks: drawn_blocks(host, c, ""),
                rpm: f(18)?,
                target: (at(16)? != 0).then_some(at(17)?),
                prev_y: y,
                e,
            }
        }
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------
// Client

struct Shown {
    rev: i32,
    shape: Shape,
    /// Where offset 0 of a gliding structure was last drawn, in the world (for clicks).
    drawn_at: Option<[f64; 3]>,
    /// A windmill's turn as the client runs it: its angle at a time and the speed since, so a
    /// speed change turns on from where it is (`turn_contraption`).
    base: Option<(f32, f64, f32)>,
}

thread_local! {
    /// The structures this client draws, by their anchor.
    static SHOWN: RefCell<HashMap<[i32; 3], Shown>> = RefCell::new(HashMap::new());
}

fn client_reset() {
    SHOWN.with_borrow_mut(HashMap::clear);
}

/// Each client tick: elevators run on as the server runs them.
fn client_tick(_host: &mut dyn ClientHost) {
    SHOWN.with_borrow_mut(|s| {
        for (pos, shown) in s.iter_mut() {
            if let Shape::Elevator {
                e,
                rpm,
                target,
                prev_y,
                ..
            } = &mut shown.shape
            {
                *prev_y = e.y;
                if !e.settled(*target) {
                    e.tick(pos[1], *rpm, *target);
                }
            }
        }
    });
}

fn draw_blocks(draw: &mut dyn BlockEntityDrawContext, blocks: &[Drawn]) {
    let own = draw.light();
    for b in blocks {
        let props: Vec<(&str, &str)> = b
            .props
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        draw.set_light(b.light.unwrap_or(own));
        draw.push();
        draw.translate(b.at);
        draw.block(b.block, &props);
        if let Some((block, info)) = b.create {
            draw_parts(draw, block, &info, &props);
        }
        // `BlazeBurnerMovementBehaviour.renderInContraption`: a carried burner's blaze.
        if b.block == states::key(create_engine::Block::BlazeBurner) {
            let prop = |k: &str| props.iter().find(|(n, _)| *n == k).map(|(_, v)| *v);
            let heat = match prop("blaze") {
                Some("smouldering") => create_engine::fan::Heat::Smouldering,
                Some("fading") => create_engine::fan::Heat::Fading,
                Some("kindled") => create_engine::fan::Heat::Kindled,
                Some("seething") => create_engine::fan::Heat::Seething,
                _ => create_engine::fan::Heat::None,
            };
            let facing = prop("facing")
                .and_then(|f| {
                    create_engine::kinetics::Dir::ALL
                        .into_iter()
                        .find(|d| crate::glue::kinetics::dir_name(*d) == f)
                })
                .unwrap_or(create_engine::kinetics::Dir::South);
            let hash = (b.at[0] as i32 * 31 + b.at[1] as i32) * 31 + b.at[2] as i32;
            crate::glue::processing::burner_draw::draw_moving(draw, heat, facing, hash);
        }
        draw.pop();
    }
    draw.set_light(own);
}

/// Blocks whose model is the whole block, drawn by its block entity in the world
/// (`RenderShape.ENTITYBLOCK_ANIMATED`): shafts, cogs, cranks and valve handles, the gantry
/// shaft, crushing wheels, flywheels, large water wheels, turntables, belts and copycats.
fn whole_model(block: create_engine::Block) -> bool {
    use create_engine::Block as B;
    create_engine::dye::is_valve_handle(block)
        || matches!(
            block,
            B::Shaft
                | B::PoweredShaft
                | B::Cogwheel
                | B::LargeCogwheel
                | B::GantryShaft
                | B::CrushingWheel
                | B::Flywheel
                | B::LargeWaterWheel
                | B::HandCrank
                | B::Turntable
                | B::Belt
                | B::CopycatPanel
                | B::CopycatStep
        )
}

/// A moving block's kinetic parts at rest (`ContraptionKineticRenderer`: what its block
/// entity's visual draws in the world, its shafts, cogs, drill head or fan blades), over its
/// block model; none for a block whose model is already all of it.
fn draw_parts(
    draw: &mut dyn BlockEntityDrawContext,
    block: create_engine::Block,
    info: &create_engine::parts::StateInfo,
    props: &[(&str, &str)],
) {
    use create_engine::client::{Model, draws};
    if whole_model(block) {
        return;
    }
    for d in draws(block, info, [0; 3], [0.0; 2]) {
        if d.model == Model::Own {
            continue;
        }
        let Some((model, prop)) = super::super::kinetics::part_model(block, d.model) else {
            continue;
        };
        draw.push();
        let axis = match d.axis {
            Axis::X => [1.0, 0.0, 0.0],
            Axis::Y => [0.0, 1.0, 0.0],
            Axis::Z => [0.0, 0.0, 1.0],
        };
        if d.phase != 0.0 {
            draw.rotate_about(axis, d.phase, [0.5; 3]);
        }
        match prop {
            Some(p) => draw.block(model, &[p]),
            None => draw.block(model, props),
        };
        draw.pop();
    }
}

/// The speed the kinetic data gives the anchor's own part (slot 0).
fn own_speed(data: &Compound) -> f32 {
    match data.get(&"Speeds".into()) {
        Some(Tag::List(l)) => match l.first() {
            Some(Tag::Float(bits)) => f32::from_bits(*bits),
            _ => 0.0,
        },
        _ => 0.0,
    }
}

/// Draws the structure of the anchor at `pos` whose data is `data`.
fn submit(
    host: &dyn ClientHost,
    pos: [i32; 3],
    data: &Compound,
    draw: &mut dyn BlockEntityDrawContext,
) {
    let Some(Tag::Compound(c)) = data.get(&KEY.into()).filter(|_| holds_structure(data)) else {
        SHOWN.with_borrow_mut(|s| s.remove(&pos));
        return;
    };
    let rev = int(c, "Rev").unwrap_or(0);
    let fresh = SHOWN.with_borrow(|s| s.get(&pos).is_none_or(|x| x.rev != rev));
    if fresh {
        let Some(shape) = parse(host, c) else {
            return;
        };
        SHOWN.with_borrow_mut(|s| {
            s.insert(
                pos,
                Shown {
                    rev,
                    shape,
                    drawn_at: None,
                    base: None,
                },
            )
        });
    }
    let now = draw.game_time() as f64 + f64::from(draw.partial());
    let anim = draw.animation_time();
    let speed = own_speed(data);
    SHOWN.with_borrow_mut(|s| {
        let Some(shown) = s.get_mut(&pos) else {
            return;
        };
        let shown = &mut *shown;
        match &shown.shape {
            Shape::Turn {
                blocks,
                axis,
                angle,
                since,
                rpm,
            } => {
                let (angle, since, rpm) = match rpm {
                    Some(r) => (*angle, *since, *r),
                    None => {
                        let base = shown.base.get_or_insert((*angle, *since, speed));
                        if base.2 != speed {
                            let here = (base.0 + base.2 * 0.3 * (now - base.1) as f32) % 360.0;
                            *base = (here, now, speed);
                        }
                        *base
                    }
                };
                let rate = rpm * 0.3;
                let here = angle + rate * (now - since) as f32;
                let phase = (f64::from(here) - anim * f64::from(rate)) % 360.0;
                draw.spin(*axis, [0.5; 3], rate, phase as f32);
                draw_blocks(draw, blocks);
                draw.still();
                draw.keep();
            }
            Shape::Clock {
                hour,
                minute,
                axis,
                angles,
                speeds,
                since,
            } => {
                for (blocks, angle, rate) in
                    [(hour, angles.0, speeds.0), (minute, angles.1, speeds.1)]
                {
                    let here = angle + rate * (now - since) as f32;
                    let phase = (f64::from(here) - anim * f64::from(rate)) % 360.0;
                    draw.spin(*axis, [0.5; 3], rate, phase as f32);
                    draw_blocks(draw, blocks);
                    draw.still();
                }
                draw.keep();
            }
            Shape::Glide {
                blocks,
                origin,
                dir,
                offset,
                per_tick,
                since,
                clamp,
                rope,
            } => {
                let mut at = offset + per_tick * (now - since) as f32;
                if let Some((lo, hi)) = clamp {
                    at = at.clamp(*lo, *hi);
                }
                let shift = [0, 1, 2].map(|i| origin[i] + dir[i] * at);
                shown.drawn_at = Some([0, 1, 2].map(|i| f64::from(pos[i]) + f64::from(shift[i])));
                draw.push();
                draw.translate(shift);
                draw_blocks(draw, blocks);
                draw.pop();
                // Rope: whole blocks between the pulley and its magnet.
                if *rope {
                    draw_rope(host, draw, at);
                }
            }
            Shape::Elevator {
                blocks, e, prev_y, ..
            } => {
                let y = prev_y + (e.y - prev_y) * f64::from(draw.partial());
                let origin = [
                    (e.anchor[0] - pos[0]) as f32,
                    (y - f64::from(pos[1])) as f32,
                    (e.anchor[2] - pos[2]) as f32,
                ];
                draw.push();
                draw.translate(origin);
                draw_blocks(draw, blocks);
                draw.pop();
            }
        }
    });
}

/// The channel a client's click on contraption controls riding a gliding structure goes to
/// the server on: the structure's anchor and the block's offset in it, six ints.
pub const CONTROLS_CHANNEL: &str = "create:contraption_controls";

/// Reads a click sent on [`CONTROLS_CHANNEL`].
pub fn decode_click(data: &[u8]) -> Option<([i32; 3], [i32; 3])> {
    let v: Vec<i32> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| i32::from_le_bytes(*b))
        .collect();
    let [a, b, c, x, y, z] = v[..] else {
        return None;
    };
    Some(([a, b, c], [x, y, z]))
}

/// Where a ray from `eye` to `target` enters the unit box at `min`, as a fraction of the ray.
fn ray_box(eye: [f64; 3], target: [f64; 3], min: [f64; 3]) -> Option<f64> {
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for i in 0..3 {
        let d = target[i] - eye[i];
        let (a, b) = (min[i], min[i] + 1.0);
        if d.abs() < 1e-9 {
            if eye[i] < a || eye[i] > b {
                return None;
            }
            continue;
        }
        let (t0, t1) = ((a - eye[i]) / d, (b - eye[i]) / d);
        lo = lo.max(t0.min(t1));
        hi = hi.min(t0.max(t1));
        if lo > hi {
            return None;
        }
    }
    Some(lo)
}

/// `ContraptionHandlerClient.rightClickingOnContraptionsGetsHandledLocally` for contraption
/// controls: a use click whose ray (as far as the player reaches, or the block the crosshair is
/// on) meets a contraption controls block of a gliding structure this client draws goes to the
/// server as a click on it, and nothing else happens. Turning structures carry no controls.
pub fn use_key(
    host: &mut dyn ClientHost,
    event: &mut mod_api::InteractionKeyMappingTriggeredEvent,
) -> mod_api::Flow {
    use mod_api::{CrosshairTarget, Flow, Hand, InteractionKey};
    if event.key != InteractionKey::Use
        || event.hand != Hand::Main
        || event.cancelled
        || SHOWN.with_borrow(HashMap::is_empty)
    {
        return Flow::Continue;
    }
    let (Some(player), Some(camera)) = (host.local_player(), host.camera()) else {
        return Flow::Continue;
    };
    if player.is_spectator(&*host) == Some(true) {
        return Flow::Continue;
    }
    let Some(controls) = host
        .block_state_id("create:contraption_controls")
        .map(|s| s.block())
    else {
        return Flow::Continue;
    };
    let eye = camera.position;
    let (yaw, pitch) = (camera.yaw.to_radians(), camera.pitch.to_radians());
    let look = [
        f64::from(-yaw.sin() * pitch.cos()),
        f64::from(-pitch.sin()),
        f64::from(yaw.cos() * pitch.cos()),
    ];
    let mut reach: f64 = if player.is_creative(&*host) == Some(true) {
        5.0
    } else {
        4.5
    };
    match host.crosshair_target() {
        Some(CrosshairTarget::Block { hit, .. }) => {
            let d: f64 = (0..3).map(|i| (hit.point[i] - eye[i]).powi(2)).sum();
            reach = d.sqrt().min(reach);
        }
        Some(CrosshairTarget::Entity { .. }) => return Flow::Continue,
        _ => {}
    }
    let target: [f64; 3] = std::array::from_fn(|i| eye[i] + look[i] * reach);
    let hit = SHOWN.with_borrow(|s| {
        let mut best: Option<(f64, [i32; 3], [i32; 3])> = None;
        for (anchor, shown) in s {
            let (Shape::Glide { blocks, .. }, Some(at)) = (&shown.shape, shown.drawn_at) else {
                continue;
            };
            for b in blocks.iter().filter(|b| b.block == controls) {
                let min = [0, 1, 2].map(|i| at[i] + f64::from(b.at[i]));
                if let Some(t) = ray_box(eye, target, min)
                    && best.is_none_or(|x| t < x.0)
                {
                    best = Some((t, *anchor, b.at.map(|v| v.round() as i32)));
                }
            }
        }
        best
    });
    let Some((_, anchor, local)) = hit else {
        return Flow::Continue;
    };
    let data: Vec<u8> = anchor
        .iter()
        .chain(&local)
        .flat_map(|v| v.to_le_bytes())
        .collect();
    super::super::net::send_to_server(host, CONTROLS_CHANNEL, &data);
    event.cancelled = true;
    event.swing_hand = true;
    Flow::Cancel
}

thread_local! {
    static ROPE: RefCell<Option<Option<BlockKey>>> = const { RefCell::new(None) };
}

/// Rope from under the pulley down to its magnet, `offset` blocks down (`ceil - 1` whole
/// blocks).
fn draw_rope(host: &dyn ClientHost, draw: &mut dyn BlockEntityDrawContext, offset: f32) {
    let Some(rope) = ROPE.with_borrow_mut(|r| {
        *r.get_or_insert_with(|| host.block_state_id("create:rope").map(|s| s.block()))
    }) else {
        return;
    };
    let n = (offset.ceil() as i32 - 1).max(0);
    for i in 1..=n {
        draw.push();
        draw.translate([0.0, -(i as f32), 0.0]);
        draw.block(rope, &[]);
        draw.pop();
    }
}

/// The part every Create block entity carries that draws a structure its block anchors.
struct StructurePart;

#[mod_api::horizon_part]
impl BlockPart for StructurePart {}

#[mod_api::horizon_part]
impl BlockEntityPart for StructurePart {
    /// An anchor came back with a structure in its data: the level tick looks whether it runs.
    fn block_entity_on_load(&self, host: HostSide, event: &mut BlockEntityOnLoadEvent) {
        let HostSide::Server(host) = host else {
            return;
        };
        let entity = event.entity;
        let Some(data) = entity.data(&*host) else {
            return;
        };
        if !holds_structure(&data) {
            return;
        }
        let (Some(level), Some(pos)) = (entity.level(&*host), entity.position(&*host)) else {
            return;
        };
        let Some(dim) = level.dimension().id(&*host).and_then(|d| level_index(&d)) else {
            return;
        };
        LOADED.with_borrow_mut(|l| l.push((dim, pos)));
    }

    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        if !event.entity.nbt.contains_key(&KEY.into())
            && SHOWN.with_borrow(|s| !s.contains_key(&event.pos))
        {
            return;
        }
        submit(host, event.pos, &event.entity.nbt, event.draw);
    }

    /// A structure is drawn wherever it moves to: never culled.
    fn block_entity_render_bounding_box(
        &self,
        _host: &dyn ClientHost,
        event: &mut BlockEntityRenderBoundingBoxEvent<'_>,
    ) {
        if event.entity.nbt.contains_key(&KEY.into()) {
            *event.bounds = [
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
                f32::INFINITY,
                f32::INFINITY,
                f32::INFINITY,
            ];
        }
    }

    fn block_entity_view_distance(
        &self,
        _host: &dyn ClientHost,
        event: &mut BlockEntityViewDistanceEvent<'_>,
    ) {
        if matches!(
            event.block,
            "create:mechanical_bearing"
                | "create:windmill_bearing"
                | "create:clockwork_bearing"
                | "create:mechanical_piston"
                | "create:sticky_mechanical_piston"
                | "create:rope_pulley"
                | "create:elevator_pulley"
                | "create:gantry_shaft"
        ) {
            event.distance = event.distance.max(VIEW_DISTANCE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turning_structure_is_kept_and_a_sliding_one_is_not() {
        let with = |kind: i32| {
            let mut c = Compound::new();
            c.insert("Kind".into(), Tag::Int(kind));
            Compound::from([(KEY.into(), Tag::Compound(c))])
        };
        assert!(!moving(&with(TURN)));
        assert!(!moving(&with(CLOCK)));
        assert!(moving(&with(GLIDE)));
        assert!(moving(&with(ELEVATOR)));
        assert!(!moving(&Compound::new()));
    }
}
