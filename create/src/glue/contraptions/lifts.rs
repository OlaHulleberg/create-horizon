//! Clockwork bearings, rope pulleys and elevators (`lifts.rs` in the engine): their structures
//! leave the world while they run and come back where they stop; clients draw each from its
//! block's data (`draw`), told when it starts, stops or changes speed. Redstone and elevator
//! contacts: their signals, placement and lights; an elevator contact's floor names (made up as
//! Create's `initNames` does, edited on its screen), door setting and the floor its column's
//! elevator last passed or reached (`broadcastFloorData`). Saved under `create_lifts` and
//! `create_contacts`.
//!
//! An elevator's sliding doors open at the floor it is called to on the sides that floor's
//! contact's door control lets open (`SlidingDoorMovementBehaviour`, `DoorControlBehaviour`),
//! and follow a door control changed while it stands there.
//!
//! A redstone contact placed or wrenched to face an elevator column is that column's elevator
//! contact, and an elevator contact wrenched away from it (or up or down) a redstone contact
//! again (`RedstoneContactItem.getPlacementState`, `onWrenched`).
//!
//! An elevator's contraption controls show its column's floors (`FLOORS`, sent as
//! `ElevatorContraption.tickStorage` sends them) and call the floor a click picks
//! (`elevator_controls_clicked`).

use super::super::persist;
use super::super::server::{ServerLevel, UPDATE_CLIENTS, update_all};
use super::super::states::{self, key};
use super::super::world::with_world;
use super::{
    actors, add, clicker, create_info, dir_name, draw, found, in_click_level, state_of_text,
    state_text, state_with, take_blocks,
};
use create_engine::Block;
use create_engine::actors::Carried;
use create_engine::kinetics::{Axis, Dir};
use create_engine::lifts::Held;
use mod_api::{
    BlockPart, BlockSignalEvent, BlockState, Compound, InteractionResult, IsSignalSourceEvent,
    LevelHost, Registrar, ServerHost, StateForPlacementEvent, Tag, UseItemOnEvent,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// An elevator column's floors.
type Floors = Vec<create_engine::elevator::Floor>;

/// A clock's hands' degrees a tick.
type HandSpeeds = (f32, f32);

/// A saved structure, and its elevator column's floor call.
type Saved = (
    [i32; 3],
    Held,
    Option<(create_engine::elevator::ColumnKey, i32)>,
);

thread_local! {
    /// Elevators that moved since they were put together (`firstMovement` is over).
    static ELEVATOR_MOVED: RefCell<std::collections::HashSet<(usize, [i32; 3])>> =
        RefCell::new(std::collections::HashSet::new());
    /// What running rope pulleys, elevators and clocks carry besides their blocks
    /// (`Contraption.actors`, glue, controls; storage is kept apart), by dimension and pulley or
    /// bearing; only those whose structure has any.
    static PULLEY_CARRIED: RefCell<HashMap<(usize, [i32; 3]), Carried>> =
        RefCell::new(HashMap::new());
    /// The degrees a tick each running clock's hands were last shown turning at.
    static CLOCK_SHOWN: RefCell<HashMap<(usize, [i32; 3]), HandSpeeds>> =
        RefCell::new(HashMap::new());
    /// The height in its column each running elevator's contact last passed
    /// (`ContactMovementBehaviour.visitNewPosition`).
    static VISITED: RefCell<HashMap<(usize, [i32; 3]), i32>> = RefCell::new(HashMap::new());
    /// The floors each running elevator's clients were last told of (`ElevatorFloorListPacket`).
    static FLOORS_SENT: RefCell<HashMap<(usize, [i32; 3]), Floors>> =
        RefCell::new(HashMap::new());
}

/// The key in an elevator pulley's data under which its column's floors are, for its
/// contraption controls (`ElevatorContraption.namesList`): each floor's height, short and long
/// name.
pub const FLOORS: &str = "Floors";

pub(super) fn forget() {
    ELEVATOR_MOVED.with_borrow_mut(std::collections::HashSet::clear);
    PULLEY_CARRIED.with_borrow_mut(HashMap::clear);
    CLOCK_SHOWN.with_borrow_mut(HashMap::clear);
    VISITED.with_borrow_mut(HashMap::clear);
    FLOORS_SENT.with_borrow_mut(HashMap::clear);
}

pub(super) fn register(r: &mut dyn Registrar) {
    for b in [
        Block::ClockworkBearing,
        Block::RopePulley,
        Block::ElevatorPulley,
    ] {
        r.block_part(key(b), LiftPart(b));
    }
    r.block_part(key(Block::RedstoneContact), ContactPart);
    r.block_part(key(Block::ElevatorContact), ContactPart);
}

/// `DimensionType.natural`: the overworld's and other dimensions' clocks, not the Nether's and
/// the End's (they run 24 times as fast).
fn natural(dim: usize) -> bool {
    dim == 0
}

fn below(p: [i32; 3], n: i32) -> [i32; 3] {
    [p[0], p[1] - n, p[2]]
}

fn speed(dim: usize, pos: [i32; 3]) -> f32 {
    with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0)
}

/// Blocks a structure took go back: each where it was, or dropped where something else took
/// its place (`Contraption.addBlocksToWorld`).
fn lift_put_back(
    lv: &mut ServerLevel<'_>,
    key: [i32; 3],
    origin: [i32; 3],
    blocks: &[([i32; 3], u32)],
) {
    actors::expect_back(lv, key, |o| add(origin, o));
    super::put_back_blocks(lv, blocks, |lv, p| lv.passable(p));
}

fn mark_unsaved(lv: &ServerLevel<'_>, pos: [i32; 3]) {
    persist::mark_pos_unsaved(lv.dim, pos);
}

impl ServerLevel<'_> {
    /// The level's lowest `y` and height.
    fn build_range(&self) -> (i32, i32) {
        self.level.height(&*self.host).unwrap_or((-64, 384))
    }
}

/// Clockwork bearings, rope pulleys, elevators and contacts, after the kinetic solve. Nothing
/// runs while none of them has anything to do.
pub(super) fn tick_lifts(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    // `ElevatorContraption.tickStorage`: every 10 ticks each elevator's clients hear of its
    // column's floors when they changed.
    if lv.game_time() % 10 == 0 {
        let elevators: Vec<[i32; 3]> =
            with_world(|w| w.dims[dim].lifts.elevators.keys().copied().collect())
                .unwrap_or_default();
        for pos in elevators {
            send_floors(lv, pos);
        }
    }
    let busy = with_world(|w| {
        let l = &w.dims[dim].lifts;
        !(l.orphans.is_empty()
            && l.clock_checks.is_empty()
            && l.clocks.is_empty()
            && l.pulley_checks.is_empty()
            && l.pulleys.is_empty()
            && l.elevator_clicks.is_empty()
            && l.travelling.is_empty()
            && l.contact_checks.is_empty()
            && l.contact_ticks.is_empty()
            && l.changes.is_empty()
            && l.names_due.is_empty()
            && l.contacts_changed.is_empty())
    });
    if busy != Some(true) {
        return;
    }
    let orphans =
        with_world(|w| std::mem::take(&mut w.dims[dim].lifts.orphans)).unwrap_or_default();
    for (pos, held) in orphans {
        let origin = match &held {
            Held::Pulley(p) => {
                Some(p.placed_at(create_engine::pulley::grid_offset(p.offset, p.range)))
            }
            Held::Elevator(e) => Some(e.placed_at()),
            // A clock's hands go back turned to 0, where they started.
            Held::Clock(_) => Some(pos),
        };
        lift_put_back(lv, pos, origin.unwrap_or(pos), &held.put_back(pos));
        // What rode with it (its storage too, kept apart from its actors) goes back.
        if let Some(origin) = origin {
            let mut carried = PULLEY_CARRIED
                .with_borrow_mut(|c| c.remove(&(dim, pos)))
                .unwrap_or_default();
            actors::release(lv, pos, origin, &mut carried);
        }
        FLOORS_SENT.with_borrow_mut(|m| m.remove(&(dim, pos)));
        draw::hide(lv, pos);
        mark_unsaved(lv, pos);
    }
    tick_clocks(lv);
    tick_pulleys(lv);
    tick_elevators(lv);
    tick_contacts(lv);
    contact_names(lv);
    let changes = with_world(|w| {
        let mut v = std::mem::take(&mut w.dims[dim].lifts.changes);
        v.sort();
        v.dedup();
        v
    })
    .unwrap_or_default();
    for pos in changes {
        show(lv, pos);
    }
}

/// A clock, pulley or elevator at `pos` to its clients as it is now, or gone.
fn show(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let running = with_world(|w| {
        let l = &w.dims[dim].lifts;
        (
            l.clocks.contains_key(&pos),
            l.pulleys.contains_key(&pos),
            l.elevators.contains_key(&pos),
        )
    })
    .unwrap_or_default();
    match running {
        (true, ..) => show_clock(lv, pos),
        (_, true, _) => show_pulley(lv, pos),
        (.., true) => show_elevator(lv, pos),
        _ => draw::hide(lv, pos),
    }
}

/// The sky a clock reads now.
fn sky(lv: &ServerLevel<'_>, rpm: f32, facing: Dir) -> create_engine::clock::Sky {
    create_engine::clock::Sky {
        rpm,
        day_time: lv.level.day_time(&*lv.host).unwrap_or(0),
        natural: natural(lv.dim),
        positive: facing.positive(),
    }
}

fn show_clock(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(c) = with_world(|w| w.dims[dim].lifts.clocks.get(&pos).cloned()).flatten() else {
        return;
    };
    let speeds = c.clock.speeds(&sky(lv, c.rpm, c.facing));
    CLOCK_SHOWN.with_borrow_mut(|s| s.insert((dim, pos), speeds));
    draw::show_clock(
        lv,
        pos,
        draw::Hands {
            hour: &c.hour,
            minute: c.minute.as_ref().map_or(&[][..], |(_, b)| b),
            axis: c.facing.axis(),
            angles: (c.clock.hour, c.clock.minute),
            speeds,
        },
    );
}

/// `ClockworkBearingBlockEntity.tick`: a bearing whose speed changed or that was clicked
/// (`assembleNextTick`) assembles if it isn't running, comes apart if it is and stopped; running
/// clocks turn their hands toward the day time, their clients told when a hand's speed changes.
fn tick_clocks(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].lifts.clock_checks)).unwrap_or_default();
    checks.sort();
    checks.dedup();
    for pos in checks {
        let Some((Block::ClockworkBearing, info)) = create_info(lv.block(pos)) else {
            continue;
        };
        let Some(facing) = info.facing else {
            continue;
        };
        let rpm = speed(dim, pos);
        let running = with_world(|w| w.dims[dim].lifts.clocks.contains_key(&pos)) == Some(true);
        if running {
            if rpm == 0.0 {
                disassemble_clock(lv, pos);
            } else {
                with_world(|w| {
                    if let Some(c) = w.dims[dim].lifts.clocks.get_mut(&pos) {
                        c.rpm = rpm;
                    }
                });
                show_clock(lv, pos);
            }
        } else {
            assemble_clock(lv, pos, facing, rpm);
        }
    }
    let running: Vec<([i32; 3], f32, Dir)> = with_world(|w| {
        w.dims[dim]
            .lifts
            .clocks
            .iter()
            .map(|(p, c)| (*p, c.rpm, c.facing))
            .collect()
    })
    .unwrap_or_default();
    for (pos, rpm, facing) in running {
        if rpm == 0.0 {
            continue;
        }
        let sky = sky(lv, rpm, facing);
        let speeds = with_world(|w| {
            let c = w.dims[dim].lifts.clocks.get_mut(&pos)?;
            let speeds = c.clock.speeds(&sky);
            c.clock.tick(&sky);
            Some(speeds)
        })
        .flatten();
        let shown = CLOCK_SHOWN.with_borrow(|s| s.get(&(dim, pos)).copied());
        if let Some(speeds) = speeds
            && shown != Some(speeds)
        {
            show_clock(lv, pos);
        }
    }
}

/// The clock's hand arrangement picked on its value box (`operationMode`): a running clock's
/// hands turn toward their new targets from the next tick.
pub(super) fn clock_hands_set(lv: &mut ServerLevel<'_>, pos: [i32; 3], index: u8) {
    let dim = lv.dim;
    let hands = create_engine::clock::Hands::from_index(index);
    let running = with_world(|w| {
        let c = w.dims[dim].lifts.clocks.get_mut(&pos)?;
        c.clock.hands = hands;
        Some(())
    })
    .flatten();
    if running.is_some() {
        show_clock(lv, pos);
        mark_unsaved(lv, pos);
    }
}

/// `ClockworkBearingBlockEntity.assemble`: the hour hand and the minute hand leave the world.
fn assemble_clock(lv: &mut ServerLevel<'_>, pos: [i32; 3], facing: Dir, rpm: f32) {
    let dim = lv.dim;
    let hands = match create_engine::clock::gather(pos, facing, |p| found(lv, p)) {
        Ok(Some(hands)) => hands,
        Ok(None) => return,
        Err(r) => {
            super::set_exception(lv, pos, Some(&r));
            return;
        }
    };
    super::set_exception(lv, pos, None);
    let mut all = hands.hour.clone();
    if let Some((_, m)) = &hands.minute {
        all.extend(m.iter().copied());
    }
    actors::carry_storage(lv, pos, pos, &all);
    let extras = actors::carry_extras(lv, pos, &all, |p| p == pos);
    if !extras.controls.is_empty() || !extras.glue.is_empty() {
        PULLEY_CARRIED.with_borrow_mut(|c| c.insert((dim, pos), extras));
    }
    crate::glue::advancements::clock_assembled(
        lv,
        pos,
        &hands.hour,
        hands.minute.as_ref().map(|m| &m.1[..]),
    );
    take_blocks(lv, pos, &hands.hour);
    if let Some((_, m)) = &hands.minute {
        take_blocks(lv, pos, m);
    }
    let arrangement = create_engine::clock::Hands::from_index(super::option_at(dim, pos) as u8);
    let c = create_engine::lifts::RunningClock {
        clock: create_engine::clock::Clock {
            hands: arrangement,
            ..Default::default()
        },
        facing,
        rpm,
        hour: hands.hour,
        minute: hands.minute,
    };
    with_world(|w| {
        w.dims[dim].lifts.clocks.insert(pos, c);
    });
    show_clock(lv, pos);
    mark_unsaved(lv, pos);
}

/// `ClockworkBearingBlockEntity.disassemble`: the hands turn back to 0 and go back.
fn disassemble_clock(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(c) = with_world(|w| w.dims[dim].lifts.clocks.remove(&pos)).flatten() else {
        return;
    };
    CLOCK_SHOWN.with_borrow_mut(|s| s.remove(&(dim, pos)));
    let held = Held::Clock(c);
    lift_put_back(lv, pos, pos, &held.put_back(pos));
    actors::release_storage(lv, pos, |o| add(pos, o));
    // The hands turned back to 0: their glue and controls go back where they were.
    if let Some(mut carried) = PULLEY_CARRIED.with_borrow_mut(|c| c.remove(&(dim, pos))) {
        actors::release_extras(lv, &mut carried, |o| add(pos, o));
    }
    draw::hide(lv, pos);
    mark_unsaved(lv, pos);
}

/// For a pulley not yet started: whether its block at `local` (offsets from `anchor`) is a
/// breaker that can break the block at `cell`, as `breaker_clears` once it runs.
fn breaker_below(
    lv: &ServerLevel<'_>,
    blocks: &[([i32; 3], u32)],
    anchor: [i32; 3],
    local: [i32; 3],
    cell: [i32; 3],
) -> bool {
    use create_engine::actors::{Actor, Kind};
    let Some((_, state)) = blocks.iter().find(|(o, _)| *o == local) else {
        return false;
    };
    let Some((block, info)) = create_info(*state) else {
        return false;
    };
    let (Some(kind), Some(facing)) = (Kind::of(block), info.facing) else {
        return false;
    };
    let one = Carried {
        actors: vec![Actor::new(local, kind, facing, 0)],
        ..Carried::default()
    };
    actors::breaker_clears(lv, anchor, &one, local, cell)
}

/// Where offset 0 of a pulley's structure is now: its anchor, `offset - initial` lower.
fn pulley_origin(p: &create_engine::pulley::Pulley, offset: f32) -> [f64; 3] {
    [
        f64::from(p.anchor[0]),
        f64::from(p.anchor[1]) - f64::from(offset - p.initial as f32),
        f64::from(p.anchor[2]),
    ]
}

/// `LinearActuatorBlockEntity.tick` for rope pulleys: speed changes start, stop or turn them
/// (`assembleNextTick`); running ones move and stop at their ends, against the world, or (an
/// empty rope) at the block they reach.
fn tick_pulleys(lv: &mut ServerLevel<'_>) {
    use create_engine::pulley::{Step, grid_offset};
    let dim = lv.dim;
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].lifts.pulley_checks)).unwrap_or_default();
    checks.sort();
    checks.dedup();
    let mut later = Vec::new();
    for &pos in &checks {
        if !matches!(create_info(lv.block(pos)), Some((Block::RopePulley, _))) {
            continue;
        }
        // Loaded moving, it goes on once its network has a speed.
        if super::unsolved(dim, pos) {
            later.push(pos);
            continue;
        }
        let rpm = speed(dim, pos);
        let running = with_world(|w| {
            let limit = w.dims[dim].kinetics.sequence_limit(pos, 0, true);
            let p = w.dims[dim].lifts.pulleys.get_mut(&pos)?;
            // `onSpeedChanged`: one kept together by its mode moves again, and its limit
            // starts again from the context it has now.
            p.waiting = false;
            if !super::keeps_loaded_limit(dim, pos) {
                p.limit = limit;
            }
            if rpm != 0.0 {
                p.speed_changed(rpm);
            }
            Some(grid_offset(p.offset, p.range))
        })
        .flatten();
        match running {
            Some(grid) if rpm == 0.0 => try_stop_pulley(lv, pos, grid),
            Some(_) => show_pulley(lv, pos),
            None if rpm != 0.0 => assemble_pulley(lv, pos, rpm),
            None => {}
        }
    }
    if !later.is_empty() {
        with_world(|w| w.dims[dim].lifts.pulley_checks.extend(later));
    }
    let moving: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].lifts.pulleys.keys().copied().collect()).unwrap_or_default();
    for pos in moving {
        // A pulley told of its speed this tick only answers it (`sendData`).
        if checks.contains(&pos) || super::unsolved(dim, pos) {
            continue;
        }
        let Some(mut p) = with_world(|w| w.dims[dim].lifts.pulleys.remove(&pos)).flatten() else {
            continue;
        };
        if p.waiting {
            with_world(|w| w.dims[dim].lifts.pulleys.insert(pos, p));
            continue;
        }
        // Its actors first (`tickActors`); while one stalls, the pulley holds (`isStalled`).
        if let Some(mut carried) = PULLEY_CARRIED.with_borrow_mut(|c| c.remove(&(dim, pos))) {
            let at = pulley_origin(&p, p.offset);
            let stalled = actors::tick_actors(lv, pos, &mut carried, at);
            carried.stalled = stalled;
            super::actor_draw::show_actors(lv, pos, pos, &carried);
            PULLEY_CARRIED.with_borrow_mut(|c| c.insert((dim, pos), carried));
            if stalled {
                with_world(|w| w.dims[dim].lifts.pulleys.insert(pos, p));
                continue;
            }
        }
        let before = p.offset;
        let step = {
            let lv: &ServerLevel<'_> = lv;
            p.step(
                p.rpm,
                pos,
                |c, l| {
                    lv.passable(c)
                        || PULLEY_CARRIED.with_borrow(|m| {
                            m.get(&(dim, pos))
                                .is_some_and(|k| actors::breaker_clears(lv, pos, k, l, c))
                        })
                },
                |c| {
                    found(lv, c).is_some_and(|f| f.movable)
                        && lv
                            .block_name(c)
                            .is_none_or(|n| n != "create:rope" && n != "create:pulley_magnet")
                },
            )
        };
        if before < 200.0 && p.offset >= 200.0 {
            crate::glue::advancements::award_placer(lv, pos, "pulley_maxed");
        }
        with_world(|w| w.dims[dim].lifts.pulleys.insert(pos, p));
        match step {
            // Its offset is saved now and then while it moves, as a piston's is.
            Step::Move(_) if lv.game_time() % 10 == 0 => mark_unsaved(lv, pos),
            Step::Move(_) => {}
            Step::Stop(grid) => try_stop_pulley(lv, pos, grid),
            Step::Grab(grid) => {
                stop_pulley(lv, pos, grid);
                with_world(|w| w.dims[dim].lifts.pulley_checks.push(pos));
            }
        }
    }
}

/// `PulleyBlockEntity.assemble`: the rope and magnet under the pulley set its offset; the
/// structure under them and the rope leave the world.
fn assemble_pulley(lv: &mut ServerLevel<'_>, pos: [i32; 3], rpm: f32) {
    use create_engine::pulley::{self, MAX_ROPE_LENGTH, Pulley};
    let dim = lv.dim;
    let rope_at = |lv: &ServerLevel<'_>, p: [i32; 3]| {
        lv.name_props(p)
            .filter(|(n, _)| n == "create:rope" || n == "create:pulley_magnet")
            .map(|(_, props)| super::flag(&props, "waterlogged"))
    };
    let mut i = 1;
    while i <= MAX_ROPE_LENGTH && rope_at(lv, below(pos, i)).is_some() {
        i += 1;
    }
    let offset = i - 1;
    let range = pulley::extension_range(pos[1], lv.build_range().0);
    if (offset >= range && rpm > 0.0) || (offset <= 0 && rpm < 0.0) {
        return;
    }
    let anchor = below(pos, offset + 1);
    let blocks = match pulley::gather(anchor, offset, |p| found(lv, p)) {
        Ok(b) => b,
        Err(r) => {
            super::set_exception(lv, pos, Some(&r));
            return;
        }
    };
    super::set_exception(lv, pos, None);
    let mut p = Pulley::new(anchor, offset, range, blocks);
    p.rpm = rpm;
    p.limit = with_world(|w| w.dims[dim].kinetics.sequence_limit(pos, 0, true)).flatten();
    // `isCollidingWithWorld` a block further down: it doesn't start.
    if rpm > 0.0
        && !p.blocks.is_empty()
        && p.collides(offset as f32 + 1.0, true, &|c, l| {
            lv.passable(c) || breaker_below(lv, &p.blocks, anchor, l, c)
        })
    {
        return;
    }
    // `removeRopes`: rope in water leaves water.
    for j in 1..=offset {
        let c = below(pos, j);
        if rope_at(lv, c) == Some(true)
            && let Some(water) = lv.host.block_state_id("minecraft:water")
        {
            lv.set_state(c, water, UPDATE_CLIENTS);
        } else {
            lv.set_air(c, UPDATE_CLIENTS);
        }
    }
    // Its actors, storage, glue and controls ride with it (`Contraption.addBlock`).
    let carried = actors::carry(lv, pos, anchor, &p.blocks, |_| false);
    if !carried.actors.is_empty() || !carried.controls.is_empty() || !carried.glue.is_empty() {
        PULLEY_CARRIED.with_borrow_mut(|c| c.insert((dim, pos), carried));
    }
    crate::glue::advancements::assembled(lv, pos, &p.blocks);
    take_blocks(lv, anchor, &p.blocks);
    with_world(|w| w.dims[dim].lifts.pulleys.insert(pos, p));
    show_pulley(lv, pos);
    mark_unsaved(lv, pos);
}

/// `LinearActuatorBlockEntity.tryDisassemble` for a pulley stopping at whole offset `grid`: by
/// its movement mode it comes apart (`MOVE_PLACE`, or `MOVE_PLACE_RETURNED` at its starting
/// offset) or holds there, together, until its speed changes (`waitingForSpeedChange`).
fn try_stop_pulley(lv: &mut ServerLevel<'_>, pos: [i32; 3], grid: i32) {
    let dim = lv.dim;
    let mode = super::option_at(dim, pos);
    let keep = with_world(|w| {
        let p = w.dims[dim].lifts.pulleys.get_mut(&pos)?;
        let keep = mode == 2 || (mode == 1 && grid != p.initial);
        if keep {
            p.offset = grid as f32;
            p.waiting = true;
        }
        Some(keep)
    })
    .flatten();
    match keep {
        Some(true) => {
            show_pulley(lv, pos);
            mark_unsaved(lv, pos);
        }
        Some(false) => stop_pulley(lv, pos, grid),
        None => {}
    }
}

/// `PulleyBlockEntity.disassemble` at whole offset `grid`: the magnet at the rope's end and
/// rope above it (`shouldCreateRopes`), then the structure under them.
fn stop_pulley(lv: &mut ServerLevel<'_>, pos: [i32; 3], grid: i32) {
    let dim = lv.dim;
    let Some(mut p) = with_world(|w| w.dims[dim].lifts.pulleys.remove(&pos)).flatten() else {
        return;
    };
    let (rope_cells, magnet) = create_engine::pulley::rope_cells(pos, grid);
    for (c, name) in magnet
        .map(|m| (m, "create:pulley_magnet"))
        .into_iter()
        .chain(rope_cells.into_iter().map(|r| (r, "create:rope")))
    {
        // `getDestroySpeed != -1`: what is there breaks, unless it can't.
        if found(lv, c).is_some_and(|f| !f.movable) {
            continue;
        }
        let wet = lv.block_name(c).is_some_and(|n| n == "minecraft:water");
        let w = if wet { "true" } else { "false" };
        if let Some(s) = state_with(&*lv.host, name, &[("waterlogged", w)]) {
            lv.set_num(c, s, UPDATE_CLIENTS);
        }
    }
    p.offset = grid as f32;
    draw::set_pulley_rest(lv, pos, grid);
    let origin = p.placed_at(create_engine::pulley::grid_offset(p.offset, p.range));
    let held = Held::Pulley(p);
    lift_put_back(lv, pos, origin, &held.put_back(pos));
    // What rode with it goes back with its blocks (`disassemble`).
    let mut carried = PULLEY_CARRIED
        .with_borrow_mut(|c| c.remove(&(dim, pos)))
        .unwrap_or_default();
    actors::release(lv, pos, origin, &mut carried);
    draw::hide(lv, pos);
    mark_unsaved(lv, pos);
}

/// A rope pulley to its clients: its structure (offsets from its anchor) gliding down from its
/// offset now at its blocks a tick (positive down), within its range, rope down to it.
fn show_pulley(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(p) = with_world(|w| w.dims[dim].lifts.pulleys.get(&pos).cloned()).flatten() else {
        return;
    };
    // The magnet and rope are drawn with it (`draw_hanging`).
    let blocks = p.blocks.clone();
    let origin = [
        p.anchor[0] - pos[0],
        p.anchor[1] + p.initial - pos[1],
        p.anchor[2] - pos[2],
    ];
    draw::show_glide(
        lv,
        pos,
        draw::Glide {
            key: pos,
            origin,
            blocks: &blocks,
            dir: [0, -1, 0],
            offset: p.offset,
            per_tick: if p.waiting { 0.0 } else { p.speed(p.rpm) },
            clamp: Some(match p.limit {
                Some(l) => ((p.offset - l).max(0.0), (p.offset + l).min(p.range as f32)),
                None => (0.0, p.range as f32),
            }),
            rope: true,
        },
    );
}

fn show_elevator(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some((e, target, rpm)) = with_world(|w| {
        let l = &w.dims[dim].lifts;
        let e = l.elevators.get(&pos)?.clone();
        let target = l.columns.get(&e.column).and_then(|c| c.target);
        Some((e, target, w.speed(dim, pos).unwrap_or(0.0)))
    })
    .flatten() else {
        return;
    };
    draw::show_elevator(lv, pos, &e, rpm, target);
}

/// `ElevatorColumn.compileNamesList`: the floors of `column`, bottom up, each an elevator
/// contact's height and names.
fn column_floors(
    lv: &ServerLevel<'_>,
    column: create_engine::elevator::ColumnKey,
) -> Vec<create_engine::elevator::Floor> {
    let dim = lv.dim;
    let (ys, names) = with_world(|w| {
        let l = &w.dims[dim].lifts;
        let ys = l
            .columns
            .get(&column)
            .map(|c| c.contacts.clone())
            .unwrap_or_default();
        let names: Vec<Option<(String, String)>> = ys
            .iter()
            .map(|y| {
                l.contacts
                    .get(&[column.x, *y, column.z])
                    .map(|n| (n.short.clone(), n.long.clone()))
            })
            .collect();
        (ys, names)
    })
    .unwrap_or_default();
    ys.into_iter()
        .zip(names)
        .filter(|(y, _)| {
            matches!(
                create_info(lv.block([column.x, *y, column.z])),
                Some((Block::ElevatorContact, _))
            )
        })
        .map(|(y, n)| {
            let (short, long) = n.unwrap_or_default();
            create_engine::elevator::Floor { y, short, long }
        })
        .collect()
}

/// The running elevator at `pos` tells its clients its column's floors when they changed
/// (`ElevatorFloorListPacket`).
fn send_floors(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(column) =
        with_world(|w| w.dims[dim].lifts.elevators.get(&pos).map(|e| e.column)).flatten()
    else {
        return;
    };
    let floors = column_floors(lv, column);
    let same = FLOORS_SENT.with_borrow(|m| m.get(&(dim, pos)) == Some(&floors));
    if same {
        return;
    }
    let list = floors
        .iter()
        .map(|f| {
            let mut c = Compound::new();
            c.insert("Y".into(), Tag::Int(f.y));
            c.insert("Short".into(), Tag::String(f.short.as_str().into()));
            c.insert("Long".into(), Tag::String(f.long.as_str().into()));
            Tag::Compound(c)
        })
        .collect();
    super::actor_draw::write_key(lv, pos, FLOORS, Some(Tag::List(list)));
    FLOORS_SENT.with_borrow_mut(|m| m.insert((dim, pos), floors));
}

/// The floors in an elevator pulley's data ([`FLOORS`]), on its clients.
pub fn floors_of(nbt: &Compound) -> Vec<create_engine::elevator::Floor> {
    let Some(Tag::List(list)) = super::super::kinetics::synced(nbt, FLOORS) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else { return None };
            let text = |k: &str| match c.get(&k.into()) {
                Some(Tag::String(s)) => s.to_string_lossy(),
                _ => String::new(),
            };
            let Some(Tag::Int(y)) = c.get(&"Y".into()) else {
                return None;
            };
            Some(create_engine::elevator::Floor {
                y: *y,
                short: text("Short"),
                long: text("Long"),
            })
        })
        .collect()
}

/// What the running rope pulley or clockwork bearing at `pos` carries (its actors, controls and
/// the kinds turned off), for a click on its contraption controls, and where each of its blocks
/// is now; None for anything else (an elevator's controls pick floors).
pub(super) fn take_lift(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(Carried, actors::Place)> {
    let dim = lv.dim;
    let origin = with_world(|w| {
        let l = &w.dims[dim].lifts;
        if let Some(p) = l.pulleys.get(&pos) {
            return Some(pulley_origin(p, p.offset).map(|v| v.floor() as i32));
        }
        l.clocks.contains_key(&pos).then_some(pos)
    })
    .flatten()?;
    let carried = PULLEY_CARRIED
        .with_borrow(|c| c.get(&(dim, pos)).cloned())
        .unwrap_or_default();
    Some((carried, Box::new(move |o| add(origin, o))))
}

/// What [`take_lift`] took, changed by its controls, goes back.
pub(super) fn put_lift(lv: &mut ServerLevel<'_>, pos: [i32; 3], carried: Carried) {
    let dim = lv.dim;
    let empty = carried.actors.is_empty() && carried.controls.is_empty() && carried.glue.is_empty();
    if !empty {
        PULLEY_CARRIED.with_borrow_mut(|c| c.insert((dim, pos), carried));
    }
    mark_unsaved(lv, pos);
}

/// `ContraptionControlsMovingInteraction.elevatorInteraction` on the server: a click on the
/// contraption controls at offset `local` of the elevator whose pulley is at `key` clicks
/// (`CONTROLLER_CLICK`, `CONTRAPTION_ASSEMBLE`), and the floor its client picked, `target`, is
/// called (`ElevatorTargetFloorPacket`: within 50 blocks, a contact of its column it can
/// reach, `callToContactAndUpdate` unpowered). False when `key` is no running elevator.
pub(super) fn elevator_controls_clicked(
    lv: &mut ServerLevel<'_>,
    player: mod_api::PlayerRef,
    key: [i32; 3],
    local: [i32; 3],
    target: Option<i32>,
) -> bool {
    let dim = lv.dim;
    let Some(e) = with_world(|w| w.dims[dim].lifts.elevators.get(&key).cloned()).flatten() else {
        return false;
    };
    let at = [
        f64::from(e.anchor[0] + local[0]) + 0.5,
        e.y + f64::from(local[1]) + 0.5,
        f64::from(e.anchor[2] + local[2]) + 0.5,
    ];
    let pos = at.map(|v| v.floor() + 0.5);
    let blocks = mod_api::SoundSource::Blocks;
    lv.level.play_sound(
        &mut *lv.host,
        "create:controller_click",
        pos,
        blocks,
        1.0,
        1.5,
    );
    lv.level.play_sound(
        &mut *lv.host,
        "create:contraption_assemble",
        pos,
        blocks,
        0.75,
        0.8,
    );
    let Some(y) = target else {
        return true;
    };
    let near = player
        .entity()
        .position(&*lv.host)
        .is_some_and(|p| (0..3).map(|i| (p[i] - at[i]).powi(2)).sum::<f64>() <= 50.0 * 50.0);
    let column = e.column;
    let listed = with_world(|w| {
        w.dims[dim]
            .lifts
            .columns
            .get(&column)
            .is_some_and(|c| c.contacts.contains(&y))
    }) == Some(true);
    if !near || !listed || y < e.min_contact || y > e.max_contact {
        return true;
    }
    let contact = [column.x, y, column.z];
    if let Some((Block::ElevatorContact, info)) = create_info(lv.block(contact))
        && let Some(f) = info.facing
    {
        call_contact(lv, contact, f, false);
    }
    true
}

/// Elevators: clicked pulleys assemble or come apart (`clicked`); travelling ones move toward
/// their column's floor and light its contact when they arrive; settled ones wait for a call.
fn tick_elevators(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut clicks =
        with_world(|w| std::mem::take(&mut w.dims[dim].lifts.elevator_clicks)).unwrap_or_default();
    clicks.sort();
    clicks.dedup();
    for pos in clicks {
        let running = with_world(|w| w.dims[dim].lifts.elevators.contains_key(&pos)) == Some(true);
        if running {
            disassemble_elevator(lv, pos);
        } else {
            assemble_elevator(lv, pos);
        }
    }
    let travelling: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].lifts.travelling.iter().copied().collect()).unwrap_or_default();
    let now = lv.game_time();
    for pos in travelling {
        // Its actors first (`tickActors`); while one stalls, the elevator holds.
        if let Some(mut carried) = PULLEY_CARRIED.with_borrow_mut(|c| c.remove(&(dim, pos))) {
            let at = with_world(|w| {
                w.dims[dim].lifts.elevators.get(&pos).map(|e| {
                    [
                        f64::from(e.anchor[0]),
                        f64::from(e.anchor[1] + e.initial) - f64::from(e.offset),
                        f64::from(e.anchor[2]),
                    ]
                })
            })
            .flatten();
            let stalled = at.is_some_and(|at| actors::tick_actors(lv, pos, &mut carried, at));
            carried.stalled = stalled;
            super::actor_draw::show_actors(lv, pos, pos, &carried);
            PULLEY_CARRIED.with_borrow_mut(|c| c.insert((dim, pos), carried));
            if stalled {
                continue;
            }
        }
        let rpm = speed(dim, pos);
        let done = with_world(|w| {
            let l = &mut w.dims[dim].lifts;
            let e = l.elevators.get_mut(&pos)?;
            let target = l.columns.get(&e.column).and_then(|c| c.target);
            let t = e.tick(pos[1], rpm, target);
            let column = e.column;
            let settled = e.settled(target);
            // The block its contact reaches into (`getActiveAreaOffset`: .65 out of the
            // contact), by the centre of the contact's block.
            let visiting = (e.y + f64::from(e.contact_y) + 0.5).floor() as i32;
            if settled {
                l.travelling.remove(&pos);
            }
            Some((
                t.reached.map(|y| [column.x, y, column.z]),
                settled,
                column,
                visiting,
            ))
        })
        .flatten();
        let Some((reached, settled, column, visiting)) = done else {
            with_world(|w| w.dims[dim].lifts.travelling.remove(&pos));
            continue;
        };
        // `visitNewPosition`: an elevator contact facing back at it as it passes tells the
        // column's floor (`broadcastFloorData`).
        let moved = VISITED.with_borrow_mut(|v| v.insert((dim, pos), visiting) != Some(visiting));
        if moved {
            let at = [column.x, visiting, column.z];
            if matches!(create_info(lv.block(at)), Some((Block::ElevatorContact, i)) if i.facing == Some(column.side))
            {
                broadcast_floor(lv, column, at);
            }
        }
        elevator_doors(lv, pos);
        // `triggerContact`: an elevator contact there that isn't lit lights on its next tick.
        if let Some(c) = reached
            && let Some((Block::ElevatorContact, info)) = create_info(lv.block(c))
            && !info.powering
        {
            with_world(|w| {
                let l = &mut w.dims[dim].lifts;
                l.contact_lit.insert(c);
                l.schedule(c, now);
            });
        }
        if settled {
            show_elevator(lv, pos);
            mark_unsaved(lv, pos);
        }
    }
}

/// `ElevatorContactBlockEntity.tick`'s `deferNameGenerator` (`ElevatorColumn.initNames`), and
/// the contacts whose names or floor changed: saved, sent to those looking at their screen,
/// and the display links reading them told (`notifyGatherers`).
fn contact_names(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let due = with_world(|w| std::mem::take(&mut w.dims[dim].lifts.names_due)).unwrap_or_default();
    for key in due {
        with_world(|w| w.dims[dim].lifts.init_column_names(key));
    }
    let changed =
        with_world(|w| std::mem::take(&mut w.dims[dim].lifts.contacts_changed)).unwrap_or_default();
    for pos in changed {
        mark_unsaved(lv, pos);
        crate::glue::display::notify_gatherers(lv, pos);
        // Its door control changed: the doors of an elevator standing at its column follow
        // (`SlidingDoorMovementBehaviour.tick` reads the mode each tick).
        let elevators: Vec<[i32; 3]> = with_world(|w| {
            w.dims[dim]
                .lifts
                .elevators
                .iter()
                .filter(|(_, e)| [e.column.x, e.column.z] == [pos[0], pos[2]])
                .map(|(p, _)| *p)
                .collect()
        })
        .unwrap_or_default();
        for e in elevators {
            elevator_doors(lv, e);
        }
    }
}

/// `CurrentFloorDisplaySource`: the floor the elevator contact at `pos` was last told
/// (`lastReportedCurrentFloor`); None for any other block.
pub(crate) fn contact_floor(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<String> {
    let Some((Block::ElevatorContact, _)) = create_info(lv.block(pos)) else {
        return None;
    };
    let dim = lv.dim;
    Some(
        with_world(|w| {
            w.dims[dim]
                .lifts
                .contacts
                .get(&pos)
                .map(|c| c.floor.clone())
        })
        .flatten()
        .unwrap_or_default(),
    )
}

/// `ElevatorContraption.broadcastFloorData`: the elevator contact at `contact` (facing its
/// column) tells its column's contacts its short name as the floor reached.
fn broadcast_floor(
    lv: &ServerLevel<'_>,
    column: create_engine::elevator::ColumnKey,
    contact: [i32; 3],
) {
    let Some((Block::ElevatorContact, _)) = create_info(lv.block(contact)) else {
        return;
    };
    let dim = lv.dim;
    with_world(|w| {
        let l = &mut w.dims[dim].lifts;
        let name = l
            .contacts
            .get(&contact)
            .map(|c| c.short.clone())
            .unwrap_or_default();
        l.floor_reached(column, &name);
    });
}

/// `SlidingDoorMovementBehaviour.tickOpen` for an elevator's sliding doors: they open while
/// it has arrived at a floor (`ec.arrived`) on the sides the door control of the contact it is
/// called to lets open (`getElevatorDoorControl`, `shouldOpenAt`: none without a contact there),
/// and close as it leaves, the door in the world each faces out toward opening and closing with
/// it. Not before the elevator first moved (`firstMovement`).
fn elevator_doors(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(e) = with_world(|w| w.dims[dim].lifts.elevators.get(&pos).cloned()).flatten() else {
        return;
    };
    let moved = ELEVATOR_MOVED.with_borrow_mut(|m| {
        if !e.arrived {
            m.insert((dim, pos));
        }
        m.contains(&(dim, pos))
    });
    let arrived = e.arrived && moved;
    // `getCurrentTargetY` and the contact there (`contactAt`); its `DoorControlBehaviour`.
    let control = arrived
        .then(|| {
            let target = with_world(|w| {
                let c = w.dims[dim].lifts.columns.get(&e.column)?.target;
                e.reachable(c)
            })
            .flatten()?;
            let contact = [e.column.x, target, e.column.z];
            let Some((Block::ElevatorContact, _)) = create_info(lv.block(contact)) else {
                return None;
            };
            with_world(|w| {
                w.dims[dim]
                    .lifts
                    .contacts
                    .get(&contact)
                    .map_or(0, |c| c.door)
            })
        })
        .flatten();
    type Door = (usize, [i32; 3], Vec<(String, String)>, String);
    let doors: Vec<Door> = e
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, (_, s))| {
            create_info(*s).is_some_and(|(b, _)| create_engine::dye::is_sliding_door(b))
        })
        .filter_map(|(i, (o, s))| {
            let (name, props) = super::name_props(&*lv.host, *s)?;
            Some((i, *o, props, name))
        })
        .collect();
    if doors.is_empty() {
        return;
    }
    let lo = e.blocks.iter().fold([i32::MAX; 3], |m, (o, _)| {
        [m[0].min(o[0]), m[1].min(o[1]), m[2].min(o[2])]
    });
    let hi = e.blocks.iter().fold([i32::MIN; 3], |m, (o, _)| {
        [m[0].max(o[0] + 1), m[1].max(o[1] + 1), m[2].max(o[2] + 1)]
    });
    let center = [0, 1, 2].map(|i| f64::from(lo[i] + hi[i]) / 2.0);
    let mut changed = Vec::new();
    let mut sound = None;
    for (i, o, props, name) in &doors {
        let prop = |k: &str| {
            props
                .iter()
                .find(|(pk, _)| pk == k)
                .map(|(_, v)| v.as_str())
        };
        // `getDoorFacing`: along its facing's axis, away from the structure's middle.
        let Some(facing) = prop("facing").and_then(super::dir_of) else {
            continue;
        };
        let axis = facing.axis();
        let k = match axis {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        };
        let f = facing.offset();
        let along = f64::from(o[k]) + 0.5 - f64::from(f[k]) * 0.45 - center[k];
        let Some(out) = Dir::ALL
            .into_iter()
            .find(|d| d.axis() == axis && d.positive() == (along >= 0.0))
        else {
            continue;
        };
        let open = control.is_some_and(|m| create_engine::lifts::door_control_matches(m, out));
        if prop("open") == Some(if open { "true" } else { "false" }) {
            continue;
        }
        let set: Vec<(&str, &str)> = props
            .iter()
            .map(|(k, v)| {
                if k == "open" {
                    (k.as_str(), if open { "true" } else { "false" })
                } else {
                    (k.as_str(), v.as_str())
                }
            })
            .collect();
        if let Some(id) = state_with(&*lv.host, name, &set) {
            changed.push((*i, id));
        }
        if prop("half") != Some("lower") {
            continue;
        }
        let here = [
            e.anchor[0] + o[0],
            (e.y + f64::from(o[1])).round() as i32,
            e.anchor[2] + o[2],
        ];
        let world_door = add(here, out.offset());
        if let Some((n, p)) = lv.name_props(world_door)
            && n.ends_with("_door")
            && p.iter()
                .find(|(k, _)| k == "facing")
                .and_then(|(_, v)| super::dir_of(v))
                .is_some_and(|d| d.axis() == axis)
        {
            lv.set_property(
                world_door,
                "open",
                if open { "true" } else { "false" },
                update_all(),
            );
        }
        if open {
            sound = Some(here);
        }
    }
    if changed.is_empty() {
        return;
    }
    with_world(|w| {
        if let Some(e) = w.dims[dim].lifts.elevators.get_mut(&pos) {
            for (i, id) in &changed {
                if let Some(b) = e.blocks.get_mut(*i) {
                    b.1 = *id;
                }
            }
        }
    });
    if let Some(at) = sound {
        lv.level.play_sound(
            &mut *lv.host,
            "minecraft:block.iron_door.open",
            super::center(at),
            mod_api::SoundSource::Blocks,
            0.125,
            1.0,
        );
    }
    show_elevator(lv, pos);
    mark_unsaved(lv, pos);
}

/// `ElevatorPulleyBlockEntity.assemble`: the structure under the free blocks below the pulley,
/// with exactly one redstone contact facing sideways, leaves the world; its column's redstone
/// contacts become elevator contacts (`gatherAll`) and it is called to the floor it is at.
fn assemble_elevator(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    use create_engine::elevator::{self, Elevator};
    use create_engine::pulley::{self, MAX_ROPE_LENGTH};
    let dim = lv.dim;
    if !matches!(create_info(lv.block(pos)), Some((Block::ElevatorPulley, _))) {
        return;
    }
    if speed(dim, pos) == 0.0 {
        return;
    }
    let mut i = 1;
    while i <= MAX_ROPE_LENGTH && lv.passable(below(pos, i)) {
        i += 1;
    }
    let offset = i - 1;
    let anchor = below(pos, offset + 1);
    let Ok(blocks) = pulley::gather(anchor, offset, |p| found(lv, p)) else {
        return;
    };
    // `ElevatorContraption.capture`: its redstone contacts facing sideways; one, no more.
    let contacts: Vec<([i32; 3], Dir)> = blocks
        .iter()
        .filter_map(|(o, s)| match create_info(*s) {
            Some((Block::RedstoneContact, i)) => {
                i.facing.filter(|f| f.axis() != Axis::Y).map(|f| (*o, f))
            }
            _ => None,
        })
        .collect();
    let [(local, facing)] = contacts[..] else {
        return;
    };
    let (column, contact_y) = elevator::contraption_column(anchor, local, facing);
    let conflict = with_world(|w| {
        w.dims[dim]
            .lifts
            .columns
            .get(&column)
            .is_some_and(|c| c.active)
    });
    if conflict != Some(false) {
        return;
    }
    let mut carried = actors::carry(lv, pos, anchor, &blocks, |_| false);
    // An elevator's controls choose floors: none turns its kind off as it starts
    // (`ElevatorContraption.disableActorOnStart` does nothing), and they forget their filters
    // (`ContraptionControlsMovement.startMoving`).
    carried.disabled.clear();
    for a in &mut carried.actors {
        a.disabled = false;
    }
    for c in &mut carried.controls {
        c.1.filter = None;
    }
    if !carried.actors.is_empty() || !carried.controls.is_empty() || !carried.glue.is_empty() {
        PULLEY_CARRIED.with_borrow_mut(|c| c.insert((dim, pos), carried));
    }
    crate::glue::advancements::assembled(lv, pos, &blocks);
    take_blocks(lv, anchor, &blocks);
    let (min_y, height) = lv.build_range();
    let range = pulley::extension_range(pos[1], min_y);
    let e = Elevator::new(pos[1], anchor, offset, range, column, contact_y, blocks);
    let target = e.floor_now(pos[1]);
    // `ElevatorColumn.gatherAll`: redstone contacts in the column facing it join it.
    for y in min_y..min_y + height {
        let p = [column.x, y, column.z];
        let Some((Block::RedstoneContact, i)) = create_info(lv.block(p)) else {
            continue;
        };
        if i.facing != Some(column.side) {
            continue;
        }
        let powered = if i.powered { "true" } else { "false" };
        if let Some(s) = state_with(
            &*lv.host,
            "create:elevator_contact",
            &[("facing", dir_name(column.side)), ("powered", powered)],
        ) {
            lv.set_num(p, s, update_all());
        }
    }
    let now = lv.game_time();
    let here = [column.x, target, column.z];
    let light = matches!(
        create_info(lv.block(here)),
        Some((Block::ElevatorContact, i)) if !i.powering
    );
    with_world(|w| {
        let l = &mut w.dims[dim].lifts;
        l.columns.entry(column).or_default().target = Some(target);
        l.set_active(column, true);
        l.elevators.insert(pos, e);
        l.travelling.insert(pos);
        // `lazyTick`: standing at its floor, it lights that floor's contact.
        if light {
            l.contact_lit.insert(here);
            l.schedule(here, now);
        }
    });
    let visiting = with_world(|w| {
        w.dims[dim]
            .lifts
            .elevators
            .get(&pos)
            .map(|e| (e.y + f64::from(e.contact_y) + 0.5).floor() as i32)
    })
    .flatten();
    if let Some(v) = visiting {
        VISITED.with_borrow_mut(|m| m.insert((dim, pos), v));
    }
    broadcast_floor(lv, column, here);
    show_elevator(lv, pos);
    mark_unsaved(lv, pos);
}

/// `ElevatorPulleyBlockEntity.disassemble`: its column rests; the structure goes back at the
/// nearest whole block (no rope: `shouldCreateRopes` is false).
fn disassemble_elevator(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(e) = with_world(|w| {
        let l = &mut w.dims[dim].lifts;
        l.travelling.remove(&pos);
        let e = l.elevators.remove(&pos)?;
        l.set_active(e.column, false);
        Some(e)
    })
    .flatten() else {
        return;
    };
    let origin = e.placed_at();
    ELEVATOR_MOVED.with_borrow_mut(|m| m.remove(&(dim, pos)));
    let held = Held::Elevator(e);
    lift_put_back(lv, pos, origin, &held.put_back(pos));
    if let Some(mut carried) = PULLEY_CARRIED.with_borrow_mut(|c| c.remove(&(dim, pos))) {
        actors::release(lv, pos, origin, &mut carried);
    }
    draw::hide(lv, pos);
    draw::set_pulley_rest(lv, pos, -1);
    // Its floors go with the contraption (a new one is told them again).
    if FLOORS_SENT
        .with_borrow_mut(|m| m.remove(&(dim, pos)))
        .is_some()
    {
        super::actor_draw::write_key(lv, pos, FLOORS, None);
    }
    mark_unsaved(lv, pos);
}

/// Reads a level's states: the server's level or a level reader.
type Read<'a> = &'a dyn Fn([i32; 3]) -> Option<BlockState>;

/// The Create block and facts of the state at `pos`.
fn info_at(read: Read<'_>, pos: [i32; 3]) -> Option<(Block, create_engine::parts::StateInfo)> {
    create_info(states::id(read(pos)?)?)
}

/// Whether the block `facing` of a contact is a contact facing back
/// (`RedstoneContactBlock.hasValidContact`).
fn contact_faces_back(read: Read<'_>, pos: [i32; 3], facing: Dir) -> bool {
    let other = match info_at(read, add(pos, facing.offset())) {
        Some((Block::RedstoneContact | Block::ElevatorContact, i)) => i.facing,
        _ => None,
    };
    create_engine::elevator::faces_back(facing, other)
}

/// Contacts: redstone contacts power while one faces them (`updateShape`); an elevator contact
/// given a signal calls the elevator to its floor (`neighborChanged`,
/// `callToContactAndUpdate`), and lights while one faces it or an elevator arrived at it
/// (`tick` after `scheduleActivation`).
fn tick_contacts(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let now = lv.game_time();
    let Some((mut checks, due)) = with_world(|w| {
        let l = &mut w.dims[dim].lifts;
        let (due, later): (Vec<_>, Vec<_>) = l.contact_ticks.drain(..).partition(|t| t.1 <= now);
        l.contact_ticks = later;
        (std::mem::take(&mut l.contact_checks), due)
    }) else {
        return;
    };
    checks.sort();
    checks.dedup();
    for pos in checks {
        let Some((block, info)) = create_info(lv.block(pos)) else {
            continue;
        };
        let Some(facing) = info.facing else {
            continue;
        };
        let valid = contact_faces_back(&|p| lv.block_state(p), pos, facing);
        // A contact a moving one powers stays powered until that one leaves it.
        let held = with_world(|w| w.dims[dim].lifts.contact_held.contains(&pos)) == Some(true);
        match block {
            Block::RedstoneContact if info.powered != valid && !(info.powered && held) => {
                lv.set_flags(pos, &[("powered", valid)], update_all());
            }
            Block::ElevatorContact => {
                let signal = lv.has_neighbor_signal(pos);
                if info.powered != signal {
                    lv.set_flags(pos, &[("powered", signal)], UPDATE_CLIENTS);
                    if signal && !info.calling {
                        call_contact(lv, pos, facing, true);
                    }
                }
                // `updateShape` (the block it faces changed): a contact an elevator lit keeps
                // its light through other neighbour changes.
                if info.powering != valid && !(info.powering && held) {
                    with_world(|w| w.dims[dim].lifts.schedule(pos, now));
                }
            }
            _ => {}
        }
    }
    for (pos, _) in due {
        let (block, info) = match create_info(lv.block(pos)) {
            Some((b @ (Block::ElevatorContact | Block::RedstoneContact), i)) => (b, i),
            _ => continue,
        };
        // `RedstoneContactBlock.tick`: a contact a moving one left powers while one faces it.
        if block == Block::RedstoneContact {
            with_world(|w| w.dims[dim].lifts.contact_held.remove(&pos));
            let valid = info
                .facing
                .is_some_and(|f| contact_faces_back(&|p| lv.block_state(p), pos, f));
            if info.powered != valid {
                lv.set_flags(pos, &[("powered", valid)], update_all());
            }
            continue;
        }
        let lit = with_world(|w| w.dims[dim].lifts.contact_lit.remove(&pos)) == Some(true);
        let should = lit
            || info
                .facing
                .is_some_and(|f| contact_faces_back(&|p| lv.block_state(p), pos, f));
        with_world(|w| {
            let held = &mut w.dims[dim].lifts.contact_held;
            if lit {
                held.insert(pos);
            } else {
                held.remove(&pos);
            }
        });
        if info.powering || should {
            lv.set_flags(pos, &[("powering", should)], UPDATE_CLIENTS);
        }
        lv.update_signal_neighbors(pos);
    }
}

/// `ElevatorContactBlock.callToContactAndUpdate`: this contact calls, the others of its column
/// stop calling and go dark on their next tick, and the column's elevator sets off to it.
fn call_contact(lv: &mut ServerLevel<'_>, pos: [i32; 3], facing: Dir, powered: bool) {
    let dim = lv.dim;
    if powered {
        lv.set_flags(pos, &[("calling", true), ("powered", true)], UPDATE_CLIENTS);
    } else {
        lv.set_flags(pos, &[("calling", true)], UPDATE_CLIENTS);
    }
    lv.update_signal_neighbors(pos);
    let column = create_engine::elevator::column_of(pos, facing);
    let others: Vec<i32> = with_world(|w| {
        w.dims[dim]
            .lifts
            .columns
            .get(&column)
            .map(|c| c.contacts.clone())
    })
    .flatten()
    .unwrap_or_default();
    let now = lv.game_time();
    for y in others {
        let p = [pos[0], y, pos[2]];
        if p == pos {
            continue;
        }
        if !matches!(create_info(lv.block(p)), Some((Block::ElevatorContact, _))) {
            continue;
        }
        lv.set_flags(p, &[("calling", false)], UPDATE_CLIENTS);
        with_world(|w| w.dims[dim].lifts.schedule(p, now));
    }
    with_world(|w| w.dims[dim].lifts.call(column, pos[1]));
}

// ---------------------------------------------------------------------------------------------
// Parts

struct LiftPart(Block);

#[mod_api::horizon_part]
impl BlockPart for LiftPart {
    /// A right click on a clockwork bearing, rope pulley or elevator pulley with an empty hand,
    /// not sneaking (`useItemOn`): a running clock or elevator comes apart, else it assembles on
    /// the next tick; a rope pulley looks at its speed again (`assembleNextTick`).
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let c = clicker(&*host, event.click);
        if c.sneaking || c.held.is_some() || c.after_empty_main {
            return None;
        }
        let pos = event.click.pos;
        let block = self.0;
        in_click_level(host, event.click, |lv| {
            let dim = lv.dim;
            match block {
                Block::ClockworkBearing => {
                    let running =
                        with_world(|w| w.dims[dim].lifts.clocks.contains_key(&pos)) == Some(true);
                    if running {
                        disassemble_clock(lv, pos);
                    } else {
                        with_world(|w| w.dims[dim].lifts.clock_checks.push(pos));
                    }
                }
                Block::RopePulley => {
                    with_world(|w| w.dims[dim].lifts.pulley_checks.push(pos));
                }
                _ => {
                    with_world(|w| w.dims[dim].lifts.elevator_clicks.push(pos));
                }
            }
        });
        Some(InteractionResult::Success)
    }
}

/// `ElevatorColumn.get(level, new ColumnCoords(x, z, facing)) != null` for a contact at `pos`
/// facing `facing` (never up or down): a column exists there, with an elevator or another
/// contact than the one at height `own` (whose block entity keeps the column it joined).
fn facing_column(dim: usize, pos: [i32; 3], facing: Dir, own: Option<i32>) -> bool {
    if facing.axis() == Axis::Y {
        return false;
    }
    let key = create_engine::elevator::column_of(pos, facing);
    with_world(|w| {
        w.dims[dim]
            .lifts
            .columns
            .get(&key)
            .is_some_and(|c| c.active || c.contacts.iter().any(|y| Some(*y) != own))
    }) == Some(true)
}

/// `RedstoneContactBlock.onWrenched` / `ElevatorContactBlock.onWrenched` after the wrench
/// turned the contact at `pos`: a redstone contact now facing an elevator column becomes its
/// elevator contact, an elevator contact facing up, down or no column a redstone contact
/// (`setBlockAndUpdate` with `copyProperties`: its facing and power).
pub(crate) fn contact_wrenched(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let Some((block, info)) = create_info(lv.block(pos)) else {
        return;
    };
    let Some(facing) = info.facing else {
        return;
    };
    let column = facing_column(lv.dim, pos, facing, Some(pos[1]));
    let to = match block {
        Block::RedstoneContact if column => "create:elevator_contact",
        Block::ElevatorContact if !column => "create:redstone_contact",
        _ => return,
    };
    let powered = if info.powered { "true" } else { "false" };
    if let Some(s) = state_with(
        &*lv.host,
        to,
        &[("facing", dir_name(facing)), ("powered", powered)],
    ) {
        lv.set_num(pos, s, update_all());
    }
}

struct ContactPart;

#[mod_api::horizon_part]
impl BlockPart for ContactPart {
    /// `RedstoneContactBlock.getStateForPlacement`: facing away from the player's view, or onto
    /// the clicked face when sneaking or a contact is there; powered when a contact faces back.
    fn state_for_placement(
        &self,
        level: &dyn LevelHost,
        event: &mut StateForPlacementEvent<'_>,
    ) -> Option<Option<BlockState>> {
        let name = level.state_block(event.state)?;
        if name != "create:redstone_contact" {
            return None;
        }
        let cx = event.context;
        let pos = [cx.pos.x, cx.pos.y, cx.pos.z];
        let ctx = create_engine::place::Context {
            yaw: cx.yaw,
            pitch: cx.pitch,
            sneaking: cx.sneaking,
            clicked_face: Dir::ALL[cx.face as usize],
        };
        let (facing, powered) = create_engine::place::redstone_contact(ctx, |d| {
            contact_faces_back(&|p| level.block_state(p), pos, d)
        });
        // `RedstoneContactItem.getPlacementState`: facing an elevator column, it is that
        // column's elevator contact (`copyProperties`).
        let dim = super::super::world::level_index(event.dimension.key().id());
        let name = if dim.is_some_and(|dim| facing_column(dim, pos, facing, None)) {
            "create:elevator_contact"
        } else {
            "create:redstone_contact"
        };
        let text = format!("{name}[facing={},powered={powered}]", dir_name(facing));
        Some(level.block_state_id(&text))
    }

    /// `ElevatorContactBlock.useItemOn`: anything but the wrench opens its screen (Create opens
    /// it on the client; here the server sends what the contact holds).
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let (Block::ElevatorContact, _) = states::id(event.state).and_then(create_info)? else {
            return None;
        };
        let c = clicker(&*host, event.click);
        if c.held.as_deref() == Some("create:wrench") {
            return None;
        }
        if c.after_empty_main {
            return Some(InteractionResult::Success);
        }
        let player = c.player?;
        let pos = event.click.pos;
        in_click_level(host, event.click, |lv| {
            crate::glue::display::contact_screen::open(lv, player, pos);
        });
        Some(InteractionResult::Success)
    }

    /// A contact's signal toward the reader on `face` (`getSignal`).
    fn block_signal(&self, level: &dyn LevelHost, event: &mut BlockSignalEvent) {
        let Some((block, info)) = states::id(event.state).and_then(create_info) else {
            return;
        };
        let face = Dir::ALL[event.face as usize];
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        match block {
            Block::ElevatorContact => {
                let reader = add(pos, face.offset());
                let reader_is_contact = matches!(
                    info_at(&|p| level.block_state(p), reader),
                    Some((Block::ElevatorContact, _))
                );
                event.signal =
                    create_engine::elevator::contact_signal(info.powering, reader_is_contact);
            }
            Block::RedstoneContact => {
                if let Some(facing) = info.facing {
                    event.signal = create_engine::elevator::redstone_contact_signal(
                        info.powered,
                        facing,
                        face.opposite(),
                    );
                }
            }
            _ => {}
        }
    }

    /// `isSignalSource`: an elevator contact while powering, a redstone contact while powered.
    fn is_signal_source(&self, _level: &dyn LevelHost, event: &mut IsSignalSourceEvent) {
        match states::id(event.state).and_then(create_info) {
            Some((Block::ElevatorContact, i)) if i.powering => event.source = true,
            Some((Block::RedstoneContact, i)) if i.powered => event.source = true,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Saved with the column

/// Running clocks, pulleys and elevators of a column, saved under `create_lifts` (their block
/// states as text): `kind` 0 a clock, 1 a pulley, 2 an elevator, its numbers in `nums` (floats
/// as their bits), and its blocks.
pub(super) fn chunk_save(lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
    let dim = lv.dim;
    let Some((all, targets)) = with_world(|w| {
        let l = &w.dims[dim].lifts;
        let v = l.column(chunk[0], chunk[1]);
        // A column's floor call is kept with its elevator.
        let targets: Vec<Option<i32>> = v
            .iter()
            .map(|(_, h)| match h {
                Held::Elevator(e) => l.columns.get(&e.column).and_then(|c| c.target),
                _ => None,
            })
            .collect();
        (v, targets)
    }) else {
        return;
    };
    if all.is_empty() {
        return;
    }
    let host = &*lv.host;
    let blocks_tags = |c: &mut Compound, suffix: &str, blocks: &[([i32; 3], u32)]| {
        c.insert(
            format!("blocks{suffix}").as_str().into(),
            Tag::IntArray(blocks.iter().flat_map(|(o, _)| *o).collect()),
        );
        c.insert(
            format!("states{suffix}").as_str().into(),
            Tag::List(
                blocks
                    .iter()
                    .map(|(_, s)| {
                        Tag::String(state_text(host, *s).unwrap_or_default().as_str().into())
                    })
                    .collect(),
            ),
        );
    };
    let bits = |f: f32| f.to_bits() as i32;
    let list = all
        .iter()
        .zip(targets)
        .map(|((pos, held), target)| {
            let mut c = Compound::new();
            c.insert("pos".into(), Tag::IntArray(pos.to_vec()));
            // An elevator that moved since it assembled (`MovementContext.firstMovement`
            // false, saved as `FirstMovement`): its doors open at floors again after a load.
            if ELEVATOR_MOVED.with_borrow(|m| m.contains(&(dim, *pos))) {
                c.insert("moved".into(), Tag::Byte(1));
            }
            // What its storage blocks hold (`MountedStorageManager`), and its glue, controls
            // and the kinds they turned off.
            if let Some(t) = actors::storage_to_nbt(lv, *pos) {
                c.insert("storage".into(), t);
            }
            PULLEY_CARRIED.with_borrow(|m| {
                if let Some(k) = m.get(&(dim, *pos)) {
                    actors::extras_to_nbt(&mut c, k);
                }
            });
            let (kind, nums) = match held {
                Held::Clock(k) => {
                    blocks_tags(&mut c, "", &k.hour);
                    if let Some((_, m)) = &k.minute {
                        blocks_tags(&mut c, "2", m);
                    }
                    (
                        0,
                        vec![
                            k.facing as i32,
                            i32::from(k.clock.hands.index()),
                            bits(k.clock.hour),
                            bits(k.clock.minute),
                            bits(k.rpm),
                            k.minute.as_ref().map_or(0, |(r, _)| *r),
                        ],
                    )
                }
                Held::Pulley(p) => {
                    blocks_tags(&mut c, "", &p.blocks);
                    super::limit_to_nbt(&mut c, p.limit);
                    (
                        1,
                        vec![
                            p.anchor[0],
                            p.anchor[1],
                            p.anchor[2],
                            p.initial,
                            bits(p.offset),
                            p.range,
                            bits(p.rpm),
                            i32::from(p.waiting),
                        ],
                    )
                }
                Held::Elevator(e) => {
                    blocks_tags(&mut c, "", &e.blocks);
                    let y = e.y.to_bits();
                    (
                        2,
                        vec![
                            e.anchor[0],
                            e.anchor[1],
                            e.anchor[2],
                            e.initial,
                            bits(e.offset),
                            (y >> 32) as i32,
                            y as i32,
                            bits(e.prev),
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
                        ],
                    )
                }
            };
            c.insert("kind".into(), Tag::IntArray(vec![kind]));
            c.insert("nums".into(), Tag::IntArray(nums));
            Tag::Compound(c)
        })
        .collect();
    data.insert("create_lifts".into(), Tag::List(list));
}

/// What `chunk_save` wrote: saved structures wait for their blocks; elevators' columns keep
/// their floor calls (`lifts_from_nbt`, `load_lifts`).
pub(super) fn chunk_load(lv: &mut ServerLevel<'_>, data: &Compound) {
    use create_engine::lifts::RunningClock;
    let Some(Tag::List(list)) = data.get(&"create_lifts".into()) else {
        return;
    };
    let host = &*lv.host;
    let blocks = |c: &Compound, suffix: &str| -> Option<Vec<([i32; 3], u32)>> {
        let Some(Tag::IntArray(o)) = c.get(&format!("blocks{suffix}").as_str().into()) else {
            return None;
        };
        let Some(Tag::List(s)) = c.get(&format!("states{suffix}").as_str().into()) else {
            return None;
        };
        Some(
            o.as_chunks::<3>()
                .0
                .iter()
                .zip(s)
                .filter_map(|(o, s)| {
                    let Tag::String(s) = s else { return None };
                    Some((
                        [o[0], o[1], o[2]],
                        state_of_text(host, &s.to_string_lossy())?,
                    ))
                })
                .collect(),
        )
    };
    let f = |i: i32| f32::from_bits(i as u32);
    let dir = |i: i32| Dir::ALL[i.clamp(0, 5) as usize];
    let saved: Vec<Saved> = list
        .iter()
        .filter_map(|c| {
            let Tag::Compound(c) = c else { return None };
            let Some(Tag::IntArray(p)) = c.get(&"pos".into()) else {
                return None;
            };
            let Some(Tag::IntArray(n)) = c.get(&"nums".into()) else {
                return None;
            };
            let Some(Tag::IntArray(kind)) = c.get(&"kind".into()) else {
                return None;
            };
            let kind = *kind.first()?;
            let pos = [*p.first()?, *p.get(1)?, *p.get(2)?];
            actors::storage_from_nbt(lv, pos, c.get(&"storage".into()));
            if c.contains_key(&"moved".into()) {
                ELEVATOR_MOVED.with_borrow_mut(|m| m.insert((lv.dim, pos)));
            }
            let at = |i: usize| n.get(i).copied();
            let mut call = None;
            let held = match kind {
                0 => Held::Clock(RunningClock {
                    clock: create_engine::clock::Clock {
                        hour: f(at(2)?),
                        minute: f(at(3)?),
                        hands: create_engine::clock::Hands::from_index(at(1)? as u8),
                    },
                    facing: dir(at(0)?),
                    rpm: f(at(4)?),
                    hour: blocks(c, "")?,
                    minute: blocks(c, "2").map(|b| (at(5).unwrap_or(0), b)),
                }),
                1 => {
                    let mut x = create_engine::pulley::Pulley::new(
                        [at(0)?, at(1)?, at(2)?],
                        at(3)?,
                        at(5)?,
                        blocks(c, "")?,
                    );
                    x.offset = f(at(4)?);
                    x.rpm = f(at(6)?);
                    x.waiting = at(7) == Some(1);
                    x.limit = super::limit_from_nbt(lv.dim, pos, c);
                    Held::Pulley(x)
                }
                2 => {
                    let column = create_engine::elevator::ColumnKey {
                        x: at(10)?,
                        z: at(11)?,
                        side: dir(at(12)?),
                    };
                    if at(16) == Some(1) {
                        call = Some((column, at(17)?));
                    }
                    Held::Elevator(create_engine::elevator::Elevator {
                        anchor: [at(0)?, at(1)?, at(2)?],
                        initial: at(3)?,
                        offset: f(at(4)?),
                        y: f64::from_bits((at(5)? as u32 as u64) << 32 | at(6)? as u32 as u64),
                        prev: f(at(7)?),
                        arrived: at(8)? != 0,
                        range: at(9)?,
                        column,
                        contact_y: at(13)?,
                        min_contact: at(14)?,
                        max_contact: at(15)?,
                        blocks: blocks(c, "")?,
                    })
                }
                _ => return None,
            };
            // Its actors found again from its blocks, with what it carried besides.
            let extras = actors::extras_from_nbt(c);
            let carried = match &held {
                Held::Pulley(x) => actors::with_extras(actors::bearing_actors(&x.blocks), &extras),
                Held::Elevator(e) => {
                    actors::with_extras(actors::bearing_actors(&e.blocks), &extras)
                }
                Held::Clock(_) => extras,
            };
            let empty =
                carried.actors.is_empty() && carried.controls.is_empty() && carried.glue.is_empty();
            PULLEY_CARRIED.with_borrow_mut(|m| {
                if empty {
                    m.remove(&(lv.dim, pos));
                } else {
                    m.insert((lv.dim, pos), carried);
                }
            });
            Some((pos, held, call))
        })
        .collect();
    let dim = lv.dim;
    with_world(|w| {
        let l = &mut w.dims[dim].lifts;
        let mut held = Vec::with_capacity(saved.len());
        for (pos, h, call) in saved {
            if let Some((key, y)) = call {
                l.columns.entry(key).or_default().target = Some(y);
            }
            held.push((pos, h));
        }
        l.load(held.into_iter());
    });
}

/// Elevator contacts' names, door setting and floor shown (`ElevatorContactBlockEntity.write`:
/// `ShortName`, `LongName`, `LastReportedCurrentFloor`, and the door behaviour's `DoorControl`),
/// saved with their column under `create_contacts`.
pub(super) fn contacts_save(lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
    let dim = lv.dim;
    let list: Vec<Tag> = with_world(|w| {
        w.dims[dim]
            .lifts
            .contacts
            .iter()
            .filter(|(p, _)| [p[0] >> 4, p[2] >> 4] == chunk)
            .map(|(p, c)| {
                let mut t = Compound::new();
                t.insert("Pos".into(), Tag::IntArray(p.to_vec()));
                t.insert("ShortName".into(), Tag::String(c.short.as_str().into()));
                t.insert("LongName".into(), Tag::String(c.long.as_str().into()));
                t.insert(
                    "LastReportedCurrentFloor".into(),
                    Tag::String(c.floor.as_str().into()),
                );
                let door = create_engine::lifts::DOOR_CONTROLS
                    .get(usize::from(c.door))
                    .copied()
                    .unwrap_or("all");
                t.insert(
                    "DoorControl".into(),
                    Tag::String(door.to_uppercase().as_str().into()),
                );
                Tag::Compound(t)
            })
            .collect()
    })
    .unwrap_or_default();
    if !list.is_empty() {
        data.insert("create_contacts".into(), Tag::List(list));
    }
}

/// What `contacts_save` wrote.
pub(super) fn contacts_load(lv: &mut ServerLevel<'_>, data: &Compound) {
    let Some(Tag::List(list)) = data.get(&"create_contacts".into()) else {
        return;
    };
    let text = |c: &Compound, k: &str| match c.get(&k.into()) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    let dim = lv.dim;
    with_world(|w| {
        let l = &mut w.dims[dim].lifts;
        for t in list {
            let Tag::Compound(c) = t else { continue };
            let Some(Tag::IntArray(p)) = c.get(&"Pos".into()) else {
                continue;
            };
            let [x, y, z] = p[..] else { continue };
            let door = text(c, "DoorControl").to_lowercase();
            l.contacts.insert(
                [x, y, z],
                create_engine::lifts::ContactNames {
                    short: text(c, "ShortName"),
                    long: text(c, "LongName"),
                    door: create_engine::lifts::DOOR_CONTROLS
                        .iter()
                        .position(|d| *d == door)
                        .unwrap_or(0) as u8,
                    floor: text(c, "LastReportedCurrentFloor"),
                },
            );
        }
    });
}
