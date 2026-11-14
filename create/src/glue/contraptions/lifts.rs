//! Clockwork bearings, rope pulleys and elevators (`lifts.rs` in the engine): their structures
//! leave the world while they run and come back where they stop; clients draw each from its
//! block's data (`draw`), told when it starts, stops or changes speed. Redstone and elevator
//! contacts: their signals, placement and lights. Saved under `create_lifts`.

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

/// A clock's hands' degrees a tick.
type HandSpeeds = (f32, f32);

/// A saved structure, and its elevator column's floor call.
type Saved = (
    [i32; 3],
    Held,
    Option<(create_engine::elevator::ColumnKey, i32)>,
);

thread_local! {
    /// What running rope pulleys and elevators carry besides their blocks (`Contraption.actors`,
    /// storage, glue, controls), by dimension and pulley; only those whose structure has any.
    static PULLEY_CARRIED: RefCell<HashMap<(usize, [i32; 3]), Carried>> =
        RefCell::new(HashMap::new());
    /// The degrees a tick each running clock's hands were last shown turning at.
    static CLOCK_SHOWN: RefCell<HashMap<(usize, [i32; 3]), HandSpeeds>> =
        RefCell::new(HashMap::new());
}

pub(super) fn forget() {
    PULLEY_CARRIED.with_borrow_mut(HashMap::clear);
    CLOCK_SHOWN.with_borrow_mut(HashMap::clear);
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
fn lift_put_back(lv: &mut ServerLevel<'_>, blocks: &[([i32; 3], u32)]) {
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
            && l.changes.is_empty())
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
        lift_put_back(lv, &held.put_back(pos));
        // What rode with it (its storage too, kept apart from its actors) goes back.
        if let Some(origin) = origin {
            let mut carried = PULLEY_CARRIED
                .with_borrow_mut(|c| c.remove(&(dim, pos)))
                .unwrap_or_default();
            actors::release(lv, pos, origin, &mut carried);
        }
        draw::hide(lv, pos);
        mark_unsaved(lv, pos);
    }
    tick_clocks(lv);
    tick_pulleys(lv);
    tick_elevators(lv);
    tick_contacts(lv);
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
    let Ok(Some(hands)) = create_engine::clock::gather(pos, facing, |p| found(lv, p)) else {
        return;
    };
    let mut all = hands.hour.clone();
    if let Some((_, m)) = &hands.minute {
        all.extend(m.iter().copied());
    }
    actors::carry_storage(lv, pos, pos, &all);
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
    lift_put_back(lv, &held.put_back(pos));
    actors::release_storage(lv, pos, |o| add(pos, o));
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
    for &pos in &checks {
        if !matches!(create_info(lv.block(pos)), Some((Block::RopePulley, _))) {
            continue;
        }
        let rpm = speed(dim, pos);
        let running = with_world(|w| {
            let p = w.dims[dim].lifts.pulleys.get_mut(&pos)?;
            // `onSpeedChanged`: one kept together by its mode moves again.
            p.waiting = false;
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
    let moving: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].lifts.pulleys.keys().copied().collect()).unwrap_or_default();
    for pos in moving {
        // A pulley told of its speed this tick only answers it (`sendData`).
        if checks.contains(&pos) {
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
            PULLEY_CARRIED.with_borrow_mut(|c| c.insert((dim, pos), carried));
            if stalled {
                with_world(|w| w.dims[dim].lifts.pulleys.insert(pos, p));
                continue;
            }
        }
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
        with_world(|w| w.dims[dim].lifts.pulleys.insert(pos, p));
        match step {
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
    let Ok(blocks) = pulley::gather(anchor, offset, |p| found(lv, p)) else {
        return;
    };
    let mut p = Pulley::new(anchor, offset, range, blocks);
    p.rpm = rpm;
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
    let origin = p.placed_at(create_engine::pulley::grid_offset(p.offset, p.range));
    let held = Held::Pulley(p);
    lift_put_back(lv, &held.put_back(pos));
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
    // The magnet, a block over the anchor, glides with it.
    let mut blocks = p.blocks.clone();
    if let Some(m) = state_with(&*lv.host, "create:pulley_magnet", &[]) {
        blocks.push(([0, 1, 0], m));
    }
    let origin = [
        p.anchor[0] - pos[0],
        p.anchor[1] + p.initial - pos[1],
        p.anchor[2] - pos[2],
    ];
    draw::show_glide(
        lv,
        pos,
        draw::Glide {
            origin,
            blocks: &blocks,
            dir: [0, -1, 0],
            offset: p.offset,
            per_tick: if p.waiting {
                0.0
            } else {
                create_engine::pulley::movement(p.rpm)
            },
            clamp: Some((0.0, p.range as f32)),
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
            if settled {
                l.travelling.remove(&pos);
            }
            Some((t.reached.map(|y| [column.x, y, column.z]), settled))
        })
        .flatten();
        let Some((reached, settled)) = done else {
            with_world(|w| w.dims[dim].lifts.travelling.remove(&pos));
            continue;
        };
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
    let carried = actors::carry(lv, pos, anchor, &blocks, |_| false);
    if !carried.actors.is_empty() || !carried.controls.is_empty() || !carried.glue.is_empty() {
        PULLEY_CARRIED.with_borrow_mut(|c| c.insert((dim, pos), carried));
    }
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
    let held = Held::Elevator(e);
    lift_put_back(lv, &held.put_back(pos));
    if let Some(mut carried) = PULLEY_CARRIED.with_borrow_mut(|c| c.remove(&(dim, pos))) {
        actors::release(lv, pos, origin, &mut carried);
    }
    draw::hide(lv, pos);
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
        match block {
            Block::RedstoneContact if info.powered != valid => {
                lv.set_flags(pos, &[("powered", valid)], update_all());
            }
            Block::ElevatorContact => {
                let signal = lv.has_neighbor_signal(pos);
                if info.powered != signal {
                    lv.set_flags(pos, &[("powered", signal)], UPDATE_CLIENTS);
                    if signal && !info.calling {
                        call_contact(lv, pos, facing);
                    }
                }
                // `updateShape` (the block it faces changed): a contact an elevator lit keeps
                // its light through other neighbour changes.
                let held =
                    with_world(|w| w.dims[dim].lifts.contact_held.contains(&pos)) == Some(true);
                if info.powering != valid && !(info.powering && held) {
                    with_world(|w| w.dims[dim].lifts.schedule(pos, now));
                }
            }
            _ => {}
        }
    }
    for (pos, _) in due {
        let Some((Block::ElevatorContact, info)) = create_info(lv.block(pos)) else {
            continue;
        };
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
fn call_contact(lv: &mut ServerLevel<'_>, pos: [i32; 3], facing: Dir) {
    let dim = lv.dim;
    lv.set_flags(pos, &[("calling", true), ("powered", true)], UPDATE_CLIENTS);
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
        let text = format!(
            "create:redstone_contact[facing={},powered={powered}]",
            dir_name(facing)
        );
        Some(level.block_state_id(&text))
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
            // What its storage blocks hold (`MountedStorageManager`).
            if let Some(t) = actors::storage_to_nbt(lv, *pos) {
                c.insert("storage".into(), t);
            }
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
