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
use create_engine::states;
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
    ROLLERS_PLACED.with_borrow_mut(Vec::clear);
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
    use horizon_world::nbt::Tag;
    let here = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]);
    let text = |s: &Option<String>| Tag::String(s.as_deref().unwrap_or("").into());
    let Some((areas, controls, rollers, observers)) = with_world(|w| {
        let d = &w.dims[dim];
        let areas: Vec<i32> = d
            .glue
            .iter()
            .filter(|g| g.column() == (chunk[0], chunk[1]))
            .flat_map(|g| g.min.into_iter().chain(g.max))
            .collect();
        let mut controls: Vec<_> = d.controls.iter().filter(|(p, _)| here(p)).collect();
        controls.sort_by_key(|e| *e.0);
        let controls: Vec<([i32; 3], create_engine::actors::Controls)> =
            controls.into_iter().map(|(p, c)| (*p, c.clone())).collect();
        let mut rollers: Vec<_> = d
            .rollers
            .iter()
            .filter(|(p, _)| here(p))
            .map(|(p, v)| (*p, v.clone()))
            .collect();
        rollers.sort_by_key(|e| e.0);
        let mut observers: Vec<_> = d
            .observer_filters
            .iter()
            .filter(|(p, _)| here(p))
            .map(|(p, f)| (*p, f.clone()))
            .collect();
        observers.sort_by_key(|e| e.0);
        (areas, controls, rollers, observers)
    }) else {
        return;
    };
    if areas.is_empty() && controls.is_empty() && rollers.is_empty() && observers.is_empty() {
        return;
    }
    let mut own = horizon_world::nbt::Compound::new();
    if !areas.is_empty() {
        own.insert("areas".into(), Tag::IntArray(areas));
    }
    // Contraption controls (`Filter`, `Disabled`, `Powered`): x, y, z and the two flags.
    if !controls.is_empty() {
        let mut c = horizon_world::nbt::Compound::new();
        let nums = controls
            .iter()
            .flat_map(|(p, x)| {
                [
                    p[0],
                    p[1],
                    p[2],
                    i32::from(x.disabled) | i32::from(x.powered) << 1,
                ]
            })
            .collect();
        c.insert("nums".into(), Tag::IntArray(nums));
        let items = controls.iter().map(|(_, x)| text(&x.filter)).collect();
        c.insert("filters".into(), Tag::List(items));
        own.insert("controls".into(), Tag::Compound(c));
    }
    // Rollers (`Filter`, `ScrollValue`): x, y, z and the mode.
    if !rollers.is_empty() {
        let mut c = horizon_world::nbt::Compound::new();
        let nums = rollers
            .iter()
            .flat_map(|(p, (_, m))| {
                let mode = actors::RollingMode::ALL
                    .iter()
                    .position(|x| x == m)
                    .unwrap_or(0);
                [p[0], p[1], p[2], mode as i32]
            })
            .collect();
        c.insert("nums".into(), Tag::IntArray(nums));
        let items = rollers.iter().map(|(_, (f, _))| text(f)).collect();
        c.insert("filters".into(), Tag::List(items));
        own.insert("rollers".into(), Tag::Compound(c));
    }
    // Track observers' filters: the filter stacks.
    if !observers.is_empty() {
        let mut c = horizon_world::nbt::Compound::new();
        let pos = observers.iter().flat_map(|(p, _)| *p).collect();
        let items = observers
            .iter()
            .map(|(_, f)| super::create_items::filter_to_nbt(f))
            .collect();
        c.insert("pos".into(), Tag::IntArray(pos));
        c.insert("items".into(), Tag::List(items));
        own.insert("observers".into(), Tag::Compound(c));
    }
    data.insert("create_glue".into(), Tag::Compound(own));
}

/// What `glue_chunk_save` wrote of the blocks that ride contraptions and the track observers.
fn mounted_chunk_load(dim: usize, saved: &horizon_world::nbt::Compound) {
    use horizon_world::nbt::Tag;
    let list = |key: &str| -> Option<(Vec<i32>, Vec<Tag>)> {
        match saved.get(&key.into()) {
            Some(Tag::Compound(c)) => {
                let nums = match c.get(&"nums".into()).or_else(|| c.get(&"pos".into())) {
                    Some(Tag::IntArray(v)) => v.clone(),
                    _ => return None,
                };
                let items = match c.get(&"filters".into()).or_else(|| c.get(&"items".into())) {
                    Some(Tag::List(v)) => v.clone(),
                    _ => return None,
                };
                Some((nums, items))
            }
            _ => None,
        }
    };
    let text = |t: &Tag| match t {
        Tag::String(s) => Some(s.to_string_lossy()).filter(|s| !s.is_empty()),
        _ => None,
    };
    let controls: Vec<([i32; 3], actors::Controls)> = list("controls")
        .map(|(nums, items)| {
            nums.chunks_exact(4)
                .zip(&items)
                .map(|(n, f)| {
                    let c = actors::Controls {
                        filter: text(f),
                        disabled: n[3] & 1 != 0,
                        powered: n[3] & 2 != 0,
                    };
                    ([n[0], n[1], n[2]], c)
                })
                .collect()
        })
        .unwrap_or_default();
    let rollers: Vec<([i32; 3], (Option<String>, actors::RollingMode))> = list("rollers")
        .map(|(nums, items)| {
            nums.chunks_exact(4)
                .zip(&items)
                .map(|(n, f)| {
                    let mode = actors::RollingMode::ALL
                        .get(n[3].clamp(0, 2) as usize)
                        .copied()
                        .unwrap_or_default();
                    ([n[0], n[1], n[2]], (text(f), mode))
                })
                .collect()
        })
        .unwrap_or_default();
    let observers: Vec<([i32; 3], create_engine::filter::Filter)> = list("observers")
        .map(|(nums, items)| {
            nums.chunks_exact(3)
                .zip(&items)
                .filter_map(|(p, i)| {
                    Some(([p[0], p[1], p[2]], super::create_items::filter_from_nbt(i)?))
                })
                .collect()
        })
        .unwrap_or_default();
    with_world(|w| {
        let d = &mut w.dims[dim];
        d.controls.extend(controls);
        d.rollers.extend(rollers);
        for (p, f) in observers {
            d.railway.set_observer_filtered(p, true);
            d.observer_filters.insert(p, f);
        }
    });
}

/// A column's glue back as it loads.
pub(super) fn glue_chunk_load(dim: usize, saved: Option<&horizon_world::nbt::Compound>) {
    if let Some(s) = saved {
        mounted_chunk_load(dim, s);
    }
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
        // What its block entity held rides with it (`blockEntityData`), out of the world's
        // tables before the block leaves.
        if block == create_engine::Block::ContraptionControls {
            let c = with_world(|w| w.dims[dim].controls.remove(&at(*o)))
                .flatten()
                .unwrap_or_default();
            carried.controls.push((*o, c));
        }
        if let (Some(kind), Some(facing)) = (Kind::of(block), facing) {
            let id = NEXT_BREAKER.get() + 1;
            NEXT_BREAKER.set(id);
            let mut a = Actor::new(*o, kind, facing, -id);
            if kind == Kind::Roller
                && let Some((filter, mode)) =
                    with_world(|w| w.dims[dim].rollers.remove(&at(*o))).flatten()
            {
                a.filter = filter;
                a.mode = mode;
            }
            carried.actors.push(a);
        }
    }
    // `startMoving`: controls set off turn their kind off.
    let controls: Vec<actors::Controls> = carried.controls.iter().map(|c| c.1.clone()).collect();
    actors::start_moving(&mut carried, &controls);
    if !carried.controls.is_empty() || carried.actors.iter().any(|a| a.kind == Kind::Roller) {
        game.native_mark_chunk_unsaved(dimension, [origin[0] >> 4, origin[2] >> 4]);
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
    // Block entity data back with the blocks put back: rollers' settings, and contraption
    // controls off as the contraption has their kind (`ContraptionControlsMovement.stopMoving`).
    let block_at = |game: &Game<'_>, p: [i32; 3]| {
        let s = game.world.block(p[0], p[1], p[2]).0;
        with_world(|w| w.table.get(s).map(|(b, _)| b)).flatten()
    };
    let mut settled = false;
    for a in carried.actors.iter().filter(|a| a.kind == Kind::Roller) {
        let p = at(a.local);
        if block_at(game, p) == Some(create_engine::Block::MechanicalRoller) {
            let v = (a.filter.clone(), a.mode);
            with_world(|w| w.dims[dim].rollers.insert(p, v));
            settled = true;
        }
    }
    for (o, mut c) in std::mem::take(&mut carried.controls) {
        let p = at(o);
        if block_at(game, p) == Some(create_engine::Block::ContraptionControls) {
            c.disabled = actors::stays_disabled(&carried.disabled, &c.filter);
            with_world(|w| w.dims[dim].controls.insert(p, c));
            settled = true;
        }
    }
    if settled {
        game.native_mark_chunk_unsaved(dimension, [origin[0] >> 4, origin[2] >> 4]);
    }
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
    /// The contraption's storage: its dimension and key.
    dim: usize,
    key: [i32; 3],
}

/// The block `item` places, a slab doubled (`getStateToPaveWith`).
fn pave_state(game: &Game<'_>, item: &str) -> Option<BlockStateId> {
    let state = game.items().get(item)?.block?;
    let registry = game.pack.assets.registry;
    let s = registry.state(state)?;
    if s.properties.contains_key("type") && s.name.ends_with("_slab") {
        let mut d = s.clone();
        d.properties.insert("type".into(), "double".into());
        return registry.id(&d);
    }
    Some(state)
}

/// `getStateToPaveWithAsSlab`: the slab of `item`'s block, by Create's names (`_slab`, its
/// plural's or its planks' wood), None when there is none.
fn pave_slab(game: &Game<'_>, item: &str) -> Option<String> {
    let state = game.items().get(item)?.block?;
    let name = game.pack.assets.registry.state(state)?.name.clone();
    if name.ends_with("_slab") {
        return Some(name);
    }
    let (ns, path) = name.split_once(':')?;
    let mut tries = vec![format!("{path}_slab")];
    if path.len() > 1
        && let Some(p) = path.strip_suffix('s')
    {
        tries.push(format!("{p}_slab"));
    }
    if path.len() > 7
        && let Some(p) = path.strip_suffix("planks")
    {
        tries.push(format!("{}_slab", p.trim_end_matches('_')));
    }
    tries
        .into_iter()
        .map(|t| format!("{ns}:{t}"))
        .find(|t| game.pack.assets.registry.default_state(t).is_some())
}

/// `RollerBlockEntity.isValidMaterial`: a block item whose block has no block entity, is no
/// stair, and has a full block's collision.
pub(super) fn valid_pave_material(game: &Game<'_>, item: &str) -> bool {
    let Some(state) = pave_state(game, item) else {
        return false;
    };
    let name = game
        .pack
        .assets
        .registry
        .state(state)
        .map_or(String::new(), |s| s.name.clone());
    let flags = game.res.behaviours().facts(state).flags;
    use horizon_world::level::data::state_flags as f;
    flags & f::BLOCK_ENTITY == 0
        && flags & f::FULL_COLLISION != 0
        && flags & f::COLLISION_EMPTY == 0
        && !name.ends_with("_stairs")
}

impl actors::Probe for Reader<'_, '_> {
    fn material(&self, filter: Option<&str>) -> u32 {
        STORAGE.with_borrow_mut(|m| {
            let Some(list) = m.get_mut(&(self.dim, self.key)) else {
                return 0;
            };
            let mut n = 0;
            for (_, s) in list.iter_mut() {
                for stack in body::slots(s).into_iter().flatten().flatten() {
                    if filter.is_none_or(|f| *stack.item == *f) {
                        n += stack.count;
                    }
                }
            }
            n
        })
    }

    fn fill(&self, pos: [i32; 3], filter: Option<&str>) -> actors::Fill {
        let game = self.game;
        if !game.world.loaded(pos[0], pos[2]) {
            return actors::Fill::Blocked;
        }
        let here = game.world.block(pos[0], pos[1], pos[2]);
        let b = game.res.behaviours();
        let registry = game.pack.assets.registry;
        let name = registry.state(here).map(|s| s.name.as_str());
        let same = match filter {
            None => b.is_air(here),
            Some(item) => pave_state(game, item)
                .and_then(|p| registry.state(p))
                .is_some_and(|p| Some(p.name.as_str()) == name),
        };
        if same {
            return actors::Fill::Same;
        }
        let tagged = |t: &str| b.tag(t).is_some_and(|s| s.contains(b.block(here)));
        let empty =
            b.facts(here).flags & horizon_world::level::data::state_flags::COLLISION_EMPTY != 0;
        if !tagged("minecraft:leaves")
            && !b.is_replaceable(here)
            && (!empty || tagged("minecraft:portals"))
        {
            return actors::Fill::Blocked;
        }
        actors::Fill::Open
    }

    fn is_pave(&self, pos: [i32; 3], item: &str, slab: bool) -> bool {
        let game = self.game;
        let here = game.world.block(pos[0], pos[1], pos[2]);
        let Some(name) = game
            .pack
            .assets
            .registry
            .state(here)
            .map(|s| s.name.clone())
        else {
            return false;
        };
        if slab {
            return pave_slab(game, item).is_some_and(|s| s == name);
        }
        pave_state(game, item)
            .and_then(|p| game.pack.assets.registry.state(p))
            .is_some_and(|p| p.name == name)
    }

    fn pave_falls(&self, item: &str) -> bool {
        // The falling blocks the breakers know (`FallingBlock`).
        let game = self.game;
        let Some(state) = pave_state(game, item) else {
            return false;
        };
        game.pack.assets.registry.state(state).is_some_and(|s| {
            let n = s.name.as_str();
            matches!(
                n,
                "minecraft:sand"
                    | "minecraft:red_sand"
                    | "minecraft:gravel"
                    | "minecraft:anvil"
                    | "minecraft:dragon_egg"
            ) || n.ends_with("_concrete_powder")
        })
    }

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
            // RollerMovementBehaviour.canBreak: nothing beside a portal, not track, not without
            // collision.
            Kind::Roller => {
                !no_collision
                    && !track
                    && !Dir::ALL.into_iter().any(|d| {
                        let o = d.offset();
                        let s = game
                            .world
                            .block(pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]);
                        b.tag("minecraft:portals")
                            .is_some_and(|t| t.contains(b.block(s)))
                    })
            }
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
        if let Kind::Interface { fluid } = a.kind {
            stalled |= tick_interface(game, dim, key, a, position, fluid);
            continue;
        }
        stalled |= a.tick(
            position,
            facing,
            actors::CONTROLLED_LOWER,
            &Reader { game, dim, key },
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
            // `RollerMovementBehaviour.destroyBlock`: nothing from a block needing an iron,
            // stone or diamond tool, else each stack kept one time in two.
            let b = game.res.behaviours();
            let state = game.world.block(p[0], p[1], p[2]);
            let tagged = |t: &str| b.tag(t).is_some_and(|s| s.contains(b.block(state)));
            let no_harvest = a.kind == Kind::Roller
                && (tagged("minecraft:needs_iron_tool")
                    || tagged("minecraft:needs_stone_tool")
                    || tagged("minecraft:needs_diamond_tool"));
            let shovel = ItemStack::new("minecraft:iron_shovel", 1);
            let drops = body::break_block(game, dimension, p, snow.then_some(&shovel));
            for stack in drops {
                if a.kind == Kind::Roller && (no_harvest || body::random_float(game) < 0.5) {
                    continue;
                }
                collect(game, dim, key, a, stack);
            }
        }
        Effect::Pave(p) => pave(game, dimension, dim, key, a, p),
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

/// A roller's `tryFill` placing: one item its filter passes out of the contraption's storage
/// (`ItemHelper.extract(..., 1, false)`), its block set (`setBlockAndUpdate`; air with no
/// filter).
fn pave(game: &mut Game<'_>, dimension: &str, dim: usize, key: [i32; 3], a: &Actor, p: [i32; 3]) {
    let filter = a.filter.as_deref();
    let taken = STORAGE.with_borrow_mut(|m| {
        let list = m.get_mut(&(dim, key))?;
        for (_, s) in list.iter_mut() {
            let Some(slots) = body::slots(s) else {
                continue;
            };
            for slot in slots.iter_mut() {
                let Some(stack) = slot.as_mut() else {
                    continue;
                };
                if filter.is_some_and(|f| *stack.item != *f) {
                    continue;
                }
                stack.count -= 1;
                if stack.count == 0 {
                    *slot = None;
                }
                return Some(());
            }
        }
        None
    });
    if taken.is_none() {
        return;
    }
    let state = match filter {
        Some(item) => pave_state(game, item),
        None => game.pack.assets.registry.default_state("minecraft:air"),
    };
    if let Some(s) = state {
        game.native_set_block(dimension, p, s, super::native_hooks::flags::UPDATE_ALL);
    }
}

/// The stationary interfaces a moving one may reach this tick, read out of the world: each one's
/// block (its kind and facing) and timer; written back after.
struct Interfaces {
    fluid: bool,
    found: Vec<([i32; 3], Dir, bool, create_engine::psi::Stationary)>,
}

impl create_engine::psi::World for Interfaces {
    fn interface_at(&self, pos: [i32; 3], facing: Dir) -> bool {
        self.found.iter().any(|(p, f, fluid, s)| {
            *p == pos && *f == facing.opposite() && *fluid == self.fluid && !s.powered
        })
    }
    fn stationary(&mut self, pos: [i32; 3]) -> Option<&mut create_engine::psi::Stationary> {
        let fluid = self.fluid;
        self.found
            .iter_mut()
            .find(|(p, _, f, _)| *p == pos && *f == fluid)
            .map(|e| &mut e.3)
    }
}

/// A moving interface's tick (`PortableStorageInterfaceMovement`) against the stationary
/// interfaces at its block, the one beyond and the one it works with; whether it stalls.
fn tick_interface(
    game: &Game<'_>,
    dim: usize,
    key: [i32; 3],
    a: &mut Actor,
    position: [f64; 3],
    fluid: bool,
) -> bool {
    let grid = position.map(|v| v.floor() as i32);
    let o = a.facing.offset();
    let mut near = vec![grid, [grid[0] + o[0], grid[1] + o[1], grid[2] + o[2]]];
    near.extend(a.interface.working);
    let mut world = Interfaces {
        fluid,
        found: Vec::new(),
    };
    for p in near {
        let state = game.world.block(p[0], p[1], p[2]).0;
        let found = with_world(|w| {
            let (b, info) = w.table.get(state)?;
            let f = match b {
                create_engine::Block::PortableStorageInterface => false,
                create_engine::Block::PortableFluidInterface => true,
                _ => return None,
            };
            Some((p, info.facing?, f, *w.dims[dim].psis.get(&p)?))
        })
        .flatten();
        if let Some(e) = found
            && !world.found.iter().any(|x| x.0 == p)
        {
            world.found.push(e);
        }
    }
    let stalled = a.tick_interface(position, &mut world, key);
    with_world(|w| {
        let d = &mut w.dims[dim];
        for (p, _, _, s) in world.found {
            let was = d.psis.get(&p).copied();
            if was == Some(s) {
                continue;
            }
            if was.is_some_and(|w| w.is_connected()) != s.is_connected() {
                d.analog_checks.push(p);
            }
            if was.is_some_and(|w| w.connected.is_none()) && s.connected.is_some() {
                // `startTransferringTo` → `updateNeighborsAt`: a chute under it looks again.
                d.chute_pulls.wake([p[0], p[1] - 1, p[2]]);
            }
            d.psis.insert(p, s);
            if !d.psi_running.contains(&p) {
                d.psi_running.push(p);
            }
        }
    });
    stalled
}

/// The contraption an item interface at `pos` hands its storage to now (`canTransfer`), if
/// any: whatever reaches the interface reaches that storage.
fn psi_connected(game: &Game<'_>, dim: usize, pos: [i32; 3]) -> Option<[i32; 3]> {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    states::index(state)?;
    with_world(|w| {
        let (b, _) = w.table.get(state)?;
        if b != create_engine::Block::PortableStorageInterface {
            return None;
        }
        w.dims[dim].psis.get(&pos)?.can_transfer()
    })
    .flatten()
}

/// Whether an item interface stands at `pos` (connected or not).
pub(super) fn is_item_interface(game: &Game<'_>, pos: [i32; 3]) -> bool {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    states::index(state).is_some()
        && with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
            == Some(create_engine::Block::PortableStorageInterface)
}

/// How much of `stack` `psi_insert` would put in (simulated); None when no connected item
/// interface is at `pos`.
pub(super) fn psi_room(
    game: &Game<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: &ItemStack,
) -> Option<u32> {
    let key = psi_connected(game, dim, pos)?;
    let limit = body::slot_limit(game, stack);
    Some(STORAGE.with_borrow_mut(|m| {
        let Some(list) = m.get_mut(&(dim, key)) else {
            return 0;
        };
        let mut room = 0u32;
        for (_, s) in list.iter_mut() {
            for slot in body::slots(s).into_iter().flatten() {
                room += match slot {
                    Some(s) if body::same_stack(s, stack) => limit.saturating_sub(s.count),
                    Some(_) => 0,
                    None => limit,
                };
            }
        }
        room.min(stack.count)
    }))
}

/// `onContentTransferred`: the connection holds `getTransferTimeout` ticks more.
fn psi_transferred(dim: usize, pos: [i32; 3]) {
    with_world(|w| {
        let d = &mut w.dims[dim];
        if let Some(s) = d.psis.get_mut(&pos) {
            s.on_content_transferred();
        }
        if !d.psi_running.contains(&pos) {
            d.psi_running.push(pos);
        }
    });
}

/// `InterfaceItemHandler.insertItem` (`ItemHandlerHelper.insertItemStacked` as Create's chutes
/// and funnels put items in): into the storage of the contraption the interface at `pos` is
/// connected to, onto the same items first. Err(stack) when no connected item interface is
/// there; Ok(what did not fit).
pub(super) fn psi_insert(
    game: &Game<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: ItemStack,
) -> Result<Option<ItemStack>, ItemStack> {
    let Some(key) = psi_connected(game, dim, pos) else {
        return Err(stack);
    };
    let limit = body::slot_limit(game, &stack);
    let before = stack.count;
    let left = STORAGE.with_borrow_mut(|m| {
        let Some(list) = m.get_mut(&(dim, key)) else {
            return Some(stack);
        };
        let mut stack = stack;
        // Same items first, then empty slots, storage block by storage block.
        for empty in [false, true] {
            for (_, s) in list.iter_mut() {
                let Some(slots) = body::slots(s) else {
                    continue;
                };
                for slot in slots.iter_mut() {
                    if stack.count == 0 {
                        return None;
                    }
                    match slot {
                        Some(s) if !empty && body::same_stack(s, &stack) => {
                            let put = stack.count.min(limit.saturating_sub(s.count));
                            s.count += put;
                            stack.count -= put;
                        }
                        None if empty => {
                            let put = stack.count.min(limit);
                            *slot = Some(ItemStack {
                                count: put,
                                ..stack.clone()
                            });
                            stack.count -= put;
                        }
                        _ => {}
                    }
                }
            }
        }
        (stack.count > 0).then_some(stack)
    });
    if left.as_ref().map_or(0, |s| s.count) != before {
        psi_transferred(dim, pos);
    }
    Ok(left)
}

/// `InterfaceItemHandler.extractItem` through `ItemHelper.extract` (up to `amount` of the first
/// item `test` passes, gathered from the slots in order) from the storage of the contraption the
/// interface at `pos` is connected to. Err(()) when no connected item interface is there.
pub(super) fn psi_extract(
    game: &Game<'_>,
    dim: usize,
    pos: [i32; 3],
    test: impl Fn(&Game<'_>, &ItemStack) -> bool,
    amount: u32,
) -> Result<Option<ItemStack>, ()> {
    let key = psi_connected(game, dim, pos).ok_or(())?;
    let taken = STORAGE.with_borrow_mut(|m| {
        let list = m.get_mut(&(dim, key))?;
        let mut out: Option<ItemStack> = None;
        for (_, s) in list.iter_mut() {
            let Some(slots) = body::slots(s) else {
                continue;
            };
            for slot in slots.iter_mut() {
                let want = amount - out.as_ref().map_or(0, |o| o.count);
                if want == 0 {
                    return out;
                }
                let Some(stack) = slot.as_mut() else {
                    continue;
                };
                let fits = match &out {
                    Some(o) => body::same_stack(o, stack),
                    None => test(game, stack),
                };
                if !fits {
                    continue;
                }
                let n = want.min(stack.count);
                stack.count -= n;
                match &mut out {
                    Some(o) => o.count += n,
                    None => {
                        out = Some(ItemStack {
                            count: n,
                            ..stack.clone()
                        })
                    }
                }
                if stack.count == 0 {
                    *slot = None;
                }
            }
        }
        out
    });
    if taken.is_some() {
        psi_transferred(dim, pos);
    }
    Ok(taken)
}

thread_local! {
    /// Rollers placed this tick (`setPlacedBy` → `searchForSharedValues`), once their facing is
    /// set.
    static ROLLERS_PLACED: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
}

/// A roller was placed: it takes the settings of a roller beside it across its facing.
pub(super) fn roller_placed(dim: usize, pos: [i32; 3]) {
    ROLLERS_PLACED.with_borrow_mut(|r| r.push((dim, pos)));
}

/// The roller at `pos`: its state and facing.
fn roller_at(game: &Game<'_>, pos: [i32; 3]) -> Option<(BlockStateId, Dir)> {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    with_world(|w| {
        let (b, info) = w.table.get(state.0)?;
        (b == create_engine::Block::MechanicalRoller).then_some((state, info.facing?))
    })
    .flatten()
}

/// `shareValuesToAdjacent`: the rollers in line across its facing, of the same state, up to 99
/// each way, take its filter and mode.
fn share_roller(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    let Some((state, facing)) = roller_at(game, pos) else {
        return;
    };
    let Some(v) = with_world(|w| w.dims[dim].rollers.get(&pos).cloned()).flatten() else {
        return;
    };
    let o = create_engine::display::clockwise(facing).offset();
    for side in [1, -1] {
        for i in 1..100 {
            let p = [pos[0] + o[0] * side * i, pos[1], pos[2] + o[2] * side * i];
            if game.world.block(p[0], p[1], p[2]) != state {
                break;
            }
            with_world(|w| w.dims[dim].rollers.insert(p, v.clone()));
        }
    }
    let dimension = game.dim.id().to_owned();
    game.native_mark_chunk_unsaved(&dimension, [pos[0] >> 4, pos[2] >> 4]);
}

/// After the contraptions moved: rollers placed take a neighbour's settings, contraption
/// controls follow redstone, stationary interfaces read their power and those connected or
/// letting go tick. Nothing runs while none of these has anything to do.
pub(super) fn tick_mounted(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let placed = ROLLERS_PLACED.with_borrow_mut(|r| {
        if r.is_empty() {
            Vec::new()
        } else {
            let mine: Vec<[i32; 3]> = r.iter().filter(|e| e.0 == dim).map(|e| e.1).collect();
            r.retain(|e| e.0 != dim);
            mine
        }
    });
    for pos in placed {
        // `searchForSharedValues`: the first roller beside it of the same state.
        let Some((state, facing)) = roller_at(game, pos) else {
            continue;
        };
        let o = create_engine::display::clockwise(facing).offset();
        for side in [1, -1] {
            let p = [pos[0] + o[0] * side, pos[1], pos[2] + o[2] * side];
            if game.world.block(p[0], p[1], p[2]) != state {
                continue;
            }
            let Some(v) = with_world(|w| w.dims[dim].rollers.get(&p).cloned()).flatten() else {
                continue;
            };
            with_world(|w| w.dims[dim].rollers.insert(pos, v));
            share_roller(game, dim, pos);
            break;
        }
    }
    let Some((controls, psis, running)) = with_world(|w| {
        let d = &mut w.dims[dim];
        if d.controls_checks.is_empty() && d.psi_checks.is_empty() && d.psi_running.is_empty() {
            return None;
        }
        Some((
            std::mem::take(&mut d.controls_checks),
            std::mem::take(&mut d.psi_checks),
            !d.psi_running.is_empty(),
        ))
    })
    .flatten() else {
        return;
    };
    let signal = |game: &mut Game<'_>, p: [i32; 3]| {
        let at = horizon_core::position::BlockPos::new(p[0], p[1], p[2]);
        game.with_level(|b, level, ticks| b.has_neighbor_signal(level, ticks, at))
    };
    for pos in controls {
        let powered = signal(game, pos);
        let changed = with_world(|w| {
            w.dims[dim]
                .controls
                .get_mut(&pos)
                .is_some_and(|c| c.powered(powered))
        });
        if changed == Some(true) {
            game.native_mark_chunk_unsaved(dimension, [pos[0] >> 4, pos[2] >> 4]);
        }
    }
    for pos in psis {
        let powered = signal(game, pos);
        with_world(|w| {
            let d = &mut w.dims[dim];
            let Some(s) = d.psis.get_mut(&pos) else {
                return;
            };
            let was = s.is_connected() && s.connected.is_some();
            s.neighbour_changed(powered);
            if was != (s.is_connected() && s.connected.is_some()) {
                d.analog_checks.push(pos);
            }
            if !s.idle() && !d.psi_running.contains(&pos) {
                d.psi_running.push(pos);
            }
        });
    }
    if !running {
        return;
    }
    with_world(|w| {
        let d = &mut w.dims[dim];
        let mut list = std::mem::take(&mut d.psi_running);
        list.retain(|p| {
            let Some(s) = d.psis.get_mut(p) else {
                return false;
            };
            let was = s.is_connected();
            s.tick();
            if was != s.is_connected() {
                d.analog_checks.push(*p);
            }
            !s.idle()
        });
        d.psi_running = list;
    });
}

/// A click on contraption controls or a roller (`useWithoutItem`, `FilteringBehaviour`):
/// true when it was one of them. Controls: on the filter slot the held item becomes the kind of
/// actor it switches (`create:contraption_controlled` only; an empty hand clears it), elsewhere
/// it turns its kind off or on, with Create's status and click. A roller: on its filter slot
/// the held block item becomes its pave material (`isValidMaterial`), an empty hand clears it,
/// shared with the rollers in line; its mode is set with `/create_debug roller` (the value box,
/// gap 7).
pub(super) fn use_mounted(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    face: [i32; 3],
    block: create_engine::Block,
    back: bool,
    canceled: &mut bool,
) -> bool {
    use create_engine::Block;
    if !matches!(block, Block::ContraptionControls | Block::MechanicalRoller) {
        return false;
    }
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    let Some(facing) = with_world(|w| w.table.get(state).and_then(|(_, i)| i.facing)).flatten()
    else {
        return true;
    };
    let held = game.native_held().map(|h| h.0);
    if held.as_deref() == Some("create:wrench") {
        return true;
    }
    let on_slot = |game: &Game<'_>, slot: ([f64; 3], f64)| {
        game.pick().is_some_and(|hit| {
            let local = [0, 1, 2].map(|i| hit.point[i] - f64::from(pos[i]));
            let d2: f64 = (0..3).map(|i| (local[i] - slot.0[i]).powi(2)).sum();
            d2.sqrt() < slot.1
        })
    };
    let _ = face;
    let roller = block == Block::MechanicalRoller;
    let slot = if roller {
        actors::roller_slot(facing, true)
    } else {
        actors::controls_slot(facing)
    };
    if !back && may_use_value_box(game) && on_slot(game, slot) {
        // `FilteringBehaviour.onShortInteract`: the predicate, then a ghost copy of the item.
        let item = held.clone();
        let fits = match item.as_deref() {
            None => true,
            Some(i) if roller => valid_pave_material(game, i),
            Some(i) => actors::CONTRAPTION_CONTROLLED.contains(&i),
        };
        *canceled = true;
        if !fits {
            return true;
        }
        with_world(|w| {
            let d = &mut w.dims[dim];
            if roller {
                d.rollers.entry(pos).or_default().0 = item;
            } else {
                d.controls.entry(pos).or_default().filter = item;
            }
        });
        if roller {
            share_roller(game, dim, pos);
        }
        let dimension = game.dim.id().to_owned();
        game.native_mark_chunk_unsaved(&dimension, [pos[0] >> 4, pos[2] >> 4]);
        let center = pos.map(|v| f64::from(v) + 0.5);
        game.audio.play(
            "minecraft:entity.item_frame.add_item",
            Some(center),
            0.25,
            0.1,
        );
        return true;
    }
    // Sneaking with something held skips the block's use (`ServerPlayerGameMode.useItemOn`).
    if roller || (back && held.is_some()) {
        return true;
    }
    // ContraptionControlsBlock.useWithoutItem.
    *canceled = true;
    let Some((on, filter)) = with_world(|w| {
        let c = w.dims[dim].controls.entry(pos).or_default();
        let on = c.click();
        (on, c.filter.clone())
    }) else {
        return true;
    };
    controls_status(game, pos, filter.as_deref(), on);
    let dimension = game.dim.id().to_owned();
    game.native_mark_chunk_unsaved(&dimension, [pos[0] >> 4, pos[2] >> 4]);
    true
}

/// `ContraptionControlsBlockEntity.sendStatus` and the `controller_click` sound (pitch 1.5 on,
/// 0.8 off).
fn controls_status(game: &mut Game<'_>, at: [i32; 3], filter: Option<&str>, on: bool) {
    use horizon_client::text::Text;
    let state = Text::translate(
        if on {
            "create.contraption.controls.actor_toggle.on"
        } else {
            "create.contraption.controls.actor_toggle.off"
        },
        &[],
    );
    let line = match filter {
        None => Text::translate("create.contraption.controls.all_actor_toggle", &[state]),
        Some(item) => Text::translate(
            "create.contraption.controls.specific_actor_toggle",
            &[Text::literal(item), state],
        ),
    };
    if let Some(p) = game.native_acting_player() {
        game.native_action_bar(p.uuid, line);
    }
    let center = at.map(|v| f64::from(v) + 0.5);
    game.audio.play(
        "create:controller_click",
        Some(center),
        1.0,
        if on { 1.5 } else { 0.8 },
    );
}

/// `/create_debug roller <pos> <mode>`: a roller's mode (its value box's scroll), shared with the
/// rollers in line.
pub(super) fn debug_roller(game: &mut Game<'_>, dim: usize, pos: [i32; 3], mode: &str) -> String {
    let Some(m) = actors::RollingMode::read(mode) else {
        return "modes: tunnel_pave, straight_fill, wide_fill".into();
    };
    if roller_at(game, pos).is_none() {
        return format!("no roller at {} {} {}", pos[0], pos[1], pos[2]);
    }
    let filter = with_world(|w| {
        let r = w.dims[dim].rollers.entry(pos).or_default();
        r.1 = m;
        r.0.clone()
    })
    .flatten();
    share_roller(game, dim, pos);
    format!(
        "roller {} {} {}: {} {}",
        pos[0],
        pos[1],
        pos[2],
        m.name(),
        filter.as_deref().unwrap_or("no material")
    )
}

/// `/create_debug controls <contraption> <filter|all|block>`: a click on contraption controls
/// carried by the moving contraption at a piston or gantry carriage's start
/// (`ContraptionControlsMovingInteraction.handlePlayerInteraction`; clicking a moving
/// contraption needs gap 3). `block` clicks the first contraption controls block it carries,
/// with that block's filter, its status and click sound; a filter names the kind directly.
pub(super) fn debug_controls(
    game: &mut Game<'_>,
    dim: usize,
    key: [i32; 3],
    filter: Option<Option<String>>,
) -> String {
    let mut effects = Vec::new();
    let done = with_world(|w| {
        let d = &mut w.dims[dim];
        let (carried, origin) = match d.pistons.get_mut(&key) {
            Some(m) => (&mut m.carried, m.anchor),
            None => {
                let g = d.gantries.iter_mut().find(|g| g.start == key)?;
                (&mut g.carried, g.start)
            }
        };
        let (filter, at) = match filter {
            Some(f) => (f, None),
            None => {
                let Some((o, c)) = carried.controls.first() else {
                    return Some(None);
                };
                (c.filter.clone(), Some([0, 1, 2].map(|i| origin[i] + o[i])))
            }
        };
        let kinds: Vec<&str> = carried.actors.iter().map(|a| a.kind.item()).collect();
        let disable = actors::toggle(&mut carried.disabled, &filter, &kinds);
        actors::set_active(&mut carried.actors, &filter, !disable, &mut effects);
        Some(Some((disable, carried.actors.clone(), filter, at)))
    })
    .flatten();
    let (disable, list, filter, at) = match done {
        Some(Some(done)) => done,
        Some(None) => {
            return format!(
                "no contraption controls on the contraption at {} {} {}",
                key[0], key[1], key[2]
            );
        }
        None => return format!("no moving contraption at {} {} {}", key[0], key[1], key[2]),
    };
    apply_cracks(game, &list, &effects);
    if let Some(at) = at {
        controls_status(game, at, filter.as_deref(), !disable);
    }
    let what = filter.as_deref().unwrap_or("all actors");
    format!("{what} {}", if disable { "off" } else { "on" })
}

/// Create's contraption debug commands (`glue`, `controls`); false for any other command.
pub(super) fn contraption_command(game: &mut Game<'_>, command: &str) -> bool {
    let glue_args = command.strip_prefix("create_debug glue ");
    let controls_args = command.strip_prefix("create_debug controls ");
    let roller_args = command.strip_prefix("create_debug roller ");
    if glue_args.is_none() && controls_args.is_none() && roller_args.is_none() {
        return false;
    }
    let player = game.native_acting_player();
    let dimension = player
        .as_ref()
        .map_or_else(|| "minecraft:overworld".to_owned(), |p| p.dimension.clone());
    let here = player.as_ref().map(|p| p.position);
    let rest = glue_args.or(controls_args).or(roller_args).unwrap_or("");
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
    } else if roller_args.is_some() {
        let dim = dimension_index(&dimension).unwrap_or(0);
        match (pos(0), tokens.get(3)) {
            (Some(at), Some(mode)) => debug_roller(game, dim, at, mode),
            _ => "usage: create_debug roller <x> <y> <z> <tunnel_pave|straight_fill|wide_fill>"
                .into(),
        }
    } else {
        let dim = dimension_index(&dimension).unwrap_or(0);
        match (pos(0), tokens.get(3)) {
            (Some(key), Some(f)) => {
                let filter = match *f {
                    "block" => None,
                    "all" => Some(None),
                    f => Some(Some(f.to_owned())),
                };
                debug_controls(game, dim, key, filter)
            }
            _ => "usage: create_debug controls <x> <y> <z> <item|all|block>".into(),
        }
    };
    game.native_feedback(horizon_client::text::Text::literal(line));
    true
}
