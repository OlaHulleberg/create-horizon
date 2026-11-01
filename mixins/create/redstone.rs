// Create's redstone in the game: redstone links and powered latches. A sibling of the `create`
// module (`runtime.rs`).
use super::Game;
use super::create::*;
use super::create_logistics::*;
use create_engine::kinetics::Dir;
use create_engine::states;

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
    let registry = game.pack.assets.registry;
    let current = game.world.block(pos[0], pos[1], pos[2]);
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
    game.native_set_block(
        dimension,
        pos,
        id,
        super::native_hooks::flags::UPDATE_CLIENTS,
    );
    game.native_update_signal_neighbors(dimension, pos);
}

/// `DiodeBlock.getInputSignal` > 0 at the latch's back, and a powered latch's
/// `isPoweredOnSides`: a signal toward it, or redstone dust beside it with power.
pub(super) fn latch_inputs(game: &mut Game<'_>, pos: [i32; 3], facing: Dir) -> (bool, bool) {
    let registry = game.pack.assets.registry;
    let wire = |game: &Game<'_>, p: [i32; 3]| {
        registry
            .state(game.world.block(p[0], p[1], p[2]))
            .is_some_and(|s| {
                s.name == "minecraft:redstone_wire"
                    && s.properties.get("power").is_some_and(|v| v != "0")
            })
    };
    let powered_from = |game: &mut Game<'_>, d: Dir| {
        let o = d.offset();
        let at = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
        let dir = horizon_core::geometry::Direction::ALL[d as usize];
        let bp = horizon_core::position::BlockPos::new(at[0], at[1], at[2]);
        game.with_level(|b, level, ticks| b.signal(level, ticks, bp, dir)) > 0 || wire(game, at)
    };
    let back = powered_from(game, facing);
    // getClockWise / getCounterClockWise.
    let (left, right) = match facing {
        Dir::North => (Dir::East, Dir::West),
        Dir::East => (Dir::South, Dir::North),
        Dir::South => (Dir::West, Dir::East),
        _ => (Dir::North, Dir::South),
    };
    let side = powered_from(game, left) || powered_from(game, right);
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
        if again {
            next.push((pos, now + 1));
        }
    }
    for pos in checks {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let Some((kind, l)) = latch_state(state) else {
            continue;
        };
        let (back, side) = latch_inputs(game, pos, l.facing);
        let pending = next.iter().any(|t: &([i32; 3], i64)| t.0 == pos)
            || with_world(|w| w.dims[dim].latch_ticks.iter().any(|t| t.0 == pos)) == Some(true);
        if !pending && create_engine::latch::needs_tick(kind, &l, back, side) {
            next.push((pos, now + 1));
        }
    }
    with_world(|w| w.dims[dim].latch_ticks.extend(next));
}
