//! The level tick of latches, controller rails, brass diodes, rose quartz lamps, cuckoo clocks
//! and redstone links: what changed since the last tick (the engine's queues, filled by the
//! blocks' changes and their neighbours'), read from the world and written back. Nothing runs
//! when nothing is queued.

use super::{block_at, direction, offset};
use crate::glue::net;
use crate::glue::persist;
use crate::glue::server::{ServerLevel, UPDATE_CLIENTS, update_all};
use crate::glue::world::with_world;
use create_engine::Block;
use create_engine::kinetics::Dir;
use mod_api::{NeighbourUpdate, PlayerRef};
use std::collections::HashSet;

/// A latch's rules and state; None for any other block.
pub(crate) fn latch_of(
    block: Block,
    info: &create_engine::parts::StateInfo,
) -> Option<(create_engine::latch::Kind, create_engine::latch::Latch)> {
    let kind = create_engine::world::latch_kind(block)?;
    Some((
        kind,
        create_engine::latch::Latch {
            facing: info.facing?,
            powered: info.powered,
            powering: info.powering,
            powered_side: info.powered_side,
        },
    ))
}

/// A brass diode's kind and state; None for any other block.
pub(crate) fn diode_of(
    block: Block,
    info: &create_engine::parts::StateInfo,
) -> Option<(create_engine::diode::Kind, create_engine::diode::State)> {
    let kind = create_engine::world::diode_kind(block)?;
    Some((
        kind,
        create_engine::diode::State {
            facing: info.facing?,
            powered: info.powered,
            powering: info.powering,
            inverted: info.inverted,
        },
    ))
}

/// A rose quartz lamp's state; None for any other block.
pub(crate) fn lamp_of(
    block: Block,
    info: &create_engine::parts::StateInfo,
) -> Option<create_engine::lamp::Lamp> {
    (block == Block::RoseQuartzLamp).then_some(create_engine::lamp::Lamp {
        powered: info.powered,
        powering: info.powering,
        activate: info.activate,
    })
}

/// A redstone link's `facing`, `receiver` and `powered`; None for any other block.
pub(crate) fn link_of(
    block: Block,
    info: &create_engine::parts::StateInfo,
) -> Option<(Dir, bool, bool)> {
    (block == Block::RedstoneLink).then_some((info.facing?, info.receiver, info.powered))
}

/// A controller rail's shape, way and power; None for any other block.
pub(crate) fn rail_of(
    block: Block,
    info: &create_engine::parts::StateInfo,
) -> Option<create_engine::rail::Rail> {
    (block == Block::ControllerRail).then(|| create_engine::rail::Rail {
        shape: info.rail.unwrap_or_default(),
        backwards: info.backwards,
        power: info.power,
    })
}

// ---------------------------------------------------------------------------------------------
// Latches and diodes

/// Sets a latch's new state (`setBlock(.., UPDATE_CLIENTS)`), then the blocks it powers read it
/// again (`DiodeBlock.onPlace` -> `updateNeighborsInFront`).
pub(crate) fn set_latch(lv: &mut ServerLevel<'_>, pos: [i32; 3], l: create_engine::latch::Latch) {
    let has_side = lv
        .block_state(pos)
        .is_some_and(|s| lv.state_property(s, "powered_side").is_some());
    let mut props = vec![("powered", l.powered), ("powering", l.powering)];
    if has_side {
        props.push(("powered_side", l.powered_side));
    }
    lv.set_flags(pos, &props, UPDATE_CLIENTS);
    update_in_front(lv, pos, l.facing);
}

/// `DiodeBlock.updateNeighborsInFront`: the block the diode at `pos` (input side `facing`)
/// powers reads it again, then that block's neighbours but the diode.
fn update_in_front(lv: &mut ServerLevel<'_>, pos: [i32; 3], facing: Dir) {
    let Some(state) = lv.block_state(pos) else {
        return;
    };
    let front = offset(pos, facing.opposite());
    let _ = lv.level.neighbor_changed(&mut *lv.host, front, state);
    let _ = lv.level.update_neighbors(
        &mut *lv.host,
        front,
        state,
        NeighbourUpdate::NeighboursExcept(direction(facing)),
    );
}

/// Whether a diode at `pos` is powered from its side `d` (`DiodeBlock.getInputSignal` > 0 from
/// there): a signal toward it, or redstone dust there with power.
pub(crate) fn powered_from(lv: &ServerLevel<'_>, pos: [i32; 3], d: Dir) -> bool {
    let at = offset(pos, d);
    if lv.signal(at, direction(d)) > 0 {
        return true;
    }
    let Some(state) = lv.block_state(at) else {
        return false;
    };
    lv.state_name(state).as_deref() == Some("minecraft:redstone_wire")
        && lv.state_property(state, "power").is_some_and(|v| v != "0")
}

/// `DiodeBlock.getInputSignal` > 0 at the latch's back, and a powered latch's
/// `isPoweredOnSides`.
fn latch_inputs(lv: &ServerLevel<'_>, pos: [i32; 3], facing: Dir) -> (bool, bool) {
    let back = powered_from(lv, pos, facing);
    // getClockWise / getCounterClockWise.
    let (left, right) = match facing {
        Dir::North => (Dir::East, Dir::West),
        Dir::East => (Dir::South, Dir::North),
        Dir::South => (Dir::West, Dir::East),
        _ => (Dir::North, Dir::South),
    };
    let side = powered_from(lv, pos, left) || powered_from(lv, pos, right);
    (back, side)
}

/// Latches: their due ticks (`tick`), then the ones whose neighbours changed
/// (`checkTickOnNeighbor`), which take a tick one game tick later. Nothing runs without either.
pub(super) fn tick_latches(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let now = lv.game_time();
    let Some((due, checks)) = with_world(|w| {
        let d = &mut w.dims[dim];
        if d.latch_ticks.is_empty() && d.latch_checks.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let (due, later): (Vec<_>, Vec<_>) = d.latch_ticks.drain(..).partition(|t| t.1 <= now);
        d.latch_ticks = later;
        (due, std::mem::take(&mut d.latch_checks))
    }) else {
        return;
    };
    if due.is_empty() && checks.is_empty() {
        return;
    }
    let mut next = Vec::new();
    // Latches with a tick coming, to look up in one step (a counter's ripple re-checks many).
    let mut pending: HashSet<[i32; 3]> =
        with_world(|w| w.dims[dim].latch_ticks.iter().map(|t| t.0).collect()).unwrap_or_default();
    for (pos, _) in due {
        let Some((kind, l)) = block_at(lv, pos).and_then(|(b, i)| latch_of(b, &i)) else {
            continue;
        };
        let (back, side) = latch_inputs(lv, pos, l.facing);
        let (n, again) = create_engine::latch::tick(kind, &l, back, side);
        if n != l {
            set_latch(lv, pos, n);
        }
        if again && pending.insert(pos) {
            next.push((pos, now + 1));
        }
    }
    for pos in checks {
        let Some((kind, l)) = block_at(lv, pos).and_then(|(b, i)| latch_of(b, &i)) else {
            continue;
        };
        let (back, side) = latch_inputs(lv, pos, l.facing);
        if !pending.contains(&pos) && create_engine::latch::needs_tick(kind, &l, back, side) {
            pending.insert(pos);
            next.push((pos, now + 1));
        }
    }
    with_world(|w| w.dims[dim].latch_ticks.extend(next));
}

/// `BrassDiodeBlock.toggle`: `inverted` flips (`setBlock(.., UPDATE_ALL)`, its output with it)
/// and a lever clicks.
pub(crate) fn toggle_diode(lv: &mut ServerLevel<'_>, pos: [i32; 3], inverted: bool) {
    if lv.set_flags(pos, &[("inverted", !inverted)], update_all()) {
        lv.update_signal_neighbors(pos);
    }
    lv.play_block_sound(
        "minecraft:block.lever.click",
        pos,
        0.3,
        create_engine::diode::toggle_pitch(inverted),
    );
}

/// Brass diodes: their due block ticks (`DiodeBlock.tick`: `powered` follows the input), then
/// their block entities' ticks (`BrassDiodeBlockEntity.tick`: `powering`), then the ones whose
/// input may have changed (`checkTickOnNeighbor`), which take a block tick later. Nothing runs
/// without any of these.
pub(super) fn tick_diodes(lv: &mut ServerLevel<'_>) {
    use create_engine::diode;
    let dim = lv.dim;
    let now = lv.game_time();
    let Some((due, checks, running)) = with_world(|w| {
        let d = &mut w.dims[dim];
        let (due, later): (Vec<_>, Vec<_>) = d.diode_ticks.drain(..).partition(|t| t.1 <= now);
        d.diode_ticks = later;
        (
            due,
            std::mem::take(&mut d.diode_checks),
            !d.diode_running.is_empty(),
        )
    }) else {
        return;
    };
    if due.is_empty() && checks.is_empty() && !running {
        return;
    }
    let mut next = Vec::new();
    // The diodes whose clients count on from a new start (their input changed, or they were
    // placed or looked at again).
    let mut shown: Vec<[i32; 3]> = checks.iter().map(|c| c.0).collect();
    for (pos, _) in due {
        let Some((_, s)) = block_at(lv, pos).and_then(|(b, i)| diode_of(b, &i)) else {
            continue;
        };
        let input = powered_from(lv, pos, s.facing);
        let (powered, again) = diode::block_tick(s.powered, input);
        // `setBlock(.., UPDATE_CLIENTS)`: its output does not follow `powered`, so no neighbour
        // reads it again.
        if lv.set_flags(pos, &[("powered", powered)], UPDATE_CLIENTS) {
            shown.push(pos);
        }
        if again {
            next.push((pos, now + diode::BLOCK_DELAY));
        }
    }
    // `ServerLevel.tick` runs block ticks before block entities: a diode just `powered` above
    // (and so now running) ticks in this game tick too.
    if running || with_world(|w| !w.dims[dim].diode_running.is_empty()) == Some(true) {
        for (pos, powering) in with_world(|w| w.tick_diodes(dim)).unwrap_or_default() {
            // `setBlockAndUpdate`, then `DiodeBlock.onPlace` -> `updateNeighborsInFront`.
            if lv.set_flags(pos, &[("powering", powering)], update_all()) {
                lv.update_signal_neighbors(pos);
            }
            persist::mark_pos_unsaved(dim, pos);
        }
    }
    // The diodes with a block tick scheduled (`hasScheduledTick`), gathered once.
    let mut pending: HashSet<[i32; 3]> = if checks.is_empty() {
        HashSet::new()
    } else {
        let mut p: HashSet<[i32; 3]> =
            with_world(|w| w.dims[dim].diode_ticks.iter().map(|t| t.0).collect())
                .unwrap_or_default();
        p.extend(next.iter().map(|t: &([i32; 3], i64)| t.0));
        p
    };
    for (pos, delay) in checks {
        let Some((_, s)) = block_at(lv, pos).and_then(|(b, i)| diode_of(b, &i)) else {
            continue;
        };
        if !pending.contains(&pos) && diode::needs_tick(s.powered, powered_from(lv, pos, s.facing))
        {
            pending.insert(pos);
            next.push((pos, now + delay));
        }
    }
    if !next.is_empty() {
        with_world(|w| w.dims[dim].diode_ticks.extend(next));
    }
    for pos in shown {
        show_diode(lv, pos);
    }
}

/// The block entity data a brass diode's clients count its indicator on from: its count, delay
/// and input at a game time (`show_diode`).
pub(crate) const DIODE_SHOWN: &str = "DiodeShown";

/// Puts what the diode at `pos` holds now in its block entity for its clients
/// (`BrassDiodeBlockEntity`'s `sendData`; between them each client's block entity counts on by
/// itself, as Create's ticks on both sides).
pub(crate) fn show_diode(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(d) = with_world(|w| w.dims[dim].diodes.get(&pos).copied()).flatten() else {
        return;
    };
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let now = lv.game_time();
    let shown = vec![
        i64::from(d.state),
        i64::from(d.max),
        i64::from(d.powered),
        now,
    ];
    let data = mod_api::Compound::from([(DIODE_SHOWN.into(), mod_api::Tag::LongArray(shown))]);
    let _ = entity.merge_data(&mut *lv.host, data);
}

// ---------------------------------------------------------------------------------------------
// Rose quartz lamps

/// Rose quartz lamps: those just placed (`getStateForPlacement`: `powered` as placed), their
/// due ticks (`tick`), then the ones whose neighbours changed (`neighborChanged`): a new signal
/// turns off the rest of the lamp's cluster and lights it at its next tick. Nothing runs
/// without any of these.
pub(super) fn tick_lamps(lv: &mut ServerLevel<'_>) {
    use create_engine::lamp;
    let dim = lv.dim;
    let now = lv.game_time();
    let Some((placed, due, checks)) = with_world(|w| {
        let d = &mut w.dims[dim];
        let (due, later): (Vec<_>, Vec<_>) = d.lamp_ticks.drain(..).partition(|t| t.1 <= now);
        d.lamp_ticks = later;
        (
            std::mem::take(&mut d.lamp_placed),
            due,
            std::mem::take(&mut d.lamp_checks),
        )
    }) else {
        return;
    };
    if placed.is_empty() && due.is_empty() && checks.is_empty() {
        return;
    }
    let lamp_at =
        |lv: &ServerLevel<'_>, p: [i32; 3]| block_at(lv, p).and_then(|(b, i)| lamp_of(b, &i));
    for pos in placed {
        if lamp_at(lv, pos).is_none() {
            continue;
        }
        let signal = lv.has_neighbor_signal(pos);
        lv.set_flags(pos, &[("powered", signal)], UPDATE_CLIENTS);
    }
    for (pos, _) in due {
        let Some(l) = lamp_at(lv, pos) else {
            continue;
        };
        if let Some(n) = lamp::tick(&l) {
            lv.set_flags(
                pos,
                &[("activate", n.activate), ("powering", n.powering)],
                UPDATE_CLIENTS,
            );
        }
        lv.update_signal_neighbors(pos);
    }
    // `scheduleActivation`: a tick next game tick unless one is due.
    let schedule = |p: [i32; 3]| {
        with_world(|w| {
            let t = &mut w.dims[dim].lamp_ticks;
            if !t.iter().any(|t| t.0 == p) {
                t.push((p, now + 1));
            }
        });
    };
    for pos in checks {
        let Some(l) = lamp_at(lv, pos) else {
            continue;
        };
        let signal = lv.has_neighbor_signal(pos);
        match lamp::neighbor_changed(&l, signal) {
            lamp::Change::None => {}
            lamp::Change::Unpower => {
                lv.set_flags(pos, &[("powered", false)], UPDATE_CLIENTS);
            }
            lamp::Change::Activate => {
                let cluster = lamp::cluster(pos, |p| lamp_at(lv, p).is_some());
                for p in cluster {
                    lv.set_flags(p, &[("powering", false)], UPDATE_CLIENTS);
                    schedule(p);
                }
                lv.set_flags(
                    pos,
                    &[("activate", true), ("powered", true), ("powering", true)],
                    UPDATE_CLIENTS,
                );
                lv.update_signal_neighbors(pos);
                schedule(pos);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Controller rails

/// What `calculatePower` reads: the strongest signal beside a block, and controller rails.
struct RailWorld<'a, 'b>(&'a ServerLevel<'b>);

impl create_engine::rail::Probe for RailWorld<'_, '_> {
    fn signal(&self, pos: [i32; 3]) -> i32 {
        self.0.best_neighbor_signal(pos)
    }
    fn rail(&self, pos: [i32; 3]) -> Option<create_engine::rail::Rail> {
        block_at(self.0, pos).and_then(|(b, i)| rail_of(b, &i))
    }
}

/// Controller rails whose neighbour changed (`BaseRailBlock.neighborChanged` → `updateState`):
/// `power` is worked out again and set when it changed (`placeAndNotify`: the rails beside it
/// are told, and work theirs out in turn). Nothing runs without a change.
pub(super) fn tick_controller_rails(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].rail_checks)).unwrap_or_default();
    if checks.is_empty() {
        return;
    }
    checks.sort_unstable();
    checks.dedup();
    const POWERS: [&str; 16] = [
        "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15",
    ];
    for pos in checks {
        let Some(rail) = block_at(lv, pos).and_then(|(b, i)| rail_of(b, &i)) else {
            continue;
        };
        let power = create_engine::rail::power(&RailWorld(lv), pos);
        if power != rail.power {
            lv.set_property(
                pos,
                "power",
                POWERS[usize::from(power.min(15))],
                update_all(),
            );
            super::analog_touched(dim, pos);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Cuckoo clocks

/// Cuckoo clocks' animations, from the server to the clients: position, then the animation.
pub(crate) const CUCKOO_CHANNEL: &str = "create:cuckoo";

/// `CuckooClockBlockEntity.tick` on the server, after a level's tick: in the overworld (the only
/// `natural` dimension) turning clocks start the pig at noon and the creeper at dusk, the
/// mysterious one its surprise, and the clients are told; a surprise at its progress 50 breaks
/// the clock and blows up. Nothing runs without turning clocks.
pub(super) fn tick_cuckoos(lv: &mut ServerLevel<'_>) {
    if lv.dimension != "minecraft:overworld" {
        return;
    }
    let dim = lv.dim;
    let day_time = lv.level.day_time(&*lv.host).unwrap_or(0);
    let (hours, minutes) = create_engine::cuckoo::time(day_time, true);
    let Some(events) = with_world(|w| w.tick_cuckoos(dim, hours, minutes)) else {
        return;
    };
    if events.is_empty() {
        return;
    }
    let players = lv.players();
    for (pos, kind, boom) in events {
        if boom {
            // The surprise: `contraptions`' blast.
            lv.cuckoo_surprise(pos);
            continue;
        }
        let creeper = Some(create_engine::cuckoo::Animation::Creeper);
        if create_engine::cuckoo::due(hours, minutes, false) == creeper {
            crate::glue::advancements::award_placer_near(lv, pos, "cuckoo_clock", 32.0);
        }
        let mut m = Vec::with_capacity(13);
        for v in pos {
            m.extend_from_slice(&v.to_le_bytes());
        }
        m.push(kind.index());
        net::send_to_players(&mut *lv.host, &players, CUCKOO_CHANNEL, &m);
    }
}

// ---------------------------------------------------------------------------------------------
// Redstone links

/// `RedstoneLinkBlock.getPower`: the strongest signal into it from each side, and into each
/// side but its back as from above.
fn link_power(lv: &ServerLevel<'_>, pos: [i32; 3], facing: Dir) -> i32 {
    let mut power = 0;
    for d in Dir::ALL {
        let at = offset(pos, d);
        power = power.max(lv.signal(at, direction(d)));
        if d != facing.opposite() {
            power = power.max(lv.signal(at, direction(Dir::Up)));
        }
    }
    power
}

/// Redstone links: the ones whose input may have changed (`neighborChanged` schedules a tick,
/// `updateTransmittedSignal`: a transmitter's `powered` and what it sends; one that lost the
/// block behind it breaks), then the receivers whose network's signal changed (`tick`:
/// `powered` follows it, the block and the one behind it update). Nothing runs without either.
pub(super) fn tick_links(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let Some((checks, shown)) = with_world(|w| {
        let d = &mut w.dims[dim];
        (
            std::mem::take(&mut d.link_checks),
            std::mem::take(&mut d.link_shown),
        )
    }) else {
        return;
    };
    if !shown.is_empty() {
        let players = lv.players();
        send_links(lv, &players, &shown);
    }
    let mut done: HashSet<[i32; 3]> = HashSet::new();
    for pos in checks {
        if !done.insert(pos) {
            continue;
        }
        let Some((facing, receiver, powered)) = block_at(lv, pos).and_then(|(b, i)| link_of(b, &i))
        else {
            continue;
        };
        // `canSurvive`: the block behind it can't be replaced.
        let behind = offset(pos, facing.opposite());
        let replaceable = lv.block_state(behind).is_some_and(|s| {
            use mod_api::HostBlockStates;
            lv.host.block_states().replaceable(s) == Some(true)
        });
        if replaceable {
            lv.destroy_block(pos, true);
            continue;
        }
        if receiver {
            continue;
        }
        // The panels it is joined to: 15 while one is satisfied (`shouldBePoweredTristate`);
        // nothing changes while one of them is not loaded.
        let Some(panels) = crate::glue::stock::link_panels_power(lv, pos) else {
            continue;
        };
        let power = link_power(lv, pos, facing).max(if panels { 15 } else { 0 });
        if powered != (power > 0) {
            lv.set_property(
                pos,
                "powered",
                if power > 0 { "true" } else { "false" },
                UPDATE_CLIENTS,
            );
        }
        // Its column is saved when what it sends changed (the engine marks it).
        with_world(|w| w.link_transmit(dim, pos, power));
    }
    let Some(mut changes) = with_world(|w| std::mem::take(&mut w.dims[dim].link_changes)) else {
        return;
    };
    if changes.is_empty() {
        return;
    }
    changes.sort_unstable();
    changes.dedup();
    for pos in changes {
        let Some((_, true, powered)) = block_at(lv, pos).and_then(|(b, i)| link_of(b, &i)) else {
            continue;
        };
        let received = with_world(|w| w.dims[dim].links.received(pos)).unwrap_or(0);
        if powered != (received > 0) {
            // `setBlockAndUpdate`.
            lv.set_property(
                pos,
                "powered",
                if received > 0 { "true" } else { "false" },
                update_all(),
            );
        }
        lv.update_signal_neighbors(pos);
        // `updateSelfAndAttached`: `panelSupport.notifyPanels`.
        crate::glue::stock::link_panels_changed(lv, pos);
    }
}

/// Links' frequencies to `players` (none: the link is gone, its items hidden).
pub(super) fn send_links(lv: &mut ServerLevel<'_>, players: &[PlayerRef], links: &[[i32; 3]]) {
    if players.is_empty() || links.is_empty() {
        return;
    }
    let dim = lv.dim;
    let mut shown: Vec<create_engine::link::Shown> = Vec::new();
    for &pos in links {
        let facing = block_at(lv, pos)
            .and_then(|(b, i)| link_of(b, &i))
            .map_or(Dir::Up, |l| l.0);
        let key = with_world(|w| w.dims[dim].links.links.get(&pos).map(|l| l.key.clone()))
            .flatten()
            .unwrap_or_default();
        shown.push((pos, facing, key));
    }
    // Messages well under the size limit: 64 links each.
    for chunk in shown.chunks(64) {
        let m = create_engine::link::encode(chunk);
        net::send_to_players(&mut *lv.host, players, create_engine::link::CHANNEL, &m);
    }
    for contents in &shown {
        crate::glue::drawn::publish(
            lv,
            contents.0,
            crate::glue::drawn::snapshot([create_engine::link::encode(std::slice::from_ref(
                contents,
            ))]),
        );
    }
}

/// `RedstoneLinkBlock.toggleMode` (sneaking with an empty hand, or the wrench): transmitter to
/// receiver and back, `powered` as its neighbours power it; its link changes sides on the way.
pub(crate) fn toggle_link(lv: &mut ServerLevel<'_>, pos: [i32; 3], receiver: bool) {
    let dim = lv.dim;
    let powered = lv.has_neighbor_signal(pos);
    let Some(state) = lv.block_state(pos) else {
        return;
    };
    let next = {
        use mod_api::HostBlockStates;
        let facts = lv.host.block_states();
        let flag = |b: bool| if b { "true" } else { "false" };
        state
            .with(&facts, "receiver", flag(!receiver))
            .and_then(|s| s.with(&facts, "powered", flag(powered)))
    };
    if let Ok(next) = next {
        lv.set_state(pos, next, update_all());
    }
    // The receiver's `tick` sets `powered` from its network at once.
    with_world(|w| w.dims[dim].link_changes.push(pos));
    // Its panels see it listen or not (`isOutput`), and a transmitter sends with them.
    crate::glue::stock::link_panels_changed(lv, pos);
    with_world(|w| w.dims[dim].link_checks.push(pos));
}
