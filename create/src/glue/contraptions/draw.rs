//! Moving structures on clients. A structure travels in its anchor's block entity data
//! (`Contraption`), which the game sends to every client that sees the anchor (when its chunk
//! is sent and whenever it changes), and the anchor's block entity draws it: each block's model
//! (`BlockEntityDrawContext::block`) under a pose turning about the bearing's axis (`spin`, kept
//! between frames like the kinetic parts) or moved along its line each frame from the offset
//! and speed it was sent with. Elevators run their own ticks on the client, as Create's do.

use super::super::server::ServerLevel;
use super::super::states;
use super::super::subsystem::Subsystem;
use super::super::world::{level_index, with_world};
use super::actor_draw::{self, Frame, Motion};
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
/// A rope or elevator pulley's offset at rest, in its block entity's data: a rope pulley's
/// rope length, an elevator's -1 once it came apart (`ElevatorPulleyBlockEntity.disassemble`);
/// none for one never run (0).
pub const PULLEY_REST: &str = "PulleyRest";

const TURN: i32 = 0;
const GLIDE: i32 = 1;
const CLOCK: i32 = 2;
const ELEVATOR: i32 = 3;

/// How far from the camera anchors draw their structures, in blocks.
const VIEW_DISTANCE: f64 = 256.0;

/// Each structure written gets a new number, so a client knows it changed.
static REVISION: AtomicI32 = AtomicI32::new(1);

/// The drawing of moving structures: its part on the kinetic block entity type, and the
/// client's elevators. After `Kinetics` in `SUBSYSTEMS`, which declares the type.
pub struct Drawing;

impl Subsystem for Drawing {
    fn synced_keys(&self) -> &'static [&'static str] {
        &[
            KEY,
            PULLEY_REST,
            super::LAST_EXCEPTION,
            actor_draw::ACTORS,
            actor_draw::PSI,
            actor_draw::CONTROLS_OFF,
            super::lifts::FLOORS,
        ]
    }

    fn animated(
        &self,
        block: create_engine::Block,
        _pos: [i32; 3],
        entity: &mod_api::ModBlockEntity,
    ) -> bool {
        moving(&entity.nbt)
            || (holds_structure(&entity.nbt) && actor_draw::view(&entity.nbt).is_some())
            || actor_draw::animated(block, entity)
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
        if let Err(e) = r.block_entity_part(&kind, actor_draw::PlacedActors) {
            crate::create_log!("[create] the actors' drawing part was refused: {e:?}");
        }
    }

    fn world_closed(&self) {
        LOADED.with_borrow_mut(Vec::clear);
        actor_draw::forget();
    }

    /// Anchors that came back with a structure in their data that is not running (one that
    /// stopped after its column was last written): they draw nothing.
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

    fn models_baked(&self, host: &mut dyn ClientHost) {
        super::vanilla_draw::models_baked(host);
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

/// Whether `data` holds a structure (a key taken away reaches clients as absent:
/// `kinetics::synced`).
fn holds_structure(data: &Compound) -> bool {
    matches!(
        super::super::kinetics::synced(data, KEY),
        Some(Tag::Compound(_))
    )
}

// ---------------------------------------------------------------------------------------------
// Server: writing a structure into its anchor

/// `blocks` (offsets from `place`, in whole blocks from the anchor) with their states, the
/// light each is drawn with, and what the block entity of each shows (`Contraption.updateTags`:
/// what contraption `key` carries for it, by the same offset).
fn blocks_tags(
    c: &mut Compound,
    suffix: &str,
    lv: &ServerLevel<'_>,
    (anchor, key): ([i32; 3], [i32; 3]),
    place: [i32; 3],
    blocks: &[([i32; 3], u32)],
) {
    let host = &*lv.host;
    let mut entities = super::actors::shown_entities(lv, key);
    if !entities.is_empty() {
        c.insert(
            format!("Entities{suffix}").as_str().into(),
            Tag::List(
                blocks
                    .iter()
                    .map(|(o, _)| Tag::Compound(entities.remove(o).unwrap_or_default()))
                    .collect(),
            ),
        );
    }
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
            if data.remove(&KEY.into()).is_none() {
                return;
            }
        }
    }
    let _ = entity.replace_data(&mut *lv.host, data);
}

/// The pulley at `pos` came to rest `offset` blocks out (see `PULLEY_REST`).
pub(super) fn set_pulley_rest(lv: &mut ServerLevel<'_>, pos: [i32; 3], offset: i32) {
    if let Some(entity) = lv.host.block_entity(lv.level, pos) {
        let data = Compound::from([(PULLEY_REST.into(), Tag::Int(offset))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
}

/// The anchor at `anchor` draws nothing (its structure stopped or came apart).
pub(super) fn hide(lv: &mut ServerLevel<'_>, anchor: [i32; 3]) {
    write(lv, anchor, None);
    actor_draw::hide_actors(lv, anchor);
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
    blocks_tags(&mut c, "", lv, (anchor, anchor), [0; 3], blocks);
    c.insert("Axis".into(), Tag::Int(axis_index(axis)));
    let now = lv.game_time();
    c.insert("Angle".into(), float(turn.map_or(0.0, |t| t.angle_at(now))));
    if let Some(t) = turn {
        c.insert("Rpm".into(), float(t.rpm));
        // What is left of a sequenced turn from now.
        if let Some(l) = t.limit {
            let left = (l - (t.angle_at(now) - t.angle).abs()).max(0.0);
            c.insert("Limit".into(), float(left));
        }
    }
    write(lv, anchor, Some(c));
}

/// A structure moving along a line: its blocks (offsets from `origin`, itself from the anchor)
/// at `origin + dir × offset` now, moving `per_tick` blocks a tick, within `clamp`; `rope`: a
/// rope pulley's, with rope down to its magnet.
pub(super) struct Glide<'a> {
    /// The contraption whose carried block entities it shows (`actors`' key).
    pub key: [i32; 3],
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
    blocks_tags(&mut c, "", lv, (anchor, g.key), place, g.blocks);
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
    blocks_tags(&mut c, "", lv, (anchor, anchor), [0; 3], h.hour);
    blocks_tags(&mut c, "2", lv, (anchor, anchor), [0; 3], h.minute);
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
    blocks_tags(&mut c, "", lv, (anchor, anchor), place, &e.blocks);
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
    state: mod_api::BlockState,
    /// What its block entity showed when the structure took it (`Contraption.updateTags`).
    entity: Option<mod_api::ModBlockEntity>,
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
    let entities = match c.get(&format!("Entities{suffix}").as_str().into()) {
        Some(Tag::List(l)) => l.as_slice(),
        _ => &[],
    };
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
            let entity = match entities.get(i) {
                Some(Tag::Compound(e)) if !e.is_empty() => {
                    let mut entity = mod_api::ModBlockEntity::default();
                    entity.nbt = e.clone();
                    Some(entity)
                }
                _ => None,
            };
            Some(Drawn {
                at: o.map(|v| v as f32),
                state,
                entity,
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
        /// The degrees a sequenced turn may still go from `since`.
        limit: Option<f32>,
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

/// The axis `v` (a unit axis vector) lies along.
fn axis_enum(v: [f32; 3]) -> Axis {
    if v[0] != 0.0 {
        Axis::X
    } else if v[2] != 0.0 {
        Axis::Z
    } else {
        Axis::Y
    }
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
            limit: get_float(c, "Limit"),
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
    /// The axis and angle a turning structure with actors or controls was last drawn at (for
    /// clicks).
    turned: Option<(Axis, f32)>,
    /// An elevator's column's floors (`namesList`), and the call and floors its controls were
    /// last set to show (`syncControlDisplays`).
    floors: Vec<create_engine::elevator::Floor>,
    synced: Option<(i32, Vec<create_engine::elevator::Floor>)>,
}

thread_local! {
    /// The structures this client draws, by their anchor.
    static SHOWN: RefCell<HashMap<[i32; 3], Shown>> = RefCell::new(HashMap::new());
}

fn client_reset() {
    SHOWN.with_borrow_mut(HashMap::clear);
    actor_draw::client_reset();
}

/// Each client tick: elevators run on as the server runs them, and the contraption controls on
/// moving structures run their animations (`ContraptionControlsMovement.tick`): on an elevator
/// the floor they show (`tickFloorSelection`, set to the floor called when the floors or the
/// call change, `syncControlDisplays`) and the indicator toward the floor called.
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
            let controls: Vec<[i32; 3]> = shape_blocks(&shown.shape)
                .iter()
                .filter(|b| is_controls(b))
                .map(|b| b.at.map(|v| v.round() as i32))
                .collect();
            if controls.is_empty() {
                continue;
            }
            let Shape::Elevator { e, target, .. } = &shown.shape else {
                for local in controls {
                    actor_draw::with_controls(*pos, local, |a| a.tick());
                }
                continue;
            };
            let current = (e.y + f64::from(e.contact_y)).round() as i32;
            let client_target = target.unwrap_or(current);
            let sync = (client_target, shown.floors.clone());
            let resync = shown.synced.as_ref() != Some(&sync);
            let index = create_engine::elevator::floor_index(&shown.floors, client_target);
            let view = actor_draw::ElevatorView {
                floors: &shown.floors,
                target: client_target,
                min_contact: e.min_contact,
                max_contact: e.max_contact,
            };
            for local in controls {
                actor_draw::with_controls(*pos, local, |a| {
                    if resync && let Some(i) = index {
                        a.index = i;
                    }
                    view.select(&mut a.index);
                    a.tick_elevator(client_target, current);
                });
            }
            if resync {
                shown.synced = Some(sync);
            }
        }
    });
}

/// The blocks a structure draws (a clock's hour hand's and minute hand's).
fn shape_blocks(shape: &Shape) -> Vec<&Drawn> {
    match shape {
        Shape::Turn { blocks, .. }
        | Shape::Glide { blocks, .. }
        | Shape::Elevator { blocks, .. } => blocks.iter().collect(),
        Shape::Clock { hour, minute, .. } => hour.iter().chain(minute).collect(),
    }
}

fn is_controls(b: &Drawn) -> bool {
    matches!(
        b.create,
        Some((create_engine::Block::ContraptionControls, _))
    )
}

/// Draws a structure's blocks; `actors`: its actors' moving parts this frame (none: at rest).
fn draw_blocks(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    anchor: [i32; 3],
    blocks: &[Drawn],
    actors: Option<&Frame<'_>>,
) {
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
        // What its block entity holds first: a translucent model (a nixie tube's glass) then
        // blends over it rather than hiding it.
        if let (Some((block, _)), Some(entity)) = (b.create, &b.entity) {
            draw_carried(host, draw, anchor, b, block, entity);
        }
        draw.block(b.block, &props);
        if let Some((block, info)) = b.create {
            let actor = actors.filter(|_| actor_draw::draws(block));
            draw_parts(draw, block, &info, &props, actor.is_some());
            match actor {
                Some(frame) => {
                    let local = b.at.map(|v| v.round() as i32);
                    actor_draw::draw_moving(draw, frame, block, &info, local);
                }
                // No word of its actors yet (or a clock's hands): at rest.
                None if actor_draw::draws(block)
                    && block != create_engine::Block::MechanicalDrill =>
                {
                    let rest = actor_draw::Doing {
                        lit: true,
                        ..actor_draw::Doing::default()
                    };
                    actor_draw::actor_parts(&mut actor_draw::Drawing(draw), block, &info, &rest);
                }
                None => {}
            }
        } else {
            draw_entity_block(host, draw, b.block, &props);
            if let Some(entity) = &b.entity {
                let id = b.block.id(host);
                if super::vanilla_draw::draws(&id) {
                    super::vanilla_draw::draw(host, draw, &id, &props, &entity.nbt);
                }
            }
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

/// A vanilla block whose block entity draws all of it (`RenderShape.ENTITYBLOCK_ANIMATED`: a
/// chest, an ender chest, a shulker box), as `renderBlockEntities` draws it riding: its item's
/// model as authored (the same special renderer), turned to the block's facing as
/// `ChestRenderer` turns it (`-facing.toYRot()` about its middle), closed.
fn draw_entity_block(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    block: BlockKey,
    props: &[(&str, &str)],
) {
    let id = block.id(host);
    let drawn = matches!(
        id.as_str(),
        "minecraft:chest" | "minecraft:trapped_chest" | "minecraft:ender_chest"
    ) || id.ends_with("shulker_box");
    if !drawn {
        return;
    }
    let Some(stack) = host
        .block_item(block)
        .and_then(|i| mod_api::ItemStack::new(i, 1))
    else {
        return;
    };
    let prop = |k: &str| props.iter().find(|(n, _)| *n == k).map(|(_, v)| *v);
    let y_rot = match prop("facing") {
        Some("west") => 90.0,
        Some("north") => 180.0,
        Some("east") => 270.0,
        _ => 0.0,
    };
    draw.push();
    if let Some(f) = prop("facing").filter(|_| id.ends_with("shulker_box")) {
        // `ShulkerBoxRenderer`: the box turned from up to its facing.
        let (axis, deg) = match f {
            "down" => ([1.0, 0.0, 0.0], 180.0),
            "north" => ([1.0, 0.0, 0.0], -90.0),
            "south" => ([1.0, 0.0, 0.0], 90.0),
            "west" => ([0.0, 0.0, 1.0], 90.0),
            "east" => ([0.0, 0.0, 1.0], -90.0),
            _ => ([1.0, 0.0, 0.0], 0.0),
        };
        draw.rotate_about(axis, deg, [0.5; 3]);
    } else {
        draw.rotate_about([0.0, 1.0, 0.0], -y_rot, [0.5; 3]);
    }
    draw.translate([0.5; 3]);
    draw.item(&stack, mod_api::ItemDisplayContext::None, None, 0);
    draw.pop();
}

/// Blocks whose block entity is not drawn while it rides (`MovementBehaviour
/// .disableBlockEntityRendering`): their movement draws them (`renderInContraption`).
fn drawn_by_movement(block: create_engine::Block) -> bool {
    use create_engine::Block as B;
    matches!(
        block,
        B::Deployer
            | B::MechanicalDrill
            | B::MechanicalSaw
            | B::MechanicalHarvester
            | B::MechanicalRoller
            | B::PortableStorageInterface
            | B::PortableFluidInterface
            | B::BlazeBurner
            | B::MechanicalBearing
    )
}

/// `BlockEntityRenderHelper.renderBlockEntities` for one block of a structure: what its block
/// entity held, drawn by each subsystem as its renderer draws it (`Subsystem::draw_carried`),
/// for a position of its own far from the world's blocks (`anchor`, the block's offset, and
/// far up), with the structure keeping what it draws.
fn draw_carried(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    anchor: [i32; 3],
    b: &Drawn,
    block: create_engine::Block,
    entity: &mod_api::ModBlockEntity,
) {
    if drawn_by_movement(block) {
        return;
    }
    let pos = [
        anchor[0] + b.at[0] as i32,
        anchor[1] + b.at[1] as i32 + CARRIED_UP,
        anchor[2] + b.at[2] as i32,
    ];
    let name = format!("create:{}", block.name());
    let mut inner = Unkept(draw, false);
    let mut event = mod_api::BlockEntitySubmitEvent {
        pos,
        block: &name,
        state: b.state,
        entity,
        draw: &mut inner,
    };
    for s in super::super::SUBSYSTEMS {
        s.draw_carried(host, block, &mut event);
    }
}

/// How far up a riding block entity's stand-in position is ([`draw_carried`]): above any level.
const CARRIED_UP: i32 = 1 << 20;

/// A draw context whose `keep` does nothing: the structure keeps what is drawn, or not. A
/// part a block entity turns (`spin`) is drawn at its phase, still: the structure's own turn is
/// the draw's spin, which a spin inside it would replace (and a riding block entity's kinetic
/// parts stand still, as its speed is not carried).
struct Unkept<'a>(&'a mut dyn BlockEntityDrawContext, bool);

impl Drop for Unkept<'_> {
    fn drop(&mut self) {
        if self.1 {
            self.0.pop();
        }
    }
}

impl BlockEntityDrawContext for Unkept<'_> {
    fn push(&mut self) {
        self.0.push();
    }
    fn pop(&mut self) {
        self.0.pop();
    }
    fn translate(&mut self, by: [f32; 3]) {
        self.0.translate(by);
    }
    fn rotate_about(&mut self, axis: [f32; 3], degrees: f32, pivot: [f32; 3]) {
        self.0.rotate_about(axis, degrees, pivot);
    }
    fn scale(&mut self, by: [f32; 3]) {
        self.0.scale(by);
    }
    fn keep(&mut self) {}
    fn partial(&self) -> f32 {
        self.0.partial()
    }
    fn game_time(&self) -> i64 {
        self.0.game_time()
    }
    fn animation_time(&self) -> f64 {
        self.0.animation_time()
    }
    fn light(&self) -> u32 {
        self.0.light()
    }
    fn set_light(&mut self, light: u32) {
        self.0.set_light(light);
    }
    fn quad(&mut self, sprite: &str, corners: [[f32; 3]; 4], uv: [f32; 4], tint: u32) -> bool {
        self.0.quad(sprite, corners, uv, tint)
    }
    fn cube(
        &mut self,
        from: [f32; 3],
        to: [f32; 3],
        faces: [Option<mod_api::CubeFace<'_>>; 6],
        tint: u32,
    ) -> bool {
        self.0.cube(from, to, faces, tint)
    }
    fn block(&mut self, block: BlockKey, properties: &[(&str, &str)]) -> bool {
        self.0.block(block, properties)
    }
    fn item(
        &mut self,
        stack: &mod_api::ItemStack,
        display: mod_api::ItemDisplayContext,
        owner_yaw: Option<f32>,
        seed: i32,
    ) -> bool {
        self.0.item(stack, display, owner_yaw, seed)
    }
    fn spin(&mut self, axis: [f32; 3], pivot: [f32; 3], _degrees_per_tick: f32, phase: f32) {
        if self.1 {
            self.0.pop();
        }
        self.0.push();
        self.0.rotate_about(axis, phase, pivot);
        self.1 = true;
    }
    fn still(&mut self) {
        if self.1 {
            self.0.pop();
            self.1 = false;
        }
    }
    fn model(&mut self, model: mod_api::ModelRef, tint: u32) -> Result<(), mod_api::ModelError> {
        self.0.model(model, tint)
    }
    fn terrain_model(
        &mut self,
        model: mod_api::ModelRef,
        options: mod_api::TerrainModelOptions<'_>,
    ) -> Result<(), mod_api::TerrainModelError> {
        self.0.terrain_model(model, options)
    }
    fn text(
        &mut self,
        text: &mod_api::Text,
        at: [f32; 2],
        color: u32,
        glowing: bool,
        outline: Option<u32>,
    ) -> Result<f32, mod_api::TextFormError> {
        self.0.text(text, at, color, glowing, outline)
    }
    fn text_width(&self, text: &mod_api::Text) -> Result<f32, mod_api::TextFormError> {
        self.0.text_width(text)
    }
    fn model_part(&mut self, part: &mod_api::ModelPart, sprite: &str, tint: u32) -> bool {
        self.0.model_part(part, sprite, tint)
    }
    fn fluid_box(
        &mut self,
        stack: &mod_api::FluidStack,
        bounds: [f32; 6],
        options: mod_api::FluidBoxOptions,
    ) -> Result<(), mod_api::FluidBoxError> {
        self.0.fluid_box(stack, bounds, options)
    }
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
/// block model; none for a block whose model is already all of it. An actor drawn moving
/// (`actor`) draws its own drill head.
fn draw_parts(
    draw: &mut dyn BlockEntityDrawContext,
    block: create_engine::Block,
    info: &create_engine::parts::StateInfo,
    props: &[(&str, &str)],
    actor: bool,
) {
    use create_engine::client::{Model, draws};
    if whole_model(block) {
        return;
    }
    for d in draws(block, info, [0; 3], [0.0; 2]) {
        if d.model == Model::Own || (actor && matches!(d.model, Model::DrillHead(_))) {
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
    elevator_turn: Option<f32>,
) {
    let Some(Tag::Compound(c)) = data.get(&KEY.into()).filter(|_| holds_structure(data)) else {
        SHOWN.with_borrow_mut(|s| s.remove(&pos));
        actor_draw::client_forget(pos);
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
                    turned: None,
                    floors: Vec::new(),
                    synced: None,
                },
            )
        });
    }
    let now = draw.game_time() as f64 + f64::from(draw.partial());
    let partial = draw.partial();
    let camera = host.camera().map(|c| c.position);
    let anim = draw.animation_time();
    let speed = own_speed(data);
    let view = actor_draw::view(data);
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
                limit,
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
                let turned = rate * (now - since) as f32;
                // A sequenced turn is held where its limit runs out.
                let held = limit.map(|l| turned.clamp(-l, l));
                let here = angle + held.unwrap_or(turned);
                match (&view, held) {
                    // Its actors move each frame: all of it is drawn each frame, turned.
                    (Some(view), _) => {
                        let stopped = held.is_some_and(|h| h != turned);
                        let frame = Frame {
                            anchor: pos,
                            view,
                            motion: Motion::Turn {
                                axis: axis_enum(*axis),
                                angle: here % 360.0,
                                rate: if stopped { 0.0 } else { rate },
                            },
                            now,
                            partial,
                            camera,
                            elevator: None,
                        };
                        shown.turned = Some((axis_enum(*axis), here % 360.0));
                        draw.push();
                        draw.rotate_about(*axis, here % 360.0, [0.5; 3]);
                        draw_blocks(host, draw, pos, blocks, Some(&frame));
                        draw.pop();
                    }
                    // A sequenced turn is drawn each frame.
                    (None, Some(_)) => {
                        draw.push();
                        draw.rotate_about(*axis, here % 360.0, [0.5; 3]);
                        draw_blocks(host, draw, pos, blocks, None);
                        draw.pop();
                    }
                    (None, None) => {
                        let phase = (f64::from(here) - anim * f64::from(rate)) % 360.0;
                        draw.spin(*axis, [0.5; 3], rate, phase as f32);
                        draw_blocks(host, draw, pos, blocks, None);
                        draw.still();
                        draw.keep();
                    }
                }
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
                    draw_blocks(host, draw, pos, blocks, None);
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
                let free = offset + per_tick * (now - since) as f32;
                let mut at = free;
                if let Some((lo, hi)) = clamp {
                    at = at.clamp(*lo, *hi);
                }
                let shift = [0, 1, 2].map(|i| origin[i] + dir[i] * at);
                shown.drawn_at = Some([0, 1, 2].map(|i| f64::from(pos[i]) + f64::from(shift[i])));
                // Held at an end of its range, it doesn't move.
                let step = if at == free { *per_tick } else { 0.0 };
                let frame = view.as_ref().map(|view| Frame {
                    anchor: pos,
                    view,
                    motion: Motion::Glide {
                        shift,
                        velocity: dir.map(|d| d * step),
                    },
                    now,
                    partial,
                    camera,
                    elevator: None,
                });
                draw.push();
                draw.translate(shift);
                draw_blocks(host, draw, pos, blocks, frame.as_ref());
                draw.pop();
                // The magnet and rope down to it.
                if *rope {
                    draw_hanging(host, draw, None, at, true);
                }
            }
            Shape::Elevator {
                blocks,
                e,
                prev_y,
                target,
                ..
            } => {
                let y = prev_y + (e.y - prev_y) * f64::from(draw.partial());
                let origin = [
                    (e.anchor[0] - pos[0]) as f32,
                    (y - f64::from(pos[1])) as f32,
                    (e.anchor[2] - pos[2]) as f32,
                ];
                shown.drawn_at = Some([0, 1, 2].map(|i| f64::from(pos[i]) + f64::from(origin[i])));
                let floors = super::lifts::floors_of(data);
                if floors != shown.floors {
                    shown.floors = floors;
                }
                let current = (e.y + f64::from(e.contact_y)).round() as i32;
                let elevator = actor_draw::ElevatorView {
                    floors: &shown.floors,
                    target: target.unwrap_or(current),
                    min_contact: e.min_contact,
                    max_contact: e.max_contact,
                };
                let frame = view.as_ref().map(|view| Frame {
                    anchor: pos,
                    view,
                    motion: Motion::Glide {
                        shift: origin,
                        velocity: [0.0, (e.y - prev_y) as f32, 0.0],
                    },
                    now,
                    partial,
                    camera,
                    elevator: Some(elevator),
                });
                draw.push();
                draw.translate(origin);
                draw_blocks(host, draw, pos, blocks, frame.as_ref());
                draw.pop();
                // The magnet a block over its anchor, and the belt up to the pulley.
                let offset = (f64::from(pos[1]) - y - 1.0) as f32;
                draw_hanging(host, draw, elevator_turn.or(Some(180.0)), offset, true);
            }
        }
    });
}

/// The channel a client's click on contraption controls riding a moving structure goes to
/// the server on: the structure's anchor and the block's offset in it, six ints, and on an
/// elevator the height of the floor its controls show when it is not the one called
/// (`ElevatorTargetFloorPacket`).
pub const CONTROLS_CHANNEL: &str = "create:contraption_controls";

/// Reads a click sent on [`CONTROLS_CHANNEL`].
pub fn decode_click(data: &[u8]) -> Option<([i32; 3], [i32; 3], Option<i32>)> {
    let v: Vec<i32> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| i32::from_le_bytes(*b))
        .collect();
    match v[..] {
        [a, b, c, x, y, z] => Some(([a, b, c], [x, y, z], None)),
        [a, b, c, x, y, z, t] => Some(([a, b, c], [x, y, z], Some(t))),
        _ => None,
    }
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

/// A contraption controls block a client's ray meets on a structure it draws.
struct Picked {
    anchor: [i32; 3],
    local: [i32; 3],
    /// Where the ray meets it, from the block's corner in the structure's own frame.
    hit: [f64; 3],
}

/// `ContraptionHandlerClient.getRayInputs` and `rayTraceContraption` for contraption controls:
/// the nearest contraption controls block on a structure this client draws (gliding, turning
/// or an elevator) that the player's ray (as far as they reach, or to the block the crosshair is
/// on) meets, its ray turned back into the structure's own frame. None while the crosshair is on
/// an entity.
fn pick_controls(host: &dyn ClientHost) -> Option<Picked> {
    use mod_api::CrosshairTarget;
    if SHOWN.with_borrow(HashMap::is_empty) {
        return None;
    }
    let (player, camera) = (host.local_player()?, host.camera()?);
    if player.is_spectator(host) == Some(true) {
        return None;
    }
    let eye = camera.position;
    let (yaw, pitch) = (camera.yaw.to_radians(), camera.pitch.to_radians());
    let look = [
        f64::from(-yaw.sin() * pitch.cos()),
        f64::from(-pitch.sin()),
        f64::from(yaw.cos() * pitch.cos()),
    ];
    let mut reach: f64 = if player.is_creative(host) == Some(true) {
        5.0
    } else {
        4.5
    };
    match host.crosshair_target() {
        Some(CrosshairTarget::Block { hit, .. }) => {
            let d: f64 = (0..3).map(|i| (hit.point[i] - eye[i]).powi(2)).sum();
            reach = d.sqrt().min(reach);
        }
        Some(CrosshairTarget::Entity { .. }) => return None,
        _ => {}
    }
    let target: [f64; 3] = std::array::from_fn(|i| eye[i] + look[i] * reach);
    SHOWN.with_borrow(|s| {
        let mut best: Option<(f64, Picked)> = None;
        for (anchor, shown) in s {
            // Offset 0 of the structure in the world, and the ray in its own frame.
            let (blocks, at, ray) = match (&shown.shape, shown.drawn_at, shown.turned) {
                (Shape::Glide { blocks, .. } | Shape::Elevator { blocks, .. }, Some(at), _) => {
                    (blocks, at, (eye, target))
                }
                (Shape::Turn { blocks, .. }, _, Some((axis, angle))) => {
                    let c = anchor.map(|v| f64::from(v) + 0.5);
                    let back = |p: [f64; 3]| {
                        let r = [0, 1, 2].map(|i| p[i] - c[i]);
                        let t = create_engine::actors::turn_vec(r, axis, -f64::from(angle));
                        [0, 1, 2].map(|i| t[i] + c[i])
                    };
                    (blocks, anchor.map(f64::from), (back(eye), back(target)))
                }
                _ => continue,
            };
            for b in blocks.iter().filter(|b| is_controls(b)) {
                let min = [0, 1, 2].map(|i| at[i] + f64::from(b.at[i]));
                if let Some(t) = ray_box(ray.0, ray.1, min)
                    && best.as_ref().is_none_or(|x| t < x.0)
                {
                    let hit = [0, 1, 2].map(|i| ray.0[i] + (ray.1[i] - ray.0[i]) * t - min[i]);
                    let local = b.at.map(|v| v.round() as i32);
                    best = Some((
                        t,
                        Picked {
                            anchor: *anchor,
                            local,
                            hit,
                        },
                    ));
                }
            }
        }
        best.map(|b| b.1)
    })
}

/// The elevator drawn by `anchor`: its floors, the floor it is called to and the contacts it
/// reaches, and where its contact is now; None for any other structure.
fn elevator_of<R>(
    anchor: [i32; 3],
    f: impl FnOnce(actor_draw::ElevatorView<'_>, i32) -> R,
) -> Option<R> {
    SHOWN.with_borrow(|s| {
        let shown = s.get(&anchor)?;
        let Shape::Elevator { e, target, .. } = &shown.shape else {
            return None;
        };
        let current = (e.y + f64::from(e.contact_y)).round() as i32;
        let view = actor_draw::ElevatorView {
            floors: &shown.floors,
            target: target.unwrap_or(current),
            min_contact: e.min_contact,
            max_contact: e.max_contact,
        };
        Some(f(view, current))
    })
}

/// `ContraptionHandlerClient.rightClickingOnContraptionsGetsHandledLocally` for contraption
/// controls: a use click whose ray meets a contraption controls block of a structure this
/// client draws presses its button and goes to the server as a click on it, and nothing else
/// happens (`ContraptionControlsMovingInteraction`). On an elevator it asks for the floor the
/// controls show when that is not the one called (`elevatorInteraction`: the button pressed only
/// then).
pub fn use_key(
    host: &mut dyn ClientHost,
    event: &mut mod_api::InteractionKeyMappingTriggeredEvent,
) -> mod_api::Flow {
    use mod_api::{Flow, Hand, InteractionKey};
    if event.key != InteractionKey::Use || event.hand != Hand::Main || event.cancelled {
        return Flow::Continue;
    }
    let Some(p) = pick_controls(&*host) else {
        return Flow::Continue;
    };
    let mut ints: Vec<i32> = p.anchor.iter().chain(&p.local).copied().collect();
    let elevator = elevator_of(p.anchor, |view, _| {
        actor_draw::with_controls(p.anchor, p.local, |a| {
            let sel = view.select(&mut a.index);
            (sel.target_y != view.target).then(|| {
                a.press();
                sel.target_y
            })
        })
    });
    match elevator {
        Some(Some(y)) => ints.push(y),
        Some(None) => {}
        None => actor_draw::with_controls(p.anchor, p.local, |a| a.press()),
    }
    let data: Vec<u8> = ints.iter().flat_map(|v| v.to_le_bytes()).collect();
    super::super::net::send_to_server(host, CONTROLS_CHANNEL, &data);
    event.cancelled = true;
    event.swing_hand = true;
    Flow::Cancel
}

/// `ElevatorControlsHandler.onScroll`: scrolling while the player's ray meets the button of an
/// elevator's contraption controls (`ElevatorControlsSlot.testHit`: within .85 of the slot's
/// scale of the button's middle) picks the floor above or below for them to show (a click
/// calls it), with the value scroll's click, higher toward the top floor.
pub fn scroll(host: &mut dyn ClientHost, delta: f64) -> bool {
    if delta == 0.0 {
        return false;
    }
    let Some(p) = pick_controls(&*host) else {
        return false;
    };
    let Some(facing) = SHOWN.with_borrow(|s| {
        let shown = s.get(&p.anchor)?;
        let Shape::Elevator { blocks, .. } = &shown.shape else {
            return None;
        };
        blocks
            .iter()
            .find(|b| is_controls(b) && b.at.map(|v| v.round() as i32) == p.local)
            .and_then(|b| b.create.and_then(|(_, i)| i.facing))
    }) else {
        return false;
    };
    // `ControlsSlot.getLocalOffset`: (8, 14, 5.5) px turned to the controls' facing.
    let yaw = f64::from(create_engine::display::horizontal_angle(facing));
    let v =
        create_engine::actors::turn_vec([0.0, 14.0 / 16.0 - 0.5, 5.5 / 16.0 - 0.5], Axis::Y, yaw);
    let offset = [v[0] + 0.5, v[1] + 0.5, v[2] + 0.5];
    let d: f64 = (0..3).map(|i| (p.hit[i] - offset[i]).powi(2)).sum();
    if d.sqrt() >= 0.508 * 0.85 {
        return false;
    }
    let step = if delta > 0.0 {
        delta.ceil()
    } else {
        delta.floor()
    } as i32;
    let moved = elevator_of(p.anchor, |view, _| {
        actor_draw::with_controls(p.anchor, p.local, |a| {
            let prev = a.index;
            a.index += step;
            view.select(&mut a.index);
            (prev != a.index && !view.floors.is_empty())
                .then(|| a.index as f32 / view.floors.len() as f32)
        })
    })
    .flatten();
    if let Some(f) = moved {
        let pitch = 1.0 + (1.5 - 1.0) * f;
        let at = SHOWN.with_borrow(|s| s.get(&p.anchor).and_then(|x| x.drawn_at));
        if let Some(at) = at {
            let point = [0, 1, 2].map(|i| (at[i] + f64::from(p.local[i]) + p.hit[i]).floor() + 0.5);
            let sound =
                mod_api::SoundPlay::at("create:scroll_value", mod_api::SoundSource::Players, point)
                    .with_pitch(pitch);
            host.play_local_sound(&sound, 0);
        }
    }
    true
}

thread_local! {
    static HANGING: RefCell<Option<[Option<BlockKey>; 2]>> = const { RefCell::new(None) };
}

/// `AbstractPulleyRenderer.renderSafe` (a rope pulley's: `turn` none) and
/// `ElevatorPulleyRenderer.renderSafe` (an elevator's, turned `turn` degrees about up): the
/// magnet (or its half near the pulley) `offset` blocks down while running or at 0, the rope's
/// (or belt's) half under the pulley, and while running the rope down to the magnet.
fn draw_hanging(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    turn: Option<f32>,
    offset: f32,
    running: bool,
) {
    let [rope, magnet] = HANGING.with_borrow_mut(|h| {
        *h.get_or_insert_with(|| {
            ["create:rope", "create:pulley_magnet"]
                .map(|b| host.block_state_id(b).map(|s| s.block()))
        })
    });
    let at = |draw: &mut dyn BlockEntityDrawContext, key: Option<BlockKey>, down: f32| {
        let Some(key) = key else {
            return;
        };
        // The rope and magnet blocks have no facing; the partials are drawn as written.
        let props: &[(&str, &str)] = if Some(key) == rope || Some(key) == magnet {
            &[]
        } else {
            &[("facing", "up")]
        };
        draw.push();
        draw.translate([0.0, -down, 0.0]);
        if let Some(a) = turn {
            draw.rotate_about([0.0, 1.0, 0.0], a, [0.5; 3]);
        }
        draw.block(key, props);
        draw.pop();
    };
    let part = states::partial;
    let f = offset % 1.0;
    let nudge = if f > 0.75 { f - 1.0 } else { f };
    match turn {
        None => {
            if running || offset == 0.0 {
                let m = if offset > 0.25 {
                    magnet
                } else {
                    part("part_rope_half_magnet")
                };
                at(draw, m, offset);
            }
            if offset > 0.75 && !(0.25..=0.75).contains(&f) {
                at(draw, part("part_rope_half"), nudge);
            }
            if running {
                let mut i = 0;
                while (i as f32) < offset - 1.25 && i < 512 {
                    at(draw, rope, offset - i as f32 - 1.0);
                    i += 1;
                }
            }
        }
        Some(_) => {
            if running || offset == 0.0 {
                at(draw, part("part_elevator_magnet"), offset);
            }
            if offset == 0.0 {
                return;
            }
            if !(0.25..=0.75).contains(&f) {
                at(draw, part("part_elevator_belt_half"), nudge);
            }
            if running {
                let mut i = 0;
                while (i as f32) < offset - 0.25 && i < 512 {
                    at(draw, part("part_elevator_belt"), offset - i as f32);
                    i += 1;
                }
            }
        }
    }
}

/// A rope or elevator pulley at rest draws what hangs from it (`PULLEY_REST`).
fn draw_resting(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let elevator = match event.block {
        "create:rope_pulley" => false,
        "create:elevator_pulley" => true,
        _ => return,
    };
    let offset = match super::super::kinetics::synced(&event.entity.nbt, PULLEY_REST) {
        Some(Tag::Int(v)) => *v as f32,
        _ => 0.0,
    };
    let turn = elevator.then(|| facing_turn(host, event.state));
    draw_hanging(host, event.draw, turn, offset, false);
}

/// An elevator pulley's turn by its state's facing.
fn facing_turn(host: &dyn ClientHost, state: mod_api::BlockState) -> f32 {
    let facing = host
        .block_state_string(state)
        .and_then(|t| {
            let (_, props) = states::split(&t);
            props
                .iter()
                .find(|(k, _)| *k == "facing")
                .map(|(_, v)| match *v {
                    "south" => Dir::South,
                    "west" => Dir::West,
                    "east" => Dir::East,
                    _ => Dir::North,
                })
        })
        .unwrap_or(Dir::North);
    elevator_turn(facing)
}

/// `180 + AngleHelper.horizontalAngle(facing)`.
pub(super) fn elevator_turn(facing: Dir) -> f32 {
    180.0 + create_engine::decor::horizontal_angle(facing)
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
        if !holds_structure(&event.entity.nbt) && SHOWN.with_borrow(|s| !s.contains_key(&event.pos))
        {
            draw_resting(host, event);
            return;
        }
        let turn =
            (event.block == "create:elevator_pulley").then(|| facing_turn(host, event.state));
        submit(host, event.pos, &event.entity.nbt, event.draw, turn);
    }

    /// A structure is drawn wherever it moves to: never culled.
    fn block_entity_render_bounding_box(
        &self,
        _host: &dyn ClientHost,
        event: &mut BlockEntityRenderBoundingBoxEvent<'_>,
    ) {
        if holds_structure(&event.entity.nbt) {
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
