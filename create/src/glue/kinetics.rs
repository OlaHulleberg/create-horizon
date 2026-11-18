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
use create_engine::kinetics::{
    Axis, Context, Dir, Event, PartLocation, SavedKineticState, SavedNetwork,
};
use create_engine::world::keeps_state;
use mod_api::HostBlockStates;
use mod_api::{
    BlockEntityDrawContext, BlockEntityOnLoadEvent, BlockEntityPart, BlockEntitySubmitEvent,
    BlockEntityTypeKey, BlockEntityUpdateTagEvent, BlockKey, BlockPart, ClientHost, Compound,
    HostSide, Registrar, RegistrationContext, Tag,
};
use std::sync::OnceLock;

/// The type's id: every Create block the world keeps state for.
pub const TYPE: &str = "create:kinetic";

/// The key in a block entity's data under which its parts' speeds are, `[slot 0, slot 1]`.
pub const SPEEDS: &str = "Speeds";
/// The keys of its first part's theoretical speed (`getTheoreticalSpeed`, what the goggles
/// show) and of whether its network is overstressed (`isOverStressed`).
pub const THEORY: &str = "Theory";
pub const OVER: &str = "Over";
/// Whether a speed controller shows its bracket (`SpeedControllerBlockEntity.hasBracket`).
pub(crate) const BRACKET: &str = "Bracket";
/// A speed setting a moving structure carries (`Subsystem::carry_out`).
const CARRIED_SPEED: &str = "Speed";
/// Server-only KineticBlockEntity.write data, one compound per engine port.
const SAVED_KINETICS: &str = "KineticState";
const SOURCE_FACE: &str = "create:source_face";
/// In an update tag: the server is frozen (`/tick freeze`), so the block's parts are drawn at
/// rest (`KineticBlockEntity.getSpeed` is 0 while `tickRateManager().isFrozen()`). Sent from the
/// server's state as each tag is made, never saved.
const FROZEN: &str = "Frozen";
/// A count a freeze or thaw writes into each turning block entity, so its clients get a new
/// update tag (with or without [`FROZEN`]) and draw it again.
const FREEZES: &str = "Freezes";

thread_local! {
    /// The freeze each level's turning parts were last drawn for (`/tick freeze`), by level.
    static FROZEN_SHOWN: std::cell::RefCell<Vec<bool>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// How many freezes and thaws were shown ([`FREEZES`]).
    static FREEZE_COUNT: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
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

    /// A creative motor's or speed controller's speed (`ScrollValue`) rides with it.
    fn carry_out(&self, lv: &mut ServerLevel<'_>, pos: [i32; 3], _b: Block, out: &mut Compound) {
        if let Some(rpm) = with_world(|w| w.take_motor_setting(lv.dim, pos)).flatten() {
            out.insert(CARRIED_SPEED.into(), Tag::Int(rpm));
        }
    }

    fn carry_in(
        &self,
        lv: &mut ServerLevel<'_>,
        pos: [i32; 3],
        _b: Block,
        data: &Compound,
        _turn: create_engine::contraption::Transform,
    ) {
        if let Some(Tag::Int(rpm)) = data.get(&CARRIED_SPEED.into()) {
            let dim = lv.dim;
            with_world(|w| w.load_settings(dim, std::iter::once((pos, *rpm))));
        }
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

    /// Before the rest of the level tick: cranks and valves
    /// turning down, water wheels, split shafts following redstone, chain drives and sequenced
    /// gearshifts. The solve is `Solve`'s, later.
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        tick_inputs(lv);
    }
}

/// The kinetic solve, after every tick that may change what turns: `settle_belts`, `w.tick`,
/// its events.
pub struct Solve;

impl Subsystem for Solve {
    fn write_entities(&self, lv: &mut ServerLevel<'_>) {
        publish_saved(lv);
        let positions = with_world(|w| {
            w.dims[lv.dim]
                .sequencers
                .keys()
                .copied()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
        for pos in positions {
            super::sequencer::save_run(lv, pos);
        }
    }

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

/// Identifies this port's old persisted format, never a newly placed empty entity.
fn loading_legacy_save(data: &Compound) -> bool {
    !data.contains_key(&SAVED_KINETICS.into())
        && (data.contains_key(&SPEEDS.into()) || data.contains_key(&THEORY.into()))
}

/// FluidValveBlockEntity.write/read delegates Pointer to LerpedFloat's three float fields.
fn valve_pointer_to_nbt(pointer: create_engine::fluids::Pointer) -> Compound {
    Compound::from([
        ("Value".into(), Tag::Float(pointer.value.to_bits())),
        ("Target".into(), Tag::Float(pointer.target.to_bits())),
        ("Speed".into(), Tag::Float(pointer.speed.to_bits())),
    ])
}
fn valve_pointer_from_nbt(data: &Compound) -> create_engine::fluids::Pointer {
    let float = |key: &str| match data.get(&key.into()) {
        Some(Tag::Float(bits)) => f32::from_bits(*bits),
        _ => 0.0,
    };
    create_engine::fluids::Pointer {
        value: float("Value"),
        target: float("Target"),
        speed: float("Speed"),
    }
}

/// Java hand crank / valve use data, independent of the kinetic ports.
fn handle_to_nbt(h: create_engine::valve::SavedHandle) -> Compound {
    Compound::from([
        ("InUse".into(), Tag::Int(i32::from(h.in_use))),
        ("Backwards".into(), Tag::Byte(i8::from(h.backwards))),
        ("ScrollValue".into(), Tag::Int(h.angle)),
        (
            "TotalUseTicks".into(),
            Tag::Int(i32::from(h.total_use_ticks)),
        ),
        ("StartAngle".into(), Tag::Int(h.start_angle)),
        ("TargetAngle".into(), Tag::Int(h.target_angle)),
    ])
}
fn handle_from_nbt(data: &Compound) -> create_engine::valve::SavedHandle {
    let int = |key: &str| match data.get(&key.into()) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    };
    create_engine::valve::SavedHandle {
        in_use: int("InUse").clamp(0, 255) as u8,
        backwards: matches!(data.get(&"Backwards".into()), Some(Tag::Byte(v)) if *v != 0),
        angle: int("ScrollValue"),
        total_use_ticks: int("TotalUseTicks").clamp(0, 255) as u8,
        start_angle: int("StartAngle"),
        target_angle: int("TargetAngle"),
    }
}

/// KineticBlockEntity.write's server fields.
fn saved_to_nbt(saved: SavedKineticState) -> Compound {
    let mut out = Compound::from([
        ("Speed".into(), Tag::Float(saved.speed.to_bits())),
        (
            "NeedsSpeedUpdate".into(),
            Tag::Byte(i8::from(saved.needs_speed_update)),
        ),
    ]);
    if let Some(source) = saved.source {
        out.insert("Source".into(), Tag::IntArray(source.pos.to_vec()));
        out.insert("SourcePort".into(), Tag::Byte(source.sub as i8));
    }
    if let Some(n) = saved.network {
        out.insert(
            "Network".into(),
            Tag::Compound(Compound::from([
                ("Id".into(), Tag::Long(n.id)),
                ("Stress".into(), Tag::Float(n.stress.to_bits())),
                ("Capacity".into(), Tag::Float(n.capacity.to_bits())),
                ("Size".into(), Tag::Int(n.size as i32)),
                ("AddedStress".into(), Tag::Float(n.added_stress.to_bits())),
                (
                    "AddedCapacity".into(),
                    Tag::Float(n.added_capacity.to_bits()),
                ),
            ])),
        );
    }
    if let Some((context, relative)) = saved.sequence {
        out.insert(
            "Sequence".into(),
            Tag::Compound(Compound::from([
                ("Distance".into(), Tag::Byte(i8::from(context.distance))),
                ("Value".into(), Tag::Float(context.value.to_bits())),
                ("Run".into(), Tag::Long(context.run as i64)),
                ("RelativeValue".into(), Tag::Float(relative.to_bits())),
            ])),
        );
    }
    out
}
/// KineticBlockEntity.read; no saved data means a freshly placed entity or an older save.
fn saved_from_nbt(data: &Compound) -> Option<SavedKineticState> {
    fn float(c: &Compound, k: &str) -> f32 {
        match c.get(&k.into()) {
            Some(Tag::Float(v)) => f32::from_bits(*v),
            _ => 0.0,
        }
    }
    fn int(c: &Compound, k: &str) -> i32 {
        match c.get(&k.into()) {
            Some(Tag::Int(v)) => *v,
            _ => 0,
        }
    }
    let Some(Tag::Float(speed)) = data.get(&"Speed".into()) else {
        return None;
    };
    let source = match data.get(&"Source".into()) {
        Some(Tag::IntArray(p)) if p.len() == 3 => Some(PartLocation {
            pos: [p[0], p[1], p[2]],
            sub: match data.get(&"SourcePort".into()) {
                Some(Tag::Byte(1)) => 1,
                _ => 0,
            },
        }),
        _ => None,
    };
    let network = match data.get(&"Network".into()) {
        Some(Tag::Compound(n)) => match n.get(&"Id".into()) {
            Some(Tag::Long(id)) => Some(SavedNetwork {
                id: *id,
                stress: float(n, "Stress"),
                capacity: float(n, "Capacity"),
                size: int(n, "Size").max(0) as u32,
                added_stress: float(n, "AddedStress"),
                added_capacity: float(n, "AddedCapacity"),
            }),
            _ => None,
        },
        _ => None,
    };
    let sequence = match data.get(&"Sequence".into()) {
        Some(Tag::Compound(c)) => Some((
            Context {
                distance: matches!(c.get(&"Distance".into()), Some(Tag::Byte(1))),
                value: float(c, "Value"),
                run: match c.get(&"Run".into()) {
                    Some(Tag::Long(v)) => *v as u32,
                    _ => 0,
                },
            },
            float(c, "RelativeValue"),
        )),
        _ => None,
    };
    Some(SavedKineticState {
        speed: f32::from_bits(*speed),
        source,
        network,
        sequence,
        needs_speed_update: matches!(data.get(&"NeedsSpeedUpdate".into()), Some(Tag::Byte(1))),
    })
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
        super::advancements::water_wheel(lv, pos, samples.iter().map(|s| s.0), score != 0);
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
            with_world(|w| w.transmission_power_changing(dim, pos));
            let value = if now { "true" } else { "false" };
            if with_world(|w| w.reads_signal(state)) == Some(true) {
                if let Some(next) = lv
                    .block_state(pos)
                    .and_then(|s| s.with(&lv.host.block_states(), "powered", value).ok())
                {
                    with_world(|w| {
                        w.setting_known_shape(dim, pos, super::states::id_or_none(next))
                    });
                    lv.set_state(
                        pos,
                        next,
                        super::server::UPDATE_CLIENTS | mod_api::BlockUpdateFlags::KNOWN_SHAPE,
                    );
                }
            } else {
                lv.set_property(pos, "powered", value, super::server::UPDATE_CLIENTS);
            }
        }
        // `ChainGearshiftBlockEntity.neighbourChanged`: the signal's strength sets its chain.
        if with_world(|w| w.reads_signal(state)) == Some(true) {
            let signal = lv.best_neighbor_signal(pos);
            with_world(|w| w.set_chain_signal(dim, pos, signal));
        }
    }
    // Encased chain drives placed or beside a change take their place in their chain
    // (`ChainDriveBlock.updateShape`).
    apply_chain_shapes(lv);
    tick_sequencers(lv);
    tick_brackets(lv);
}

/// ChainDriveBlock.updateShape runs as part of the block change, including kinetic breaks.
fn apply_chain_shapes(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    for (pos, drive) in with_world(|w| w.take_chain_shapes(dim)).unwrap_or_default() {
        let along = if drive.along_first { "true" } else { "false" };
        lv.set_property(pos, "axis_along_first", along, super::server::update_all());
        lv.set_property(pos, "part", drive.part.name(), super::server::update_all());
    }
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

/// Apply synchronous propagation effects before any interaction or tick publication.
pub(crate) fn drain_effects(lv: &mut ServerLevel<'_>) {
    apply_events(lv, Vec::new());
}

/// A level's solve: the network solved, the blocks Create breaks broken, and every changed
/// part's speed put in its block entity's data for its clients.
fn tick_solve(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut events = Vec::new();
    let Some((steps, removed, unsaved)) = with_world(|w| {
        w.settle_belts(dim);
        let (_, steps) = w.tick(dim, &mut events);
        (
            steps,
            w.dims[dim].kinetics.take_removed(),
            w.take_unsaved(dim),
        )
    }) else {
        return;
    };
    apply_sequencer_steps(lv, steps);
    for (cx, cz) in unsaved {
        persist::mark_unsaved(dim, [cx, cz]);
    }
    for pos in removed {
        set_speeds(lv, pos, None, Some((0.0, false)));
    }
    apply_events(lv, events);
}

/// World callbacks can enqueue more breaks; finish their chain shapes before returning.
fn apply_events(lv: &mut ServerLevel<'_>, mut events: Vec<Event>) {
    loop {
        for e in events {
            match e {
                // Create's `world.destroyBlock(pos, true)`: the block breaks with its drops.
                Event::Break { pos } => {
                    lv.destroy_block(pos, true);
                    apply_chain_shapes(lv);
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
        events = with_world(|w| w.dims[lv.dim].kinetics.take_pending_events()).unwrap_or_default();
        if events.is_empty() {
            break;
        }
    }
}

/// Publish the engine saved-change journals without advancing simulation.
fn publish_saved(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    with_world(|w| w.flush_kinetic_callbacks(dim));
    let handles = with_world(|w| w.take_handle_changes(dim)).unwrap_or_default();
    for pos in handles {
        if let Some(saved) = with_world(|w| w.saved_handle(dim, pos)).flatten() {
            lv.write_entity_field(pos, "HandleState", handle_to_nbt(saved));
        }
    }
    let changed = with_world(|w| w.dims[dim].kinetics.take_saved_changes()).unwrap_or_default();
    for pos in changed {
        if let Some(pointer) = with_world(|w| w.saved_valve_pointer(dim, pos)).flatten() {
            lv.write_entity_field(pos, "Pointer", valve_pointer_to_nbt(pointer));
        }
        let Some(states) = with_world(|w| {
            (!w.dims[dim].kinetics.awaiting_legacy_load(pos)).then(|| {
                [
                    w.dims[dim].kinetics.saved_state(pos, 0),
                    w.dims[dim].kinetics.saved_state(pos, 1),
                ]
            })
        })
        .flatten() else {
            continue;
        };
        let ports: Compound = states
            .into_iter()
            .enumerate()
            .filter_map(|(i, state)| {
                state.map(|s| (["0", "1"][i].into(), Tag::Compound(saved_to_nbt(s))))
            })
            .collect();
        // KineticBlockEntity.write: a `Source` or `Network` the entity no longer has is gone.
        lv.write_entity_field(pos, SAVED_KINETICS, ports);
        if let Some(signal) = with_world(|w| w.saved_chain_signal(dim, pos)).flatten()
            && let Some(entity) = lv.host.block_entity(lv.level, pos)
        {
            let data = Compound::from([("Signal".into(), Tag::Int(signal))]);
            let _ = entity.merge_data(&mut *lv.host, data);
        }
    }
}

/// Sequenced gearshifts: those whose neighbour changed look at their power
/// (`SequencedGearshiftBlock.tick`). Instruction ticks run after the base kinetic tick in
/// `CreateWorld::tick`.
fn tick_sequencers(lv: &mut ServerLevel<'_>) {
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
    apply_sequencer_steps(lv, steps);
}

/// Write the state requested by the engine after the corresponding kinetic phase.
pub(super) fn apply_sequencer_steps(
    lv: &mut ServerLevel<'_>,
    steps: Vec<([i32; 3], create_engine::sequencer::Step)>,
) {
    use create_engine::sequencer::Step;
    for (pos, step) in steps {
        super::sequencer::save_run(lv, pos);
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

/// The speeds in a block entity's data; on a client, 0 while the server is frozen
/// (`KineticBlockEntity.getSpeed`), so its parts stand at their resting angle.
pub fn speeds_of(data: &Compound) -> [f32; 2] {
    if synced(data, FROZEN).is_some() {
        return [0.0; 2];
    }
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
    let source = with_world(|w| w.dims[lv.dim].kinetics.source_facing(pos)).flatten();
    data.insert(
        SOURCE_FACE.into(),
        Tag::Int(source.map_or(-1, |d| d as i32)),
    );
    let _ = entity.merge_data(&mut *lv.host, data);
}

/// `/tick freeze` and its end in level `lv`: every block entity with a turning part gets a new
/// update tag, so its clients draw its parts at rest while frozen (`getSpeed` 0) and turning
/// again after. Runs each level tick, frozen or not; nothing happens without a change.
pub fn show_freeze(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let frozen = !super::world::running();
    let changed = FROZEN_SHOWN.with_borrow_mut(|f| {
        if f.len() <= dim {
            f.resize(dim + 1, false);
        }
        std::mem::replace(&mut f[dim], frozen) != frozen
    });
    if !changed {
        return;
    }
    let mut turning: Vec<[i32; 3]> = with_world(|w| {
        w.dims[dim]
            .kinetics
            .parts()
            .filter(|p| p.2 != 0.0)
            .map(|p| p.0)
            .collect()
    })
    .unwrap_or_default();
    turning.sort_unstable();
    turning.dedup();
    let count = FREEZE_COUNT.get().wrapping_add(1);
    FREEZE_COUNT.set(count);
    for pos in turning {
        if let Some(entity) = lv.host.block_entity(lv.level, pos) {
            let data = Compound::from([(FREEZES.into(), Tag::Int(count))]);
            let _ = entity.merge_data(&mut *lv.host, data);
        }
    }
}

/// The world closed: the next one starts unfrozen.
pub fn forget_freeze() {
    FROZEN_SHOWN.with_borrow_mut(Vec::clear);
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
        let data = entity.data(&*host).unwrap_or_default();
        let saved = match data.get(&SAVED_KINETICS.into()) {
            Some(Tag::Compound(c)) => Some(c.clone()),
            _ => None,
        };
        // Previous builds persisted drawing state but no kinetic snapshot. A brand-new
        // player's block entity has neither: it must retain Create's placement lifecycle.
        let legacy = loading_legacy_save(&data);
        with_world(|w| {
            w.block_loaded(dim, pos, state);
            let signal = match data.get(&"Signal".into()) {
                Some(Tag::Int(signal)) => *signal,
                _ => 0,
            };
            w.restore_chain_signal(dim, pos, signal);
            // BeltBlockEntity.read restores Controller/Index/Length before initialize.
            // Register the saved kinetic part before asking restore_state to read it.
            if let Some(Tag::Compound(belt)) = data.get(&"BeltState".into())
                && let (Some(Tag::IntArray(c)), Some(Tag::Int(index)), Some(Tag::Int(length))) = (
                    belt.get(&"Controller".into()),
                    belt.get(&"Index".into()),
                    belt.get(&"Length".into()),
                )
                && let [x, y, z] = c.as_slice()
            {
                w.restore_belt_segment(
                    dim,
                    pos,
                    create_engine::belt::SavedSegment {
                        controller: [*x, *y, *z],
                        index: *index,
                        length: *length,
                    },
                );
            }
            if let Some(Tag::Compound(depot)) = data.get(&"DepotState".into()) {
                w.restore_depot(dim, pos, super::logistics::depot::saved_from_nbt(depot));
            }
            if let Some(Tag::Compound(handle)) = data.get(&"HandleState".into()) {
                w.restore_handle(dim, pos, handle_from_nbt(handle));
            }
            if let Some(Tag::Compound(pointer)) = data.get(&"Pointer".into()) {
                w.restore_valve_pointer(dim, pos, valve_pointer_from_nbt(pointer));
            }
            if let Some(ports) = saved {
                for sub in 0..2 {
                    if let Some(Tag::Compound(data)) = ports.get(&["0", "1"][sub as usize].into())
                        && let Some(saved) = saved_from_nbt(data)
                    {
                        w.dims[dim].kinetics.restore_state(pos, sub, saved);
                    }
                }
            } else if legacy {
                for sub in 0..2 {
                    w.dims[dim]
                        .kinetics
                        .loading_save_without_kinetic_state(pos, sub);
                }
            }
        });
        if with_world(|w| w.block_at(dim, pos)).flatten() == Some(Block::SequencedGearshift) {
            super::sequencer::loaded(&*host, dim, pos, entity);
        }
    }

    fn block_entity_update_tag(
        &self,
        event: &mut BlockEntityUpdateTagEvent<'_>,
    ) -> Option<Compound> {
        // The speeds, and what other subsystems keep in the same block entity for its clients:
        // every synced key, `absent()` for one the block entity doesn't hold (`synced`).
        let mut shown: Compound = [SPEEDS, THEORY, OVER, BRACKET, SAVED_KINETICS, SOURCE_FACE]
            .into_iter()
            .chain(
                super::SUBSYSTEMS
                    .iter()
                    .flat_map(|s| s.synced_keys().iter().copied()),
            )
            .map(|k| {
                let value = event.entity.nbt.get(&k.into()).cloned();
                (k.into(), value.unwrap_or_else(absent))
            })
            .collect();
        // `/tick freeze`: drawn at rest while the server is frozen.
        let frozen = if super::world::running() {
            absent()
        } else {
            Tag::Byte(1)
        };
        shown.insert(FROZEN.into(), frozen);
        // How often a player looked at its value box: a new number makes clients draw it again
        // (`interaction::value_text`).
        if let Some(n) = super::interaction::values::looks(event.pos) {
            shown.insert(super::interaction::values::LOOKS.into(), Tag::Int(n));
        }
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
        let source = match event.entity.nbt.get(&SOURCE_FACE.into()) {
            Some(Tag::Int(n)) => create_engine::kinetics::Dir::ALL.get(*n as usize).copied(),
            _ => None,
        };
        for d in client::draws_from_source(block, &info, event.pos, speeds, source) {
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
        super::interaction::value_text::draw(host, event.pos, event.draw);
        if !systems
            .iter()
            .any(|s| s.animated(block, event.pos, event.entity))
        {
            event.draw.keep();
        }
    }
}

/// What an update tag carries for a synced key (`Subsystem::synced_keys`) the block entity
/// doesn't hold: a client merges each update tag into its copy (`block_entity_update_tag`), so a
/// key left out would keep its last value there. No synced key holds an empty long array.
pub fn absent() -> Tag {
    Tag::LongArray(Vec::new())
}

/// Synced key `key` of a block entity's data as its clients should read it: None when the server
/// took it away (`absent`).
pub fn synced<'a>(nbt: &'a Compound, key: &str) -> Option<&'a Tag> {
    nbt.get(&key.into())
        .filter(|t| !matches!(t, Tag::LongArray(v) if v.is_empty()))
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
        Model::RopeCoil(d) => part("part_rope_coil", dir_name(d)),
        Model::ElevatorCoil(d) => part("part_elevator_coil", dir_name(d)),
        Model::BacktankShaft { netherite } => part(
            if netherite {
                "part_netherite_backtank_shaft"
            } else {
                "part_copper_backtank_shaft"
            },
            "up",
        ),
        // Posed partials (belts, the mixer's pole and head) and the press head on its stroke
        // are drawn by their own systems.
        Model::Belt(..)
        | Model::BeltPulley(_)
        | Model::MixerPole
        | Model::MixerHead
        | Model::PressHead(_) => None,
    }
}

/// The model of part `model` of `block` and the property it is drawn with (None: `block`'s
/// own properties); None for parts drawn by their own systems.
pub fn part_model(
    block: Block,
    model: Model,
) -> Option<(BlockKey, Option<(&'static str, &'static str)>)> {
    model_block(block, model)
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

#[cfg(test)]
mod saved_state_tests {
    #[test]
    fn valve_pointer_nbt_round_trip_preserves_all_fields() {
        let pointer = create_engine::fluids::Pointer {
            value: 0.3,
            target: 1.0,
            speed: 0.1,
        };
        assert_eq!(
            super::valve_pointer_from_nbt(&super::valve_pointer_to_nbt(pointer)),
            pointer
        );
    }

    #[test]
    fn handle_use_nbt_roundtrip() {
        let saved = create_engine::valve::SavedHandle {
            in_use: 6,
            backwards: true,
            angle: -45,
            total_use_ticks: 7,
            start_angle: 0,
            target_angle: -90,
        };
        assert_eq!(super::handle_from_nbt(&super::handle_to_nbt(saved)), saved);
    }

    use super::*;
    #[test]
    fn only_old_persisted_data_requests_legacy_migration() {
        assert!(!loading_legacy_save(&Compound::new()));
        let mut old = Compound::from([(
            SPEEDS.into(),
            Tag::List(vec![Tag::Float(16.0f32.to_bits()), Tag::Float(0)]),
        )]);
        assert!(loading_legacy_save(&old));
        old.insert(SAVED_KINETICS.into(), Tag::Compound(Compound::new()));
        assert!(!loading_legacy_save(&old));
    }
    #[test]
    fn kinetic_block_entity_data_round_trips_all_server_fields() {
        let saved = SavedKineticState {
            speed: -32.0,
            sequence: Some((
                Context {
                    distance: true,
                    value: 90.0,
                    run: 7,
                },
                2.5,
            )),
            source: Some(PartLocation {
                pos: [-17, 64, 18],
                sub: 1,
            }),
            network: Some(SavedNetwork {
                id: create_engine::kinetics::network_id([-17, 64, 18]),
                stress: 256.0,
                capacity: 2048.0,
                size: 3,
                added_stress: 8.0,
                added_capacity: 64.0,
            }),
            needs_speed_update: false,
        };
        assert_eq!(saved_from_nbt(&saved_to_nbt(saved)), Some(saved));
        let stopped = SavedKineticState {
            source: None,
            network: None,
            sequence: None,
            speed: 0.0,
            needs_speed_update: true,
        };
        assert_eq!(saved_from_nbt(&saved_to_nbt(stopped)), Some(stopped));
        assert_eq!(saved_from_nbt(&Compound::new()), None);
    }
}
