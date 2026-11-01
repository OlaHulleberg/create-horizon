// Game glue for the Create port, injected as `crate::game::create`. Rules live in the
// `create_engine` crate; this module only turns the game's hooks into engine calls.
// State lives on the server thread that fires the hooks (one per world).

use super::Game;
use super::create_contraptions::*;
use super::create_fluids::*;
use super::create_logistics::*;
use super::create_processing::*;
use super::create_redstone::*;
use super::create_trains::*;
use super::native_lifecycle::WorldPhase;
use create_engine::client::{self, Model, Update};
use create_engine::kinetics::{Axis, Dir, Event};
use create_engine::states;
use create_engine::world::{CreateWorld, StateTable, dimension_index};
use horizon_world::registry::{BlockRegistry, BlockStateId};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    pub(super) static WORLD: RefCell<Option<CreateWorld>> = const { RefCell::new(None) };
}

/// Builds the state table from the registry once (the content mod's states never change while the
/// game runs). Returns false when Create's content isn't installed: every hook then stays a miss.
pub(super) fn ensure(registry: &BlockRegistry) -> bool {
    WORLD.with_borrow_mut(|w| {
        if w.is_some() {
            return true;
        }
        let names = (0..registry.len()).filter_map(|i| {
            registry
                .state(BlockStateId(i as u32))
                .map(|s| s.name.as_str())
        });
        let Some((first, len)) = states::find_range(names) else {
            states::clear_range();
            return false;
        };
        states::set_range(first, len);
        let table = StateTable::new(
            first,
            (first..first + len).filter_map(|i| {
                let s = registry.state(BlockStateId(i))?;
                Some((
                    s.name.as_str(),
                    s.properties.iter().map(|(k, v)| (k.as_str(), v.as_str())),
                ))
            }),
        );
        eprintln!("[create] {len} Create block states from id {first}");
        *w = Some(CreateWorld::new(table));
        true
    })
}

pub(super) fn with_world<R>(f: impl FnOnce(&mut CreateWorld) -> R) -> Option<R> {
    WORLD.with_borrow_mut(|w| w.as_mut().map(f))
}

/// World open: index Create's block states so hot hooks can test them with one compare.
/// World close: forget them, so a world without Create pays nothing.
pub(super) fn world_lifecycle(game: &mut Game<'_>, phase: &WorldPhase) {
    match phase {
        WorldPhase::WorldOpenReady => {
            if !ensure(game.pack.assets.registry) {
                eprintln!("[create] no Create block states: the mod stays idle");
            }
        }
        WorldPhase::WorldCloseFinal
        | WorldPhase::WorldOpenFailed
        | WorldPhase::WorldCloseFailed => {
            WORLD.with_borrow_mut(|w| *w = None);
            RECIPES.with_borrow_mut(|r| *r = None);
            states::clear_range();
        }
    }
}

pub(super) fn level_index(dimension: horizon_world::level::Dimension) -> usize {
    match dimension {
        horizon_world::level::Dimension::Overworld => 0,
        horizon_world::level::Dimension::Nether => 1,
        horizon_world::level::Dimension::End => 2,
    }
}

/// A committed block change on the server. Hot: two range tests, then return, for any change
/// that is not Create's.
#[inline(always)]
pub(super) fn block_changed(
    dimension: horizon_world::level::Dimension,
    pos: horizon_core::position::BlockPos,
    old: BlockStateId,
    new: BlockStateId,
) {
    if states::index(old.0).is_none() && states::index(new.0).is_none() {
        return;
    }
    with_world(|w| w.block_changed(level_index(dimension), [pos.x, pos.y, pos.z], old.0, new.0));
}

/// A neighbour of a block changed on the server. Hot: one range test, then return, for any block
/// that is not Create's.
#[inline(always)]
pub(super) fn neighbor_changed(
    dimension: horizon_world::level::Dimension,
    pos: horizon_core::position::BlockPos,
    state: BlockStateId,
) {
    if states::index(state.0).is_none() {
        return;
    }
    with_world(|w| w.neighbor_changed(level_index(dimension), [pos.x, pos.y, pos.z], state.0));
}

/// A column joined a server level: find its Create blocks. Sections whose palette holds one
/// state only are tested once; the others are unpacked on the stack, never allocated.
pub(super) fn chunk_load(
    game: &mut Game<'_>,
    dimension: &str,
    chunk: &horizon_client::chunk::Chunk,
) {
    if !ensure(game.pack.assets.registry) {
        return;
    }
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    // Settings saved with the column (`chunk_data_save`), before its blocks are found.
    let saved = settings_from_nbt(Game::native_chunk_data(chunk, "create"));
    let belts = belts_from_nbt(Game::native_chunk_data(chunk, "create"));
    let tanks = tanks_from_nbt(Game::native_chunk_data(chunk, "create"));
    let windmills = windmills_from_nbt(game, Game::native_chunk_data(chunk, "create"));
    let crushers = crushers_from_nbt(Game::native_chunk_data(chunk, "create"));
    let trains = trains_from_nbt(game, Game::native_chunk_data(chunk, "create"));
    let names = station_names_from_nbt(Game::native_chunk_data(chunk, "create"));
    let burners = burners_from_nbt(Game::native_chunk_data(chunk, "create"));
    let links = links_from_nbt(Game::native_chunk_data(chunk, "create"));
    let curves = curves_from_nbt(Game::native_chunk_data(chunk, "create"));
    let diodes = diodes_from_nbt(Game::native_chunk_data(chunk, "create"));
    let deployers = deployers_from_nbt(Game::native_chunk_data(chunk, "create"));
    let brass: Vec<([i32; 3], create_engine::tunnel::Brass)> = {
        use horizon_world::nbt::Tag;
        match Game::native_chunk_data(chunk, "create").and_then(|d| d.get(&"brass".into())) {
            Some(Tag::Compound(c)) => match (c.get(&"nums".into()), c.get(&"items".into())) {
                (Some(Tag::IntArray(nums)), Some(Tag::List(items))) => nums
                    .chunks_exact(6)
                    .zip(items)
                    .map(|(v, item)| {
                        let item = match item {
                            Tag::String(s) => s.to_string_lossy(),
                            _ => String::new(),
                        };
                        let n = v[5].clamp(0, 64) as u8;
                        (
                            [v[0], v[1], v[2]],
                            create_engine::tunnel::Brass {
                                stack: (!item.is_empty() && n > 0).then_some((item, n)),
                                entered_from: Dir::ALL.get(v[4].clamp(0, 6) as usize).copied(),
                                mode: create_engine::tunnel::Mode::ALL[v[3].clamp(0, 4) as usize],
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    };
    let basins: Vec<([i32; 3], create_engine::basin::Tanks)> =
        match Game::native_chunk_data(chunk, "create").and_then(|d| d.get(&"basins".into())) {
            Some(horizon_world::nbt::Tag::IntArray(v)) => v
                .chunks_exact(11)
                .map(|c| {
                    let tank = |i: usize| {
                        create_engine::tank::Fluid::from_index(c[3 + 2 * i].clamp(0, 255) as u8)
                            .map(|f| (f, c[4 + 2 * i].clamp(0, 1000) as u32))
                    };
                    (
                        [c[0], c[1], c[2]],
                        create_engine::basin::Tanks {
                            input: [tank(0), tank(1)],
                            output: [tank(2), tank(3)],
                        },
                    )
                })
                .collect(),
            _ => Vec::new(),
        };
    let ranges: Vec<([i32; 3], i32)> =
        match Game::native_chunk_data(chunk, "create").and_then(|d| d.get(&"chassis".into())) {
            Some(horizon_world::nbt::Tag::IntArray(v)) => v
                .chunks_exact(4)
                .map(|c| ([c[0], c[1], c[2]], c[3].clamp(1, 16)))
                .collect(),
            _ => Vec::new(),
        };
    let (vaults, filters) = vaults_from_nbt(Game::native_chunk_data(chunk, "create"));
    let mut buffer = [0u32; 4096];
    with_world(|w| {
        w.dims[dim].vaults.load(vaults.into_iter());
        w.dims[dim].chute_filters.extend(filters);
        w.load_settings(dim, saved.into_iter());
        w.load_belt_items(dim, belts.into_iter());
        w.load_tanks(dim, tanks.into_iter());
        w.load_windmills(dim, windmills.into_iter());
        w.load_crushers(dim, crushers.into_iter());
        w.load_trains(dim, trains);
        w.dims[dim].railway.load_names(names.into_iter());
        w.load_burners(dim, burners.into_iter());
        w.load_links(dim, links.into_iter());
        for c in curves {
            w.dims[dim].track_curves.add(c);
        }
        w.load_diodes(dim, diodes.into_iter());
        w.load_deployers(dim, deployers.into_iter());
        w.load_brass(dim, brass.into_iter());
        w.dims[dim].chassis_range.extend(ranges);
        w.dims[dim].basin_fluids.extend(basins);
        for (i, section) in chunk.sections.iter().enumerate() {
            if let Some(single) = section.blocks.uniform()
                && states::index(single).is_none()
            {
                continue;
            }
            section.unpack(&mut buffer);
            if !buffer.iter().any(|&s| states::index(s).is_some()) {
                continue;
            }
            let base = [chunk.x * 16, chunk.min_y() + 16 * i as i32, chunk.z * 16];
            w.section_loaded(dim, base, &buffer);
        }
    });
}

pub(super) fn chunk_unload(dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    if let Some(dim) = dimension_index(dimension) {
        with_world(|w| w.column_unloaded(dim, chunk.x, chunk.z));
    }
}

thread_local! {
    /// Commands a player's hook asked for, run at the next level tick (`native_run_command` only
    /// runs where no player's state is swapped in).
    pub(super) static LATER: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// After a server level's tick: re-solve what changed there (nothing when nothing did), break
/// the blocks Create would break, and tell the level's players what turns now.
pub(super) fn level_tick_post(game: &mut Game<'_>, dimension: &str) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    if LATER.with(|l| !l.borrow().is_empty()) {
        for line in LATER.with(|l| std::mem::take(&mut *l.borrow_mut())) {
            game.native_run_command(&line);
        }
    }
    with_world(|w| w.tick_cranks(dim));
    // Water wheels whose surroundings changed, and one in 60 of the others each tick.
    let tick = game.world.game_time as u64;
    for (pos, axis) in with_world(|w| w.take_wheel_checks(dim, tick)).unwrap_or_default() {
        let samples: Vec<([i32; 3], [f64; 3])> = create_engine::waterwheel::small_offsets(axis)
            .into_iter()
            .map(|o| {
                let at = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
                (o, flow_at(game, dimension, at))
            })
            .collect();
        let score = create_engine::waterwheel::flow_score(axis, &samples);
        with_world(|w| w.set_wheel_flow(dim, pos, score));
    }
    // Split shafts follow redstone (ClutchBlock.neighborChanged): set `powered` before solving,
    // so the change turns the network in this tick's solve.
    for pos in with_world(|w| w.take_redstone(dim)).unwrap_or_default() {
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let Some(was) = with_world(|w| w.powered(state.0)).flatten() else {
            continue;
        };
        let at = horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]);
        let now = game.with_level(|b, level, ticks| b.has_neighbor_signal(level, ticks, at));
        if now != was {
            let value = if now { "true" } else { "false" };
            game.native_set_block_property_with(
                pos,
                "powered",
                value,
                super::native_hooks::flags::UPDATE_CLIENTS,
            );
        }
        // ChainGearshiftBlockEntity.neighbourChanged: the signal's strength sets its chain.
        if with_world(|w| w.reads_signal(state.0)) == Some(true) {
            let signal =
                game.with_level(|b, level, ticks| b.best_neighbor_signal(level, ticks, at));
            with_world(|w| w.set_chain_signal(dim, pos, signal));
        }
    }
    tick_chain_drives(game, dim);
    tick_sequencers(game, dim);
    tick_latches(game, dimension, dim);
    tick_diodes(game, dimension, dim);
    tick_lamps(game, dimension, dim);
    tick_links(game, dimension, dim);
    tick_bearings(game, dimension, dim);
    tick_gantries(game, dimension, dim);
    tick_crafters(game, dimension, dim);
    tick_windmills(game, dimension, dim);
    tick_millstones(game, dim);
    tick_fans(game, dimension, dim);
    tick_crushers(game, dimension, dim);
    tick_burners(game, dim);
    tick_mixers(game, dim);
    tick_pipes(game, dimension, dim);
    tick_boilers(game, dimension, dim);
    tick_tank_states(game, dimension, dim);
    tick_pistons(game, dimension, dim);
    tick_trains(game, dimension, dim);
    sync_curves(game, dimension, dim);
    tick_vaults(game, dim);
    tick_chutes(game, dim);
    tick_drills(game, dim);
    tick_saws(game, dim);
    tick_spouts(game, dim);
    tick_drains(game, dim);
    tick_funnels(game, dim);
    tick_tunnels(game, dimension, dim);
    tick_brass(game, dim);
    tick_depots(game, dimension, dim);
    tick_belts(game, dimension, dim);
    tick_presses(game, dimension, dim);
    tick_deployers(game, dimension, dim);
    send_tank_changes(game, dimension, dim);
    let mut events = Vec::new();
    with_world(|w| w.settle_belts(dim));
    let Some((removed, unsaved, gauges)) = with_world(|w| {
        w.tick(dim, &mut events);
        (
            w.dims[dim].kinetics.take_removed(),
            w.take_unsaved(dim),
            w.take_gauge_changes(dim),
        )
    }) else {
        return;
    };
    for (cx, cz) in unsaved {
        game.native_mark_chunk_unsaved(dimension, [cx, cz]);
    }
    if events.is_empty() && removed.is_empty() {
        send_gauge_changes(game, dimension, dim, &gauges);
        return;
    }
    let mut updates: Vec<Update> = removed
        .into_iter()
        .map(|pos| Update::Clear { pos })
        .collect();
    for e in events {
        match e {
            Event::Break { pos } => {
                // Create's `world.destroyBlock(pos, true)`: the block breaks with its drops.
                game.native_run_command(&format!(
                    "execute in {dimension} run setblock {} {} {} air destroy",
                    pos[0], pos[1], pos[2]
                ));
            }
            Event::Speed {
                pos, sub, speed, ..
            } => {
                // `MechanicalBearingBlockEntity.onSpeedChanged`: `assembleNextTick`.
                let state = game.world.block(pos[0], pos[1], pos[2]).0;
                let block = with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten();
                if block == Some(create_engine::Block::MechanicalBearing) {
                    with_world(|w| w.dims[dim].bearing_checks.push(pos));
                }
                // `GantryShaftBlockEntity.onSpeedChanged`: the carriages on it try to start.
                if block == Some(create_engine::Block::GantryShaft) {
                    gantry_carriages_on(game, dim, pos);
                }
                // A crafter that starts turning looks at its items (`lazyTick`).
                if block == Some(create_engine::Block::MechanicalCrafter) {
                    with_world(|w| w.dims[dim].crafter_checks.push(pos));
                }
                // A press that starts turning looks at the depot below it again.
                with_world(|w| {
                    if w.dims[dim].presses.contains_key(&pos) {
                        w.wake_depot(dim, [pos[0], pos[1] - 2, pos[2]]);
                    }
                });
                updates.push(Update::Set { pos, sub, speed })
            }
        }
    }
    let players: Vec<u128> = game
        .native_server_players()
        .into_iter()
        .filter(|p| p.dimension == dimension)
        .map(|p| p.uuid)
        .collect();
    send(game, &players, &updates);
    // After the speeds: a client forgets a cleared block's needle.
    send_gauge_changes(game, dimension, dim, &gauges);
}

/// Encased chain drives placed or beside a change take their place in their chain
/// (`ChainDriveBlock.updateShape`). Nothing runs without one.
fn tick_chain_drives(game: &mut Game<'_>, dim: usize) {
    for (pos, drive) in with_world(|w| w.take_chain_shapes(dim)).unwrap_or_default() {
        let along = if drive.along_first { "true" } else { "false" };
        let flags = super::native_hooks::flags::UPDATE_ALL;
        game.native_set_block_property_with(pos, "axis_along_first", along, flags);
        game.native_set_block_property_with(pos, "part", drive.part.name(), flags);
    }
}

/// Sequenced gearshifts: those whose neighbour changed look at their power
/// (`SequencedGearshiftBlock.tick`), those running an instruction tick; their blocks show the
/// step (`state`). Nothing runs without one.
fn tick_sequencers(game: &mut Game<'_>, dim: usize) {
    use create_engine::sequencer::Step;
    let mut steps = Vec::new();
    for (pos, running) in with_world(|w| w.take_sequencer_checks(dim)).unwrap_or_default() {
        let at = horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]);
        let powered = game.with_level(|b, level, ticks| b.has_neighbor_signal(level, ticks, at));
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
            Step::Ended => {
                let at = horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]);
                if game.with_level(|b, level, ticks| b.has_neighbor_signal(level, ticks, at)) {
                    continue;
                }
                0
            }
        };
        const STATES: [&str; 6] = ["0", "1", "2", "3", "4", "5"];
        game.native_set_block_property_with(
            pos,
            "state",
            STATES[usize::from(state.min(5))],
            super::native_hooks::flags::UPDATE_ALL,
        );
    }
}

pub(super) fn send(game: &mut Game<'_>, players: &[u128], updates: &[Update]) {
    if updates.is_empty() {
        return;
    }
    let messages = client::encode(updates);
    for &p in players {
        for m in &messages {
            // A player who left meanwhile just misses it.
            let _ = game.native_net_send_to_player(p, client::CHANNEL, m);
        }
    }
}

/// A client asks for every part of its dimension when it joins (`sync`).
pub(super) fn net_from_client(game: &mut Game<'_>, player: u128, channel: &str, data: &[u8]) {
    if channel == SCROLL {
        scroll_from_client(game, player, data);
        return;
    }
    if channel == VALUE_SETTINGS {
        value_settings_from_client(game, player, data);
        return;
    }
    if channel != client::CHANNEL || data != b"sync" {
        return;
    }
    let Some(dimension) = game
        .native_server_players()
        .into_iter()
        .find(|p| p.uuid == player)
        .map(|p| p.dimension)
    else {
        return;
    };
    let Some(dim) = dimension_index(&dimension) else {
        return;
    };
    let updates: Vec<Update> = with_world(|w| {
        w.dims[dim]
            .kinetics
            .parts()
            .map(|(pos, sub, speed, _)| Update::Set { pos, sub, speed })
            .collect()
    })
    .unwrap_or_default();
    send(game, &[player], &updates);
    let depots: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].depots.keys().copied().collect()).unwrap_or_default();
    send_depots(game, &[player], dim, &depots);
    let presses: Vec<[i32; 3]> = with_world(|w| {
        w.dims[dim]
            .presses
            .iter()
            .filter(|(_, p)| p.running)
            .map(|(k, _)| *k)
            .collect()
    })
    .unwrap_or_default();
    send_presses(game, &[player], dim, &presses);
    let gauges: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].gauges.keys().copied().collect()).unwrap_or_default();
    send_gauges(game, &[player], dim, &gauges);
    let belts: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].belts.keys().copied().collect()).unwrap_or_default();
    send_belts(game, &[player], dim, &belts);
    let tanks: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].tanks.multis.keys().copied().collect()).unwrap_or_default();
    send_tanks(game, &[player], dim, &tanks);
    let windmills: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].windmills.keys().copied().collect()).unwrap_or_default();
    send_windmills(game, &[player], dim, &windmills);
    let links: Vec<[i32; 3]> = with_world(|w| {
        w.dims[dim]
            .links
            .links
            .iter()
            .filter(|(_, l)| l.key != create_engine::link::Key::default())
            .map(|(p, _)| *p)
            .collect()
    })
    .unwrap_or_default();
    send_links(game, &[player], dim, &links);
    let curves: Vec<_> = with_world(|w| {
        let c = &w.dims[dim].track_curves;
        c.at.keys()
            .map(|p| (*p, c.drawn_from(*p)))
            .filter(|(_, l)| !l.is_empty())
            .collect()
    })
    .unwrap_or_default();
    send_curves(game, &[player], &curves);
    let deployers: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].deployers.keys().copied().collect()).unwrap_or_default();
    send_deployers(game, &[player], dim, &deployers);
    let trains: Vec<u32> =
        with_world(|w| w.dims[dim].railway.trains.iter().map(|t| t.id).collect())
            .unwrap_or_default();
    send_trains(game, dim, &trains, true);
}

/// A player right-clicks a block (`HandCrankBlock.useItemOn`): a hand crank turns for 10 ticks,
/// backwards while sneaking, and tires the player (`crankHungerMultiplier` 0.01 per RPM).
pub(super) fn right_click_block(
    game: &mut Game<'_>,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    let was = *canceled;
    // The off hand's click after a main-hand click a Create block took: vanilla's client never
    // sends it (SUCCESS ends the use), Horizon's does, sometimes a few ticks later; it is taken
    // once, not acted on.
    if game.hand == horizon_client::first_person::Hand::Off {
        let player = game.native_acting_player().map_or(0, |p| p.uuid);
        if TAKEN_OFF_HAND.with(|t| t.get()).is_some_and(|(p, at)| {
            p == player && (at..=at + TAKEN_WINDOW).contains(&game.world.game_time)
        }) {
            TAKEN_OFF_HAND.with(|t| t.set(None));
            *canceled = true;
            return;
        }
    }
    right_click_inner(game, pos, face, canceled);
    if *canceled && !was {
        // Create's `useItemOn` answered SUCCESS: the client sends no item use after it. Ours
        // does, so the item use that follows this click is taken (`use_item`).
        let player = game.native_acting_player().map_or(0, |p| p.uuid);
        let taken = Some((player, game.world.game_time));
        TAKEN_USE.with(|t| t.set(taken));
        if game.hand == horizon_client::first_person::Hand::Main {
            TAKEN_OFF_HAND.with(|t| t.set(taken));
        }
    }
}

/// How many game ticks after a taken click its follow-ups (the item use, the off hand's click)
/// may still arrive.
pub(super) const TAKEN_WINDOW: i64 = 10;

thread_local! {
    /// The player and game time of the last click a Create block took: for the item use and
    /// the off hand's click that follow it, each taken once.
    pub(super) static TAKEN_USE: std::cell::Cell<Option<(u128, i64)>> = const { std::cell::Cell::new(None) };
    pub(super) static TAKEN_OFF_HAND: std::cell::Cell<Option<(u128, i64)>> = const { std::cell::Cell::new(None) };
}

/// The item use that follows a click a Create block took (the same tick, or the next).
pub(super) fn use_item(
    game: &mut Game<'_>,
    stack: &horizon_client::items::ItemStack,
    claimed: &mut bool,
) {
    if &*stack.item == "create:experience_nugget" {
        use_nugget(game, stack.count);
        *claimed = true;
        return;
    }
    let Some((player, at)) = TAKEN_USE.with(|t| t.get()) else {
        return;
    };
    let now = game.world.game_time;
    let me = game.native_acting_player().map_or(0, |p| p.uuid);
    if me == player && (at..=at + TAKEN_WINDOW).contains(&now) {
        *claimed = true;
        TAKEN_USE.with(|t| t.set(None));
    }
}

/// `ExperienceNuggetItem.use`: the stack (one when sneaking) thrown out from before the eyes as
/// experience orbs, 3 points a nugget, and used up.
pub(super) fn use_nugget(game: &mut Game<'_>, count: u32) {
    let Some((eye, look)) = game.aim() else {
        return;
    };
    let Some(dimension) = game.native_acting_player().map(|p| p.dimension) else {
        return;
    };
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let used = if sneaking { 1 } else { count };
    // `look × rotate((-.75, 0, 0), -yRot, Y)`: to the side of the view.
    let yaw = f64::from(game.yaw).to_radians();
    let side = [-0.75 * yaw.cos(), 0.0, -0.75 * yaw.sin()];
    let cross = [
        look[1] * side[2] - look[2] * side[1],
        look[2] * side[0] - look[0] * side[2],
        look[0] * side[1] - look[1] * side[0],
    ];
    let at = [
        eye[0] + look[0] * 0.5 + cross[0],
        eye[1] + look[1] * 0.5 + cross[1],
        eye[2] + look[2] * 0.5 + cross[2],
    ];
    // `offsetRandomly(ZERO, level.random, 1).normalize()`: a scatter; any draw will do.
    let mut seed = game.world.game_time as u64 ^ 0x9e37_79b9_7f4a_7c15;
    let mut draw = || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 40) as f64 / (1u64 << 24) as f64 - 0.5) * 2.0
    };
    for value in create_engine::nugget::orbs(used) {
        let o = [draw(), draw(), draw()];
        let n = (o[0] * o[0] + o[1] * o[1] + o[2] * o[2]).sqrt().max(1e-9);
        let m = [
            look[0] * 0.2 + o[0] / n * 0.1,
            look[1] * 0.2 + 0.2 + o[1] / n * 0.1,
            look[2] * 0.2 + o[2] / n * 0.1,
        ];
        // Commands don't run inside a player's hook: the orbs appear at the next level tick.
        LATER.with(|l| {
            l.borrow_mut().push(format!(
                "execute in {dimension} run summon minecraft:experience_orb {} {} {} {{Value:{value}s,Motion:[{}d,{}d,{}d]}}",
                at[0], at[1], at[2], m[0], m[1], m[2]
            ))
        });
    }
    let slot = if game.hand == horizon_client::first_person::Hand::Off {
        &mut game.offhand
    } else {
        &mut game.inventory[game.selected]
    };
    if let Some(s) = slot.as_mut() {
        s.count = s.count.saturating_sub(used);
        if s.count == 0 {
            *slot = None;
        }
    }
}

/// The item entities of the level `dimension` whose box touches `lo`..`hi`
/// (`Level.getEntitiesOfClass(ItemEntity.class, box)`), on the server: `f` gets each one's id,
/// stack, position and motion and may change the stack and the motion; one left empty is removed.
pub(super) fn items_in(
    game: &mut Game<'_>,
    dimension: &str,
    lo: [f64; 3],
    hi: [f64; 3],
    mut f: impl FnMut(u32, &mut horizon_client::items::ItemStack, [f64; 3], &mut [f64; 3]),
) {
    if game.native_window() || dimension != game.dim.id() {
        return;
    }
    let mut emptied = false;
    for item in game.dropped.iter_mut() {
        let p = [item.position.x, item.position.y, item.position.z];
        if !create_engine::belt::item_touches(p, lo, hi) {
            continue;
        }
        let mut v = [item.velocity.x, item.velocity.y, item.velocity.z];
        f(item.id, &mut item.stack, p, &mut v);
        (item.velocity.x, item.velocity.y, item.velocity.z) = (v[0], v[1], v[2]);
        emptied |= item.stack.count == 0;
    }
    if emptied {
        game.dropped.retain(|i| i.stack.count > 0);
    }
}

pub(super) fn right_click_inner(
    game: &mut Game<'_>,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    if *canceled || states::index(game.world.block(pos[0], pos[1], pos[2]).0).is_none() {
        return;
    }
    let Some(dim) = game
        .native_acting_player()
        .and_then(|p| dimension_index(&p.dimension))
    else {
        return;
    };
    if game.spectator() {
        return;
    }
    let back = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    let block = with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten();
    if block == Some(create_engine::Block::Millstone) {
        use_millstone(game, pos, canceled);
        return;
    }
    if matches!(
        block,
        Some(create_engine::Block::CreativeMotor | create_engine::Block::BrassTunnel)
    ) {
        use_value_box(game, pos, face, canceled);
        return;
    }
    if block == Some(create_engine::Block::Depot) {
        use_depot(game, dim, pos, face, canceled);
        return;
    }
    if block == Some(create_engine::Block::SmartChute) {
        use_smart_chute(game, dim, pos, face, canceled);
        return;
    }
    if matches!(
        block,
        Some(create_engine::Block::LinearChassis | create_engine::Block::SecondaryLinearChassis)
    ) {
        use_chassis(game, pos, face, back, canceled);
        return;
    }
    if block == Some(create_engine::Block::MechanicalCrafter) {
        use_crafter(game, dim, pos, face, canceled);
        return;
    }
    if block == Some(create_engine::Block::Deployer) {
        if let Some(f) = with_world(|w| w.table.get(state).and_then(|(_, i)| i.facing)).flatten() {
            use_deployer(game, dim, pos, f, canceled);
        }
        return;
    }
    if block == Some(create_engine::Block::Belt) {
        use_belt(game, dim, pos, canceled);
        return;
    }
    if block == Some(create_engine::Block::FluidTank) {
        use_tank(game, dim, pos, canceled);
        return;
    }
    if block == Some(create_engine::Block::TrackStation) {
        *canceled = true;
        use_station(game, dim, pos);
        return;
    }
    if block == Some(create_engine::Block::BlazeBurner) {
        use_burner(game, dim, pos, canceled);
        return;
    }
    if block == Some(create_engine::Block::WindmillBearing) {
        use_windmill(game, dim, pos, back, canceled);
        return;
    }
    if let Some((facing, receiver, _)) = link_state(state) {
        use_link(game, dim, pos, (facing, receiver), back, canceled);
        return;
    }
    if let Some((_, s)) = diode_state(state) {
        // A click on its value box is the box's (a short one toggles it, `onShortInteract`).
        use_value_box(game, pos, face, canceled);
        // BrassDiodeBlock.useItemOn -> `toggle`: not while sneaking, nor with a wrench, nor for
        // a player who may not build.
        if *canceled
            || back
            || !game.may_build()
            || game.native_held().is_some_and(|h| h.0 == "create:wrench")
        {
            return;
        }
        *canceled = true;
        if let Some(p) = game.native_acting_player() {
            toggle_diode(game, &p.dimension, pos, s.inverted);
        }
        return;
    }
    if let Some((kind, l)) = latch_state(state) {
        // ToggleLatchBlock.useItemOn: not while sneaking, nor with a wrench.
        if back || game.native_held().is_some_and(|h| h.0 == "create:wrench") {
            return;
        }
        *canceled = true;
        if let Some(n) = create_engine::latch::activated(kind, &l)
            && let Some(p) = game.native_acting_player()
        {
            set_latch(game, &p.dimension, pos, n);
        }
        return;
    }
    if with_world(|w| w.turn_crank(dim, pos, back)) != Some(true) {
        return;
    }
    *canceled = true;
    if !game.creative
        && let Some(p) = game.player.as_mut()
    {
        p.cause_food_exhaustion(create_engine::parts::CRANK_RPM * 0.01);
    }
}

/// A player scrolled on a block (`create:scroll`: position, steps); what a client sends is
/// checked here: the player's own dimension, and the block within their reach.
pub(super) const SCROLL: &str = "create:scroll";

pub(super) fn scroll_from_client(game: &mut Game<'_>, player: u128, data: &[u8]) {
    let [x, y, z, steps] = match data {
        d if d.len() == 16 => {
            [0, 4, 8, 12].map(|i| i32::from_le_bytes(d[i..i + 4].try_into().unwrap()))
        }
        _ => return,
    };
    let Some(p) = game
        .native_server_players()
        .into_iter()
        .find(|p| p.uuid == player)
    else {
        return;
    };
    let Some(dim) = dimension_index(&p.dimension) else {
        return;
    };
    let center = [f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5];
    let d2: f64 = (0..3).map(|i| (p.position[i] - center[i]).powi(2)).sum();
    // ServerGamePacketListenerImpl: blocks within the interaction range (plus a margin).
    if d2 > 8.0 * 8.0 {
        return;
    }
    let pos = [x, y, z];
    let steps = steps.clamp(-64, 64);
    let state = game.world.block(x, y, z).0;
    let block = with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten();
    use create_engine::Block as B;
    let value = match block {
        Some(B::CreativeMotor | B::RotationSpeedController) => {
            with_world(|w| w.scroll_motor(dim, pos, steps));
            return;
        }
        // `ChassisScrollValueBehaviour`: the whole group of chassis takes the range, 1 to 16.
        Some(B::LinearChassis | B::SecondaryLinearChassis) => {
            let group = chassis_group(game, pos);
            let now = with_world(|w| w.dims[dim].chassis_range.get(&pos).copied())
                .flatten()
                .unwrap_or(create_engine::contraption::CHASSIS_RANGE);
            let range = (now + steps).clamp(1, 16);
            with_world(|w| w.set_chassis_range(dim, &group, range));
            range
        }
        Some(B::BrassTunnel) => with_world(|w| {
            let b = w.dims[dim].brass.get_mut(&pos)?;
            b.mode = b.mode.scrolled(steps);
            let i = b.mode.index() as i32;
            w.mark_unsaved(dim, pos);
            Some(i)
        })
        .flatten()
        .unwrap_or(0),
        _ => return,
    };
    // The value back to the player (its value box shows it).
    let mut m = Vec::with_capacity(16);
    for v in [x, y, z, value] {
        m.extend_from_slice(&v.to_le_bytes());
    }
    let _ = game.native_net_send_to_player(player, VALUE, &m);
}

/// Values set by the wheel or a board, and the value a board asked for, from the server to the
/// player.
pub(super) const VALUE: &str = "create:value";

thread_local! {
    /// Values of the value boxes this client has seen set, by block.
    pub(super) static VALUES: RefCell<HashMap<[i32; 3], i32>> = RefCell::new(HashMap::new());
}

pub(super) fn values_from_server(d: &[u8]) {
    if d.len() != 16 {
        return;
    }
    let [x, y, z, v] = [0, 4, 8, 12].map(|i| i32::from_le_bytes(d[i..i + 4].try_into().unwrap()));
    let pos = [x, y, z];
    VALUES.with_borrow_mut(|m| m.insert(pos, v));
    // The setting a held press or an open board waits for (`getValueSettings`).
    HOLD.with_borrow_mut(|h| {
        if let Some(h) = h.as_mut().filter(|h| h.pos == pos) {
            h.known = true;
        }
    });
    BOARD.with_borrow_mut(|b| {
        if let Some(open) = b.as_mut().filter(|o| o.pos == pos) {
            if !open.moved {
                open.pick = board_pick(open.board, v);
            }
            open.known = true;
        }
    });
}

/// A pick on a value board (`ValueSettingsPacket`: position, row, column), a board's question
/// for the current setting (the position alone), or a short click on a value box (position and
/// whether a wrench is in hand), from the client.
pub(super) const VALUE_SETTINGS: &str = "create:value_settings";

/// `ValueSettingsPacket.applySettings` (`BlockEntityConfigurationPacket.handle`: no spectator or
/// adventure player), checked as `scroll_from_client` checks a scroll. The current setting goes
/// back either way, as the client's board starts on it.
pub(super) fn value_settings_from_client(game: &mut Game<'_>, player: u128, data: &[u8]) {
    let word = |i: usize| i32::from_le_bytes(data[i..i + 4].try_into().unwrap());
    // A short click's last word: whether the main hand holds a wrench (a guest's hand is the
    // client's to see).
    let ([x, y, z], pick, short) = match data.len() {
        12 => ([0, 4, 8].map(word), None, None),
        16 => ([0, 4, 8].map(word), None, Some(word(12) != 0)),
        20 => (
            [0, 4, 8].map(word),
            Some(create_engine::board::Settings {
                row: word(12),
                value: word(16),
            }),
            None,
        ),
        _ => return,
    };
    let Some(p) = game
        .native_server_players()
        .into_iter()
        .find(|p| p.uuid == player)
    else {
        return;
    };
    // GameType ids: adventure 2, spectator 3.
    if matches!(p.game_mode, 2 | 3) {
        return;
    }
    let Some(dim) = dimension_index(&p.dimension) else {
        return;
    };
    let center = [f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5];
    let d2: f64 = (0..3).map(|i| (p.position[i] - center[i]).powi(2)).sum();
    if d2 > 8.0 * 8.0 {
        return;
    }
    let pos = [x, y, z];
    let state = game.world.block(x, y, z).0;
    let block = with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten();
    use create_engine::Block as B;
    use create_engine::tunnel::{self, Mode};
    let value = match block {
        Some(B::CreativeMotor | B::RotationSpeedController) => with_world(|w| {
            if let Some(pick) = pick {
                w.board_motor(dim, pos, pick);
            }
            w.motor_setting(dim, pos).map(|rpm| rpm as i32)
        }),
        // `ScrollOptionBehaviour` over Create's seven `SelectionMode`s: `RANDOMIZE` and
        // `SYNCHRONIZE` are not ported and leave the mode as it is (as do connected tunnels,
        // which would take it too).
        Some(B::BrassTunnel) => with_world(|w| {
            let b = w.dims[dim].brass.get_mut(&pos)?;
            let now = b.mode.index() as i32;
            let picked = pick.and_then(|s| {
                create_engine::board::option_value(s, now, tunnel::OPTIONS.len() as i32)
            });
            if let Some(mode) = picked.and_then(Mode::from_option) {
                b.mode = mode;
                w.mark_unsaved(dim, pos);
            }
            w.dims[dim].brass.get(&pos).map(|b| b.mode.index() as i32)
        }),
        // `BrassDiodeScrollValueBehaviour`: `onShortInteract` toggles the diode (`toggle`: not
        // with a wrench; sneaking and no building were never a short click); `setValueSettings`
        // sets its delay.
        Some(B::PulseRepeater | B::PulseExtender | B::PulseTimer) => {
            if short == Some(false)
                && let Some((_, s)) = diode_state(state)
            {
                toggle_diode(game, &p.dimension, pos, s.inverted);
            }
            with_world(|w| {
                if let Some(pick) = pick {
                    w.set_diode_delay(dim, pos, create_engine::board::diode_value(pick));
                }
                w.dims[dim].diodes.get(&pos).map(|d| d.max)
            })
        }
        _ => return,
    };
    let Some(value) = value.flatten() else {
        return;
    };
    // The value back to the player (its value box and board show it).
    let mut m = Vec::with_capacity(16);
    for v in [x, y, z, value] {
        m.extend_from_slice(&v.to_le_bytes());
    }
    let _ = game.native_net_send_to_player(player, VALUE, &m);
}

/// `ValueSettingsInputHandler.canInteract` (not sneaking, a spectator or in adventure) and no
/// clipboard in the main hand (it copies settings instead).
pub(super) fn may_use_value_box(game: &Game<'_>) -> bool {
    game.may_build()
        && !game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft)
        && !game
            .inventory
            .get(game.selected)
            .and_then(|s| s.as_ref())
            .is_some_and(|s| &*s.item == "create:clipboard")
}

/// Whether a hit at `local` (in the block) on its face `side` lands on the active value box of
/// the block at `pos` (`ValueSettingsInputHandler.onBlockActivated`: `isActive`,
/// `Sided.isSideActive`, `testHit`): a creative motor's speed, a distributing brass tunnel's mode.
pub(super) fn value_box_hit(
    game: &Game<'_>,
    table: &StateTable,
    pos: [i32; 3],
    side: Dir,
    local: [f64; 3],
) -> bool {
    use create_engine::Block as B;
    use create_engine::board;
    let Some((block, info)) = table.get(game.world.block(pos[0], pos[1], pos[2]).0) else {
        return false;
    };
    let slot = match (block, info.axis, info.tunnel) {
        (B::CreativeMotor, ..) => info.facing.and_then(|f| board::motor_slot(f, side)),
        // `BrassDiodeScrollSlot`: not a sided box, any face reaches it.
        (B::PulseRepeater | B::PulseExtender | B::PulseTimer, ..) => {
            Some(create_engine::diode::SLOT)
        }
        // `selectionMode.onlyActiveWhen(this::hasDistributionBehaviour)`.
        (B::BrassTunnel, Some(axis), Some(shape)) => {
            let tunnel = |d: Dir| {
                let o = d.offset();
                let s = game.world.block(pos[0] + o[0], pos[1], pos[2] + o[2]).0;
                table
                    .get(s)
                    .is_some_and(|(b, _)| matches!(b, B::AndesiteTunnel | B::BrassTunnel))
            };
            create_engine::tunnel::distributes(axis, shape, tunnel)
                .then(|| board::tunnel_mode_slot(side))
                .flatten()
        }
        _ => None,
    };
    slot.is_some_and(|s| board::hits_box(s, local))
}

/// `ValueSettingsInputHandler.onBlockActivated` on the server: a click on an active value box is
/// taken (`setCanceled`: the block does not use it) whatever the hand holds but a clipboard. The
/// client holds it to open the board (`tick_value_board`); a short click does no more
/// (`ScrollValueBehaviour.onShortInteract` acts for fake players only).
pub(super) fn use_value_box(
    game: &mut Game<'_>,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    if !may_use_value_box(game) {
        return;
    }
    let (Some(side), Some(hit)) = (Dir::from_offset(face), game.pick()) else {
        return;
    };
    let local = [0, 1, 2].map(|i| hit.point[i] - f64::from(pos[i]));
    let game = &*game;
    if with_world(|w| value_box_hit(game, &w.table, pos, side, local)) == Some(true) {
        *canceled = true;
    }
}

/// A value board open on this client (`ValueSettingsScreen`).
pub(super) struct OpenBoard {
    pub(super) pos: [i32; 3],
    pub(super) board: create_engine::board::Board,
    /// The pick under the cursor: the wheel moves it, as there is no free cursor while playing.
    pub(super) pick: create_engine::board::Settings,
    pub(super) ticks: u32,
    /// Whether the server told the block's setting (the pick starts on it), and whether the
    /// wheel moved the pick: a board that knows neither sends nothing.
    pub(super) known: bool,
    pub(super) moved: bool,
}

/// The use press held on a value box (`ValueSettingsClient.interactHeldPos`,
/// `interactHeldTicks`), and whether the server told its setting yet.
pub(super) struct Held {
    pub(super) pos: [i32; 3],
    pub(super) hold: create_engine::board::Hold,
    pub(super) known: bool,
}

thread_local! {
    pub(super) static HOLD: RefCell<Option<Held>> = const { RefCell::new(None) };
    pub(super) static BOARD: RefCell<Option<OpenBoard>> = const { RefCell::new(None) };
    /// Whether use was down last tick: a new press is a click.
    pub(super) static USE_DOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A board's pick for the block's setting `v` (`getValueSettings`): a motor's speed, an option.
pub(super) fn board_pick(
    board: create_engine::board::Board,
    v: i32,
) -> create_engine::board::Settings {
    if board.icons {
        create_engine::board::Settings { row: 0, value: v }
    } else if board == create_engine::board::DIODE {
        create_engine::board::diode_settings(v)
    } else {
        create_engine::board::kinetic_settings(v)
    }
}

/// The board of the value box at `pos` (`createBoard`), starting on the block's setting when the
/// server told it (`known`), else on what this client sees: the creative motor's speed and a
/// brass tunnel's mode (the chassis' board needs the wrench, whose use turns the block here).
pub(super) fn board_of(game: &Game<'_>, pos: [i32; 3], known: bool) -> Option<OpenBoard> {
    use create_engine::Block as B;
    use create_engine::board;
    let state = game.world.block(pos[0], pos[1], pos[2]);
    states::index(state.0)?;
    let (block, facing, speed) = CLIENT.with_borrow(|c| {
        let c = c.as_ref()?;
        let (block, info) = c.table.get(state.0)?;
        Some((block, info.facing, c.speeds.get(&pos).map_or(0.0, |s| s[0])))
    })?;
    let seen = VALUES.with_borrow(|m| m.get(&pos).copied());
    let board = match block {
        B::CreativeMotor => board::MOTOR,
        // `BrassTunnelBlockEntity.selectionMode` (`logistics.when_multiple_outputs_available`).
        B::BrassTunnel => board::options(
            "Distribution Method",
            create_engine::tunnel::OPTIONS.len() as i32,
        ),
        B::PulseRepeater | B::PulseExtender | B::PulseTimer => board::DIODE,
        _ => return None,
    };
    let setting = match (block, seen.filter(|_| known)) {
        (_, Some(v)) => v,
        (B::CreativeMotor, None) => facing.map_or(speed, |f| {
            create_engine::kinetics::convert_to_direction(speed, f)
        }) as i32,
        (_, None) => seen.unwrap_or_else(|| {
            create_engine::world::diode_kind(block).map_or(0, |k| k.default_delay())
        }),
    };
    Some(OpenBoard {
        pos,
        board,
        pick: board_pick(board, setting),
        ticks: 0,
        known,
        moved: false,
    })
}

/// The block under the crosshair when the hit lands on its active value box.
pub(super) fn aimed_value_box(game: &Game<'_>) -> Option<[i32; 3]> {
    let hit = game.pick()?;
    let pos = hit.pos;
    states::index(game.world.block(pos[0], pos[1], pos[2]).0)?;
    let side = Dir::from_offset(hit.face)?;
    let local = [0, 1, 2].map(|i| hit.point[i] - f64::from(pos[i]));
    CLIENT
        .with_borrow(|c| Some(value_box_hit(game, &c.as_ref()?.table, pos, side, local)))?
        .then_some(pos)
}

/// `ValueSettingsClient.tick` and the board's release (`ValueSettingsScreen.saveAndClose`): a
/// use click on an active value box starts a hold, which goes on while the player may interact
/// and aims at that box; on its sixth tick the board opens. Letting go of use sends the pick.
pub(super) fn tick_value_board(game: &mut Game<'_>) {
    // Hot: `keyUse.isDown()` is one load; nothing else runs while no press is new or held.
    let down = game.right;
    let pressed = down && !USE_DOWN.replace(down);
    let released = BOARD.with_borrow_mut(|b| {
        let open = b.as_mut()?;
        open.ticks += 1;
        if down { None } else { b.take() }
    });
    if let Some(open) = released {
        // Never a guess: unmoved and with no answer from the server, nothing changes.
        if !open.known && !open.moved {
            return;
        }
        let mut data = Vec::with_capacity(20);
        for v in [
            open.pos[0],
            open.pos[1],
            open.pos[2],
            open.pick.row,
            open.pick.value,
        ] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        let _ = game.native_net_send_to_server(VALUE_SETTINGS, &data);
        return;
    }
    if BOARD.with_borrow(Option::is_some) {
        return;
    }
    let holding = HOLD.with_borrow(Option::is_some);
    if !holding && !pressed {
        return;
    }
    let aimed = if may_use_value_box(game) {
        aimed_value_box(game)
    } else {
        None
    };
    if !holding {
        // `startInteractionWith`, and the server asked for the setting the board starts on.
        let Some(pos) = aimed else {
            return;
        };
        HOLD.with_borrow_mut(|h| {
            *h = Some(Held {
                pos,
                hold: Default::default(),
                known: false,
            })
        });
        let mut data = Vec::with_capacity(12);
        for v in pos {
            data.extend_from_slice(&v.to_le_bytes());
        }
        let _ = game.native_net_send_to_server(VALUE_SETTINGS, &data);
    }
    // `cancelInteraction` when the player may no longer interact or looks off the box.
    let step = HOLD.with_borrow_mut(|h| {
        let held = h.as_mut().filter(|held| Some(held.pos) == aimed)?;
        Some((held.pos, held.known, held.hold.tick(down)))
    });
    match step {
        Some((_, _, create_engine::board::Step::Wait)) => {}
        Some((pos, known, create_engine::board::Step::Open)) => {
            HOLD.with_borrow_mut(|h| *h = None);
            if let Some(open) = board_of(game, pos, known) {
                BOARD.with_borrow_mut(|b| *b = Some(open));
            }
        }
        // A short click: `onShortInteract` on the server (`ValueSettingsPacket` with its hand:
        // whether it holds a wrench).
        Some((pos, _, create_engine::board::Step::Short)) => {
            HOLD.with_borrow_mut(|h| *h = None);
            let wrench = game
                .inventory
                .get(game.selected)
                .and_then(|s| s.as_ref())
                .is_some_and(|s| &*s.item == "create:wrench");
            let mut data = Vec::with_capacity(16);
            for v in [pos[0], pos[1], pos[2], i32::from(wrench)] {
                data.extend_from_slice(&v.to_le_bytes());
            }
            let _ = game.native_net_send_to_server(VALUE_SETTINGS, &data);
        }
        // The press is off the box.
        None => HOLD.with_borrow_mut(|h| *h = None),
    }
}

/// `ValueSettingsScreen.mouseScrolled` on the open board: a column a step, a milestone with
/// sneak held (for shift). Whether a board took the wheel.
pub(super) fn scroll_board(game: &mut Game<'_>, steps: f32) -> bool {
    if BOARD.with_borrow(Option::is_none) {
        return false;
    }
    if steps == 0.0 {
        return true;
    }
    let shift = game.native_mapping("key.sneak").is_some_and(|m| m.0);
    BOARD.with_borrow_mut(|b| {
        if let Some(open) = b.as_mut()
            && let Some(column) = open.board.scrolled(open.pick, steps > 0.0, shift)
        {
            open.pick.value = column;
            open.moved = true;
        }
    });
    true
}

/// `ValueSettingsScreen.renderWindow`, with plain fills where Create draws its GUI sprites
/// (`AllGuiTextures.VALUE_SETTINGS_*`, from the player's jar). Whether a board is open.
pub(super) fn value_board(game: &mut Game<'_>, batch: &mut horizon_client::hud::HudBatch) -> bool {
    use create_engine::board::{self, Layout, Settings};
    use create_engine::tunnel::OPTIONS;
    let Some((b, pick, ticks)) =
        BOARD.with_borrow(|b| b.as_ref().map(|o| (o.board, o.pick, o.ticks)))
    else {
        return false;
    };
    let label = b
        .rows
        .iter()
        .map(|r| game.native_text_width(r) as i32)
        .max()
        .unwrap_or(0);
    let l = Layout::new(b, label, [batch.width as i32, batch.height as i32]);
    let key = game
        .native_mapping("key.use")
        .map_or_else(|| "Use".to_owned(), |m| m.1);
    let tip = board::release_to_confirm(&key);
    let text = |s: Settings| {
        if b.icons {
            OPTIONS[s.value.clamp(0, OPTIONS.len() as i32 - 1) as usize].to_owned()
        } else if b == board::DIODE {
            board::diode_text(s)
        } else {
            board::kinetic_text(s)
        }
    };
    let mut fattest =
        (game.native_text_width(&tip) as i32).max(game.native_text_width(b.title) as i32);
    if b.icons {
        for value in 0..=b.max {
            let w = game.native_text_width(&text(Settings { row: 0, value }));
            fattest = fattest.max(w as i32);
        }
    }
    let f = |v: i32| v as f32;
    let centred =
        |game: &Game<'_>, text: &str, x: i32| f(x - game.native_text_width(text) as i32 / 2);
    let open = ticks as f32;
    game.native_fill(
        batch,
        [0.0, 0.0, batch.width, batch.height],
        board::shade(open),
    );
    let fr = l.frame(fattest, open);
    game.native_fill(
        batch,
        [f(fr.x), f(fr.y), f(fr.width), f(fr.height)],
        0xE02A_1E14,
    );
    if fr.text {
        let at = [centred(game, b.title, fr.text_x), f(fr.title_y)];
        game.native_text(batch, b.title, at, 0xDD_DDDD, false);
        let at = [centred(game, &tip, fr.text_x), f(fr.tip_y)];
        game.native_text(batch, &tip, at, 0xDD_DDDD, false);
    }
    for (i, row) in b.rows.iter().enumerate() {
        let y = l.top + i as i32 * 11;
        if !b.icons {
            game.native_fill(
                batch,
                [f(l.left - 4), f(y), f(l.label + 8), 11.0],
                0xFFC8_A864,
            );
            let bar = [f(l.bar_x()), f(y + 1), f(b.bar_width()), 8.0];
            game.native_fill(batch, bar, 0xFF8A_6A3A);
            game.native_text(batch, row, [f(l.left), f(y + 1)], 0x44_2000, false);
        }
        for x in l.milestones() {
            let mark = [f(x), f(y + 1), f(b.milestone_size()), 8.0];
            game.native_fill(batch, mark, 0xFF44_2000);
        }
    }
    let label = text(pick);
    let width = if b.icons {
        16
    } else {
        game.native_text_width(&label) as i32
    };
    let [x, y, w] = Layout::cursor(l.coordinate(pick.row, pick.value), width);
    game.native_fill(batch, [f(x - 3), f(y), f(w + 6), 14.0], 0xFFFB_DC7D);
    if !b.icons {
        game.native_text(batch, &label, [f(x + 2), f(y + 3)], 0x44_2000, false);
    } else if fr.text {
        let at = [centred(game, &label, fr.text_x), f(fr.option_y)];
        game.native_text(batch, &label, at, 0xFB_DC7D, false);
    }
    true
}

/// `GoggleOverlayRenderer` for kinetic blocks: with goggles on the head, the block under the
/// crosshair tells its stress (`KineticBlockEntity.addToGoggleTooltip`: impact × speed;
/// `GeneratingKineticBlockEntity`: capacity × speed), in a tooltip right of the crosshair.
pub(super) fn hud(game: &mut Game<'_>, batch: &mut horizon_client::hud::HudBatch) {
    if value_board(game, batch) {
        return;
    }
    value_tip(game, batch);
    goggle_overlay(game, batch);
}

/// `ValueSettingsClient.render` for the creative motor's speed (`KineticScrollValueBehaviour`):
/// its label over the hotbar, centred, with the value and how to change it here (the wheel;
/// holding use opens Create 6's value board, `value_board`).
pub(super) fn value_tip(game: &mut Game<'_>, batch: &mut horizon_client::hud::HudBatch) {
    // Free when unused: no ray while no kinetic block has a speed here and no wrench is held.
    let wrench = game
        .inventory
        .get(game.selected)
        .and_then(|s| s.as_ref())
        .is_some_and(|s| &*s.item == "create:wrench");
    if !wrench && CLIENT.with_borrow(|c| c.as_ref().is_none_or(|c| c.speeds.is_empty())) {
        return;
    }
    let Some(hit) = game.pick() else {
        return;
    };
    let pos = hit.pos;
    let state = game.world.block(pos[0], pos[1], pos[2]);
    // Hot: one range test for any other block.
    if states::index(state.0).is_none() || !client_ensure(game) {
        return;
    }
    let Some((block, facing, speed)) = CLIENT.with_borrow(|c| {
        let c = c.as_ref()?;
        let (block, info) = c.table.get(state.0)?;
        Some((block, info.facing, c.speeds.get(&pos).map_or(0.0, |s| s[0])))
    }) else {
        return;
    };
    use create_engine::Block as B;
    let seen = VALUES.with_borrow(|m| m.get(&pos).copied());
    let (label, value) = match block {
        // The setting, from the speed the motor gives along its axis.
        B::CreativeMotor => {
            let setting = facing.map_or(speed, |f| {
                create_engine::kinetics::convert_to_direction(speed, f)
            });
            ("Generated Speed in RPM", format!("{setting:.0}"))
        }
        // `ChassisScrollValueBehaviour` (with the wrench).
        B::LinearChassis | B::SecondaryLinearChassis if wrench => (
            "Range of Sticky Sides",
            seen.unwrap_or(create_engine::contraption::CHASSIS_RANGE)
                .to_string(),
        ),
        // `BrassTunnelBlockEntity.selectionMode`.
        B::BrassTunnel => (
            "Distribution Method",
            create_engine::tunnel::Mode::ALL[seen.unwrap_or(0).clamp(0, 4) as usize]
                .label()
                .to_owned(),
        ),
        _ => return,
    };
    let lines = [
        (label.to_owned(), 0xFB_DC7D),
        (value, 0xFF_FFFF),
        ("Scroll to change".to_owned(), 0xFF_FFFF),
    ];
    let mut y = batch.height - 75.0 - lines.len() as f32 * 12.0;
    for (text, rgb) in &lines {
        let w = game.native_text_width(text);
        game.native_text(batch, text, [batch.width / 2.0 - w / 2.0, y], *rgb, true);
        y += 12.0;
    }
}

/// `GoggleOverlayRenderer` for kinetic blocks.
pub(super) fn goggle_overlay(game: &mut Game<'_>, batch: &mut horizon_client::hud::HudBatch) {
    // Hot: one compare on the head slot.
    if !game.armor[0]
        .as_ref()
        .is_some_and(|s| &*s.item == "create:goggles")
    {
        return;
    }
    if !client_ensure(game) {
        return;
    }
    let Some(hit) = game.pick() else {
        return;
    };
    let pos = hit.pos;
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some((block, impact, capacity, speed)) = CLIENT.with_borrow(|c| {
        let c = c.as_ref()?;
        let (block, info) = c.table.get(state.0)?;
        let [part, _] = create_engine::parts::parts(block, info, &Default::default());
        let part = part?;
        let speed = c.speeds.get(&pos).map_or(0.0, |s| s[0]);
        Some((block, part.impact, part.capacity, speed))
    }) else {
        return;
    };
    // Create's tooltip: a header, then each stat indented by four spaces and its value by one more;
    // a line may change colour along the way.
    let mut lines: Vec<Vec<(String, u32)>> = Vec::new();
    // LangNumberFormat: grouped thousands, at most two decimals (en_us).
    let number = |v: f32| {
        let text = format!("{:.2}", v);
        let text = text.trim_end_matches('0').trim_end_matches('.');
        let (int, frac) = text.split_once('.').unwrap_or((text, ""));
        let mut grouped = String::new();
        for (i, c) in int.chars().enumerate() {
            if i > 0 && (int.len() - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(c);
        }
        if frac.is_empty() {
            grouped
        } else {
            format!("{grouped}.{frac}")
        }
    };
    if impact != 0.0 {
        lines.push(vec![("    Kinetic Stats:".into(), 0xFFFFFF)]);
        lines.push(vec![("    Kinetic Stress Impact:".into(), 0xAAAAAA)]);
        lines.push(vec![(
            format!("     {}su at current speed", number(impact * speed.abs())),
            0x55FFFF,
        )]);
    }
    if capacity != 0.0 {
        lines.push(vec![("    Generator Stats:".into(), 0xFFFFFF)]);
        lines.push(vec![("    Kinetic Stress Capacity:".into(), 0xAAAAAA)]);
        lines.push(vec![(
            format!("     {}su at current speed", number(capacity * speed.abs())),
            0x55FFFF,
        )]);
    }
    // GaugeBlockEntity.addToGoggleTooltip: what the server last said of it.
    if let Some(kind) = create_engine::world::gauge_kind(block) {
        let shown = GAUGES.with_borrow(|g| g.shown.get(&pos).copied().unwrap_or_default());
        lines.extend(create_engine::gauge::tooltip(kind, &shown));
    }
    if lines.is_empty() {
        return;
    }
    let line_width =
        |l: &[(String, u32)]| -> f32 { l.iter().map(|(t, _)| game.native_text_width(t)).sum() };
    // RemovedGuiUtils.drawHoveringText at (width / 2 + 20, height / 2): vanilla's tooltip box.
    let w = lines.iter().map(|l| line_width(l)).fold(0.0f32, f32::max);
    let h = lines.len() as f32 * 10.0 - 2.0;
    let x = (batch.width / 2.0 + 20.0).min(batch.width - w - 20.0) + 4.0;
    let y = (batch.height / 2.0).min(batch.height - h - 20.0) + 4.0;
    game.native_fill(batch, [x - 3.0, y - 4.0, w + 6.0, h + 8.0], 0xF010_0010);
    game.native_fill(batch, [x - 3.0, y - 3.0, w + 6.0, 1.0], 0x5050_00FF);
    game.native_fill(batch, [x - 3.0, y + h + 2.0, w + 6.0, 1.0], 0x5028_007F);
    for (i, line) in lines.iter().enumerate() {
        let mut dx = 0.0;
        for (text, rgb) in line {
            game.native_text(batch, text, [x + dx, y + i as f32 * 10.0], *rgb, true);
            dx += game.native_text_width(text);
        }
    }
}

/// The mouse wheel on the client: a creative motor under the crosshair takes it.
pub(super) fn wheel(game: &mut Game<'_>, steps: f32, claimed: &mut bool) {
    if *claimed || !client_ensure(game) {
        return;
    }
    if scroll_board(game, steps) {
        *claimed = true;
        return;
    }
    let Some(hit) = game.pick() else {
        return;
    };
    let pos = hit.pos;
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let wrench = game
        .inventory
        .get(game.selected)
        .and_then(|s| s.as_ref())
        .is_some_and(|s| &*s.item == "create:wrench");
    // The motor's or speed controller's speed, a brass tunnel's mode, and (with the wrench,
    // `requiresWrench`) a chassis' range.
    let scrolls = CLIENT.with_borrow(|c| {
        c.as_ref()
            .and_then(|c| c.table.get(state.0))
            .is_some_and(|(b, _)| {
                use create_engine::Block as B;
                matches!(
                    b,
                    B::CreativeMotor | B::RotationSpeedController | B::BrassTunnel
                ) || wrench && matches!(b, B::LinearChassis | B::SecondaryLinearChassis)
            })
    });
    if !scrolls {
        return;
    }
    *claimed = true;
    let steps = steps.round() as i32;
    let mut data = Vec::with_capacity(16);
    for v in [pos[0], pos[1], pos[2], steps] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    let _ = game.native_net_send_to_server(SCROLL, &data);
}

/// Settings saved with a column: `motors` holds x, y, z and speed for each motor.
pub(super) fn settings_from_nbt(
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<([i32; 3], i32)> {
    let Some(horizon_world::nbt::Tag::IntArray(v)) = data.and_then(|d| d.get(&"motors".into()))
    else {
        return Vec::new();
    };
    v.chunks_exact(4)
        .map(|c| ([c[0], c[1], c[2]], c[3]))
        .collect()
}

/// Belts' items as saved (`BeltInventory.write`): for each belt its controller (`pos`), each
/// item's numbers (position and side offset as float bits, angle, count, locked) and its id.
pub(super) fn belts_to_nbt(
    belts: &[([i32; 3], create_engine::belt::Inventory)],
) -> horizon_world::nbt::Tag {
    use horizon_world::nbt::{Compound, Tag};
    Tag::List(
        belts
            .iter()
            .map(|(c, inv)| {
                let mut b = Compound::new();
                b.insert("pos".into(), Tag::IntArray(c.to_vec()));
                let numbers = inv
                    .items
                    .iter()
                    .flat_map(|i| {
                        [
                            i.position.to_bits() as i32,
                            i.side_offset.to_bits() as i32,
                            i.angle,
                            i32::from(i.count),
                            i32::from(i.locked),
                        ]
                    })
                    .collect();
                b.insert("items".into(), Tag::IntArray(numbers));
                b.insert(
                    "ids".into(),
                    Tag::List(
                        inv.items
                            .iter()
                            .map(|i| Tag::String(i.item.as_str().into()))
                            .collect(),
                    ),
                );
                b.insert(
                    "positive".into(),
                    Tag::IntArray(vec![i32::from(inv.positive)]),
                );
                Tag::Compound(b)
            })
            .collect(),
    )
}

/// Crushing wheel controllers' contents (`ProcessingInventory`): each one's position, time left
/// (float bits) and whether its recipe was applied, its stacks' ids and counts.
/// `LinkBehaviour.write` (`FrequencyFirst`, `FrequencyLast`) and `Transmit`, per link.
pub(super) fn links_to_nbt(
    links: &[([i32; 3], create_engine::link::Key, i32)],
) -> horizon_world::nbt::Tag {
    use horizon_world::nbt::{Compound, Tag};
    Tag::List(
        links
            .iter()
            .map(|(p, key, transmit)| {
                let mut b = Compound::new();
                b.insert(
                    "pos".into(),
                    Tag::IntArray(vec![p[0], p[1], p[2], *transmit]),
                );
                b.insert("FrequencyFirst".into(), Tag::String(key.0.as_str().into()));
                b.insert("FrequencyLast".into(), Tag::String(key.1.as_str().into()));
                Tag::Compound(b)
            })
            .collect(),
    )
}

/// `DeployerBlockEntity.write`: `State`, `Timer`, `Reach`, the held stack and `Mode`, per deployer.
pub(super) fn deployers_to_nbt(
    list: &[([i32; 3], create_engine::deployer::Deployer)],
) -> horizon_world::nbt::Tag {
    use horizon_world::nbt::{Compound, Tag};
    Tag::List(
        list.iter()
            .map(|(p, d)| {
                let mut b = Compound::new();
                let (item, n) = d.held.clone().unwrap_or_default();
                b.insert(
                    "pos".into(),
                    Tag::IntArray(vec![
                        p[0],
                        p[1],
                        p[2],
                        i32::from(d.state.index()),
                        d.timer,
                        d.reach.to_bits() as i32,
                        i32::from(n),
                        i32::from(d.mode.index()),
                    ]),
                );
                b.insert("HeldItem".into(), Tag::String(item.as_str().into()));
                Tag::Compound(b)
            })
            .collect(),
    )
}

pub(super) fn deployers_from_nbt(
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<([i32; 3], create_engine::deployer::Deployer)> {
    use horizon_world::nbt::Tag;
    let Some(Tag::List(list)) = data.and_then(|d| d.get(&"deployers".into())) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|b| {
            let Tag::Compound(b) = b else { return None };
            let Some(Tag::IntArray(v)) = b.get(&"pos".into()) else {
                return None;
            };
            let item = match b.get(&"HeldItem".into()) {
                Some(Tag::String(s)) => s.to_string_lossy(),
                _ => String::new(),
            };
            let n = (*v.get(6)?).clamp(0, 64) as u8;
            Some((
                [*v.first()?, *v.get(1)?, *v.get(2)?],
                create_engine::deployer::Deployer {
                    state: create_engine::deployer::State::from_index(*v.get(3)? as u8),
                    timer: *v.get(4)?,
                    reach: f32::from_bits(*v.get(5)? as u32),
                    held: (!item.is_empty() && n > 0).then_some((item, n)),
                    // Saved before modes: using.
                    mode: create_engine::deployer::Mode::from_index(
                        v.get(7).copied().unwrap_or(0) as u8
                    ),
                    breaking: None,
                },
            ))
        })
        .collect()
}

pub(super) fn links_from_nbt(
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<([i32; 3], (create_engine::link::Key, i32))> {
    use horizon_world::nbt::Tag;
    let Some(Tag::List(list)) = data.and_then(|d| d.get(&"links".into())) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|b| {
            let Tag::Compound(b) = b else { return None };
            let Some(Tag::IntArray(pos)) = b.get(&"pos".into()) else {
                return None;
            };
            let item = |k: &str| match b.get(&k.into()) {
                Some(Tag::String(s)) => s.to_string_lossy(),
                _ => String::new(),
            };
            Some((
                [*pos.first()?, *pos.get(1)?, *pos.get(2)?],
                (
                    (item("FrequencyFirst"), item("FrequencyLast")),
                    (*pos.get(3)?).clamp(0, 15),
                ),
            ))
        })
        .collect()
}

pub(super) fn crushers_to_nbt(
    crushers: &[([i32; 3], create_engine::crusher::Crusher)],
) -> horizon_world::nbt::Tag {
    use horizon_world::nbt::{Compound, Tag};
    Tag::List(
        crushers
            .iter()
            .map(|(p, c)| {
                let mut b = Compound::new();
                b.insert(
                    "pos".into(),
                    Tag::IntArray(vec![
                        p[0],
                        p[1],
                        p[2],
                        c.remaining.to_bits() as i32,
                        i32::from(c.applied),
                    ]),
                );
                b.insert(
                    "ids".into(),
                    Tag::List(
                        c.slots
                            .iter()
                            .map(|s| Tag::String(s.0.as_str().into()))
                            .collect(),
                    ),
                );
                b.insert(
                    "counts".into(),
                    Tag::IntArray(c.slots.iter().map(|s| i32::from(s.1)).collect()),
                );
                Tag::Compound(b)
            })
            .collect(),
    )
}

pub(super) fn crushers_from_nbt(
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<([i32; 3], create_engine::crusher::Crusher)> {
    use horizon_world::nbt::Tag;
    let Some(Tag::List(list)) = data.and_then(|d| d.get(&"crushers".into())) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|b| {
            let Tag::Compound(b) = b else { return None };
            let Some(Tag::IntArray(pos)) = b.get(&"pos".into()) else {
                return None;
            };
            let Some(Tag::List(ids)) = b.get(&"ids".into()) else {
                return None;
            };
            let Some(Tag::IntArray(counts)) = b.get(&"counts".into()) else {
                return None;
            };
            let slots = ids
                .iter()
                .zip(counts)
                .filter_map(|(id, n)| match id {
                    Tag::String(id) => Some((id.to_string_lossy(), (*n).clamp(1, 255) as u8)),
                    _ => None,
                })
                .collect();
            let c = create_engine::crusher::Crusher {
                entity: None,
                slots,
                remaining: f32::from_bits(*pos.get(3)? as u32),
                applied: *pos.get(4)? != 0,
            };
            Some(([*pos.first()?, *pos.get(1)?, *pos.get(2)?], c))
        })
        .collect()
}

/// Brass diodes saved with a column: position, then `State` and the delay.
pub(super) fn diodes_from_nbt(
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<([i32; 3], (i32, i32))> {
    let Some(horizon_world::nbt::Tag::IntArray(v)) = data.and_then(|d| d.get(&"diodes".into()))
    else {
        return Vec::new();
    };
    v.chunks_exact(5)
        .map(|c| ([c[0], c[1], c[2]], (c[3], c[4])))
        .collect()
}

pub(super) fn burners_from_nbt(
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<([i32; 3], create_engine::burner::Burner)> {
    let Some(horizon_world::nbt::Tag::IntArray(v)) = data.and_then(|d| d.get(&"burners".into()))
    else {
        return Vec::new();
    };
    v.chunks_exact(5)
        .map(|c| {
            let b = create_engine::burner::Burner {
                fuel: create_engine::burner::Fuel::from_index(c[3]),
                remaining: c[4],
            };
            ([c[0], c[1], c[2]], b)
        })
        .collect()
}

pub(super) fn belts_from_nbt(
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<([i32; 3], create_engine::belt::Inventory)> {
    use horizon_world::nbt::Tag;
    let Some(Tag::List(list)) = data.and_then(|d| d.get(&"belts".into())) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|b| {
            let Tag::Compound(b) = b else { return None };
            let Some(Tag::IntArray(pos)) = b.get(&"pos".into()) else {
                return None;
            };
            let Some(Tag::IntArray(numbers)) = b.get(&"items".into()) else {
                return None;
            };
            let Some(Tag::List(ids)) = b.get(&"ids".into()) else {
                return None;
            };
            let positive = matches!(b.get(&"positive".into()), Some(Tag::IntArray(v)) if v.first() == Some(&1));
            let items = numbers
                .chunks_exact(5)
                .zip(ids)
                .filter_map(|(n, id)| {
                    let Tag::String(id) = id else { return None };
                    let mut item = create_engine::belt::Item::new(
                        &id.to_string_lossy(),
                        n[3].clamp(1, 255) as u8,
                        n[2],
                        false,
                    );
                    item.position = f32::from_bits(n[0] as u32);
                    item.side_offset = f32::from_bits(n[1] as u32);
                    item.locked = n[4] != 0;
                    Some(item)
                })
                .collect();
            let c = [*pos.first()?, *pos.get(1)?, *pos.get(2)?];
            Some((c, create_engine::belt::Inventory::from_saved(items, positive)))
        })
        .collect()
}

/// A chunk is being saved: Create's settings in it, as NBT (the only time it writes NBT).
pub(super) fn chunk_data_save(
    game: &Game<'_>,
    dimension: &str,
    chunk: [i32; 2],
    data: &mut horizon_world::nbt::Compound,
) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let settings = with_world(|w| w.column_settings(dim, chunk[0], chunk[1])).unwrap_or_default();
    let belts = with_world(|w| w.column_belts(dim, chunk[0], chunk[1])).unwrap_or_default();
    let tanks = with_world(|w| w.column_tanks(dim, chunk[0], chunk[1])).unwrap_or_default();
    let windmills = with_world(|w| w.column_windmills(dim, chunk[0], chunk[1])).unwrap_or_default();
    let crushers = with_world(|w| w.column_crushers(dim, chunk[0], chunk[1])).unwrap_or_default();
    let trains = with_world(|w| w.column_trains(dim, chunk[0], chunk[1])).unwrap_or_default();
    let names =
        with_world(|w| w.dims[dim].railway.column_names(chunk[0], chunk[1])).unwrap_or_default();
    let burners = with_world(|w| w.column_burners(dim, chunk[0], chunk[1])).unwrap_or_default();
    let links = with_world(|w| w.column_links(dim, chunk[0], chunk[1])).unwrap_or_default();
    let curves =
        with_world(|w| w.dims[dim].track_curves.column(chunk[0], chunk[1])).unwrap_or_default();
    let diodes = with_world(|w| w.column_diodes(dim, chunk[0], chunk[1])).unwrap_or_default();
    let deployers = with_world(|w| w.column_deployers(dim, chunk[0], chunk[1])).unwrap_or_default();
    let brass = with_world(|w| w.column_brass(dim, chunk[0], chunk[1])).unwrap_or_default();
    // Basins' tanks: x, y, z, then fluid (255: none) and amount for each of the four.
    let basins: Vec<i32> = with_world(|w| {
        let mut v: Vec<([i32; 3], create_engine::basin::Tanks)> = w.dims[dim]
            .basin_fluids
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]))
            .map(|(p, t)| (*p, *t))
            .collect();
        v.sort_by_key(|e| e.0);
        v.into_iter()
            .flat_map(|(p, t)| {
                let mut out = p.to_vec();
                for f in t.input.iter().chain(&t.output) {
                    out.push(f.map_or(255, |f| i32::from(f.0.index())));
                    out.push(f.map_or(0, |f| f.1 as i32));
                }
                out
            })
            .collect()
    })
    .unwrap_or_default();
    // Chassis ranges set with the wrench: x, y, z, range each.
    let ranges: Vec<i32> = with_world(|w| {
        let mut v: Vec<([i32; 3], i32)> = w.dims[dim]
            .chassis_range
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]))
            .map(|(p, r)| (*p, *r))
            .collect();
        v.sort();
        v.into_iter()
            .flat_map(|(p, r)| [p[0], p[1], p[2], r])
            .collect()
    })
    .unwrap_or_default();
    let vaults = with_world(|w| w.dims[dim].vaults.column(chunk[0], chunk[1])).unwrap_or_default();
    let filters: Vec<([i32; 3], String)> = with_world(|w| {
        let mut v: Vec<([i32; 3], String)> = w.dims[dim]
            .chute_filters
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]))
            .map(|(p, f)| (*p, f.clone()))
            .collect();
        v.sort();
        v
    })
    .unwrap_or_default();
    if settings.is_empty()
        && vaults.is_empty()
        && filters.is_empty()
        && belts.is_empty()
        && tanks.is_empty()
        && windmills.is_empty()
        && crushers.is_empty()
        && burners.is_empty()
        && links.is_empty()
        && diodes.is_empty()
        && deployers.is_empty()
        && ranges.is_empty()
        && basins.is_empty()
        && brass.is_empty()
        && trains.is_empty()
        && names.is_empty()
        && curves.is_empty()
    {
        data.remove(&"create".into());
        return;
    }
    let mut own = horizon_world::nbt::Compound::new();
    vaults_to_nbt(&mut own, &vaults, &filters);
    if !settings.is_empty() {
        let motors: Vec<i32> = settings
            .iter()
            .flat_map(|(p, rpm)| [p[0], p[1], p[2], *rpm])
            .collect();
        own.insert("motors".into(), horizon_world::nbt::Tag::IntArray(motors));
    }
    if !belts.is_empty() {
        own.insert("belts".into(), belts_to_nbt(&belts));
    }
    if !crushers.is_empty() {
        own.insert("crushers".into(), crushers_to_nbt(&crushers));
    }
    if !trains.is_empty() {
        own.insert("trains".into(), trains_to_nbt(game, &trains));
    }
    if !names.is_empty() {
        own.insert("station_names".into(), station_names_to_nbt(&names));
    }
    if !links.is_empty() {
        own.insert("links".into(), links_to_nbt(&links));
    }
    if !curves.is_empty() {
        own.insert("track_curves".into(), curves_to_nbt(&curves));
    }
    if !diodes.is_empty() {
        // Brass diodes (`BrassDiodeBlockEntity.write`): x, y, z, `State` and the delay each.
        let v: Vec<i32> = diodes
            .iter()
            .flat_map(|(p, state, max)| [p[0], p[1], p[2], *state, *max])
            .collect();
        own.insert("diodes".into(), horizon_world::nbt::Tag::IntArray(v));
    }
    if !deployers.is_empty() {
        own.insert("deployers".into(), deployers_to_nbt(&deployers));
    }
    if !ranges.is_empty() {
        own.insert("chassis".into(), horizon_world::nbt::Tag::IntArray(ranges));
    }
    if !brass.is_empty() {
        // x, y, z, mode, side it came in by (6: none), count, then the item ids.
        use horizon_world::nbt::Tag;
        let mut nums = Vec::new();
        let mut items = Vec::new();
        for (p, b) in &brass {
            let (item, n) = b.stack.clone().unwrap_or_default();
            nums.extend([
                p[0],
                p[1],
                p[2],
                b.mode.index() as i32,
                b.entered_from.map_or(6, |d| d as i32),
                i32::from(n),
            ]);
            items.push(Tag::String(item.as_str().into()));
        }
        let mut c = horizon_world::nbt::Compound::new();
        c.insert("nums".into(), Tag::IntArray(nums));
        c.insert("items".into(), Tag::List(items));
        own.insert("brass".into(), Tag::Compound(c));
    }
    if !basins.is_empty() {
        own.insert("basins".into(), horizon_world::nbt::Tag::IntArray(basins));
    }
    if !burners.is_empty() {
        // x, y, z, fuel type, burn time left for each burner (`fuelLevel`, `burnTimeRemaining`).
        let v = burners
            .iter()
            .flat_map(|(p, b)| [p[0], p[1], p[2], b.fuel.index(), b.remaining])
            .collect();
        own.insert("burners".into(), horizon_world::nbt::Tag::IntArray(v));
    }
    if !tanks.is_empty() {
        // x, y, z, fluid, amount, width, height, windows for each controller
        // (`FluidTankBlockEntity`'s `TankContent`, `Size`, `Height`, `Window`).
        let v = tanks
            .iter()
            .flat_map(|(p, m)| {
                [
                    p[0],
                    p[1],
                    p[2],
                    i32::from(m.tank.fluid.map_or(255, |f| f.index())),
                    m.tank.amount as i32,
                    i32::from(m.width),
                    i32::from(m.height),
                    i32::from(m.window),
                ]
            })
            .collect();
        own.insert("tank_multis".into(), horizon_world::nbt::Tag::IntArray(v));
    }
    if !windmills.is_empty() {
        own.insert("windmills".into(), windmills_to_nbt(game, &windmills));
    }
    data.insert("create".into(), horizon_world::nbt::Tag::Compound(own));
}

/// The client's side: the parts it shows, from the server's speeds.
pub(super) struct ClientParts {
    pub(super) table: StateTable,
    pub(super) speeds: HashMap<[i32; 3], [f32; 2]>,
    pub(super) models: HashMap<(u32, Model), u32>,
    /// Speeds that came before their block did: tried again for a few ticks.
    pub(super) pending: Vec<([i32; 3], u8)>,
    pub(super) synced: bool,
}

thread_local! {
    pub(super) static CLIENT: RefCell<Option<ClientParts>> = const { RefCell::new(None) };
}

pub(super) fn axis_index(a: Axis) -> u8 {
    match a {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    }
}

pub(super) fn dir_name(d: Dir) -> &'static str {
    match d {
        Dir::Down => "down",
        Dir::Up => "up",
        Dir::North => "north",
        Dir::South => "south",
        Dir::West => "west",
        Dir::East => "east",
    }
}

/// The state whose model a part is: the block's own, or Create's shaft / half shaft.
pub(super) fn model_state(
    registry: &BlockRegistry,
    own: BlockStateId,
    model: Model,
) -> Option<BlockStateId> {
    let find = |name: &str, key: &str, value: &str| {
        let default = registry.default_state(name)?;
        let mut s = registry.state(default)?.clone();
        s.properties.insert(key.to_owned(), value.to_owned());
        registry.id(&s)
    };
    match model {
        Model::Own => Some(own),
        Model::Shaft(a) => find(
            "create:shaft",
            "axis",
            ["x", "y", "z"][axis_index(a) as usize],
        ),
        Model::ShaftHalf(d) => find("create:part_shaft_half", "facing", dir_name(d)),
        Model::CrankHandle(d) => find("create:part_hand_crank_handle", "facing", dir_name(d)),
        Model::WaterWheel(d) => find("create:part_water_wheel", "facing", dir_name(d)),
        Model::MillstoneCog => find("create:part_millstone_cog", "facing", "up"),
        Model::PressHead(d) => find("create:part_press_head", "facing", dir_name(d)),
        Model::Belt(..) | Model::BeltPulley(_) => None,
        Model::BearingTop(d) => find("create:part_bearing_top_wooden", "facing", dir_name(d)),
        Model::BearingTopMetal(d) => find("create:part_bearing_top", "facing", dir_name(d)),
        Model::FanPropeller(d) => find("create:part_fan_propeller", "facing", dir_name(d)),
        Model::ShaftlessCog => find("create:part_cogwheel_shaftless", "facing", "up"),
        Model::PumpCog(d) => find("create:part_pump_cog", "facing", dir_name(d)),
        Model::DrillHead(d) => find("create:part_drill_head", "facing", dir_name(d)),
        Model::MixerPole | Model::MixerHead => None,
        Model::GaugeHead(d, create_engine::gauge::Kind::Speed) => {
            find("create:part_gauge_head_speed", "facing", dir_name(d))
        }
        Model::GaugeHead(d, create_engine::gauge::Kind::Stress) => {
            find("create:part_gauge_head_stress", "facing", dir_name(d))
        }
        Model::GaugeDial(d) => find("create:part_gauge_dial", "facing", dir_name(d)),
    }
}

/// The part model a draw needs: the block state's own or a partial's, posed for belts.
pub(super) fn part_model(
    game: &mut Game<'_>,
    registry: &BlockRegistry,
    own: BlockStateId,
    model: Model,
) -> Option<u32> {
    use create_engine::belt;
    let partial = |name: &str| {
        let default = registry.default_state(name)?;
        let mut s = registry.state(default)?.clone();
        s.properties.insert("facing".to_owned(), "up".to_owned());
        registry.id(&s)
    };
    match model {
        Model::Belt(seg, bottom) => {
            let state = partial(belt::partial(&seg, bottom)?)?;
            game.native_part_model_posed(state, belt::segment_pose(&seg))
        }
        Model::BeltPulley(seg) => {
            let state = partial("create:part_belt_pulley")?;
            game.native_part_model_posed(state, belt::pulley_pose(&seg))
        }
        // The pole and head at rest: `getRenderedHeadOffset` is 7/16 when not running.
        Model::MixerPole | Model::MixerHead => {
            let name = if model == Model::MixerPole {
                "create:part_mixer_pole"
            } else {
                "create:part_mixer_head"
            };
            let state = partial(name)?;
            let down = glam::Mat4::from_translation(glam::Vec3::new(0.0, -7.0 / 16.0, 0.0));
            game.native_part_model_posed(state, down)
        }
        _ => model_state(registry, own, model).and_then(|s| game.native_part_model(s)),
    }
}

/// Shows the parts of the block at `pos` at its current speeds; false when the block there is
/// not (yet) one that draws parts.
pub(super) fn show(game: &mut Game<'_>, c: &mut ClientParts, pos: [i32; 3]) -> bool {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some((block, info)) = c.table.get(state.0) else {
        return false;
    };
    let speeds = c.speeds.get(&pos).copied().unwrap_or_default();
    let draws = client::draws(block, info, pos, speeds);
    if draws.is_empty() {
        return false;
    }
    let registry = game.pack.assets.registry;
    let mut used = 0u8;
    for d in &draws {
        let handle = match c.models.get(&(state.0, d.model)) {
            Some(&h) => Some(h),
            None => part_model(game, registry, state, d.model).inspect(|&h| {
                c.models.insert((state.0, d.model), h);
            }),
        };
        if let Some(h) = handle {
            game.native_part_set(
                client::key(pos, d.slot),
                h,
                pos,
                axis_index(d.axis),
                d.rpm,
                d.phase,
            );
            used |= 1 << d.slot;
        }
    }
    for slot in 0..client::SLOTS {
        if used & (1 << slot) == 0 {
            game.native_part_clear(client::key(pos, slot));
        }
    }
    // The dials were drawn at rest: put the needle back where it is.
    if create_engine::world::gauge_kind(block).is_some()
        && let Some(n) = GAUGES.with_borrow(|g| g.needles.get(&pos).copied())
    {
        let still = create_engine::gauge::Needle { prev: n.state, ..n };
        draw_gauge_dials(game, c, pos, &still);
    }
    true
}

pub(super) fn hide(game: &mut Game<'_>, pos: [i32; 3]) {
    for slot in 0..client::SLOTS {
        game.native_part_clear(client::key(pos, slot));
    }
}

pub(super) fn client_ensure(game: &Game<'_>) -> bool {
    CLIENT.with_borrow_mut(|c| {
        if c.is_some() {
            return true;
        }
        let registry = game.pack.assets.registry;
        let names = (0..registry.len()).filter_map(|i| {
            registry
                .state(BlockStateId(i as u32))
                .map(|s| s.name.as_str())
        });
        let Some((first, len)) = states::find_range(names) else {
            return false;
        };
        // A LAN guest or a server's player has no server thread here to set it.
        states::set_range(first, len);
        let table = StateTable::new(
            first,
            (first..first + len).filter_map(|i| {
                let s = registry.state(BlockStateId(i))?;
                Some((
                    s.name.as_str(),
                    s.properties.iter().map(|(k, v)| (k.as_str(), v.as_str())),
                ))
            }),
        );
        *c = Some(ClientParts {
            table,
            speeds: HashMap::new(),
            models: HashMap::new(),
            pending: Vec::new(),
            synced: false,
        });
        true
    })
}

/// The server's speeds, on the client.
pub(super) fn net_from_server(game: &mut Game<'_>, channel: &str, data: &[u8]) {
    if channel == create_engine::depot::CHANNEL {
        depots_from_server(game, data);
        return;
    }
    if channel == create_engine::link::CHANNEL {
        links_from_server(game, data);
        return;
    }
    if channel == create_engine::track_curves::CHANNEL {
        curves_from_server(game, data);
        return;
    }
    if channel == VALUE {
        values_from_server(data);
        return;
    }
    if channel == create_engine::deployer::CHANNEL {
        deployers_from_server(game, data);
        return;
    }
    if channel == create_engine::press::CHANNEL {
        presses_from_server(game, data);
        return;
    }
    if channel == create_engine::gauge::CHANNEL {
        gauges_from_server(data);
        return;
    }
    if channel == create_engine::belt::CHANNEL {
        belts_from_server(game, data);
        return;
    }
    if channel == create_engine::tank::CHANNEL {
        tanks_from_server(game, data);
        return;
    }
    if channel == TRAIN_CHANNEL {
        trains_from_server(game, data);
        return;
    }
    if channel == PISTON_CHANNEL {
        pistons_from_server(game, data);
        return;
    }
    if channel == CONTRAPTION_CHANNEL {
        contraptions_from_server(game, data);
        return;
    }
    if channel != client::CHANNEL || !client_ensure(game) {
        return;
    }
    let Some(mut c) = CLIENT.with_borrow_mut(Option::take) else {
        return;
    };
    for u in client::decode(data) {
        match u {
            Update::Set { pos, sub, speed } => {
                c.speeds.entry(pos).or_default()[usize::from(sub & 1)] = speed;
                if !show(game, &mut c, pos) {
                    c.pending.push((pos, 0));
                }
                turn_contraption(game, pos, speed);
            }
            Update::Clear { pos } => {
                c.speeds.remove(&pos);
                hide(game, pos);
                GAUGES.with_borrow_mut(|g| g.forget(pos));
            }
        }
    }
    CLIENT.with_borrow_mut(|slot| *slot = Some(c));
}

/// Each tick of the window's game: ask the server for every part once in a world, try parts
/// whose block hadn't arrived again, and forget everything once out of a world.
pub(super) fn client_tick(game: &mut Game<'_>) {
    if !game.native_window() {
        return;
    }
    if !game.native_playing() {
        CLIENT.with_borrow_mut(|c| *c = None);
        DEPOTS.with_borrow_mut(|d| *d = ClientDepots::default());
        PRESSES.with_borrow_mut(|c| *c = ClientPresses::default());
        BELTS.with_borrow_mut(|b| *b = ClientBelts::default());
        GAUGES.with_borrow_mut(|g| *g = ClientGauges::default());
        return;
    }
    tick_client_depots(game);
    tick_client_presses(game);
    tick_client_belts(game);
    if !client_ensure(game) {
        return;
    }
    tick_value_board(game);
    let Some(mut c) = CLIENT.with_borrow_mut(Option::take) else {
        return;
    };
    if !c.synced
        && game
            .native_net_send_to_server(client::CHANNEL, b"sync")
            .is_ok()
    {
        c.synced = true;
    }
    if !c.pending.is_empty() {
        let pending = std::mem::take(&mut c.pending);
        for (pos, tries) in pending {
            if c.speeds.contains_key(&pos) && !show(game, &mut c, pos) && tries < 100 {
                c.pending.push((pos, tries + 1));
            }
        }
    }
    tick_client_gauges(game, &mut c);
    CLIENT.with_borrow_mut(|slot| *slot = Some(c));
}

/// Create's recipes of the types its machines run, read once per world (and again on
/// `/reload`, when the game's table changes): typed and indexed by input item.
pub(super) struct Recipes {
    /// Where the game's list of milling recipes was, and its length, when these were read:
    /// `/reload` builds a new table.
    pub(super) table: (usize, usize),
    pub(super) milling: create_engine::processing::Book,
    pub(super) pressing: create_engine::processing::Book,
    pub(super) crushing: create_engine::processing::Book,
    /// Create's cutting recipes (the saw).
    pub(super) cutting: create_engine::processing::Book,
    /// Create's mixing recipes made of items only (fluids are gap 5).
    pub(super) mixing: Vec<create_engine::basin::Recipe>,
    /// Create's compacting recipes (a press over a basin), items only.
    pub(super) compacting: Vec<create_engine::basin::Recipe>,
    /// The fan's: Create's washing and haunting, vanilla's cooking.
    pub(super) splashing: create_engine::processing::Book,
    pub(super) haunting: create_engine::processing::Book,
    pub(super) smoking: create_engine::processing::Book,
    pub(super) smelting: create_engine::processing::Book,
    pub(super) blasting: create_engine::processing::Book,
    /// The deployer's: `create:deploying`, then `create:item_application` (automatable ones).
    pub(super) deploying: Vec<create_engine::deployer::Recipe>,
    /// The item drain's and the spout's: `create:emptying`, `create:filling` with the fluids
    /// the machines carry.
    pub(super) emptying: Vec<(String, create_engine::drain::Emptying)>,
    pub(super) filling: Vec<create_engine::spout::Filling>,
}

impl Recipes {
    pub(super) fn fan(&self) -> create_engine::fan::Books<'_> {
        create_engine::fan::Books {
            splashing: &self.splashing,
            haunting: &self.haunting,
            smoking: &self.smoking,
            smelting: &self.smelting,
            blasting: &self.blasting,
        }
    }
}

/// Reads Create's recipe books the first time a machine needs them in a world, and again after
/// `/reload`. False when there are none (Create's data isn't installed).
pub(super) fn ensure_recipes(game: &Game<'_>) -> bool {
    let list = game.native_recipes_of_type("create:milling");
    let table = (list.as_ptr() as usize, list.len());
    if RECIPES.with_borrow(|r| r.as_ref().is_some_and(|r| r.table == table)) {
        return true;
    }
    let milling = read_book(game, "create:milling");
    let pressing = read_book(game, "create:pressing");
    if milling.is_empty() && pressing.is_empty() {
        return false;
    }
    RECIPES.with_borrow_mut(|r| {
        *r = Some(Recipes {
            table,
            milling,
            pressing,
            crushing: read_book(game, "create:crushing"),
            mixing: read_basin(game, "create:mixing"),
            cutting: read_book(game, "create:cutting"),
            compacting: read_basin(game, "create:compacting"),
            splashing: read_book(game, "create:splashing"),
            haunting: read_book(game, "create:haunting"),
            smoking: read_cooking(game, "minecraft:smoking"),
            smelting: read_cooking(game, "minecraft:smelting"),
            blasting: read_cooking(game, "minecraft:blasting"),
            deploying: ["create:deploying", "create:item_application"]
                .into_iter()
                .flat_map(|k| read_application(game, k))
                .collect(),
            emptying: read_emptying(game),
            filling: read_filling(game),
        })
    });
    true
}

thread_local! {
    pub(super) static RECIPES: RefCell<Option<Recipes>> = const { RefCell::new(None) };
}

/// A vanilla cooking recipe type (`minecraft:smelting`) as a book: its one ingredient, its one
/// result. These are the game's own table (vanilla's, the content mods' and the world's packs'),
/// in file order; `native_recipes_of_type` serves only mods' own types.
pub(super) fn read_cooking(game: &Game<'_>, kind: &str) -> create_engine::processing::Book {
    use create_engine::processing::{Book, Output, Recipe};
    let recipes = game
        .recipes()
        .cooking
        .iter()
        .filter(|c| c.kind == kind)
        .map(|c| Recipe {
            id: c.id.clone(),
            ingredients: vec![c.ingredient.1.iter().map(|s| s.to_string()).collect()],
            results: vec![Output {
                item: c.result.item.to_string(),
                count: c.result.count.min(255) as u8,
                chance: 1.0,
            }],
            time: 0,
        })
        .collect();
    Book::new(recipes)
}

/// A basin recipe type: item ingredients and results only (a fluid ingredient or result drops the
/// recipe until fluids are ported), with its `heat_requirement`.
/// A recipe's fluid (`FluidIngredient` or a result): its fluid by id or tag, and amount.
pub(super) fn recipe_fluid(json: &serde_json::Value) -> Option<(create_engine::tank::Fluid, u32)> {
    let amount = u32::try_from(json["amount"].as_u64()?).ok()?;
    let id = json["fluid"]
        .as_str()
        .or_else(|| json["tag"].as_str())
        .or_else(|| json["id"].as_str())?;
    Some((create_engine::tank::Fluid::of_id(id)?, amount))
}

/// `create:emptying`: each item its fluid and what is left of it.
pub(super) fn read_emptying(game: &Game<'_>) -> Vec<(String, create_engine::drain::Emptying)> {
    let mut out = Vec::new();
    for r in game.native_recipes_of_type("create:emptying") {
        let json = &*r.json;
        let Some(items) = json["ingredients"]
            .get(0)
            .and_then(|i| game.native_recipe_ingredient(i))
        else {
            continue;
        };
        let results = json["results"].as_array().cloned().unwrap_or_default();
        let Some((fluid, amount)) = results.iter().find_map(recipe_fluid) else {
            continue;
        };
        let leftover = results
            .iter()
            .find(|o| o.get("amount").is_none())
            .and_then(|o| game.native_recipe_result(o))
            .map(|s| s.item.to_string());
        for item in items.1.iter() {
            out.push((
                item.to_string(),
                create_engine::drain::Emptying {
                    fluid,
                    amount,
                    leftover: leftover.clone(),
                },
            ));
        }
    }
    out
}

/// `create:filling`: an item and a fluid, what it becomes.
pub(super) fn read_filling(game: &Game<'_>) -> Vec<create_engine::spout::Filling> {
    let mut out = Vec::new();
    for r in game.native_recipes_of_type("create:filling") {
        let json = &*r.json;
        let Some(items) = json["ingredients"]
            .get(0)
            .and_then(|i| game.native_recipe_ingredient(i))
        else {
            continue;
        };
        let Some((fluid, amount)) = json["ingredients"].get(1).and_then(recipe_fluid) else {
            continue;
        };
        let Some(result) = json["results"]
            .get(0)
            .and_then(|o| game.native_recipe_result(o))
        else {
            continue;
        };
        for item in items.1.iter() {
            out.push(create_engine::spout::Filling {
                item: item.to_string(),
                fluid,
                amount,
                result: result.item.to_string(),
            });
        }
    }
    out
}

/// `ItemApplicationRecipe`s: the item worked on, the one held, the results; `_manual_only`
/// ones left out (`CAN_BE_AUTOMATED`).
pub(super) fn read_application(
    game: &Game<'_>,
    kind: &str,
) -> Vec<create_engine::deployer::Recipe> {
    use create_engine::processing::Output;
    game.native_recipes_of_type(kind)
        .iter()
        .filter(|r| !r.id.ends_with("_manual_only"))
        .filter_map(|r| {
            let json = &*r.json;
            let mut ingredients = json["ingredients"].as_array()?.iter().map(|i| {
                game.native_recipe_ingredient(i)
                    .map(|ing| ing.1.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            });
            let target = ingredients.next()??;
            let tool = ingredients.next()??;
            let results = json["results"]
                .as_array()?
                .iter()
                .map(|o| {
                    let stack = game.native_recipe_result(o)?;
                    Some(Output {
                        item: stack.item.to_string(),
                        count: stack.count.min(255) as u8,
                        chance: o["chance"].as_f64().unwrap_or(1.0) as f32,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(create_engine::deployer::Recipe {
                id: r.id.clone(),
                target,
                tool,
                results,
                keep_held: json["keep_held_item"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

pub(super) fn read_basin(game: &Game<'_>, kind: &str) -> Vec<create_engine::basin::Recipe> {
    use create_engine::basin::{HeatReq, Recipe};
    use create_engine::processing::Output;
    game.native_recipes_of_type(kind)
        .iter()
        .filter_map(|r| {
            let json = &*r.json;
            let mut ingredients = Vec::new();
            let mut fluids = Vec::new();
            for i in json["ingredients"].as_array()? {
                if i.get("amount").is_some() {
                    fluids.push(recipe_fluid(i)?);
                } else {
                    let ing = game.native_recipe_ingredient(i)?;
                    ingredients.push(ing.1.iter().map(|s| s.to_string()).collect::<Vec<_>>());
                }
            }
            let mut results = Vec::new();
            let mut fluid_results = Vec::new();
            for o in json["results"].as_array()? {
                if o.get("amount").is_some() {
                    fluid_results.push(recipe_fluid(o)?);
                    continue;
                }
                let stack = game.native_recipe_result(o)?;
                results.push(Output {
                    item: stack.item.to_string(),
                    count: stack.count.min(255) as u8,
                    chance: o["chance"].as_f64().unwrap_or(1.0) as f32,
                });
            }
            Some(Recipe {
                id: r.id.clone(),
                ingredients,
                results,
                time: json["processing_time"].as_i64().unwrap_or(0) as i32,
                heat: HeatReq::read(json["heat_requirement"].as_str()),
                fluids,
                fluid_results,
            })
        })
        .collect()
}

pub(super) fn read_book(game: &Game<'_>, kind: &str) -> create_engine::processing::Book {
    use create_engine::processing::{Book, Output, Recipe};
    let recipes = game
        .native_recipes_of_type(kind)
        .iter()
        .filter_map(|r| {
            let json = &*r.json;
            let ingredients = json["ingredients"]
                .as_array()?
                .iter()
                .map(|i| {
                    game.native_recipe_ingredient(i)
                        .map(|ing| ing.1.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                })
                .collect::<Option<Vec<_>>>()?;
            let results = json["results"]
                .as_array()?
                .iter()
                .filter_map(|o| {
                    let stack = game.native_recipe_result(o)?;
                    Some(Output {
                        item: stack.item.to_string(),
                        count: stack.count.min(255) as u8,
                        chance: o["chance"].as_f64().unwrap_or(1.0) as f32,
                    })
                })
                .collect();
            Some(Recipe {
                id: r.id.clone(),
                ingredients,
                results,
                time: json["processing_time"].as_i64().unwrap_or(0) as i32,
            })
        })
        .collect();
    Book::new(recipes)
}

/// `MillstoneBlock`'s block entity: its id, saved like any block entity with its slots.
pub(super) fn block_entity(
    block: &str,
    entity: &mut Option<(String, horizon_world::nbt::Compound)>,
) {
    if entity.is_none()
        && matches!(
            block,
            "create:millstone"
                | "create:depot"
                | "create:basin"
                | "create:chute"
                | "create:smart_chute"
                | "create:item_vault"
                | "create:mechanical_crafter"
        )
    {
        *entity = Some((block.into(), horizon_world::nbt::Compound::new()));
    }
}

/// The millstone's slots (`MillstoneInventoryHandler`): hoppers put into the input (0) and take
/// from the nine outputs, through any face; no menu.
pub(super) fn block_entity_container(
    block: &str,
    container: &mut Option<horizon_client::containers::ModContainer>,
) {
    if container.is_some() {
        return;
    }
    if block == "create:depot" {
        // DepotItemHandler: in only to the held slot while the depot holds nothing; out of
        // every slot; through any face.
        let all: Vec<usize> = (0..create_engine::depot::SLOTS).collect();
        *container = Some(horizon_client::containers::ModContainer {
            slots: create_engine::depot::SLOTS,
            faces: std::array::from_fn(|_| all.clone()),
            inputs: vec![0],
            outputs: all,
            menu: None,
            only_when_empty: true,
        });
        return;
    }
    if block == "create:mechanical_crafter" {
        // MechanicalCrafterBlockEntity.Inventory: one item, put in while it is empty.
        *container = Some(horizon_client::containers::ModContainer {
            slots: 1,
            faces: std::array::from_fn(|_| vec![0]),
            inputs: vec![0],
            outputs: vec![],
            menu: None,
            only_when_empty: true,
        });
        return;
    }
    if block == "create:item_vault" {
        // ItemVaultBlockEntity's ItemStackHandler (`vaultCapacity` stacks): in and out through
        // any face. Hoppers and comparators see this block's own slots; Create's funnels and
        // chutes the whole vault's (`insert_vault`, `extract_vault`).
        let all: Vec<usize> = (0..create_engine::vault::SLOTS).collect();
        *container = Some(horizon_client::containers::ModContainer {
            slots: create_engine::vault::SLOTS,
            faces: std::array::from_fn(|_| all.clone()),
            inputs: all.clone(),
            outputs: all,
            menu: None,
            only_when_empty: false,
        });
        return;
    }
    if block == "create:smart_chute" {
        // ChuteItemHandler of a smart chute: hoppers below take its stack; nothing is put in
        // from outside, as a hopper here could not ask its filter (`canAcceptItem`): it takes
        // from the hopper above itself (`handleInputFromAbove`).
        *container = Some(horizon_client::containers::ModContainer {
            slots: 1,
            faces: std::array::from_fn(|_| vec![0]),
            inputs: vec![],
            outputs: vec![0],
            menu: None,
            only_when_empty: true,
        });
        return;
    }
    if block == "create:chute" {
        // ChuteItemHandler: one stack, taken only while the chute holds nothing.
        *container = Some(horizon_client::containers::ModContainer {
            slots: 1,
            faces: std::array::from_fn(|_| vec![0]),
            inputs: vec![0],
            outputs: vec![0],
            menu: None,
            only_when_empty: true,
        });
        return;
    }
    if block == "create:basin" {
        // The basin's inventories: hoppers put into the nine inputs and take from the nine
        // outputs.
        let all: Vec<usize> = (0..create_engine::basin::SLOTS).collect();
        *container = Some(horizon_client::containers::ModContainer {
            slots: create_engine::basin::SLOTS,
            faces: std::array::from_fn(|_| all.clone()),
            inputs: create_engine::basin::INPUTS.collect(),
            outputs: create_engine::basin::OUTPUTS.collect(),
            menu: None,
            only_when_empty: false,
        });
        return;
    }
    if block != "create:millstone" {
        return;
    }
    let all: Vec<usize> = (0..10).collect();
    *container = Some(horizon_client::containers::ModContainer {
        slots: 10,
        faces: std::array::from_fn(|_| all.clone()),
        inputs: vec![0],
        outputs: (1..10).collect(),
        menu: None,
        only_when_empty: false,
    });
}

/// `WaterWheelBlockEntity.getFlowVectorAtPosition`: the fluid's flow, or a bubble column's
/// push (up, or down when it drags down).
pub(super) fn flow_at(game: &Game<'_>, dimension: &str, at: [i32; 3]) -> [f64; 3] {
    let state = game.world.block(at[0], at[1], at[2]);
    if let Some(s) = game.pack.assets.registry.state(state)
        && s.name == "minecraft:bubble_column"
    {
        let down = s.properties.get("drag").is_some_and(|v| v == "true");
        return [0.0, if down { -1.0 } else { 1.0 }, 0.0];
    }
    game.native_fluid_flow(dimension, at).unwrap_or_default()
}

/// `/create_debug speed <pos>`: the kinetic speed the server solved at a block, for tests and
/// for players who want the number (operators only).
pub(super) fn register_commands(commands: &mut Vec<super::native_server::NativeCommand>) {
    commands.push(super::native_server::NativeCommand::op(
        "create_debug speed <pos:block_pos>",
        2,
    ));
    // A train's destination and a station's assembly, for tests: Create gives them with a
    // schedule and the station's screen.
    commands.push(super::native_server::NativeCommand::op(
        "create_debug train <pos:block_pos>",
        2,
    ));
    commands.push(super::native_server::NativeCommand::op(
        "create_debug assemble <pos:block_pos>",
        2,
    ));
    // A schedule (the schedule item), a station's name (its screen) and a curve between two
    // tracks (the track item's two clicks), for tests.
    commands.push(super::native_server::NativeCommand::op(
        "create_debug schedule <pos:block_pos> <entries:text>",
        2,
    ));
    commands.push(super::native_server::NativeCommand::op(
        "create_debug station_name <pos:block_pos> <name:text>",
        2,
    ));
    commands.push(super::native_server::NativeCommand::op(
        "create_debug curve <from:block_pos> <to:block_pos>",
        2,
    ));
}

pub(super) fn perform_command(game: &mut Game<'_>, command: &str, ran: &mut bool) {
    let train = command.strip_prefix("create_debug train ");
    let assemble = command.strip_prefix("create_debug assemble ");
    let schedule = command.strip_prefix("create_debug schedule ");
    let name = command.strip_prefix("create_debug station_name ");
    let curve = command.strip_prefix("create_debug curve ");
    let Some(rest) = command
        .strip_prefix("create_debug speed ")
        .or(train)
        .or(assemble)
        .or(schedule)
        .or(name)
        .or(curve)
    else {
        return;
    };
    *ran = true;
    let player = game.native_acting_player();
    let dim = player
        .as_ref()
        .and_then(|p| dimension_index(&p.dimension))
        .unwrap_or(0);
    // Block coordinates; `~` is the player's own, as vanilla's block positions take it.
    let here = player.as_ref().map(|p| p.position);
    let parts: Vec<i32> = rest
        .split_whitespace()
        .enumerate()
        .filter_map(|(n, t)| match t.strip_prefix('~') {
            Some(off) => Some(here?.get(n % 3)?.floor() as i32 + off.parse::<i32>().unwrap_or(0)),
            None => t.parse().ok(),
        })
        .collect();
    if schedule.is_some() || name.is_some() {
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        let text = tokens.get(3..).map(|t| t.join(" ")).unwrap_or_default();
        let line = match (parts.get(0..3), schedule.is_some()) {
            (Some(&[x, y, z]), true) => schedule_train(dim, [x, y, z], &text),
            (Some(&[x, y, z]), false) => name_station(dim, [x, y, z], &text),
            (_, true) => "usage: create_debug schedule <x> <y> <z> <station[@seconds]>...".into(),
            (_, false) => "usage: create_debug station_name <x> <y> <z> <name>".into(),
        };
        game.native_feedback(horizon_client::text::Text::literal(line));
        return;
    }
    if curve.is_some() {
        let line = match parts.as_slice() {
            [a, b, c, x, y, z] => debug_curve(game, dim, [*a, *b, *c], [*x, *y, *z]),
            _ => "usage: create_debug curve <x> <y> <z> <x> <y> <z>".into(),
        };
        game.native_feedback(horizon_client::text::Text::literal(line));
        return;
    }
    if assemble.is_some() {
        if let [x, y, z] = parts.as_slice() {
            use_station(game, dim, [*x, *y, *z]);
        }
        return;
    }
    if train.is_some() {
        let line = match parts.as_slice() {
            [x, y, z] => send_train(game, dim, [*x, *y, *z]),
            _ => "usage: create_debug train <x> <y> <z>".into(),
        };
        game.native_feedback(horizon_client::text::Text::literal(line));
        return;
    }
    let line = match parts.as_slice() {
        [x, y, z] => match with_world(|w| w.speed(dim, [*x, *y, *z])).flatten() {
            Some(speed) => format!("create speed at {x} {y} {z}: {speed} rpm"),
            None => format!("create speed at {x} {y} {z}: not kinetic"),
        },
        _ => "usage: create_debug speed <x> <y> <z>".into(),
    };
    game.native_feedback(horizon_client::text::Text::literal(line));
}

/// A hopper put into or took from a block's slots: a depot's held stack arrives from above
/// (`new TransportedItemStack(stack)`, a quarter-less slide from the edge of nothing) or left.
pub(super) fn contents_changed(game: &mut Game<'_>, pos: [i32; 3]) {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    if states::index(state).is_none() {
        return;
    }
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    // Chutes asleep under this container (or under its vault) look into it again.
    with_world(|w| w.container_changed(dim, pos));
    // An item put into a crafter: its chain may start (`insertItem` -> `checkCompletedRecipe`).
    if with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
        == Some(create_engine::Block::MechanicalCrafter)
    {
        with_world(|w| w.dims[dim].crafter_checks.push(pos));
        return;
    }
    // A chute was given a stack (a hopper above): it falls from the top.
    if matches!(
        with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten(),
        Some(create_engine::Block::Chute | create_engine::Block::SmartChute)
    ) {
        with_world(|w| {
            w.dims[dim].chutes.entry(pos).or_insert(1.0);
        });
        return;
    }
    // A basin's contents changed: the mixer over it looks again.
    if with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
        == Some(create_engine::Block::Basin)
    {
        with_world(|w| w.dims[dim].mixer_checks.push([pos[0], pos[1] + 2, pos[2]]));
        return;
    }
    // Outer: the world is open; inner: a depot is there (its held stack's motion, if any).
    let Some(had) = with_world(|w| w.dims[dim].depots.get(&pos).copied()).flatten() else {
        return;
    };
    let held = game
        .world
        .block_entities
        .get(&(pos[0], pos[1], pos[2]))
        .and_then(|b| match b {
            horizon_client::containers::BlockEntity::Mod(m) => m.items.first().cloned().flatten(),
            _ => None,
        });
    let now = match (had, held) {
        (_, None) => None,
        (Some(h), Some(_)) => Some(h),
        (None, Some(s)) => {
            let up = upright(game, &s.item);
            Some(create_engine::depot::Held {
                position: 0.0,
                ..create_engine::depot::Held::new(random_angle(game, pos), up)
            })
        }
    };
    with_world(|w| w.depot_put(dim, pos, now));
}

/// The depots a client draws: their contents and the held stack's motion, which it slides
/// itself, tick by tick, as the server does.
#[derive(Default)]
pub(super) struct ClientDepots {
    pub(super) shown: HashMap<[i32; 3], create_engine::depot::Contents>,
    pub(super) sliding: Vec<[i32; 3]>,
}

thread_local! {
    pub(super) static DEPOTS: RefCell<ClientDepots> = RefCell::new(ClientDepots::default());
}

/// `getStateForPlacement` of Create's directional blocks: a block placed from an item turns to
/// the facing Create gives it (from the shafts around it, the player's view and sneaking).
pub(super) fn place_block(game: &mut Game<'_>, pos: [i32; 3], state: BlockStateId) {
    if states::index(state.0).is_none() {
        return;
    }
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    let clicked_face = CLICKED
        .get()
        .filter(|c| c.0 == game.world.game_time)
        .and_then(|c| Dir::from_offset(c.1))
        .unwrap_or(Dir::Up);
    let block = with_world(|w| w.table.get(state.0).map(|(b, _)| b)).flatten();
    if block == Some(create_engine::Block::SteamEngine) {
        place_engine(game, pos, state);
        return;
    }
    if block == Some(create_engine::Block::ItemVault) {
        // ItemVaultBlock.getStateForPlacement: the axis of the vault it was placed against
        // unless sneaking, else the player's horizontal facing's.
        let o = clicked_face.offset();
        let on = game
            .world
            .block(pos[0] - o[0], pos[1] - o[1], pos[2] - o[2])
            .0;
        let preferred = with_world(|w| {
            w.table
                .get(on)
                .filter(|(b, _)| *b == create_engine::Block::ItemVault)
                .and_then(|(_, i)| i.axis)
        })
        .flatten();
        let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
        let axis = match preferred {
            Some(a) if !sneaking => a,
            _ => create_engine::place::horizontal_direction(game.yaw).axis(),
        };
        let now = with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.axis)).flatten();
        if now != Some(axis) {
            game.native_set_block_property(pos, "axis", if axis == Axis::X { "x" } else { "z" });
        }
        return;
    }
    if block == Some(create_engine::Block::FluidPipe) {
        // FluidPipeBlock.getStateForPlacement: straight toward the player's view when nothing is
        // around.
        let looking = create_engine::place::ordered_by_nearest(game.yaw, game.pitch)[0];
        let dimension = game.dim.id().to_owned();
        connect_pipe(game, &dimension, pos, Some(looking));
        return;
    }
    if let Some((now, along_now)) = with_world(|w| {
        let (b, info) = w.table.get(state.0)?;
        create_engine::world::gauge_kind(b).map(|_| (info.facing, info.along_first))
    })
    .flatten()
    {
        // GaugeBlock.getStateForPlacement: the block it was placed on (behind the clicked face)
        // carries its shaft on through it.
        let o = clicked_face.offset();
        let on = [pos[0] - o[0], pos[1] - o[1], pos[2] - o[2]];
        let look = create_engine::place::ordered_by_nearest(game.yaw, game.pitch)[0];
        let horizontal = create_engine::place::horizontal_direction(game.yaw);
        let Some((facing, along)) = with_world(|w| {
            create_engine::gauge::placement(
                clicked_face,
                look,
                horizontal,
                w.has_shaft_towards(dim, on, clicked_face),
                |side| {
                    let o = side.offset();
                    let at = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
                    w.has_shaft_towards(dim, at, side.opposite())
                },
            )
        }) else {
            return;
        };
        if now != Some(facing) {
            game.native_set_block_property(pos, "facing", dir_name(facing));
        }
        if along_now != along {
            let value = if along { "true" } else { "false" };
            game.native_set_block_property(pos, "axis_along_first", value);
        }
        return;
    }
    let ctx = create_engine::place::Context {
        yaw: game.yaw,
        pitch: game.pitch,
        sneaking: game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft),
        clicked_face,
    };
    let facing = with_world(|w| {
        let (block, info) = w.table.get(state.0)?;
        let now = info.facing;
        let to = create_engine::place::facing(block, ctx, |side| {
            let o = side.offset();
            let at = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
            w.has_shaft_towards(dim, at, side.opposite())
        })?;
        (now != Some(to)).then_some(to)
    })
    .flatten();
    if let Some(f) = facing {
        game.native_set_block_property(pos, "facing", dir_name(f));
    }
    // Create's axis blocks: chain drives, the speed controller, the sequenced gearshift.
    let axis = with_world(|w| {
        let (block, info) = w.table.get(state.0)?;
        let above = w.large_cog_axis(dim, [pos[0], pos[1] + 1, pos[2]]);
        let to = create_engine::place::axis(
            block,
            ctx,
            |side| {
                let o = side.offset();
                let at = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
                w.has_shaft_towards(dim, at, side.opposite())
            },
            above,
        )?;
        (info.axis != Some(to.0) || info.vertical != to.1).then_some((block, to))
    })
    .flatten();
    if let Some((block, (a, vertical))) = axis {
        let name = match a {
            Axis::X => "x",
            Axis::Y => "y",
            Axis::Z => "z",
        };
        game.native_set_block_property(pos, "axis", name);
        if block == create_engine::Block::SequencedGearshift {
            game.native_set_block_property(
                pos,
                "vertical",
                if vertical { "true" } else { "false" },
            );
        }
    }
}

/// A block state of `name` with `props` set (others at their defaults).
pub(super) fn state_with(
    registry: &BlockRegistry,
    name: &str,
    props: &[(&str, &str)],
) -> Option<BlockStateId> {
    let mut s = registry.state(registry.default_state(name)?)?.clone();
    for (k, v) in props {
        s.properties.insert((*k).to_owned(), (*v).to_owned());
    }
    registry.id(&s)
}

/// Presses a client draws mid-cycle: their cycle, run on from the server's last word, the head
/// waiting at its lowest point until the server says it pressed (as Create's client does).
#[derive(Default)]
pub(super) struct ClientPresses {
    pub(super) running: HashMap<[i32; 3], (create_engine::press::Press, bool)>,
}

thread_local! {
    pub(super) static PRESSES: RefCell<ClientPresses> = RefCell::new(ClientPresses::default());
}

/// A belt's surroundings on the server: what lies past its end, and the press over a segment.
pub(super) struct BeltSide<'a, 'g> {
    pub(super) game: &'a mut Game<'g>,
    pub(super) dim: usize,
    pub(super) controller: [i32; 3],
    pub(super) segment: create_engine::belt::Segment,
    pub(super) length: i32,
    pub(super) rpm: f32,
    pub(super) positive: bool,
}

impl BeltSide<'_, '_> {
    pub(super) fn past_end(&self) -> [i32; 3] {
        create_engine::belt::position_for_offset(
            self.controller,
            &self.segment,
            self.length,
            if self.positive { self.length } else { -1 },
        )
    }

    pub(super) fn movement(&self) -> create_engine::kinetics::Dir {
        create_engine::belt::movement_facing(self.rpm, self.segment.facing)
    }

    /// `DirectBeltInputBehaviour.handleInsertion` at `next`, the item moving toward `side`: a
    /// depot or a belt there. What is left of it, None when nothing there takes items.
    pub(super) fn insert_at(
        &mut self,
        next: [i32; 3],
        side: create_engine::kinetics::Dir,
        item: &create_engine::belt::Item,
    ) -> Option<u8> {
        insert_into(self.game, self.dim, next, side, item)
    }
}

impl create_engine::belt::Surroundings for BeltSide<'_, '_> {
    fn ending(&mut self) -> create_engine::belt::Ending {
        use create_engine::belt::Ending;
        let next = self.past_end();
        let has = |w: &mut CreateWorld| {
            let d = &w.dims[self.dim];
            d.depots.contains_key(&next) || d.belt_of.contains_key(&next)
        };
        if with_world(has) == Some(true) {
            return Ending::Insert;
        }
        // BlockHelper.hasBlockSolidSide toward the belt.
        let state = self.game.world.block(next[0], next[1], next[2]);
        let face = self.movement().opposite() as u8;
        if self.game.res.facts().get(state).sturdy_full & (1 << face) != 0 {
            return Ending::Blocked;
        }
        Ending::Eject
    }

    fn insert(&mut self, item: &create_engine::belt::Item) -> Option<u8> {
        let (next, side) = (self.past_end(), self.movement());
        self.insert_at(next, side, item)
    }

    fn tunnel(&mut self, segment: i32, item: &mut create_engine::belt::Item) -> bool {
        use create_engine::belt::Slope;
        if self.segment.slope != Slope::Horizontal {
            return false;
        }
        let at = create_engine::belt::position_for_offset(
            self.controller,
            &self.segment,
            self.length,
            segment,
        );
        let t = [at[0], at[1] + 1, at[2]];
        let Some((axis, shape)) = tunnel_state(self.game.world.block(t[0], t[1], t[2]).0) else {
            return false;
        };
        let movement = self.movement();
        let dim = self.dim;
        // A brass tunnel that distributes (`hasDistributionBehaviour`: an open side across its
        // axis) takes the whole stack while it holds none, and holds back the next.
        if with_world(|w| w.dims[dim].brass.contains_key(&t)) == Some(true) {
            if !shape.junction() {
                return false;
            }
            let took = with_world(|w| {
                let d = &mut w.dims[dim];
                let b = d.brass.get_mut(&t)?;
                if b.stack.is_some() {
                    return Some(false);
                }
                // `setStackToDistribute`.
                b.stack = Some((item.item.clone(), item.count));
                b.entered_from = Some(movement.opposite());
                b.progress = -1;
                b.new_item = true;
                if !d.brass_running.contains(&t) {
                    d.brass_running.push(t);
                }
                w.mark_unsaved(dim, t);
                Some(true)
            })
            .flatten();
            if took == Some(true) {
                item.count = 0;
            }
            return true;
        }
        if !create_engine::tunnel::splits(axis, shape, movement, item.count) {
            return false;
        }
        for d in create_engine::tunnel::sides(axis, shape) {
            if d.axis() == axis {
                continue;
            }
            // A flap: not where another tunnel continues it.
            let o = d.offset();
            if tunnel_state(self.game.world.block(t[0] + o[0], t[1], t[2] + o[2]).0).is_some() {
                continue;
            }
            let out = [at[0] + o[0], at[1], at[2] + o[2]];
            let one = create_engine::belt::Item {
                count: 1,
                ..item.clone()
            };
            match self.insert_at(out, d, &one) {
                None => continue,
                Some(0) => {}
                Some(_) => return true,
            }
            item.count -= 1;
            if item.count <= 1 {
                break;
            }
        }
        false
    }

    fn process(
        &mut self,
        segment: i32,
        item: &mut create_engine::belt::Item,
        locked: bool,
    ) -> Option<create_engine::belt::Process> {
        use create_engine::belt::Process;
        use create_engine::press::{CYCLE, Mode};
        let at = create_engine::belt::position_for_offset(
            self.controller,
            &self.segment,
            self.length,
            segment,
        );
        let above = [at[0], at[1] + 2, at[2]];
        let dim = self.dim;
        let mut press = with_world(|w| w.dims[dim].presses.get(&above).copied()).flatten()?;
        if !locked && blocked(self.game, [at[0], at[1] + 1, at[2]]) {
            return None;
        }
        let speed = with_world(|w| w.speed(dim, above)).flatten().unwrap_or(0.0);
        if speed == 0.0 {
            return Some(Process::Pass);
        }
        if !locked {
            // BeltPressingCallbacks.onItemReceived.
            if press.running {
                return Some(Process::Hold);
            }
            let fits = RECIPES.with_borrow(|r| {
                r.as_ref()
                    .is_some_and(|r| r.pressing.find_single(&item.item).is_some())
            });
            if !fits {
                return Some(Process::Pass);
            }
            press.start(Mode::Belt);
            with_world(|w| {
                w.dims[dim].presses.insert(above, press);
                w.dims[dim].press_changes.push(above);
            });
            return Some(Process::Hold);
        }
        // whenItemHeld.
        if !press.running {
            return Some(Process::Pass);
        }
        if press.ticks != CYCLE / 2 {
            return Some(Process::Hold);
        }
        let seed = self.game.world.game_time as u64 ^ (at[0] as u64).wrapping_mul(0x9E37_79B9);
        let results = RECIPES.with_borrow(|r| {
            let r = r.as_ref()?;
            let id = r.pressing.find_single(&item.item)?;
            Some(create_engine::processing::roll(
                r.pressing.get(id),
                &mut create_engine::processing::Rng::new(seed),
            ))
        })?;
        let mut outputs = Vec::new();
        for (name, n) in results {
            let mut out = item.clone();
            out.item = name;
            out.count = n;
            out.locked = true;
            out.angle = random_angle(self.game, at);
            outputs.push(out);
        }
        let held = (item.count > 1).then(|| {
            let mut left = item.clone();
            left.count -= 1;
            left
        });
        Some(Process::Convert { held, outputs })
    }
}

/// The belts a client draws: what the server last said, run on tick by tick, and what was
/// uploaded for each item (so an item is uploaded again only when its motion changes).
#[derive(Default)]
pub(super) struct ClientBelts {
    pub(super) belts: HashMap<[i32; 3], (create_engine::belt::Contents, Vec<Shown>)>,
}

/// What was uploaded for an item: enough to tell, without looking anything up, whether it now
/// moves or looks differently.
#[derive(Clone, PartialEq)]
pub(super) struct Shown {
    pub(super) item: String,
    pub(super) count: u8,
    pub(super) angle: i32,
    pub(super) flat: bool,
    pub(super) velocity: [f32; 3],
    pub(super) copies: u8,
}

thread_local! {
    pub(super) static BELTS: RefCell<ClientBelts> = RefCell::new(ClientBelts::default());
    /// `isGui3d` by item id, looked up once.
    pub(super) static BLOCK_ITEMS: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
}

/// The client's belt surroundings: what the server said lies past the end; machines hold
/// locked items, nothing else acts.
pub(super) struct ClientSide(pub(super) create_engine::belt::Ending);

impl create_engine::belt::Surroundings for ClientSide {
    fn ending(&mut self) -> create_engine::belt::Ending {
        self.0
    }
    fn insert(&mut self, _: &create_engine::belt::Item) -> Option<u8> {
        None
    }
    fn process(
        &mut self,
        _: i32,
        item: &mut create_engine::belt::Item,
        locked: bool,
    ) -> Option<create_engine::belt::Process> {
        (locked || item.locked).then_some(create_engine::belt::Process::Hold)
    }
}

/// `BlockState.getSignal` / `getDirectSignal` of a latch or a redstone link (hot: one range test
/// for others).
pub(super) fn block_signal(
    dimension: horizon_world::level::Dimension,
    pos: horizon_core::position::BlockPos,
    state: BlockStateId,
    face: horizon_core::geometry::Direction,
    direct: bool,
    signal: &mut i32,
) {
    if states::index(state.0).is_none() {
        return;
    }
    // The hook names the face touching the reader; vanilla's direction points from the reader
    // to the block.
    let side = Dir::ALL[face as usize].opposite();
    if let Some((_, l)) = latch_state(state.0) {
        *signal = create_engine::latch::signal(&l, side);
        return;
    }
    if let Some((_, s)) = diode_state(state.0) {
        *signal = create_engine::diode::signal(&s, side);
        return;
    }
    if let Some(l) = lamp_state(state.0) {
        // The reader, on the face the hook names: another lamp reads 0.
        let o = Dir::ALL[face as usize].offset();
        let reader = [pos.x + o[0], pos.y + o[1], pos.z + o[2]];
        let reader_is_lamp = WORLD.with(|w| {
            w.try_borrow().ok().is_some_and(|w| {
                w.as_ref()
                    .and_then(|w| w.block_at(level_index(dimension), reader))
                    == Some(create_engine::Block::RoseQuartzLamp)
            })
        });
        *signal = create_engine::lamp::signal(&l, reader_is_lamp, direct);
        return;
    }
    if let Some((facing, receiver, _)) = link_state(state.0) {
        let received = if receiver {
            WORLD
                .with(|w| {
                    let w = w.try_borrow().ok()?;
                    Some(
                        w.as_ref()?.dims[level_index(dimension)]
                            .links
                            .received([pos.x, pos.y, pos.z]),
                    )
                })
                .unwrap_or(0)
        } else {
            0
        };
        *signal = create_engine::link::signal(receiver, facing, received, side, direct);
    }
}

/// `isSignalSource`: a latch, a brass diode and a rose quartz lamp give a signal, and a powered
/// receiver.
pub(super) fn is_signal_source(state: BlockStateId, source: &mut bool) {
    if latch_state(state.0).is_some()
        || diode_state(state.0).is_some()
        || lamp_state(state.0).is_some()
        || link_state(state.0).is_some_and(|(_, receiver, powered)| receiver && powered)
    {
        *source = true;
    }
}

/// A latch's or a brass diode's scheduled tick is the glue's: the repeater it is built on doesn't
/// run its own.
pub(super) fn tick_block(state: BlockStateId, handled: &mut bool) {
    if latch_state(state.0).is_some() || diode_state(state.0).is_some() {
        *handled = true;
    }
}

/// `WrenchItem.useOn` on a Create block: sneaking, `onSneakWrenched` (its item to the player
/// unless creative, the block gone without drops); else `onWrenched`, turned about the axis of
/// the face clicked. Belts (their own wrench use) and other blocks are left alone.
pub(super) fn use_item_on(
    game: &mut Game<'_>,
    stack: &horizon_client::items::ItemStack,
    pos: [i32; 3],
    face: [i32; 3],
    claimed: &mut bool,
) {
    // The face a placement that follows was clicked on (`BlockPlaceContext.getClickedFace`).
    CLICKED.set(Some((game.world.game_time, face)));
    if &*stack.item == "create:track" {
        use_track(game, pos, face, claimed);
        return;
    }
    if &*stack.item != "create:wrench" {
        return;
    }
    let state = game.world.block(pos[0], pos[1], pos[2]);
    if states::index(state.0).is_none() {
        return;
    }
    let block = with_world(|w| w.table.get(state.0).map(|(b, _)| b)).flatten();
    let Some(block) = block else {
        return;
    };
    if block == create_engine::Block::Belt {
        return;
    }
    // The hook runs for the level in place.
    let dimension = game.dim.id().to_owned();
    let Some(clicked) = Dir::from_offset(face) else {
        return;
    };
    *claimed = true;
    let registry = game.pack.assets.registry;
    let Some(current) = registry.state(state).cloned() else {
        return;
    };
    if game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft) {
        if !game.creative && game.items().get(&current.name).is_some() {
            game.native_give_items(&current.name, 1);
        }
        if let Some(air) = registry.default_state("minecraft:air") {
            game.native_set_block(&dimension, pos, air, super::native_hooks::flags::UPDATE_ALL);
        }
        return;
    }
    if block == create_engine::Block::Deployer
        && let Some(dim) = dimension_index(&dimension)
        && wrench_deployer(game, dim, pos)
    {
        return;
    }
    // A `facing` that never points up is a horizontal one (HORIZONTAL_FACING).
    let horizontal = {
        let mut up = current.clone();
        up.properties.insert("facing".into(), "up".into());
        current.properties.contains_key("facing") && registry.id(&up).is_none()
    };
    let props: std::collections::BTreeMap<String, String> = current
        .properties
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let turned = create_engine::wrench::rotated(&props, horizontal, clicked);
    if turned == props {
        return;
    }
    let mut next = current.clone();
    for (k, v) in turned {
        next.properties.insert(k, v);
    }
    if let Some(id) = registry.id(&next) {
        game.native_set_block(&dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
    }
}

/// The block states of windmills as text (`name[key=value,...]`), to save.
pub(super) fn state_text(registry: &BlockRegistry, id: u32) -> String {
    let Some(s) = registry.state(BlockStateId(id)) else {
        return "minecraft:air".into();
    };
    if s.properties.is_empty() {
        return s.name.clone();
    }
    let props: Vec<String> = s
        .properties
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    format!("{}[{}]", s.name, props.join(","))
}

pub(super) fn state_of_text(registry: &BlockRegistry, text: &str) -> Option<u32> {
    let (name, rest) = text.split_once('[').unwrap_or((text, ""));
    let default = registry.default_state(name)?;
    let mut s = registry.state(default)?.clone();
    for kv in rest
        .trim_end_matches(']')
        .split(',')
        .filter(|x| !x.is_empty())
    {
        let (k, v) = kv.split_once('=')?;
        s.properties.insert(k.to_owned(), v.to_owned());
    }
    registry.id(&s).map(|i| i.0)
}

/// Tells the players in `dimension` of gauges whose needle moved after a solve.
pub(super) fn send_gauge_changes(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    gauges: &[[i32; 3]],
) {
    if gauges.is_empty() {
        return;
    }
    let players = players_in(game, dimension);
    send_gauges(game, &players, dim, gauges);
}

/// Tells `players` what the gauges at `gauges` show (`GaugeBlockEntity.sendData`).
pub(super) fn send_gauges(game: &mut Game<'_>, players: &[u128], dim: usize, gauges: &[[i32; 3]]) {
    let all: Vec<([i32; 3], create_engine::gauge::Shown)> = with_world(|w| {
        gauges
            .iter()
            .filter_map(|p| Some((*p, w.dims[dim].gauges.get(p)?.shown)))
            .collect()
    })
    .unwrap_or_default();
    for m in create_engine::gauge::encode(&all) {
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::gauge::CHANNEL, &m);
        }
    }
}

/// Gauges on the client: what the server last said of each (for goggles), each needle, and the
/// needles still moving (the only ones that tick).
#[derive(Default)]
pub(super) struct ClientGauges {
    pub(super) shown: HashMap<[i32; 3], create_engine::gauge::Shown>,
    pub(super) needles: HashMap<[i32; 3], create_engine::gauge::Needle>,
    pub(super) moving: Vec<[i32; 3]>,
}

impl ClientGauges {
    pub(super) fn forget(&mut self, pos: [i32; 3]) {
        self.shown.remove(&pos);
        self.needles.remove(&pos);
        self.moving.retain(|p| *p != pos);
    }
}

thread_local! {
    pub(super) static GAUGES: RefCell<ClientGauges> = RefCell::new(ClientGauges::default());
}

/// The server's gauges: each needle sets off toward its new target.
pub(super) fn gauges_from_server(data: &[u8]) {
    GAUGES.with_borrow_mut(|g| {
        for (pos, s) in create_engine::gauge::decode(data) {
            g.shown.insert(pos, s);
            g.needles.entry(pos).or_default().target = s.dial;
            if !g.moving.contains(&pos) {
                g.moving.push(pos);
            }
        }
    });
}

/// Each client tick: moving needles take a step (`GaugeBlockEntity.tick`) and glide to it; a
/// settled one is drawn still (`GaugeVisual.beginFrame` leaves it once `Mth.equal`) and leaves
/// the list, so no glide outlives its tick.
pub(super) fn tick_client_gauges(game: &mut Game<'_>, c: &mut ClientParts) {
    // Taken out and put back (no copy): drawing does not touch the gauges.
    let mut moving = GAUGES.with_borrow_mut(|g| std::mem::take(&mut g.moving));
    if moving.is_empty() {
        return;
    }
    let time = game.world.game_time as u64;
    moving.retain(|&pos| {
        let Some(mut n) = GAUGES.with_borrow(|g| g.needles.get(&pos).copied()) else {
            return false;
        };
        let mut rng = create_engine::processing::Rng::new(time ^ client::key(pos, 0));
        n.tick(&mut rng);
        let settled = n.settled();
        let drawn = if settled {
            create_engine::gauge::Needle { prev: n.state, ..n }
        } else {
            n
        };
        draw_gauge_dials(game, c, pos, &drawn);
        GAUGES.with_borrow_mut(|g| g.needles.insert(pos, n));
        !settled
    });
    GAUGES.with_borrow_mut(|g| {
        // Anything added meanwhile stays (none is today).
        moving.append(&mut g.moving);
        g.moving = moving;
    });
}

/// Turns the dials of the gauge at `pos` from the needle's last tick to this one
/// (`GaugeVisual.DialFace.updateTransform`, about the dial's pivot).
pub(super) fn draw_gauge_dials(
    game: &mut Game<'_>,
    c: &mut ClientParts,
    pos: [i32; 3],
    n: &create_engine::gauge::Needle,
) {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some((facing, along_first)) = c
        .table
        .get(state.0)
        .filter(|(b, _)| create_engine::world::gauge_kind(*b).is_some())
        .and_then(|(_, i)| Some((i.facing?, i.along_first)))
    else {
        return;
    };
    let axis = create_engine::parts::directional_axis(facing, along_first);
    let registry = game.pack.assets.registry;
    let now = game.native_parts_seconds();
    for (i, face) in create_engine::gauge::dial_faces(facing, axis).enumerate() {
        let model = Model::GaugeDial(face);
        let handle = match c.models.get(&(state.0, model)) {
            Some(&h) => Some(h),
            None => part_model(game, registry, state, model).inspect(|&h| {
                c.models.insert((state.0, model), h);
            }),
        };
        let Some(h) = handle else {
            continue;
        };
        let g = create_engine::gauge::dial_glide(face, n.prev, n.state, now);
        game.native_part_glide(
            client::key(pos, client::GAUGE_DIAL_SLOT + i as u8),
            h,
            pos,
            axis_index(g.axis),
            g.rpm,
            g.phase,
            g.offset,
            g.velocity,
        );
    }
}
