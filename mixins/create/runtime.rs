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
pub(crate) fn world_lifecycle(game: &mut Game<'_>, phase: &WorldPhase) {
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
pub(crate) fn block_changed(
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
pub(crate) fn neighbor_changed(
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
pub(crate) fn chunk_load(
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
    let mut buffer = [0u32; 4096];
    with_world(|w| {
        w.load_settings(dim, saved.into_iter());
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

pub(crate) fn chunk_unload(dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    if let Some(dim) = dimension_index(dimension) {
        with_world(|w| w.column_unloaded(dim, chunk.x, chunk.z));
    }
}

/// After a server level's tick: re-solve what changed there (nothing when nothing did), break
/// the blocks Create would break, and tell the level's players what turns now.
pub(crate) fn level_tick_post(game: &mut Game<'_>, dimension: &str) {
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
    tick_millstones(game, dim);
    let mut events = Vec::new();
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
            } => updates.push(Update::Set { pos, sub, speed }),
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
pub(crate) fn net_from_client(game: &mut Game<'_>, player: u128, channel: &str, data: &[u8]) {
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
}

/// A player right-clicks a block (`HandCrankBlock.useItemOn`): a hand crank turns for 10 ticks,
/// backwards while sneaking, and tires the player (`crankHungerMultiplier` 0.01 per RPM).
pub(crate) fn right_click_block(game: &mut Game<'_>, pos: [i32; 3], canceled: &mut bool) {
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

/// The mouse wheel on the client: a creative motor under the crosshair takes it.
pub(crate) fn wheel(game: &mut Game<'_>, steps: f32, claimed: &mut bool) {
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

/// A chunk is being saved: Create's settings in it, as NBT (the only time it writes NBT).
pub(crate) fn chunk_data_save(
    dimension: &str,
    chunk: [i32; 2],
    data: &mut horizon_world::nbt::Compound,
) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let settings = with_world(|w| w.column_settings(dim, chunk[0], chunk[1])).unwrap_or_default();
    if settings.is_empty() {
        data.remove(&"create".into());
        return;
    }
    let motors: Vec<i32> = settings
        .iter()
        .flat_map(|(p, rpm)| [p[0], p[1], p[2], *rpm])
        .collect();
    let mut own = horizon_world::nbt::Compound::new();
    own.insert("motors".into(), horizon_world::nbt::Tag::IntArray(motors));
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
            None => model_state(registry, state, d.model)
                .and_then(|s| game.native_part_model(s))
                .inspect(|&h| {
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
pub(crate) fn net_from_server(game: &mut Game<'_>, channel: &str, data: &[u8]) {
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
pub(crate) fn client_tick(game: &mut Game<'_>) {
    if !game.native_window() {
        return;
    }
    if !game.native_playing() {
        CLIENT.with_borrow_mut(|c| *c = None);
        return;
    }
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
    milling: create_engine::processing::Book,
}

thread_local! {
    static RECIPES: RefCell<Option<Recipes>> = const { RefCell::new(None) };
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
    if RECIPES.with_borrow(Option::is_none) {
        let milling = read_book(game, "create:milling");
        if milling.is_empty() {
            return;
        }
        RECIPES.with_borrow_mut(|r| *r = Some(Recipes { milling }));
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
pub(crate) fn block_entity(
    block: &str,
    entity: &mut Option<(String, horizon_world::nbt::Compound)>,
) {
    if entity.is_none() && block == "create:millstone" {
        *entity = Some((
            "create:millstone".into(),
            horizon_world::nbt::Compound::new(),
        ));
    }
}

/// The millstone's slots (`MillstoneInventoryHandler`): hoppers put into the input (0) and take
/// from the nine outputs, through any face; no menu.
pub(crate) fn block_entity_container(
    block: &str,
    container: &mut Option<horizon_client::containers::ModContainer>,
) {
    if container.is_some() || block != "create:millstone" {
        return;
    }
    let all: Vec<usize> = (0..10).collect();
    *container = Some(horizon_client::containers::ModContainer {
        slots: 10,
        faces: std::array::from_fn(|_| all.clone()),
        inputs: vec![0],
        outputs: (1..10).collect(),
        menu: None,
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
pub(crate) fn register_commands(commands: &mut Vec<super::native_server::NativeCommand>) {
    commands.push(super::native_server::NativeCommand::op(
        "create_debug speed <pos:block_pos>",
        2,
    ));
}

pub(crate) fn perform_command(game: &mut Game<'_>, command: &str, ran: &mut bool) {
    let Some(rest) = command.strip_prefix("create_debug speed ") else {
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
    let line = match parts.as_slice() {
        [x, y, z] => match with_world(|w| w.speed(dim, [*x, *y, *z])).flatten() {
            Some(speed) => format!("create speed at {x} {y} {z}: {speed} rpm"),
            None => format!("create speed at {x} {y} {z}: not kinetic"),
        },
        _ => "usage: create_debug speed <x> <y> <z>".into(),
    };
    game.native_feedback(horizon_client::text::Text::literal(line));
}
