//! What moving contraptions carry besides their blocks, and Create's glue and stickers: super
//! glue areas the structure search reads (set by `/create_debug glue`, standing in for the
//! item's selection), the sticker's redstone, actors (drills, saws, harvesters, ploughs,
//! rollers, interfaces) ticked only while their contraption moves, mounted storage (chests and
//! barrels) that keeps its items and takes what the actors collect, and contraption controls
//! (`/create_debug controls`). Create's rules are in `create_engine::glue` and
//! `create_engine::actors`. Saved under `create_glue`.
//! Track observers' filters (the trains' slot, read as the items port reads filters) are kept
//! there too (`observers`), as in the legacy glue.

use super::super::items::filters;
use super::super::persist::{mark_pos_unsaved, mark_unsaved};
use super::super::server::{ServerLevel, UPDATE_CLIENTS, update_all};
use super::super::states::{self, key};
use super::super::world::with_world;
use super::{AIRS, add, center, create_info, dir_of, flag, found, prop};
use create_engine::Block;
use create_engine::actors::{self, Actor, Carried, Effect, Kind};
use create_engine::glue::{self, Glue};
use create_engine::kinetics::Dir;
use mod_api::{
    BlockHit, BlockPart, BlockShapeKind, Command, CommandArgumentValue, CommandSource, Compound,
    Flow, Hand, HostBlockStates, HostTransaction, Identifier, InteractionResult, ItemKey, ItemPart,
    ItemStack, OnBlockPlacedByEvent, PlayerAction, PlayerActionSetup, RegisterCommandsEvent,
    Registrar, RunModCommandEvent, ServerHost, SoundSource, Tag, UseItemOnEvent,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// A storage block's items while its contraption moves: its slots.
type Stored = Vec<Option<ItemStack>>;

/// A moving contraption's storage blocks: each one's offset and what it holds.
type Mounted = Vec<([i32; 3], Stored)>;

/// A roller's settings: its pave material and its mode.
type RollerSettings = (Option<String>, actors::RollingMode);

/// A chest's or barrel's slots.
const STORAGE_SLOTS: u32 = 27;

thread_local! {
    /// Storage blocks' items while their contraption moves, by dimension and contraption (a
    /// piston's position, a gantry's start): each block's offset and what it holds.
    static STORAGE: RefCell<HashMap<(usize, [i32; 3]), Mounted>> = RefCell::new(HashMap::new());
    /// `BlockBreakingKineticBlockEntity.NEXT_BREAKER_ID`.
    static NEXT_BREAKER: Cell<i32> = const { Cell::new(0) };
    /// Rollers placed this tick (`setPlacedBy` → `searchForSharedValues`).
    static ROLLERS_PLACED: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
}

/// The world closed: what moving contraptions carried goes with it.
pub(super) fn forget() {
    STORAGE.with_borrow_mut(HashMap::clear);
    NEXT_BREAKER.set(0);
    ROLLERS_PLACED.with_borrow_mut(Vec::clear);
}

pub(super) fn register(r: &mut dyn Registrar) {
    r.block_part(key(Block::ContraptionControls), MountedPart);
    r.block_part(key(Block::MechanicalRoller), MountedPart);
    let roller = r.item_key("create:mechanical_roller");
    r.item_part(roller, RollerItem);
}

fn next_breaker() -> i32 {
    let id = NEXT_BREAKER.get() + 1;
    NEXT_BREAKER.set(id);
    id
}

/// The faces of `p` glue holds, for the structure search.
pub(super) fn glued_at(dim: usize, p: [i32; 3]) -> u8 {
    with_world(|w| glue::glued_faces(&w.dims[dim].glue, p)).unwrap_or(0)
}

// ---------------------------------------------------------------------------------------------
// Mounted storage (`MountedStorageManager` for Create's simple item storages)

fn storage_name(name: &str) -> bool {
    matches!(
        name,
        "minecraft:chest" | "minecraft:trapped_chest" | "minecraft:barrel"
    )
}

/// Whether `pos` holds a storage block a contraption carries.
pub(super) fn is_storage(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    lv.block_name(pos).is_some_and(|n| storage_name(&n))
        && lv.host.block_entity(lv.level, pos).is_some()
}

/// The items of the chest, trapped chest or barrel at `pos`, taken out of it (it is left empty,
/// so it spills nothing when its block goes); None for any other block.
fn take_storage(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> Option<Stored> {
    if !is_storage(lv, pos) {
        return None;
    }
    let entity = lv.host.block_entity(lv.level, pos)?;
    let stored: Stored = (0..STORAGE_SLOTS)
        .map(|i| entity.item_slot(&*lv.host, i).ok().flatten())
        .collect();
    let _ = lv.host.transaction(|host, tx| {
        for (i, s) in stored.iter().enumerate() {
            if s.is_some() {
                entity.set_item_slot(host, tx, i as u32, None)?;
            }
        }
        Ok::<(), mod_api::BlockEntityItemError>(())
    });
    Some(stored)
}

/// The carried items back in the storage block now at `pos` (`addBlocksToWorld`).
fn put_storage(lv: &mut ServerLevel<'_>, pos: [i32; 3], stored: Stored) {
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let _ = lv.host.transaction(|host, tx| {
        for (i, s) in stored.into_iter().enumerate() {
            entity.set_item_slot(host, tx, i as u32, s)?;
        }
        Ok::<(), mod_api::BlockEntityItemError>(())
    });
}

/// A chest slot's limit for a stack (`min(64, getMaxStackSize)`).
fn slot_limit(lv: &ServerLevel<'_>, stack: &ItemStack) -> u32 {
    lv.host.max_stack_size(stack.item()).unwrap_or(64).min(64)
}

fn same_stack(a: &ItemStack, b: &ItemStack) -> bool {
    a.is_stackable_with(b)
}

fn item_id(lv: &ServerLevel<'_>, stack: &ItemStack) -> String {
    stack.item().id(&*lv.host)
}

fn item_key_of(lv: &ServerLevel<'_>, id: &str) -> Option<ItemKey> {
    lv.host.item_key(&Identifier::parse(id).ok()?)
}

fn stack_of(lv: &ServerLevel<'_>, id: &str) -> Option<ItemStack> {
    item_key_of(lv, id).map(ItemStack::of)
}

// ---------------------------------------------------------------------------------------------
// Glue

/// `SuperGlueEntity.isSideSticky` for the glue selection: the sides of a block already sticky.
fn sticky_sides(lv: &ServerLevel<'_>, p: [i32; 3]) -> u8 {
    let Some((name, props)) = lv.name_props(p) else {
        return 0;
    };
    let facing = || prop(&props, "facing").and_then(dir_of).map_or(0, Dir::bit);
    match name.as_str() {
        "minecraft:slime_block" | "minecraft:honey_block" => 0x3f,
        "create:sticky_mechanical_piston"
        | "create:sticker"
        | "create:gantry_carriage"
        | "create:mechanical_bearing"
        | "create:windmill_bearing"
        | "create:clockwork_bearing" => facing(),
        "create:cart_assembler" => Dir::Up.bit(),
        "create:linear_chassis" | "create:secondary_linear_chassis" => {
            let (top, bottom) = match prop(&props, "axis").unwrap_or("y") {
                "x" => (Dir::East, Dir::West),
                "z" => (Dir::South, Dir::North),
                _ => (Dir::Up, Dir::Down),
            };
            (if flag(&props, "sticky_top") {
                top.bit()
            } else {
                0
            }) | (if flag(&props, "sticky_bottom") {
                bottom.bit()
            } else {
                0
            })
        }
        _ => 0,
    }
}

/// `/create_debug glue <from> <to>`: what `SuperGlueSelectionPacket` does once the player
/// picked two corners with the glue item (its charges and reach aside).
fn debug_glue(lv: &mut ServerLevel<'_>, from: [i32; 3], to: [i32; 3]) -> String {
    let dim = lv.dim;
    let placed = glue::place(from, to, |p| found(lv, p).map(|_| sticky_sides(lv, p)));
    match placed {
        Ok(g) => {
            with_world(|w| w.dims[dim].glue.push(g));
            let (cx, cz) = g.column();
            mark_unsaved(dim, [cx, cz]);
            format!(
                "glued {} {} {} to {} {} {}",
                g.min[0], g.min[1], g.min[2], g.max[0], g.max[1], g.max[2]
            )
        }
        Err(glue::Refused::TooFar) => "glue: the corners are 25 blocks apart or more".into(),
        Err(glue::Refused::NotConnected) => "glue: the corners don't hold together".into(),
    }
}

/// The glue areas, contraption controls, rollers and track observers' filters of a column,
/// saved with it under `create_glue`.
pub(super) fn glue_chunk_save(
    host: &dyn mod_api::Host,
    dim: usize,
    chunk: [i32; 2],
    data: &mut Compound,
) {
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
        let mut controls: Vec<([i32; 3], actors::Controls)> = d
            .controls
            .iter()
            .filter(|(p, _)| here(p))
            .map(|(p, c)| (*p, c.clone()))
            .collect();
        controls.sort_by_key(|e| e.0);
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
    // Others' entries (a track observer's) stay beside these.
    let mut own = match data.remove(&"create_glue".into()) {
        Some(Tag::Compound(c)) => c,
        _ => Compound::new(),
    };
    if !areas.is_empty() {
        own.insert("areas".into(), Tag::IntArray(areas));
    }
    // Contraption controls (`Filter`, `Disabled`, `Powered`): x, y, z and the two flags.
    if !controls.is_empty() {
        let mut c = Compound::new();
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
        let mut c = Compound::new();
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
        let mut c = Compound::new();
        let pos = observers.iter().flat_map(|(p, _)| *p).collect();
        let items = observers
            .iter()
            .map(|(_, f)| filters::filter_to_nbt(host, f).unwrap_or_else(|| Tag::String("".into())))
            .collect();
        c.insert("pos".into(), Tag::IntArray(pos));
        c.insert("items".into(), Tag::List(items));
        own.insert("observers".into(), Tag::Compound(c));
    }
    data.insert("create_glue".into(), Tag::Compound(own));
}

/// A column's glue, contraption controls, rollers and track observers' filters back as it
/// loads.
pub(super) fn glue_chunk_load(host: &dyn mod_api::Host, dim: usize, data: &Compound) {
    let Some(Tag::Compound(saved)) = data.get(&"create_glue".into()) else {
        return;
    };
    let list = |key: &str| -> Option<(Vec<i32>, Vec<Tag>)> {
        let Some(Tag::Compound(c)) = saved.get(&key.into()) else {
            return None;
        };
        let Some(Tag::IntArray(nums)) = c.get(&"nums".into()) else {
            return None;
        };
        let Some(Tag::List(items)) = c.get(&"filters".into()) else {
            return None;
        };
        Some((nums.clone(), items.clone()))
    };
    let text = |t: &Tag| match t {
        Tag::String(s) => Some(s.to_string_lossy()).filter(|s| !s.is_empty()),
        _ => None,
    };
    let controls: Vec<([i32; 3], actors::Controls)> = list("controls")
        .map(|(nums, items)| {
            nums.as_chunks::<4>()
                .0
                .iter()
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
    let rollers: Vec<([i32; 3], RollerSettings)> = list("rollers")
        .map(|(nums, items)| {
            nums.as_chunks::<4>()
                .0
                .iter()
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
    let areas: Vec<Glue> = match saved.get(&"areas".into()) {
        Some(Tag::IntArray(v)) => v
            .as_chunks::<6>()
            .0
            .iter()
            .map(|c| Glue {
                min: [c[0], c[1], c[2]],
                max: [c[3], c[4], c[5]],
            })
            .collect(),
        _ => Vec::new(),
    };
    let observers: Vec<([i32; 3], create_engine::filter::Filter)> =
        match saved.get(&"observers".into()) {
            Some(Tag::Compound(c)) => match (c.get(&"pos".into()), c.get(&"items".into())) {
                (Some(Tag::IntArray(pos)), Some(Tag::List(items))) => pos
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .zip(items)
                    .filter_map(|(p, i)| Some((*p, filters::filter_from_nbt(host, i)?)))
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
    with_world(|w| {
        let d = &mut w.dims[dim];
        d.controls.extend(controls);
        d.rollers.extend(rollers);
        for (p, f) in observers {
            d.railway.set_observer_filtered(p, true);
            d.observer_filters.insert(p, f);
        }
        for g in areas {
            if !d.glue.contains(&g) {
                d.glue.push(g);
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

// ---------------------------------------------------------------------------------------------
// Stickers

/// Stickers whose neighbour changed (`StickerBlock.neighborChanged`): `powered` follows the
/// signal and each rise flips `extended`. Nothing runs without a change.
pub(super) fn tick_stickers(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].sticker_checks)).unwrap_or_default();
    if checks.is_empty() {
        return;
    }
    checks.sort();
    checks.dedup();
    for pos in checks {
        let Some((name, props)) = lv.name_props(pos) else {
            continue;
        };
        if name != "create:sticker" {
            continue;
        }
        let (powered, extended) = (flag(&props, "powered"), flag(&props, "extended"));
        let signal = lv.has_neighbor_signal(pos);
        let Some((p, e)) = glue::sticker_neighbor(powered, extended, signal) else {
            continue;
        };
        let text = |v: bool| if v { "true" } else { "false" };
        if e != extended {
            lv.set_property(pos, "extended", text(e), UPDATE_CLIENTS);
        }
        lv.set_property(pos, "powered", text(p), UPDATE_CLIENTS);
    }
}

// ---------------------------------------------------------------------------------------------
// Carrying and putting back

/// What a contraption takes besides its blocks as it assembles (`Contraption.addBlock`,
/// `removeBlocksFromWorld`): its actors (`startMoving`), its storage blocks' items, and the glue
/// that held it, shrunk to it. `origin` is where offset 0 of `blocks` is in the world; `skip`
/// names the blocks anchoring it. Called before its blocks leave the world.
pub(super) fn carry(
    lv: &mut ServerLevel<'_>,
    key: [i32; 3],
    origin: [i32; 3],
    blocks: &[([i32; 3], u32)],
    skip: impl Fn([i32; 3]) -> bool,
) -> Carried {
    let dim = lv.dim;
    let at = |o: [i32; 3]| add(origin, o);
    let mut carried = Carried::default();
    for (o, state) in blocks {
        let Some((block, info)) = create_info(*state) else {
            continue;
        };
        // What its block entity held rides with it (`blockEntityData`), out of the world's
        // tables before the block leaves.
        if block == Block::ContraptionControls {
            let c = with_world(|w| w.dims[dim].controls.remove(&at(*o)))
                .flatten()
                .unwrap_or_default();
            carried.controls.push((*o, c));
        }
        if let (Some(kind), Some(facing)) = (Kind::of(block), info.facing) {
            let mut a = Actor::new(*o, kind, facing, -next_breaker());
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
        mark_pos_unsaved(dim, origin);
    }
    let mut stored = Vec::new();
    for (o, _) in blocks {
        if let Some(s) = take_storage(lv, at(*o)) {
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
            mark_unsaved(dim, [cx, cz]);
        }
    }
    carried
}

/// What a contraption carried goes back with its blocks (`addBlocksToWorld`, `stop`): its
/// storage blocks' items into the storage blocks now at `origin` + their offset (dropped where
/// one couldn't be put back), its glue there, and its breakers' cracks gone.
pub(super) fn release(
    lv: &mut ServerLevel<'_>,
    key: [i32; 3],
    origin: [i32; 3],
    carried: &mut Carried,
) {
    let dim = lv.dim;
    let mut effects = Vec::new();
    for a in carried.actors.iter_mut() {
        a.cancel(&mut effects);
    }
    apply_cracks(lv, &carried.actors, &effects);
    let at = |o: [i32; 3]| add(origin, o);
    // Block entity data back with the blocks put back: rollers' settings, and contraption
    // controls off as the contraption has their kind (`ContraptionControlsMovement.stopMoving`).
    let mut settled = false;
    for a in carried.actors.iter().filter(|a| a.kind == Kind::Roller) {
        let p = at(a.local);
        if lv.create_block(p) == Some(Block::MechanicalRoller) {
            let v = (a.filter.clone(), a.mode);
            with_world(|w| w.dims[dim].rollers.insert(p, v));
            settled = true;
        }
    }
    for (o, mut c) in std::mem::take(&mut carried.controls) {
        let p = at(o);
        if lv.create_block(p) == Some(Block::ContraptionControls) {
            c.disabled = actors::stays_disabled(&carried.disabled, &c.filter);
            with_world(|w| w.dims[dim].controls.insert(p, c));
            settled = true;
        }
    }
    if settled {
        mark_pos_unsaved(dim, origin);
    }
    let stored = STORAGE
        .with_borrow_mut(|m| m.remove(&(dim, key)))
        .unwrap_or_default();
    for (o, s) in stored {
        let p = at(o);
        // A fresh storage block where it went back: it takes the carried items.
        if is_storage(lv, p) {
            put_storage(lv, p, s);
            continue;
        }
        for stack in s.into_iter().flatten() {
            lv.drop_stack(center(p), [0.0; 3], stack, 10);
        }
    }
    if carried.glue.is_empty() {
        return;
    }
    let placed: Vec<Glue> = carried.glue.drain(..).map(|g| g.moved(origin)).collect();
    for g in &placed {
        let (cx, cz) = g.column();
        mark_unsaved(dim, [cx, cz]);
    }
    with_world(|w| w.dims[dim].glue.extend(placed));
}

// ---------------------------------------------------------------------------------------------
// What actors read of the world

/// What an actor reads of the world (`canBreak`, `FallingBlock`), and the contraption's storage.
struct Reader<'a, 'b> {
    lv: &'a ServerLevel<'b>,
    key: [i32; 3],
}

impl ServerLevel<'_> {
    fn in_tag(&self, pos: [i32; 3], tag: &str) -> bool {
        self.block_state(pos)
            .and_then(|s| self.host.block_in_tag(s.block(), tag))
            .unwrap_or(false)
    }

    /// The state's collision boxes (`getCollisionShape`).
    fn collision(&self, pos: [i32; 3]) -> Vec<[f64; 6]> {
        self.block_state(pos)
            .and_then(|s| {
                self.level
                    .block_shape(&*self.host, s, pos, BlockShapeKind::Collision)
            })
            .unwrap_or_default()
    }
}

/// The block `item` places, a slab doubled (`getStateToPaveWith`).
fn pave_state(lv: &ServerLevel<'_>, item: &str) -> Option<u32> {
    let block = lv.host.item_block(item_key_of(lv, item)?)?;
    let state = lv.host.block_state_default(block)?;
    let id = super::num(state);
    let (name, props) = super::name_props(&*lv.host, id)?;
    if name.ends_with("_slab") && prop(&props, "type").is_some() {
        let p: Vec<(String, String)> = props
            .into_iter()
            .map(|(k, v)| {
                if k == "type" {
                    (k, "double".into())
                } else {
                    (k, v)
                }
            })
            .collect();
        let p: Vec<(&str, &str)> = p.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        return super::state_with(&*lv.host, &name, &p);
    }
    Some(id)
}

fn pave_name(lv: &ServerLevel<'_>, item: &str) -> Option<String> {
    super::name_props(&*lv.host, pave_state(lv, item)?).map(|(n, _)| n)
}

/// `getStateToPaveWithAsSlab`: the slab of `item`'s block, by Create's names (`_slab`, its
/// plural's or its planks' wood), None when there is none.
fn pave_slab(lv: &ServerLevel<'_>, item: &str) -> Option<String> {
    let name = pave_name(lv, item)?;
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
        .find(|t| lv.host.block_state_id(t).is_some())
}

/// `RollerBlockEntity.isValidMaterial`: a block item whose block has no block entity, is no
/// stair, and has a full block's collision.
fn valid_pave_material(lv: &ServerLevel<'_>, item: &str) -> bool {
    let Some(state) = pave_state(lv, item).and_then(super::unnum) else {
        return false;
    };
    let name = lv
        .host
        .block_state_string(state)
        .map(|t| states::split(&t).0.to_owned())
        .unwrap_or_default();
    let block_entity = lv
        .host
        .block_states()
        .has_block_entity(state)
        .unwrap_or(true);
    let shape = lv
        .level
        .block_shape(&*lv.host, state, [0, 0, 0], BlockShapeKind::Collision)
        .unwrap_or_default();
    let full = shape == [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
    !block_entity && full && !name.ends_with("_stairs")
}

const FALLING: [&str; 11] = [
    "minecraft:sand",
    "minecraft:red_sand",
    "minecraft:suspicious_sand",
    "minecraft:gravel",
    "minecraft:suspicious_gravel",
    "minecraft:anvil",
    "minecraft:chipped_anvil",
    "minecraft:damaged_anvil",
    "minecraft:dragon_egg",
    "minecraft:scaffolding",
    "minecraft:pointed_dripstone",
];

impl actors::Probe for Reader<'_, '_> {
    fn material(&self, filter: Option<&str>) -> u32 {
        STORAGE.with_borrow(|m| {
            let Some(list) = m.get(&(self.lv.dim, self.key)) else {
                return 0;
            };
            list.iter()
                .flat_map(|(_, s)| s.iter().flatten())
                .filter(|stack| filter.is_none_or(|f| item_id(self.lv, stack) == f))
                .map(ItemStack::count)
                .sum()
        })
    }

    fn fill(&self, pos: [i32; 3], filter: Option<&str>) -> actors::Fill {
        let lv = self.lv;
        if !lv.is_loaded(pos) {
            return actors::Fill::Blocked;
        }
        let Some(state) = lv.block_state(pos) else {
            return actors::Fill::Blocked;
        };
        let name = lv.block_name(pos).unwrap_or_default();
        let same = match filter {
            None => AIRS.contains(&name.as_str()),
            Some(item) => pave_name(lv, item).is_some_and(|p| p == name),
        };
        if same {
            return actors::Fill::Same;
        }
        let replaceable = lv.host.block_states().replaceable(state).unwrap_or(false);
        let empty = lv.collision(pos).is_empty();
        if !lv.in_tag(pos, "minecraft:leaves")
            && !replaceable
            && (!empty || lv.in_tag(pos, "minecraft:portals"))
        {
            return actors::Fill::Blocked;
        }
        actors::Fill::Open
    }

    fn is_pave(&self, pos: [i32; 3], item: &str, slab: bool) -> bool {
        let lv = self.lv;
        let Some(name) = lv.block_name(pos) else {
            return false;
        };
        if slab {
            return pave_slab(lv, item).is_some_and(|s| s == name);
        }
        pave_name(lv, item).is_some_and(|p| p == name)
    }

    fn pave_falls(&self, item: &str) -> bool {
        // The falling blocks the breakers know (`FallingBlock`).
        pave_name(self.lv, item).is_some_and(|n| {
            matches!(
                n.as_str(),
                "minecraft:sand"
                    | "minecraft:red_sand"
                    | "minecraft:gravel"
                    | "minecraft:anvil"
                    | "minecraft:dragon_egg"
            ) || n.ends_with("_concrete_powder")
        })
    }

    fn breakable(&self, kind: Kind, pos: [i32; 3]) -> Option<f32> {
        let lv = self.lv;
        let state = lv.block_state(pos)?;
        let name = lv.block_name(pos).unwrap_or_default();
        let air = AIRS.contains(&name.as_str());
        let replaceable = lv.host.block_states().replaceable(state).unwrap_or(false);
        let liquid = lv
            .host
            .state_fluid_state(state)
            .is_some_and(|f| f.amount > 0)
            && replaceable;
        let no_collision = lv.collision(pos).is_empty();
        let track = name == "create:track";
        let hardness = lv.host.block_states().destroy_speed(state).unwrap_or(0.0);
        if kind == Kind::Plough {
            let p = actors::Ploughed {
                air,
                farmland_below: lv
                    .block_name([pos[0], pos[1] - 1, pos[2]])
                    .is_some_and(|n| n == "minecraft:farmland"),
                liquid: liquid || name == "minecraft:bubble_column",
                track,
                blacklisted: lv.in_tag(pos, "create:plough_blacklist"),
                whitelisted: lv.in_tag(pos, "create:plough_whitelist"),
                no_collision,
            };
            // `getDestroySpeed` still paces it (grass breaks at once).
            return actors::plough_breaks(&p).then_some(hardness.max(0.0));
        }
        // `BlockBreakingKineticBlockEntity.isBreakable`.
        if air || liquid || hardness < 0.0 || lv.in_tag(pos, "create:non_breakable") {
            return None;
        }
        let ok = match kind {
            Kind::Drill => !no_collision && !track,
            Kind::Saw => sawable(lv, pos, &name),
            // RollerMovementBehaviour.canBreak: nothing beside a portal, not track, not without
            // collision.
            Kind::Roller => {
                !no_collision
                    && !track
                    && !Dir::ALL
                        .into_iter()
                        .any(|d| lv.in_tag(add(pos, d.offset()), "minecraft:portals"))
            }
            _ => false,
        };
        ok.then_some(hardness)
    }

    fn falling(&self, pos: [i32; 3]) -> bool {
        self.lv
            .block_name(pos)
            .is_some_and(|n| FALLING.contains(&n.as_str()) || n.ends_with("_concrete_powder"))
    }
}

/// `SawBlockEntity.isSawable`: logs, leaves, roots, bamboo, cactus, sugar cane, kelp, chorus
/// plants and Create's solid crops; not saplings.
fn sawable(lv: &ServerLevel<'_>, pos: [i32; 3], name: &str) -> bool {
    if lv.in_tag(pos, "minecraft:saplings") {
        return false;
    }
    lv.in_tag(pos, "minecraft:logs")
        || lv.in_tag(pos, "minecraft:leaves")
        || lv.in_tag(pos, "create:solid_crops")
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

// ---------------------------------------------------------------------------------------------
// What actors do

/// `destroyBlockProgress` for a breaker's cracks.
fn apply_cracks(lv: &mut ServerLevel<'_>, list: &[Actor], effects: &[Effect]) {
    let id = list.first().map_or(0, |a| a.breaker_id);
    for e in effects {
        if let Effect::Crack(p, stage) = e {
            lv.level.set_break_progress(&mut *lv.host, id, *p, *stage);
        }
    }
}

/// `BlockHelper.destroyBlockAs` with no player: the block broken with its loot drawn with
/// `tool` (none when blocks don't drop), mined ice becoming water over a block that holds it.
/// The drops, for the actor to collect or drop.
fn break_block(lv: &mut ServerLevel<'_>, at: [i32; 3], tool: Option<&ItemStack>) -> Vec<ItemStack> {
    let ice = lv.block_name(at).is_some_and(|n| n == "minecraft:ice");
    let drops = lv
        .level
        .destroy_block(&mut *lv.host, at, true, tool, true)
        .ok()
        .flatten()
        .unwrap_or_default();
    // `IceBlock.playerDestroy`: over a block that blocks motion or a liquid, water.
    if ice {
        let under = [at[0], at[1] - 1, at[2]];
        let liquid = lv
            .block_state(under)
            .and_then(|s| lv.host.state_fluid_state(s))
            .is_some_and(|f| f.amount > 0);
        if (!lv.collision(under).is_empty() || liquid)
            && let Some(water) = lv.host.block_state_id("minecraft:water")
        {
            lv.set_state(at, water, update_all());
        }
    }
    drops
}

/// `collectOrDropItem`: into the contraption's storage (`moveItemsToStorage`), what is left
/// dropped at the actor with `(motion + (0, .5, 0)) × random × .3`.
fn collect(lv: &mut ServerLevel<'_>, key: [i32; 3], actor: &Actor, stack: ItemStack) {
    let limit = slot_limit(lv, &stack);
    let dim = lv.dim;
    let left = STORAGE.with_borrow_mut(|m| {
        let Some(list) = m.get_mut(&(dim, key)) else {
            return Some(stack);
        };
        let mut stack = stack;
        for (_, s) in list.iter_mut() {
            match actors::insert(
                s,
                stack.clone(),
                ItemStack::count,
                |s, n| {
                    let _ = s.set_count(n.max(1));
                },
                same_stack,
                |_| limit,
            ) {
                Some(rest) => stack = rest,
                None => return None,
            }
        }
        Some(stack)
    });
    let Some(stack) = left else {
        return;
    };
    let Some(at) = actor.position else {
        return;
    };
    let scale = f64::from(lv.random_float()) * 0.3;
    let m = actor.motion;
    lv.drop_stack(
        at,
        [m[0] * scale, (m[1] + 0.5) * scale, m[2] * scale],
        stack,
        10,
    );
}

/// The actors of a bearing's structure (its blocks' offsets from the bearing and states):
/// breakers, harvesters, ploughs and rollers, each a breaker of its own. Its storage, glue and
/// controls don't ride a bearing.
pub(super) fn bearing_actors(blocks: &[([i32; 3], u32)]) -> Carried {
    let mut carried = Carried::default();
    for (o, state) in blocks {
        let Some((block, info)) = create_info(*state) else {
            continue;
        };
        let (Some(kind), Some(facing)) = (Kind::of(block), info.facing) else {
            continue;
        };
        if matches!(kind, Kind::Interface { .. }) {
            continue;
        }
        carried
            .actors
            .push(Actor::new(*o, kind, facing, -next_breaker()));
    }
    carried
}

/// A turning contraption's actors stop (`stopMoving`): what they were breaking is left as it
/// was, its cracks gone.
pub(super) fn cancel_actors(lv: &mut ServerLevel<'_>, carried: &mut Carried) {
    let mut effects = Vec::new();
    for a in carried.actors.iter_mut() {
        a.cancel(&mut effects);
    }
    apply_cracks(lv, &carried.actors, &effects);
}

/// `tickActors` for a bearing's structure turned `deg` about `axis` through the bearing at
/// `bearing`. Returns whether one stalls it.
pub(super) fn tick_turning_actors(
    lv: &mut ServerLevel<'_>,
    key: [i32; 3],
    carried: &mut Carried,
    bearing: [i32; 3],
    axis: create_engine::kinetics::Axis,
    deg: f64,
) -> bool {
    tick_posed_actors(
        lv,
        key,
        carried,
        center(bearing),
        &actors::one_turn(axis, deg),
    )
}

/// `tickActors` for a turning structure whose anchor block's centre is at `pivot`, turned by
/// `turns` about it: each actor works where it is turned to, facing the way it turned
/// (`toGlobalVector`, `rotation`). Returns whether one stalls it.
pub fn tick_posed_actors(
    lv: &mut ServerLevel<'_>,
    key: [i32; 3],
    carried: &mut Carried,
    pivot: [f64; 3],
    turns: &actors::Turns,
) -> bool {
    let mut stalled = false;
    for i in 0..carried.actors.len() {
        let mut effects = Vec::new();
        let a = &mut carried.actors[i];
        a.turns = *turns;
        let position = actors::posed_position(a, pivot, turns);
        let facing = actors::posed_facing(a.facing, turns);
        stalled |= a.tick(
            position,
            facing,
            actors::CONTROLLED_LOWER,
            &Reader { lv, key },
            &mut effects,
        );
        let a = carried.actors[i].clone();
        for e in effects {
            apply(lv, key, &a, e);
        }
    }
    stalled
}

/// `ContraptionCollider.isCollidingWithWorld` for a breaker: the contraption's block at `local`
/// is a breaker (drill, saw, plough, roller) that can break the block at `cell`, so it doesn't
/// stop the contraption there (`canBreak`).
pub(super) fn breaker_clears(
    lv: &ServerLevel<'_>,
    key: [i32; 3],
    carried: &Carried,
    local: [i32; 3],
    cell: [i32; 3],
) -> bool {
    use actors::Probe;
    let reader = Reader { lv, key };
    carried
        .actors
        .iter()
        .find(|a| a.local == local && a.kind.breaks())
        .is_some_and(|a| reader.breakable(a.kind, cell).is_some())
}

/// `AbstractContraptionEntity.tickActors` for a translating contraption whose offset 0 is at
/// `translation` in the world now: each active actor ticks, and what it does happens. Returns
/// whether one stalls the contraption. Only moving contraptions with actors call it.
pub fn tick_actors(
    lv: &mut ServerLevel<'_>,
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
            stalled |= tick_interface(lv, key, a, position, fluid);
            continue;
        }
        stalled |= a.tick(
            position,
            facing,
            actors::CONTROLLED_LOWER,
            &Reader { lv, key },
            &mut effects,
        );
        let a = carried.actors[i].clone();
        for e in effects {
            apply(lv, key, &a, e);
        }
    }
    stalled
}

fn apply(lv: &mut ServerLevel<'_>, key: [i32; 3], a: &Actor, e: Effect) {
    match e {
        Effect::Crack(p, stage) => {
            lv.level
                .set_break_progress(&mut *lv.host, a.breaker_id, p, stage);
        }
        Effect::Hit(p) => {
            // `playSound(.., getHitSound(), NEUTRAL, .25, 1)`.
            if let Some(sound) = lv
                .block_state(p)
                .and_then(|s| lv.host.block_states().sound(s))
            {
                lv.level.play_sound(
                    &mut *lv.host,
                    &sound.hit,
                    center(p),
                    SoundSource::Neutral,
                    0.25,
                    1.0,
                );
            }
        }
        Effect::Destroy(p) => {
            // The plough's snow drops as an iron shovel digs it (`onBlockBroken`).
            let snow =
                a.kind == Kind::Plough && lv.block_name(p).is_some_and(|n| n == "minecraft:snow");
            // `RollerMovementBehaviour.destroyBlock`: nothing from a block needing an iron,
            // stone or diamond tool, else each stack kept one time in two.
            let no_harvest = a.kind == Kind::Roller
                && (lv.in_tag(p, "minecraft:needs_iron_tool")
                    || lv.in_tag(p, "minecraft:needs_stone_tool")
                    || lv.in_tag(p, "minecraft:needs_diamond_tool"));
            let shovel = snow
                .then(|| stack_of(lv, "minecraft:iron_shovel"))
                .flatten();
            let drops = break_block(lv, p, shovel.as_ref());
            for stack in drops {
                if a.kind == Kind::Roller && (no_harvest || lv.random_float() < 0.5) {
                    continue;
                }
                collect(lv, key, a, stack);
            }
        }
        Effect::Pave(p) => pave(lv, key, a, p),
        Effect::Harvest(p) => harvest(lv, key, a, p),
        Effect::Till(p) => till(lv, p),
    }
}

/// The plough's hoe on the block a ray down from the block's centre hits (it, if it has a
/// shape, else the one below: `PloughMovementBehaviour.visitNewPosition`), used by a fake
/// player of the mod's own (`PloughFakePlayer`), looking down at its top.
fn till(lv: &mut ServerLevel<'_>, p: [i32; 3]) {
    let above = [p[0], p[1] + 1, p[2]];
    let outline = lv
        .block_state(above)
        .and_then(|s| {
            lv.level
                .block_shape(&*lv.host, s, above, BlockShapeKind::Outline)
        })
        .unwrap_or_default();
    let target = if outline.is_empty() { p } else { above };
    let Some(hoe) = stack_of(lv, "minecraft:diamond_hoe") else {
        return;
    };
    let Some(player) = lv.host.fake_player(lv.level, PLOUGH_PLAYER, "Plough") else {
        return;
    };
    let top = [
        f64::from(target[0]) + 0.5,
        f64::from(target[1]) + 1.0,
        f64::from(target[2]) + 0.5,
    ];
    let eye = [top[0], top[1] + 0.5, top[2]];
    let hit = BlockHit::new(top, [0, 1, 0], false);
    let action = PlayerAction::UseOn {
        hand: Hand::Main,
        pos: target,
        hit,
    };
    let setup = PlayerActionSetup::new(Some(hoe), eye, 0.0, 90.0, false);
    let _ = player.perform_action_scoped(&mut *lv.host, action, setup);
}

/// The UUID of the plough's fake player.
const PLOUGH_PLAYER: u128 = 0x9e2f_aded_cafe_4ec2_c314_dad1_29ae_971e;

/// `HarvesterMovementBehaviour.visitNewPosition` on the server.
fn harvest(lv: &mut ServerLevel<'_>, key: [i32; 3], a: &Actor, p: [i32; 3]) {
    let Some((name, props)) = lv.name_props(p) else {
        return;
    };
    if AIRS.contains(&name.as_str()) {
        return;
    }
    let bush = lv.in_tag(p, "minecraft:flowers") || lv.in_tag(p, "minecraft:saplings");
    let age = prop(&props, "age")
        .and_then(|v| v.parse::<u8>().ok())
        .zip(actors::age_max(&name));
    let reached = actors::Reached {
        plant: actors::plant_of(&name, bush),
        age,
        no_collision: lv.collision(p).is_empty(),
        leaves: lv.in_tag(p, "minecraft:leaves"),
    };
    let Some((replant, shears, _chance)) =
        actors::harvest(&reached, lv.in_tag(p, "create:non_harvestable"))
    else {
        return;
    };
    let cut = actors::cut(&reached);
    let tool = shears.then(|| stack_of(lv, "minecraft:shears")).flatten();
    let drops = break_block(lv, p, tool.as_ref());
    let mut seed = actors::planted_by(&name).filter(|_| replant);
    for stack in drops {
        let mut stack = Some(stack);
        if let Some(s) = &stack
            && seed.is_some_and(|i| item_id(lv, s) == i)
        {
            stack = stack.and_then(|s| {
                let n = s.count();
                s.shrink(1).filter(|_| n > 1)
            });
            seed = None;
        }
        if let Some(stack) = stack {
            collect(lv, key, a, stack);
        }
    }
    if let actors::Cut::Age(n) = cut {
        let p2: Vec<(String, String)> = props
            .into_iter()
            .map(|(k, v)| {
                if k == "age" {
                    (k, n.to_string())
                } else {
                    (k, v)
                }
            })
            .collect();
        let p2: Vec<(&str, &str)> = p2.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        if let Some(id) = super::state_with(&*lv.host, &name, &p2) {
            lv.set_num(p, id, update_all());
        }
    }
}

/// A roller's `tryFill` placing: one item its filter passes out of the contraption's storage
/// (`ItemHelper.extract(..., 1, false)`), its block set (`setBlockAndUpdate`; air with no
/// filter).
fn pave(lv: &mut ServerLevel<'_>, key: [i32; 3], a: &Actor, p: [i32; 3]) {
    let filter = a.filter.as_deref();
    let dim = lv.dim;
    let host = &*lv.host;
    let taken = STORAGE.with_borrow_mut(|m| {
        let list = m.get_mut(&(dim, key))?;
        for (_, s) in list.iter_mut() {
            for slot in s.iter_mut() {
                let Some(stack) = slot.as_ref() else {
                    continue;
                };
                if filter.is_some_and(|f| stack.item().id(host) != f) {
                    continue;
                }
                *slot = slot.take().and_then(|s| s.shrink(1));
                return Some(());
            }
        }
        None
    });
    if taken.is_none() {
        return;
    }
    let state = match filter {
        Some(item) => pave_state(lv, item),
        None => super::state_with(&*lv.host, "minecraft:air", &[]),
    };
    if let Some(s) = state {
        lv.set_num(p, s, update_all());
    }
}

// ---------------------------------------------------------------------------------------------
// Portable storage interfaces

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
    lv: &ServerLevel<'_>,
    key: [i32; 3],
    a: &mut Actor,
    position: [f64; 3],
    fluid: bool,
) -> bool {
    let dim = lv.dim;
    let grid = position.map(|v| v.floor() as i32);
    let mut near = vec![grid, add(grid, a.facing.offset())];
    near.extend(a.interface.working);
    let mut world = Interfaces {
        fluid,
        found: Vec::new(),
    };
    for p in near {
        let state = lv.block(p);
        let found = with_world(|w| {
            let (b, info) = w.table.get(state)?;
            let f = match b {
                Block::PortableStorageInterface => false,
                Block::PortableFluidInterface => true,
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
fn psi_connected(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<[i32; 3]> {
    let state = lv.block(pos);
    create_engine::states::index(state)?;
    let dim = lv.dim;
    with_world(|w| {
        let (b, _) = w.table.get(state)?;
        if b != Block::PortableStorageInterface {
            return None;
        }
        w.dims[dim].psis.get(&pos)?.can_transfer()
    })
    .flatten()
}

/// Whether an item interface stands at `pos` (connected or not).
pub fn is_item_interface(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    lv.create_block(pos) == Some(Block::PortableStorageInterface)
}

/// How much of `stack` `psi_insert` would put in (simulated); None when no connected item
/// interface is at `pos`.
pub fn psi_room(lv: &ServerLevel<'_>, pos: [i32; 3], stack: &ItemStack) -> Option<u32> {
    let key = psi_connected(lv, pos)?;
    let limit = slot_limit(lv, stack);
    let dim = lv.dim;
    Some(STORAGE.with_borrow(|m| {
        let Some(list) = m.get(&(dim, key)) else {
            return 0;
        };
        let mut room = 0u32;
        for (_, s) in list {
            for slot in s {
                room += match slot {
                    Some(s) if same_stack(s, stack) => limit.saturating_sub(s.count()),
                    Some(_) => 0,
                    None => limit,
                };
            }
        }
        room.min(stack.count())
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
pub fn psi_insert(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    stack: ItemStack,
) -> Result<Option<ItemStack>, ItemStack> {
    let Some(key) = psi_connected(lv, pos) else {
        return Err(stack);
    };
    let limit = slot_limit(lv, &stack);
    let before = stack.count();
    let dim = lv.dim;
    let left = STORAGE.with_borrow_mut(|m| {
        let Some(list) = m.get_mut(&(dim, key)) else {
            return Some(stack);
        };
        let mut left = stack.count();
        // Same items first, then empty slots, storage block by storage block.
        for empty in [false, true] {
            for (_, s) in list.iter_mut() {
                for slot in s.iter_mut() {
                    if left == 0 {
                        return None;
                    }
                    match slot {
                        Some(s) if !empty && same_stack(s, &stack) => {
                            let put = left.min(limit.saturating_sub(s.count()));
                            let _ = s.set_count(s.count() + put);
                            left -= put;
                        }
                        None if empty => {
                            let put = left.min(limit);
                            let mut s = stack.clone();
                            let _ = s.set_count(put);
                            *slot = Some(s);
                            left -= put;
                        }
                        _ => {}
                    }
                }
            }
        }
        (left > 0).then(|| {
            let mut s = stack;
            let _ = s.set_count(left);
            s
        })
    });
    if left.as_ref().map_or(0, ItemStack::count) != before {
        psi_transferred(dim, pos);
    }
    Ok(left)
}

/// `InterfaceItemHandler.extractItem` through `ItemHelper.extract` (up to `amount` of the first
/// item `test` passes, gathered from the slots in order) from the storage of the contraption the
/// interface at `pos` is connected to. Err(()) when no connected item interface is there.
pub fn psi_extract(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    test: impl Fn(&ItemStack) -> bool,
    amount: u32,
) -> Result<Option<ItemStack>, ()> {
    let key = psi_connected(lv, pos).ok_or(())?;
    let dim = lv.dim;
    let taken = STORAGE.with_borrow_mut(|m| {
        let list = m.get_mut(&(dim, key))?;
        let mut out: Option<ItemStack> = None;
        for (_, s) in list.iter_mut() {
            for slot in s.iter_mut() {
                let want = amount - out.as_ref().map_or(0, ItemStack::count);
                if want == 0 {
                    return out;
                }
                let Some(stack) = slot.as_ref() else {
                    continue;
                };
                let fits = match &out {
                    Some(o) => same_stack(o, stack),
                    None => test(stack),
                };
                if !fits {
                    continue;
                }
                let n = want.min(stack.count());
                match &mut out {
                    Some(o) => {
                        let _ = o.set_count(o.count() + n);
                    }
                    None => {
                        let mut o = stack.clone();
                        let _ = o.set_count(n);
                        out = Some(o);
                    }
                }
                *slot = slot.take().and_then(|s| s.shrink(n));
            }
        }
        out
    });
    if taken.is_some() {
        psi_transferred(dim, pos);
    }
    Ok(taken)
}

// ---------------------------------------------------------------------------------------------
// Rollers and contraption controls in the world

/// The roller at `pos`: its state and facing.
fn roller_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(u32, Dir)> {
    let state = lv.block(pos);
    match create_info(state) {
        Some((Block::MechanicalRoller, i)) => Some((state, i.facing?)),
        _ => None,
    }
}

/// `shareValuesToAdjacent`: the rollers in line across its facing, of the same state, up to 99
/// each way, take its filter and mode.
fn share_roller(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some((state, facing)) = roller_at(lv, pos) else {
        return;
    };
    let Some(v) = with_world(|w| w.dims[dim].rollers.get(&pos).cloned()).flatten() else {
        return;
    };
    let o = create_engine::display::clockwise(facing).offset();
    for side in [1, -1] {
        for i in 1..100 {
            let p = [pos[0] + o[0] * side * i, pos[1], pos[2] + o[2] * side * i];
            if lv.block(p) != state {
                break;
            }
            with_world(|w| w.dims[dim].rollers.insert(p, v.clone()));
        }
    }
    mark_pos_unsaved(dim, pos);
}

/// After the contraptions moved: rollers placed take a neighbour's settings, contraption
/// controls follow redstone, stationary interfaces read their power and those connected or
/// letting go tick. Nothing runs while none of these has anything to do.
pub(super) fn tick_mounted(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let placed = ROLLERS_PLACED.with_borrow_mut(|r| {
        if r.is_empty() {
            return Vec::new();
        }
        let mine: Vec<[i32; 3]> = r.iter().filter(|e| e.0 == dim).map(|e| e.1).collect();
        r.retain(|e| e.0 != dim);
        mine
    });
    for pos in placed {
        // `searchForSharedValues`: the first roller beside it of the same state.
        let Some((state, facing)) = roller_at(lv, pos) else {
            continue;
        };
        let o = create_engine::display::clockwise(facing).offset();
        for side in [1, -1] {
            let p = [pos[0] + o[0] * side, pos[1], pos[2] + o[2] * side];
            if lv.block(p) != state {
                continue;
            }
            let Some(v) = with_world(|w| w.dims[dim].rollers.get(&p).cloned()).flatten() else {
                continue;
            };
            with_world(|w| w.dims[dim].rollers.insert(pos, v));
            share_roller(lv, pos);
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
    for pos in controls {
        let powered = lv.has_neighbor_signal(pos);
        let changed = with_world(|w| {
            w.dims[dim]
                .controls
                .get_mut(&pos)
                .is_some_and(|c| c.powered(powered))
        });
        if changed == Some(true) {
            mark_pos_unsaved(dim, pos);
        }
    }
    for pos in psis {
        let powered = lv.has_neighbor_signal(pos);
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

/// A roller's item placed it (`setPlacedBy` → `searchForSharedValues`, at the level's tick).
struct RollerItem;

#[mod_api::horizon_part]
impl ItemPart for RollerItem {
    fn on_block_placed_by(&self, host: &mut dyn ServerHost, event: &mut OnBlockPlacedByEvent<'_>) {
        let Some(level) = event.placer.and_then(|p| p.level(&*host)) else {
            return;
        };
        let Some(dimension) = level.dimension().id(&*host) else {
            return;
        };
        if let Some(dim) = super::super::world::level_index(&dimension) {
            ROLLERS_PLACED.with_borrow_mut(|r| r.push((dim, event.pos)));
        }
    }
}

struct MountedPart;

#[mod_api::horizon_part]
impl BlockPart for MountedPart {
    /// A click on contraption controls or a roller (`useWithoutItem`, `FilteringBehaviour`).
    /// Controls: on the filter slot the held item becomes the kind of actor it switches
    /// (`create:contraption_controlled` only; an empty hand clears it), elsewhere it turns its
    /// kind off or on, with Create's status and click. A roller: on its filter slot the held
    /// block item becomes its pave material (`isValidMaterial`), an empty hand clears it, shared
    /// with the rollers in line; its mode is set with `/create_debug roller` (the value box).
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let c = super::clicker(&*host, event.click);
        let pos = event.click.pos;
        let point = event.click.hit.point;
        in_click(host, event.click, |lv| use_mounted(lv, pos, point, &c))?
    }
}

fn in_click<R>(
    host: &mut dyn ServerHost,
    click: &mod_api::BlockClick<'_>,
    f: impl FnOnce(&mut ServerLevel<'_>) -> R,
) -> Option<R> {
    super::in_click_level(host, click, f)
}

fn use_mounted(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    point: [f64; 3],
    c: &super::Clicker,
) -> Option<InteractionResult> {
    let dim = lv.dim;
    let (block, info) = create_info(lv.block(pos))?;
    let facing = info.facing?;
    if c.held.as_deref() == Some("create:wrench") {
        return None;
    }
    let roller = block == Block::MechanicalRoller;
    let slot = if roller {
        actors::roller_slot(facing, true)
    } else {
        actors::controls_slot(facing)
    };
    let local = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]));
    let d2: f64 = (0..3).map(|i| (local[i] - slot.0[i]).powi(2)).sum();
    let on_slot = d2.sqrt() < slot.1;
    if !c.sneaking && on_slot {
        // `FilteringBehaviour.onShortInteract`: the predicate, then a ghost copy of the item.
        let item = c.held.clone();
        let fits = match item.as_deref() {
            None => true,
            Some(i) if roller => valid_pave_material(lv, i),
            Some(i) => actors::CONTRAPTION_CONTROLLED.contains(&i),
        };
        if !fits {
            return Some(InteractionResult::Success);
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
            share_roller(lv, pos);
        }
        mark_pos_unsaved(dim, pos);
        lv.level.play_sound(
            &mut *lv.host,
            "minecraft:entity.item_frame.add_item",
            center(pos),
            SoundSource::Blocks,
            0.25,
            0.1,
        );
        return Some(InteractionResult::Success);
    }
    // Sneaking with something held skips the block's use (`ServerPlayerGameMode.useItemOn`).
    if roller || (c.sneaking && c.held.is_some()) {
        return None;
    }
    // ContraptionControlsBlock.useWithoutItem.
    let (on, filter) = with_world(|w| {
        let c = w.dims[dim].controls.entry(pos).or_default();
        let on = c.click();
        (on, c.filter.clone())
    })?;
    controls_status(lv, c.player, pos, filter.as_deref(), on);
    mark_pos_unsaved(dim, pos);
    Some(InteractionResult::Success)
}

/// `ContraptionControlsBlockEntity.sendStatus` and the `controller_click` sound (pitch 1.5 on,
/// 0.8 off).
fn controls_status(
    lv: &mut ServerLevel<'_>,
    player: Option<mod_api::PlayerRef>,
    at: [i32; 3],
    filter: Option<&str>,
    on: bool,
) {
    use mod_api::Text;
    let state = Text::translate(
        if on {
            "create.contraption.controls.actor_toggle.on"
        } else {
            "create.contraption.controls.actor_toggle.off"
        },
        Vec::new(),
    );
    let line = match filter {
        None => Text::translate("create.contraption.controls.all_actor_toggle", vec![state]),
        Some(item) => Text::translate(
            "create.contraption.controls.specific_actor_toggle",
            vec![Text::literal(item), state],
        ),
    };
    if let Some(p) = player {
        let _ = p.action_bar(&mut *lv.host, line);
    }
    lv.level.play_sound(
        &mut *lv.host,
        "create:controller_click",
        center(at),
        SoundSource::Blocks,
        1.0,
        if on { 1.5 } else { 0.8 },
    );
}

// ---------------------------------------------------------------------------------------------
// Commands

/// `/create_debug roller <pos> <mode>`: a roller's mode (its value box's scroll), shared with the
/// rollers in line.
fn debug_roller(lv: &mut ServerLevel<'_>, pos: [i32; 3], mode: &str) -> String {
    let Some(m) = actors::RollingMode::read(mode) else {
        return "modes: tunnel_pave, straight_fill, wide_fill".into();
    };
    if roller_at(lv, pos).is_none() {
        return format!("no roller at {} {} {}", pos[0], pos[1], pos[2]);
    }
    let dim = lv.dim;
    let filter = with_world(|w| {
        let r = w.dims[dim].rollers.entry(pos).or_default();
        r.1 = m;
        r.0.clone()
    })
    .flatten();
    share_roller(lv, pos);
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
/// contraption needs a moving entity). `block` clicks the first contraption controls block it
/// carries, with that block's filter, its status and click sound; a filter names the kind
/// directly.
fn debug_controls(
    lv: &mut ServerLevel<'_>,
    player: Option<mod_api::PlayerRef>,
    key: [i32; 3],
    filter: Option<Option<String>>,
) -> String {
    let dim = lv.dim;
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
                (c.filter.clone(), Some(add(origin, *o)))
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
    apply_cracks(lv, &list, &effects);
    if let Some(at) = at {
        controls_status(lv, player, at, filter.as_deref(), !disable);
    }
    let what = filter.as_deref().unwrap_or("all actors");
    format!("{what} {}", if disable { "off" } else { "on" })
}

pub(super) fn register_commands(event: &mut RegisterCommandsEvent<'_>) {
    for usage in [
        "create_debug glue <from:block_pos> <to:block_pos>",
        "create_debug roller <pos:block_pos> <mode:word>",
        "create_debug controls <pos:block_pos> <filter:text>",
    ] {
        event.commands.push(Command::op(usage, 2));
    }
}

/// Create's contraption debug commands (`glue`, `controls`, `roller`).
pub(super) fn run_command(host: &mut dyn ServerHost, event: &mut RunModCommandEvent<'_>) -> Flow {
    let cmd = event.command;
    let lits: Vec<&str> = cmd.literals.iter().map(String::as_str).collect();
    let sub = match lits.as_slice() {
        ["create_debug", s] if matches!(*s, "glue" | "roller" | "controls") => *s,
        _ => return Flow::Continue,
    };
    let pos = |name: &str| match cmd.argument(name) {
        Some(CommandArgumentValue::BlockPos(p)) => Some(*p),
        _ => None,
    };
    let word = |name: &str| match cmd.argument(name) {
        Some(CommandArgumentValue::String(s)) => Some(s.clone()),
        _ => None,
    };
    let player = match event.source {
        CommandSource::Player { player, .. } => Some(*player),
        _ => None,
    };
    let Some(dimension) = cmd.dimension.id(&*host) else {
        return Flow::Cancel;
    };
    let line = {
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return Flow::Cancel;
        };
        match sub {
            "glue" => match (pos("from"), pos("to")) {
                (Some(from), Some(to)) => debug_glue(&mut lv, from, to),
                _ => "usage: create_debug glue <x> <y> <z> <x> <y> <z>".into(),
            },
            "roller" => match (pos("pos"), word("mode")) {
                (Some(at), Some(mode)) => debug_roller(&mut lv, at, &mode),
                _ => "usage: create_debug roller <x> <y> <z> <tunnel_pave|straight_fill|wide_fill>"
                    .into(),
            },
            _ => match (pos("pos"), word("filter")) {
                (Some(at), Some(f)) => {
                    let filter = match f.trim() {
                        "block" => None,
                        "all" => Some(None),
                        f => Some(Some(f.to_owned())),
                    };
                    debug_controls(&mut lv, player, at, filter)
                }
                _ => "usage: create_debug controls <x> <y> <z> <item|all|block>".into(),
            },
        }
    };
    let _ = host.commands().feedback(mod_api::Text::literal(line));
    Flow::Cancel
}
