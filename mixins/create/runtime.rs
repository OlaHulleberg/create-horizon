// Game glue for the Create port, injected as `crate::game::create`. Rules live in the
// `create_engine` crate; this module only turns the game's hooks into engine calls.
// State lives on the server thread that fires the hooks (one per world).

use super::Game;
use super::create_contraptions::*;
use super::create_processing::*;
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
    let burners = burners_from_nbt(Game::native_chunk_data(chunk, "create"));
    let links = links_from_nbt(Game::native_chunk_data(chunk, "create"));
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
    let mut buffer = [0u32; 4096];
    with_world(|w| {
        w.load_settings(dim, saved.into_iter());
        w.load_belt_items(dim, belts.into_iter());
        w.load_tanks(dim, tanks.into_iter());
        w.load_windmills(dim, windmills.into_iter());
        w.load_crushers(dim, crushers.into_iter());
        w.load_trains(dim, trains);
        w.load_burners(dim, burners.into_iter());
        w.load_links(dim, links.into_iter());
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
    }
    tick_latches(game, dimension, dim);
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
    tick_pistons(game, dimension, dim);
    tick_trains(game, dimension, dim);
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
    let Some((removed, unsaved)) = with_world(|w| {
        w.tick(dim, &mut events);
        (w.dims[dim].kinetics.take_removed(), w.take_unsaved(dim))
    }) else {
        return;
    };
    for (cx, cz) in unsaved {
        game.native_mark_chunk_unsaved(dimension, [cx, cz]);
    }
    if events.is_empty() && removed.is_empty() {
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
    let belts: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].belts.keys().copied().collect()).unwrap_or_default();
    send_belts(game, &[player], dim, &belts);
    let tanks: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].tanks.keys().copied().collect()).unwrap_or_default();
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
    let deployers: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].deployers.keys().copied().collect()).unwrap_or_default();
    send_deployers(game, &[player], dim, &deployers);
    let trains: Vec<u32> =
        with_world(|w| w.dims[dim].trains.iter().map(|t| t.id).collect()).unwrap_or_default();
    for id in trains {
        send_train(game, dim, [i32::MIN, 0, id as i32]);
    }
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
    if block == Some(create_engine::Block::Depot) {
        use_depot(game, dim, pos, face, canceled);
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
        Some(B::CreativeMotor) => {
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

/// Values set by the wheel, from the server to the player who set them.
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
    VALUES.with_borrow_mut(|m| m.insert([x, y, z], v));
}

/// `GoggleOverlayRenderer` for kinetic blocks: with goggles on the head, the block under the
/// crosshair tells its stress (`KineticBlockEntity.addToGoggleTooltip`: impact × speed;
/// `GeneratingKineticBlockEntity`: capacity × speed), in a tooltip right of the crosshair.
pub(super) fn hud(game: &mut Game<'_>, batch: &mut horizon_client::hud::HudBatch) {
    value_tip(game, batch);
    goggle_overlay(game, batch);
}

/// `ValueSettingsClient.render` for the creative motor's speed (`KineticScrollValueBehaviour`):
/// its label over the hotbar, centred, with the value and how to change it here (the wheel:
/// Create 6's value board is not ported).
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
    let Some((impact, capacity, speed)) = CLIENT.with_borrow(|c| {
        let c = c.as_ref()?;
        let (block, info) = c.table.get(state.0)?;
        let [part, _] = create_engine::parts::parts(block, info, &Default::default());
        let part = part?;
        let speed = c.speeds.get(&pos).map_or(0.0, |s| s[0]);
        Some((part.impact, part.capacity, speed))
    }) else {
        return;
    };
    // Create's tooltip: a header, then each stat indented by four spaces and its value by one more.
    let mut lines: Vec<(String, u32)> = Vec::new();
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
        lines.push(("    Kinetic Stats:".into(), 0xFFFFFF));
        lines.push(("    Kinetic Stress Impact:".into(), 0xAAAAAA));
        lines.push((
            format!("     {}su at current speed", number(impact * speed.abs())),
            0x55FFFF,
        ));
    }
    if capacity != 0.0 {
        lines.push(("    Generator Stats:".into(), 0xFFFFFF));
        lines.push(("    Kinetic Stress Capacity:".into(), 0xAAAAAA));
        lines.push((
            format!("     {}su at current speed", number(capacity * speed.abs())),
            0x55FFFF,
        ));
    }
    if lines.is_empty() {
        return;
    }
    // RemovedGuiUtils.drawHoveringText at (width / 2 + 20, height / 2): vanilla's tooltip box.
    let w = lines
        .iter()
        .map(|l| game.native_text_width(&l.0))
        .fold(0.0f32, f32::max);
    let h = lines.len() as f32 * 10.0 - 2.0;
    let x = (batch.width / 2.0 + 20.0).min(batch.width - w - 20.0) + 4.0;
    let y = (batch.height / 2.0).min(batch.height - h - 20.0) + 4.0;
    game.native_fill(batch, [x - 3.0, y - 4.0, w + 6.0, h + 8.0], 0xF010_0010);
    game.native_fill(batch, [x - 3.0, y - 3.0, w + 6.0, 1.0], 0x5050_00FF);
    game.native_fill(batch, [x - 3.0, y + h + 2.0, w + 6.0, 1.0], 0x5028_007F);
    for (i, (text, rgb)) in lines.iter().enumerate() {
        game.native_text(batch, text, [x, y + i as f32 * 10.0], *rgb, true);
    }
}

/// The mouse wheel on the client: a creative motor under the crosshair takes it.
pub(super) fn wheel(game: &mut Game<'_>, steps: f32, claimed: &mut bool) {
    if *claimed || !client_ensure(game) {
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
    // The motor's speed, a brass tunnel's mode, and (with the wrench, `requiresWrench`) a
    // chassis' range.
    let scrolls = CLIENT.with_borrow(|c| {
        c.as_ref()
            .and_then(|c| c.table.get(state.0))
            .is_some_and(|(b, _)| {
                use create_engine::Block as B;
                matches!(b, B::CreativeMotor | B::BrassTunnel)
                    || wrench && matches!(b, B::LinearChassis | B::SecondaryLinearChassis)
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

/// `DeployerBlockEntity.write`: `State`, `Timer`, `Reach` and the held stack, per deployer.
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
    let burners = with_world(|w| w.column_burners(dim, chunk[0], chunk[1])).unwrap_or_default();
    let links = with_world(|w| w.column_links(dim, chunk[0], chunk[1])).unwrap_or_default();
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
    if settings.is_empty()
        && belts.is_empty()
        && tanks.is_empty()
        && windmills.is_empty()
        && crushers.is_empty()
        && burners.is_empty()
        && links.is_empty()
        && deployers.is_empty()
        && ranges.is_empty()
        && basins.is_empty()
        && brass.is_empty()
        && trains.is_empty()
    {
        data.remove(&"create".into());
        return;
    }
    let mut own = horizon_world::nbt::Compound::new();
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
    if !links.is_empty() {
        own.insert("links".into(), links_to_nbt(&links));
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
        // x, y, z, fluid, amount for each tank (`FluidTankBlockEntity`'s `TankContent`).
        let v = tanks
            .iter()
            .flat_map(|(p, t)| {
                [
                    p[0],
                    p[1],
                    p[2],
                    i32::from(t.fluid.map_or(255, |f| f.index())),
                    t.amount as i32,
                ]
            })
            .collect();
        own.insert("tanks".into(), horizon_world::nbt::Tag::IntArray(v));
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
        return;
    }
    tick_client_depots(game);
    tick_client_presses(game);
    tick_client_belts(game);
    if !client_ensure(game) {
        return;
    }
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
}

pub(super) fn perform_command(game: &mut Game<'_>, command: &str, ran: &mut bool) {
    let train = command.strip_prefix("create_debug train ");
    let assemble = command.strip_prefix("create_debug assemble ");
    let Some(rest) = command
        .strip_prefix("create_debug speed ")
        .or(train)
        .or(assemble)
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
            Some(off) => Some(here?.get(n)?.floor() as i32 + off.parse::<i32>().unwrap_or(0)),
            None => t.parse().ok(),
        })
        .collect();
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

/// `BeltHelper.isItemUpright`: items of `create:upright_on_belt` stand, unless also in
/// `create:not_upright_on_belt`. (Fluid-holding items stand in Create too: not yet.)
pub(super) fn upright(game: &Game<'_>, item: &str) -> bool {
    let tags = &game.items().item_tags;
    let has = |t: &str| tags.get(t).is_some_and(|m| m.contains(item));
    has("create:upright_on_belt") && !has("create:not_upright_on_belt")
}

/// `TransportedItemStack`'s `R.nextInt(360)`: Create's shared random; any angle will do.
pub(super) fn random_angle(game: &Game<'_>, pos: [i32; 3]) -> i32 {
    let mut x = (game.world.game_time as u64)
        ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (pos[2] as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (pos[1] as u64) << 40;
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x % 360) as i32
}

/// `SharedDepotBlockMethods.onUse` on the depot's top: everything it holds goes to the
/// player, then the stack in hand is put down, sliding in from the player's side.
pub(super) fn use_depot(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    if face != [0, 1, 0] {
        return;
    }
    *canceled = true;
    // The main hand's use always succeeds, so the off hand is never asked.
    if game.hand != horizon_client::first_person::Hand::Main {
        return;
    }
    let hand = game.inventory.get_mut(game.selected).and_then(Option::take);
    let facing = create_engine::depot::Side::of_yaw(game.yaw);
    let put = hand.as_ref().map(|s| {
        create_engine::depot::Held::put_by_player(
            random_angle(game, pos),
            upright(game, &s.item),
            facing,
        )
    });
    let taken = game.native_with_mod_block_entity(pos, |be| {
        be.items
            .resize(be.items.len().max(create_engine::depot::SLOTS), None);
        let taken: Vec<_> = be.items.iter_mut().filter_map(Option::take).collect();
        be.items[0] = hand.clone();
        taken
    });
    let Some(taken) = taken else {
        // No block entity: the stack stays in hand.
        if let Some(slot) = game.inventory.get_mut(game.selected) {
            *slot = hand;
        }
        return;
    };
    for s in taken {
        game.native_give_items(&s.item, s.count);
    }
    with_world(|w| w.depot_put(dim, pos, put));
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
    // An item put into a crafter: its chain may start (`insertItem` -> `checkCompletedRecipe`).
    if with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
        == Some(create_engine::Block::MechanicalCrafter)
    {
        with_world(|w| w.dims[dim].crafter_checks.push(pos));
        return;
    }
    // A chute was given a stack (a hopper above): it falls from the top.
    if with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
        == Some(create_engine::Block::Chute)
    {
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

/// `DepotBehaviour.tick` for depots whose stack slides, then the depots whose contents
/// changed go to the players of the level.
pub(super) fn tick_depots(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let sliding = with_world(|w| w.dims[dim].sliding.clone()).unwrap_or_default();
    if !sliding.is_empty() {
        let standing: Vec<[i32; 3]> = sliding
            .iter()
            .copied()
            .filter(|p| {
                depot_items(game, *p)
                    .first()
                    .is_some_and(|(slot, item, _)| *slot == 0 && upright(game, item))
            })
            .collect();
        with_world(|w| w.tick_depots(dim, |p| standing.contains(&p)));
    }
    let changed = with_world(|w| w.take_depot_changes(dim)).unwrap_or_default();
    if changed.is_empty() {
        return;
    }
    // A depot that joined holding a stack (a saved world, a block placed with its items) has
    // it at rest in the centre, as Create reads `HeldItem` back.
    for &pos in &changed {
        let unknown = with_world(|w| matches!(w.dims[dim].depots.get(&pos), Some(None)));
        if unknown != Some(true) {
            continue;
        }
        if let Some((_, item, _)) = depot_items(game, pos).into_iter().find(|s| s.0 == 0) {
            let held =
                create_engine::depot::Held::new(random_angle(game, pos), upright(game, &item));
            with_world(|w| {
                if let Some(h) = w.dims[dim].depots.get_mut(&pos) {
                    *h = Some(held);
                }
                w.wake_depot(dim, pos);
            });
        }
    }
    let players: Vec<u128> = game
        .native_server_players()
        .into_iter()
        .filter(|p| p.dimension == dimension)
        .map(|p| p.uuid)
        .collect();
    send_depots(game, &players, dim, &changed);
}

/// The stacks of the depot at `pos`, by slot.
pub(super) fn depot_items(game: &Game<'_>, pos: [i32; 3]) -> Vec<(u8, String, u8)> {
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(horizon_client::containers::BlockEntity::Mod(m)) => m
            .items
            .iter()
            .take(create_engine::depot::SLOTS)
            .enumerate()
            .filter_map(|(i, s)| {
                let s = s.as_ref()?;
                Some((i as u8, s.item.to_string(), s.count.min(255) as u8))
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn send_depots(game: &mut Game<'_>, players: &[u128], dim: usize, depots: &[[i32; 3]]) {
    let contents: Vec<create_engine::depot::Contents> = depots
        .iter()
        .map(|&pos| {
            let held = with_world(|w| w.dims[dim].depots.get(&pos).copied().flatten()).flatten();
            let stacks = if with_world(|w| w.dims[dim].depots.contains_key(&pos)) == Some(true) {
                depot_items(game, pos)
            } else {
                Vec::new()
            };
            create_engine::depot::Contents {
                pos,
                held: held.unwrap_or(create_engine::depot::Held::new(0, false)),
                stacks,
            }
        })
        .collect();
    for m in create_engine::depot::encode(&contents) {
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::depot::CHANNEL, &m);
        }
    }
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

/// A key for copy `copy` of the stack in `slot` of the depot at `pos`, apart from the kinetic
/// parts' keys (a well-mixed hash of the three).
pub(super) fn depot_key(pos: [i32; 3], slot: u8, copy: u8) -> u64 {
    let mut x = client::key(pos, 0)
        ^ (u64::from(slot) << 8 | u64::from(copy)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31) | 1 << 63
}

pub(super) const COPIES: u8 = 4;

pub(super) fn hide_depot(game: &mut Game<'_>, pos: [i32; 3]) {
    for slot in 0..create_engine::depot::SLOTS as u8 {
        for copy in 0..COPIES {
            game.native_part_clear(depot_key(pos, slot, copy));
        }
    }
}

/// Draws a depot's stacks as Create's `DepotRenderer` does: the held stack at its offset, turned
/// by its angle; each output 0.35 out at 45° steps. `next` is where the held stack will be a
/// tick from now, for the GPU to glide it there.
pub(super) fn show_depot(
    game: &mut Game<'_>,
    c: &create_engine::depot::Contents,
    next: Option<create_engine::depot::Held>,
) {
    use create_engine::depot;
    hide_depot(game, c.pos);
    let now = game.native_parts_seconds();
    for (slot, item, count) in &c.stacks {
        let block_item = game.native_item_uses_block_light(item).unwrap_or(false);
        let (poses, phase, offset, velocity) = if *slot == 0 {
            let at = c.held.offset();
            let to = next.map_or(at, |n| n.offset());
            let poses = depot::stack_poses(
                u32::from(*count),
                block_item,
                &mut depot::JavaRandom::new(0),
            );
            (poses, c.held.angle as f32, at, (to - at) * 20.0)
        } else {
            let (poses, phase) =
                depot::output_place(usize::from(*slot) - 1, u32::from(*count), block_item);
            (poses, phase, glam::Vec3::ZERO, glam::Vec3::ZERO)
        };
        for (copy, pose) in poses.into_iter().take(usize::from(COPIES)).enumerate() {
            let Some(model) = game.native_part_item_model(item, depot::top_centre() * pose) else {
                continue;
            };
            game.native_part_glide(
                depot_key(c.pos, *slot, copy as u8),
                model,
                c.pos,
                1,
                0.0,
                phase,
                [offset.x, offset.y, offset.z, now],
                velocity.to_array(),
            );
        }
    }
}

/// Depots' contents from the server.
pub(super) fn depots_from_server(game: &mut Game<'_>, data: &[u8]) {
    for c in create_engine::depot::decode(data) {
        let pos = c.pos;
        if c.stacks.is_empty() {
            hide_depot(game, pos);
            DEPOTS.with_borrow_mut(|d| {
                d.shown.remove(&pos);
                d.sliding.retain(|p| *p != pos);
            });
            continue;
        }
        let sliding = c.held.sliding() && c.stacks.iter().any(|s| s.0 == 0);
        let mut next = c.held;
        next.slide(false);
        show_depot(game, &c, sliding.then_some(next));
        DEPOTS.with_borrow_mut(|d| {
            d.shown.insert(pos, c);
            d.sliding.retain(|p| *p != pos);
            if sliding {
                d.sliding.push(pos);
            }
        });
    }
}

/// Each client tick: held stacks still sliding move on a tick (`DepotBehaviour.tick` on the
/// client), drawn gliding to where they will be next.
pub(super) fn tick_client_depots(game: &mut Game<'_>) {
    let sliding = DEPOTS.with_borrow(|d| d.sliding.clone());
    for pos in sliding {
        let Some(mut c) = DEPOTS.with_borrow(|d| d.shown.get(&pos).cloned()) else {
            continue;
        };
        let up = c
            .stacks
            .iter()
            .find(|s| s.0 == 0)
            .is_some_and(|s| upright(game, &s.1));
        c.held.slide(up);
        let still = c.held.sliding();
        let mut next = c.held;
        next.slide(up);
        show_depot(game, &c, still.then_some(next));
        DEPOTS.with_borrow_mut(|d| {
            d.shown.insert(pos, c);
            if !still {
                d.sliding.retain(|p| *p != pos);
            }
        });
    }
}

/// `BeltProcessingBehaviour.isBlocked`: a block with a collision shape between a depot and the
/// press above it. (Funnels, which Create lets through, are not ported yet.)
pub(super) fn blocked(game: &Game<'_>, at: [i32; 3]) -> bool {
    let state = game.world.block(at[0], at[1], at[2]);
    game.res.behaviours().facts(state).flags
        & horizon_world::level::data::state_flags::COLLISION_EMPTY
        == 0
}

/// The players of `dimension`.
pub(super) fn players_in(game: &Game<'_>, dimension: &str) -> Vec<u128> {
    game.native_server_players()
        .into_iter()
        .filter(|p| p.dimension == dimension)
        .map(|p| p.uuid)
        .collect()
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
    if with_world(|w| w.table.get(state.0).map(|(b, _)| b)).flatten()
        == Some(create_engine::Block::FluidPipe)
    {
        // FluidPipeBlock.getStateForPlacement: straight toward the player's view when nothing is
        // around.
        let looking = create_engine::place::ordered_by_nearest(game.yaw, game.pitch)[0];
        let dimension = game.dim.id().to_owned();
        connect_pipe(game, &dimension, pos, Some(looking));
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
}

/// A stack's limit (at most 64).
pub(super) fn stack_limit(game: &Game<'_>, item: &str) -> u8 {
    game.items()
        .get(item)
        .map_or(64, |x| x.max_stack)
        .clamp(1, 64) as u8
}

/// `ChuteBlockEntity` taking an item that landed on it (`canAcceptItem`: only while empty): the
/// whole stack, at the top.
pub(super) fn chute_take(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: &mut horizon_client::items::ItemStack,
) {
    let taken = game
        .native_with_mod_block_entity(pos, |be| {
            be.items.resize(be.items.len().max(1), None);
            if be.items[0].is_some() {
                return false;
            }
            be.items[0] = Some(stack.clone());
            true
        })
        .unwrap_or(false);
    if taken {
        stack.count = 0;
        with_world(|w| w.dims[dim].chutes.insert(pos, 1.0));
    }
}

pub(super) fn chute_stack(
    game: &Game<'_>,
    pos: [i32; 3],
) -> Option<horizon_client::items::ItemStack> {
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(horizon_client::containers::BlockEntity::Mod(m)) => m.items.first().cloned().flatten(),
        _ => None,
    }
}

pub(super) fn set_chute_stack(
    game: &mut Game<'_>,
    pos: [i32; 3],
    stack: Option<horizon_client::items::ItemStack>,
) {
    game.native_with_mod_block_entity(pos, |be| {
        be.items.resize(be.items.len().max(1), None);
        be.items[0] = stack;
    });
}

/// Chutes holding a stack (`ChuteBlockEntity.tick` falling, `handleDownwardOutput`): into the
/// container below (as a hopper puts in), else the chute below if it is empty, else out of the
/// bottom when nothing solid is under it; it waits in the middle while the way is shut.
pub(super) fn tick_chutes(game: &mut Game<'_>, dim: usize) {
    use create_engine::chute::{Fall, fall};
    let moving: Vec<([i32; 3], f32)> =
        with_world(|w| w.dims[dim].chutes.iter().map(|(p, v)| (*p, *v)).collect())
            .unwrap_or_default();
    for (pos, position) in moving {
        let Some(stack) = chute_stack(game, pos) else {
            with_world(|w| w.dims[dim].chutes.remove(&pos));
            continue;
        };
        let below = [pos[0], pos[1] - 1, pos[2]];
        let below_state = game.world.block(below[0], below[1], below[2]);
        let below_chute = with_world(|w| w.table.get(below_state.0).map(|(b, _)| b)).flatten()
            == Some(create_engine::Block::Chute);
        let container = !below_chute
            && game
                .world
                .block_entities
                .contains_key(&(below[0], below[1], below[2]));
        let can = if below_chute {
            chute_stack(game, below).is_none()
        } else if container {
            true
        } else {
            // Block.canSupportRigidBlock: nothing to drop onto.
            game.res.behaviours().facts(below_state).flags
                & horizon_world::level::data::state_flags::COLLISION_EMPTY
                != 0
        };
        let next = match fall(position, can) {
            Fall::At(p) => Some(p),
            Fall::Out if below_chute => {
                set_chute_stack(game, below, Some(stack));
                set_chute_stack(game, pos, None);
                with_world(|w| w.dims[dim].chutes.insert(below, 1.0));
                None
            }
            Fall::Out if container => {
                let count = stack.count;
                let left = game
                    .native_insert_item(below, horizon_core::geometry::Direction::Up, stack.clone())
                    .filter(|s| s.count > 0);
                if left.as_ref().is_some_and(|l| l.count == count) {
                    Some(0.5)
                } else {
                    let done = left.is_none();
                    set_chute_stack(game, pos, left);
                    (!done).then_some(0.5)
                }
            }
            Fall::Out => {
                // An item 12/16 below the centre, falling at a quarter block a tick.
                let (x, y, z) = (
                    f64::from(pos[0]) + 0.5,
                    f64::from(pos[1]) + 0.5 - 0.75,
                    f64::from(pos[2]) + 0.5,
                );
                game.native_run_command(&format!(
                    "summon minecraft:item {x} {y} {z} {{Item:{{id:'{}',count:{}}},Motion:[0d,-0.25d,0d]}}",
                    stack.item, stack.count
                ));
                set_chute_stack(game, pos, None);
                None
            }
        };
        with_world(|w| match next {
            Some(p) => {
                w.dims[dim].chutes.insert(pos, p);
            }
            None => {
                w.dims[dim].chutes.remove(&pos);
            }
        });
    }
}

/// Spouts holding fluid (`SpoutBlockEntity.whenItemHeld` on the depot two below, `tick`): the
/// held empty bucket is filled after the 20-tick cycle (at its 5th-last tick).
pub(super) fn tick_spouts(game: &mut Game<'_>, dim: usize) {
    let spouts: Vec<([i32; 3], create_engine::spout::Spout)> = with_world(|w| {
        w.dims[dim]
            .spouts
            .iter()
            .filter(|(_, s)| s.fluid.is_some() || s.processing >= 0)
            .map(|(p, s)| (*p, *s))
            .collect()
    })
    .unwrap_or_default();
    for (pos, mut s) in spouts {
        let depot = [pos[0], pos[1] - 2, pos[2]];
        let on_depot = with_world(|w| w.dims[dim].depots.contains_key(&depot)).unwrap_or(false);
        let held = on_depot
            .then(|| depot_items(game, depot).into_iter().find(|x| x.0 == 0))
            .flatten();
        if !ensure_recipes(game) {
            continue;
        }
        let filled = RECIPES.with_borrow(|r| {
            let r = r.as_ref()?;
            create_engine::spout::step(&mut s, held.as_ref().map(|h| h.1.as_str()), &r.filling)
        });
        if let (Some(out), Some((_, item, count))) = (filled, held) {
            let out = out.as_str();
            // `convertToAndLeaveHeld`: one is filled; the rest stays held, the filled one goes to
            // the outputs.
            game.native_with_mod_block_entity(depot, |be| {
                be.items
                    .resize(be.items.len().max(create_engine::depot::SLOTS), None);
                let stack = |i: &str, n: u32| horizon_client::items::ItemStack {
                    item: i.into(),
                    count: n,
                    damage: 0,
                    extra: None,
                };
                if count <= 1 {
                    be.items[0] = Some(stack(out, 1));
                } else {
                    be.items[0] = Some(stack(&item, u32::from(count) - 1));
                    // Into the outputs as the depot stacks them (`insertItemStacked`).
                    let outputs = &mut be.items[create_engine::depot::OUTPUTS];
                    if let Some(s) = outputs
                        .iter_mut()
                        .flatten()
                        .find(|s| &*s.item == out && s.count < 64)
                    {
                        s.count += 1;
                    } else if let Some(slot) = outputs.iter_mut().find(|s| s.is_none()) {
                        *slot = Some(stack(out, 1));
                    }
                }
            });
            with_world(|w| w.depot_changed(dim, depot));
        }
        with_world(|w| w.dims[dim].spouts.insert(pos, s));
    }
}

/// Item drains holding items (`ItemDrainBlockEntity.tick`): a filled bucket is emptied into the
/// tank each cycle while it has room; the empty bucket is thrown off on top; anything else is
/// let go the same way.
pub(super) fn tick_drains(game: &mut Game<'_>, dim: usize) {
    let drains: Vec<[i32; 3]> = with_world(|w| {
        w.dims[dim]
            .drains
            .iter()
            .filter(|(_, d)| d.held.is_some())
            .map(|(p, _)| *p)
            .collect()
    })
    .unwrap_or_default();
    if !drains.is_empty() && !ensure_recipes(game) {
        return;
    }
    for pos in drains {
        let Some(out) = RECIPES.with_borrow(|r| {
            let emptying = r.as_ref().map_or(&[][..], |r| &r.emptying[..]);
            with_world(|w| {
                let d = w.dims[dim].drains.get_mut(&pos)?;
                let held = d.held.clone();
                if held
                    .as_ref()
                    .is_some_and(|h| create_engine::drain::contents(&h.0, emptying).is_none())
                {
                    d.held = None;
                    return held.map(|h| (h.0, h.1));
                }
                create_engine::drain::tick(d, emptying)?.map(|left| (left, 1))
            })
            .flatten()
        }) else {
            continue;
        };
        let (x, y, z) = (
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]) + 1.0,
            f64::from(pos[2]) + 0.5,
        );
        game.native_run_command(&format!(
            "summon minecraft:item {x} {y} {z} {{Item:{{id:'{}',count:{}}},PickupDelay:20}}",
            out.0, out.1
        ));
    }
}

/// Extracting funnels (`FunnelBlockEntity.activateExtractor`): every 8 ticks, while no item lies
/// in front, a stack leaves the inventory behind and drops in front.
pub(super) fn tick_funnels(game: &mut Game<'_>, dim: usize) {
    let funnels: Vec<[i32; 3]> = with_world(|w| {
        let d = &mut w.dims[dim];
        d.funnels
            .iter_mut()
            .filter_map(|(p, cooldown)| {
                if *cooldown == u32::MAX {
                    return None;
                }
                if *cooldown > 0 {
                    *cooldown -= 1;
                    return None;
                }
                Some(*p)
            })
            .collect()
    })
    .unwrap_or_default();
    for pos in funnels {
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let Some(s) = game.pack.assets.registry.state(state) else {
            continue;
        };
        if s.properties.get("extracting").is_none_or(|v| v != "true")
            || s.properties.get("powered").is_some_and(|v| v == "true")
        {
            // Inserting or paused: dormant until its state changes.
            with_world(|w| w.dims[dim].funnels.insert(pos, u32::MAX));
            continue;
        }
        let Some(facing) =
            with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten()
        else {
            continue;
        };
        let (at, v) = create_engine::funnel::output(pos, facing);
        // Something still in front: wait (`lastObserved`, the overflow area).
        let mut blocked = false;
        let dimension = game.dim.id().to_owned();
        items_in(
            game,
            &dimension,
            [at[0] - 0.5, at[1] - 0.5, at[2] - 0.5],
            [at[0] + 0.5, at[1] + 0.5, at[2] + 0.5],
            |_, _, _, _| {
                blocked = true;
            },
        );
        if blocked {
            continue;
        }
        let o = facing.offset();
        let behind = [pos[0] - o[0], pos[1] - o[1], pos[2] - o[2]];
        let face = horizon_core::geometry::Direction::ALL[facing as usize];
        let Some(stack) = super::machines::create_extract::extract_item(
            game,
            behind,
            face,
            create_engine::funnel::AMOUNT,
        ) else {
            continue;
        };
        game.native_run_command(&format!(
            "summon minecraft:item {} {} {} {{Item:{{id:'{}',count:{}}},Motion:[{}d,{}d,{}d]}}",
            at[0], at[1], at[2], stack.item, stack.count, v[0], v[1], v[2]
        ));
        with_world(|w| {
            w.dims[dim]
                .funnels
                .insert(pos, create_engine::funnel::EXTRACTION_COOLDOWN)
        });
    }
}

/// What a block is to the pipes (`FluidPropagator`): a pipe and its open sides, a pump and its
/// facing, a tank, open space a pipe may end in, or neither.
pub(super) fn pipe_cell(game: &Game<'_>, p: [i32; 3]) -> create_engine::fluids::Cell {
    use create_engine::Block;
    use create_engine::fluids::Cell;
    if !game.world.loaded(p[0], p[2]) {
        return Cell::Solid;
    }
    let state = game.world.block(p[0], p[1], p[2]);
    if let Some((block, info)) =
        with_world(|w| w.table.get(state.0).map(|(b, i)| (b, *i))).flatten()
    {
        return match block {
            Block::FluidPipe => Cell::Pipe(pipe_sides_of(game, state)),
            Block::MechanicalPump => info.facing.map_or(Cell::Solid, Cell::Pump),
            Block::FluidTank | Block::Spout | Block::ItemDrain | Block::Basin => Cell::Tank,
            _ => Cell::Solid,
        };
    }
    let b = game.res.behaviours();
    if b.is_replaceable(state) || b.fluid(state).kind() != horizon_world::fluid::FluidKind::Empty {
        Cell::Open
    } else {
        Cell::Solid
    }
}

/// A pipe state's open sides (`Dir::bit`).
pub(super) fn pipe_sides_of(game: &Game<'_>, state: BlockStateId) -> u8 {
    let Some(s) = game.pack.assets.registry.state(state) else {
        return 0;
    };
    Dir::ALL
        .into_iter()
        .filter(|d| s.properties.get(dir_name(*d)).is_some_and(|v| v == "true"))
        .fold(0, |m, d| m | d.bit())
}

/// `FluidPipeBlock.updateBlockState` for the pipe at `pos`, written when it changes.
pub(super) fn connect_pipe(
    game: &mut Game<'_>,
    dimension: &str,
    pos: [i32; 3],
    preferred: Option<Dir>,
) {
    use create_engine::fluids;
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let is_pipe = with_world(|w| w.table.get(state.0).map(|(b, _)| b)).flatten()
        == Some(create_engine::Block::FluidPipe);
    if !is_pipe {
        return;
    }
    let previous = pipe_sides_of(game, state);
    let sides = fluids::pipe_sides(
        previous,
        |d| {
            let o = d.offset();
            pipe_cell(game, [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]])
        },
        preferred.unwrap_or(Dir::North),
    );
    if sides == previous && preferred.is_none() {
        return;
    }
    let Some(mut s) = game.pack.assets.registry.state(state).cloned() else {
        return;
    };
    for d in Dir::ALL {
        let open = if sides & d.bit() != 0 {
            "true"
        } else {
            "false"
        };
        s.properties.insert(dir_name(d).to_owned(), open.to_owned());
    }
    if let Some(id) = game.pack.assets.registry.id(&s)
        && id != state
    {
        game.native_set_block(dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
    }
}

/// Pipes and pumps: pipes that changed connect again; turning pumps find their ends when pipes
/// changed (`distributePressureTo`) and carry fluid from the first end behind them to the
/// first end in front that takes it, `max(1, pressure / 2)` mB a tick (`FluidNetwork`).
pub(super) fn tick_pipes(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::fluids;
    use create_engine::tank::BUCKET;
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].pipe_checks)).unwrap_or_default();
    checks.sort();
    checks.dedup();
    for pos in checks {
        connect_pipe(game, dimension, pos, None);
    }
    let changed =
        with_world(|w| std::mem::replace(&mut w.dims[dim].pipes_changed, false)).unwrap_or(false);
    let pumps = with_world(|w| w.dims[dim].pumping.clone()).unwrap_or_default();
    for pos in pumps {
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let Some(facing) =
            with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten()
        else {
            continue;
        };
        let Some(mut pump) = with_world(|w| w.dims[dim].pumps.get(&pos).cloned()).flatten() else {
            continue;
        };
        if changed || !pump.found {
            pump.pull = fluids::reach(pos, facing.opposite(), speed, |p| pipe_cell(game, p));
            pump.push = fluids::reach(pos, facing, speed, |p| pipe_cell(game, p));
            pump.found = true;
        }
        if let (Some(from), false) = (pump.pull.first().copied(), pump.push.is_empty()) {
            let amount = fluids::transfer_speed(from.pressure);
            // What the source can give this tick.
            let drained_from_drain = from.tank
                && with_world(|w| w.dims[dim].drains.contains_key(&from.pos)).unwrap_or(false);
            let from_basin = from.tank && basin_at(game, from.pos);
            let offer = if from_basin {
                // A basin gives from its outputs.
                let mut t = basin_tanks(dim, from.pos);
                create_engine::basin::drain(&mut t.output, amount)
            } else if drained_from_drain {
                with_world(|w| {
                    w.dims[dim]
                        .drains
                        .get(&from.pos)
                        .and_then(|d| d.fluid.map(|(f, n)| (f, n.min(amount))))
                })
                .flatten()
            } else if from.tank {
                with_world(|w| {
                    w.dims[dim]
                        .tanks
                        .get(&from.pos)
                        .and_then(|t| t.fluid.map(|f| (f, t.amount.min(amount))))
                })
                .flatten()
            } else {
                if pump.buffer.is_none() {
                    // `OpenEndedPipe.removeFluidFromSpace`: a source block gives a bucket's worth.
                    let s = game.world.block(from.pos[0], from.pos[1], from.pos[2]);
                    let f = game.res.behaviours().fluid(s);
                    let kind = f.kind();
                    use horizon_world::fluid::FluidKind;
                    let fluid =
                        fluids::open_fluid(kind == FluidKind::Water, kind == FluidKind::Lava);
                    if let Some(fluid) = fluid.filter(|_| f.is_source()) {
                        pump.buffer = Some((fluid, BUCKET));
                        let name = if fluid == create_engine::tank::Fluid::Water {
                            "minecraft:water"
                        } else {
                            "minecraft:lava"
                        };
                        if let Some(left) =
                            state_with(game.pack.assets.registry, name, &[("level", "14")])
                        {
                            game.native_set_block(
                                dimension,
                                from.pos,
                                left,
                                super::native_hooks::flags::UPDATE_ALL,
                            );
                        }
                    }
                }
                pump.buffer.map(|(f, n)| (f, n.min(amount)))
            };
            if let Some((fluid, n)) = offer {
                // The first end in front that takes some.
                let mut moved = 0;
                for to in &pump.push {
                    moved = if to.tank && basin_at(game, to.pos) {
                        // A basin takes into its inputs.
                        let mut t = basin_tanks(dim, to.pos);
                        let took = create_engine::basin::fill(&mut t.input, fluid, n);
                        if took > 0 {
                            with_world(|w| w.set_basin_fluids(dim, to.pos, t));
                        }
                        took
                    } else if to.tank {
                        with_world(|w| {
                            let d = &mut w.dims[dim];
                            let took = match d.spouts.get_mut(&to.pos) {
                                Some(s) => create_engine::spout::fill(s, fluid, n),
                                None => d
                                    .tanks
                                    .get_mut(&to.pos)
                                    .map_or(0, |t| t.fill_amount(fluid, n)),
                            };
                            if took > 0 {
                                d.tank_changes.push(to.pos);
                            }
                            took
                        })
                        .unwrap_or(0)
                    } else {
                        // `OpenEndedPipe.provideFluidToSpace`: a bucket's worth becomes a source.
                        let have = pump.out.filter(|o| o.0 == fluid).map_or(0, |o| o.1);
                        let total = have + n;
                        if !fluid.placeable() {
                            // Create's own fluids have no blocks in the world yet.
                            0
                        } else if total >= BUCKET {
                            let name = if fluid == create_engine::tank::Fluid::Water {
                                "minecraft:water"
                            } else {
                                "minecraft:lava"
                            };
                            let here = game.world.block(to.pos[0], to.pos[1], to.pos[2]);
                            if !game.res.behaviours().is_replaceable(here) {
                                0
                            } else {
                                if let Some(src) =
                                    state_with(game.pack.assets.registry, name, &[("level", "0")])
                                {
                                    game.native_set_block(
                                        dimension,
                                        to.pos,
                                        src,
                                        super::native_hooks::flags::UPDATE_ALL,
                                    );
                                }
                                pump.out = (total > BUCKET).then_some((fluid, total - BUCKET));
                                n
                            }
                        } else {
                            pump.out = Some((fluid, total));
                            n
                        }
                    };
                    if moved > 0 {
                        break;
                    }
                }
                if moved > 0 {
                    if from_basin {
                        let mut t = basin_tanks(dim, from.pos);
                        create_engine::basin::drain(&mut t.output, moved);
                        with_world(|w| w.set_basin_fluids(dim, from.pos, t));
                    } else if drained_from_drain {
                        with_world(|w| {
                            if let Some(d) = w.dims[dim].drains.get_mut(&from.pos) {
                                create_engine::drain::drain(d, moved);
                            }
                        });
                    } else if from.tank {
                        with_world(|w| {
                            let d = &mut w.dims[dim];
                            if let Some(t) = d.tanks.get_mut(&from.pos) {
                                t.drain_amount(moved);
                                d.tank_changes.push(from.pos);
                            }
                        });
                    } else if let Some((f, left)) = pump.buffer {
                        pump.buffer = (left > moved).then_some((f, left - moved));
                    }
                    with_world(|w| w.mark_unsaved(dim, pos));
                }
            }
        }
        with_world(|w| {
            w.dims[dim].pumps.insert(pos, pump);
        });
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

/// A block entity slot as the processing rules see it.
pub(super) fn slot_of(
    s: &Option<horizon_client::items::ItemStack>,
) -> create_engine::processing::Slot {
    s.as_ref()
        .map(|s| (s.item.to_string(), s.count.min(255) as u8))
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

/// The belt segment at `pos`: its controller, its index along the chain, and its state.
pub(super) fn belt_segment(
    game: &Game<'_>,
    dim: usize,
    pos: [i32; 3],
) -> Option<([i32; 3], i32, create_engine::belt::Segment)> {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    let seg = with_world(|w| w.table.get(state).and_then(|(_, i)| i.belt)).flatten()?;
    let (c, index) = with_world(|w| {
        let d = &w.dims[dim];
        let c = *d.belt_of.get(&pos)?;
        let index = d.belts.get(&c)?.chain.iter().position(|p| *p == pos)? as i32;
        Some((c, index))
    })
    .flatten()?;
    Some((c, index, seg))
}

/// `BeltBlock.useItemOn` with an empty main hand: every item within reach of the segment's
/// centre goes to the player.
pub(super) fn use_belt(game: &mut Game<'_>, dim: usize, pos: [i32; 3], canceled: &mut bool) {
    if game.native_held().is_some_and(|h| h.0 != "minecraft:air")
        || game.hand != horizon_client::first_person::Hand::Main
        || game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft)
    {
        return;
    }
    let Some((c, index, _)) = belt_segment(game, dim, pos) else {
        return;
    };
    *canceled = true;
    let taken = with_world(|w| {
        let b = w.dims[dim].belts.get_mut(&c)?;
        let taken = b.items.take_within(index as f32 + 0.5, 0.55);
        if !taken.is_empty() {
            w.dims[dim].belt_changes.push(c);
        }
        Some(taken)
    })
    .flatten()
    .unwrap_or_default();
    for i in taken {
        game.native_give_items(&i.item, u32::from(i.count));
    }
}

/// `BeltBlock.updateEntityAfterFallOn` / `entityInside` for an item, and
/// `SharedDepotBlockMethods.onLanded`: a belt takes a landing stack onto its segment (when
/// nothing just put there is in the way), a depot takes it when it holds nothing.
pub(super) fn item_landed(
    game: &mut Game<'_>,
    pos: [i32; 3],
    stack: &mut horizon_client::items::ItemStack,
    at: [f64; 3],
) {
    // The entity's own block, else the one under it; neither a Create block: done.
    let own = [
        at[0].floor() as i32,
        at[1].floor() as i32,
        at[2].floor() as i32,
    ];
    let s_own = game.world.block(own[0], own[1], own[2]).0;
    let s_pos = game.world.block(pos[0], pos[1], pos[2]).0;
    if states::index(s_own).is_none() && states::index(s_pos).is_none() {
        return;
    }
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    let kind = |s: u32| with_world(|w| w.table.get(s).map(|(b, _)| b)).flatten();
    let (spot, block) = match (kind(s_own), kind(s_pos)) {
        (Some(b @ create_engine::Block::Belt), _) => (own, b),
        (_, Some(b @ create_engine::Block::Belt)) => (pos, b),
        (_, Some(b @ create_engine::Block::Depot)) => (pos, b),
        (Some(b @ create_engine::Block::Basin), _) => (own, b),
        (_, Some(b @ create_engine::Block::Chute)) => (pos, b),
        (_, Some(b @ create_engine::Block::MechanicalSaw)) => (pos, b),
        (_, Some(b @ create_engine::Block::ItemDrain)) => (pos, b),
        (
            _,
            Some(b @ (create_engine::Block::AndesiteFunnel | create_engine::Block::BrassFunnel)),
        ) => (pos, b),
        _ => return,
    };
    if stack.extra.is_some() {
        // Stacks with components keep them only as entities, for now.
        return;
    }
    if block == create_engine::Block::Basin {
        basin_take(game, dim, spot, stack);
        return;
    }
    if block == create_engine::Block::Chute {
        chute_take(game, dim, spot, stack);
        return;
    }
    if block == create_engine::Block::MechanicalSaw {
        saw_take(game, dim, spot, stack);
        return;
    }
    if matches!(
        block,
        create_engine::Block::AndesiteFunnel | create_engine::Block::BrassFunnel
    ) {
        // FunnelBlock.entityInside, inserting: into the inventory it is on, as much as fits.
        let state = game.world.block(spot[0], spot[1], spot[2]);
        let Some(s) = game.pack.assets.registry.state(state) else {
            return;
        };
        let extracting = s.properties.get("extracting").is_some_and(|v| v == "true");
        let powered = s.properties.get("powered").is_some_and(|v| v == "true");
        let facing = with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten();
        if extracting || powered {
            return;
        }
        let Some(facing) = facing else {
            return;
        };
        let o = facing.offset();
        let behind = [spot[0] - o[0], spot[1] - o[1], spot[2] - o[2]];
        let face = horizon_core::geometry::Direction::ALL[facing as usize];
        let left = game.native_insert_item(behind, face, stack.clone());
        stack.count = left.map_or(0, |l| l.count);
        return;
    }
    if block == create_engine::Block::ItemDrain {
        // ItemDrainBlockEntity's insertion: a stack it can hold, while it holds nothing.
        let took = with_world(|w| {
            let d = w.dims[dim].drains.entry(spot).or_default();
            if d.held.is_some() {
                return false;
            }
            d.held = Some((stack.item.to_string(), stack.count.min(64) as u8));
            true
        })
        .unwrap_or(false);
        if took {
            stack.count = stack.count.saturating_sub(64);
        }
        return;
    }
    let max = game
        .items()
        .get(&stack.item)
        .map_or(64, |i| i.max_stack)
        .clamp(1, 64);
    let count = stack.count.min(max) as u8;
    let up = upright(game, &stack.item);
    if block == create_engine::Block::Depot {
        // DirectBeltInputBehaviour from above: only into an empty depot.
        if !depot_items(game, spot).is_empty() {
            return;
        }
        let put = horizon_client::items::ItemStack {
            item: stack.item.clone(),
            count: u32::from(count),
            damage: 0,
            extra: None,
        };
        if game
            .native_with_mod_block_entity(spot, |be| {
                be.items
                    .resize(be.items.len().max(create_engine::depot::SLOTS), None);
                be.items[0] = Some(put);
            })
            .is_none()
        {
            return;
        }
        stack.count -= u32::from(count);
        let held = create_engine::depot::Held::new(random_angle(game, spot), up);
        with_world(|w| w.depot_put(dim, spot, Some(held)));
        return;
    }
    let Some((c, index, seg)) = belt_segment(game, dim, spot) else {
        return;
    };
    if !seg.transports() {
        return;
    }
    let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
    let angle = random_angle(game, spot);
    let added = with_world(|w| {
        let b = w.dims[dim].belts.get_mut(&c)?;
        let movement = create_engine::belt::movement_facing(rpm, b.segment.facing);
        // ItemHandlerBeltSegment.insertItem: canInsertAt(segment), from above.
        if !b
            .items
            .can_insert_at_from_side(index, create_engine::kinetics::Dir::Up, movement)
        {
            return None;
        }
        let mut item = create_engine::belt::Item::new(&stack.item, count, angle, up);
        item.inserted_at = index;
        item.inserted_from = Some(create_engine::kinetics::Dir::Up);
        item.position = index as f32 + 0.5 + if b.items.positive { -1.0 } else { 1.0 } / 16.0;
        b.items.add(item);
        w.dims[dim].belt_changes.push(c);
        Some(())
    })
    .flatten();
    if added.is_some() {
        stack.count -= u32::from(count);
    }
}

/// `DirectBeltInputBehaviour.handleInsertion` at `next`, the item moving toward `side`: a
/// depot or a belt there. What is left of it, None when nothing there takes items.
pub(super) fn insert_into(
    game: &mut Game<'_>,
    dim: usize,
    next: [i32; 3],
    side: create_engine::kinetics::Dir,
    item: &create_engine::belt::Item,
) -> Option<u8> {
    let is_depot = with_world(|w| w.dims[dim].depots.contains_key(&next)) == Some(true);
    if is_depot {
        // DepotBehaviour.tryInsertingFromSide: only into an empty depot.
        if !depot_items(game, next).is_empty() {
            return Some(item.count);
        }
        let put = horizon_client::items::ItemStack {
            item: item.item.as_str().into(),
            count: u32::from(item.count),
            damage: 0,
            extra: None,
        };
        game.native_with_mod_block_entity(next, |be| {
            be.items
                .resize(be.items.len().max(create_engine::depot::SLOTS), None);
            be.items[0] = Some(put);
        })?;
        let held = create_engine::depot::Held {
            position: 0.0,
            from: create_engine::depot::Side::from_dir(side),
            angle: item.angle,
            side_offset: item.side_offset,
            locked: false,
            fan: item.fan,
        };
        with_world(|w| w.depot_put(dim, next, Some(held)));
        return Some(0);
    }
    // BeltBlockEntity.tryInsertingFromSide on the next belt.
    let (c2, index, seg2) = belt_segment(game, dim, next)?;
    if !seg2.transports() {
        return None;
    }
    let rpm2 = with_world(|w| w.speed(dim, c2)).flatten().unwrap_or(0.0);
    let speed2 = create_engine::belt::direction_aware_speed(rpm2, seg2.facing);
    with_world(|w| {
        let b = w.dims[dim].belts.get_mut(&c2)?;
        let movement2 = create_engine::belt::movement_facing(rpm2, b.segment.facing);
        if rpm2 == 0.0
            || movement2 == side.opposite()
            || !b.items.can_insert_at_from_side(index, side, movement2)
        {
            return Some(item.count);
        }
        let mut moved = item.clone();
        moved.locked = false;
        moved.position = index as f32 + 0.5 - speed2.signum() / 16.0;
        if movement2 != side {
            moved.side_offset = if side.positive() { 0.675 } else { -0.675 };
            if side.axis() == create_engine::kinetics::Axis::X {
                moved.side_offset = -moved.side_offset;
            }
        } else {
            // A smoother hand-over from the belt behind it.
            let extra = 0.26;
            moved.position = if speed2 > 0.0 {
                index as f32 - extra
            } else {
                index as f32 + 1.0 + extra
            };
        }
        moved.inserted_at = index;
        moved.inserted_from = Some(side);
        b.items.add(moved);
        w.dims[dim].belt_changes.push(c2);
        Some(0)
    })
    .flatten()
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

/// `BeltInventory.eject`: an item thrown off the end, forward and a little up.
pub(super) fn eject_from_belt(
    game: &mut Game<'_>,
    controller: [i32; 3],
    seg: &create_engine::belt::Segment,
    length: i32,
    rpm: f32,
    item: &create_engine::belt::Item,
) {
    let at = create_engine::belt::vector_for_offset(controller, seg, length, item.position);
    let speed = create_engine::belt::movement_speed(rpm)
        .abs()
        .max(1.0 / 8.0);
    let d = create_engine::belt::movement_facing(rpm, seg.facing).offset();
    let motion = [
        f64::from(d[0]) * f64::from(speed),
        1.0 / 8.0,
        f64::from(d[2]) * f64::from(speed),
    ];
    game.native_run_command(&format!(
        "summon minecraft:item {:.4} {:.4} {:.4} {{Item:{{id:'{}',count:{}}},Motion:[{:.4}d,{:.4}d,{:.4}d]}}",
        at[0],
        at[1] + 6.0 / 16.0,
        at[2],
        item.item,
        item.count,
        motion[0],
        motion[1],
        motion[2]
    ));
}

/// Belts: segments broken next to a broken one become shafts or air, a broken belt's items
/// drop, then belts holding items move them (`BeltInventory.tick`); belts at rest or empty cost
/// nothing.
pub(super) fn tick_belts(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let (breaks, drops) = with_world(|w| {
        let d = &mut w.dims[dim];
        (
            std::mem::take(&mut d.belt_breaks),
            std::mem::take(&mut d.belt_drops),
        )
    })
    .unwrap_or_default();
    for b in breaks {
        let block = match b.shaft {
            Some(a) => format!(
                "create:shaft[axis={}]",
                ["x", "y", "z"][axis_index(a) as usize]
            ),
            None => "minecraft:air".to_owned(),
        };
        game.native_run_command(&format!(
            "execute in {dimension} run setblock {} {} {} {block}",
            b.pos[0], b.pos[1], b.pos[2]
        ));
    }
    for (c, seg, length, item) in drops {
        eject_from_belt(game, c, &seg, length, 0.0, &item);
    }
    let busy = with_world(|w| w.busy_belts(dim)).unwrap_or_default();
    if !busy.is_empty() {
        ensure_recipes(game);
    }
    for c in busy {
        let Some((seg, length, mut items)) = with_world(|w| {
            let b = w.dims[dim].belts.get_mut(&c)?;
            Some((
                b.segment,
                b.chain.len() as i32,
                std::mem::take(&mut b.items),
            ))
        })
        .flatten() else {
            continue;
        };
        let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
        let speed = create_engine::belt::direction_aware_speed(rpm, seg.facing);
        let mut side = BeltSide {
            game,
            dim,
            controller: c,
            segment: seg,
            length,
            rpm,
            positive: speed > 0.0,
        };
        let horizontal = seg.slope == create_engine::belt::Slope::Horizontal;
        let ticked = items.tick(length, speed, horizontal, &mut side);
        with_world(|w| {
            if let Some(b) = w.dims[dim].belts.get_mut(&c) {
                // Items another belt put here meanwhile go after ours.
                let added = std::mem::replace(&mut b.items, items);
                for i in added.items {
                    b.items.add(i);
                }
            }
            if ticked.changed {
                w.dims[dim].belt_changes.push(c);
            }
        });
        for item in &ticked.ejected {
            eject_from_belt(game, c, &seg, length, rpm, item);
        }
    }
    let changed = with_world(|w| {
        let mut v = std::mem::take(&mut w.dims[dim].belt_changes);
        v.sort();
        v.dedup();
        for c in &v {
            w.mark_unsaved(dim, *c);
        }
        v
    })
    .unwrap_or_default();
    if !changed.is_empty() {
        let players = players_in(game, dimension);
        send_belts(game, &players, dim, &changed);
    }
}

pub(super) fn send_belts(game: &mut Game<'_>, players: &[u128], dim: usize, belts: &[[i32; 3]]) {
    let all: Vec<create_engine::belt::Contents> = belts
        .iter()
        .map(|&c| {
            with_world(|w| w.dims[dim].belts.get(&c).cloned())
                .flatten()
                .map_or(
                    create_engine::belt::Contents {
                        controller: c,
                        segment: create_engine::belt::Segment {
                            facing: create_engine::kinetics::Dir::North,
                            slope: create_engine::belt::Slope::Horizontal,
                            part: create_engine::belt::Part::Start,
                        },
                        length: 0,
                        ending: create_engine::belt::Ending::Eject,
                        items: create_engine::belt::Inventory::default(),
                    },
                    |b| {
                        let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
                        let speed =
                            create_engine::belt::direction_aware_speed(rpm, b.segment.facing);
                        let mut side = BeltSide {
                            game,
                            dim,
                            controller: c,
                            segment: b.segment,
                            length: b.chain.len() as i32,
                            rpm,
                            positive: speed > 0.0,
                        };
                        let ending = create_engine::belt::Surroundings::ending(&mut side);
                        create_engine::belt::Contents {
                            controller: c,
                            segment: b.segment,
                            length: b.chain.len() as u16,
                            ending,
                            items: b.items,
                        }
                    },
                )
        })
        .collect();
    for m in create_engine::belt::encode(&all) {
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::belt::CHANNEL, &m);
        }
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

pub(super) fn is_block_item(game: &Game<'_>, item: &str) -> bool {
    if let Some(b) = BLOCK_ITEMS.with_borrow(|m| m.get(item).copied()) {
        return b;
    }
    let b = game.native_item_uses_block_light(item).unwrap_or(false);
    BLOCK_ITEMS.with_borrow_mut(|m| m.insert(item.to_owned(), b));
    b
}

/// A key for copy `copy` of item `index` on the belt at `controller`, apart from the others.
pub(super) fn belt_key(controller: [i32; 3], index: usize, copy: u8) -> u64 {
    depot_key(controller, 200, copy) ^ (index as u64 + 1).wrapping_mul(0xD6E8_FEB8_6659_FD93)
        | 1 << 63
}

/// The client's belt surroundings: what the server said lies past the end; machines hold
/// locked items, nothing else acts.
pub(super) struct ClientSide(create_engine::belt::Ending);

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

/// Draws the items of a belt: each copy a GPU item part gliding at its belt's speed; uploads
/// only what moves differently from before.
pub(super) fn draw_belt(
    game: &mut Game<'_>,
    contents: &create_engine::belt::Contents,
    shown: &mut Vec<Shown>,
    rpm: f32,
    redraw: bool,
) {
    use create_engine::belt;
    let c = contents.controller;
    let length = i32::from(contents.length);
    let speed = belt::direction_aware_speed(rpm, contents.segment.facing);
    let now = game.native_parts_seconds();
    if redraw {
        for (i, s) in shown.iter().enumerate() {
            for copy in 0..s.copies {
                game.native_part_clear(belt_key(c, i, copy));
            }
        }
        shown.clear();
    }
    for (i, item) in contents.items.items.iter().enumerate() {
        let place = belt::item_place(&contents.segment, length, item.position, item.side_offset);
        let moving = !item.locked && speed != 0.0;
        let velocity = if moving {
            belt::item_velocity(&contents.segment, length, item.position, speed).to_array()
        } else {
            [0.0; 3]
        };
        let flat = place.tilt == 0.0;
        // Moving as it was uploaded: the GPU already has it where it is.
        if let Some(s) = shown.get(i)
            && s.velocity == velocity
            && s.flat == flat
            && s.angle == item.angle
            && s.count == item.count
            && s.item == item.item
        {
            continue;
        }
        let block_item = is_block_item(game, &item.item);
        let copies = belt::item_copies(u32::from(item.count), block_item, item.angle);
        // Flat: baked about the block's centre, turned by the item's angle on the GPU; on a
        // slope the tilt comes first, so the angle is baked in.
        let (centre, phase) = if flat {
            (glam::Vec3::new(0.5, 0.0, 0.5), item.angle as f32)
        } else {
            (glam::Vec3::ZERO, 0.0)
        };
        let tilt = if flat {
            glam::Mat4::IDENTITY
        } else {
            let axis = if place.tilt_axis == create_engine::kinetics::Axis::Z {
                glam::Vec3::Z
            } else {
                glam::Vec3::X
            };
            glam::Mat4::from_axis_angle(axis, place.tilt.to_radians())
                * glam::Mat4::from_rotation_y((item.angle as f32).to_radians())
        };
        let old_copies = shown.get(i).map_or(0, |s| s.copies);
        for copy in copies.len() as u8..old_copies {
            game.native_part_clear(belt_key(c, i, copy));
        }
        for (copy, pose) in copies.iter().enumerate() {
            let Some(model) = game.native_part_item_model(
                &item.item,
                glam::Mat4::from_translation(centre) * tilt * *pose,
            ) else {
                continue;
            };
            let at = place.at - centre;
            game.native_part_glide(
                belt_key(c, i, copy as u8),
                model,
                c,
                1,
                0.0,
                phase,
                [at.x, at.y, at.z, now],
                velocity,
            );
        }
        let entry = Shown {
            item: item.item.clone(),
            count: item.count,
            angle: item.angle,
            flat,
            velocity,
            copies: copies.len() as u8,
        };
        if i < shown.len() {
            shown[i] = entry;
        } else {
            shown.push(entry);
        }
    }
}

pub(super) fn belts_from_server(game: &mut Game<'_>, data: &[u8]) {
    for contents in create_engine::belt::decode(data) {
        let c = contents.controller;
        let old = BELTS.with_borrow_mut(|b| b.belts.remove(&c));
        let mut shown = old.map(|o| o.1).unwrap_or_default();
        if contents.length == 0 {
            for (i, s) in shown.iter().enumerate() {
                for copy in 0..s.copies {
                    game.native_part_clear(belt_key(c, i, copy));
                }
            }
            continue;
        }
        let rpm = belt_rpm(c);
        draw_belt(game, &contents, &mut shown, rpm, true);
        BELTS.with_borrow_mut(|b| b.belts.insert(c, (contents, shown)));
    }
}

pub(super) fn belt_rpm(controller: [i32; 3]) -> f32 {
    CLIENT.with_borrow(|c| {
        c.as_ref()
            .and_then(|c| c.speeds.get(&controller))
            .map_or(0.0, |s| s[0])
    })
}

/// Each client tick: belts with items move them on (`BeltInventory.tick` on the client).
pub(super) fn tick_client_belts(game: &mut Game<'_>) {
    let busy: Vec<[i32; 3]> = BELTS.with_borrow(|b| {
        b.belts
            .iter()
            .filter(|(_, (c, _))| !c.items.items.is_empty())
            .map(|(k, _)| *k)
            .collect()
    });
    for c in busy {
        let Some((mut contents, mut shown)) = BELTS.with_borrow_mut(|b| b.belts.remove(&c)) else {
            continue;
        };
        let rpm = belt_rpm(c);
        let speed = create_engine::belt::direction_aware_speed(rpm, contents.segment.facing);
        let before = contents.items.items.len();
        let horizontal = contents.segment.slope == create_engine::belt::Slope::Horizontal;
        contents.items.tick(
            i32::from(contents.length),
            speed,
            horizontal,
            &mut ClientSide(contents.ending),
        );
        let redraw = contents.items.items.len() != before;
        draw_belt(game, &contents, &mut shown, rpm, redraw);
        BELTS.with_borrow_mut(|b| b.belts.insert(c, (contents, shown)));
    }
}

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

/// `isSignalSource`: a latch gives a signal, and a powered receiver.
pub(super) fn is_signal_source(state: BlockStateId, source: &mut bool) {
    if latch_state(state.0).is_some()
        || link_state(state.0).is_some_and(|(_, receiver, powered)| receiver && powered)
    {
        *source = true;
    }
}

/// Brass tunnels holding a stack (`BrassTunnelBlockEntity.tick`): two ticks after it arrives
/// (then every `brassTunnelTimer` while some is left), the stack is split among the outputs
/// beside and ahead of the segment: a belt or depot across each open side, the belt ahead.
pub(super) fn tick_brass(game: &mut Game<'_>, dim: usize) {
    use create_engine::tunnel;
    let Some(list) = with_world(|w| w.dims[dim].brass_running.clone()) else {
        return;
    };
    for t in list {
        let Some(mut b) = with_world(|w| w.dims[dim].brass.get(&t).cloned()).flatten() else {
            with_world(|w| w.dims[dim].brass_running.retain(|p| *p != t));
            continue;
        };
        let below = [t[0], t[1] - 1, t[2]];
        if b.progress > 0 {
            b.progress -= 1;
        }
        let belt = belt_segment(game, dim, below);
        let rpm = belt
            .as_ref()
            .and_then(|(c, ..)| with_world(|w| w.speed(dim, *c)).flatten())
            .unwrap_or(0.0);
        let save = |b: &tunnel::Brass| {
            with_world(|w| {
                let d = &mut w.dims[dim];
                if let Some(x) = d.brass.get_mut(&t) {
                    *x = b.clone();
                }
                if b.stack.is_none() {
                    d.brass_running.retain(|p| *p != t);
                }
            });
        };
        let (Some((_, _, seg)), Some((item, count))) = (belt, b.stack.clone()) else {
            save(&b);
            continue;
        };
        if rpm == 0.0 {
            save(&b);
            continue;
        }
        let movement = create_engine::belt::movement_facing(rpm, seg.facing);
        let Some((axis, shape)) = tunnel_state(game.world.block(t[0], t[1], t[2]).0) else {
            continue;
        };
        // `addValidOutputsOf`: the sides across first, then ahead; never back.
        let open = tunnel::sides(axis, shape);
        let order = [Dir::South, Dir::West, Dir::North, Dir::East];
        let outputs: Vec<Dir> = order
            .iter()
            .filter(|d| d.axis() != movement.axis())
            .chain(order.iter().filter(|d| **d == movement))
            .copied()
            .filter(|d| open.contains(d) && Some(*d) != b.entered_from)
            .filter(|d| {
                let o = d.offset();
                let p = [below[0] + o[0], below[1], below[2] + o[2]];
                with_world(|w| {
                    w.dims[dim].depots.contains_key(&p) || w.dims[dim].belt_of.contains_key(&p)
                }) == Some(true)
            })
            .collect();
        if b.progress == -1 {
            if outputs.is_empty() {
                save(&b);
                continue;
            }
            if b.new_item {
                b.new_item = false;
                b.progress = 2;
            } else {
                b.progress = tunnel::BRASS_TIMER;
                save(&b);
                continue;
            }
        }
        if b.progress != 0 {
            save(&b);
            continue;
        }
        // `insertIntoTunnel`: a depot takes a stack while empty, a belt one it has room for.
        let left = tunnel::distribute(
            count,
            outputs.len(),
            b.previous_output,
            b.mode,
            |i, n, simulate| {
                let d = outputs[i];
                let o = d.offset();
                let p = [below[0] + o[0], below[1], below[2] + o[2]];
                if simulate {
                    let depot = with_world(|w| w.dims[dim].depots.contains_key(&p)) == Some(true);
                    if depot && !depot_items(game, p).is_empty() {
                        return Some(n);
                    }
                    return Some(0);
                }
                insert_into(
                    game,
                    dim,
                    p,
                    d,
                    &create_engine::belt::Item::new(&item, n, 180, false),
                )
            },
        );
        b.stack = (left > 0).then(|| (item.clone(), left));
        if b.stack.is_none() {
            b.entered_from = None;
        }
        b.previous_output = (b.previous_output + 1) % outputs.len().max(1);
        b.progress = -1;
        save(&b);
        with_world(|w| w.mark_unsaved(dim, t));
    }
}

/// An andesite tunnel's axis and shape; None for any other block.
pub(super) fn tunnel_state(
    state: u32,
) -> Option<(create_engine::kinetics::Axis, create_engine::tunnel::Shape)> {
    states::index(state)?;
    WORLD.with(|w| {
        let w = w.try_borrow().ok()?;
        let (block, info) = w.as_ref()?.table.get(state)?;
        matches!(
            block,
            create_engine::Block::AndesiteTunnel | create_engine::Block::BrassTunnel
        )
        .then_some((info.axis?, info.tunnel?))
    })
}

/// Andesite tunnels: those just placed case their belt (`BeltTunnelItem`); those whose
/// surroundings changed break without a cased, flat belt below (`canSurvive`) or take the shape
/// the belts and depots beside their segment give (`updateShape`, `getTunnelState`).
pub(super) fn tick_tunnels(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::belt::Slope;
    use create_engine::kinetics::{Axis, Dir};
    let Some((new, kept, fresh)) = with_world(|w| {
        let d = &mut w.dims[dim];
        (
            std::mem::take(&mut d.tunnels_new),
            std::mem::take(&mut d.tunnel_checks),
            std::mem::take(&mut d.tunnel_fresh),
        )
    }) else {
        return;
    };
    if new.is_empty() && kept.is_empty() && fresh.is_empty() {
        return;
    }
    let belt_below = |game: &Game<'_>, p: [i32; 3]| {
        let state = game.world.block(p[0], p[1] - 1, p[2]).0;
        with_world(|w| w.table.get(state).and_then(|(_, i)| i.belt)).flatten()
    };
    let cased = |game: &Game<'_>, p: [i32; 3]| {
        game.pack
            .assets
            .registry
            .state(game.world.block(p[0], p[1] - 1, p[2]))
            .is_some_and(|s| s.properties.get("casing").is_some_and(|v| v == "true"))
    };
    for pos in new {
        if tunnel_state(game.world.block(pos[0], pos[1], pos[2]).0).is_none() {
            continue;
        }
        if belt_below(game, pos).is_some_and(|s| s.slope == Slope::Horizontal) && !cased(game, pos)
        {
            game.native_set_block_property_with(
                [pos[0], pos[1] - 1, pos[2]],
                "casing",
                "true",
                super::native_hooks::flags::UPDATE_ALL,
            );
        }
    }
    let mut checks: Vec<([i32; 3], bool)> = fresh
        .into_iter()
        .map(|p| (p, true))
        .chain(kept.into_iter().map(|p| (p, false)))
        .collect();
    // A fresh look wins over a kept one for the same tunnel.
    checks.sort_by_key(|c| (c.0, !c.1));
    checks.dedup_by_key(|c| c.0);
    for (pos, at_once) in checks {
        let Some(now) = tunnel_state(game.world.block(pos[0], pos[1], pos[2]).0) else {
            continue;
        };
        let seg = belt_below(game, pos).filter(|s| s.slope == Slope::Horizontal);
        let Some(seg) = seg.filter(|_| cased(game, pos)) else {
            game.native_run_command(&format!(
                "execute in {dimension} run setblock {} {} {} air destroy",
                pos[0], pos[1], pos[2]
            ));
            continue;
        };
        let axis = seg.facing.axis();
        // `hasValidOutput` beside the belt segment: a belt across it, or a depot.
        let output = |game: &Game<'_>, side: Dir| {
            let o = side.offset();
            let p = [pos[0] + o[0], pos[1] - 1, pos[2] + o[2]];
            let state = game.world.block(p[0], p[1], p[2]).0;
            with_world(|w| match w.table.get(state) {
                Some((create_engine::Block::Depot, _)) => true,
                Some((_, i)) => i.belt.is_some_and(|b| b.facing.axis() == side.axis()),
                None => false,
            })
            .unwrap_or(false)
        };
        let left = create_engine::tunnel::left(axis);
        let (on_left, on_right) = (output(game, left), output(game, left.opposite()));
        // `canHaveWindow`: tunnels on both ends.
        let fw = if axis == Axis::X {
            Dir::East
        } else {
            Dir::South
        };
        let window = [fw, fw.opposite()].into_iter().all(|d| {
            let o = d.offset();
            tunnel_state(game.world.block(pos[0] + o[0], pos[1], pos[2] + o[2]).0).is_some()
        });
        let fresh = (
            axis,
            create_engine::tunnel::shape_for(on_left, on_right, window),
        );
        let (axis2, shape2) = if at_once {
            fresh
        } else {
            create_engine::tunnel::updated(now, fresh)
        };
        if (axis2, shape2) == now {
            continue;
        }
        let registry = game.pack.assets.registry;
        let current = game.world.block(pos[0], pos[1], pos[2]);
        let Some(mut s) = registry.state(current).cloned() else {
            continue;
        };
        let axis_name = if axis2 == Axis::X { "x" } else { "z" };
        s.properties.insert("axis".into(), axis_name.into());
        s.properties.insert("shape".into(), shape2.name().into());
        if let Some(id) = registry.id(&s) {
            game.native_set_block(dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
        }
    }
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

/// A latch's scheduled tick is the glue's: the repeater it is built on doesn't run its own.
pub(super) fn tick_block(state: BlockStateId, handled: &mut bool) {
    if latch_state(state.0).is_some() {
        *handled = true;
    }
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

pub(super) fn tanks_from_nbt(
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<([i32; 3], create_engine::tank::Tank)> {
    let Some(horizon_world::nbt::Tag::IntArray(v)) = data.and_then(|d| d.get(&"tanks".into()))
    else {
        return Vec::new();
    };
    v.chunks_exact(5)
        .map(|c| {
            (
                [c[0], c[1], c[2]],
                create_engine::tank::Tank {
                    fluid: create_engine::tank::Fluid::from_index(c[3].clamp(0, 255) as u8),
                    amount: c[4].max(0) as u32,
                },
            )
        })
        .collect()
}

/// `FluidTankBlock.useItemOn` for a creative player (Create 6 leaves survival to spouts and
/// drains): a filled bucket empties into the tank, an empty one takes a bucket's worth (and
/// is given, filled, unless the player has one already), each only whole.
pub(super) fn use_tank(game: &mut Game<'_>, dim: usize, pos: [i32; 3], canceled: &mut bool) {
    use create_engine::tank::Fluid;
    if !game.creative {
        return;
    }
    let Some((held, _, _)) = game.native_held() else {
        return;
    };
    let Some(mut t) = with_world(|w| w.dims[dim].tanks.get(&pos).copied()).flatten() else {
        return;
    };
    let changed = if let Some(f) = Fluid::of_bucket(&held) {
        t.fill(f)
    } else if held == "minecraft:bucket" {
        match t.drain() {
            Some(f) => {
                if game.native_count_items(f.bucket()) == 0 {
                    game.native_give_items(f.bucket(), 1);
                }
                true
            }
            None => false,
        }
    } else {
        return;
    };
    *canceled = true;
    if changed {
        with_world(|w| {
            w.dims[dim].tanks.insert(pos, t);
            w.dims[dim].tank_changes.push(pos);
            w.mark_unsaved(dim, pos);
        });
    }
}

pub(super) fn send_tank_changes(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let changed = with_world(|w| {
        let mut v = std::mem::take(&mut w.dims[dim].tank_changes);
        v.sort();
        v.dedup();
        v
    })
    .unwrap_or_default();
    if changed.is_empty() {
        return;
    }
    let players = players_in(game, dimension);
    send_tanks(game, &players, dim, &changed);
}

pub(super) fn send_tanks(game: &mut Game<'_>, players: &[u128], dim: usize, tanks: &[[i32; 3]]) {
    let all: Vec<([i32; 3], create_engine::tank::Tank)> = tanks
        .iter()
        .map(|&p| {
            (
                p,
                with_world(|w| w.dims[dim].tanks.get(&p).copied())
                    .flatten()
                    .unwrap_or_default(),
            )
        })
        .collect();
    for m in create_engine::tank::encode(&all) {
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::tank::CHANNEL, &m);
        }
    }
}

/// A tank's fluid on the client: `FluidTankRenderer`'s box, the fluid's still texture tinted
/// (water by its default colour), one part a tank, uploaded when its contents change.
pub(super) fn tanks_from_server(game: &mut Game<'_>, data: &[u8]) {
    use create_engine::tank::Fluid;
    let registry = game.pack.assets.registry;
    for (pos, t) in create_engine::tank::decode(data) {
        let key = depot_key(pos, 210, 0);
        let Some((min, max)) = create_engine::tank::fluid_box(&t) else {
            game.native_part_clear(key);
            continue;
        };
        let (name, tint) = match t.fluid {
            Some(Fluid::Water) => ("create:part_fluid_water", Some(0x3F_76E4)),
            Some(Fluid::Honey) => ("create:part_fluid_honey", None),
            Some(Fluid::Chocolate) => ("create:part_fluid_chocolate", None),
            Some(Fluid::Milk) => ("create:part_fluid_milk", None),
            _ => ("create:part_fluid_lava", None),
        };
        let Some(state) = registry.default_state(name).and_then(|d| {
            let mut s = registry.state(d)?.clone();
            s.properties.insert("facing".into(), "up".into());
            registry.id(&s)
        }) else {
            continue;
        };
        let pose = glam::Mat4::from_translation(glam::Vec3::from_array(min))
            * glam::Mat4::from_scale(glam::Vec3::from_array(max) - glam::Vec3::from_array(min));
        if let Some(model) = game.native_part_model_tinted(state, pose, tint) {
            game.native_part_set(key, model, pos, 1, 0.0, 0.0);
        }
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
