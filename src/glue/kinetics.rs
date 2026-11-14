//! Rotational power: Create's blocks with state of their own are one block entity type, so the
//! game tells the mod when each comes into a level and draws its turning parts; the engine's
//! network solves once per level tick, and each part's speed reaches clients in its block
//! entity's data (`Speeds`), from which its renderer turns the part on the animation clock.

use super::persist;
use super::server::ServerLevel;
use super::states;
use super::subsystem::Subsystem;
use super::world::{level_index, with_world};
use create_engine::Block;
use create_engine::client::{self, Draw, Model};
use create_engine::kinetics::{Axis, Dir, Event};
use create_engine::world::keeps_state;
use mod_api::{
    BlockEntityDrawContext, BlockEntityOnLoadEvent, BlockEntityPart, BlockEntitySubmitEvent,
    BlockEntityTypeKey, BlockEntityUpdateTagEvent, BlockPart, ClientHost, Compound, HostSide,
    Registrar, RegistrationContext, Tag,
};
use std::sync::OnceLock;

/// The type's id: every Create block the world keeps state for.
pub const TYPE: &str = "create:kinetic";

/// The key in a block entity's data under which its parts' speeds are, `[slot 0, slot 1]`.
const SPEEDS: &str = "Speeds";

static KIND: OnceLock<BlockEntityTypeKey> = OnceLock::new();

/// The block entity type of Create's kept blocks.
pub fn kind() -> Option<BlockEntityTypeKey> {
    KIND.get().copied()
}

pub struct Kinetics;

impl Subsystem for Kinetics {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        let blocks: Vec<String> = Block::ALL
            .iter()
            .filter(|&&b| keeps_state(b))
            .map(|b| format!("create:{}", b.name()))
            .collect();
        let names: Vec<&str> = blocks.iter().map(String::as_str).collect();
        let kind = r.block_entity_type(TYPE).blocks(&names).build();
        let _ = KIND.set(kind);
        if let Err(e) = r.block_entity_part(&kind, KineticEntity) {
            eprintln!("[create] the kinetic block entity's part was refused: {e:?}");
        }
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        let saved = settings_from_nbt(data);
        let dim = lv.dim;
        with_world(|w| w.load_settings(dim, saved.into_iter()));
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        let settings =
            with_world(|w| w.column_settings(lv.dim, chunk[0], chunk[1])).unwrap_or_default();
        if !settings.is_empty() {
            let motors = settings
                .iter()
                .flat_map(|(p, rpm)| [p[0], p[1], p[2], *rpm])
                .collect();
            data.insert("motors".into(), Tag::IntArray(motors));
        }
    }

    fn chunk_unload(&self, dim: usize, chunk: [i32; 2]) {
        with_world(|w| w.column_unloaded(dim, chunk[0], chunk[1]));
    }

    fn block_changed(&self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        with_world(|w| w.block_changed(dim, pos, old, new));
    }

    fn neighbor_changed(&self, dim: usize, pos: [i32; 3], state: u32) {
        with_world(|w| w.neighbor_changed(dim, pos, state));
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        tick(lv);
    }
}

/// Settings saved with a column: `motors` holds x, y, z and speed for each creative motor.
fn settings_from_nbt(data: &Compound) -> Vec<([i32; 3], i32)> {
    match data.get(&"motors".into()) {
        Some(Tag::IntArray(v)) => v
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| ([c[0], c[1], c[2]], c[3]))
            .collect(),
        _ => Vec::new(),
    }
}

/// A level's kinetic tick: cranks and valves turning down, split shafts following redstone,
/// the network solved, the blocks Create breaks broken, and every changed part's speed put in
/// its block entity's data for its clients.
fn tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    with_world(|w| w.tick_cranks(dim));
    // Water wheels whose surroundings changed, and one in 60 of the others each tick.
    let now = lv.game_time() as u64;
    for (pos, axis, large) in with_world(|w| w.take_wheel_checks(dim, now)).unwrap_or_default() {
        let offsets = if large {
            create_engine::waterwheel::large_offsets(axis)
        } else {
            create_engine::waterwheel::small_offsets(axis).to_vec()
        };
        let samples: Vec<([i32; 3], [f64; 3])> = offsets
            .into_iter()
            .map(|o| {
                let at = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
                let flow = lv
                    .level
                    .fluid_flow(&*lv.host, at)
                    .map_or([0.0; 3], |v| [v.x, v.y, v.z]);
                (o, flow)
            })
            .collect();
        let score = create_engine::waterwheel::flow_score(axis, &samples);
        with_world(|w| w.set_wheel_flow(dim, pos, score));
    }
    // Clutches and gearshifts follow redstone (`ClutchBlock.neighborChanged`): `powered` is set
    // before solving, so the change turns the network in this tick's solve.
    for pos in with_world(|w| w.take_redstone(dim)).unwrap_or_default() {
        let state = lv.block(pos);
        let Some(was) = with_world(|w| w.powered(state)).flatten() else {
            continue;
        };
        let now = lv.has_neighbor_signal(pos);
        if now != was {
            let value = if now { "true" } else { "false" };
            lv.set_property(pos, "powered", value, super::server::UPDATE_CLIENTS);
        }
        // `ChainGearshiftBlockEntity.neighbourChanged`: the signal's strength sets its chain.
        if with_world(|w| w.reads_signal(state)) == Some(true) {
            let signal = lv.best_neighbor_signal(pos);
            with_world(|w| w.set_chain_signal(dim, pos, signal));
        }
    }
    // Encased chain drives placed or beside a change take their place in their chain
    // (`ChainDriveBlock.updateShape`).
    for (pos, drive) in with_world(|w| w.take_chain_shapes(dim)).unwrap_or_default() {
        let along = if drive.along_first { "true" } else { "false" };
        lv.set_property(pos, "axis_along_first", along, super::server::update_all());
        lv.set_property(pos, "part", drive.part.name(), super::server::update_all());
    }
    tick_sequencers(lv);
    let mut events = Vec::new();
    let Some((removed, unsaved)) = with_world(|w| {
        w.settle_belts(dim);
        w.tick(dim, &mut events);
        (w.dims[dim].kinetics.take_removed(), w.take_unsaved(dim))
    }) else {
        return;
    };
    for (cx, cz) in unsaved {
        persist::mark_unsaved(dim, [cx, cz]);
    }
    for pos in removed {
        set_speeds(lv, pos, None);
    }
    for e in events {
        match e {
            // Create's `world.destroyBlock(pos, true)`: the block breaks with its drops.
            Event::Break { pos } => {
                lv.destroy_block(pos, true);
            }
            Event::Speed {
                pos, sub, speed, ..
            } => {
                let mut speeds = current_speeds(lv, pos);
                speeds[usize::from(sub & 1)] = speed;
                set_speeds(lv, pos, Some(speeds));
                for s in super::SUBSYSTEMS {
                    s.speed_changed(lv, pos, speed);
                }
            }
        }
    }
}

/// Sequenced gearshifts: those whose neighbour changed look at their power
/// (`SequencedGearshiftBlock.tick`), those running an instruction tick; their blocks show the
/// step (`state`).
fn tick_sequencers(lv: &mut ServerLevel<'_>) {
    use create_engine::sequencer::Step;
    let dim = lv.dim;
    let mut steps = Vec::new();
    for (pos, running) in with_world(|w| w.take_sequencer_checks(dim)).unwrap_or_default() {
        let powered = lv.has_neighbor_signal(pos);
        if let Some(step) =
            with_world(|w| w.sequencer_redstone(dim, pos, powered, running)).flatten()
        {
            steps.push((pos, step));
        }
    }
    steps.extend(with_world(|w| w.tick_sequencers(dim)).unwrap_or_default());
    for (pos, step) in steps {
        let state = match step {
            Step::Run(n) => n,
            Step::Idle => 0,
            // `run` past the end: back to 0 unless still powered.
            Step::Ended if lv.has_neighbor_signal(pos) => continue,
            Step::Ended => 0,
        };
        const STATES: [&str; 6] = ["0", "1", "2", "3", "4", "5"];
        lv.set_property(
            pos,
            "state",
            STATES[usize::from(state.min(5))],
            super::server::update_all(),
        );
    }
}

/// The speeds a block entity shows now.
fn current_speeds(lv: &ServerLevel<'_>, pos: [i32; 3]) -> [f32; 2] {
    lv.host
        .block_entity(lv.level, pos)
        .and_then(|e| e.data(&*lv.host))
        .map_or([0.0; 2], |d| speeds_of(&d))
}

fn speeds_of(data: &Compound) -> [f32; 2] {
    match data.get(&SPEEDS.into()) {
        Some(Tag::List(l)) => {
            let at = |i: usize| match l.get(i) {
                Some(Tag::Float(bits)) => f32::from_bits(*bits),
                _ => 0.0,
            };
            [at(0), at(1)]
        }
        _ => [0.0; 2],
    }
}

/// Puts `speeds` (none: stopped, it left the network) in the block entity at `pos`, which
/// sends them to the clients that see it.
fn set_speeds(lv: &mut ServerLevel<'_>, pos: [i32; 3], speeds: Option<[f32; 2]>) {
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let [a, b] = speeds.unwrap_or_default();
    let data = Compound::from([(
        SPEEDS.into(),
        Tag::List(vec![Tag::Float(a.to_bits()), Tag::Float(b.to_bits())]),
    )]);
    let _ = entity.merge_data(&mut *lv.host, data);
}

/// The kinetic block entity: found by the world when it loads, drawn by its parts.
struct KineticEntity;

#[mod_api::horizon_part]
impl BlockPart for KineticEntity {}

#[mod_api::horizon_part]
impl BlockEntityPart for KineticEntity {
    fn block_entity_on_load(&self, host: HostSide, event: &mut BlockEntityOnLoadEvent) {
        let HostSide::Server(host) = host else {
            return;
        };
        let entity = event.entity;
        let (Some(level), Some(pos)) = (entity.level(&*host), entity.position(&*host)) else {
            return;
        };
        let Some(dimension) = level.dimension().id(&*host) else {
            return;
        };
        let Some(dim) = level_index(&dimension) else {
            return;
        };
        let Some(state) = level.block_state(&*host, pos).and_then(states::id) else {
            return;
        };
        super::ensure_column(host, &dimension, [pos[0] >> 4, pos[2] >> 4]);
        with_world(|w| w.block_loaded(dim, pos, state));
    }

    fn block_entity_update_tag(
        &self,
        event: &mut BlockEntityUpdateTagEvent<'_>,
    ) -> Option<Compound> {
        // The parts' speeds, and the structure a bearing, piston or pulley moves (`contraptions`).
        let mut tag = Compound::new();
        for key in [SPEEDS, super::contraptions::draw::KEY] {
            if let Some(v) = event.entity.nbt.get(&key.into()) {
                tag.insert(key.into(), v.clone());
            }
        }
        (!tag.is_empty()).then_some(tag)
    }

    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some(id) = states::id(event.state) else {
            return;
        };
        let Some((block, info)) =
            super::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
        else {
            return;
        };
        let speeds = speeds_of(&event.entity.nbt);
        let text = host.block_state_string(event.state).unwrap_or_default();
        let (_, own) = states::split(&text);
        // A structure moving along a line is drawn again each frame: nothing of it is kept.
        let moving = super::contraptions::draw::moving(&event.entity.nbt);
        for d in client::draws(block, &info, event.pos, speeds) {
            if moving {
                draw_turning(event.draw, block, &own, &d);
            } else {
                draw_part(event.draw, block, &own, &d);
            }
        }
    }
}

/// The unit vector of `axis`.
fn axis_vector(axis: Axis) -> [f32; 3] {
    match axis {
        Axis::X => [1.0, 0.0, 0.0],
        Axis::Y => [0.0, 1.0, 0.0],
        Axis::Z => [0.0, 0.0, 1.0],
    }
}

fn axis_name(axis: Axis) -> &'static str {
    match axis {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

pub fn dir_name(d: Dir) -> &'static str {
    match d {
        Dir::Down => "down",
        Dir::Up => "up",
        Dir::North => "north",
        Dir::South => "south",
        Dir::West => "west",
        Dir::East => "east",
    }
}

/// The block and properties whose model a part is: the block's own, Create's shaft, or one of
/// the content's partial models (`create:part_*` blocks, never placed, one model per facing).
fn model_block(
    block: Block,
    model: Model,
) -> Option<(Block, Option<(&'static str, &'static str)>)> {
    let part = |name: &str, facing: &'static str| {
        Block::from_name(name).map(|b| (b, Some(("facing", facing))))
    };
    match model {
        Model::Own => Some((block, None)),
        Model::Shaft(a) => Some((Block::Shaft, Some(("axis", axis_name(a))))),
        Model::ShaftHalf(d) => part("part_shaft_half", dir_name(d)),
        Model::CrankHandle(d) => part("part_hand_crank_handle", dir_name(d)),
        Model::WaterWheel(d) => part("part_water_wheel", dir_name(d)),
        Model::MillstoneCog => part("part_millstone_cog", "up"),
        Model::PressHead(d) => part("part_press_head", dir_name(d)),
        Model::BearingTop(d) => part("part_bearing_top_wooden", dir_name(d)),
        Model::BearingTopMetal(d) => part("part_bearing_top", dir_name(d)),
        Model::FanPropeller(d) => part("part_fan_propeller", dir_name(d)),
        Model::ShaftlessCog => part("part_cogwheel_shaftless", "up"),
        Model::PumpCog(d) => part("part_pump_cog", dir_name(d)),
        Model::DrillHead(d) => part("part_drill_head", dir_name(d)),
        Model::GaugeHead(d, create_engine::gauge::Kind::Speed) => {
            part("part_gauge_head_speed", dir_name(d))
        }
        Model::GaugeHead(d, create_engine::gauge::Kind::Stress) => {
            part("part_gauge_head_stress", dir_name(d))
        }
        Model::GaugeDial(d) => part("part_gauge_dial", dir_name(d)),
        Model::ArmCog => part("part_arm_cog", "up"),
        Model::HoseCoil(d) => part("part_hose_coil", dir_name(d)),
        // Posed partials (belts, the mixer's pole and head) are drawn by their own systems.
        Model::Belt(..) | Model::BeltPulley(_) | Model::MixerPole | Model::MixerHead => None,
    }
}

/// Draws one turning part of `block` (whose state has the properties `own`): its model at
/// rest, turning at its speed about its axis through the block's centre (`rpm * 0.3` degrees a
/// tick), from its starting angle. Kept until the block entity changes.
pub fn draw_part(
    draw: &mut dyn BlockEntityDrawContext,
    block: Block,
    own: &[(&str, &str)],
    d: &Draw,
) {
    draw_turning(draw, block, own, d);
    draw.keep();
}

/// [`draw_part`] for this frame only.
fn draw_turning(
    draw: &mut dyn BlockEntityDrawContext,
    block: Block,
    own: &[(&str, &str)],
    d: &Draw,
) {
    let Some((model, prop)) = model_block(block, d.model) else {
        return;
    };
    draw.spin(axis_vector(d.axis), [0.5; 3], d.rpm * 0.3, d.phase);
    match prop {
        Some(p) => draw.block(states::key(model), &[p]),
        None => draw.block(states::key(model), own),
    };
    draw.still();
}
