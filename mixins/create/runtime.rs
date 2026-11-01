// Game glue for the Create port, injected as `crate::game::create`. Rules live in the
// `create_engine` crate; this module only turns the game's hooks into engine calls.
// State lives on the server thread that fires the hooks (one per world).

use super::Game;
use super::native_lifecycle::WorldPhase;
use create_engine::client::{self, Model, Update};
use create_engine::kinetics::{Axis, Dir, Event};
use create_engine::states;
use create_engine::world::{CreateWorld, StateTable, dimension_index};
use horizon_world::registry::{BlockRegistry, BlockStateId};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static WORLD: RefCell<Option<CreateWorld>> = const { RefCell::new(None) };
}

/// Builds the state table from the registry once (the content mod's states never change while the
/// game runs). Returns false when Create's content isn't installed: every hook then stays a miss.
fn ensure(registry: &BlockRegistry) -> bool {
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

fn with_world<R>(f: impl FnOnce(&mut CreateWorld) -> R) -> Option<R> {
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

fn level_index(dimension: horizon_world::level::Dimension) -> usize {
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

/// After a server level's tick: re-solve what changed there (nothing when nothing did), break
/// the blocks Create would break, and tell the level's players what turns now.
pub(super) fn level_tick_post(game: &mut Game<'_>, dimension: &str) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
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

fn send(game: &mut Game<'_>, players: &[u128], updates: &[Update]) {
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
const TAKEN_WINDOW: i64 = 10;

thread_local! {
    /// The player and game time of the last click a Create block took: for the item use and
    /// the off hand's click that follow it, each taken once.
    static TAKEN_USE: std::cell::Cell<Option<(u128, i64)>> = const { std::cell::Cell::new(None) };
    static TAKEN_OFF_HAND: std::cell::Cell<Option<(u128, i64)>> = const { std::cell::Cell::new(None) };
}

/// The item use that follows a click a Create block took (the same tick, or the next).
pub(super) fn use_item(game: &mut Game<'_>, claimed: &mut bool) {
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

fn right_click_inner(game: &mut Game<'_>, pos: [i32; 3], face: [i32; 3], canceled: &mut bool) {
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
const SCROLL: &str = "create:scroll";

fn scroll_from_client(game: &mut Game<'_>, player: u128, data: &[u8]) {
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
    with_world(|w| w.scroll_motor(dim, [x, y, z], steps.clamp(-64, 64)));
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
fn value_tip(game: &mut Game<'_>, batch: &mut horizon_client::hud::HudBatch) {
    // Free when unused: no ray while no kinetic block has a speed here.
    if CLIENT.with_borrow(|c| c.as_ref().is_none_or(|c| c.speeds.is_empty())) {
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
    let Some((facing, speed)) = CLIENT.with_borrow(|c| {
        let c = c.as_ref()?;
        let (block, info) = c.table.get(state.0)?;
        (block == create_engine::Block::CreativeMotor)
            .then(|| (info.facing, c.speeds.get(&pos).map_or(0.0, |s| s[0])))
    }) else {
        return;
    };
    // The setting, from the speed the motor gives along its axis.
    let setting = facing.map_or(speed, |f| {
        create_engine::kinetics::convert_to_direction(speed, f)
    });
    let lines = [
        ("Generated Speed in RPM".to_owned(), 0xFB_DC7D),
        (format!("{setting:.0}"), 0xFF_FFFF),
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
fn goggle_overlay(game: &mut Game<'_>, batch: &mut horizon_client::hud::HudBatch) {
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
    let is_motor = CLIENT.with_borrow(|c| {
        c.as_ref()
            .and_then(|c| c.table.get(state.0))
            .is_some_and(|(b, _)| b == create_engine::Block::CreativeMotor)
    });
    if !is_motor {
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
fn settings_from_nbt(data: Option<&horizon_world::nbt::Compound>) -> Vec<([i32; 3], i32)> {
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
fn belts_to_nbt(belts: &[([i32; 3], create_engine::belt::Inventory)]) -> horizon_world::nbt::Tag {
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
fn links_to_nbt(links: &[([i32; 3], create_engine::link::Key, i32)]) -> horizon_world::nbt::Tag {
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
fn deployers_to_nbt(
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

fn deployers_from_nbt(
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

fn links_from_nbt(
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

fn crushers_to_nbt(
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

fn crushers_from_nbt(
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

fn burners_from_nbt(
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

fn belts_from_nbt(
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
    if settings.is_empty()
        && belts.is_empty()
        && tanks.is_empty()
        && windmills.is_empty()
        && crushers.is_empty()
        && burners.is_empty()
        && links.is_empty()
        && deployers.is_empty()
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
struct ClientParts {
    table: StateTable,
    speeds: HashMap<[i32; 3], [f32; 2]>,
    models: HashMap<(u32, Model), u32>,
    /// Speeds that came before their block did: tried again for a few ticks.
    pending: Vec<([i32; 3], u8)>,
    synced: bool,
}

thread_local! {
    static CLIENT: RefCell<Option<ClientParts>> = const { RefCell::new(None) };
}

fn axis_index(a: Axis) -> u8 {
    match a {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    }
}

fn dir_name(d: Dir) -> &'static str {
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
fn model_state(registry: &BlockRegistry, own: BlockStateId, model: Model) -> Option<BlockStateId> {
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
fn part_model(
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
fn show(game: &mut Game<'_>, c: &mut ClientParts, pos: [i32; 3]) -> bool {
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

fn hide(game: &mut Game<'_>, pos: [i32; 3]) {
    for slot in 0..client::SLOTS {
        game.native_part_clear(client::key(pos, slot));
    }
}

fn client_ensure(game: &Game<'_>) -> bool {
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
struct Recipes {
    /// Where the game's list of milling recipes was, and its length, when these were read:
    /// `/reload` builds a new table.
    table: (usize, usize),
    milling: create_engine::processing::Book,
    pressing: create_engine::processing::Book,
    crushing: create_engine::processing::Book,
    /// Create's cutting recipes (the saw).
    cutting: create_engine::processing::Book,
    /// Create's mixing recipes made of items only (fluids are gap 5).
    mixing: Vec<create_engine::basin::Recipe>,
    /// Create's compacting recipes (a press over a basin), items only.
    compacting: Vec<create_engine::basin::Recipe>,
    /// The fan's: Create's washing and haunting, vanilla's cooking.
    splashing: create_engine::processing::Book,
    haunting: create_engine::processing::Book,
    smoking: create_engine::processing::Book,
    smelting: create_engine::processing::Book,
    blasting: create_engine::processing::Book,
    /// The deployer's: `create:deploying`, then `create:item_application` (automatable ones).
    deploying: Vec<create_engine::deployer::Recipe>,
}

impl Recipes {
    fn fan(&self) -> create_engine::fan::Books<'_> {
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
fn ensure_recipes(game: &Game<'_>) -> bool {
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
        })
    });
    true
}

thread_local! {
    static RECIPES: RefCell<Option<Recipes>> = const { RefCell::new(None) };
}

/// A vanilla cooking recipe type (`minecraft:smelting`) as a book: its one ingredient, its one
/// result. These are the game's own table (vanilla's, the content mods' and the world's packs'),
/// in file order; `native_recipes_of_type` serves only mods' own types.
fn read_cooking(game: &Game<'_>, kind: &str) -> create_engine::processing::Book {
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
/// `ItemApplicationRecipe`s: the item worked on, the one held, the results; `_manual_only`
/// ones left out (`CAN_BE_AUTOMATED`).
fn read_application(game: &Game<'_>, kind: &str) -> Vec<create_engine::deployer::Recipe> {
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

fn read_basin(game: &Game<'_>, kind: &str) -> Vec<create_engine::basin::Recipe> {
    use create_engine::basin::{HeatReq, Recipe};
    use create_engine::processing::Output;
    game.native_recipes_of_type(kind)
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
                .map(|o| {
                    if o.get("amount").is_some() {
                        return None;
                    }
                    let stack = game.native_recipe_result(o)?;
                    Some(Output {
                        item: stack.item.to_string(),
                        count: stack.count.min(255) as u8,
                        chance: o["chance"].as_f64().unwrap_or(1.0) as f32,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Recipe {
                id: r.id.clone(),
                ingredients,
                results,
                time: json["processing_time"].as_i64().unwrap_or(0) as i32,
                heat: HeatReq::read(json["heat_requirement"].as_str()),
            })
        })
        .collect()
}

fn read_book(game: &Game<'_>, kind: &str) -> create_engine::processing::Book {
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

/// The millstones that turn: Create's `MillstoneBlockEntity.tick` on their slots (input 0,
/// outputs 1 to 9). Millstones that don't turn cost nothing.
fn tick_millstones(game: &mut Game<'_>, dim: usize) {
    let turning = with_world(|w| w.turning_millstones(dim)).unwrap_or_default();
    if turning.is_empty() {
        return;
    }
    if !ensure_recipes(game) {
        return;
    }
    let seed = game.world.game_time as u64;
    for (pos, speed) in turning {
        let Some(mut state) = with_world(|w| w.dims[dim].millstones.get(&pos).copied()).flatten()
        else {
            continue;
        };
        let max = |item: &str| {
            game.items()
                .get(item)
                .map_or(64, |i| i.max_stack)
                .clamp(1, 64) as u8
        };
        // Stack limits of what the slots hold, read before the block entity is borrowed.
        let limits: Vec<u8> = game
            .world
            .block_entities
            .get(&(pos[0], pos[1], pos[2]))
            .and_then(|b| match b {
                horizon_client::containers::BlockEntity::Mod(m) => Some(
                    m.items
                        .iter()
                        .map(|s| s.as_ref().map_or(64, |s| max(&s.item)))
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_default();
        if limits.len() < 10 {
            continue;
        }
        let result_limits: Vec<(String, u8)> = RECIPES.with_borrow(|r| {
            r.as_ref().map_or_else(Vec::new, |r| {
                r.milling
                    .recipes
                    .iter()
                    .flat_map(|x| x.results.iter())
                    .map(|o| (o.item.clone(), max(&o.item)))
                    .collect()
            })
        });
        game.native_with_mod_block_entity(pos, |be| {
            if be.items.len() < 10 {
                return;
            }
            let input = be.items[0].as_ref().map(|s| s.item.to_string());
            let outputs_full = be.items[1..10]
                .iter()
                .zip(&limits[1..10])
                .any(|(s, l)| s.as_ref().is_some_and(|s| s.count >= u32::from(*l)));
            RECIPES.with_borrow(|r| {
                let Some(r) = r else { return };
                let Some(done) = create_engine::processing::tick_millstone(
                    &mut state,
                    speed,
                    input.as_deref(),
                    outputs_full,
                    &r.milling,
                ) else {
                    return;
                };
                // process(): one input, the rolled results stacked into the outputs.
                if let Some(s) = be.items[0].as_mut() {
                    s.count -= 1;
                    if s.count == 0 {
                        be.items[0] = None;
                    }
                }
                let mut rng = create_engine::processing::Rng::new(
                    seed ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9) ^ (pos[2] as u64) << 20,
                );
                let mut slots: Vec<create_engine::processing::Slot> = be.items[1..10]
                    .iter()
                    .map(|s| {
                        s.as_ref()
                            .map(|s| (s.item.to_string(), s.count.min(255) as u8))
                    })
                    .collect();
                for (item, n) in create_engine::processing::roll(r.milling.get(done), &mut rng) {
                    let limit = result_limits
                        .iter()
                        .find(|(i, _)| *i == item)
                        .map_or(64, |(_, l)| *l);
                    create_engine::processing::insert_stacked(&mut slots, &item, n, limit);
                }
                for (slot, s) in be.items[1..10].iter_mut().zip(slots) {
                    *slot = s.map(|(item, count)| horizon_client::items::ItemStack {
                        item: item.as_str().into(),
                        count: u32::from(count),
                        damage: 0,
                        extra: None,
                    });
                }
            });
        });
        with_world(|w| {
            if let Some(m) = w.dims[dim].millstones.get_mut(&pos) {
                *m = state;
            }
        });
    }
}

/// `MillstoneBlock`'s block entity: its id, saved like any block entity with its slots.
pub(super) fn block_entity(
    block: &str,
    entity: &mut Option<(String, horizon_world::nbt::Compound)>,
) {
    if entity.is_none()
        && matches!(
            block,
            "create:millstone" | "create:depot" | "create:basin" | "create:chute"
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

/// `MillstoneBlock.useItemOn` with an empty hand: the outputs go to the player (the input when
/// there are none); with an item, the block's own use (closed: no menu opens).
fn use_millstone(game: &mut Game<'_>, pos: [i32; 3], canceled: &mut bool) {
    *canceled = true;
    if game.native_held().is_some_and(|h| h.0 != "minecraft:air") {
        return;
    }
    let taken = game
        .native_with_mod_block_entity(pos, |be| {
            let outputs: Vec<_> = be
                .items
                .iter_mut()
                .skip(1)
                .filter_map(Option::take)
                .collect();
            if outputs.is_empty() {
                be.items
                    .first_mut()
                    .and_then(Option::take)
                    .into_iter()
                    .collect()
            } else {
                outputs
            }
        })
        .unwrap_or_default();
    for s in taken {
        game.native_give_items(&s.item, s.count);
    }
}

/// `WaterWheelBlockEntity.getFlowVectorAtPosition`: the fluid's flow, or a bubble column's
/// push (up, or down when it drags down).
fn flow_at(game: &Game<'_>, dimension: &str, at: [i32; 3]) -> [f64; 3] {
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
fn upright(game: &Game<'_>, item: &str) -> bool {
    let tags = &game.items().item_tags;
    let has = |t: &str| tags.get(t).is_some_and(|m| m.contains(item));
    has("create:upright_on_belt") && !has("create:not_upright_on_belt")
}

/// `TransportedItemStack`'s `R.nextInt(360)`: Create's shared random; any angle will do.
fn random_angle(game: &Game<'_>, pos: [i32; 3]) -> i32 {
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
fn use_depot(game: &mut Game<'_>, dim: usize, pos: [i32; 3], face: [i32; 3], canceled: &mut bool) {
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
fn tick_depots(game: &mut Game<'_>, dimension: &str, dim: usize) {
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
fn depot_items(game: &Game<'_>, pos: [i32; 3]) -> Vec<(u8, String, u8)> {
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

fn send_depots(game: &mut Game<'_>, players: &[u128], dim: usize, depots: &[[i32; 3]]) {
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
struct ClientDepots {
    shown: HashMap<[i32; 3], create_engine::depot::Contents>,
    sliding: Vec<[i32; 3]>,
}

thread_local! {
    static DEPOTS: RefCell<ClientDepots> = RefCell::new(ClientDepots::default());
}

/// A key for copy `copy` of the stack in `slot` of the depot at `pos`, apart from the kinetic
/// parts' keys (a well-mixed hash of the three).
fn depot_key(pos: [i32; 3], slot: u8, copy: u8) -> u64 {
    let mut x = client::key(pos, 0)
        ^ (u64::from(slot) << 8 | u64::from(copy)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31) | 1 << 63
}

const COPIES: u8 = 4;

fn hide_depot(game: &mut Game<'_>, pos: [i32; 3]) {
    for slot in 0..create_engine::depot::SLOTS as u8 {
        for copy in 0..COPIES {
            game.native_part_clear(depot_key(pos, slot, copy));
        }
    }
}

/// Draws a depot's stacks as Create's `DepotRenderer` does: the held stack at its offset, turned
/// by its angle; each output 0.35 out at 45° steps. `next` is where the held stack will be a
/// tick from now, for the GPU to glide it there.
fn show_depot(
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
fn depots_from_server(game: &mut Game<'_>, data: &[u8]) {
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
fn tick_client_depots(game: &mut Game<'_>) {
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
fn blocked(game: &Game<'_>, at: [i32; 3]) -> bool {
    let state = game.world.block(at[0], at[1], at[2]);
    game.res.behaviours().facts(state).flags
        & horizon_world::level::data::state_flags::COLLISION_EMPTY
        == 0
}

/// The players of `dimension`.
fn players_in(game: &Game<'_>, dimension: &str) -> Vec<u128> {
    game.native_server_players()
        .into_iter()
        .filter(|p| p.dimension == dimension)
        .map(|p| p.uuid)
        .collect()
}

/// The deployer two above `depot`, facing down, and its speed; None for anything else.
fn deployer_above(
    game: &Game<'_>,
    dim: usize,
    depot: [i32; 3],
) -> Option<(create_engine::deployer::Deployer, f32)> {
    let above = [depot[0], depot[1] + 2, depot[2]];
    let state = game.world.block(above[0], above[1], above[2]).0;
    states::index(state)?;
    with_world(|w| {
        let (block, info) = w.table.get(state)?;
        if block != create_engine::Block::Deployer || info.facing != Some(Dir::Down) {
            return None;
        }
        let d = w.dims[dim].deployers.get(&above)?.clone();
        Some((d, w.speed(dim, above).unwrap_or(0.0)))
    })
    .flatten()
}

/// `BeltDeployerCallbacks` for the depot at `depot`: true when a deployer above holds its stack
/// (the depot asks again next tick).
fn deployer_on_depot(game: &mut Game<'_>, dim: usize, depot: [i32; 3]) -> bool {
    use create_engine::deployer::{self, Depot};
    let Some((mut d, rpm)) = deployer_above(game, dim, depot) else {
        return false;
    };
    let above = [depot[0], depot[1] + 2, depot[2]];
    let Some(mut held) =
        with_world(|w| w.dims[dim].depots.get(&depot).copied().flatten()).flatten()
    else {
        return false;
    };
    if held.sliding() && held.position < 0.5 - 1.0 / 16.0 {
        return false;
    }
    let Some((_, item, count)) = depot_items(game, depot).into_iter().find(|s| s.0 == 0) else {
        return false;
    };
    if !ensure_recipes(game) {
        return false;
    }
    let tool = d.held.as_ref().map(|h| h.0.clone()).unwrap_or_default();
    let recipe = RECIPES.with_borrow(|r| {
        r.as_ref()
            .and_then(|r| deployer::find(&r.deploying, &item, &tool).cloned())
    });
    let step = deployer::on_depot(&d, rpm, held.locked, recipe.is_some());
    let hold = match step {
        Depot::Pass => {
            held.locked = false;
            false
        }
        Depot::Hold => {
            held.locked = true;
            true
        }
        Depot::Start => {
            // The ray from the deployer's face to the block two ahead (`start`).
            let state = game.world.block(depot[0], depot[1], depot[2]);
            let top = game
                .res
                .facts()
                .get(state)
                .outline
                .iter()
                .map(|b| b[4])
                .fold(0.0, f64::max);
            d.start((1.0 - top) as f32);
            held.locked = true;
            with_world(|w| {
                let dd = &mut w.dims[dim];
                dd.deployers.insert(above, d.clone());
                if !dd.deploying.contains(&above) {
                    dd.deploying.push(above);
                }
                dd.deployer_changes.push(above);
            });
            true
        }
        Depot::Apply => {
            let Some(r) = recipe else {
                return false;
            };
            // `RecipeApplier.applyRecipeOn` on one of the stack, `getMaxOutputCount` 4.
            let seed = game.world.game_time as u64 ^ (depot[0] as u64).wrapping_mul(0x9E37_79B9);
            let p = create_engine::processing::Recipe {
                id: r.id.clone(),
                ingredients: Vec::new(),
                results: r.results.clone(),
                time: 0,
            };
            let results: Vec<(String, u8)> =
                create_engine::processing::roll(&p, &mut create_engine::processing::Rng::new(seed))
                    .into_iter()
                    .take(4)
                    .collect();
            held = results_on_depot(game, dim, depot, count, held, results);
            held.locked = true;
            deployer::use_held(&mut d.held, &r);
            with_world(|w| w.set_deployer(dim, above, d.clone()));
            true
        }
    };
    with_world(|w| {
        if let Some(h) = w.dims[dim].depots.get_mut(&depot) {
            *h = h.map(|_| held);
        }
    });
    hold
}

/// Deployers mid-stroke (`DeployerBlockEntity.tick` without a fake player): the stroke runs;
/// one back at rest has the depot below look at it again. Nothing runs without one.
fn tick_deployers(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::deployer::{State, Tick};
    let Some(list) = with_world(|w| w.dims[dim].deploying.clone()) else {
        return;
    };
    for pos in list {
        with_world(|w| {
            let rpm = w.speed(dim, pos).unwrap_or(0.0);
            let d = &mut w.dims[dim];
            let Some(dp) = d.deployers.get_mut(&pos) else {
                d.deploying.retain(|p| *p != pos);
                return;
            };
            if dp.tick(rpm) == Tick::Changed {
                d.deployer_changes.push(pos);
                if dp.state == State::Waiting {
                    d.depot_work.push([pos[0], pos[1] - 2, pos[2]]);
                }
            }
            if !dp.busy() {
                d.deploying.retain(|p| *p != pos);
            }
        });
    }
    let Some((changes, drops)) = with_world(|w| {
        let d = &mut w.dims[dim];
        let mut c = std::mem::take(&mut d.deployer_changes);
        c.sort();
        c.dedup();
        (c, std::mem::take(&mut d.deployer_drops))
    }) else {
        return;
    };
    for (pos, (item, n)) in drops {
        game.native_run_command(&format!(
            "execute in {dimension} run summon minecraft:item {}.5 {}.5 {}.5 {{Item:{{id:'{item}',count:{n}}}}}",
            pos[0], pos[1], pos[2]
        ));
    }
    if !changes.is_empty() {
        let players = players_in(game, dimension);
        send_deployers(game, &players, dim, &changes);
    }
}

/// Deployers' strokes to `players`: where the hand is and how fast it moves, what it holds.
fn send_deployers(game: &mut Game<'_>, players: &[u128], dim: usize, list: &[[i32; 3]]) {
    let mut shown = Vec::new();
    for &pos in list {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let facing = with_world(|w| w.table.get(state).and_then(|(_, i)| i.facing)).flatten();
        let rpm = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let d = with_world(|w| w.dims[dim].deployers.get(&pos).cloned()).flatten();
        shown.push(match (facing, d) {
            (Some(f), Some(d)) => {
                let (at, v) = d.hand(rpm);
                create_engine::deployer::Shown {
                    pos,
                    facing: f,
                    at,
                    per_tick: v,
                    held: d.held.map(|h| h.0),
                }
            }
            _ => create_engine::deployer::Shown {
                pos,
                facing: Dir::Up,
                at: f32::NAN,
                per_tick: 0.0,
                held: None,
            },
        });
    }
    for chunk in shown.chunks(64) {
        let m = create_engine::deployer::encode(chunk);
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::deployer::CHANNEL, &m);
        }
    }
}

/// `DeployerRenderer`: the pole and the hand (pointing, or holding) at the hand's offset, glided
/// along the facing on the GPU between messages.
fn deployers_from_server(game: &mut Game<'_>, data: &[u8]) {
    for s in create_engine::deployer::decode(data) {
        let pole = depot_key(s.pos, 110, 0);
        let hand = depot_key(s.pos, 111, 0);
        game.native_part_clear(pole);
        game.native_part_clear(hand);
        if s.at.is_nan() {
            continue;
        }
        let o = s.facing.offset().map(|v| v as f32);
        let now = game.native_parts_seconds();
        let offset = [o[0] * s.at, o[1] * s.at, o[2] * s.at, now];
        let v = [
            o[0] * s.per_tick * 20.0,
            o[1] * s.per_tick * 20.0,
            o[2] * s.per_tick * 20.0,
        ];
        let facing = dir_name(s.facing);
        let hand_block = if s.held.is_some() {
            "create:part_deployer_hand_holding"
        } else {
            "create:part_deployer_hand_pointing"
        };
        for (key, block) in [(pole, "create:part_deployer_pole"), (hand, hand_block)] {
            let registry = game.pack.assets.registry;
            let id = registry.default_state(block).and_then(|d| {
                let mut st = registry.state(d)?.clone();
                st.properties.insert("facing".to_owned(), facing.to_owned());
                registry.id(&st)
            });
            if let Some(model) = id.and_then(|id| game.native_part_model(id)) {
                game.native_part_glide(key, model, s.pos, 0, 0.0, 0.0, offset, v);
            }
        }
    }
}

/// `AbstractChassisBlock.useItemOn` for a linear chassis: a slime ball on an end along its axis
/// glues it (on a glued end: every end that may be glued); sneaking with an empty hand unglues
/// it. An end against a chassis of its kind on its axis can't be glued (`glueAllowedOnSide`).
fn use_chassis(
    game: &mut Game<'_>,
    pos: [i32; 3],
    face: [i32; 3],
    sneaking: bool,
    canceled: &mut bool,
) {
    let registry = game.pack.assets.registry;
    let id = game.world.block(pos[0], pos[1], pos[2]);
    let Some(mut s) = registry.state(id).cloned() else {
        return;
    };
    let held = game.native_held().map(|h| h.0);
    let slime = held.as_deref() == Some("minecraft:slime_ball")
        || held.as_deref() == Some("create:super_glue");
    let axis = s.properties.get("axis").cloned().unwrap_or_default();
    // getGlueableSide: an end on its axis, sticky_top toward the positive end.
    let side_of = |f: [i32; 3]| -> Option<&'static str> {
        let i = ["x", "y", "z"].iter().position(|a| *a == axis)?;
        match f[i] {
            1 => Some("sticky_top"),
            -1 => Some("sticky_bottom"),
            _ => None,
        }
    };
    let Some(side) = side_of(face) else {
        return;
    };
    let allowed = |game: &Game<'_>, f: [i32; 3]| {
        registry
            .state(
                game.world
                    .block(pos[0] + f[0], pos[1] + f[1], pos[2] + f[2]),
            )
            .is_none_or(|o| o.name != s.name || o.properties.get("axis") != Some(&axis))
    };
    let glued = s.properties.get(side).is_some_and(|v| v == "true");
    if slime && glued {
        for f in [
            [1, 0, 0],
            [-1, 0, 0],
            [0, 1, 0],
            [0, -1, 0],
            [0, 0, 1],
            [0, 0, -1],
        ] {
            if let Some(other) = side_of(f)
                && allowed(game, f)
            {
                s.properties.insert(other.into(), "true".into());
            }
        }
    } else if (slime || (sneaking && held.is_none())) && glued != slime && allowed(game, face) {
        s.properties
            .insert(side.into(), if slime { "true" } else { "false" }.into());
    } else {
        return;
    }
    *canceled = true;
    if let Some(new) = registry.id(&s)
        && new != id
    {
        let dimension = game.dim.id().to_owned();
        game.native_set_block(&dimension, pos, new, super::native_hooks::flags::UPDATE_ALL);
    }
}

/// `DeployerBlock.useItemOn`: a click on its front half swaps the player's hand with the
/// deployer's (plain stacks only here: components would be lost).
fn use_deployer(game: &mut Game<'_>, dim: usize, pos: [i32; 3], facing: Dir, canceled: &mut bool) {
    if game.native_held().is_some_and(|h| h.0 == "create:wrench") {
        return;
    }
    let Some(hit) = game.pick() else {
        return;
    };
    // The hit along the facing from the back face: less than .75 is the back's own use.
    let o = facing.offset();
    let along = (0..3)
        .map(|i| {
            (hit.point[i] - (f64::from(pos[i]) + 0.5 - 0.5 * f64::from(o[i]))) * f64::from(o[i])
        })
        .sum::<f64>();
    if along.abs() < 0.75 {
        return;
    }
    *canceled = true;
    if game.hand != horizon_client::first_person::Hand::Main {
        return;
    }
    let hand = game.inventory.get(game.selected).cloned().flatten();
    if hand
        .as_ref()
        .is_some_and(|s| s.extra.is_some() || s.damage != 0)
    {
        return;
    }
    let Some(mut d) = with_world(|w| w.dims[dim].deployers.get(&pos).cloned()).flatten() else {
        return;
    };
    if d.held.is_none() && hand.is_none() {
        return;
    }
    let back = d
        .held
        .take()
        .map(|(item, n)| horizon_client::items::ItemStack {
            item: item.as_str().into(),
            count: u32::from(n),
            damage: 0,
            extra: None,
        });
    d.held = hand.map(|s| (s.item.to_string(), s.count.min(64) as u8));
    if let Some(slot) = game.inventory.get_mut(game.selected) {
        *slot = back;
    }
    with_world(|w| {
        w.set_deployer(dim, pos, d);
        w.dims[dim].depot_work.push([pos[0], pos[1] - 2, pos[2]]);
    });
}

/// Depots that ask their press, then running presses: `DepotBehaviour.tick` with
/// `BeltPressingCallbacks`, then `PressingBehaviour.tick`. Nothing runs without either.
fn tick_presses(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::press::{self, Step, Tick};
    let Some((work, running)) = with_world(|w| {
        let d = &mut w.dims[dim];
        let running: Vec<[i32; 3]> = d
            .presses
            .iter()
            .filter(|(_, p)| p.running)
            .map(|(k, _)| *k)
            .collect();
        (std::mem::take(&mut d.depot_work), running)
    }) else {
        return;
    };
    if work.is_empty() && running.is_empty() {
        return;
    }
    if !ensure_recipes(game) {
        return;
    }
    let mut keep = Vec::new();
    for depot in work {
        let above = [depot[0], depot[1] + 2, depot[2]];
        if deployer_on_depot(game, dim, depot) {
            keep.push(depot);
            continue;
        }
        let Some(mut p) = with_world(|w| w.dims[dim].presses.get(&above).copied()).flatten() else {
            continue;
        };
        let Some(mut held) =
            with_world(|w| w.dims[dim].depots.get(&depot).copied().flatten()).flatten()
        else {
            continue;
        };
        if held.sliding() && held.position < 0.5 - 1.0 / 16.0 {
            continue;
        }
        let Some((_, item, count)) = depot_items(game, depot).into_iter().find(|s| s.0 == 0) else {
            continue;
        };
        let speed = with_world(|w| w.speed(dim, above)).flatten().unwrap_or(0.0);
        let in_way = blocked(game, [depot[0], depot[1] + 1, depot[2]]);
        let fits = RECIPES.with_borrow(|r| {
            r.as_ref()
                .is_some_and(|r| r.pressing.find_single(&item).is_some())
        });
        let step = press::depot_step(&mut held, &p, speed, in_way, fits);
        match step {
            Step::Idle => {}
            Step::Wait => keep.push(depot),
            Step::Start => {
                p.start(press::Mode::Belt);
                with_world(|w| {
                    w.dims[dim].presses.insert(above, p);
                    w.dims[dim].press_changes.push(above);
                });
                keep.push(depot);
            }
            Step::Process => {
                held = press_on_depot(game, dim, depot, &item, count, held);
                keep.push(depot);
            }
        }
        with_world(|w| {
            if let Some(h) = w.dims[dim].depots.get_mut(&depot) {
                *h = h.map(|_| held);
            }
        });
    }
    for at in running {
        let speed = with_world(|w| w.speed(dim, at)).flatten().unwrap_or(0.0);
        let basin_mode = with_world(|w| {
            let d = &mut w.dims[dim];
            let p = d.presses.get_mut(&at)?;
            let tick = p.tick(speed);
            match tick {
                Tick::Moving => {}
                Tick::Down => d.press_changes.push(at),
                Tick::Done => {
                    d.press_changes.push(at);
                    let below = [at[0], at[1] - 2, at[2]];
                    if d.depots.contains_key(&below) && !keep.contains(&below) {
                        keep.push(below);
                    }
                }
            }
            (p.mode == press::Mode::Basin).then_some(tick)
        })
        .flatten();
        // Over a basin: compact at the bottom (`tryProcessInBasin`), look again when done
        // (`onPressingCompleted`).
        match basin_mode {
            Some(Tick::Down) => compact_in_basin(game, dim, at),
            Some(Tick::Done) => {
                with_world(|w| w.dims[dim].mixer_checks.push(at));
            }
            _ => {}
        }
    }
    let changed = with_world(|w| {
        let d = &mut w.dims[dim];
        for p in keep {
            if !d.depot_work.contains(&p) {
                d.depot_work.push(p);
            }
        }
        let mut c = std::mem::take(&mut d.press_changes);
        c.sort();
        c.dedup();
        c
    })
    .unwrap_or_default();
    if !changed.is_empty() {
        let players = players_in(game, dimension);
        send_presses(game, &players, dim, &changed);
    }
}

/// `BeltPressingCallbacks.whenItemHeld` at the head's lowest point: one of the stack (all of
/// it when it is one) becomes the recipe's rolled results. A single item is replaced by them
/// (the first held, centred; the rest in the outputs); otherwise the rest stays held and the
/// results go to the outputs. What finds no room drops above the depot. Returns the held
/// stack's motion after.
fn press_on_depot(
    game: &mut Game<'_>,
    dim: usize,
    depot: [i32; 3],
    item: &str,
    count: u8,
    held: create_engine::depot::Held,
) -> create_engine::depot::Held {
    use create_engine::processing;
    let seed = game.world.game_time as u64
        ^ (depot[0] as u64).wrapping_mul(0x9E37_79B9)
        ^ (depot[2] as u64) << 20;
    let results = RECIPES.with_borrow(|r| {
        let r = r.as_ref()?;
        let id = r.pressing.find_single(item)?;
        Some(processing::roll(
            r.pressing.get(id),
            &mut processing::Rng::new(seed),
        ))
    });
    let Some(results) = results else {
        return held;
    };
    results_on_depot(game, dim, depot, count, held, results)
}

/// A processing result on a depot's held stack (`handleProcessingOnItem`): one of the stack
/// (all of it when it is one) becomes `results`: a single item is replaced by them (the first
/// held, centred; the rest in the outputs); otherwise the rest stays held and the results go to
/// the outputs. What finds no room drops above the depot. The held stack's motion after.
fn results_on_depot(
    game: &mut Game<'_>,
    dim: usize,
    depot: [i32; 3],
    count: u8,
    held: create_engine::depot::Held,
    results: Vec<(String, u8)>,
) -> create_engine::depot::Held {
    use create_engine::processing;
    let limits: Vec<u8> = results
        .iter()
        .map(|(i, _)| game.items().get(i).map_or(64, |x| x.max_stack).clamp(1, 64) as u8)
        .collect();
    let stack = |item: &str, count: u8| horizon_client::items::ItemStack {
        item: item.into(),
        count: u32::from(count),
        damage: 0,
        extra: None,
    };
    let single = count == 1;
    let angle = random_angle(game, depot);
    let mut next = held;
    let dropped = game
        .native_with_mod_block_entity(depot, |be| {
            be.items
                .resize(be.items.len().max(create_engine::depot::SLOTS), None);
            let mut rest = results.iter().zip(&limits);
            if single {
                be.items[0] = None;
                if let Some(((first, n), _)) = rest.next() {
                    be.items[0] = Some(stack(first, *n));
                    next = create_engine::depot::Held {
                        angle,
                        position: 0.5,
                        locked: true,
                        ..held
                    };
                }
            } else if let Some(s) = be.items[0].as_mut() {
                s.count -= 1;
            }
            let mut outputs: Vec<processing::Slot> = be.items[create_engine::depot::OUTPUTS]
                .iter()
                .map(slot_of)
                .collect();
            let mut dropped = Vec::new();
            for ((i, n), limit) in rest {
                let left = processing::insert_stacked(&mut outputs, i, *n, *limit);
                if left > 0 {
                    dropped.push((i.clone(), left));
                }
            }
            for (slot, s) in be.items[create_engine::depot::OUTPUTS]
                .iter_mut()
                .zip(outputs)
            {
                *slot = s.map(|(i, n)| stack(&i, n));
            }
            dropped
        })
        .unwrap_or_default();
    for (i, n) in dropped {
        // Containers.dropItemStack at the depot's centre, half a block up.
        game.native_run_command(&format!(
            "summon minecraft:item {}.5 {}.0 {}.5 {{Item:{{id:'{i}',count:{n}}}}}",
            depot[0],
            depot[1] + 1,
            depot[2]
        ));
    }
    if results.is_empty() && single {
        with_world(|w| w.depot_put(dim, depot, None));
    }
    with_world(|w| w.depot_changed(dim, depot));
    next
}

thread_local! {
    /// The face of the last block an item was used on, and when: `native_place_block` doesn't
    /// say which face was clicked, the use before it does.
    static CLICKED: std::cell::Cell<Option<(i64, [i32; 3])>> = const { std::cell::Cell::new(None) };
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

/// `BlazeBurnerBlock.useItemOn`: flint and steel lights an empty burner (a lit burner, no blaze);
/// fuel goes to the blaze (`tryInsert` with a player's overflow), one used up outside creative
/// (a lava bucket leaves its bucket).
fn use_burner(game: &mut Game<'_>, dim: usize, pos: [i32; 3], canceled: &mut bool) {
    use create_engine::burner::Fuel;
    use create_engine::fan::Heat;
    let Some((held, _, plain)) = game.native_held() else {
        return;
    };
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some((heat, facing)) =
        with_world(|w| w.table.get(state.0).map(|(_, i)| (i.heat, i.facing))).flatten()
    else {
        return;
    };
    if heat == Some(Heat::None) {
        if held == "minecraft:flint_and_steel" {
            let facing = facing.map_or("north", dir_name);
            if let Some(lit) = state_with(
                game.pack.assets.registry,
                "create:lit_blaze_burner",
                &[("facing", facing)],
            ) {
                let dimension = game.dim.id().to_owned();
                game.native_set_block(&dimension, pos, lit, super::native_hooks::flags::UPDATE_ALL);
                *canceled = true;
            }
        }
        return;
    }
    if !plain {
        return;
    }
    let stack = horizon_client::items::ItemStack {
        item: held.as_str().into(),
        count: 1,
        damage: 0,
        extra: None,
    };
    let tagged = |t: &str| {
        game.items()
            .item_tags
            .get(t)
            .is_some_and(|m| m.contains(&held))
    };
    // A data map's superheating fuel (the blaze cake), the special tag; else a furnace fuel's
    // burn time, the regular tag (as coal).
    let (fuel, time) = if held == "create:blaze_cake" || tagged("create:blaze_burner_fuel/special")
    {
        (Fuel::Special, 3200)
    } else {
        let t = game.res.providers().burn_time(game.items(), &stack, false);
        if t > 0 {
            (Fuel::Normal, t)
        } else if tagged("create:blaze_burner_fuel/regular") {
            (Fuel::Normal, 1600)
        } else {
            return;
        }
    };
    let Some(heat) = with_world(|w| {
        w.fuel_burner(dim, pos, fuel, time, true)
            .then(|| w.dims[dim].burners.get(&pos).map(|b| b.heat()))
            .flatten()
    })
    .flatten() else {
        return;
    };
    *canceled = true;
    game.native_set_block_property(pos, "blaze", create_engine::burner::heat_name(heat));
    if !game.creative {
        let remainder = game.items().get(&held).and_then(|i| i.remainder.clone());
        game.native_take_items(&held, 1);
        if let Some(r) = remainder {
            game.native_give_items(&r, 1);
        }
    }
}

/// `BasinBlock.updateEntityAfterFallOn`: an item landing in a basin goes into its inputs as far
/// as it fits (`insertItem`).
fn basin_take(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: &mut horizon_client::items::ItemStack,
) {
    let max = game
        .items()
        .get(&stack.item)
        .map_or(64, |x| x.max_stack)
        .clamp(1, 64) as u8;
    let item = stack.item.to_string();
    let count = stack.count.min(255) as u8;
    let left = game
        .native_with_mod_block_entity(pos, |be| {
            be.items
                .resize(be.items.len().max(create_engine::basin::SLOTS), None);
            let mut inputs: Vec<create_engine::processing::Slot> = be.items
                [create_engine::basin::INPUTS]
                .iter()
                .map(slot_of)
                .collect();
            let left = create_engine::processing::insert_stacked(&mut inputs, &item, count, max);
            for (slot, s) in be.items[create_engine::basin::INPUTS]
                .iter_mut()
                .zip(inputs)
            {
                *slot = s.map(|(i, n)| horizon_client::items::ItemStack {
                    item: i.as_str().into(),
                    count: u32::from(n),
                    damage: 0,
                    extra: None,
                });
            }
            left
        })
        .unwrap_or(count);
    if left != count {
        stack.count = u32::from(left);
        with_world(|w| w.dims[dim].mixer_checks.push([pos[0], pos[1] + 2, pos[2]]));
    }
}

/// A stack's limit (at most 64).
fn stack_limit(game: &Game<'_>, item: &str) -> u8 {
    game.items()
        .get(item)
        .map_or(64, |x| x.max_stack)
        .clamp(1, 64) as u8
}

/// `ChuteBlockEntity` taking an item that landed on it (`canAcceptItem`: only while empty): the
/// whole stack, at the top.
fn chute_take(
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

fn chute_stack(game: &Game<'_>, pos: [i32; 3]) -> Option<horizon_client::items::ItemStack> {
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(horizon_client::containers::BlockEntity::Mod(m)) => m.items.first().cloned().flatten(),
        _ => None,
    }
}

fn set_chute_stack(
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
fn tick_chutes(game: &mut Game<'_>, dim: usize) {
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

/// Turning drills (`BlockBreakingKineticBlockEntity.tick`): the block in front breaks in steps
/// (`isBreakable`: not air, a fluid, unbreakable or `create:non_breakable`), dropping what it
/// drops; a drill that broke one looks again at its next lazy tick (every 10 ticks).
fn tick_drills(game: &mut Game<'_>, dim: usize) {
    let drills = with_world(|w| w.dims[dim].drilling.clone()).unwrap_or_default();
    if drills.is_empty() {
        return;
    }
    let lazy = game.world.game_time % 10 == 0;
    for pos in drills {
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let Some(facing) =
            with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten()
        else {
            continue;
        };
        let o = facing.offset();
        let at = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
        let target = game.world.block(at[0], at[1], at[2]);
        let b = game.res.behaviours();
        let fluid = b.fluid(target).kind() != horizon_world::fluid::FluidKind::Empty
            && b.is_replaceable(target);
        let tagged = b
            .tag("create:non_breakable")
            .is_some_and(|t| t.contains(b.block(target)));
        let hardness = if b.is_air(target) || fluid || tagged {
            None
        } else {
            create_engine::drill::breakable(b.facts(target).hardness)
        };
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let Some(step) = with_world(|w| {
            let d = w.dims[dim].drills.get_mut(&pos)?;
            if lazy {
                d.lazy();
            }
            Some(d.tick(speed, hardness))
        })
        .flatten() else {
            continue;
        };
        if step == create_engine::drill::Step::Broken {
            // Level.destroyBlock with drops (`BlockHelper.destroyBlock`).
            game.native_run_command(&format!(
                "setblock {} {} {} air destroy",
                at[0], at[1], at[2]
            ));
        }
    }
}

/// `SawBlockEntity.insertItem` for an item landing on a saw facing up, while it holds nothing.
fn saw_take(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: &mut horizon_client::items::ItemStack,
) {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    if with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten() != Some(Dir::Up) {
        return;
    }
    let took = with_world(|w| {
        let s = w.dims[dim].saws.entry(pos).or_default();
        if !s.slots.is_empty() {
            return false;
        }
        s.slots = vec![(stack.item.to_string(), stack.count.min(64) as u8)];
        s.remaining = -1.0;
        s.applied = false;
        true
    })
    .unwrap_or(false);
    if took {
        stack.count = stack.count.saturating_sub(64);
    }
}

/// Saws holding items (`SawBlockEntity.tick`): their cutting recipe in turn, applied near the
/// end, the results thrown off the way the saw turns (onto a belt or depot there, else as items).
fn tick_saws(game: &mut Game<'_>, dim: usize) {
    use create_engine::saw::Step;
    let saws: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].saws.keys().copied().collect()).unwrap_or_default();
    if saws.is_empty() || !ensure_recipes(game) {
        return;
    }
    for pos in saws {
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let Some(along_first) =
            with_world(|w| w.table.get(state.0).map(|(_, i)| i.along_first)).flatten()
        else {
            continue;
        };
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let Some(mut saw) = with_world(|w| w.dims[dim].saws.get(&pos).cloned()).flatten() else {
            continue;
        };
        let item = saw.slots.first().map(|s| s.0.clone()).unwrap_or_default();
        match saw.tick(speed) {
            Step::Wait => {}
            Step::Start => RECIPES.with_borrow(|r| {
                let Some(r) = r.as_ref() else { return };
                let list: Vec<&create_engine::processing::Recipe> = r
                    .cutting
                    .matching(&item)
                    .into_iter()
                    .map(|i| r.cutting.get(i))
                    .collect();
                saw.start(&list);
            }),
            Step::Apply => {
                let mut rng = create_engine::processing::Rng::new(
                    game.world.game_time as u64
                        ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9)
                        ^ (pos[2] as u64) << 20,
                );
                RECIPES.with_borrow(|r| {
                    let Some(r) = r.as_ref() else { return };
                    let list = r.cutting.matching(&item);
                    let recipe = list.get(saw.index).map(|&i| r.cutting.get(i));
                    saw.apply(recipe, |i: &str| stack_limit(game, i), &mut rng);
                });
            }
            Step::Out => {
                let m = create_engine::saw::movement(along_first, speed);
                let (x, y, z) = (
                    f64::from(pos[0]) + 0.5 + f64::from(m[0]) * 0.5,
                    f64::from(pos[1]) + 1.0,
                    f64::from(pos[2]) + 0.5 + f64::from(m[2]) * 0.5,
                );
                let (vx, vz) = (f64::from(m[0]) * 0.0625, f64::from(m[2]) * 0.0625);
                for (item, count) in &saw.slots {
                    game.native_run_command(&format!(
                        "summon minecraft:item {x} {y} {z} {{Item:{{id:'{item}',count:{count}}},Motion:[{vx}d,0.125d,{vz}d]}}"
                    ));
                }
                saw.clear();
            }
        }
        with_world(|w| {
            if saw.slots.is_empty() {
                w.dims[dim].saws.remove(&pos);
            } else {
                w.dims[dim].saws.insert(pos, saw);
            }
        });
    }
}

/// Spouts holding fluid (`SpoutBlockEntity.whenItemHeld` on the depot two below, `tick`): the
/// held empty bucket is filled after the 20-tick cycle (at its 5th-last tick).
fn tick_spouts(game: &mut Game<'_>, dim: usize) {
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
        let fluid = s.fluid.map(|f| f.0);
        if create_engine::spout::step(&mut s, held.as_ref().map(|h| h.1.as_str()))
            && let (Some((_, item, count)), Some(fluid)) = (held, fluid)
            && let Some(out) = create_engine::spout::filled(&item, fluid)
        {
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
                    if let Some(slot) = be.items[create_engine::depot::OUTPUTS]
                        .iter_mut()
                        .find(|s| s.is_none())
                    {
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
fn tick_drains(game: &mut Game<'_>, dim: usize) {
    let drains: Vec<[i32; 3]> = with_world(|w| {
        w.dims[dim]
            .drains
            .iter()
            .filter(|(_, d)| d.held.is_some())
            .map(|(p, _)| *p)
            .collect()
    })
    .unwrap_or_default();
    for pos in drains {
        let Some(out) = with_world(|w| {
            let d = w.dims[dim].drains.get_mut(&pos)?;
            let held = d.held.clone();
            if held
                .as_ref()
                .is_some_and(|h| create_engine::drain::contents(&h.0).is_none())
            {
                d.held = None;
                return held.map(|h| (h.0, h.1));
            }
            create_engine::drain::tick(d).then(|| ("minecraft:bucket".to_owned(), 1))
        })
        .flatten() else {
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
fn tick_funnels(game: &mut Game<'_>, dim: usize) {
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
        game.native_items_in(
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
        let Some(stack) = game.native_extract_item(behind, face, create_engine::funnel::AMOUNT)
        else {
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

/// The heat under a basin (`BasinBlockEntity.getHeatLevelOf`): a blaze burner's, smouldering for
/// a lit burner or another passive heater, else none.
fn basin_heat(game: &Game<'_>, basin: [i32; 3]) -> create_engine::fan::Heat {
    use create_engine::fan::Heat;
    let state = game.world.block(basin[0], basin[1] - 1, basin[2]);
    if let Some(h) = with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.heat)).flatten() {
        return h;
    }
    let b = game.res.behaviours();
    if b.tag("create:passive_boiler_heaters")
        .is_some_and(|t| t.contains(b.block(state)))
    {
        // BlockHelper.isNotUnheated: an unlit campfire is no heater.
        let lit = game
            .pack
            .assets
            .registry
            .state(state)
            .and_then(|s| s.properties.get("lit").cloned());
        if lit.as_deref() != Some("false") {
            return Heat::Smouldering;
        }
    }
    Heat::None
}

/// The slots of the basin at `pos`, all 18.
fn basin_slots(game: &Game<'_>, pos: [i32; 3]) -> Option<Vec<create_engine::processing::Slot>> {
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(horizon_client::containers::BlockEntity::Mod(m)) => {
            let mut v: Vec<_> = m.items.iter().map(slot_of).collect();
            v.resize(create_engine::basin::SLOTS, None);
            Some(v)
        }
        _ => None,
    }
}

/// Mechanical mixers: those asked to look at their basin (`updateBasin`: turning, not running,
/// a recipe the basin can do), then the running ones' cycle (`tick`), mixing at the bottom
/// (`applyBasinRecipe`) and going on with the same recipe while it still fits.
fn tick_mixers(game: &mut Game<'_>, dim: usize) {
    use create_engine::basin::{self, MixStep};
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].mixer_checks)).unwrap_or_default();
    let running = with_world(|w| w.dims[dim].mixing.clone()).unwrap_or_default();
    if checks.is_empty() && running.is_empty() {
        return;
    }
    if !ensure_recipes(game) {
        return;
    }
    for pos in running {
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let Some(mut m) = with_world(|w| w.dims[dim].mixers.get(&pos).copied()).flatten() else {
            continue;
        };
        let time = RECIPES.with_borrow(|r| {
            r.as_ref()
                .and_then(|r| m.recipe.and_then(|i| r.mixing.get(i)).map(|x| x.time))
                .unwrap_or(0)
        });
        let basin_pos = [pos[0], pos[1] - 2, pos[2]];
        match m.tick(speed, time) {
            MixStep::None => {}
            MixStep::Done => checks.push(pos),
            MixStep::Apply => {
                let heat = basin_heat(game, basin_pos);
                let mut rng = create_engine::processing::Rng::new(
                    game.world.game_time as u64
                        ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9)
                        ^ (pos[2] as u64) << 20,
                );
                if let (Some(mut slots), Some(i)) = (basin_slots(game, basin_pos), m.recipe) {
                    let limit = |i: &str| stack_limit(game, i);
                    let done = RECIPES.with_borrow(|r| {
                        r.as_ref()
                            .and_then(|r| r.mixing.get(i))
                            .is_some_and(|recipe| {
                                basin::apply(&mut slots, recipe, heat, &limit, &mut rng, false)
                            })
                    });
                    if done {
                        let again = RECIPES.with_borrow(|r| {
                            r.as_ref()
                                .and_then(|r| r.mixing.get(i))
                                .is_some_and(|recipe| {
                                    basin::apply(
                                        &mut slots.clone(),
                                        recipe,
                                        heat,
                                        &limit,
                                        &mut rng,
                                        true,
                                    )
                                })
                        });
                        game.native_with_mod_block_entity(basin_pos, |be| {
                            be.items = slots
                                .iter()
                                .map(|s| {
                                    s.as_ref().map(|(i, n)| horizon_client::items::ItemStack {
                                        item: i.as_str().into(),
                                        count: u32::from(*n),
                                        damage: 0,
                                        extra: None,
                                    })
                                })
                                .collect();
                        });
                        // `continueWithPreviousRecipe`: stays down for the next one.
                        if again {
                            m.running_ticks = 20;
                        }
                    }
                }
            }
        }
        with_world(|w| {
            let d = &mut w.dims[dim];
            d.mixers.insert(pos, m);
            if !m.running {
                d.mixing.retain(|p| *p != pos);
            }
        });
    }
    checks.sort();
    checks.dedup();
    for pos in checks {
        let Some(mut m) = with_world(|w| w.dims[dim].mixers.get(&pos).copied()).flatten() else {
            press_on_basin(game, dim, pos);
            continue;
        };
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let basin_pos = [pos[0], pos[1] - 2, pos[2]];
        let is_basin = {
            let s = game.world.block(basin_pos[0], basin_pos[1], basin_pos[2]);
            with_world(|w| w.table.get(s.0).map(|(b, _)| b)).flatten()
                == Some(create_engine::Block::Basin)
        };
        if !is_basin {
            // `onBasinRemoved`.
            if m.running {
                m.running = false;
                m.running_ticks = 0;
                with_world(|w| {
                    w.dims[dim].mixers.insert(pos, m);
                    w.dims[dim].mixing.retain(|p| *p != pos);
                });
            }
            continue;
        }
        if speed == 0.0 || m.running {
            continue;
        }
        let Some(slots) = basin_slots(game, basin_pos) else {
            continue;
        };
        let heat = basin_heat(game, basin_pos);
        let limit = |i: &str| stack_limit(game, i);
        let found = RECIPES.with_borrow(|r| {
            r.as_ref().and_then(|r| {
                basin::matching(&r.mixing, &slots, heat, &limit)
                    .first()
                    .copied()
            })
        });
        if let Some(i) = found {
            m.start(i);
            with_world(|w| {
                let d = &mut w.dims[dim];
                d.mixers.insert(pos, m);
                if !d.mixing.contains(&pos) {
                    d.mixing.push(pos);
                }
            });
        }
    }
}

/// `MechanicalPressBlockEntity` as a basin operator (`updateBasin`): a turning press with a
/// basin two below starts a cycle over it (`Mode.BASIN`) for the compacting recipe with the most
/// ingredients the basin can do.
fn press_on_basin(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    use create_engine::{basin, press};
    let Some(p) = with_world(|w| w.dims[dim].presses.get(&pos).copied()).flatten() else {
        return;
    };
    let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
    if p.running || speed == 0.0 {
        return;
    }
    let basin_pos = [pos[0], pos[1] - 2, pos[2]];
    let Some(slots) = basin_slots(game, basin_pos).filter(|_| {
        let s = game.world.block(basin_pos[0], basin_pos[1], basin_pos[2]);
        with_world(|w| w.table.get(s.0).map(|(b, _)| b)).flatten()
            == Some(create_engine::Block::Basin)
    }) else {
        return;
    };
    let heat = basin_heat(game, basin_pos);
    let limit = |i: &str| stack_limit(game, i);
    let found = RECIPES.with_borrow(|r| {
        r.as_ref().and_then(|r| {
            basin::matching(&r.compacting, &slots, heat, &limit)
                .first()
                .copied()
        })
    });
    if let Some(i) = found {
        with_world(|w| {
            let d = &mut w.dims[dim];
            if let Some(p) = d.presses.get_mut(&pos) {
                p.start(press::Mode::Basin);
            }
            d.press_basin.insert(pos, i);
            d.press_changes.push(pos);
        });
    }
}

/// `tryProcessInBasin` at the head's lowest point: the press's compacting recipe on the basin.
fn compact_in_basin(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    use create_engine::basin;
    let Some(i) = with_world(|w| w.dims[dim].press_basin.get(&pos).copied()).flatten() else {
        return;
    };
    let basin_pos = [pos[0], pos[1] - 2, pos[2]];
    let Some(mut slots) = basin_slots(game, basin_pos) else {
        return;
    };
    let heat = basin_heat(game, basin_pos);
    let mut rng = create_engine::processing::Rng::new(
        game.world.game_time as u64
            ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9)
            ^ (pos[2] as u64) << 20,
    );
    let limit = |i: &str| stack_limit(game, i);
    let done = RECIPES.with_borrow(|r| {
        r.as_ref()
            .and_then(|r| r.compacting.get(i))
            .is_some_and(|recipe| basin::apply(&mut slots, recipe, heat, &limit, &mut rng, false))
    });
    if done {
        game.native_with_mod_block_entity(basin_pos, |be| {
            be.items = slots
                .iter()
                .map(|s| {
                    s.as_ref().map(|(i, n)| horizon_client::items::ItemStack {
                        item: i.as_str().into(),
                        count: u32::from(*n),
                        damage: 0,
                        extra: None,
                    })
                })
                .collect();
        });
    }
}

/// What a block is to the pipes (`FluidPropagator`): a pipe and its open sides, a pump and its
/// facing, a tank, open space a pipe may end in, or neither.
fn pipe_cell(game: &Game<'_>, p: [i32; 3]) -> create_engine::fluids::Cell {
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
            Block::FluidTank | Block::Spout | Block::ItemDrain => Cell::Tank,
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
fn pipe_sides_of(game: &Game<'_>, state: BlockStateId) -> u8 {
    let Some(s) = game.pack.assets.registry.state(state) else {
        return 0;
    };
    Dir::ALL
        .into_iter()
        .filter(|d| s.properties.get(dir_name(*d)).is_some_and(|v| v == "true"))
        .fold(0, |m, d| m | d.bit())
}

/// `FluidPipeBlock.updateBlockState` for the pipe at `pos`, written when it changes.
fn connect_pipe(game: &mut Game<'_>, dimension: &str, pos: [i32; 3], preferred: Option<Dir>) {
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
fn tick_pipes(game: &mut Game<'_>, dimension: &str, dim: usize) {
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
            let offer = if drained_from_drain {
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
                    moved = if to.tank {
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
                        if total >= BUCKET {
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
                    if drained_from_drain {
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

/// Burning blaze burners (`BlazeBurnerBlockEntity.tick`): the fuel burns down; a changed heat is
/// written to the block (`blaze`), which fans and basins read.
fn tick_burners(game: &mut Game<'_>, dim: usize) {
    let changed = with_world(|w| {
        if w.dims[dim].burning.is_empty() {
            Vec::new()
        } else {
            w.tick_burners(dim)
        }
    })
    .unwrap_or_default();
    for (pos, heat) in changed {
        game.native_set_block_property(pos, "blaze", create_engine::burner::heat_name(heat));
    }
}

/// A block state of `name` with `props` set (others at their defaults).
fn state_with(
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

/// Crushing wheels: the controllers between wheels turning against each other
/// (`CrushingWheelBlock.updateControllers`, after a wheel is placed, broken or changes speed),
/// then the turning controllers' `tick`.
fn tick_crushers(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].crusher_checks)).unwrap_or_default();
    checks.sort();
    checks.dedup();
    for pos in checks {
        fix_controllers(game, dimension, dim, pos);
    }
    let running = with_world(|w| w.dims[dim].running_crushers.clone()).unwrap_or_default();
    if running.is_empty() || !ensure_recipes(game) {
        return;
    }
    for pos in running {
        tick_crusher(game, dimension, dim, pos);
    }
}

fn fix_controllers(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    use create_engine::{Block, crusher};
    let registry = game.pack.assets.registry;
    let at = |p: [i32; 3], side: Dir, n: i32| {
        let o = side.offset();
        [p[0] + o[0] * n, p[1] + o[1] * n, p[2] + o[2] * n]
    };
    let block_of = |game: &Game<'_>, p: [i32; 3]| {
        let s = game.world.block(p[0], p[1], p[2]);
        with_world(|w| w.table.get(s.0).map(|(b, i)| (b, i.axis))).flatten()
    };
    let wheel = block_of(game, pos).filter(|b| b.0 == Block::CrushingWheel);
    let air = BlockStateId(0);
    for side in Dir::ALL {
        let cpos = at(pos, side, 1);
        let existing = block_of(game, cpos).is_some_and(|b| b.0 == Block::CrushingWheelController);
        let want = wheel.and_then(|(_, axis)| {
            let axis = axis?;
            let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
            let opos = at(pos, side, 2);
            let other = block_of(game, opos)
                .filter(|b| b.0 == Block::CrushingWheel)
                .and_then(|b| b.1)
                .map(|a| {
                    (
                        a,
                        with_world(|w| w.speed(dim, opos)).flatten().unwrap_or(0.0),
                    )
                });
            crusher::controller(axis, side, speed, other).map(|c| (c, speed))
        });
        match want {
            // A wheel that went (`onRemove`) or no partner: no controller.
            None => {
                if existing && (wheel.is_none() || wheel.and_then(|w| w.1) != Some(side.axis())) {
                    game.native_set_block(
                        dimension,
                        cpos,
                        air,
                        super::native_hooks::flags::UPDATE_ALL,
                    );
                }
            }
            Some(((valid, facing), speed)) => {
                let here = game.world.block(cpos[0], cpos[1], cpos[2]);
                if !existing && !game.res.behaviours().is_replaceable(here) {
                    continue;
                }
                let Some(state) = state_with(
                    registry,
                    "create:crushing_wheel_controller",
                    &[
                        ("facing", dir_name(facing)),
                        ("valid", if valid { "true" } else { "false" }),
                    ],
                ) else {
                    continue;
                };
                if here != state {
                    game.native_set_block(
                        dimension,
                        cpos,
                        state,
                        super::native_hooks::flags::UPDATE_ALL,
                    );
                }
                let crushing = if valid {
                    crusher::crushing_speed(speed)
                } else {
                    0.0
                };
                with_world(|w| w.set_crusher_speed(dim, cpos, crushing));
            }
        }
    }
}

/// `CrushingWheelControllerBlockEntity.tick` (and the controller block's `entityInside`).
fn tick_crusher(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    use create_engine::{crusher, processing};
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some(facing) = with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten()
    else {
        return;
    };
    let Some((mut c, speed)) = with_world(|w| w.dims[dim].crushers.get(&pos).cloned()).flatten()
    else {
        return;
    };
    let cell = |grow: f64| {
        let lo = pos.map(|v| f64::from(v) - grow);
        let hi = pos.map(|v| f64::from(v) + 1.0 + grow);
        (lo, hi)
    };
    let before = c.clone();
    if let Some(id) = c.entity {
        // The held item: drawn to the middle and in; taken a quarter past the middle.
        let (lo, hi) = cell(0.5);
        let mut found = None;
        game.native_items_in(dimension, lo, hi, |i, stack, at, _| {
            if i == id {
                found = Some((stack.item.to_string(), stack.count.min(255) as u8, at));
            }
        });
        match found {
            None => c.entity = None,
            Some((item, count, at)) => {
                let (motion, taken) = crusher::hold(pos, facing, speed, at);
                if taken {
                    let time = RECIPES.with_borrow(|r| {
                        let r = r.as_ref()?;
                        crusher::recipe(&r.crushing, &r.milling, &item).map(|x| x.time)
                    });
                    crusher::intake(&mut c, &item, count, time);
                }
                game.native_items_in(dimension, lo, hi, |i, stack, _, v| {
                    if i == id {
                        if taken {
                            stack.count = 0;
                        } else {
                            *v = motion;
                        }
                    }
                });
            }
        }
    } else if c.slots.is_empty() {
        // `checkEntityForProcessing`: the first item inside starts it.
        let (lo, hi) = cell(0.0);
        let mut first = None;
        game.native_items_in(dimension, lo, hi, |i, _, at, _| {
            if !crusher::bypassed(pos, facing, at) {
                first.get_or_insert(i);
            }
        });
        c.entity = first;
    } else {
        match crusher::tick_inventory(&mut c, speed) {
            crusher::Step::Wait => {}
            crusher::Step::Apply => {
                let max =
                    |i: &str| game.items().get(i).map_or(64, |x| x.max_stack).clamp(1, 64) as u8;
                let mut rng = processing::Rng::new(
                    game.world.game_time as u64
                        ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9)
                        ^ (pos[2] as u64) << 20,
                );
                let item = c.slots[0].0.clone();
                RECIPES.with_borrow(|r| {
                    let recipe = r
                        .as_ref()
                        .and_then(|r| crusher::recipe(&r.crushing, &r.milling, &item));
                    crusher::apply(&mut c, recipe, max, &mut rng);
                });
            }
            crusher::Step::Output => {
                // Results thrown out past the facing side (`outPos`, `outSpeed`).
                let (at, v) = crusher::output(pos, facing);
                for (item, count) in std::mem::take(&mut c.slots) {
                    game.native_run_command(&format!(
                        "summon minecraft:item {} {} {} {{Item:{{id:'{item}',count:{count}}},Motion:[{}d,{}d,{}d]}}",
                        at[0], at[1], at[2], v[0], v[1], v[2]
                    ));
                }
            }
        }
    }
    with_world(|w| {
        if let Some(e) = w.dims[dim].crushers.get_mut(&pos) {
            e.0 = c.clone();
        }
        if c.slots != before.slots {
            w.mark_unsaved(dim, pos);
        }
    });
}

/// Encased fans that turn (`EncasedFanBlockEntity.tick`): the current looked at again when due
/// (`AirCurrent.rebuild`), then the depots and belts in it processed
/// (`tickAffectedHandlers`). Stopped fans cost nothing.
fn tick_fans(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let fans = with_world(|w| w.fans_to_tick(dim)).unwrap_or_default();
    if fans.is_empty() {
        return;
    }
    let recipes = ensure_recipes(game);
    // Item entities' progress, kept for those still in a current this tick.
    let mut kept = std::collections::HashMap::new();
    let mut old = with_world(|w| std::mem::take(&mut w.dims[dim].item_fan)).unwrap_or_default();
    for pos in fans {
        let Some((rebuild, speed, facing)) = with_world(|w| {
            let f = w.dims[dim].fans.get(&pos)?;
            Some((f.rebuild, f.speed, f.facing))
        })
        .flatten() else {
            continue;
        };
        if rebuild {
            let (current, handlers) = match facing {
                Some(facing) if speed != 0.0 => fan_current(game, dim, pos, facing, speed),
                _ => Default::default(),
            };
            with_world(|w| {
                if let Some(f) = w.dims[dim].fans.get_mut(&pos) {
                    f.rebuild = false;
                    f.changed |= f.current != current;
                    f.current = current;
                    f.handlers = handlers;
                }
            });
        }
        if speed == 0.0 || !recipes {
            continue;
        }
        fan_on_items(game, dimension, dim, pos, &mut old, &mut kept);
        let handlers = with_world(|w| w.dims[dim].fans.get(&pos).map(|f| f.handlers.clone()))
            .flatten()
            .unwrap_or_default();
        for (at, kind) in handlers {
            let Some(kind) = kind else {
                continue;
            };
            fan_on_depot(game, dim, at, kind);
            fan_on_belt(game, dim, at, kind);
        }
    }
    with_world(|w| w.dims[dim].item_fan = kept);
}

/// `AirCurrent.tickAffectedEntities` for items: those its search caught (every 5 ticks) and still
/// in its box are pushed along the current, and processed by the type at their distance
/// (`FanProcessing.applyProcessing`): the first result replaces the stack, the others are
/// dropped beside it with its motion, none discards it.
fn fan_on_items(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    old: &mut std::collections::HashMap<u32, create_engine::fan::Progress>,
    kept: &mut std::collections::HashMap<u32, create_engine::fan::Progress>,
) {
    use create_engine::{fan, processing};
    let Some((facing, speed, current, search)) = with_world(|w| {
        let f = w.dims[dim].fans.get_mut(&pos)?;
        let search = f.search_cooldown == 0;
        f.search_cooldown = if search { 5 } else { f.search_cooldown - 1 };
        Some((f.facing?, f.speed, f.current.clone(), search))
    })
    .flatten() else {
        return;
    };
    let Some((lo, hi)) = fan::bounds(pos, facing, current.max) else {
        return;
    };
    let mut items = Vec::new();
    game.native_items_in(dimension, lo, hi, |id, stack, at, v| {
        items.push((id, stack.item.to_string(), stack.count, at, *v));
    });
    let caught: Vec<u32> = if search {
        let ids: Vec<u32> = items.iter().map(|i| i.0).collect();
        with_world(|w| {
            if let Some(f) = w.dims[dim].fans.get_mut(&pos) {
                f.caught = ids.clone();
            }
        });
        ids
    } else {
        with_world(|w| w.dims[dim].fans.get(&pos).map(|f| f.caught.clone()))
            .flatten()
            .unwrap_or_default()
    };
    if items.is_empty() {
        return;
    }
    let flow = if current.pushing {
        facing
    } else {
        facing.opposite()
    };
    let centre = [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + 0.5,
        f64::from(pos[2]) + 0.5,
    ];
    let max = |i: &str| game.items().get(i).map_or(64, |x| x.max_stack).clamp(1, 64) as u8;
    let mut rng =
        processing::Rng::new(game.world.game_time as u64 ^ (pos[0] as u64) << 16 ^ pos[2] as u64);
    // id → (motion, the stack it becomes when processed: None to discard).
    let mut changes: std::collections::HashMap<u32, ([f64; 3], Option<Option<(String, u8)>>)> =
        Default::default();
    let mut spawn = Vec::new();
    for (id, item, count, at, v) in items {
        if !caught.contains(&id) {
            continue;
        }
        let distance = ((at[0] - centre[0]).powi(2)
            + (at[1] - centre[1]).powi(2)
            + (at[2] - centre[2]).powi(2))
        .sqrt();
        let motion = fan::push(flow, speed, current.max, distance, v);
        let mut result = None;
        if let Some(kind) = current.type_at(fan::aligned_distance(at, pos, facing) as f32) {
            let mut p = kept
                .remove(&id)
                .or_else(|| old.remove(&id))
                .unwrap_or_default();
            let fire_resistant = game.items().get(&item).is_some_and(|i| i.fire_resistant);
            let count = count.min(255) as u8;
            let done = RECIPES.with_borrow(|r| {
                let r = r.as_ref()?;
                let can = fan::can_process(kind, &item, fire_resistant, r.fan());
                if !fan::entity_can(&p, kind, can)
                    || !fan::advance_entity(&mut p, kind, u32::from(count))
                {
                    return None;
                }
                fan::process(kind, &item, count, r.fan(), max, &mut rng)
            });
            kept.insert(id, p);
            if let Some(mut stacks) = done {
                if stacks.is_empty() {
                    result = Some(None);
                } else {
                    let first = stacks.remove(0);
                    for s in stacks {
                        spawn.push((at, motion, s));
                    }
                    result = Some(Some(first));
                }
            }
        }
        changes.insert(id, (motion, result));
    }
    game.native_items_in(dimension, lo, hi, |id, stack, _, v| {
        let Some((motion, result)) = changes.get(&id) else {
            return;
        };
        *v = *motion;
        match result {
            Some(Some((item, count))) => {
                stack.item = item.as_str().into();
                stack.count = u32::from(*count);
                stack.damage = 0;
                stack.extra = None;
            }
            Some(None) => stack.count = 0,
            None => {}
        }
    });
    for (at, v, (item, count)) in spawn {
        game.native_run_command(&format!(
            "summon minecraft:item {} {} {} {{Item:{{id:'{item}',count:{count}}},Motion:[{}d,{}d,{}d]}}",
            at[0], at[1], at[2], v[0], v[1], v[2]
        ));
    }
}

/// `AirCurrent.rebuild` and `findAffectedHandlers` for the fan at `pos`.
fn fan_current(
    game: &Game<'_>,
    dim: usize,
    pos: [i32; 3],
    facing: Dir,
    speed: f32,
) -> (
    create_engine::fan::Current,
    Vec<([i32; 3], Option<create_engine::fan::Kind>)>,
) {
    use create_engine::fan::{self, Current, Way};
    let o = facing.offset();
    let at = |i: i32| [pos[0] + o[0] * i, pos[1] + o[1] * i, pos[2] + o[2] * i];
    let b = game.res.behaviours();
    let transparent = b.tag("create:fan_transparent");
    let way = |i: i32| {
        let p = at(i);
        if !game.world.loaded(p[0], p[2]) {
            return Way::Unloaded;
        }
        let state = game.world.block(p[0], p[1], p[2]);
        if transparent.is_some_and(|t| t.contains(b.block(state))) {
            return Way::Open;
        }
        match b.collision_shape(state) {
            Some(shape) if shape.is_empty() => Way::Open,
            Some(shape) if shape.is_block() => Way::Full,
            Some(shape) => Way::Depth(max_depth(shape, facing)),
            None if b.facts(state).flags
                & horizon_world::level::data::state_flags::COLLISION_EMPTY
                != 0 =>
            {
                Way::Open
            }
            None => Way::Full,
        }
    };
    let max = fan::flow_limit(fan::max_distance(speed), way);
    let current = Current::build(max, fan::pushing(speed, facing), |i| {
        catalyst_at(game, at(i))
    });
    let mut handlers = Vec::new();
    for i in 1..=current.limit() {
        let segment = current.type_at((i - 1) as f32);
        for below in 0..2 {
            let p = at(i);
            let p = [p[0], p[1] - below, p[2]];
            let held = with_world(|w| {
                let d = &w.dims[dim];
                d.depots.contains_key(&p) || d.belt_of.contains_key(&p)
            });
            if held == Some(true) {
                handlers.push((p, catalyst_at(game, p).or(segment)));
            }
            if facing.axis() == Axis::Y {
                break;
            }
        }
    }
    (current, handlers)
}

/// `AirCurrent.findMaxDepth`: how far air gets into `shape` going `facing`, sampled at five
/// points of the face; infinite where a sample finds a hole.
fn max_depth(shape: &horizon_core::geometry::VoxelShape, facing: Dir) -> f32 {
    use horizon_core::geometry::Axis as A;
    let axis = match facing.axis() {
        Axis::X => A::X,
        Axis::Y => A::Y,
        Axis::Z => A::Z,
    };
    let positive = matches!(facing, Dir::East | Dir::Up | Dir::South);
    let mut depth = 0.0f64;
    for (a, b) in [
        (0.25, 0.25),
        (0.25, 0.75),
        (0.5, 0.5),
        (0.75, 0.25),
        (0.75, 0.75),
    ] {
        let d = if positive {
            let min = shape.min_at(axis, a, b);
            if min == f64::INFINITY {
                return f32::INFINITY;
            }
            min
        } else {
            let max = shape.max_at(axis, a, b);
            if max == f64::NEG_INFINITY {
                return f32::INFINITY;
            }
            1.0 - max
        };
        depth = depth.max(d);
    }
    depth as f32
}

/// `FanProcessingType.getAt` at a block: its fluid and the catalyst block tags with the state
/// properties they check (a campfire's `lit`, a blaze burner's `blaze` and `flame_type`).
fn catalyst_at(game: &Game<'_>, p: [i32; 3]) -> Option<create_engine::fan::Kind> {
    use create_engine::fan::{Catalyst, Heat, kind_at};
    use horizon_world::fluid::FluidKind;
    let b = game.res.behaviours();
    let state = game.world.block(p[0], p[1], p[2]);
    let fluid = b.fluid(state).kind();
    let block = b.block(state);
    let tagged = |t: &str| b.tag(t).is_some_and(|t| t.contains(block));
    let mut c = Catalyst {
        water: matches!(fluid, FluidKind::Water | FluidKind::FlowingWater),
        lava: matches!(fluid, FluidKind::Lava | FluidKind::FlowingLava),
        splashing: tagged("create:fan_processing_catalysts/splashing"),
        haunting: tagged("create:fan_processing_catalysts/haunting"),
        smoking: tagged("create:fan_processing_catalysts/smoking"),
        blasting: tagged("create:fan_processing_catalysts/blasting"),
        ..Default::default()
    };
    if !(c.haunting || c.smoking || c.blasting) {
        return kind_at(&c);
    }
    let Some(s) = game.pack.assets.registry.state(state) else {
        return kind_at(&c);
    };
    let prop = |k: &str| s.properties.get(k).map(String::as_str);
    if tagged("minecraft:campfires") {
        c.campfire_lit = prop("lit").map(|v| v == "true");
    }
    c.soul_flame = prop("flame_type").map(|v| v == "soul");
    c.heat = prop("blaze").and_then(|v| {
        Some(match v {
            "none" => Heat::None,
            "smouldering" => Heat::Smouldering,
            "fading" => Heat::Fading,
            "kindled" => Heat::Kindled,
            "seething" => Heat::Seething,
            _ => return None,
        })
    });
    kind_at(&c)
}

/// `DepotBehaviour.applyToAllItems` with `FanProcessing.applyProcessing`: the held stack's
/// progress, and when done, its results: the first held (centred), the rest in the outputs,
/// what finds no room dropped; none, and the stack is gone.
fn fan_on_depot(game: &mut Game<'_>, dim: usize, depot: [i32; 3], kind: create_engine::fan::Kind) {
    use create_engine::{fan, processing};
    let Some(Some(mut held)) = with_world(|w| w.dims[dim].depots.get(&depot).copied()).flatten()
    else {
        return;
    };
    let Some((_, item, count)) = depot_items(game, depot).into_iter().find(|s| s.0 == 0) else {
        return;
    };
    let fire_resistant = game.items().get(&item).is_some_and(|i| i.fire_resistant);
    let can = RECIPES.with_borrow(|r| {
        r.as_ref()
            .is_some_and(|r| fan::can_process(kind, &item, fire_resistant, r.fan()))
    });
    let done = fan::advance(&mut held.fan, kind, u32::from(count), can);
    if !done {
        with_world(|w| w.dims[dim].depots.insert(depot, Some(held)));
        return;
    }
    let mut rng = processing::Rng::new(
        game.world.game_time as u64
            ^ (depot[0] as u64).wrapping_mul(0x9E37_79B9)
            ^ (depot[2] as u64) << 20,
    );
    let max = |i: &str| game.items().get(i).map_or(64, |x| x.max_stack).clamp(1, 64) as u8;
    let results =
        RECIPES.with_borrow(|r| fan::process(kind, &item, count, r.as_ref()?.fan(), max, &mut rng));
    let Some(results) = results else {
        with_world(|w| w.dims[dim].depots.insert(depot, Some(held)));
        return;
    };
    let limits: Vec<u8> = results.iter().map(|(i, _)| max(i)).collect();
    let stack = |item: &str, count: u8| horizon_client::items::ItemStack {
        item: item.into(),
        count: u32::from(count),
        damage: 0,
        extra: None,
    };
    let dropped = game
        .native_with_mod_block_entity(depot, |be| {
            be.items
                .resize(be.items.len().max(create_engine::depot::SLOTS), None);
            be.items[0] = None;
            let mut outputs: Vec<processing::Slot> = be.items[create_engine::depot::OUTPUTS]
                .iter()
                .map(slot_of)
                .collect();
            let mut dropped = Vec::new();
            for ((i, n), limit) in results.iter().zip(&limits) {
                if be.items[0].is_none() {
                    be.items[0] = Some(stack(i, *n));
                    continue;
                }
                let left = processing::insert_stacked(&mut outputs, i, *n, *limit);
                if left > 0 {
                    dropped.push((i.clone(), left));
                }
            }
            for (slot, s) in be.items[create_engine::depot::OUTPUTS]
                .iter_mut()
                .zip(outputs)
            {
                *slot = s.map(|(i, n)| stack(&i, n));
            }
            dropped
        })
        .unwrap_or_default();
    for (i, n) in dropped {
        game.native_run_command(&format!(
            "summon minecraft:item {}.5 {}.0 {}.5 {{Item:{{id:'{i}',count:{n}}}}}",
            depot[0],
            depot[1] + 1,
            depot[2]
        ));
    }
    // getSimilar keeps the motion and the progress (done: -1, never again by this type).
    let next = (!results.is_empty()).then_some(create_engine::depot::Held {
        position: 0.5,
        ..held
    });
    with_world(|w| {
        w.dims[dim].depots.insert(depot, next);
        w.depot_changed(dim, depot);
    });
}

/// `BeltBlockEntity.applyToAllItems` with `FanProcessing.applyProcessing`: the items over the
/// segment at `at` (within .51 of its middle).
fn fan_on_belt(game: &mut Game<'_>, dim: usize, at: [i32; 3], kind: create_engine::fan::Kind) {
    use create_engine::{belt, fan, processing};
    let Some((c, index)) = with_world(|w| {
        let d = &w.dims[dim];
        let c = *d.belt_of.get(&at)?;
        let index = d.belts.get(&c)?.chain.iter().position(|p| *p == at)? as f32;
        let any = d.belts[&c]
            .items
            .items
            .iter()
            .any(|i| (index + 0.5 - i.position).abs() < 0.51);
        any.then_some((c, index))
    })
    .flatten() else {
        return;
    };
    let Some(mut inv) = with_world(|w| {
        w.dims[dim]
            .belts
            .get_mut(&c)
            .map(|b| std::mem::take(&mut b.items))
    })
    .flatten() else {
        return;
    };
    let mut rng = processing::Rng::new(
        game.world.game_time as u64
            ^ (at[0] as u64).wrapping_mul(0x9E37_79B9)
            ^ (at[2] as u64) << 20,
    );
    let max = |i: &str| game.items().get(i).map_or(64, |x| x.max_stack).clamp(1, 64) as u8;
    let mut made = 0;
    let dirty = RECIPES.with_borrow(|r| {
        let Some(r) = r.as_ref() else {
            return false;
        };
        inv.apply_within(index + 0.5, 0.51, |it| {
            let fire_resistant = game.items().get(&it.item).is_some_and(|i| i.fire_resistant);
            let can = fan::can_process(kind, &it.item, fire_resistant, r.fan());
            if !fan::advance(&mut it.fan, kind, u32::from(it.count), can) {
                return belt::Process::Pass;
            }
            let Some(results) = fan::process(kind, &it.item, it.count, r.fan(), max, &mut rng)
            else {
                return belt::Process::Pass;
            };
            let outputs = results
                .into_iter()
                .map(|(item, count)| {
                    // getSimilar: a new stack's angle (`R.nextInt(360)`), the old one's place
                    // and progress.
                    made += 1;
                    let angle = random_angle(game, [at[0] + made, at[1], at[2]]);
                    let mut out = belt::Item::new(&item, count, angle, upright(game, &item));
                    out.position = it.position;
                    out.inserted_at = it.inserted_at;
                    out.inserted_from = it.inserted_from;
                    out.fan = it.fan;
                    out
                })
                .collect();
            belt::Process::Convert {
                held: None,
                outputs,
            }
        })
    });
    with_world(|w| {
        if let Some(b) = w.dims[dim].belts.get_mut(&c) {
            b.items = inv;
        }
        if dirty {
            w.dims[dim].belt_changes.push(c);
            w.mark_unsaved(dim, c);
        }
    });
}

/// A block entity slot as the processing rules see it.
fn slot_of(s: &Option<horizon_client::items::ItemStack>) -> create_engine::processing::Slot {
    s.as_ref()
        .map(|s| (s.item.to_string(), s.count.min(255) as u8))
}

fn send_presses(game: &mut Game<'_>, players: &[u128], dim: usize, presses: &[[i32; 3]]) {
    let all: Vec<([i32; 3], create_engine::press::Press)> = presses
        .iter()
        .map(|&pos| {
            let p = with_world(|w| w.dims[dim].presses.get(&pos).copied())
                .flatten()
                .unwrap_or_default();
            (pos, p)
        })
        .collect();
    for m in create_engine::press::encode(&all) {
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::press::CHANNEL, &m);
        }
    }
}

/// Presses a client draws mid-cycle: their cycle, run on from the server's last word, the head
/// waiting at its lowest point until the server says it pressed (as Create's client does).
#[derive(Default)]
struct ClientPresses {
    running: HashMap<[i32; 3], (create_engine::press::Press, bool)>,
}

thread_local! {
    static PRESSES: RefCell<ClientPresses> = RefCell::new(ClientPresses::default());
}

/// Moves the press head at `pos` to where `p` has it, gliding to where it will be a tick on.
fn draw_press_head(game: &mut Game<'_>, pos: [i32; 3], p: &create_engine::press::Press, next: f32) {
    use create_engine::press::head_offset;
    let Some(model) = CLIENT.with_borrow(|c| {
        let c = c.as_ref()?;
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let (_, info) = c.table.get(state.0)?;
        let f = info.facing?;
        c.models.get(&(state.0, Model::PressHead(f))).copied()
    }) else {
        return;
    };
    let depth = p.mode.head_offset();
    let (now, to) = if p.running {
        (
            head_offset(p.ticks as f32) * depth,
            head_offset(next) * depth,
        )
    } else {
        (0.0, 0.0)
    };
    game.native_part_glide(
        client::key(pos, client::PRESS_HEAD_SLOT),
        model,
        pos,
        1,
        0.0,
        0.0,
        [0.0, -now, 0.0, game.native_parts_seconds()],
        [0.0, -(to - now) * 20.0, 0.0],
    );
}

/// The client's next cycle tick for a press at `speed`: `PressingBehaviour.tick` on the client,
/// which waits at the lowest point for the server.
fn client_press_step(p: &create_engine::press::Press, speed: f32, waiting: bool) -> i32 {
    use create_engine::press::CYCLE;
    if waiting && p.ticks >= CYCLE / 2 {
        return CYCLE / 2;
    }
    let next = p.ticks + create_engine::press::tick_speed(speed);
    if p.ticks < CYCLE / 2 && next >= CYCLE / 2 {
        return CYCLE / 2;
    }
    next
}

fn presses_from_server(game: &mut Game<'_>, data: &[u8]) {
    use create_engine::press::CYCLE;
    for (pos, p) in create_engine::press::decode(data) {
        // At the lowest point the server has pressed: the head goes on up.
        let waiting = p.running && p.ticks < CYCLE / 2;
        PRESSES.with_borrow_mut(|c| {
            if p.running {
                c.running.insert(pos, (p, waiting));
            } else {
                c.running.remove(&pos);
            }
        });
        let speed = press_speed(pos);
        let next = client_press_step(&p, speed, waiting) as f32;
        draw_press_head(game, pos, &p, next);
    }
}

fn press_speed(pos: [i32; 3]) -> f32 {
    CLIENT.with_borrow(|c| {
        c.as_ref()
            .and_then(|c| c.speeds.get(&pos))
            .map_or(0.0, |s| s[0])
    })
}

/// Each client tick: running presses move on a cycle step, drawn gliding to the next.
fn tick_client_presses(game: &mut Game<'_>) {
    let running: Vec<([i32; 3], create_engine::press::Press, bool)> =
        PRESSES.with_borrow(|c| c.running.iter().map(|(k, v)| (*k, v.0, v.1)).collect());
    for (pos, mut p, waiting) in running {
        let speed = press_speed(pos);
        p.ticks = client_press_step(&p, speed, waiting);
        if p.ticks > create_engine::press::CYCLE {
            p.running = false;
        }
        let next = client_press_step(&p, speed, waiting) as f32;
        draw_press_head(game, pos, &p, next);
        PRESSES.with_borrow_mut(|c| {
            if p.running {
                c.running.insert(pos, (p, waiting));
            } else {
                c.running.remove(&pos);
            }
        });
    }
}

/// The belt segment at `pos`: its controller, its index along the chain, and its state.
fn belt_segment(
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
fn use_belt(game: &mut Game<'_>, dim: usize, pos: [i32; 3], canceled: &mut bool) {
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

/// A belt's surroundings on the server: what lies past its end, and the press over a segment.
struct BeltSide<'a, 'g> {
    game: &'a mut Game<'g>,
    dim: usize,
    controller: [i32; 3],
    segment: create_engine::belt::Segment,
    length: i32,
    rpm: f32,
    positive: bool,
}

impl BeltSide<'_, '_> {
    fn past_end(&self) -> [i32; 3] {
        create_engine::belt::position_for_offset(
            self.controller,
            &self.segment,
            self.length,
            if self.positive { self.length } else { -1 },
        )
    }

    fn movement(&self) -> create_engine::kinetics::Dir {
        create_engine::belt::movement_facing(self.rpm, self.segment.facing)
    }

    /// `DirectBeltInputBehaviour.handleInsertion` at `next`, the item moving toward `side`: a
    /// depot or a belt there. What is left of it, None when nothing there takes items.
    fn insert_at(
        &mut self,
        next: [i32; 3],
        side: create_engine::kinetics::Dir,
        item: &create_engine::belt::Item,
    ) -> Option<u8> {
        let dim = self.dim;
        let is_depot = with_world(|w| w.dims[dim].depots.contains_key(&next)) == Some(true);
        if is_depot {
            // DepotBehaviour.tryInsertingFromSide: only into an empty depot.
            if !depot_items(self.game, next).is_empty() {
                return Some(item.count);
            }
            let put = horizon_client::items::ItemStack {
                item: item.item.as_str().into(),
                count: u32::from(item.count),
                damage: 0,
                extra: None,
            };
            self.game.native_with_mod_block_entity(next, |be| {
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
        let (c2, index, seg2) = belt_segment(self.game, dim, next)?;
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
fn eject_from_belt(
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
fn tick_belts(game: &mut Game<'_>, dimension: &str, dim: usize) {
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

fn send_belts(game: &mut Game<'_>, players: &[u128], dim: usize, belts: &[[i32; 3]]) {
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
struct ClientBelts {
    belts: HashMap<[i32; 3], (create_engine::belt::Contents, Vec<Shown>)>,
}

/// What was uploaded for an item: enough to tell, without looking anything up, whether it now
/// moves or looks differently.
#[derive(Clone, PartialEq)]
struct Shown {
    item: String,
    count: u8,
    angle: i32,
    flat: bool,
    velocity: [f32; 3],
    copies: u8,
}

thread_local! {
    static BELTS: RefCell<ClientBelts> = RefCell::new(ClientBelts::default());
    /// `isGui3d` by item id, looked up once.
    static BLOCK_ITEMS: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
}

fn is_block_item(game: &Game<'_>, item: &str) -> bool {
    if let Some(b) = BLOCK_ITEMS.with_borrow(|m| m.get(item).copied()) {
        return b;
    }
    let b = game.native_item_uses_block_light(item).unwrap_or(false);
    BLOCK_ITEMS.with_borrow_mut(|m| m.insert(item.to_owned(), b));
    b
}

/// A key for copy `copy` of item `index` on the belt at `controller`, apart from the others.
fn belt_key(controller: [i32; 3], index: usize, copy: u8) -> u64 {
    depot_key(controller, 200, copy) ^ (index as u64 + 1).wrapping_mul(0xD6E8_FEB8_6659_FD93)
        | 1 << 63
}

/// The client's belt surroundings: what the server said lies past the end; machines hold
/// locked items, nothing else acts.
struct ClientSide(create_engine::belt::Ending);

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
fn draw_belt(
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

fn belts_from_server(game: &mut Game<'_>, data: &[u8]) {
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

fn belt_rpm(controller: [i32; 3]) -> f32 {
    CLIENT.with_borrow(|c| {
        c.as_ref()
            .and_then(|c| c.speeds.get(&controller))
            .map_or(0.0, |s| s[0])
    })
}

/// Each client tick: belts with items move them on (`BeltInventory.tick` on the client).
fn tick_client_belts(game: &mut Game<'_>) {
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
fn latch_state(state: u32) -> Option<(create_engine::latch::Kind, create_engine::latch::Latch)> {
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

/// An andesite tunnel's axis and shape; None for any other block.
fn tunnel_state(
    state: u32,
) -> Option<(create_engine::kinetics::Axis, create_engine::tunnel::Shape)> {
    states::index(state)?;
    WORLD.with(|w| {
        let w = w.try_borrow().ok()?;
        let (block, info) = w.as_ref()?.table.get(state)?;
        (block == create_engine::Block::AndesiteTunnel).then_some((info.axis?, info.tunnel?))
    })
}

/// Andesite tunnels: those just placed case their belt (`BeltTunnelItem`); those whose
/// surroundings changed break without a cased, flat belt below (`canSurvive`) or take the shape
/// the belts and depots beside their segment give (`updateShape`, `getTunnelState`).
fn tick_tunnels(game: &mut Game<'_>, dimension: &str, dim: usize) {
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
fn link_state(state: u32) -> Option<(Dir, bool, bool)> {
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
fn link_power(game: &mut Game<'_>, pos: [i32; 3], facing: Dir) -> i32 {
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
fn tick_links(game: &mut Game<'_>, dimension: &str, dim: usize) {
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
fn send_links(game: &mut Game<'_>, players: &[u128], dim: usize, links: &[[i32; 3]]) {
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
fn links_from_server(game: &mut Game<'_>, data: &[u8]) {
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
fn toggle_link(game: &mut Game<'_>, dim: usize, pos: [i32; 3], receiver: bool) {
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
fn use_link(
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
fn set_latch(game: &mut Game<'_>, dimension: &str, pos: [i32; 3], l: create_engine::latch::Latch) {
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
fn latch_inputs(game: &mut Game<'_>, pos: [i32; 3], facing: Dir) -> (bool, bool) {
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
fn tick_latches(game: &mut Game<'_>, dimension: &str, dim: usize) {
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

fn tanks_from_nbt(
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
fn use_tank(game: &mut Game<'_>, dim: usize, pos: [i32; 3], canceled: &mut bool) {
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

fn send_tank_changes(game: &mut Game<'_>, dimension: &str, dim: usize) {
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

fn send_tanks(game: &mut Game<'_>, players: &[u128], dim: usize, tanks: &[[i32; 3]]) {
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
fn tanks_from_server(game: &mut Game<'_>, data: &[u8]) {
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

/// `WindmillBearingBlock.useItemOn` with an empty hand, not sneaking: assembled sails come
/// apart now; otherwise they assemble on the next tick (`assembleNextTick`).
fn use_windmill(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    sneaking: bool,
    canceled: &mut bool,
) {
    if sneaking {
        *canceled = true;
        return;
    }
    if game.native_held().is_some_and(|h| h.0 != "minecraft:air") {
        return;
    }
    *canceled = true;
    let running = with_world(|w| w.dims[dim].windmills.contains_key(&pos)) == Some(true);
    if running {
        let dimension = game.dim.id().to_owned();
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        if let Some(w) = with_world(|w| w.windmill_disassembled(dim, pos, state)).flatten() {
            put_back(game, &dimension, pos, &w);
        }
    } else {
        with_world(|w| {
            if !w.dims[dim].windmill_assemble.contains(&pos) {
                w.dims[dim].windmill_assemble.push(pos);
            }
        });
    }
}

/// `ControlledContraptionEntity.disassemble` at angle 0 (a windmill turns back first): each
/// block where it was, or dropped where something else took its place.
fn put_back(
    game: &mut Game<'_>,
    dimension: &str,
    bearing: [i32; 3],
    w: &create_engine::world::Windmill,
) {
    let registry = game.pack.assets.registry;
    for (o, state) in &w.blocks {
        let p = [bearing[0] + o[0], bearing[1] + o[1], bearing[2] + o[2]];
        let here = game.world.block(p[0], p[1], p[2]);
        let free = registry
            .state(here)
            .is_some_and(|s| s.name == "minecraft:air" || s.name == "minecraft:cave_air");
        if free {
            game.native_set_block(
                dimension,
                p,
                BlockStateId(*state),
                super::native_hooks::flags::UPDATE_ALL,
            );
        } else if let Some(s) = registry.state(BlockStateId(*state)) {
            game.native_run_command(&format!(
                "execute in {dimension} run summon minecraft:item {}.5 {}.5 {}.5 {{Item:{{id:'{}',count:1}}}}",
                p[0], p[1], p[2], s.name
            ));
        }
    }
}

/// Blocks Create can't move (unbreakable ones) and blocks whose block entity would be lost
/// (moving block entities isn't ported: refused rather than emptied).
fn contraption_found(game: &Game<'_>, p: [i32; 3]) -> Option<create_engine::contraption::Found> {
    use create_engine::contraption::{Found, Sticky};
    let registry = game.pack.assets.registry;
    let id = game.world.block(p[0], p[1], p[2]);
    let s = registry.state(id)?;
    if matches!(
        s.name.as_str(),
        "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:water"
            | "minecraft:lava"
    ) {
        return None;
    }
    let sail = s.name == "create:sail_frame"
        || (s.name.starts_with("create:") && s.name.ends_with("_sail"));
    let sail_facing = sail
        .then(|| {
            s.properties
                .get("facing")
                .and_then(|f| Dir::ALL.into_iter().find(|d| dir_name(*d) == f))
        })
        .flatten();
    let unmovable = matches!(
        s.name.as_str(),
        "minecraft:bedrock"
            | "minecraft:barrier"
            | "minecraft:end_portal_frame"
            | "minecraft:command_block"
            | "minecraft:chain_command_block"
            | "minecraft:repeating_command_block"
            | "minecraft:structure_block"
            | "minecraft:jigsaw"
    ) || game.world.block_entities.contains_key(&(p[0], p[1], p[2]));
    Some(Found {
        state: id.0,
        sail_facing,
        sticky: match s.name.as_str() {
            "minecraft:slime_block" => Some(Sticky::Slime),
            "minecraft:honey_block" => Some(Sticky::Honey),
            _ => None,
        },
        movable: !unmovable,
        // LinearChassisBlock: its kind, axis and glued ends.
        chassis: matches!(
            s.name.as_str(),
            "create:linear_chassis" | "create:secondary_linear_chassis"
        )
        .then(|| create_engine::contraption::Chassis {
            secondary: s.name == "create:secondary_linear_chassis",
            axis: match s.properties.get("axis").map(String::as_str) {
                Some("x") => Axis::X,
                Some("z") => Axis::Z,
                _ => Axis::Y,
            },
            sticky_top: s.properties.get("sticky_top").is_some_and(|v| v == "true"),
            sticky_bottom: s
                .properties
                .get("sticky_bottom")
                .is_some_and(|v| v == "true"),
            range: create_engine::contraption::CHASSIS_RANGE,
        }),
    })
}

/// Windmills: structures whose bearing broke go back; right-clicked bearings assemble their
/// sails (`assemble`), the blocks leaving the world; clients see the changes. Nothing runs
/// without either.
fn tick_windmills(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let Some((orphans, queued)) = with_world(|w| {
        let d = &mut w.dims[dim];
        (
            std::mem::take(&mut d.windmill_orphans),
            std::mem::take(&mut d.windmill_assemble),
        )
    }) else {
        return;
    };
    for (pos, w) in orphans {
        put_back(game, dimension, pos, &w);
    }
    for pos in queued {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let facing = with_world(|w| {
            w.table
                .get(state)
                .filter(|(b, _)| *b == create_engine::Block::WindmillBearing)
                .and_then(|(_, i)| i.facing)
        })
        .flatten();
        let Some(facing) = facing else {
            continue;
        };
        let gathered = create_engine::contraption::gather_windmill(pos, facing, |p| {
            contraption_found(game, p)
        });
        let Ok((blocks, sails)) = gathered else {
            continue;
        };
        if let Some(air) = game.pack.assets.registry.default_state("minecraft:air") {
            for (o, _) in &blocks {
                let p = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
                game.native_set_block(
                    dimension,
                    p,
                    air,
                    super::native_hooks::flags::UPDATE_CLIENTS,
                );
            }
        }
        let w = create_engine::world::Windmill {
            blocks,
            sails,
            clockwise: true,
            bearing: None,
        };
        with_world(|world| world.windmill_assembled(dim, pos, state, w));
    }
    let changed = with_world(|w| {
        let mut v = std::mem::take(&mut w.dims[dim].windmill_changes);
        v.sort();
        v.dedup();
        v
    })
    .unwrap_or_default();
    if !changed.is_empty() {
        let players = players_in(game, dimension);
        send_windmills(game, &players, dim, &changed);
    }
}

/// Mechanical bearings whose speed changed (`tick` after `onSpeedChanged`): a still one starts
/// turning what is in front of it (`assemble`); a turning one stopped puts it back turned to the
/// nearest quarter turn (`disassemble`, `ROTATE_PLACE`); otherwise the angle runs on at the new
/// speed. Nothing runs without a change.
fn tick_bearings(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::contraption::{self, Turn};
    let Some(mut checks) = with_world(|w| std::mem::take(&mut w.dims[dim].bearing_checks)) else {
        return;
    };
    if checks.is_empty() {
        return;
    }
    checks.sort();
    checks.dedup();
    let now = game.world.game_time;
    for pos in checks {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let facing = with_world(|w| {
            w.table
                .get(state)
                .filter(|(b, _)| *b == create_engine::Block::MechanicalBearing)
                .and_then(|(_, i)| i.facing)
        })
        .flatten();
        let Some(facing) = facing else {
            continue;
        };
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let running = with_world(|w| w.dims[dim].windmills.get(&pos).map(|m| m.bearing)).flatten();
        match running {
            Some(Some(turn)) if speed == 0.0 => {
                let quarter = contraption::quarter(turn.angle_at(now));
                if let Some(w) = with_world(|w| w.windmill_disassembled(dim, pos, state)).flatten()
                {
                    put_back_turned(game, dimension, pos, facing, quarter, &w);
                }
            }
            Some(Some(mut turn)) => {
                turn.rebase(now, speed);
                with_world(|w| {
                    if let Some(m) = w.dims[dim].windmills.get_mut(&pos) {
                        m.bearing = Some(turn);
                    }
                    w.dims[dim].windmill_changes.push(pos);
                });
            }
            Some(None) => {}
            None if speed != 0.0 => {
                let gathered =
                    contraption::gather_bearing(pos, facing, 0, |p| contraption_found(game, p));
                let Ok((blocks, sails)) = gathered else {
                    continue;
                };
                if let Some(air) = game.pack.assets.registry.default_state("minecraft:air") {
                    for (o, _) in &blocks {
                        let p = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
                        game.native_set_block(
                            dimension,
                            p,
                            air,
                            super::native_hooks::flags::UPDATE_CLIENTS,
                        );
                    }
                }
                let w = create_engine::world::Windmill {
                    blocks,
                    sails,
                    clockwise: true,
                    bearing: Some(Turn {
                        angle: 0.0,
                        since: now,
                        rpm: speed,
                    }),
                };
                with_world(|world| world.bearing_assembled(dim, pos, w));
            }
            None => {}
        }
    }
}

/// `ControlledContraptionEntity.disassemble` with its `StructureTransform`: each block turned
/// `quarter`° about the bearing's axis through the block in front of it, its `facing` or `axis`
/// turned with it (`rotateFacing`: `getClockWise(axis)`, `rotation.ordinal()` times); a state
/// that has no such turned form stays as it was. A block whose place is taken drops.
fn put_back_turned(
    game: &mut Game<'_>,
    dimension: &str,
    bearing: [i32; 3],
    facing: Dir,
    quarter: i32,
    w: &create_engine::world::Windmill,
) {
    use create_engine::contraption;
    let axis = facing.axis();
    let turns = contraption::clockwise_turns(quarter);
    let registry = game.pack.assets.registry;
    let f = facing.offset();
    let anchor = [bearing[0] + f[0], bearing[1] + f[1], bearing[2] + f[2]];
    let turned: Vec<([i32; 3], u32)> = w
        .blocks
        .iter()
        .map(|(o, state)| {
            let local = [o[0] - f[0], o[1] - f[1], o[2] - f[2]];
            let r = contraption::rotate(local, quarter, axis);
            let p = [anchor[0] + r[0], anchor[1] + r[1], anchor[2] + r[2]];
            let mut id = *state;
            if turns > 0
                && let Some(mut s) = registry.state(BlockStateId(*state)).cloned()
            {
                let turn_dir =
                    |d: Dir| (0..turns).fold(d, |d, _| create_engine::wrench::clockwise(d, axis));
                if let Some(d) = s
                    .properties
                    .get("facing")
                    .and_then(|v| Dir::ALL.into_iter().find(|d| dir_name(*d) == v))
                {
                    s.properties
                        .insert("facing".into(), dir_name(turn_dir(d)).into());
                } else if let Some(a) = s.properties.get("axis").cloned() {
                    let d = match a.as_str() {
                        "x" => Dir::East,
                        "y" => Dir::Up,
                        _ => Dir::South,
                    };
                    let name = match turn_dir(d).axis() {
                        Axis::X => "x",
                        Axis::Y => "y",
                        Axis::Z => "z",
                    };
                    s.properties.insert("axis".into(), name.into());
                }
                if let Some(t) = registry.id(&s) {
                    id = t.0;
                }
            }
            (p, id)
        })
        .collect();
    for (p, id) in turned {
        let here = game.world.block(p[0], p[1], p[2]);
        let free = registry
            .state(here)
            .is_some_and(|s| s.name == "minecraft:air" || s.name == "minecraft:cave_air");
        if free {
            game.native_set_block(
                dimension,
                p,
                BlockStateId(id),
                super::native_hooks::flags::UPDATE_ALL,
            );
        } else if let Some(s) = registry.state(BlockStateId(id)) {
            game.native_run_command(&format!(
                "execute in {dimension} run summon minecraft:item {}.5 {}.5 {}.5 {{Item:{{id:'{}',count:1}}}}",
                p[0], p[1], p[2], s.name
            ));
        }
    }
}

/// A gantry shaft's facing, part and power; None for any other block.
fn gantry_shaft(game: &Game<'_>, p: [i32; 3]) -> Option<(Dir, create_engine::gantry::Part, bool)> {
    let s = game
        .pack
        .assets
        .registry
        .state(game.world.block(p[0], p[1], p[2]))?;
    if s.name != "create:gantry_shaft" {
        return None;
    }
    let facing = s
        .properties
        .get("facing")
        .and_then(|v| Dir::ALL.into_iter().find(|d| dir_name(*d) == v))?;
    let part = create_engine::gantry::Part::read(s.properties.get("part")?)?;
    Some((
        facing,
        part,
        s.properties.get("powered").is_some_and(|v| v == "true"),
    ))
}

/// `checkAttachedCarriageBlocks`: carriages beside the shaft facing away from it, off its axis.
fn gantry_carriages_on(game: &Game<'_>, dim: usize, shaft: [i32; 3]) {
    let Some((facing, ..)) = gantry_shaft(game, shaft) else {
        return;
    };
    let registry = game.pack.assets.registry;
    for d in Dir::ALL {
        if d.axis() == facing.axis() {
            continue;
        }
        let o = d.offset();
        let c = [shaft[0] + o[0], shaft[1] + o[1], shaft[2] + o[2]];
        let carriage = registry
            .state(game.world.block(c[0], c[1], c[2]))
            .is_some_and(|s| {
                s.name == "create:gantry_carriage"
                    && s.properties.get("facing").map(String::as_str) == Some(dir_name(d))
            });
        if carriage {
            with_world(|w| w.dims[dim].gantry_assemble.push(c));
        }
    }
}

/// Gantries: shafts take their place in their line (`updateShape`); queued carriages on a shaft
/// they may start from take their structure out of the world (`tryAssemble`); moving ones run
/// along the shafts at their speed (`checkPinionShaft`) and go back into the world, at the
/// nearest whole block, where they stop. Nothing runs without one of them.
fn tick_gantries(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::gantry::{self, Moving, Shaft, Step};
    let Some((mut checks, mut queued, moving)) = with_world(|w| {
        let d = &mut w.dims[dim];
        (
            std::mem::take(&mut d.gantry_checks),
            std::mem::take(&mut d.gantry_assemble),
            !d.gantries.is_empty(),
        )
    }) else {
        return;
    };
    if checks.is_empty() && queued.is_empty() && !moving {
        return;
    }
    checks.sort();
    checks.dedup();
    for pos in checks {
        let Some((facing, part, _)) = gantry_shaft(game, pos) else {
            continue;
        };
        let along = |d: Dir| {
            let o = d.offset();
            gantry_shaft(game, [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]])
                .is_some_and(|(f, ..)| f == facing)
        };
        let now = gantry::Part::of(along(facing), along(facing.opposite()));
        if now != part {
            game.native_set_block_property_with(
                pos,
                "part",
                now.name(),
                super::native_hooks::flags::UPDATE_ALL,
            );
        }
    }
    queued.sort();
    queued.dedup();
    for c in queued {
        let state = game
            .pack
            .assets
            .registry
            .state(game.world.block(c[0], c[1], c[2]));
        let Some(cf) = state
            .filter(|s| s.name == "create:gantry_carriage")
            .and_then(|s| s.properties.get("facing"))
            .and_then(|v| Dir::ALL.into_iter().find(|d| dir_name(*d) == v))
        else {
            continue;
        };
        let b = cf.opposite().offset();
        let shaft = [c[0] + b[0], c[1] + b[1], c[2] + b[2]];
        let Some((sf, part, powered)) = gantry_shaft(game, shaft) else {
            continue;
        };
        let rpm = with_world(|w| w.speed(dim, shaft)).flatten().unwrap_or(0.0);
        let v = gantry::pinion_speed(rpm);
        if !gantry::can_assemble_on(part, powered, v) {
            continue;
        }
        let moving_dir = if v > 0.0 { sf } else { sf.opposite() };
        let gathered = create_engine::contraption::gather_gantry(c, cf, moving_dir, |p| {
            contraption_found(game, p)
        });
        let Ok(blocks) = gathered else {
            continue;
        };
        if let Some(air) = game.pack.assets.registry.default_state("minecraft:air") {
            for (o, _) in &blocks {
                let p = [c[0] + o[0], c[1] + o[1], c[2] + o[2]];
                game.native_set_block(
                    dimension,
                    p,
                    air,
                    super::native_hooks::flags::UPDATE_CLIENTS,
                );
            }
        }
        with_world(|w| {
            w.dims[dim].gantries.push(Moving {
                start: c,
                carriage_facing: cf,
                shaft_facing: sf,
                offset: 0.0,
                per_tick: 0.0,
                blocks,
            })
        });
    }
    let list = with_world(|w| std::mem::take(&mut w.dims[dim].gantries)).unwrap_or_default();
    let mut keep = Vec::new();
    for mut g in list {
        let sp = g.shaft_pos();
        let shaft = gantry_shaft(game, sp).map(|(facing, part, powered)| Shaft {
            facing,
            part,
            powered,
            speed: with_world(|w| w.speed(dim, sp)).flatten().unwrap_or(0.0),
        });
        match g.step(shaft) {
            Step::Move(v) => {
                if v != g.per_tick {
                    g.per_tick = v;
                    send_gantry(game, dimension, &g, false);
                }
                g.offset += v;
                keep.push(g);
            }
            Step::Stop => {
                // `disassemble` at `BlockPos.containing(anchor + .5)`.
                let at = g.carriage();
                let shift = [at[0] - g.start[0], at[1] - g.start[1], at[2] - g.start[2]];
                let registry = game.pack.assets.registry;
                for (o, id) in &g.blocks {
                    let p = [
                        g.start[0] + o[0] + shift[0],
                        g.start[1] + o[1] + shift[1],
                        g.start[2] + o[2] + shift[2],
                    ];
                    let here = game.world.block(p[0], p[1], p[2]);
                    let free = registry.state(here).is_some_and(|s| {
                        s.name == "minecraft:air" || s.name == "minecraft:cave_air"
                    });
                    if free {
                        game.native_set_block(
                            dimension,
                            p,
                            BlockStateId(*id),
                            super::native_hooks::flags::UPDATE_ALL,
                        );
                    } else if let Some(s) = registry.state(BlockStateId(*id)) {
                        game.native_run_command(&format!(
                            "execute in {dimension} run summon minecraft:item {}.5 {}.5 {}.5 {{Item:{{id:'{}',count:1}}}}",
                            p[0], p[1], p[2], s.name
                        ));
                    }
                }
                send_gantry(game, dimension, &g, true);
            }
        }
    }
    with_world(|w| {
        let d = &mut w.dims[dim];
        keep.extend(std::mem::take(&mut d.gantries));
        d.gantries = keep;
    });
}

/// A moving gantry to clients, as a piston's structure is (`create:piston`, keyed by where its
/// carriage started): gliding along the shaft line from its offset at its speed; `gone` clears
/// it.
fn send_gantry(
    game: &mut Game<'_>,
    dimension: &str,
    g: &create_engine::gantry::Moving,
    gone: bool,
) {
    let mut msg = Vec::new();
    for v in g.start {
        msg.extend_from_slice(&v.to_le_bytes());
    }
    if !gone {
        for v in g.start {
            msg.extend_from_slice(&v.to_le_bytes());
        }
        msg.push(g.shaft_facing as u8);
        msg.extend_from_slice(&g.offset.to_le_bytes());
        msg.extend_from_slice(&g.per_tick.to_le_bytes());
        let blocks = &g.blocks[..g.blocks.len().min((30_000 - 33) / 16)];
        msg.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
        for (o, s) in blocks {
            for v in o {
                msg.extend_from_slice(&v.to_le_bytes());
            }
            msg.extend_from_slice(&s.to_le_bytes());
        }
    }
    for p in players_in(game, dimension) {
        let _ = game.native_net_send_to_player(p, PISTON_CHANNEL, &msg);
    }
}

const CONTRAPTION_CHANNEL: &str = "create:contraption";
const PISTON_CHANNEL: &str = "create:piston";

/// A block's world position on a piston's structure at `offset` blocks out.
fn piston_world(anchor: [i32; 3], local: [i32; 3], facing: Dir, offset: i32) -> [i32; 3] {
    let o = facing.offset();
    [0, 1, 2].map(|a| anchor[a] + local[a] + o[a] * offset)
}

const TRAIN_CHANNEL: &str = "create:train";

/// The straight track a station targets (`TrackTargetingBehaviour`): the track block beside it,
/// and the line through it.
fn station_track(
    game: &Game<'_>,
    pos: [i32; 3],
) -> Option<([i32; 3], create_engine::trains::Line)> {
    use create_engine::trains::Line;
    let registry = game.pack.assets.registry;
    let axis_at = |p: [i32; 3]| -> Option<Axis> {
        let s = registry.state(game.world.block(p[0], p[1], p[2]))?;
        if s.name != "create:track" {
            return None;
        }
        match s.properties.get("shape").map(String::as_str) {
            Some("zo") => Some(Axis::Z),
            Some("xo") => Some(Axis::X),
            _ => None,
        }
    };
    for d in [Dir::North, Dir::South, Dir::West, Dir::East] {
        let o = d.offset();
        let t = [pos[0] + o[0], pos[1], pos[2] + o[2]];
        if let Some(axis) = axis_at(t) {
            return Some((t, Line::trace(t, axis, |p| axis_at(p) == Some(axis))));
        }
    }
    None
}

/// The station's assembly (`StationBlockEntity.assemble` / `disassemble` from its screen):
/// a train standing at it goes back into the world; else the bogeys on its track (a block above
/// it) and what sticks to them become a train.
fn use_station(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    use create_engine::trains::{Line, Train};
    let Some((target, line)) = station_track(game, pos) else {
        return;
    };
    let here = Line::along(line.axis, target) as f64;
    let dimension = game.dim.id().to_owned();
    let stopped = with_world(|w| {
        let d = &mut w.dims[dim];
        let i = d.trains.iter().position(|t| {
            t.train.line == line
                && t.train.destination.is_none()
                && (t.train.position - here).abs() < 0.5
        })?;
        Some(d.trains.remove(i))
    })
    .flatten();
    if let Some(t) = stopped {
        let shift = (t.train.position - t.start).round() as i32;
        let along = |p: [i32; 3]| {
            let mut p = p;
            match line.axis {
                Axis::X => p[0] += shift,
                Axis::Y => p[1] += shift,
                Axis::Z => p[2] += shift,
            }
            p
        };
        for (l, s) in &t.blocks {
            let p = along([t.origin[0] + l[0], t.origin[1] + l[1], t.origin[2] + l[2]]);
            game.native_set_block(
                &dimension,
                p,
                BlockStateId(*s),
                super::native_hooks::flags::UPDATE_ALL,
            );
        }
        send_train_clear(game, &dimension, t.id);
        return;
    }
    // Bogeys along the track, one above it.
    let step = |a: i32| match line.axis {
        Axis::X => [a, target[1] + 1, target[2]],
        Axis::Y => [target[0], a, target[2]],
        Axis::Z => [target[0], target[1] + 1, a],
    };
    let bogeys: Vec<[i32; 3]> = (line.from..=line.to)
        .map(step)
        .filter(|p| {
            let s = game.world.block(p[0], p[1], p[2]);
            matches!(
                with_world(|w| w.table.get(s.0).map(|(b, _)| b)).flatten(),
                Some(create_engine::Block::SmallBogey | create_engine::Block::LargeBogey)
            )
        })
        .collect();
    let Some(&origin) = bogeys.first() else {
        return;
    };
    // The bogeys and what is stuck to them (slime, honey; glue is not ported), not the track.
    let track: Vec<[i32; 3]> = (line.from..=line.to)
        .map(|a| [step(a)[0], target[1], step(a)[2]])
        .collect();
    let mut blocks: Vec<([i32; 3], u32)> = Vec::new();
    for b in &bogeys {
        // Down is the track, left out: only what sticks is taken.
        let Ok(found) =
            create_engine::piston::gather(*b, Dir::Down, &track, |p| contraption_found(game, p))
        else {
            return;
        };
        for (o, s) in found {
            let l = [
                b[0] + o[0] - origin[0],
                b[1] + o[1] - origin[1],
                b[2] + o[2] - origin[2],
            ];
            if !blocks.iter().any(|(x, _)| *x == l) {
                blocks.push((l, s));
            }
        }
    }
    let air = BlockStateId(0);
    for (l, _) in &blocks {
        let p = [origin[0] + l[0], origin[1] + l[1], origin[2] + l[2]];
        game.native_set_block(&dimension, p, air, super::native_hooks::flags::UPDATE_ALL);
    }
    let start = Line::along(line.axis, origin) as f64;
    let id = with_world(|w| {
        let d = &mut w.dims[dim];
        let id = d.trains.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        d.trains.push(create_engine::world::TrainOnTrack {
            train: Train {
                line,
                position: start,
                speed: 0.0,
                destination: None,
            },
            blocks,
            origin,
            start,
            id,
        });
        id
    })
    .unwrap_or(0);
    game.native_mark_chunk_unsaved(&dimension, [origin[0] >> 4, origin[2] >> 4]);
    send_train(game, dim, [i32::MIN, 0, id as i32]);
}

/// `create_debug train <station>`: the train on that station's track heads for it. With a
/// position of `[i32::MIN, 0, id]`, sends train `id` to clients as it is.
fn send_train(game: &mut Game<'_>, dim: usize, station: [i32; 3]) -> String {
    use create_engine::trains::Line;
    let id = if station[0] == i32::MIN {
        Some(station[2] as u32)
    } else {
        let Some((target, line)) = station_track(game, station) else {
            return "no track beside that station".into();
        };
        let dest = Line::along(line.axis, target) as f64;
        with_world(|w| {
            let t = w.dims[dim]
                .trains
                .iter_mut()
                .find(|t| t.train.line == line)?;
            t.train.destination = Some(dest);
            Some(t.id)
        })
        .flatten()
    };
    let Some(id) = id else {
        return "no train on that track".into();
    };
    let Some(t) = with_world(|w| w.dims[dim].trains.iter().find(|t| t.id == id).cloned()).flatten()
    else {
        return "no such train".into();
    };
    let mut msg = Vec::new();
    msg.extend_from_slice(&t.id.to_le_bytes());
    for v in t.origin {
        msg.extend_from_slice(&v.to_le_bytes());
    }
    msg.push(axis_index(t.train.line.axis));
    msg.extend_from_slice(&((t.train.position - t.start) as f32).to_le_bytes());
    msg.extend_from_slice(&(t.train.speed as f32).to_le_bytes());
    let blocks = &t.blocks[..t.blocks.len().min((30_000 - 29) / 16)];
    msg.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    for (o, s) in blocks {
        for v in o {
            msg.extend_from_slice(&v.to_le_bytes());
        }
        msg.extend_from_slice(&s.to_le_bytes());
    }
    let dimension = game.dim.id().to_owned();
    for p in players_in(game, &dimension) {
        let _ = game.native_net_send_to_player(p, TRAIN_CHANNEL, &msg);
    }
    format!(
        "train {id} heading for {:.1}",
        t.train.destination.unwrap_or(t.train.position)
    )
}

fn send_train_clear(game: &mut Game<'_>, dimension: &str, id: u32) {
    for p in players_in(game, dimension) {
        let _ = game.native_net_send_to_player(p, TRAIN_CHANNEL, &id.to_le_bytes());
    }
}

/// Trains running to a destination (`Train.tick`, `Navigation.tick`); clients get each one's
/// motion when it starts, every ten ticks while its speed changes, and when it stops.
fn tick_trains(game: &mut Game<'_>, _dimension: &str, dim: usize) {
    use create_engine::trains::Run;
    let running: Vec<(u32, Run, f64, (i32, i32), (i32, i32))> = with_world(|w| {
        w.dims[dim]
            .trains
            .iter_mut()
            .filter(|t| t.train.destination.is_some())
            .map(|t| {
                let before = t.train.speed;
                let was = create_engine::world::CreateWorld::train_column(t);
                let run = t.train.tick();
                (
                    t.id,
                    run,
                    (t.train.speed - before).abs(),
                    was,
                    create_engine::world::CreateWorld::train_column(t),
                )
            })
            .collect()
    })
    .unwrap_or_default();
    // Saved with the column it stands in: the one it left and the one it is in save again.
    for (_, _, _, was, now) in &running {
        if was != now {
            game.native_mark_chunk_unsaved(_dimension, [was.0, was.1]);
            game.native_mark_chunk_unsaved(_dimension, [now.0, now.1]);
        }
    }
    let running: Vec<(u32, create_engine::trains::Run, f64)> = running
        .into_iter()
        .map(|(id, run, change, _, now)| {
            if run == create_engine::trains::Run::Arrived {
                game.native_mark_chunk_unsaved(_dimension, [now.0, now.1]);
            }
            (id, run, change)
        })
        .collect();
    let tick = game.world.game_time;
    for (id, run, change) in running {
        if run == Run::Arrived || (change > 0.0 && tick % 10 == 0) {
            send_train(game, dim, [i32::MIN, 0, id as i32]);
        }
    }
}

thread_local! {
    /// Trains a client draws, by id: their part model.
    static TRAINS: RefCell<HashMap<u32, u32>> = RefCell::new(HashMap::new());
}

/// A train on the client: one part model gliding along its track at its speed from where it is
/// now (`CarriageContraptionEntity`'s motion), until the next message.
fn trains_from_server(game: &mut Game<'_>, d: &[u8]) {
    let i = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    if d.len() < 4 {
        return;
    }
    let id = u32::from_le_bytes(d[0..4].try_into().unwrap());
    let key = depot_key([id as i32, 0, 0], 222, 0);
    if d.len() < 29 {
        if let Some(old) = TRAINS.with_borrow_mut(|t| t.remove(&id)) {
            game.native_part_clear(key);
            game.native_part_model_free(old);
        }
        return;
    }
    let origin = [i(&d[4..8]), i(&d[8..12]), i(&d[12..16])];
    let axis = d[16].min(2) as usize;
    let f = |b: &[u8]| f32::from_le_bytes(b.try_into().unwrap());
    let (offset, speed) = (f(&d[17..21]), f(&d[21..25]));
    let n = u32::from_le_bytes(d[25..29].try_into().unwrap()) as usize;
    let model = match TRAINS.with_borrow(|t| t.get(&id).copied()) {
        Some(m) => m,
        None => {
            let blocks: Vec<(BlockStateId, glam::Mat4)> = d[29..]
                .chunks_exact(16)
                .take(n)
                .map(|r| {
                    let o = glam::Vec3::new(
                        i(&r[0..4]) as f32,
                        i(&r[4..8]) as f32,
                        i(&r[8..12]) as f32,
                    );
                    (
                        BlockStateId(u32::from_le_bytes(r[12..16].try_into().unwrap())),
                        glam::Mat4::from_translation(o),
                    )
                })
                .collect();
            let Some(m) = game.native_part_model_compound(&blocks) else {
                return;
            };
            TRAINS.with_borrow_mut(|t| t.insert(id, m));
            m
        }
    };
    let mut at = [0.0f32; 4];
    let mut v = [0.0f32; 3];
    at[axis] = offset;
    at[3] = game.native_parts_seconds();
    v[axis] = speed * 20.0;
    game.native_part_glide(key, model, origin, 0, 0.0, 0.0, at, v);
}

/// Mechanical pistons (`LinearActuatorBlockEntity.tick`): a speed change makes a still piston
/// assemble or a moving one stop (`assembleNextTick`); moving ones slide `getMovementSpeed`
/// a tick and are put back at their end (`tryDisassemble`). Clients glide the structure.
fn tick_pistons(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].piston_checks)).unwrap_or_default();
    let moving: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].pistons.keys().copied().collect()).unwrap_or_default();
    if checks.is_empty() && moving.is_empty() {
        return;
    }
    checks.sort();
    checks.dedup();
    for pos in checks {
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let running = with_world(|w| w.dims[dim].pistons.contains_key(&pos)).unwrap_or(false);
        match (running, speed != 0.0) {
            (false, true) => assemble_piston(game, dimension, dim, pos, speed),
            (true, false) => disassemble_piston(game, dimension, dim, pos),
            (true, true) => send_piston(game, dimension, dim, pos),
            _ => {}
        }
    }
    for pos in moving {
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let Some(facing) =
            with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten()
        else {
            continue;
        };
        let end = with_world(|w| {
            let m = w.dims[dim].pistons.get_mut(&pos)?;
            m.offset += create_engine::piston::movement(speed, facing, m.offset, m.range);
            Some(m.offset <= 0.0 || m.offset >= m.range as f32)
        })
        .flatten();
        if end == Some(true) {
            disassemble_piston(game, dimension, dim, pos);
        }
    }
}

/// `MechanicalPistonBlockEntity.assemble`: the head, poles and structure leave the world.
fn assemble_piston(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3], speed: f32) {
    use create_engine::piston::{self, PoleCell};
    let registry = game.pack.assets.registry;
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some((block, facing)) =
        with_world(|w| w.table.get(state.0).map(|(b, i)| (b, i.facing))).flatten()
    else {
        return;
    };
    let Some(facing) = facing else {
        return;
    };
    let extended = registry
        .state(state)
        .is_some_and(|s| s.properties.get("state").is_some_and(|v| v == "extended"));
    let cell = |p: [i32; 3]| {
        let s = game.world.block(p[0], p[1], p[2]);
        match with_world(|w| w.table.get(s.0).map(|(b, i)| (b, i.facing))).flatten() {
            Some((create_engine::Block::PistonExtensionPole, Some(f))) => PoleCell::Pole(f.axis()),
            Some((create_engine::Block::MechanicalPistonHead, Some(f))) => PoleCell::Head(f),
            _ => PoleCell::Other,
        }
    };
    let Some(c) = piston::collect(pos, facing, extended, cell) else {
        return;
    };
    let m = piston::movement(speed, facing, c.progress as f32, c.range);
    let result = c.progress as f32 + m.signum() * 0.5;
    if m == 0.0 || result <= 0.0 || result >= c.range as f32 {
        return;
    }
    // The poles' and head's own states, then the structure in front (searched from the anchor,
    // pushed the way it moves).
    let sticky = block == create_engine::Block::StickyMechanicalPiston;
    let head = state_with(
        registry,
        "create:mechanical_piston_head",
        &[
            ("facing", dir_name(facing)),
            ("type", if sticky { "sticky" } else { "normal" }),
        ],
    );
    let pole = state_with(
        registry,
        "create:piston_extension_pole",
        &[("facing", dir_name(facing))],
    );
    let (Some(head), Some(pole)) = (head, pole) else {
        return;
    };
    let pole_worlds: Vec<[i32; 3]> = c
        .poles
        .iter()
        .map(|(l, _)| piston_world(c.anchor, *l, facing, c.progress))
        .collect();
    let forced = if m > 0.0 { facing } else { facing.opposite() };
    let mut exclude = pole_worlds.clone();
    exclude.push(pos);
    let Ok(structure) = piston::gather(c.anchor, forced, &exclude, |p| contraption_found(game, p))
    else {
        return;
    };
    let o = facing.offset();
    let mut blocks: Vec<([i32; 3], u32)> = c
        .poles
        .iter()
        .map(|(l, is_head)| (*l, if *is_head { head.0 } else { pole.0 }))
        .collect();
    for (off, s) in &structure {
        blocks.push(([0, 1, 2].map(|a| off[a] - o[a] * c.progress), *s));
    }
    // Off the world: everything but the piston itself.
    let air = BlockStateId(0);
    for (l, _) in &blocks {
        let p = piston_world(c.anchor, *l, facing, c.progress);
        if p != pos {
            game.native_set_block(dimension, p, air, super::native_hooks::flags::UPDATE_ALL);
        }
    }
    game.native_set_block_property(pos, "state", "moving");
    with_world(|w| {
        w.dims[dim].pistons.insert(
            pos,
            create_engine::world::Moving {
                anchor: c.anchor,
                blocks,
                offset: c.progress as f32,
                range: c.range,
            },
        )
    });
    send_piston(game, dimension, dim, pos);
}

/// `tryDisassemble`: the structure goes back into the world at the nearest whole offset; the
/// piston is extended unless all the way in.
fn disassemble_piston(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    let Some(m) = with_world(|w| w.dims[dim].pistons.remove(&pos)).flatten() else {
        return;
    };
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some(facing) = with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten()
    else {
        return;
    };
    let grid = m.offset.round() as i32;
    for (l, s) in &m.blocks {
        let p = piston_world(m.anchor, *l, facing, grid);
        if p == pos {
            continue;
        }
        game.native_set_block(
            dimension,
            p,
            BlockStateId(*s),
            super::native_hooks::flags::UPDATE_ALL,
        );
    }
    game.native_set_block_property(
        pos,
        "state",
        if grid > 0 { "extended" } else { "retracted" },
    );
    let players = players_in(game, dimension);
    let mut msg = Vec::new();
    for v in pos {
        msg.extend_from_slice(&v.to_le_bytes());
    }
    for p in players {
        let _ = game.native_net_send_to_player(p, PISTON_CHANNEL, &msg);
    }
}

/// A moving piston for clients: piston, anchor, facing, offset now, blocks a tick, then each
/// block (offset from the anchor at offset 0, state: 16 bytes). Just the position: it stopped.
fn send_piston(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    let Some(m) = with_world(|w| w.dims[dim].pistons.get(&pos).cloned()).flatten() else {
        return;
    };
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some(facing) = with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten()
    else {
        return;
    };
    let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
    let per_tick = create_engine::piston::movement(speed, facing, m.offset, m.range);
    let mut msg = Vec::new();
    for v in pos.iter().chain(m.anchor.iter()) {
        msg.extend_from_slice(&v.to_le_bytes());
    }
    msg.push(facing as u8);
    msg.extend_from_slice(&m.offset.to_le_bytes());
    msg.extend_from_slice(&per_tick.to_le_bytes());
    let blocks = &m.blocks[..m.blocks.len().min((30_000 - 33) / 16)];
    msg.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    for (o, s) in blocks {
        for v in o {
            msg.extend_from_slice(&v.to_le_bytes());
        }
        msg.extend_from_slice(&s.to_le_bytes());
    }
    for p in players_in(game, dimension) {
        let _ = game.native_net_send_to_player(p, PISTON_CHANNEL, &msg);
    }
}

thread_local! {
    /// Pistons' moving structures a client draws: their part model.
    static PISTONS: RefCell<HashMap<[i32; 3], u32>> = RefCell::new(HashMap::new());
}

/// A piston's structure on the client: one part model gliding along the facing on the GPU from
/// its offset now, at its speed (`ControlledContraptionEntity`'s motion).
fn pistons_from_server(game: &mut Game<'_>, d: &[u8]) {
    let i = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    if d.len() < 12 {
        return;
    }
    let pos = [i(&d[0..4]), i(&d[4..8]), i(&d[8..12])];
    let key = depot_key(pos, 221, 0);
    if let Some(old) = PISTONS.with_borrow_mut(|c| c.remove(&pos)) {
        game.native_part_clear(key);
        game.native_part_model_free(old);
    }
    if d.len() < 37 {
        return;
    }
    let anchor = [i(&d[12..16]), i(&d[16..20]), i(&d[20..24])];
    let facing = Dir::ALL[usize::from(d[24].min(5))];
    let f = |b: &[u8]| f32::from_le_bytes(b.try_into().unwrap());
    let (offset, per_tick) = (f(&d[25..29]), f(&d[29..33]));
    let n = u32::from_le_bytes(d[33..37].try_into().unwrap()) as usize;
    let blocks: Vec<(BlockStateId, glam::Mat4)> = d[37..]
        .chunks_exact(16)
        .take(n)
        .map(|r| {
            let o = glam::Vec3::new(i(&r[0..4]) as f32, i(&r[4..8]) as f32, i(&r[8..12]) as f32);
            (
                BlockStateId(u32::from_le_bytes(r[12..16].try_into().unwrap())),
                glam::Mat4::from_translation(o),
            )
        })
        .collect();
    let Some(model) = game.native_part_model_compound(&blocks) else {
        return;
    };
    PISTONS.with_borrow_mut(|c| c.insert(pos, model));
    let o = facing.offset().map(|v| v as f32);
    let now = game.native_parts_seconds();
    game.native_part_glide(
        key,
        model,
        anchor,
        0,
        0.0,
        0.0,
        [o[0] * offset, o[1] * offset, o[2] * offset, now],
        [
            o[0] * per_tick * 20.0,
            o[1] * per_tick * 20.0,
            o[2] * per_tick * 20.0,
        ],
    );
}

/// A windmill's structure for clients: bearing, facing, then each block's offset and state (16
/// bytes); none when it came apart.
fn send_windmills(game: &mut Game<'_>, players: &[u128], dim: usize, bearings: &[[i32; 3]]) {
    for &pos in bearings {
        let w = with_world(|w| w.dims[dim].windmills.get(&pos).cloned()).flatten();
        let facing = with_world(|w| {
            w.table
                .get(game.world.block(pos[0], pos[1], pos[2]).0)
                .and_then(|(_, i)| i.facing)
        })
        .flatten()
        .unwrap_or(Dir::Up);
        let mut m = Vec::new();
        for v in pos {
            m.extend_from_slice(&v.to_le_bytes());
        }
        m.push(facing as u8);
        let w_turn = w.as_ref().and_then(|w| w.bearing);
        let blocks = w.map(|w| w.blocks).unwrap_or_default();
        let blocks = &blocks[..blocks.len().min((30_000 - 17) / 16)];
        m.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
        for (o, s) in blocks {
            for v in o {
                m.extend_from_slice(&v.to_le_bytes());
            }
            m.extend_from_slice(&s.to_le_bytes());
        }
        // Its angle now (a windmill starts at 0), for the client to turn on from.
        let now = game.world.game_time;
        let angle = w_turn.map_or(0.0, |t| t.angle_at(now));
        m.extend_from_slice(&angle.to_le_bytes());
        for &p in players {
            let _ = game.native_net_send_to_player(p, CONTRAPTION_CHANNEL, &m);
        }
    }
}

thread_local! {
    /// Structures a client draws, by bearing: their part model, the axis they turn on, and their
    /// angle at a clock time with the speed since, so a speed change turns on from where it is.
    static CONTRAPTIONS: RefCell<HashMap<[i32; 3], (u32, Dir, f32, f32, f32)>> = RefCell::new(HashMap::new());
}

fn contraptions_from_server(game: &mut Game<'_>, d: &[u8]) {
    let i = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    if d.len() < 17 {
        return;
    }
    let pos = [i(&d[0..4]), i(&d[4..8]), i(&d[8..12])];
    let facing = Dir::ALL[usize::from(d[12].min(5))];
    let n = u32::from_le_bytes(d[13..17].try_into().unwrap()) as usize;
    let key = depot_key(pos, 220, 0);
    if let Some((old, ..)) = CONTRAPTIONS.with_borrow_mut(|c| c.remove(&pos)) {
        game.native_part_clear(key);
        game.native_part_model_free(old);
    }
    let blocks: Vec<(BlockStateId, glam::Mat4)> = d[17..]
        .chunks_exact(16)
        .take(n)
        .map(|r| {
            let o = glam::Vec3::new(i(&r[0..4]) as f32, i(&r[4..8]) as f32, i(&r[8..12]) as f32);
            (
                BlockStateId(u32::from_le_bytes(r[12..16].try_into().unwrap())),
                glam::Mat4::from_translation(o),
            )
        })
        .collect();
    if blocks.is_empty() {
        return;
    }
    let Some(model) = game.native_part_model_compound(&blocks) else {
        return;
    };
    let angle = d
        .get(17 + 16 * n..21 + 16 * n)
        .map_or(0.0, |b| f32::from_le_bytes(b.try_into().unwrap()));
    let now = game.native_parts_seconds();
    let rpm = press_speed(pos);
    CONTRAPTIONS.with_borrow_mut(|c| c.insert(pos, (model, facing, angle, now, rpm)));
    turn_contraption(game, pos, rpm);
}

/// The structure turns with its bearing about the bearing's axis (through its centre).
fn turn_contraption(game: &mut Game<'_>, pos: [i32; 3], rpm: f32) {
    let Some((model, facing, angle, at, old)) = CONTRAPTIONS.with_borrow(|c| c.get(&pos).copied())
    else {
        return;
    };
    // Where it is now at its old speed (6° a second per rpm), then on from there.
    let now = game.native_parts_seconds();
    let here = (angle + old * 6.0 * (now - at)) % 360.0;
    CONTRAPTIONS.with_borrow_mut(|c| c.insert(pos, (model, facing, here, now, rpm)));
    game.native_part_set(
        depot_key(pos, 220, 0),
        model,
        pos,
        axis_index(facing.axis()),
        rpm,
        (here - rpm * 6.0 * now) % 360.0,
    );
}

/// The block states of windmills as text (`name[key=value,...]`), to save.
fn state_text(registry: &BlockRegistry, id: u32) -> String {
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

fn state_of_text(registry: &BlockRegistry, text: &str) -> Option<u32> {
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

/// Trains as saved (`Train.write`, here with the column they stand in): their line, motion,
/// first bogey and the blocks they carry.
fn trains_to_nbt(
    game: &Game<'_>,
    all: &[create_engine::world::TrainOnTrack],
) -> horizon_world::nbt::Tag {
    use horizon_world::nbt::{Compound, Tag};
    let registry = game.pack.assets.registry;
    let f = |v: f64| (v as f32).to_bits() as i32;
    Tag::List(
        all.iter()
            .map(|t| {
                let l = t.train.line;
                let mut c = Compound::new();
                c.insert(
                    "line".into(),
                    Tag::IntArray(vec![
                        i32::from(axis_index(l.axis)),
                        l.fixed[0],
                        l.fixed[1],
                        l.fixed[2],
                        l.from,
                        l.to,
                    ]),
                );
                c.insert(
                    "motion".into(),
                    Tag::IntArray(vec![
                        f(t.train.position),
                        f(t.train.speed),
                        f(t.train.destination.unwrap_or(0.0)),
                        i32::from(t.train.destination.is_some()),
                        t.origin[0],
                        t.origin[1],
                        t.origin[2],
                        f(t.start),
                    ]),
                );
                c.insert(
                    "blocks".into(),
                    Tag::IntArray(t.blocks.iter().flat_map(|(o, _)| *o).collect()),
                );
                c.insert(
                    "states".into(),
                    Tag::List(
                        t.blocks
                            .iter()
                            .map(|(_, s)| Tag::String(state_text(registry, *s).as_str().into()))
                            .collect(),
                    ),
                );
                Tag::Compound(c)
            })
            .collect(),
    )
}

fn trains_from_nbt(
    game: &Game<'_>,
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<create_engine::world::TrainOnTrack> {
    use create_engine::trains::{Line, Train};
    use horizon_world::nbt::Tag;
    let registry = game.pack.assets.registry;
    let Some(Tag::List(list)) = data.and_then(|d| d.get(&"trains".into())) else {
        return Vec::new();
    };
    let f = |v: i32| f64::from(f32::from_bits(v as u32));
    list.iter()
        .filter_map(|c| {
            let Tag::Compound(c) = c else { return None };
            let (
                Some(Tag::IntArray(l)),
                Some(Tag::IntArray(m)),
                Some(Tag::IntArray(o)),
                Some(Tag::List(s)),
            ) = (
                c.get(&"line".into()),
                c.get(&"motion".into()),
                c.get(&"blocks".into()),
                c.get(&"states".into()),
            )
            else {
                return None;
            };
            if l.len() < 6 || m.len() < 8 {
                return None;
            }
            let axis = [Axis::X, Axis::Y, Axis::Z][l[0].clamp(0, 2) as usize];
            let line = Line {
                axis,
                fixed: [l[1], l[2], l[3]],
                from: l[4],
                to: l[5],
            };
            let blocks = o
                .chunks_exact(3)
                .zip(s)
                .filter_map(|(o, s)| {
                    let Tag::String(s) = s else { return None };
                    Some((
                        [o[0], o[1], o[2]],
                        state_of_text(registry, &s.to_string_lossy())?,
                    ))
                })
                .collect();
            Some(create_engine::world::TrainOnTrack {
                train: Train {
                    line,
                    position: f(m[0]),
                    speed: f(m[1]),
                    destination: (m[3] != 0).then(|| f(m[2])),
                },
                blocks,
                origin: [m[4], m[5], m[6]],
                start: f(m[7]),
                id: 0,
            })
        })
        .collect()
}

fn windmills_to_nbt(
    game: &Game<'_>,
    all: &[([i32; 3], create_engine::world::Windmill)],
) -> horizon_world::nbt::Tag {
    use horizon_world::nbt::{Compound, Tag};
    let registry = game.pack.assets.registry;
    Tag::List(
        all.iter()
            .map(|(p, w)| {
                let mut c = Compound::new();
                c.insert("pos".into(), Tag::IntArray(p.to_vec()));
                c.insert(
                    "blocks".into(),
                    Tag::IntArray(w.blocks.iter().flat_map(|(o, _)| *o).collect()),
                );
                c.insert(
                    "states".into(),
                    Tag::List(
                        w.blocks
                            .iter()
                            .map(|(_, s)| Tag::String(state_text(registry, *s).as_str().into()))
                            .collect(),
                    ),
                );
                // A mechanical bearing's `Angle` (now) and that it is one.
                let now = game.world.game_time;
                let angle = w.bearing.map_or(0.0, |t| t.angle_at(now));
                c.insert(
                    "sails".into(),
                    Tag::IntArray(vec![
                        w.sails as i32,
                        i32::from(w.clockwise),
                        angle.to_bits() as i32,
                        i32::from(w.bearing.is_some()),
                    ]),
                );
                Tag::Compound(c)
            })
            .collect(),
    )
}

fn windmills_from_nbt(
    game: &Game<'_>,
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<([i32; 3], create_engine::world::Windmill)> {
    use horizon_world::nbt::Tag;
    let registry = game.pack.assets.registry;
    let Some(Tag::List(list)) = data.and_then(|d| d.get(&"windmills".into())) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|c| {
            let Tag::Compound(c) = c else { return None };
            let Some(Tag::IntArray(p)) = c.get(&"pos".into()) else {
                return None;
            };
            let Some(Tag::IntArray(offsets)) = c.get(&"blocks".into()) else {
                return None;
            };
            let Some(Tag::List(states)) = c.get(&"states".into()) else {
                return None;
            };
            let Some(Tag::IntArray(sails)) = c.get(&"sails".into()) else {
                return None;
            };
            let blocks = offsets
                .chunks_exact(3)
                .zip(states)
                .filter_map(|(o, s)| {
                    let Tag::String(s) = s else { return None };
                    Some((
                        [o[0], o[1], o[2]],
                        state_of_text(registry, &s.to_string_lossy())?,
                    ))
                })
                .collect();
            Some((
                [*p.first()?, *p.get(1)?, *p.get(2)?],
                create_engine::world::Windmill {
                    blocks,
                    sails: (*sails.first()?).max(0) as u32,
                    clockwise: sails.get(1) != Some(&0),
                    bearing: (sails.get(3) == Some(&1)).then(|| create_engine::contraption::Turn {
                        angle: f32::from_bits(sails.get(2).copied().unwrap_or(0) as u32),
                        since: game.world.game_time,
                        rpm: 0.0,
                    }),
                },
            ))
        })
        .collect()
}
