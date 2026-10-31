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
    let mut buffer = [0u32; 4096];
    with_world(|w| {
        w.load_settings(dim, saved.into_iter());
        w.load_belt_items(dim, belts.into_iter());
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
    tick_millstones(game, dim);
    tick_depots(game, dimension, dim);
    tick_belts(game, dimension, dim);
    tick_presses(game, dimension, dim);
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
}

/// A player right-clicks a block (`HandCrankBlock.useItemOn`): a hand crank turns for 10 ticks,
/// backwards while sneaking, and tires the player (`crankHungerMultiplier` 0.01 per RPM).
pub(super) fn right_click_block(
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
    if block == Some(create_engine::Block::Belt) {
        use_belt(game, dim, pos, canceled);
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
                b.insert("positive".into(), Tag::IntArray(vec![i32::from(inv.positive)]));
                Tag::Compound(b)
            })
            .collect(),
    )
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
    dimension: &str,
    chunk: [i32; 2],
    data: &mut horizon_world::nbt::Compound,
) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let settings = with_world(|w| w.column_settings(dim, chunk[0], chunk[1])).unwrap_or_default();
    let belts = with_world(|w| w.column_belts(dim, chunk[0], chunk[1])).unwrap_or_default();
    if settings.is_empty() && belts.is_empty() {
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
    if channel == create_engine::press::CHANNEL {
        presses_from_server(game, data);
        return;
    }
    if channel == create_engine::belt::CHANNEL {
        belts_from_server(game, data);
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
        })
    });
    true
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
    if entity.is_none() && matches!(block, "create:millstone" | "create:depot") {
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
}

pub(super) fn perform_command(game: &mut Game<'_>, command: &str, ran: &mut bool) {
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
    let hand = game
        .inventory
        .get_mut(game.selected)
        .and_then(Option::take);
    let facing = create_engine::depot::Side::of_yaw(game.yaw);
    let put = hand.as_ref().map(|s| {
        create_engine::depot::Held::put_by_player(
            random_angle(game, pos),
            upright(game, &s.item),
            facing,
        )
    });
    let taken = game
        .native_with_mod_block_entity(pos, |be| {
            be.items.resize(be.items.len().max(create_engine::depot::SLOTS), None);
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
            let held = create_engine::depot::Held::new(random_angle(game, pos), upright(game, &item));
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
    let mut x = client::key(pos, 0) ^ (u64::from(slot) << 8 | u64::from(copy)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
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
            let poses = depot::stack_poses(u32::from(*count), block_item, &mut depot::JavaRandom::new(0));
            (poses, c.held.angle as f32, at, (to - at) * 20.0)
        } else {
            let (poses, phase) = depot::output_place(usize::from(*slot) - 1, u32::from(*count), block_item);
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
        let Some((_, item, count)) = depot_items(game, depot).into_iter().find(|s| s.0 == 0)
        else {
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
        with_world(|w| {
            let d = &mut w.dims[dim];
            let Some(p) = d.presses.get_mut(&at) else {
                return;
            };
            match p.tick(speed) {
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
        });
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
        Some(processing::roll(r.pressing.get(id), &mut processing::Rng::new(seed)))
    });
    let Some(results) = results else {
        return held;
    };
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
            for (slot, s) in be.items[create_engine::depot::OUTPUTS].iter_mut().zip(outputs) {
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
    let own = [at[0].floor() as i32, at[1].floor() as i32, at[2].floor() as i32];
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
        _ => return,
    };
    if stack.extra.is_some() {
        // Stacks with components keep them only as entities, for now.
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
        let next = self.past_end();
        let side = self.movement();
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
    let speed = create_engine::belt::movement_speed(rpm).abs().max(1.0 / 8.0);
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
            Some((b.segment, b.chain.len() as i32, std::mem::take(&mut b.items)))
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
                        let speed = create_engine::belt::direction_aware_speed(rpm, b.segment.facing);
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
