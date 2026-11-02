// Create's redstone in the game: redstone links, powered latches, brass diodes and rose quartz
// lamps. A sibling of the `create` module (`runtime.rs`).
use super::Game;
use super::create::*;
use super::create_logistics::*;
use create_engine::kinetics::Dir;
use create_engine::states;
use horizon_world::registry::BlockStateId;
use std::cell::RefCell;
use std::collections::HashMap;

/// A latch's rules and state, from the state table: None for any other state. Read with
/// `try_borrow`: the signal hooks ask from inside level updates the glue itself starts.
pub(super) fn latch_state(
    state: u32,
) -> Option<(create_engine::latch::Kind, create_engine::latch::Latch)> {
    states::index(state)?;
    WORLD.with(|w| {
        let w = w.try_borrow().ok()?;
        let (block, info) = w.as_ref()?.table.get(state)?;
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
    })
}

/// A brass diode's kind and block state, from the state table: None for any other state. Read
/// with `try_borrow`, as `latch_state`.
pub(super) fn diode_state(
    state: u32,
) -> Option<(create_engine::diode::Kind, create_engine::diode::State)> {
    states::index(state)?;
    WORLD.with(|w| {
        let w = w.try_borrow().ok()?;
        let (block, info) = w.as_ref()?.table.get(state)?;
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
    })
}

/// A rose quartz lamp's state; None for any other block.
pub(super) fn lamp_state(state: u32) -> Option<create_engine::lamp::Lamp> {
    states::index(state)?;
    WORLD.with(|w| {
        let w = w.try_borrow().ok()?;
        let (block, info) = w.as_ref()?.table.get(state)?;
        (block == create_engine::Block::RoseQuartzLamp).then_some(create_engine::lamp::Lamp {
            powered: info.powered,
            powering: info.powering,
            activate: info.activate,
        })
    })
}

/// Sets boolean properties of the block at `pos` (`setBlock` with `flags`); whether its state
/// changed.
pub(super) fn set_flags(
    game: &mut Game<'_>,
    dimension: &str,
    pos: [i32; 3],
    props: &[(&str, bool)],
    flags: i32,
) -> bool {
    let registry = game.pack.assets.registry;
    let current = game.world.block(pos[0], pos[1], pos[2]);
    let Some(mut s) = registry.state(current).cloned() else {
        return false;
    };
    for (k, v) in props {
        s.properties
            .insert((*k).into(), if *v { "true" } else { "false" }.to_owned());
    }
    let Some(id) = registry.id(&s) else {
        return false;
    };
    id != current && game.native_set_block(dimension, pos, id, flags)
}

/// A redstone link's `facing`, `receiver` and `powered`; None for any other block.
pub(super) fn link_state(state: u32) -> Option<(Dir, bool, bool)> {
    states::index(state)?;
    WORLD.with(|w| {
        let w = w.try_borrow().ok()?;
        let (block, info) = w.as_ref()?.table.get(state)?;
        (block == create_engine::Block::RedstoneLink).then_some((
            info.facing?,
            info.receiver,
            info.powered,
        ))
    })
}

/// `RedstoneLinkBlock.getPower`: the strongest signal into it from each side, and into each
/// side but its back as from above.
pub(super) fn link_power(game: &mut Game<'_>, pos: [i32; 3], facing: Dir) -> i32 {
    let mut power = 0;
    for d in Dir::ALL {
        let o = d.offset();
        let at = horizon_core::position::BlockPos::new(pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]);
        let dir = horizon_core::geometry::Direction::ALL[d as usize];
        power = power.max(game.with_level(|b, level, ticks| b.signal(level, ticks, at, dir)));
        if d != facing.opposite() {
            let up = horizon_core::geometry::Direction::ALL[Dir::Up as usize];
            power = power.max(game.with_level(|b, level, ticks| b.signal(level, ticks, at, up)));
        }
    }
    power
}

/// Redstone links: the ones whose input may have changed (`neighborChanged` schedules a tick,
/// `updateTransmittedSignal`: a transmitter's `powered` and what it sends; one that lost the
/// block behind it breaks), then the receivers whose network's signal changed (`tick`:
/// `powered` follows it, the block and the one behind it update). Nothing runs without either.
pub(super) fn tick_links(game: &mut Game<'_>, dimension: &str, dim: usize) {
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
        let players: Vec<u128> = game
            .native_server_players()
            .into_iter()
            .filter(|p| p.dimension == dimension)
            .map(|p| p.uuid)
            .collect();
        send_links(game, &players, dim, &shown);
    }
    let mut done: Vec<[i32; 3]> = Vec::new();
    for pos in checks {
        if done.contains(&pos) {
            continue;
        }
        done.push(pos);
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let Some((facing, receiver, powered)) = link_state(state) else {
            continue;
        };
        // `canSurvive`: the block behind it can't be replaced.
        let o = facing.opposite().offset();
        let behind = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
        let support = game.world.block(behind[0], behind[1], behind[2]);
        if game.res.behaviours().is_replaceable(support) {
            game.native_run_command(&format!(
                "execute in {dimension} run setblock {} {} {} air destroy",
                pos[0], pos[1], pos[2]
            ));
            continue;
        }
        if receiver {
            continue;
        }
        let power = link_power(game, pos, facing);
        if powered != (power > 0) {
            game.native_set_block_property_with(
                pos,
                "powered",
                if power > 0 { "true" } else { "false" },
                super::native_hooks::flags::UPDATE_CLIENTS,
            );
        }
        with_world(|w| w.link_transmit(dim, pos, power));
    }
    let Some(mut changes) = with_world(|w| std::mem::take(&mut w.dims[dim].link_changes)) else {
        return;
    };
    changes.sort();
    changes.dedup();
    for pos in changes {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let Some((_, true, powered)) = link_state(state) else {
            continue;
        };
        let received = with_world(|w| w.dims[dim].links.received(pos)).unwrap_or(0);
        if powered != (received > 0) {
            // `setBlockAndUpdate`.
            game.native_set_block_property_with(
                pos,
                "powered",
                if received > 0 { "true" } else { "false" },
                super::native_hooks::flags::UPDATE_ALL,
            );
        }
        game.native_update_signal_neighbors(dimension, pos);
    }
}

/// Links' frequencies to `players` (none: the link is gone, its items hidden).
pub(super) fn send_links(game: &mut Game<'_>, players: &[u128], dim: usize, links: &[[i32; 3]]) {
    let mut shown: Vec<create_engine::link::Shown> = Vec::new();
    for &pos in links {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let facing = link_state(state).map_or(Dir::Up, |l| l.0);
        let key = with_world(|w| w.dims[dim].links.links.get(&pos).map(|l| l.key.clone()))
            .flatten()
            .unwrap_or_default();
        shown.push((pos, facing, key));
    }
    // Messages well under the size limit: 64 links each.
    for chunk in shown.chunks(64) {
        let m = create_engine::link::encode(chunk);
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::link::CHANNEL, &m);
        }
    }
}

/// `LinkRenderer`: each slot's item drawn in it, a part that is uploaded when it is tuned.
pub(super) fn links_from_server(game: &mut Game<'_>, data: &[u8]) {
    for (pos, facing, key) in create_engine::link::decode(data) {
        for (first, item) in [(true, &key.0), (false, &key.1)] {
            let part = depot_key(pos, if first { 100 } else { 101 }, 0);
            game.native_part_clear(part);
            if item.is_empty() {
                continue;
            }
            let block_item = game.native_item_uses_block_light(item).unwrap_or(false);
            let pose = create_engine::link::slot_pose(facing, first, block_item);
            if let Some(model) = game.native_part_item_model(item, pose) {
                game.native_part_set(part, model, pos, 1, 0.0, 0.0);
            }
        }
    }
}

/// `RedstoneLinkBlock.toggleMode` (sneaking with an empty hand, or the wrench): transmitter to
/// receiver and back, `powered` as its neighbours power it; its link changes sides on the way.
pub(super) fn toggle_link(game: &mut Game<'_>, dim: usize, pos: [i32; 3], receiver: bool) {
    let at = horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]);
    let powered = game.with_level(|b, level, ticks| b.has_neighbor_signal(level, ticks, at));
    let registry = game.pack.assets.registry;
    let current = game.world.block(pos[0], pos[1], pos[2]);
    let Some(mut s) = registry.state(current).cloned() else {
        return;
    };
    let flag = |b: bool| if b { "true" } else { "false" }.to_owned();
    s.properties.insert("receiver".into(), flag(!receiver));
    s.properties.insert("powered".into(), flag(powered));
    let Some(id) = registry.id(&s) else {
        return;
    };
    let Some(dimension) = game.native_acting_player().map(|p| p.dimension) else {
        return;
    };
    game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
    // The receiver's `tick` sets `powered` from its network at once.
    with_world(|w| w.dims[dim].link_changes.push(pos));
}

/// `LinkHandler.onBlockActivated`: the item in hand (none: empty) tunes the slot under the
/// crosshair. Sneaking with an empty hand, or a wrench, toggles the link.
pub(super) fn use_link(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    link: (Dir, bool),
    back: bool,
    canceled: &mut bool,
) {
    // `player.getItemInHand(hand)`.
    let held = if game.hand == horizon_client::first_person::Hand::Off {
        game.offhand.as_ref().map(|s| s.item.to_string())
    } else {
        game.native_held().map(|h| h.0)
    };
    if held.as_deref() == Some("create:wrench") {
        if !back {
            // `onWrenched`.
            *canceled = true;
            toggle_link(game, dim, pos, link.1);
        }
        return;
    }
    if back {
        // `useWithoutItem` while sneaking: only with both hands empty does the block see it.
        if game.native_held().is_none() && game.offhand.is_none() {
            *canceled = true;
            toggle_link(game, dim, pos, link.1);
        }
        return;
    }
    if held.as_deref() == Some("create:linked_controller") {
        return;
    }
    let Some(hit) = game.pick() else {
        return;
    };
    let local = [
        hit.point[0] - f64::from(pos[0]),
        hit.point[1] - f64::from(pos[1]),
        hit.point[2] - f64::from(pos[2]),
    ];
    let Some(first) = create_engine::link::hit_slot(link.0, local) else {
        return;
    };
    *canceled = true;
    let item = held.unwrap_or_default();
    with_world(|w| w.link_frequency(dim, pos, first, &item));
}

/// Sets a latch's new state (`setBlock(.., UPDATE_CLIENTS)`), then the blocks it powers read it
/// again (`DiodeBlock.onPlace` -> `updateNeighborsInFront`).
pub(super) fn set_latch(
    game: &mut Game<'_>,
    dimension: &str,
    pos: [i32; 3],
    l: create_engine::latch::Latch,
) {
    let current = game.world.block(pos[0], pos[1], pos[2]);
    // The state for these flags, worked out once per (state, flags) and remembered: a latch
    // flipping every tick costs a lookup, not a block state built from strings.
    let flags = u8::from(l.powered) | u8::from(l.powering) << 1 | u8::from(l.powered_side) << 2;
    let known = LATCH_STATES.with_borrow(|m| m.get(&(current.0, flags)).copied());
    let id = match known {
        Some(id) => id,
        None => {
            let registry = game.pack.assets.registry;
            let Some(mut s) = registry.state(current).cloned() else {
                return;
            };
            let flag = |b: bool| if b { "true" } else { "false" }.to_owned();
            s.properties.insert("powered".into(), flag(l.powered));
            s.properties.insert("powering".into(), flag(l.powering));
            if s.properties.contains_key("powered_side") {
                s.properties
                    .insert("powered_side".into(), flag(l.powered_side));
            }
            let Some(id) = registry.id(&s) else {
                return;
            };
            LATCH_STATES.with_borrow_mut(|m| m.insert((current.0, flags), id));
            id
        }
    };
    game.native_set_block(
        dimension,
        pos,
        id,
        super::native_hooks::flags::UPDATE_CLIENTS,
    );
    update_in_front(game, dimension, pos, l.facing);
}

/// `DiodeBlock.updateNeighborsInFront`: the block the diode at `pos` (input side `facing`)
/// powers reads it again, then that block's neighbours but the diode. Latches tick on the server
/// only.
fn update_in_front(game: &mut Game<'_>, dimension: &str, pos: [i32; 3], facing: Dir) {
    use horizon_core::geometry::Direction;
    if dimension != game.dim.id() {
        return;
    }
    let o = facing.opposite().offset();
    let front = horizon_core::position::BlockPos::new(pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]);
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let skip = Direction::ALL[facing as usize];
    game.with_level(|b, level, ticks| {
        let block = b.block(state);
        b.neighbor_changed_at(level, ticks, front, block);
        b.update_neighbors_at_except(level, ticks, front, block, skip);
    });
}

thread_local! {
    /// `set_latch`'s states by (state, flags), and `powered_from`'s answer to "is this powered
    /// redstone dust" by state: both fixed for a world's block registry.
    pub(super) static LATCH_STATES: RefCell<HashMap<(u32, u8), BlockStateId>> =
        RefCell::new(HashMap::new());
    pub(super) static POWERED_WIRE: RefCell<HashMap<u32, bool>> = RefCell::new(HashMap::new());
}

/// Whether a diode at `pos` is powered from its side `d` (`DiodeBlock.getInputSignal` > 0 from
/// there): a signal toward it, or redstone dust there with power.
pub(super) fn powered_from(game: &mut Game<'_>, pos: [i32; 3], d: Dir) -> bool {
    let o = d.offset();
    let at = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
    let dir = horizon_core::geometry::Direction::ALL[d as usize];
    let bp = horizon_core::position::BlockPos::new(at[0], at[1], at[2]);
    if game.with_level(|b, level, ticks| b.signal(level, ticks, bp, dir)) > 0 {
        return true;
    }
    let state = game.world.block(at[0], at[1], at[2]);
    if let Some(known) = POWERED_WIRE.with_borrow(|m| m.get(&state.0).copied()) {
        return known;
    }
    let wire = game.pack.assets.registry.state(state).is_some_and(|s| {
        s.name == "minecraft:redstone_wire" && s.properties.get("power").is_some_and(|v| v != "0")
    });
    POWERED_WIRE.with_borrow_mut(|m| m.insert(state.0, wire));
    wire
}

/// `DiodeBlock.getInputSignal` > 0 at the latch's back, and a powered latch's
/// `isPoweredOnSides`.
pub(super) fn latch_inputs(game: &mut Game<'_>, pos: [i32; 3], facing: Dir) -> (bool, bool) {
    let back = powered_from(game, pos, facing);
    // getClockWise / getCounterClockWise.
    let (left, right) = match facing {
        Dir::North => (Dir::East, Dir::West),
        Dir::East => (Dir::South, Dir::North),
        Dir::South => (Dir::West, Dir::East),
        _ => (Dir::North, Dir::South),
    };
    let side = powered_from(game, pos, left) || powered_from(game, pos, right);
    (back, side)
}

/// Latches: their due ticks (`tick`), then the ones whose neighbours changed
/// (`checkTickOnNeighbor`), which take a tick one game tick later. Nothing runs without either.
pub(super) fn tick_latches(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let now = game.world.game_time;
    let Some((due, checks)) = with_world(|w| {
        let d = &mut w.dims[dim];
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
    let mut pending: std::collections::HashSet<[i32; 3]> =
        with_world(|w| w.dims[dim].latch_ticks.iter().map(|t| t.0).collect()).unwrap_or_default();
    for (pos, _) in due {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let Some((kind, l)) = latch_state(state) else {
            continue;
        };
        let (back, side) = latch_inputs(game, pos, l.facing);
        let (n, again) = create_engine::latch::tick(kind, &l, back, side);
        if n != l {
            set_latch(game, dimension, pos, n);
        }
        if again && pending.insert(pos) {
            next.push((pos, now + 1));
        }
    }
    for pos in checks {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let Some((kind, l)) = latch_state(state) else {
            continue;
        };
        let (back, side) = latch_inputs(game, pos, l.facing);
        if !pending.contains(&pos) && create_engine::latch::needs_tick(kind, &l, back, side) {
            pending.insert(pos);
            next.push((pos, now + 1));
        }
    }
    with_world(|w| w.dims[dim].latch_ticks.extend(next));
}

/// `BrassDiodeBlock.toggle`: `inverted` flips (`setBlock(.., UPDATE_ALL)`, its output with it)
/// and a lever clicks.
pub(super) fn toggle_diode(game: &mut Game<'_>, dimension: &str, pos: [i32; 3], inverted: bool) {
    if set_flags(
        game,
        dimension,
        pos,
        &[("inverted", !inverted)],
        super::native_hooks::flags::UPDATE_ALL,
    ) {
        game.native_update_signal_neighbors(dimension, pos);
    }
    let center = pos.map(|v| f64::from(v) + 0.5);
    game.audio.play(
        "minecraft:block.lever.click",
        Some(center),
        0.3,
        create_engine::diode::toggle_pitch(inverted),
    );
}

/// Brass diodes: their due block ticks (`DiodeBlock.tick`: `powered` follows the input), then
/// their block entities' ticks (`BrassDiodeBlockEntity.tick`: `powering`), then the ones whose
/// input may have changed (`checkTickOnNeighbor`), which take a block tick later. Nothing runs
/// without any of these.
pub(super) fn tick_diodes(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::diode;
    let now = game.world.game_time;
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
    for (pos, _) in due {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let Some((_, s)) = diode_state(state) else {
            continue;
        };
        let input = powered_from(game, pos, s.facing);
        let (powered, again) = diode::block_tick(s.powered, input);
        // `setBlock(.., UPDATE_CLIENTS)`: its output does not follow `powered`, so no neighbour
        // reads it again.
        set_flags(
            game,
            dimension,
            pos,
            &[("powered", powered)],
            super::native_hooks::flags::UPDATE_CLIENTS,
        );
        if again {
            next.push((pos, now + diode::BLOCK_DELAY));
        }
    }
    // `ServerLevel.tick` runs block ticks before block entities: a diode just `powered` above
    // (and so now running) ticks in this game tick too.
    if running || with_world(|w| !w.dims[dim].diode_running.is_empty()) == Some(true) {
        for (pos, powering) in with_world(|w| w.tick_diodes(dim)).unwrap_or_default() {
            // `setBlockAndUpdate`, then `DiodeBlock.onPlace` -> `updateNeighborsInFront`.
            if set_flags(
                game,
                dimension,
                pos,
                &[("powering", powering)],
                super::native_hooks::flags::UPDATE_ALL,
            ) {
                game.native_update_signal_neighbors(dimension, pos);
            }
        }
    }
    // The diodes with a block tick scheduled (`hasScheduledTick`), gathered once.
    let mut pending: std::collections::HashSet<[i32; 3]> = if checks.is_empty() {
        Default::default()
    } else {
        with_world(|w| w.dims[dim].diode_ticks.iter().map(|t| t.0).collect()).unwrap_or_default()
    };
    if !checks.is_empty() {
        pending.extend(next.iter().map(|t| t.0));
    }
    for (pos, delay) in checks {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let Some((_, s)) = diode_state(state) else {
            continue;
        };
        if !pending.contains(&pos)
            && diode::needs_tick(s.powered, powered_from(game, pos, s.facing))
        {
            pending.insert(pos);
            next.push((pos, now + delay));
        }
    }
    if !next.is_empty() {
        with_world(|w| w.dims[dim].diode_ticks.extend(next));
    }
}

/// `Level.hasNeighborSignal` at `pos`.
pub(super) fn neighbor_signal(game: &mut Game<'_>, pos: [i32; 3]) -> bool {
    let at = horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]);
    game.with_level(|b, level, ticks| b.has_neighbor_signal(level, ticks, at))
}

/// Rose quartz lamps: those just placed (`getStateForPlacement`: `powered` as placed), their
/// due ticks (`tick`), then the ones whose neighbours changed (`neighborChanged`): a new signal
/// turns off the rest of the lamp's cluster and lights it at its next tick. Nothing runs
/// without any of these.
pub(super) fn tick_lamps(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::lamp;
    let now = game.world.game_time;
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
    let flags = super::native_hooks::flags::UPDATE_CLIENTS;
    for pos in placed {
        if lamp_state(game.world.block(pos[0], pos[1], pos[2]).0).is_none() {
            continue;
        }
        let signal = neighbor_signal(game, pos);
        set_flags(game, dimension, pos, &[("powered", signal)], flags);
    }
    for (pos, _) in due {
        let Some(l) = lamp_state(game.world.block(pos[0], pos[1], pos[2]).0) else {
            continue;
        };
        if let Some(n) = lamp::tick(&l) {
            set_flags(
                game,
                dimension,
                pos,
                &[("activate", n.activate), ("powering", n.powering)],
                flags,
            );
        }
        game.native_update_signal_neighbors(dimension, pos);
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
        let Some(l) = lamp_state(game.world.block(pos[0], pos[1], pos[2]).0) else {
            continue;
        };
        let signal = neighbor_signal(game, pos);
        match lamp::neighbor_changed(&l, signal) {
            lamp::Change::None => {}
            lamp::Change::Unpower => {
                set_flags(game, dimension, pos, &[("powered", false)], flags);
            }
            lamp::Change::Activate => {
                let cluster = lamp::cluster(pos, |p| {
                    lamp_state(game.world.block(p[0], p[1], p[2]).0).is_some()
                });
                for p in cluster {
                    set_flags(game, dimension, p, &[("powering", false)], flags);
                    schedule(p);
                }
                set_flags(
                    game,
                    dimension,
                    pos,
                    &[("activate", true), ("powered", true), ("powering", true)],
                    flags,
                );
                game.native_update_signal_neighbors(dimension, pos);
                schedule(pos);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Analog levers, smart observers, threshold switches and cuckoo clocks.

/// An analog lever's, smart observer's or threshold switch's block and state, from the state
/// table: None for any other block. Read with `try_borrow`, as `latch_state`.
pub(super) fn detector_block(
    state: u32,
) -> Option<(create_engine::Block, create_engine::parts::StateInfo)> {
    use create_engine::Block;
    states::index(state)?;
    WORLD.with(|w| {
        let w = w.try_borrow().ok()?;
        let (block, info) = w.as_ref()?.table.get(state)?;
        matches!(
            block,
            Block::AnalogLever | Block::ContentObserver | Block::StockpileSwitch
        )
        .then_some((block, *info))
    })
}

/// `getSignal` / `getDirectSignal` of an analog lever (its block entity's state), a smart
/// observer (15 while `powered`) or a threshold switch (15 after its delay), none into an
/// observer's or switch's target. `side` is vanilla's direction.
pub(super) fn detector_signal(
    dim: usize,
    pos: [i32; 3],
    block: create_engine::Block,
    info: &create_engine::parts::StateInfo,
    side: Dir,
    direct: bool,
) -> i32 {
    use create_engine::{Block, lever, observe};
    let facing = info.facing.unwrap_or(Dir::North);
    let read = |f: &dyn Fn(&create_engine::world::Dimension) -> i32| {
        WORLD.with(|w| {
            w.try_borrow()
                .ok()
                .and_then(|w| w.as_ref().map(|w| f(&w.dims[dim])))
                .unwrap_or(0)
        })
    };
    let target = || {
        observe::target_direction(
            info.target.unwrap_or(create_engine::steam::Face::Wall),
            facing,
        )
    };
    match block {
        Block::AnalogLever => {
            let face = info.face.unwrap_or(create_engine::steam::Face::Floor);
            let connected = lever::connected(face, facing);
            read(&|d| lever::signal(d.levers.state(pos), connected, side, direct))
        }
        _ if direct => 0,
        Block::ContentObserver => observe::observer_signal(info.powered, target(), side),
        _ => {
            let target = target();
            read(&|d| {
                d.detectors
                    .switches
                    .get(&pos)
                    .map_or(0, |s| s.signal(target, side))
            })
        }
    }
}

/// `AnalogLeverBlock.useWithoutItem` (any item but the wrench, `ItemUseOverrides`): one up, or
/// down while sneaking; the click's sound at the new state's pitch.
pub(super) fn use_lever(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    back: bool,
    canceled: &mut bool,
) {
    if game.native_held().is_some_and(|h| h.0 == "create:wrench") {
        return;
    }
    *canceled = true;
    let now = game.world.game_time;
    let Some(state) = with_world(|w| {
        let s = w.dims[dim].levers.click(pos, back, now);
        w.dims[dim].unsaved.push((pos[0] >> 4, pos[2] >> 4));
        s
    })
    .flatten() else {
        return;
    };
    let center = pos.map(|v| f64::from(v) + 0.5);
    game.audio.play(
        "minecraft:block.lever.click",
        Some(center),
        0.2,
        create_engine::lever::click_pitch(state),
    );
}

/// A click on a smart observer or threshold switch: on its filter slot
/// (`FilteredDetectorFilterSlot`, `FilteringBehaviour.onShortInteract`) the held item becomes
/// its filter, an empty hand clears it; elsewhere a switch opens its screen (none here: the
/// thresholds are set with `/create_debug threshold`), which takes the click all the same.
pub(super) fn use_detector(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    face: [i32; 3],
    switch: bool,
    info: &create_engine::parts::StateInfo,
    canceled: &mut bool,
) {
    use create_engine::observe;
    let held = game.native_held().map(|h| h.0);
    if held.as_deref() == Some("create:wrench") {
        return;
    }
    let side = Dir::from_offset(face);
    let on_slot = may_use_value_box(game)
        && held.as_deref() != Some("create:mechanical_arm")
        && match (side, game.pick(), info.target, info.facing) {
            (Some(side), Some(hit), Some(t), Some(f)) => {
                let local = [0, 1, 2].map(|i| hit.point[i] - f64::from(pos[i]));
                observe::filter_side(t, f, side, switch)
                    && create_engine::board::hits_box(observe::filter_slot(side), local)
            }
            _ => false,
        };
    if on_slot {
        *canceled = true;
        with_world(|w| {
            w.dims[dim].detectors.set_filter(pos, held);
            w.dims[dim].unsaved.push((pos[0] >> 4, pos[2] >> 4));
        });
        let center = pos.map(|v| f64::from(v) + 0.5);
        game.audio.play(
            "minecraft:entity.item_frame.add_item",
            Some(center),
            0.25,
            0.1,
        );
        return;
    }
    // ThresholdSwitchBlock.useItemOn: SUCCESS (the screen, on the client).
    if switch {
        *canceled = true;
    }
}

/// Whether a smart observer (`observer`) or threshold switch placed beside `at` would look at it
/// (`getStateForPlacement`): an item or fluid handler, and for the observer a belt, depot,
/// ejector, fluid pipe or funnel too.
fn detects(game: &Game<'_>, at: [i32; 3], observer: bool) -> bool {
    use create_engine::Block;
    let state = game.world.block(at[0], at[1], at[2]).0;
    if states::index(state).is_some() {
        let block = with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten();
        if matches!(block, Some(Block::FluidTank | Block::ItemVault)) {
            return true;
        }
        if observer
            && matches!(
                block,
                Some(
                    Block::Belt
                        | Block::Depot
                        | Block::WeightedEjector
                        | Block::FluidPipe
                        | Block::AndesiteFunnel
                        | Block::BrassFunnel
                        | Block::AndesiteBeltFunnel
                        | Block::BrassBeltFunnel
                )
            )
        {
            return true;
        }
    }
    super::machines::create_extract::is_container(game, at)
}

/// `SmartObserverBlock.getStateForPlacement` / `ThresholdSwitchBlock`'s: its target and facing
/// set at once.
pub(super) fn place_detector(
    game: &mut Game<'_>,
    pos: [i32; 3],
    state: BlockStateId,
    switch: bool,
) {
    let nearest = create_engine::place::ordered_by_nearest(game.yaw, game.pitch);
    let horizontal = create_engine::place::horizontal_direction(game.yaw);
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let (target, facing) = {
        let g = &*game;
        create_engine::observe::placement(nearest, horizontal, sneaking, |d| {
            let o = d.offset();
            detects(g, [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]], !switch)
        })
    };
    let registry = game.pack.assets.registry;
    let Some(id) = registry.state(state).cloned().and_then(|mut s| {
        s.properties.insert("target".into(), target.name().into());
        s.properties
            .insert("facing".into(), dir_name(facing).into());
        registry.id(&s)
    }) else {
        return;
    };
    if id != state {
        let dimension = game.dim.id().to_owned();
        game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
    }
}

/// What the detector at `pos` finds at `target`, and how it is watched from now on. The
/// target's own item id is read only for an observer with a filter.
fn seen_at(
    game: &Game<'_>,
    dim: usize,
    target: [i32; 3],
    block_item: bool,
) -> (create_engine::observe::Seen, create_engine::observe::Watch) {
    use create_engine::Block;
    use create_engine::observe::{CENTRE_REACH, Seen, Slot, Watch};
    let state = game.world.block(target[0], target[1], target[2]);
    let mut seen = Seen {
        block_item: if block_item {
            game.pack
                .assets
                .registry
                .state(state)
                .map(|s| s.name.clone())
                .filter(|n| n != "minecraft:air")
        } else {
            None
        },
        ..Seen::default()
    };
    let block = if states::index(state.0).is_some() {
        with_world(|w| w.table.get(state.0).map(|(b, _)| b)).flatten()
    } else {
        None
    };
    match block {
        // `TransportedItemStackHandlerBehaviour.handleCenteredProcessingOnAllItems(.45)`: the
        // items within .45 of the segment's centre.
        Some(Block::Belt) => {
            let items = with_world(|w| {
                let d = &w.dims[dim];
                let c = d.belt_of.get(&target)?;
                let belt = d.belts.get(c)?;
                let index = belt.chain.iter().position(|q| *q == target)? as f32;
                Some(
                    belt.items
                        .items
                        .iter()
                        .filter(|i| (index + 0.5 - i.position).abs() < CENTRE_REACH)
                        .map(|i| i.item.clone())
                        .collect::<Vec<String>>(),
                )
            })
            .flatten()
            .unwrap_or_default();
            seen.transported = Some(items);
            (seen, Watch::Belt)
        }
        // `DepotBehaviour.applyToAllItems`: the held stack once within .45 of the centre (a
        // stack still sliding in is looked at again each tick).
        Some(Block::Depot | Block::WeightedEjector) => {
            let held = with_world(|w| w.dims[dim].depots.get(&target).copied().flatten()).flatten();
            let item = depot_items(game, target)
                .into_iter()
                .find(|s| s.0 == 0)
                .map(|s| s.1);
            let near = held.is_none_or(|h| 0.5 - h.position <= CENTRE_REACH);
            seen.transported = Some(item.filter(|_| near).into_iter().collect());
            (seen, if near { Watch::Notified } else { Watch::Belt })
        }
        // An item vault's whole inventory (`getSlotLimit` 64).
        Some(Block::ItemVault) => {
            if let Some(parts) = vault_at(game, dim, target) {
                let slots = vault_slots(game, &parts)
                    .into_iter()
                    .map(|s| match s {
                        Some(s) => Slot {
                            max_stack: u32::from(stack_limit(game, &s.item)),
                            item: Some(s.item.to_string()),
                            count: s.count,
                        },
                        None => Slot {
                            item: None,
                            count: 0,
                            max_stack: 64,
                        },
                    })
                    .collect();
                seen.inventory = Some((slots, 64));
            }
            (seen, Watch::Notified)
        }
        // A fluid tank's fluid handler: the whole tank.
        Some(Block::FluidTank) => {
            seen.tank = with_world(|w| {
                let (_, m) = w.dims[dim].tanks.get(target)?;
                Some((m.tank.fluid, m.tank.amount, m.capacity()))
            })
            .flatten();
            (seen, Watch::Notified)
        }
        // Another Create block's slots tell of their changes; a vanilla container does not.
        other => {
            seen.inventory = super::machines::create_extract::container_slots(game, target)
                .map(|slots| (slots, 64));
            let watch = if seen.inventory.is_some() && other.is_none() {
                Watch::Lazy
            } else {
                Watch::Notified
            };
            (seen, watch)
        }
    }
}

/// A smart observer's or threshold switch's look at its target (`SmartObserverBlockEntity
/// .tick`, `ThresholdSwitchBlockEntity.updateCurrentLevel`).
fn look(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3], now: i64) {
    use create_engine::{Block, observe};
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    let Some((block, info)) = detector_block(state) else {
        return;
    };
    let (Some(t), Some(f)) = (info.target, info.facing) else {
        return;
    };
    let target = observe::target_pos(pos, t, f);
    let filter = with_world(|w| w.dims[dim].detectors.filter(pos).map(str::to_owned)).flatten();
    match block {
        Block::ContentObserver => {
            let (seen, watch) = seen_at(game, dim, target, filter.is_some());
            let found = observe::observe(filter.as_deref(), &seen);
            let effect = with_world(|w| {
                let d = &mut w.dims[dim].detectors;
                d.watch(pos, watch, now);
                d.observer_look(pos, found, now, info.powered)
            })
            .unwrap_or_default();
            if effect.power_on {
                // `activate`: `setBlockAndUpdate`, then `updateNeighborsAt`.
                set_flags(
                    game,
                    dimension,
                    pos,
                    &[("powered", true)],
                    super::native_hooks::flags::UPDATE_ALL,
                );
                game.native_update_signal_neighbors(dimension, pos);
            }
        }
        Block::StockpileSwitch => {
            let (seen, watch) = seen_at(game, dim, target, false);
            let reading = observe::fill_level(filter.as_deref(), &seen);
            // A belt is no inventory: the switch looks at it as at any other block.
            let watch = if watch == observe::Watch::Belt {
                observe::Watch::Notified
            } else {
                watch
            };
            let Some(u) = with_world(|w| {
                let d = &mut w.dims[dim];
                d.detectors.watch(pos, watch, now);
                let s = d.detectors.switches.get_mut(&pos)?;
                let before = (s.level, s.state);
                let u = s.update_level(reading);
                if (s.level, s.state) != before {
                    d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
                }
                if u.schedule {
                    d.detectors.schedule_switch(pos, now);
                }
                Some(u)
            })
            .flatten() else {
                return;
            };
            if let Some(level) = u.display
                && level != info.level
            {
                let flags = if u.update {
                    super::native_hooks::flags::UPDATE_ALL
                } else {
                    super::native_hooks::flags::UPDATE_CLIENTS
                };
                game.native_set_block_property_with(pos, "level", &level.to_string(), flags);
            }
        }
        _ => {}
    }
}

/// Analog levers whose neighbours are due to hear of them, smart observers and threshold
/// switches whose targets changed (or whose lazy interval came round, or whose belt or depot
/// carries items), their turn-offs and output ticks. Nothing runs without any of these.
pub(super) fn tick_detectors(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let now = game.world.game_time;
    let Some((due, removed, shown)) = with_world(|w| {
        w.detectors_tanks_changed(dim);
        let l = &mut w.dims[dim].levers;
        (
            l.take_due(now),
            std::mem::take(&mut l.removed),
            std::mem::take(&mut l.shown),
        )
    }) else {
        return;
    };
    // `AnalogLeverBlockEntity.tick` (`lastChange` ran out) and `onRemove`: `updateNeighbors`.
    for pos in due.into_iter().chain(removed) {
        game.native_update_signal_neighbors(dimension, pos);
    }
    if !shown.is_empty() {
        let players = players_in(game, dimension);
        send_levers(game, &players, dim, &shown);
    }
    let Some((checks, ticks)) = with_world(|w| {
        let d = &mut w.dims[dim];
        let mut checks = d.detectors.take_checks(now);
        let ticks = d.detectors.take_switch_ticks(now);
        if !d.detectors.belt_watch.is_empty() {
            // Observers on a belt (or a depot a stack slides onto) that holds items look
            // each tick.
            for p in &d.detectors.belt_watch {
                let busy = d.detectors.target_of.get(p).is_some_and(|t| {
                    d.belt_of
                        .get(t)
                        .and_then(|c| d.belts.get(c))
                        .is_some_and(|b| !b.items.items.is_empty())
                        || d.depots.get(t).is_some_and(|h| h.is_some())
                });
                if busy {
                    checks.push(*p);
                }
            }
            checks.sort();
            checks.dedup();
        }
        (checks, ticks)
    }) else {
        return;
    };
    // `ThresholdSwitchBlock.tick` -> `updatePowerAfterDelay`: the output follows, then
    // `blockUpdated`.
    for pos in ticks {
        let follows = with_world(|w| {
            w.dims[dim]
                .detectors
                .switches
                .get_mut(&pos)
                .map(|s| s.power_after_delay())
        })
        .flatten();
        if follows == Some(true) {
            game.native_update_signal_neighbors(dimension, pos);
        }
    }
    for pos in checks {
        look(game, dimension, dim, pos, now);
    }
    // `SmartObserverBlock.tick`: `powered` false (`setBlock(.., UPDATE_CLIENTS)`), then
    // `updateNeighborsAt`.
    for pos in with_world(|w| w.dims[dim].detectors.take_offs(now)).unwrap_or_default() {
        if set_flags(
            game,
            dimension,
            pos,
            &[("powered", false)],
            super::native_hooks::flags::UPDATE_CLIENTS,
        ) {
            game.native_update_signal_neighbors(dimension, pos);
        }
    }
}

/// `/create_debug threshold <pos> <offBelow> <onAbove> [inverted] [stacks]`, standing in for
/// the switch's screen (`ConfigureThresholdSwitchPacket.applySettings`), and
/// `/create_debug redstone_state <pos>`: what a lever, observer or switch holds.
pub(super) fn redstone_command(game: &mut Game<'_>, command: &str) -> bool {
    const USAGE: &str =
        "usage: create_debug threshold <x y z> <offBelow> <onAbove> [inverted] [stacks]";
    let set = command.strip_prefix("create_debug threshold ");
    let show = command.strip_prefix("create_debug redstone_state ");
    let Some(rest) = set.or(show) else {
        return false;
    };
    let player = game.native_acting_player();
    let dimension = player
        .as_ref()
        .map_or_else(|| "minecraft:overworld".to_owned(), |p| p.dimension.clone());
    let dim = create_engine::world::dimension_index(&dimension).unwrap_or(0);
    let here = player.as_ref().map(|p| p.position);
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let coord = |n: usize| -> Option<i32> {
        let t = tokens.get(n)?;
        match t.strip_prefix('~') {
            Some(off) => Some(here?.get(n % 3)?.floor() as i32 + off.parse::<i32>().unwrap_or(0)),
            None => t.parse().ok(),
        }
    };
    let (Some(x), Some(y), Some(z)) = (coord(0), coord(1), coord(2)) else {
        game.native_feedback(horizon_client::text::Text::literal(USAGE));
        return true;
    };
    let pos = [x, y, z];
    let now = game.world.game_time;
    let line = if set.is_some() {
        let num = |n: usize| tokens.get(n).and_then(|t| t.parse::<i32>().ok());
        let flag = |n: usize| tokens.get(n).is_some_and(|t| *t == "true" || *t == "1");
        match (num(3), num(4)) {
            (Some(off), Some(on)) => {
                let changed = with_world(|w| {
                    let d = &mut w.dims[dim];
                    let s = d.detectors.switches.get_mut(&pos)?;
                    s.off_below = off;
                    s.on_above = on;
                    s.in_stacks = flag(6);
                    let inverted = flag(5);
                    // `setInverted`: the output follows at once.
                    let follows = inverted != s.inverted && {
                        s.inverted = inverted;
                        s.power_after_delay()
                    };
                    d.detectors.checks.push(pos);
                    d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
                    Some(follows)
                })
                .flatten();
                match changed {
                    Some(follows) => {
                        if follows {
                            game.native_update_signal_neighbors(&dimension, pos);
                        }
                        format!("threshold switch: off below {off}, on above {on}")
                    }
                    None => "no threshold switch there".into(),
                }
            }
            _ => USAGE.into(),
        }
    } else {
        with_world(|w| redstone_state(&w.dims[dim], pos, now)).unwrap_or_default()
    };
    game.native_feedback(horizon_client::text::Text::literal(line));
    true
}

/// `/create_debug redstone_state`'s line.
fn redstone_state(d: &create_engine::world::Dimension, pos: [i32; 3], now: i64) -> String {
    if let Some(l) = d.levers.levers.get(&pos) {
        return format!(
            "analog lever: state {}, neighbours told {}",
            l.state,
            l.due
                .map_or("already".into(), |t| format!("in {} ticks", t - now))
        );
    }
    if let Some(o) = d.detectors.observers.get(&pos) {
        return format!(
            "smart observer: filter {}, target {:?}, sees {}, off {}",
            o.filter.as_deref().unwrap_or("none"),
            d.detectors.target_of.get(&pos),
            o.sustained.is_some(),
            o.off_at
                .map_or("not pending".into(), |t| format!("in {} ticks", t - now))
        );
    }
    if let Some(s) = d.detectors.switches.get(&pos) {
        return format!(
            "threshold switch: level {} of {}..{}, off below {}, on above {}, state {}, inverted \
             {}, powered {}, filter {}",
            s.level,
            s.min,
            s.max,
            s.off_below,
            s.on_above,
            s.state,
            s.inverted,
            s.powered,
            s.filter.as_deref().unwrap_or("none")
        );
    }
    "no analog lever, smart observer or threshold switch there".into()
}

/// Levers' states to `players` (gone ones: their handles hidden).
pub(super) fn send_levers(game: &mut Game<'_>, players: &[u128], dim: usize, levers: &[[i32; 3]]) {
    let mut shown: Vec<create_engine::lever::Shown> = Vec::new();
    for &pos in levers {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let at = detector_block(state)
            .filter(|(b, _)| *b == create_engine::Block::AnalogLever)
            .and_then(|(_, i)| Some((i.face?, i.facing?)));
        let lever = with_world(|w| w.dims[dim].levers.levers.get(&pos).map(|l| l.state)).flatten();
        match (at, lever) {
            (Some((face, facing)), Some(s)) => shown.push((pos, Some(s), face, facing)),
            _ => shown.push((pos, None, create_engine::steam::Face::Floor, Dir::North)),
        }
    }
    for chunk in shown.chunks(256) {
        let m = create_engine::lever::encode(chunk);
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::lever::CHANNEL, &m);
        }
    }
}

/// `AnalogLeverRenderer`'s handle, turned by the lever's state: a static part, set again when
/// the state changes (Create eases it there over a few ticks).
pub(super) fn levers_from_server(game: &mut Game<'_>, data: &[u8]) {
    for (pos, state, face, facing) in create_engine::lever::decode(data) {
        let key = depot_key(pos, 230, 0);
        let Some(state) = state else {
            game.native_part_clear(key);
            continue;
        };
        let registry = game.pack.assets.registry;
        let Some(handle) = state_with(
            registry,
            "create:part_analog_lever_handle",
            &[("facing", "up")],
        ) else {
            continue;
        };
        let pose = create_engine::lever::handle_pose(face, facing, state);
        if let Some(model) = game.native_part_model_posed(handle, pose) {
            game.native_part_set(key, model, pos, 1, 0.0, 0.0);
        }
    }
}

/// The cuckoo clocks a client draws: each one's hands' angles, and the time they last showed.
#[derive(Default)]
pub(super) struct ClientClocks {
    pub(super) hands: HashMap<[i32; 3], (f32, f32)>,
    pub(super) time: Option<(i32, i32)>,
}

thread_local! {
    pub(super) static CLOCKS: RefCell<ClientClocks> = RefCell::new(ClientClocks::default());
}

/// The hours and minutes a turning clock shows in this client's dimension now (only the
/// overworld's dimension type is `natural`).
fn clock_time(game: &Game<'_>) -> (i32, i32) {
    let natural = game.dim.id() == "minecraft:overworld";
    create_engine::cuckoo::time(game.world.day_time, natural)
}

/// `CuckooClockRenderer`'s hands of the clock at `pos` facing `facing`: where the time puts them
/// while it turns (`moveHands`), else where they were.
pub(super) fn draw_clock_hands(game: &mut Game<'_>, pos: [i32; 3], facing: Dir, turning: bool) {
    let now = clock_time(game);
    let (hour, minute) = CLOCKS.with_borrow_mut(|c| {
        let h = c.hands.entry(pos).or_insert((0.0, 0.0));
        if turning {
            *h = create_engine::cuckoo::hand_angles(now.0, now.1);
        }
        *h
    });
    let hp = create_engine::cuckoo::hand_pose(facing);
    let registry = game.pack.assets.registry;
    for (i, (name, angle)) in [
        ("create:part_cuckoo_hour_hand", hour),
        ("create:part_cuckoo_minute_hand", minute),
    ]
    .into_iter()
    .enumerate()
    {
        let Some(state) = state_with(registry, name, &[("facing", "up")]) else {
            continue;
        };
        let Some(model) = game.native_part_model_posed(state, hp.pose) else {
            continue;
        };
        let [x, y, z] = hp.offset;
        game.native_part_glide(
            create_engine::client::key(pos, create_engine::client::CUCKOO_HAND_SLOT + i as u8),
            model,
            pos,
            axis_index(hp.axis),
            0.0,
            hp.sign * angle,
            [x, y, z, 0.0],
            [0.0; 3],
        );
    }
}

/// Each client tick: when the minute shown changes, the turning clocks' hands move to it. One
/// compare a tick otherwise.
pub(super) fn tick_client_clocks(game: &mut Game<'_>, c: &ClientParts) {
    let now = clock_time(game);
    let changed = CLOCKS.with_borrow_mut(|k| {
        let changed = k.time != Some(now) && !k.hands.is_empty();
        k.time = Some(now);
        changed
    });
    if !changed {
        return;
    }
    let clocks: Vec<[i32; 3]> = CLOCKS.with_borrow(|k| k.hands.keys().copied().collect());
    for pos in clocks {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let facing = c
            .table
            .get(state)
            .filter(|(b, _)| {
                matches!(
                    b,
                    create_engine::Block::CuckooClock | create_engine::Block::MysteriousCuckooClock
                )
            })
            .and_then(|(_, i)| i.facing);
        let Some(facing) = facing else {
            CLOCKS.with_borrow_mut(|k| k.hands.remove(&pos));
            continue;
        };
        if c.speeds.get(&pos).is_some_and(|s| s[0] != 0.0) {
            draw_clock_hands(game, pos, facing, true);
        }
    }
}

/// Analog levers, smart observers and threshold switches saved with a column
/// (`create_redstone`): levers' `State` and `ChangeTimer`; observers' filters and `TurnOff`;
/// switches' thresholds, `Inverted`, `InStacks`, levels, `Powered`, `PoweredAfterDelay` and
/// filters.
pub(super) fn redstone_chunk_save(
    game: &Game<'_>,
    dim: usize,
    chunk: [i32; 2],
    data: &mut horizon_world::nbt::Compound,
) {
    use horizon_world::nbt::{Compound, Tag};
    let now = game.world.game_time;
    let Some((levers, (observers, switches))) = with_world(|w| {
        let d = &w.dims[dim];
        (
            d.levers.column(chunk[0], chunk[1]),
            d.detectors.column(chunk[0], chunk[1]),
        )
    }) else {
        return;
    };
    if levers.is_empty() && observers.is_empty() && switches.is_empty() {
        data.remove(&"create_redstone".into());
        return;
    }
    let s = |v: Option<&str>| Tag::String(v.unwrap_or("").into());
    let mut own = Compound::new();
    if !levers.is_empty() {
        let v = levers
            .iter()
            .flat_map(|(p, l)| {
                let left = l.due.map_or(0, |t| (t - now).clamp(0, 15) as i32);
                [p[0], p[1], p[2], i32::from(l.state), left]
            })
            .collect();
        own.insert("levers".into(), Tag::IntArray(v));
    }
    if !observers.is_empty() {
        let mut c = Compound::new();
        let nums = observers
            .iter()
            .flat_map(|(p, o)| {
                let left = o.off_at.map_or(-1, |t| (t - now).clamp(0, 64) as i32);
                [p[0], p[1], p[2], left]
            })
            .collect();
        c.insert("nums".into(), Tag::IntArray(nums));
        let filters = observers
            .iter()
            .map(|(_, o)| s(o.filter.as_deref()))
            .collect();
        c.insert("filters".into(), Tag::List(filters));
        own.insert("observers".into(), Tag::Compound(c));
    }
    if !switches.is_empty() {
        let mut c = Compound::new();
        let nums = switches
            .iter()
            .flat_map(|(p, x)| {
                let flags = i32::from(x.inverted)
                    | i32::from(x.in_stacks) << 1
                    | i32::from(x.state) << 2
                    | i32::from(x.powered) << 3;
                [
                    p[0],
                    p[1],
                    p[2],
                    x.on_above,
                    x.off_below,
                    flags,
                    x.level,
                    x.min,
                    x.max,
                ]
            })
            .collect();
        c.insert("nums".into(), Tag::IntArray(nums));
        let filters = switches
            .iter()
            .map(|(_, x)| s(x.filter.as_deref()))
            .collect();
        c.insert("filters".into(), Tag::List(filters));
        own.insert("switches".into(), Tag::Compound(c));
    }
    data.insert("create_redstone".into(), Tag::Compound(own));
}

/// What `redstone_chunk_save` wrote, for the blocks found as the column loads (at game time
/// `now`).
pub(super) fn redstone_from_nbt(dim: usize, now: i64, data: Option<&horizon_world::nbt::Compound>) {
    use create_engine::observe::{Observer, Switch};
    use horizon_world::nbt::Tag;
    let Some(data) = data else {
        return;
    };
    let part = |key: &str| -> Option<(Vec<i32>, Vec<Option<String>>)> {
        let Some(Tag::Compound(c)) = data.get(&key.into()) else {
            return None;
        };
        let nums = match c.get(&"nums".into()) {
            Some(Tag::IntArray(v)) => v.clone(),
            _ => return None,
        };
        let filters = match c.get(&"filters".into()) {
            Some(Tag::List(l)) => l
                .iter()
                .map(|t| match t {
                    Tag::String(s) => Some(s.to_string_lossy()).filter(|s| !s.is_empty()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        Some((nums, filters))
    };
    let levers: Vec<([i32; 3], create_engine::lever::Lever)> = match data.get(&"levers".into()) {
        Some(Tag::IntArray(v)) => v
            .chunks_exact(5)
            .map(|c| {
                let lever = create_engine::lever::Lever {
                    state: c[3].clamp(0, 15) as u8,
                    due: (c[4] > 0).then(|| now + i64::from(c[4].min(15))),
                };
                ([c[0], c[1], c[2]], lever)
            })
            .collect(),
        _ => Vec::new(),
    };
    let observers: Vec<([i32; 3], Observer)> = part("observers")
        .map(|(nums, filters)| {
            nums.chunks_exact(4)
                .enumerate()
                .map(|(i, c)| {
                    let o = Observer {
                        filter: filters.get(i).cloned().flatten(),
                        sustained: None,
                        off_at: (c[3] >= 0).then(|| now + i64::from(c[3])),
                    };
                    ([c[0], c[1], c[2]], o)
                })
                .collect()
        })
        .unwrap_or_default();
    let switches: Vec<([i32; 3], Switch)> = part("switches")
        .map(|(nums, filters)| {
            nums.chunks_exact(9)
                .enumerate()
                .map(|(i, c)| {
                    let s = Switch {
                        on_above: c[3],
                        off_below: c[4],
                        inverted: c[5] & 1 != 0,
                        in_stacks: c[5] & 2 != 0,
                        state: c[5] & 4 != 0,
                        powered: c[5] & 8 != 0,
                        level: c[6],
                        min: c[7],
                        max: c[8],
                        filter: filters.get(i).cloned().flatten(),
                    };
                    ([c[0], c[1], c[2]], s)
                })
                .collect()
        })
        .unwrap_or_default();
    with_world(|w| {
        let d = &mut w.dims[dim];
        d.levers.load(levers.into_iter());
        d.detectors
            .load(observers.into_iter(), switches.into_iter());
    });
}
