// What moving contraptions carry besides their blocks, and Create's glue and stickers: super glue
// areas the structure search reads (set by `/create_debug glue`, standing in for the item), the
// sticker's redstone, actors (drills, saws, harvesters, ploughs) ticked only while their
// contraption moves, mounted storage that keeps its items and takes what the actors collect, and
// contraption controls (`/create_debug controls`). Create's rules are in `create_engine::glue`
// and `create_engine::actors`. A sibling of the `create` module (`runtime.rs`).
use super::Game;
use super::create::*;
use super::create_contraption_body as body;
use create_engine::actors::{self, Actor, Carried, Effect, Kind};
use create_engine::glue::{self, Glue};
use create_engine::kinetics::Dir;
use create_engine::world::dimension_index;
use horizon_client::items::ItemStack;
use horizon_world::registry::BlockStateId;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Storage blocks' items while their contraption moves, by dimension and contraption (a
    /// piston's position, a gantry's start): each block's offset and what it holds.
    static STORAGE: RefCell<HashMap<(usize, [i32; 3]), Vec<([i32; 3], body::Stored)>>> =
        RefCell::new(HashMap::new());
    /// `BlockBreakingKineticBlockEntity.NEXT_BREAKER_ID`.
    static NEXT_BREAKER: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

/// The world closed: what moving contraptions carried goes with it.
pub(super) fn forget() {
    STORAGE.with_borrow_mut(HashMap::clear);
    NEXT_BREAKER.set(0);
}

/// The faces of `p` glue holds, for the structure search.
pub(super) fn glued_at(dim: usize, p: [i32; 3]) -> u8 {
    with_world(|w| glue::glued_faces(&w.dims[dim].glue, p)).unwrap_or(0)
}

/// An extended sticker's facing (`StickerBlock.EXTENDED`), None for any other block.
pub(super) fn sticker_facing(game: &Game<'_>, id: BlockStateId) -> Option<Dir> {
    let state = game.pack.assets.registry.state(id)?;
    if state.name != "create:sticker"
        || state.properties.get("extended").map(String::as_str) != Some("true")
    {
        return None;
    }
    let f = state.properties.get("facing")?;
    Dir::ALL.into_iter().find(|d| dir_name(*d) == f)
}

/// Whether a storage block's items ride with its contraption (it is not refused for its block
/// entity).
pub(super) fn carries_storage(game: &Game<'_>, p: [i32; 3]) -> bool {
    body::is_storage(game, p)
}

/// `SuperGlueEntity.isSideSticky` for the glue selection: the sides of a block already sticky.
fn sticky_sides(game: &Game<'_>, p: [i32; 3]) -> u8 {
    let Some(s) = game
        .pack
        .assets
        .registry
        .state(game.world.block(p[0], p[1], p[2]))
    else {
        return 0;
    };
    let facing = || {
        s.properties
            .get("facing")
            .and_then(|f| Dir::ALL.into_iter().find(|d| dir_name(*d) == f))
            .map_or(0, Dir::bit)
    };
    match s.name.as_str() {
        "minecraft:slime_block" | "minecraft:honey_block" => 0x3f,
        "create:sticky_mechanical_piston"
        | "create:sticker"
        | "create:gantry_carriage"
        | "create:mechanical_bearing"
        | "create:windmill_bearing"
        | "create:clockwork_bearing" => facing(),
        "create:cart_assembler" => Dir::Up.bit(),
        "create:linear_chassis" | "create:secondary_linear_chassis" => {
            let axis = s.properties.get("axis").map_or("y", String::as_str);
            let (top, bottom) = match axis {
                "x" => (Dir::East, Dir::West),
                "z" => (Dir::South, Dir::North),
                _ => (Dir::Up, Dir::Down),
            };
            let on = |k: &str| s.properties.get(k).is_some_and(|v| v == "true");
            (if on("sticky_top") { top.bit() } else { 0 })
                | (if on("sticky_bottom") { bottom.bit() } else { 0 })
        }
        _ => 0,
    }
}

/// `/create_debug glue <from> <to>`: what `SuperGlueSelectionPacket` does once the player
/// picked two corners with the glue item (its charges and reach aside).
pub(super) fn debug_glue(
    game: &mut Game<'_>,
    dimension: &str,
    from: [i32; 3],
    to: [i32; 3],
) -> String {
    let Some(dim) = dimension_index(dimension) else {
        return "no such dimension".into();
    };
    let placed = glue::place(from, to, |p| {
        super::create_contraptions::contraption_found(game, p).map(|_| sticky_sides(game, p))
    });
    match placed {
        Ok(g) => {
            with_world(|w| w.dims[dim].glue.push(g));
            let (cx, cz) = g.column();
            game.native_mark_chunk_unsaved(dimension, [cx, cz]);
            format!(
                "glued {} {} {} to {} {} {}",
                g.min[0], g.min[1], g.min[2], g.max[0], g.max[1], g.max[2]
            )
        }
        Err(glue::Refused::TooFar) => "glue: the corners are 25 blocks apart or more".into(),
        Err(glue::Refused::NotConnected) => "glue: the corners don't hold together".into(),
    }
}

/// The glue areas of a column, saved with it under `create_glue`: six ints each.
pub(super) fn glue_chunk_save(
    dim: usize,
    chunk: [i32; 2],
    data: &mut horizon_world::nbt::Compound,
) {
    let areas: Vec<i32> = with_world(|w| {
        w.dims[dim]
            .glue
            .iter()
            .filter(|g| g.column() == (chunk[0], chunk[1]))
            .flat_map(|g| g.min.into_iter().chain(g.max))
            .collect()
    })
    .unwrap_or_default();
    if areas.is_empty() {
        return;
    }
    let mut own = horizon_world::nbt::Compound::new();
    own.insert("areas".into(), horizon_world::nbt::Tag::IntArray(areas));
    data.insert("create_glue".into(), horizon_world::nbt::Tag::Compound(own));
}

/// A column's glue back as it loads.
pub(super) fn glue_chunk_load(dim: usize, saved: Option<&horizon_world::nbt::Compound>) {
    let Some(horizon_world::nbt::Tag::IntArray(v)) = saved.and_then(|d| d.get(&"areas".into()))
    else {
        return;
    };
    let areas: Vec<Glue> = v
        .chunks_exact(6)
        .map(|c| Glue {
            min: [c[0], c[1], c[2]],
            max: [c[3], c[4], c[5]],
        })
        .collect();
    with_world(|w| {
        let glue = &mut w.dims[dim].glue;
        for g in areas {
            if !glue.contains(&g) {
                glue.push(g);
            }
        }
    });
}

/// A column's glue leaves with it (its entities unload).
pub(super) fn glue_chunk_unload(dim: usize, chunk: [i32; 2]) {
    with_world(|w| {
        w.dims[dim]
            .glue
            .retain(|g| g.column() != (chunk[0], chunk[1]))
    });
}

/// Stickers whose neighbour changed (`StickerBlock.neighborChanged`): `powered` follows the
/// signal and each rise flips `extended`. Nothing runs without a change.
pub(super) fn tick_stickers(game: &mut Game<'_>, dim: usize) {
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].sticker_checks)).unwrap_or_default();
    if checks.is_empty() {
        return;
    }
    checks.sort();
    checks.dedup();
    for pos in checks {
        let Some(s) = game
            .pack
            .assets
            .registry
            .state(game.world.block(pos[0], pos[1], pos[2]))
        else {
            continue;
        };
        if s.name != "create:sticker" {
            continue;
        }
        let on = |k: &str| s.properties.get(k).is_some_and(|v| v == "true");
        let (powered, extended) = (on("powered"), on("extended"));
        let at = horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]);
        let signal = game.with_level(|b, level, ticks| b.has_neighbor_signal(level, ticks, at));
        let Some((p, e)) = glue::sticker_neighbor(powered, extended, signal) else {
            continue;
        };
        let flags = super::native_hooks::flags::UPDATE_CLIENTS;
        let text = |v: bool| if v { "true" } else { "false" };
        if e != extended {
            game.native_set_block_property_with(pos, "extended", text(e), flags);
        }
        game.native_set_block_property_with(pos, "powered", text(p), flags);
    }
}

/// What a contraption takes besides its blocks as it assembles (`Contraption.addBlock`,
/// `removeBlocksFromWorld`): its actors (`startMoving`), its storage blocks' items, and the glue
/// that held it, shrunk to it. `origin` is where offset 0 of `blocks` is in the world; `skip`
/// names the blocks anchoring it. Called before its blocks leave the world.
pub(super) fn carry(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    key: [i32; 3],
    origin: [i32; 3],
    blocks: &[([i32; 3], u32)],
    skip: impl Fn([i32; 3]) -> bool,
) -> Carried {
    let at = |o: [i32; 3]| [origin[0] + o[0], origin[1] + o[1], origin[2] + o[2]];
    let mut carried = Carried::default();
    for (o, state) in blocks {
        let Some((block, facing)) =
            with_world(|w| w.table.get(*state).map(|(b, i)| (b, i.facing))).flatten()
        else {
            continue;
        };
        if let (Some(kind), Some(facing)) = (Kind::of(block), facing) {
            let id = NEXT_BREAKER.get() + 1;
            NEXT_BREAKER.set(id);
            carried.actors.push(Actor::new(*o, kind, facing, -id));
        }
    }
    let mut stored = Vec::new();
    for (o, _) in blocks {
        if let Some(s) = body::take_storage(game, at(*o)) {
            stored.push((*o, s));
        }
    }
    if !stored.is_empty() {
        STORAGE.with_borrow_mut(|m| m.insert((dim, key), stored));
    }
    let world: Vec<[i32; 3]> = blocks.iter().map(|(o, _)| at(*o)).collect();
    let taken = with_world(|w| {
        let areas = &mut w.dims[dim].glue;
        let (used, shrunk) = glue::carried(areas, &world, &skip);
        let columns: Vec<(i32, i32)> = used.iter().map(|&i| areas[i].column()).collect();
        for i in used.into_iter().rev() {
            areas.remove(i);
        }
        (shrunk, columns)
    });
    if let Some((shrunk, columns)) = taken {
        let back = [-origin[0], -origin[1], -origin[2]];
        carried.glue = shrunk.iter().map(|g| g.moved(back)).collect();
        for (cx, cz) in columns {
            game.native_mark_chunk_unsaved(dimension, [cx, cz]);
        }
    }
    carried
}

/// What a contraption carried goes back with its blocks (`addBlocksToWorld`, `stop`): its
/// storage blocks' items into the storage blocks now at `origin` + their offset (dropped where
/// one couldn't be put back), its glue there, and its breakers' cracks gone.
pub(super) fn release(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    key: [i32; 3],
    origin: [i32; 3],
    carried: &mut Carried,
) {
    let mut effects = Vec::new();
    for a in carried.actors.iter_mut() {
        a.cancel(&mut effects);
    }
    apply_cracks(game, &carried.actors, &effects);
    let at = |o: [i32; 3]| [origin[0] + o[0], origin[1] + o[1], origin[2] + o[2]];
    let stored = STORAGE
        .with_borrow_mut(|m| m.remove(&(dim, key)))
        .unwrap_or_default();
    for (o, mut s) in stored {
        let p = at(o);
        // A fresh storage block where it went back: it takes the carried items.
        if body::is_storage(game, p) {
            body::put_storage(game, p, s);
            continue;
        }
        let centre = p.map(|v| f64::from(v) + 0.5);
        if let Some(slots) = body::slots(&mut s) {
            for stack in slots.iter_mut().filter_map(Option::take) {
                body::drop_stack(game, centre, [0.0; 3], stack);
            }
        }
    }
    if carried.glue.is_empty() {
        return;
    }
    let placed: Vec<Glue> = carried.glue.drain(..).map(|g| g.moved(origin)).collect();
    for g in &placed {
        let (cx, cz) = g.column();
        game.native_mark_chunk_unsaved(dimension, [cx, cz]);
    }
    with_world(|w| w.dims[dim].glue.extend(placed));
}

/// What an actor reads of the world (`canBreak`, `FallingBlock`).
struct Reader<'a, 'g> {
    game: &'a Game<'g>,
}

impl actors::Probe for Reader<'_, '_> {
    fn breakable(&self, kind: Kind, pos: [i32; 3]) -> Option<f32> {
        let game = self.game;
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let b = game.res.behaviours();
        let tagged = |name: &str| b.tag(name).is_some_and(|t| t.contains(b.block(state)));
        let liquid = b.fluid(state).kind() != horizon_world::fluid::FluidKind::Empty
            && b.is_replaceable(state);
        let no_collision =
            b.facts(state).flags & horizon_world::level::data::state_flags::COLLISION_EMPTY != 0;
        let name = game
            .pack
            .assets
            .registry
            .state(state)
            .map(|s| s.name.as_str());
        let track = name == Some("create:track");
        let hardness = b.facts(state).hardness;
        if kind == Kind::Plough {
            let below = game.world.block(pos[0], pos[1] - 1, pos[2]);
            let farmland = game
                .pack
                .assets
                .registry
                .state(below)
                .is_some_and(|s| s.name == "minecraft:farmland");
            let p = actors::Ploughed {
                air: b.is_air(state),
                farmland_below: farmland,
                liquid: liquid || name == Some("minecraft:bubble_column"),
                track,
                blacklisted: tagged("create:plough_blacklist"),
                whitelisted: tagged("create:plough_whitelist"),
                no_collision,
            };
            // `getDestroySpeed` still paces it (grass breaks at once).
            return actors::plough_breaks(&p).then_some(hardness.max(0.0));
        }
        // `BlockBreakingKineticBlockEntity.isBreakable`.
        if b.is_air(state) || liquid || hardness < 0.0 || tagged("create:non_breakable") {
            return None;
        }
        let ok = match kind {
            Kind::Drill => !no_collision && !track,
            Kind::Saw => sawable(game, state, name.unwrap_or("")),
            _ => false,
        };
        ok.then_some(hardness)
    }

    fn falling(&self, pos: [i32; 3]) -> bool {
        let game = self.game;
        let state = game.world.block(pos[0], pos[1], pos[2]);
        game.pack.assets.registry.state(state).is_some_and(|s| {
            let n = s.name.as_str();
            matches!(
                n,
                "minecraft:sand"
                    | "minecraft:red_sand"
                    | "minecraft:suspicious_sand"
                    | "minecraft:gravel"
                    | "minecraft:suspicious_gravel"
                    | "minecraft:anvil"
                    | "minecraft:chipped_anvil"
                    | "minecraft:damaged_anvil"
                    | "minecraft:dragon_egg"
                    | "minecraft:scaffolding"
                    | "minecraft:pointed_dripstone"
            ) || n.ends_with("_concrete_powder")
        })
    }
}

/// `SawBlockEntity.isSawable`: logs, leaves, roots, bamboo, cactus, sugar cane, kelp, chorus
/// plants and Create's solid crops; not saplings.
fn sawable(game: &Game<'_>, state: BlockStateId, name: &str) -> bool {
    let b = game.res.behaviours();
    let tagged = |t: &str| b.tag(t).is_some_and(|s| s.contains(b.block(state)));
    if tagged("minecraft:saplings") {
        return false;
    }
    tagged("minecraft:logs")
        || tagged("minecraft:leaves")
        || tagged("create:solid_crops")
        || matches!(
            name,
            "minecraft:mangrove_roots"
                | "minecraft:muddy_mangrove_roots"
                | "minecraft:bamboo"
                | "minecraft:cactus"
                | "minecraft:sugar_cane"
                | "minecraft:kelp"
                | "minecraft:kelp_plant"
                | "minecraft:chorus_plant"
        )
}

fn apply_cracks(game: &mut Game<'_>, actors: &[Actor], effects: &[Effect]) {
    let id = actors.first().map_or(0, |a| a.breaker_id);
    for e in effects {
        if let Effect::Crack(p, stage) = e {
            super::dispenser::create_use::deployer_crack(game, id, *p, *stage);
        }
    }
}

/// `collectOrDropItem`: into the contraption's storage (`moveItemsToStorage`), what is left
/// dropped at the actor with `(motion + (0, .5, 0)) × random × .3`.
fn collect(game: &mut Game<'_>, dim: usize, key: [i32; 3], actor: &Actor, mut stack: ItemStack) {
    STORAGE.with_borrow_mut(|m| {
        let Some(list) = m.get_mut(&(dim, key)) else {
            return;
        };
        for (_, s) in list.iter_mut() {
            let Some(slots) = body::slots(s) else {
                continue;
            };
            let limit = body::slot_limit(game, &stack);
            match actors::insert(
                slots,
                stack.clone(),
                |s| s.count,
                |s, n| s.count = n,
                body::same_stack,
                |_| limit,
            ) {
                Some(left) => stack = left,
                None => {
                    stack.count = 0;
                    return;
                }
            }
        }
    });
    if stack.count == 0 {
        return;
    }
    let Some(at) = actor.position else {
        return;
    };
    let scale = f64::from(body::random_float(game)) * 0.3;
    let m = actor.motion;
    body::drop_stack(
        game,
        at,
        [m[0] * scale, (m[1] + 0.5) * scale, m[2] * scale],
        stack,
    );
}

/// `AbstractContraptionEntity.tickActors` for a translating contraption whose offset 0 is at
/// `translation` in the world now: each active actor ticks, and what it does happens. Returns
/// whether one stalls the contraption. Only moving contraptions with actors call it.
pub(super) fn tick_actors(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    key: [i32; 3],
    carried: &mut Carried,
    translation: [f64; 3],
) -> bool {
    let mut stalled = false;
    for i in 0..carried.actors.len() {
        let mut effects = Vec::new();
        let a = &mut carried.actors[i];
        let position = actors::translated_position(a, translation);
        let facing = a.facing;
        stalled |= a.tick(
            position,
            facing,
            actors::CONTROLLED_LOWER,
            &Reader { game },
            &mut effects,
        );
        let a = carried.actors[i].clone();
        for e in effects {
            apply(game, dimension, dim, key, &a, e);
        }
    }
    stalled
}

fn apply(game: &mut Game<'_>, dimension: &str, dim: usize, key: [i32; 3], a: &Actor, e: Effect) {
    match e {
        Effect::Crack(p, stage) => {
            super::dispenser::create_use::deployer_crack(game, a.breaker_id, p, stage);
        }
        Effect::Hit(p) => {
            let state = game.world.block(p[0], p[1], p[2]);
            body::hit_sound(game, p, state);
        }
        Effect::Destroy(p) => {
            // The plough's snow drops as an iron shovel digs it (`onBlockBroken`).
            let snow = a.kind == Kind::Plough
                && game
                    .pack
                    .assets
                    .registry
                    .state(game.world.block(p[0], p[1], p[2]))
                    .is_some_and(|s| s.name == "minecraft:snow");
            let shovel = ItemStack::new("minecraft:iron_shovel", 1);
            let drops = body::break_block(game, dimension, p, snow.then_some(&shovel));
            for stack in drops {
                collect(game, dim, key, a, stack);
            }
        }
        Effect::Harvest(p) => harvest(game, dimension, dim, key, a, p),
        Effect::Till(p) => {
            // A ray down from the block's centre hits it if it has a shape, else the one below.
            let state = game.world.block(p[0], p[1] + 1, p[2]);
            let target = if game.res.facts().get(state).outline.is_empty() {
                p
            } else {
                [p[0], p[1] + 1, p[2]]
            };
            let hoe = ItemStack::new("minecraft:diamond_hoe", 1);
            let from = [target[0], target[1] + 2, target[2]];
            let _ = super::dispenser::create_use::use_item_at(game, from, Dir::Down, hoe);
        }
    }
}

/// `HarvesterMovementBehaviour.visitNewPosition` on the server.
fn harvest(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    key: [i32; 3],
    a: &Actor,
    p: [i32; 3],
) {
    let state = game.world.block(p[0], p[1], p[2]);
    let b = game.res.behaviours();
    if b.is_air(state) {
        return;
    }
    let tagged = |t: &str| b.tag(t).is_some_and(|s| s.contains(b.block(state)));
    let Some(s) = game.pack.assets.registry.state(state).cloned() else {
        return;
    };
    let bush = tagged("minecraft:flowers") || tagged("minecraft:saplings");
    let age = s
        .properties
        .get("age")
        .and_then(|v| v.parse::<u8>().ok())
        .zip(actors::age_max(&s.name));
    let reached = actors::Reached {
        plant: actors::plant_of(&s.name, bush),
        age,
        no_collision: b.facts(state).flags
            & horizon_world::level::data::state_flags::COLLISION_EMPTY
            != 0,
        leaves: tagged("minecraft:leaves"),
    };
    let Some((replant, shears, _chance)) =
        actors::harvest(&reached, tagged("create:non_harvestable"))
    else {
        return;
    };
    let cut = actors::cut(&reached);
    let tool = shears.then(|| ItemStack::new("minecraft:shears", 1));
    let drops = body::break_block(game, dimension, p, tool.as_ref());
    let mut seed = actors::planted_by(&s.name).filter(|_| replant);
    for mut stack in drops {
        if seed.is_some_and(|i| *stack.item == *i) {
            stack.count -= 1;
            seed = None;
        }
        if stack.count > 0 {
            collect(game, dim, key, a, stack);
        }
    }
    if let actors::Cut::Age(n) = cut {
        let mut again = s.clone();
        again.properties.insert("age".into(), n.to_string());
        if let Some(id) = game.pack.assets.registry.id(&again) {
            game.native_set_block(dimension, p, id, super::native_hooks::flags::UPDATE_ALL);
        }
    }
}

/// `/create_debug controls <contraption> <filter|all>`: a click on contraption controls carried
/// by the moving contraption at a piston or gantry carriage's start (`handlePlayerInteraction`).
pub(super) fn debug_controls(
    game: &mut Game<'_>,
    dim: usize,
    key: [i32; 3],
    filter: Option<String>,
) -> String {
    let mut effects = Vec::new();
    let done = with_world(|w| {
        let d = &mut w.dims[dim];
        let carried = match d.pistons.get_mut(&key) {
            Some(m) => &mut m.carried,
            None => &mut d.gantries.iter_mut().find(|g| g.start == key)?.carried,
        };
        let kinds: Vec<&str> = carried.actors.iter().map(|a| a.kind.item()).collect();
        let disable = actors::toggle(&mut carried.disabled, &filter, &kinds);
        actors::set_active(&mut carried.actors, &filter, !disable, &mut effects);
        Some((disable, carried.actors.clone()))
    })
    .flatten();
    let Some((disable, list)) = done else {
        return format!("no moving contraption at {} {} {}", key[0], key[1], key[2]);
    };
    apply_cracks(game, &list, &effects);
    let what = filter.as_deref().unwrap_or("all actors");
    format!("{what} {}", if disable { "off" } else { "on" })
}

/// Create's contraption debug commands (`glue`, `controls`); false for any other command.
pub(super) fn contraption_command(game: &mut Game<'_>, command: &str) -> bool {
    let glue_args = command.strip_prefix("create_debug glue ");
    let controls_args = command.strip_prefix("create_debug controls ");
    if glue_args.is_none() && controls_args.is_none() {
        return false;
    }
    let player = game.native_acting_player();
    let dimension = player
        .as_ref()
        .map_or_else(|| "minecraft:overworld".to_owned(), |p| p.dimension.clone());
    let here = player.as_ref().map(|p| p.position);
    let rest = glue_args.or(controls_args).unwrap_or("");
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let coord = |n: usize| -> Option<i32> {
        let t = tokens.get(n)?;
        match t.strip_prefix('~') {
            Some(off) => Some(here?.get(n % 3)?.floor() as i32 + off.parse::<i32>().unwrap_or(0)),
            None => t.parse().ok(),
        }
    };
    let pos = |at: usize| Some([coord(at)?, coord(at + 1)?, coord(at + 2)?]);
    let line = if glue_args.is_some() {
        match (pos(0), pos(3)) {
            (Some(from), Some(to)) => debug_glue(game, &dimension, from, to),
            _ => "usage: create_debug glue <x> <y> <z> <x> <y> <z>".into(),
        }
    } else {
        let dim = dimension_index(&dimension).unwrap_or(0);
        match (pos(0), tokens.get(3)) {
            (Some(key), Some(f)) => {
                let filter = (*f != "all").then(|| (*f).to_owned());
                debug_controls(game, dim, key, filter)
            }
            _ => "usage: create_debug controls <x> <y> <z> <item|all>".into(),
        }
    };
    game.native_feedback(horizon_client::text::Text::literal(line));
    true
}
