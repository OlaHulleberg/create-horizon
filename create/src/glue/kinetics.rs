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
    BlockEntityTypeKey, BlockEntityUpdateTagEvent, BlockKey, BlockPart, ClientHost, Compound,
    HostSide, Registrar, RegistrationContext, Tag,
};
use std::sync::OnceLock;

/// The type's id: every Create block the world keeps state for.
pub const TYPE: &str = "create:kinetic";

/// The key in a block entity's data under which its parts' speeds are, `[slot 0, slot 1]`.
const SPEEDS: &str = "Speeds";
/// The keys of its first part's theoretical speed (`getTheoreticalSpeed`, what the goggles
/// show) and of whether its network is overstressed (`isOverStressed`).
pub const THEORY: &str = "Theory";
pub const OVER: &str = "Over";
/// Whether a speed controller shows its bracket (`SpeedControllerBlockEntity.hasBracket`).
const BRACKET: &str = "Bracket";

thread_local! {
    /// Speed controllers to look at again: one, or the large cog over one, came or went.
    static BRACKETS: std::cell::RefCell<Vec<(usize, [i32; 3])>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

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
            crate::create_log!("[create] the kinetic block entity's part was refused: {e:?}");
        }
        super::transmission::register(r);
        super::sequencer::register(r);
    }

    fn client_message(
        &self,
        host: &mut dyn mod_api::ServerHost,
        player: mod_api::PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        super::sequencer::client_message(host, player, channel, data)
    }

    fn server_message(&self, host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        super::sequencer::server_message(host, channel, data)
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        super::sequencer::client_tick(host);
    }

    fn client_reset(&self) {
        super::sequencer::client_reset();
    }

    fn screen_render_post(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenRenderPostEvent<'_>,
    ) {
        super::sequencer::screen_render_post(host, event);
    }

    fn screen_mouse_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenMouseButtonPressedPreEvent<'_>,
    ) -> mod_api::Flow {
        super::sequencer::screen_mouse_pressed(host, event)
    }

    fn screen_mouse_scrolled(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::ScreenMouseScrolledPreEvent<'_>,
    ) -> mod_api::Flow {
        super::sequencer::screen_mouse_scrolled(host, event)
    }

    fn screen_closing(&self, host: &dyn ClientHost, event: &mut mod_api::ScreenClosingEvent<'_>) {
        super::sequencer::screen_closing(host, event);
    }

    fn wrench_used(
        &self,
        lv: &mut ServerLevel<'_>,
        _player: mod_api::PlayerRef,
        pos: [i32; 3],
        block: Block,
        clicked: Dir,
        _point: [f64; 3],
        sneaking: bool,
    ) -> bool {
        super::transmission::wrench_used(lv, pos, block, clicked, sneaking)
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
        let watched = with_world(|w| {
            w.block_changed(dim, pos, old, new);
            [old, new].iter().any(|&s| {
                matches!(
                    w.table.get(s).map(|(b, _)| b),
                    Some(Block::LargeCogwheel | Block::RotationSpeedController)
                )
            })
        });
        if watched == Some(true) {
            BRACKETS.with_borrow_mut(|b| {
                b.push((dim, pos));
                b.push((dim, [pos[0], pos[1] - 1, pos[2]]));
            });
        }
    }

    fn neighbor_changed(&self, dim: usize, pos: [i32; 3], state: u32) {
        with_world(|w| w.neighbor_changed(dim, pos, state));
    }

    /// Before the rest of the level tick (legacy `level_tick_post`'s head): cranks and valves
    /// turning down, water wheels, split shafts following redstone, chain drives and sequenced
    /// gearshifts. The solve is `Solve`'s, later.
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        tick_inputs(lv);
    }
}

/// The kinetic solve, after every tick that may change what turns (legacy `level_tick_post`:
/// `settle_belts`, `w.tick`, its events).
pub struct Solve;

impl Subsystem for Solve {
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        tick_solve(lv);
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

/// What feeds a level's solve: cranks and valves turning down, water wheels' flow, split shafts
/// following redstone (set before solving, so the change turns the network in this tick's
/// solve), chain drives' places and sequenced gearshifts.
fn tick_inputs(lv: &mut ServerLevel<'_>) {
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
                (o, super::sources::waterwheels::flow_at(lv, at))
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
    tick_brackets(lv);
}

/// `SpeedControllerBlockEntity.updateBracket`: a controller under a large cog on a horizontal
/// axis (`isCogwheelPresent`) shows its bracket; its block entity's data tells its clients.
fn tick_brackets(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let due: Vec<[i32; 3]> = BRACKETS.with_borrow_mut(|b| {
        let (now, later): (Vec<_>, Vec<_>) = b.drain(..).partition(|(d, _)| *d == dim);
        *b = later;
        now.into_iter().map(|(_, p)| p).collect()
    });
    for pos in due {
        let Some(bracket) = with_world(|w| {
            (w.block_at(dim, pos) == Some(Block::RotationSpeedController)).then(|| {
                w.state_at(dim, [pos[0], pos[1] + 1, pos[2]])
                    .and_then(|s| w.table.get(s))
                    .is_some_and(|(b, i)| {
                        b == Block::LargeCogwheel && i.axis.is_some_and(|a| a != Axis::Y)
                    })
            })
        })
        .flatten() else {
            continue;
        };
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let shown = entity
            .data(&*lv.host)
            .is_some_and(|d| matches!(d.get(&BRACKET.into()), Some(Tag::Byte(1))));
        if shown != bracket {
            let data = Compound::from([(BRACKET.into(), Tag::Byte(i8::from(bracket)))]);
            let _ = entity.merge_data(&mut *lv.host, data);
        }
    }
}

/// A level's solve: the network solved, the blocks Create breaks broken, and every changed
/// part's speed put in its block entity's data for its clients.
fn tick_solve(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
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
        set_speeds(lv, pos, None, Some((0.0, false)));
    }
    for e in events {
        match e {
            // Create's `world.destroyBlock(pos, true)`: the block breaks with its drops.
            Event::Break { pos } => {
                lv.destroy_block(pos, true);
            }
            Event::Speed {
                pos,
                sub,
                speed,
                overstressed,
                theoretical,
            } => {
                let mut speeds = current_speeds(lv, pos);
                speeds[usize::from(sub & 1)] = speed;
                let first = (sub == 0).then_some((theoretical, overstressed));
                set_speeds(lv, pos, Some(speeds), first);
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

pub fn speeds_of(data: &Compound) -> [f32; 2] {
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
fn set_speeds(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    speeds: Option<[f32; 2]>,
    first: Option<(f32, bool)>,
) {
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let [a, b] = speeds.unwrap_or_default();
    let mut data = Compound::from([(
        SPEEDS.into(),
        Tag::List(vec![Tag::Float(a.to_bits()), Tag::Float(b.to_bits())]),
    )]);
    if let Some((theory, over)) = first {
        data.insert(THEORY.into(), Tag::Float(theory.to_bits()));
        data.insert(OVER.into(), Tag::Byte(i8::from(over)));
    }
    let _ = entity.merge_data(&mut *lv.host, data);
}

/// The theoretical speed of the first part of the block entity at `pos` of `level`, and whether
/// its network is overstressed, as its clients last heard.
pub fn shown_theory(
    host: &dyn mod_api::Host,
    level: mod_api::LevelRef,
    pos: [i32; 3],
) -> (f32, bool) {
    let Some(d) = host.block_entity(level, pos).and_then(|e| e.data(host)) else {
        return (0.0, false);
    };
    let theory = match d.get(&THEORY.into()) {
        Some(Tag::Float(bits)) => f32::from_bits(*bits),
        _ => speeds_of(&d)[0],
    };
    let over = matches!(d.get(&OVER.into()), Some(Tag::Byte(b)) if *b != 0);
    (theory, over)
}

/// The speeds the block entity at `pos` of `level` shows (on a client, what its update tag
/// brought).
pub fn shown_speeds(host: &dyn mod_api::Host, level: mod_api::LevelRef, pos: [i32; 3]) -> [f32; 2] {
    host.block_entity(level, pos)
        .and_then(|e| e.data(host))
        .map_or([0.0; 2], |d| speeds_of(&d))
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
        if with_world(|w| w.block_at(dim, pos)).flatten() == Some(Block::SequencedGearshift) {
            super::sequencer::loaded(&*host, dim, pos, entity);
        }
    }

    fn block_entity_update_tag(
        &self,
        event: &mut BlockEntityUpdateTagEvent<'_>,
    ) -> Option<Compound> {
        // The speeds, and what other subsystems keep in the same block entity for its clients.
        let shown: Compound = [SPEEDS, THEORY, OVER, BRACKET]
            .into_iter()
            .chain(
                super::SUBSYSTEMS
                    .iter()
                    .flat_map(|s| s.synced_keys().iter().copied()),
            )
            .filter_map(|k| Some((k.into(), event.entity.nbt.get(&k.into())?.clone())))
            .collect();
        (!shown.is_empty()).then_some(shown)
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
        // Parts another subsystem draws itself (a gauge's dials, a flywheel's wheel) are left
        // to it; a block another subsystem animates this frame is drawn again next frame, the
        // rest kept turning on the animation clock until the block entity changes.
        let systems = super::SUBSYSTEMS;
        for d in client::draws(block, &info, event.pos, speeds) {
            if !systems.iter().any(|s| s.draws_itself(block, &d)) {
                draw_turning(event.draw, block, &own, &d);
            }
        }
        // SpeedControllerRenderer: the bracket a block up, turned across the controller's axis.
        if block == Block::RotationSpeedController
            && matches!(event.entity.nbt.get(&BRACKET.into()), Some(Tag::Byte(1)))
            && let Some(part) = states::partial("part_speed_controller_bracket")
        {
            let draw = &mut *event.draw;
            draw.push();
            draw.translate([0.0, 1.0, 0.0]);
            if info.axis != Some(Axis::X) {
                draw.rotate_about([0.0, 1.0, 0.0], 90.0, [0.5; 3]);
            }
            draw.block(part, &[("facing", "up")]);
            draw.pop();
        }
        if !systems
            .iter()
            .any(|s| s.animated(block, event.pos, event.entity))
        {
            event.draw.keep();
        }
    }
}

/// The unit vector of `axis`.
pub fn axis_vector(axis: Axis) -> [f32; 3] {
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
) -> Option<(BlockKey, Option<(&'static str, &'static str)>)> {
    let part = |name: &str, facing: &'static str| {
        states::partial(name).map(|b| (b, Some(("facing", facing))))
    };
    match model {
        Model::Own => Some((states::key(block), None)),
        Model::Shaft(a) => Some((states::key(Block::Shaft), Some(("axis", axis_name(a))))),
        Model::ShaftHalf(d) => part("part_shaft_half", dir_name(d)),
        Model::CrankHandle(d) => part("part_hand_crank_handle", dir_name(d)),
        Model::CrankBase(d) => part("part_hand_crank_base", dir_name(d)),
        Model::Cog { large, axis } => part(
            if large {
                "part_large_cogwheel_shaftless"
            } else {
                "part_cogwheel_shaftless"
            },
            dir_name(Dir::from_axis(axis, true)),
        ),
        Model::CogShaft(a) => part("part_cogwheel_shaft", dir_name(Dir::from_axis(a, true))),
        Model::WaterWheel(d) => part("part_water_wheel", dir_name(d)),
        Model::MillstoneCog => part("part_millstone_cog", "up"),
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
        // Posed partials (belts, the mixer's pole and head) and the press head on its stroke
        // are drawn by their own systems.
        Model::Belt(..)
        | Model::BeltPulley(_)
        | Model::MixerPole
        | Model::MixerHead
        | Model::PressHead(_) => None,
    }
}

/// Draws one turning part of `block` (whose state has the properties `own`): its model at
/// rest, turning at its speed about its axis through the block's centre (`rpm * 0.3` degrees a
/// tick), from its starting angle.
pub fn draw_turning(
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
        Some(p) => draw.block(model, &[p]),
        None => draw.block(model, own),
    };
    draw.still();
}
