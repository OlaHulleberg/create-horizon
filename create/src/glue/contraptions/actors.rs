//! What moving contraptions carry besides their blocks, and Create's glue and stickers: super
//! glue areas the structure search reads (set by the glue item, `superglue`, or
//! `/create_debug glue`), the sticker's redstone, actors (drills, saws, harvesters, ploughs,
//! rollers, deployers, redstone contacts, interfaces) ticked only while their contraption moves, mounted storage (chests,
//! barrels, item vaults and toolboxes) that keeps its items and takes what the actors collect
//! (a toolbox takes nothing), and contraption controls
//! (`/create_debug controls`). Create's rules are in `create_engine::glue` and
//! `create_engine::actors`. Saved under `create_glue`.
//! Track observers' filters (the trains' slot, read as the items port reads filters) are kept
//! there too (`observers`).

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

/// A moving contraption's other block entities: each block's offset and what it held.
type Carrying = Vec<([i32; 3], BlockData)>;

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
    /// What the other block entities of moving contraptions held, by dimension and
    /// contraption, as `STORAGE`: each block's offset and its data.
    static HELD: RefCell<HashMap<(usize, [i32; 3]), Carrying>> = RefCell::new(HashMap::new());
}

/// What a carried block's block entity held besides mounted storage
/// (`StructureBlockInfo.nbt`: `Contraption.addBlock` keeps every block entity's data, and
/// `addBlocksToWorld` loads it into the block entity placed back): a Create block entity's item
/// slots (a depot's stack), its own data and what the engine keeps for it (`Subsystem::carry_out`:
/// a motor's speed, a burner's fuel, a link's frequency...), a deployer's hand and mode, a tank
/// controller's fluid, or another block entity's saved data (a furnace's items, a sign's text).
#[derive(Clone, Debug, Default)]
struct BlockData {
    slots: Vec<Option<ItemStack>>,
    data: Option<Compound>,
    deployer: Option<create_engine::deployer::Deployer>,
    tank: Option<create_engine::tank::Multi>,
    /// The Create block it was, its block entity's data (but what the kinetics and the
    /// structure's drawing write again), and what the subsystems keep for it.
    block: Option<Block>,
    own: Option<Compound>,
    engine: Option<Compound>,
}

impl BlockData {
    fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
            && self.data.is_none()
            && self.deployer.is_none()
            && self.tank.is_none()
            && self.own.is_none()
            && self.engine.is_none()
    }
}

/// Whether the deployer at block `local` of contraption `key` punches (`Mode.PUNCH`).
pub(super) fn deployer_punches(dim: usize, key: [i32; 3], local: [i32; 3]) -> bool {
    HELD.with_borrow(|m| {
        m.get(&(dim, key)).is_some_and(|held| {
            held.iter().any(|(o, d)| {
                *o == local
                    && d.deployer
                        .as_ref()
                        .is_some_and(|d| d.mode == create_engine::deployer::Mode::Punch)
            })
        })
    })
}

/// Keys of a Create block entity's data that are not carried: its speeds and stress as the
/// network gives them, what it generates, its bracket's look, a structure it draws and its value
/// box's looks, all written again where it goes back; and its item slots, carried as slots.
fn derived_key(k: &str) -> bool {
    use super::super::kinetics;
    [
        kinetics::SPEEDS,
        kinetics::THEORY,
        kinetics::OVER,
        kinetics::BRACKET,
        super::super::sources::GENERATED,
        super::draw::KEY,
        super::super::interaction::values::LOOKS,
        // Its item slots ride as slots.
        "Items",
    ]
    .contains(&k)
}

/// The world closed: what moving contraptions carried goes with it.
pub(super) fn forget() {
    STORAGE.with_borrow_mut(HashMap::clear);
    NEXT_BREAKER.set(0);
    ROLLERS_PLACED.with_borrow_mut(Vec::clear);
    HELD.with_borrow_mut(HashMap::clear);
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

/// The block entity slots of Create's storage blocks that ride as mounted storage
/// (`ItemVaultMountedStorage`: each vault block's 20 stacks; `ToolboxMountedStorage`: a
/// toolbox's compartments and filters), read and written straight in their block entity.
fn slot_storage(name: &str) -> Option<usize> {
    if name == "create:item_vault" {
        return Some(create_engine::vault::SLOTS);
    }
    let block = create_engine::Block::from_name(name.strip_prefix("create:")?)?;
    create_engine::dye::is_toolbox(block).then_some(create_engine::toolbox::SLOTS)
}

/// Whether a carried storage block's slots take what actors collect and interfaces bring: a
/// toolbox's don't (its compartments hold one kind each, `ToolboxInventory.isItemValid`).
fn takes_any(stored: &Stored) -> bool {
    stored.len() != create_engine::toolbox::SLOTS
}

/// Lends a contraption's storage (each block's offset and slots) to its actors under `key`
/// while they tick (a cart keeps its own: `lent_back` gives it back).
pub(crate) fn lend_storage(
    dim: usize,
    key: [i32; 3],
    storage: Vec<([i32; 3], Vec<Option<ItemStack>>)>,
) {
    STORAGE.with_borrow_mut(|m| m.insert((dim, key), storage));
}

/// The storage lent under `key`, as the actors left it.
pub(crate) fn lent_back(dim: usize, key: [i32; 3]) -> Vec<([i32; 3], Vec<Option<ItemStack>>)> {
    STORAGE
        .with_borrow_mut(|m| m.remove(&(dim, key)))
        .unwrap_or_default()
}

/// Whether `pos` holds a storage block a contraption carries.
pub(crate) fn is_storage(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    lv.block_name(pos)
        .is_some_and(|n| storage_name(&n) || slot_storage(&n).is_some())
        && lv.host.block_entity(lv.level, pos).is_some()
}

/// The slots of a vault or toolbox at `pos`, taken out of its block entity (left empty, so it
/// spills nothing when its block goes).
fn take_slots(lv: &mut ServerLevel<'_>, pos: [i32; 3], size: usize) -> Option<Stored> {
    let e = lv.host.block_entity(lv.level, pos)?;
    let stored: Stored = (0..size)
        .map(|i| e.item_slot(&*lv.host, i as u32).ok().flatten())
        .collect();
    let _ = lv.host.transaction(|host, tx| {
        for (i, s) in stored.iter().enumerate() {
            if s.is_some() {
                let _ = e.set_item_slot(host, tx, i as u32, None);
            }
        }
        Ok::<(), ()>(())
    });
    Some(stored)
}

/// The carried slots back in the vault or toolbox now at `pos`; what has no slot drops.
fn put_slots(lv: &mut ServerLevel<'_>, pos: [i32; 3], size: usize, stored: Stored) {
    let Some(e) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let mut left = Vec::new();
    let _ = lv.host.transaction(|host, tx| {
        for (i, s) in stored.into_iter().enumerate() {
            let Some(stack) = s else { continue };
            if i >= size || e.set_item_slot(host, tx, i as u32, Some(stack.clone())) != Ok(true) {
                left.push(stack);
            }
        }
        Ok::<(), ()>(())
    });
    for stack in left {
        lv.drop_stack(center(pos), [0.0; 3], stack, 10);
    }
}

/// The `ITEMS` handler of the block at `pos`.
fn items_at(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
) -> Option<mod_api::Capability<dyn mod_api::ResourceHandler<mod_api::ItemResource>>> {
    lv.level
        .capability(&mut *lv.host, &mod_api::ITEMS, pos, None)
}

/// The items of the chest, trapped chest or barrel at `pos`, taken out of it through its
/// `ITEMS` handler (it is left empty, so it spills nothing when its block goes); None for any
/// other block. (A vanilla container's slots are not a mod block entity's own item slots.)
fn take_storage(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> Option<Stored> {
    if !is_storage(lv, pos) {
        return None;
    }
    if let Some(size) = lv.block_name(pos).as_deref().and_then(slot_storage) {
        return take_slots(lv, pos, size);
    }
    let items = items_at(lv, pos)?;
    let host = &mut *lv.host;
    let size = items.size(host).min(STORAGE_SLOTS as usize);
    let view: Vec<Option<(mod_api::ItemResource, u32)>> = (0..size)
        .map(|i| {
            items
                .resource(host, i)
                .map(|r| (r, items.amount(host, i)))
                .filter(|(_, n)| *n > 0)
        })
        .collect();
    let taken = host.transaction(|host, tx| {
        let mut out: Stored = Vec::with_capacity(size);
        for (i, slot) in view.iter().enumerate() {
            out.push(slot.as_ref().and_then(|(r, n)| {
                let got = items.extract(host, tx, i, r, *n);
                r.to_stack(got)
            }));
        }
        Ok::<Stored, ()>(out)
    });
    let mut stored = taken.ok()?;
    stored.resize(STORAGE_SLOTS as usize, None);
    Some(stored)
}

/// The carried items back in the storage block now at `pos` (`addBlocksToWorld`), each in its
/// slot through the block's `ITEMS` handler; what does not go in drops there.
fn put_storage(lv: &mut ServerLevel<'_>, pos: [i32; 3], stored: Stored) {
    if let Some(size) = lv.block_name(pos).as_deref().and_then(slot_storage) {
        put_slots(lv, pos, size, stored);
        return;
    }
    let Some(items) = items_at(lv, pos) else {
        for stack in stored.into_iter().flatten() {
            lv.drop_stack(center(pos), [0.0; 3], stack, 10);
        }
        return;
    };
    let left: Vec<ItemStack> = lv
        .host
        .transaction(|host, tx| {
            let mut left = Vec::new();
            for (i, s) in stored.into_iter().enumerate() {
                let Some(stack) = s else {
                    continue;
                };
                let resource = mod_api::ItemResource::of(&stack);
                let n = stack.count();
                let mut went = items.insert(host, tx, i, &resource, n);
                if went < n {
                    went += items.insert_any(host, tx, &resource, n - went);
                }
                if went < n
                    && let Some(rest) = resource.to_stack(n - went)
                {
                    left.push(rest);
                }
            }
            Ok::<Vec<ItemStack>, ()>(left)
        })
        .unwrap_or_default();
    for stack in left {
        lv.drop_stack(center(pos), [0.0; 3], stack, 10);
    }
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
pub(super) fn sticky_sides(lv: &ServerLevel<'_>, p: [i32; 3]) -> u8 {
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
    let mut carried = carry_extras(lv, origin, blocks, skip);
    for (o, state) in blocks {
        let Some((block, info)) = create_info(*state) else {
            continue;
        };
        if let (Some(kind), Some(facing)) = (Kind::of(block), info.facing) {
            let mut a = Actor::new(*o, kind, facing, -next_breaker());
            if kind == Kind::Roller
                && let Some((filter, mode)) =
                    with_world(|w| w.dims[dim].rollers.remove(&at(*o))).flatten()
            {
                a.filter = filter;
                a.mode = mode;
                mark_pos_unsaved(dim, at(*o));
            }
            carried.actors.push(a);
        }
    }
    apply_disabled(&mut carried);
    carry_storage(lv, key, origin, blocks);
    carried
}

/// What any moving structure takes besides its blocks, actors and storage as it assembles: its
/// contraption controls' settings (`blockEntityData`, out of the world's table), the kinds
/// they had turned off (`startMoving`, `disableActorOnStart`), and the glue that held it
/// (`Contraption.superglue`: the glue entities touching it leave the world, each shrunk to the
/// blocks it holds, `removeBlocksFromWorld`). Offsets as `blocks`, from `origin`; `skip` names
/// the blocks anchoring it. Called before its blocks leave the world.
pub(crate) fn carry_extras(
    lv: &mut ServerLevel<'_>,
    origin: [i32; 3],
    blocks: &[([i32; 3], u32)],
    skip: impl Fn([i32; 3]) -> bool,
) -> Carried {
    let dim = lv.dim;
    let at = |o: [i32; 3]| add(origin, o);
    let mut carried = Carried::default();
    for (o, state) in blocks {
        if create_info(*state).map(|(b, _)| b) != Some(Block::ContraptionControls) {
            continue;
        }
        let c = with_world(|w| w.dims[dim].controls.remove(&at(*o)))
            .flatten()
            .unwrap_or_default();
        mark_pos_unsaved(dim, at(*o));
        carried.controls.push((*o, c));
    }
    let controls: Vec<actors::Controls> = carried.controls.iter().map(|c| c.1.clone()).collect();
    actors::start_moving(&mut carried, &controls);
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

/// The kinds a contraption has turned off are off on its actors (`startMoving`'s
/// `setActorsActive(stack, false)`), as its actors are found again.
pub(crate) fn apply_disabled(carried: &mut Carried) {
    let mut effects = Vec::new();
    for f in carried.disabled.clone() {
        actors::set_active(&mut carried.actors, &f, false, &mut effects);
    }
}

/// A structure's actors found again from its blocks (`bearing_actors`) with what it carried
/// besides them (`extras`: its controls, the kinds turned off and its glue).
pub(crate) fn with_extras(mut carried: Carried, extras: &Carried) -> Carried {
    carried.controls.clone_from(&extras.controls);
    carried.disabled.clone_from(&extras.disabled);
    carried.glue.clone_from(&extras.glue);
    apply_disabled(&mut carried);
    carried
}

/// What a structure carried besides its blocks goes back where its blocks go, each offset `o`
/// at `place(o)` (`addBlocksToWorld`): its contraption controls, off as the contraption has
/// their kind (`ContraptionControlsMovement.stopMoving`), and its glue, its corners placed as
/// its blocks are (turned with a turning structure).
pub(crate) fn release_extras(
    lv: &mut ServerLevel<'_>,
    carried: &mut Carried,
    place: impl Fn([i32; 3]) -> [i32; 3],
) {
    let dim = lv.dim;
    for (o, mut c) in std::mem::take(&mut carried.controls) {
        let p = place(o);
        if lv.create_block(p) == Some(Block::ContraptionControls) {
            c.disabled = actors::stays_disabled(&carried.disabled, &c.filter);
            with_world(|w| w.dims[dim].controls.insert(p, c));
            super::actor_draw::sync_controls(lv, p);
            mark_pos_unsaved(dim, p);
        }
    }
    let placed: Vec<Glue> = carried
        .glue
        .drain(..)
        .map(|g| Glue::span(place(g.min), place(g.max)))
        .collect();
    for g in &placed {
        let (cx, cz) = g.column();
        mark_unsaved(dim, [cx, cz]);
    }
    if !placed.is_empty() {
        with_world(|w| w.dims[dim].glue.extend(placed));
    }
}

/// What a moving structure carries besides its blocks and actors, as its save keeps it
/// (`Contraption.writeNBT`'s `Superglue`, `DisabledActors`, and the controls' `Filter`,
/// `Disabled` and `Powered`): `Glue` corners, `Controls` (x, y, z and the flags, with `Filters`)
/// and `Off` (the kinds turned off, "*" for all).
pub(crate) fn extras_to_nbt(c: &mut Compound, carried: &Carried) {
    let text = |s: &Option<String>| Tag::String(s.as_deref().unwrap_or("").into());
    if !carried.glue.is_empty() {
        let v = carried
            .glue
            .iter()
            .flat_map(|g| g.min.into_iter().chain(g.max))
            .collect();
        c.insert("Glue".into(), Tag::IntArray(v));
    }
    if !carried.controls.is_empty() {
        let nums = carried
            .controls
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
        c.insert("Controls".into(), Tag::IntArray(nums));
        let items = carried
            .controls
            .iter()
            .map(|(_, x)| text(&x.filter))
            .collect();
        c.insert("Filters".into(), Tag::List(items));
    }
    if !carried.disabled.is_empty() {
        let off = carried
            .disabled
            .iter()
            .map(|f| Tag::String(f.as_deref().unwrap_or("*").into()))
            .collect();
        c.insert("Off".into(), Tag::List(off));
    }
}

/// What [`extras_to_nbt`] wrote.
pub(crate) fn extras_from_nbt(c: &Compound) -> Carried {
    let mut carried = Carried::default();
    if let Some(Tag::IntArray(v)) = c.get(&"Glue".into()) {
        carried.glue = v
            .as_chunks::<6>()
            .0
            .iter()
            .map(|g| Glue::span([g[0], g[1], g[2]], [g[3], g[4], g[5]]))
            .collect();
    }
    if let (Some(Tag::IntArray(nums)), Some(Tag::List(filters))) =
        (c.get(&"Controls".into()), c.get(&"Filters".into()))
    {
        carried.controls = nums
            .as_chunks::<4>()
            .0
            .iter()
            .zip(filters)
            .map(|(n, f)| {
                let filter = match f {
                    Tag::String(s) if !s.to_string_lossy().is_empty() => Some(s.to_string_lossy()),
                    _ => None,
                };
                let c = actors::Controls {
                    filter,
                    disabled: n[3] & 1 != 0,
                    powered: n[3] & 2 != 0,
                };
                ([n[0], n[1], n[2]], c)
            })
            .collect();
    }
    if let Some(Tag::List(off)) = c.get(&"Off".into()) {
        carried.disabled = off
            .iter()
            .filter_map(|t| match t {
                Tag::String(s) => {
                    let s = s.to_string_lossy();
                    Some((s != "*").then_some(s))
                }
                _ => None,
            })
            .collect();
    }
    carried
}

/// The items of the storage block at `pos` as a cart's contraption carries them (the cart
/// keeps them), taken out of it; None for a block that is no storage.
pub(crate) fn take_storage_at(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
) -> Option<Vec<Option<ItemStack>>> {
    take_storage(lv, pos)
}

/// A cart's carried items back in the storage block now at `pos`, dropped there when no storage
/// block took its place.
pub(crate) fn put_storage_at(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    stored: Vec<Option<ItemStack>>,
) {
    if is_storage(lv, pos) {
        put_storage(lv, pos, stored);
        return;
    }
    for stack in stored.into_iter().flatten() {
        lv.drop_stack(center(pos), [0.0; 3], stack, 10);
    }
}

/// `MountedStorageManager` as a contraption assembles: the items of its storage blocks
/// (`blocks`, offsets from `origin`) leave them and ride with contraption `key`.
pub(crate) fn carry_storage(
    lv: &mut ServerLevel<'_>,
    key: [i32; 3],
    origin: [i32; 3],
    blocks: &[([i32; 3], u32)],
) {
    let dim = lv.dim;
    let mut stored = Vec::new();
    let mut held = Vec::new();
    for (o, _) in blocks {
        let p = add(origin, *o);
        if let Some(s) = take_storage(lv, p) {
            stored.push((*o, s));
        } else if let Some(d) = take_block_data(lv, p) {
            held.push((*o, d));
        }
    }
    if !stored.is_empty() {
        STORAGE.with_borrow_mut(|m| m.insert((dim, key), stored));
    }
    if !held.is_empty() {
        HELD.with_borrow_mut(|m| m.insert((dim, key), held));
    }
}

/// What the block entities of contraption `key` show its clients (`Contraption.updateTags`,
/// the update tag of each block entity as it was taken): each block's offset and its block
/// entity's data, its item slots (`Items`) and what the engine kept for it (`Carried`).
pub(crate) fn shown_entities(lv: &ServerLevel<'_>, key: [i32; 3]) -> HashMap<[i32; 3], Compound> {
    let host = &*lv.host;
    HELD.with_borrow(|m| {
        let Some(list) = m.get(&(lv.dim, key)) else {
            return HashMap::new();
        };
        list.iter()
            .filter(|(_, d)| d.block.is_some() || d.data.is_some())
            .map(|(o, d)| {
                if d.block.is_none() {
                    // A vanilla block entity: what its renderer reads (a sign's text, a
                    // banner's patterns), where it has any.
                    let data = d.data.as_ref();
                    let c: Compound = super::vanilla_draw::SHOWN_KEYS
                        .iter()
                        .filter_map(|k| Some(((*k).into(), data?.get(&(*k).into())?.clone())))
                        .collect();
                    return (*o, c);
                }
                let mut c = d.own.clone().unwrap_or_default();
                let items: Vec<Tag> = d
                    .slots
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| {
                        let mut c = s.as_ref()?.to_nbt(host)?;
                        c.insert("Slot".into(), Tag::Byte(i as i8));
                        Some(Tag::Compound(c))
                    })
                    .collect();
                if !items.is_empty() {
                    c.insert("Items".into(), Tag::List(items));
                }
                if let Some(e) = &d.engine {
                    c.insert(SHOWN_CARRIED.into(), Tag::Compound(e.clone()));
                }
                (*o, c)
            })
            .collect()
    })
}

/// [`shown_entities`] of what a contraption saved ([`storage_to_nbt`]): a cart's, kept as a tag
/// while it rides.
pub(crate) fn shown_entities_of(
    lv: &ServerLevel<'_>,
    tag: Option<&Tag>,
) -> HashMap<[i32; 3], Compound> {
    if tag.is_none() {
        return HashMap::new();
    }
    // Read under a key no contraption has, and dropped again.
    let key = [i32::MIN, i32::MIN, i32::MIN];
    storage_from_nbt(lv, key, tag);
    let shown = shown_entities(lv, key);
    STORAGE.with_borrow_mut(|m| m.remove(&(lv.dim, key)));
    HELD.with_borrow_mut(|m| m.remove(&(lv.dim, key)));
    shown
}

/// The key of what the engine kept for a block in its moving block entity's data
/// ([`shown_entities`]).
pub(crate) const SHOWN_CARRIED: &str = "Carried";

/// What the block entities of a cart's contraption hold (all but its storage blocks, which
/// the cart keeps, and the anchor at offset 0), taken as it assembles and kept with the cart
/// as a save keeps it ([`storage_to_nbt`]); none when they hold nothing.
pub(crate) fn carry_held(
    lv: &mut ServerLevel<'_>,
    key: [i32; 3],
    origin: [i32; 3],
    blocks: &[([i32; 3], u32)],
) -> Option<Tag> {
    let dim = lv.dim;
    let mut held = Vec::new();
    for (o, _) in blocks.iter().filter(|(o, _)| *o != [0, 0, 0]) {
        let p = add(origin, *o);
        if is_storage(lv, p) {
            continue;
        }
        if let Some(d) = take_block_data(lv, p) {
            held.push((*o, d));
        }
    }
    if held.is_empty() {
        return None;
    }
    HELD.with_borrow_mut(|m| m.insert((dim, key), held));
    let tag = storage_to_nbt(lv, key);
    HELD.with_borrow_mut(|m| m.remove(&(dim, key)));
    tag
}

/// What [`carry_held`] kept, waiting for its blocks about to go back at `place(offset)`; then
/// [`release_storage`] puts it in them.
pub(crate) fn expect_held(
    lv: &mut ServerLevel<'_>,
    key: [i32; 3],
    tag: Option<&Tag>,
    place: impl Fn([i32; 3]) -> [i32; 3],
) {
    storage_from_nbt(lv, key, tag);
    expect_back(lv, key, place);
}

/// Whether the block entity at `pos` is one of Create's (its state the engine's, its stacks
/// its declared slots), not another's whose saved data rides.
fn create_entity(lv: &ServerLevel<'_>, e: mod_api::BlockEntityRef) -> bool {
    e.kind(&*lv.host)
        .is_some_and(|k| k.id(&*lv.host).starts_with("create:"))
}

/// What the block entity at `pos` held, taken out of it and the engine's tables before its
/// block leaves (so nothing drops as it goes): its item slots or saved data, a deployer's
/// state, a tank's part of its tank. None when it held nothing.
fn take_block_data(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> Option<BlockData> {
    let dim = lv.dim;
    let mut out = BlockData::default();
    let block = lv.create_block(pos);
    match block {
        Some(Block::Deployer) => {
            out.deployer = with_world(|w| {
                let d = &mut w.dims[dim];
                d.deploying.retain(|p| *p != pos);
                d.deployers.remove(&pos)
            })
            .flatten();
        }
        Some(Block::FluidTank) => {
            out.tank = with_world(|w| w.dims[dim].tanks.take(pos)).flatten();
        }
        _ => {}
    }
    if let Some(b) = block {
        // `getBlockEntityNBT`: what each subsystem keeps for it, taken from its tables.
        let mut engine = Compound::new();
        for s in super::super::SUBSYSTEMS {
            s.carry_out(lv, pos, b, &mut engine);
        }
        out.block = Some(b);
        if !engine.is_empty() {
            // Its column is saved again without it.
            mark_pos_unsaved(dim, pos);
            out.engine = Some(engine);
        }
    }
    if let Some(e) = lv.host.block_entity(lv.level, pos) {
        if create_entity(lv, e) {
            if let Some(data) = e.data(&*lv.host) {
                let own: Compound = data
                    .iter()
                    .filter(|(k, _)| !derived_key(&k.to_string_lossy()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                out.own = (!own.is_empty()).then_some(own);
            }
            let mut slots: Vec<Option<ItemStack>> = Vec::new();
            while let Ok(s) = e.item_slot(&*lv.host, slots.len() as u32) {
                slots.push(s);
            }
            if slots.iter().any(Option::is_some) {
                let _ = lv.host.transaction(|host, tx| {
                    for (i, s) in slots.iter().enumerate() {
                        if s.is_some() {
                            let _ = e.set_item_slot(host, tx, i as u32, None);
                        }
                    }
                    Ok::<(), ()>(())
                });
                out.slots = slots;
            }
        } else if block.is_none()
            && let Some(data) = e.data(&*lv.host)
        {
            // Emptied, so its block leaving spills nothing (`removeBlocksFromWorld` clears
            // containers first).
            let _ = e.replace_data(&mut *lv.host, Compound::new());
            out.data = Some(data);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// What contraption `key`'s deployers and tanks held waits for their blocks to be put back,
/// each at `place(offset)` (`addBlocksToWorld` places a block with its block entity's data):
/// a deployer placed there takes its state, a tank its part of its tank, which forms nothing
/// new. Called before the blocks go back.
pub(crate) fn expect_back(
    lv: &mut ServerLevel<'_>,
    key: [i32; 3],
    place: impl Fn([i32; 3]) -> [i32; 3],
) {
    use create_engine::tank;
    let dim = lv.dim;
    let list = HELD.with_borrow(|m| m.get(&(dim, key)).cloned());
    let Some(list) = list else {
        return;
    };
    // What the subsystems keep for each block, waiting for it where it goes back.
    let turn = create_engine::contraption::Transform::from_place(&place);
    for (o, data) in &list {
        if let (Some(engine), Some(b)) = (&data.engine, data.block) {
            let p = place(*o);
            for s in super::super::SUBSYSTEMS {
                s.carry_in(lv, p, b, engine, turn);
            }
            mark_pos_unsaved(dim, p);
        }
    }
    with_world(|w| {
        let d = &mut w.dims[dim];
        for (o, data) in &list {
            if let Some(dp) = &data.deployer {
                let mut dp = dp.clone();
                dp.breaking = None;
                d.deployer_saved_insert(place(*o), dp);
            }
            let Some(m) = data.tank else {
                continue;
            };
            // The tank's blocks where they go: still a tank as wide and tall (moved, or turned
            // about the vertical) keeps its shape, its corner the controller; else each block
            // is a tank of its own, the controller's holding what one block can.
            let cells: Vec<[i32; 3]> = tank::blocks(*o, m.width, m.height).map(&place).collect();
            let corner = cells.iter().fold(cells[0], |c, p| {
                [c[0].min(p[0]), c[1].min(p[1]), c[2].min(p[2])]
            });
            let same = tank::blocks(corner, m.width, m.height).all(|q| cells.contains(&q));
            let at = if same { corner } else { place(*o) };
            for q in cells {
                d.tanks.arriving.insert(q, None);
            }
            let mut m = m;
            if !same {
                m.width = 1;
                m.height = 1;
                m.tank.truncate(tank::CAPACITY);
            }
            d.tanks.arriving.insert(at, Some(m));
        }
    });
}

/// What contraption `key`'s other block entities held, back in theirs now at `place(offset)`:
/// slots into a Create block entity's slots, saved data loaded into another's; a deployer's
/// stack, or slots with nowhere to go, drop there.
fn release_held(lv: &mut ServerLevel<'_>, key: [i32; 3], place: impl Fn([i32; 3]) -> [i32; 3]) {
    let dim = lv.dim;
    let list = HELD
        .with_borrow_mut(|m| m.remove(&(dim, key)))
        .unwrap_or_default();
    for (o, data) in list {
        let p = place(o);
        // A deployer or tank that didn't go back leaves what waited for it.
        let left = with_world(|w| {
            let d = &mut w.dims[dim];
            d.tanks.arriving.remove(&p);
            d.deployer_saved_remove(p)
        })
        .flatten();
        if let Some((item, n)) = left.and_then(|d| d.held)
            && let Some(s) = stack_of(lv, &item)
        {
            let mut s = s;
            let _ = s.set_count(u32::from(n));
            lv.drop_stack(center(p), [0.0; 3], s, 10);
        }
        let entity = lv.host.block_entity(lv.level, p);
        if let Some(saved) = data.data {
            if let Some(e) = entity
                && !create_entity(lv, e)
            {
                let _ = e.replace_data(&mut *lv.host, saved);
            }
            continue;
        }
        // The same Create block back (turned, maybe): its data and what the engine kept for it
        // (`loadWithComponents`, `transform`).
        let back = data.block.is_some() && lv.create_block(p) == data.block;
        if back && let (Some(own), Some(e)) = (&data.own, entity) {
            let _ = e.merge_data(&mut *lv.host, own.clone());
            // What a block entity reads from its own data as it loads (`read`).
            if data.block == Some(Block::SequencedGearshift) {
                super::super::sequencer::loaded(&*lv.host, dim, p, e);
            }
        }
        if data.slots.is_empty() {
            continue;
        }
        let mut left = Vec::new();
        match entity.filter(|e| create_entity(lv, *e)) {
            Some(e) => {
                let _ = lv.host.transaction(|host, tx| {
                    for (i, s) in data.slots.into_iter().enumerate() {
                        let Some(stack) = s else { continue };
                        if e.set_item_slot(host, tx, i as u32, Some(stack.clone())) != Ok(true) {
                            left.push(stack);
                        }
                    }
                    Ok::<(), ()>(())
                });
            }
            None => left.extend(data.slots.into_iter().flatten()),
        }
        for stack in left {
            lv.drop_stack(center(p), [0.0; 3], stack, 10);
        }
    }
}

/// The items contraption `key` carried back into its storage blocks, each now at `place(offset)`
/// (dropped there when no storage block took its place).
pub(crate) fn release_storage(
    lv: &mut ServerLevel<'_>,
    key: [i32; 3],
    place: impl Fn([i32; 3]) -> [i32; 3],
) {
    let dim = lv.dim;
    release_held(lv, key, &place);
    let stored = STORAGE
        .with_borrow_mut(|m| m.remove(&(dim, key)))
        .unwrap_or_default();
    for (o, s) in stored {
        let p = place(o);
        // A fresh storage block where it went back: it takes the carried items.
        if is_storage(lv, p) {
            put_storage(lv, p, s);
            continue;
        }
        for stack in s.into_iter().flatten() {
            lv.drop_stack(center(p), [0.0; 3], stack, 10);
        }
    }
}

/// Every stack contraption `key` carries (`MountedStorageManager.getAllItems`), copied.
pub(crate) fn storage_stacks(dim: usize, key: [i32; 3]) -> Vec<ItemStack> {
    STORAGE.with_borrow(|m| {
        m.get(&(dim, key))
            .map(|list| {
                list.iter()
                    .flat_map(|(_, s)| s.iter().flatten().cloned())
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// What contraptions no longer kept (`keep(key)` false) carried is forgotten.
pub(crate) fn retain_storage(dim: usize, keep: impl Fn([i32; 3]) -> bool) {
    STORAGE.with_borrow_mut(|m| m.retain(|(d, k), _| *d != dim || keep(*k)));
}

/// The items contraption `key` carries, as a save keeps them: each storage block's offset and
/// its slots (`ItemStack.CODEC`, `Slot`). None when it carries none.
pub(crate) fn storage_to_nbt(lv: &ServerLevel<'_>, key: [i32; 3]) -> Option<Tag> {
    let dim = lv.dim;
    let slots_nbt = |o: &[i32; 3], slots: &[Option<ItemStack>]| {
        let items = slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let mut c = s.as_ref()?.to_nbt(&*lv.host)?;
                c.insert("Slot".into(), Tag::Int(i as i32));
                Some(Tag::Compound(c))
            })
            .collect();
        let mut c = Compound::new();
        c.insert("pos".into(), Tag::IntArray(o.to_vec()));
        c.insert("size".into(), Tag::Int(slots.len() as i32));
        c.insert("items".into(), Tag::List(items));
        c
    };
    let mut blocks: Vec<Tag> = STORAGE.with_borrow(|m| {
        m.get(&(dim, key)).map_or_else(Vec::new, |list| {
            list.iter()
                .map(|(o, slots)| Tag::Compound(slots_nbt(o, slots)))
                .collect()
        })
    });
    // Other block entities' data: marked `held`, with a deployer's and a tank's state.
    HELD.with_borrow(|m| {
        for (o, d) in m.get(&(dim, key)).into_iter().flatten() {
            let mut c = slots_nbt(o, &d.slots);
            let mut held = Compound::new();
            if let Some(data) = &d.data {
                held.insert("data".into(), Tag::Compound(data.clone()));
            }
            if let Some(b) = d.block {
                held.insert("block".into(), Tag::String(b.name().into()));
            }
            if let Some(own) = &d.own {
                held.insert("own".into(), Tag::Compound(own.clone()));
            }
            if let Some(engine) = &d.engine {
                held.insert("engine".into(), Tag::Compound(engine.clone()));
            }
            if let Some(dp) = &d.deployer {
                held.insert(
                    "deployer".into(),
                    crate::glue::processing::deployers_to_nbt(&[([0; 3], dp.clone())]),
                );
            }
            if let Some(t) = d.tank {
                let mut codes = create_engine::tank::Codes::default();
                let fluid = codes.code(t.tank.fluid);
                crate::glue::fluids::put_fluid_keys(&mut held, "tank", codes);
                held.insert(
                    "tank".into(),
                    Tag::IntArray(vec![
                        i32::from(t.width),
                        i32::from(t.height),
                        i32::from(t.window),
                        fluid,
                        t.tank.amount as i32,
                    ]),
                );
            }
            c.insert("held".into(), Tag::Compound(held));
            blocks.push(Tag::Compound(c));
        }
    });
    (!blocks.is_empty()).then_some(Tag::List(blocks))
}

/// What [`storage_to_nbt`] saved, carried again by contraption `key`.
pub(crate) fn storage_from_nbt(lv: &ServerLevel<'_>, key: [i32; 3], tag: Option<&Tag>) {
    let Some(Tag::List(list)) = tag else {
        return;
    };
    let mut stored: Mounted = Vec::new();
    let mut held: Vec<([i32; 3], BlockData)> = Vec::new();
    for b in list {
        let Tag::Compound(b) = b else { continue };
        let (Some(Tag::IntArray(p)), Some(Tag::List(items))) =
            (b.get(&"pos".into()), b.get(&"items".into()))
        else {
            continue;
        };
        let [x, y, z] = p[..] else { continue };
        let size = match b.get(&"size".into()) {
            Some(Tag::Int(n)) => (*n).clamp(0, 256) as usize,
            _ => STORAGE_SLOTS as usize,
        };
        let mut slots: Stored = vec![None; size];
        for item in items {
            let Tag::Compound(c) = item else { continue };
            let Some(Tag::Int(i)) = c.get(&"Slot".into()) else {
                continue;
            };
            if let Some(slot) = slots.get_mut(*i as usize) {
                *slot = ItemStack::from_nbt(&*lv.host, c);
            }
        }
        let Some(Tag::Compound(h)) = b.get(&"held".into()) else {
            stored.push(([x, y, z], slots));
            continue;
        };
        let mut d = BlockData {
            slots: if slots.iter().any(Option::is_some) {
                slots
            } else {
                Vec::new()
            },
            ..BlockData::default()
        };
        if let Some(Tag::Compound(data)) = h.get(&"data".into()) {
            d.data = Some(data.clone());
        }
        if let Some(Tag::String(b)) = h.get(&"block".into()) {
            d.block = Block::from_name(&b.to_string_lossy());
        }
        if let Some(Tag::Compound(own)) = h.get(&"own".into()) {
            d.own = Some(own.clone());
        }
        if let Some(Tag::Compound(engine)) = h.get(&"engine".into()) {
            d.engine = Some(engine.clone());
        }
        if let Some(t) = h.get(&"deployer".into()) {
            let c = Compound::from([("deployers".into(), t.clone())]);
            d.deployer = crate::glue::processing::deployers_from_nbt(&c)
                .into_iter()
                .next()
                .map(|e| e.1);
        }
        if let Some(Tag::IntArray(t)) = h.get(&"tank".into())
            && let [w, hh, win, f, n] = t[..]
        {
            d.tank = Some(create_engine::tank::Multi {
                width: w.clamp(1, 3) as u8,
                height: hh.clamp(1, 32) as u8,
                window: win != 0,
                tank: create_engine::tank::Tank {
                    fluid: create_engine::tank::Codes::fluid(
                        &crate::glue::fluids::fluid_keys(h, "tank"),
                        f,
                    ),
                    amount: n.max(0) as u32,
                },
            });
        }
        held.push(([x, y, z], d));
    }
    if !stored.is_empty() {
        STORAGE.with_borrow_mut(|m| m.insert((lv.dim, key), stored));
    }
    if !held.is_empty() {
        HELD.with_borrow_mut(|m| m.insert((lv.dim, key), held));
    }
}

/// What a contraption carried goes back with its blocks (`addBlocksToWorld`, `stop`): its
/// storage blocks' items into the storage blocks now at `origin` + their offset (dropped where
/// one couldn't be put back), its controls and glue there, and its breakers' cracks gone.
pub(super) fn release(
    lv: &mut ServerLevel<'_>,
    key: [i32; 3],
    origin: [i32; 3],
    carried: &mut Carried,
) {
    let dim = lv.dim;
    stop_actors(lv, &mut carried.actors);
    cancel_deployers(lv, key, carried);
    let at = |o: [i32; 3]| add(origin, o);
    // Rollers' settings back with the blocks put back.
    for a in carried.actors.iter().filter(|a| a.kind == Kind::Roller) {
        let p = at(a.local);
        if lv.create_block(p) == Some(Block::MechanicalRoller) {
            let v = (a.filter.clone(), a.mode);
            with_world(|w| w.dims[dim].rollers.insert(p, v));
            mark_pos_unsaved(dim, p);
        }
    }
    release_storage(lv, key, at);
    release_extras(lv, carried, at);
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
    /// `isRedstoneConductor`, read as a full collision cube (vanilla's default for it; the
    /// blocks that override it are not known to the API, `API_GAPS.md`).
    fn conductor(&self, pos: [i32; 3]) -> bool {
        let Some(state) = self.lv.block_state(pos) else {
            return false;
        };
        self.lv
            .level
            .block_shape(&*self.lv.host, state, pos, BlockShapeKind::Collision)
            .is_some_and(|b| b.len() == 1 && b[0] == [0.0, 0.0, 0.0, 1.0, 1.0, 1.0])
    }
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
/// Each actor's `stopMoving` (`cancelStall`): a breaker's cracks go, a contact it powered looks
/// again.
fn stop_actors(lv: &mut ServerLevel<'_>, list: &mut [Actor]) {
    for a in list.iter_mut() {
        let mut effects = Vec::new();
        a.cancel(&mut effects);
        for e in effects {
            match e {
                Effect::Crack(p, stage) => {
                    lv.level
                        .set_break_progress(&mut *lv.host, a.breaker_id, p, stage);
                }
                Effect::ContactLeft(p) => contact_left(lv, p),
                _ => {}
            }
        }
    }
}

/// `deactivateLastVisitedContact`: the contact at `p` looks at what it faces a tick later.
fn contact_left(lv: &mut ServerLevel<'_>, p: [i32; 3]) {
    let dim = lv.dim;
    let now = lv.game_time();
    with_world(|w| w.dims[dim].lifts.schedule(p, now));
}

/// `ContactMovementBehaviour.visitNewPosition`: a redstone contact at `p` facing the moving
/// contact (which faces `facing` now) is powered.
fn contact_reached(lv: &mut ServerLevel<'_>, p: [i32; 3], facing: Dir) {
    if let Some((Block::RedstoneContact, info)) = create_info(lv.block(p))
        && info.facing == Some(facing.opposite())
    {
        let dim = lv.dim;
        with_world(|w| w.dims[dim].lifts.contact_held.insert(p));
        if !info.powered {
            lv.set_flags(p, &[("powered", true)], update_all());
        }
    }
}

/// `BlockHelper.destroyBlockAs` with no player: the block broken with its loot drawn with
/// `tool` (none when blocks don't drop), mined ice becoming water over a block that holds it.
/// The drops, for the actor to collect or drop.
fn break_block(lv: &mut ServerLevel<'_>, at: [i32; 3], tool: Option<&ItemStack>) -> Vec<ItemStack> {
    let ice = lv.block_name(at).is_some_and(|n| n == "minecraft:ice");
    crate::glue::processing::toolbelt::before_removal(lv, at);
    let mut drops = lv
        .level
        .destroy_block(&mut *lv.host, at, true, tool, true)
        .ok()
        .flatten()
        .unwrap_or_default();
    crate::glue::processing::toolbelt::attach(lv.dim, at, &mut drops);
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
        // `ItemHandlerHelper.insertItem`: slot by slot, onto the same items or into an empty
        // slot. A stack can't hold none, so what is left is counted beside it.
        let mut left = stack.count();
        for (_, s) in list.iter_mut().filter(|(_, s)| takes_any(s)) {
            for slot in s.iter_mut() {
                if left == 0 {
                    return None;
                }
                match slot {
                    None => {
                        let put = left.min(limit);
                        let mut s = stack.clone();
                        let _ = s.set_count(put);
                        *slot = Some(s);
                        left -= put;
                    }
                    Some(s) if same_stack(s, &stack) => {
                        let put = left.min(limit.saturating_sub(s.count()));
                        let _ = s.set_count(s.count() + put);
                        left -= put;
                    }
                    _ => {}
                }
            }
        }
        (left > 0).then(|| {
            let mut s = stack;
            let _ = s.set_count(left);
            s
        })
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
/// breakers, harvesters, ploughs and rollers, each a breaker of its own. Its storage rides it
/// apart (`carry_storage`); its glue and controls don't.
pub(crate) fn bearing_actors(blocks: &[([i32; 3], u32)]) -> Carried {
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
pub(crate) fn cancel_actors(lv: &mut ServerLevel<'_>, key: [i32; 3], carried: &mut Carried) {
    stop_actors(lv, &mut carried.actors);
    cancel_deployers(lv, key, carried);
}

/// `DeployerMovementBehaviour.cancelStall` as contraption `key` stops: what its deployers'
/// fake players were breaking is left as it was, their cracks gone.
fn cancel_deployers(lv: &mut ServerLevel<'_>, key: [i32; 3], carried: &Carried) {
    let dim = lv.dim;
    for a in carried.actors.iter().filter(|a| a.kind == Kind::Deployer) {
        let broke = HELD.with_borrow_mut(|m| {
            let d = m
                .get_mut(&(dim, key))?
                .iter_mut()
                .find(|(o, _)| *o == a.local)?
                .1
                .deployer
                .as_mut()?;
            d.breaking.take().map(|(p, _)| p)
        });
        if let Some(p) = broke {
            lv.level
                .set_break_progress(&mut *lv.host, a.breaker_id, p, -1);
        }
    }
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
        let mut stall = a.tick(
            position,
            facing,
            actors::CONTROLLED_LOWER,
            &Reader { lv, key },
            &mut effects,
        );
        let a = carried.actors[i].clone();
        for e in effects {
            if let Some(s) = apply(lv, key, &a, e) {
                carried.actors[i].stall = s;
                stall = s;
            }
        }
        stalled |= stall;
    }
    stalled
}

/// `ContraptionCollider.isCollidingWithWorld` for an actor: the contraption's block at `local`
/// doesn't stop the contraption at `cell` when it is a breaker (drill, saw, plough, roller) that
/// can break the block there (`canBreak`), or a harvester that can harvest it (`isValidCrop`,
/// `isValidOther`); no block passes cocoa.
pub(super) fn breaker_clears(
    lv: &ServerLevel<'_>,
    key: [i32; 3],
    carried: &Carried,
    local: [i32; 3],
    cell: [i32; 3],
) -> bool {
    use actors::Probe;
    if lv.block_name(cell).as_deref() == Some("minecraft:cocoa") {
        return true;
    }
    let reader = Reader { lv, key };
    carried
        .actors
        .iter()
        .find(|a| a.local == local)
        .is_some_and(|a| match a.kind {
            Kind::Harvester => harvestable(lv, cell),
            k if k.breaks() => reader.breakable(k, cell).is_some(),
            _ => false,
        })
}

/// A block state's properties by name.
type Props = Vec<(String, String)>;

/// What a harvester reads of the plant at `p`.
fn reached(lv: &ServerLevel<'_>, p: [i32; 3]) -> Option<(String, Props, actors::Reached)> {
    let (name, props) = lv.name_props(p)?;
    if AIRS.contains(&name.as_str()) {
        return None;
    }
    let bush = lv.in_tag(p, "minecraft:flowers") || lv.in_tag(p, "minecraft:saplings");
    let age = prop(&props, "age")
        .and_then(|v| v.parse::<u8>().ok())
        .zip(actors::age_max(&name));
    let r = actors::Reached {
        plant: actors::plant_of(&name, bush),
        age,
        no_collision: lv.collision(p).is_empty(),
        leaves: lv.in_tag(p, "minecraft:leaves"),
    };
    Some((name, props, r))
}

/// `isValidCrop || isValidOther` for the block at `p`.
fn harvestable(lv: &ServerLevel<'_>, p: [i32; 3]) -> bool {
    reached(lv, p).is_some_and(|(_, _, r)| actors::valid_crop(&r) || actors::valid_other(&r))
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
        let mut stall = a.tick(
            position,
            facing,
            actors::CONTROLLED_LOWER,
            &Reader { lv, key },
            &mut effects,
        );
        let a = carried.actors[i].clone();
        for e in effects {
            if let Some(s) = apply(lv, key, &a, e) {
                carried.actors[i].stall = s;
                stall = s;
            }
        }
        stalled |= stall;
    }
    stalled
}

/// What an actor's effect does in the world; a deployer's says whether it stalls now.
fn apply(lv: &mut ServerLevel<'_>, key: [i32; 3], a: &Actor, e: Effect) -> Option<bool> {
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
        Effect::Deploy(p) => return Some(deploy(lv, key, a, Some(p))),
        Effect::Contact(p) => contact_reached(lv, p, actors::posed_facing(a.facing, &a.turns)),
        Effect::ContactLeft(p) => contact_left(lv, p),
        Effect::Redeploy => return Some(deploy(lv, key, a, None)),
        Effect::Entities { pos, damage, boost } => hit_entities(lv, a, pos, damage, boost),
    }
    None
}

/// The channel a thrown player's client is told its push on (`throwEntity` runs on the client
/// for players).
pub const THROW_CHANNEL: &str = "create:actor_throw";

thread_local! {
    /// Entities a moving structure's actors leave alone while they tick: the cart carrying it
    /// (`damageEntities`' minecart test), set by the one ticking them.
    static EXEMPT: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    /// The actors ticking ride a cart or a train (`MountedContraption`, `CarriageContraption`).
    static ON_VEHICLE: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f` for the actors of a structure on a cart or a train, with `ids` left alone by them
/// (`isPassengerOfSameVehicle`, the carrying cart).
pub fn on_vehicle<R>(ids: &[u32], f: impl FnOnce() -> R) -> R {
    EXEMPT.with_borrow_mut(|e| e.extend_from_slice(ids));
    let was = ON_VEHICLE.replace(true);
    let r = f();
    ON_VEHICLE.set(was);
    EXEMPT.with_borrow_mut(|e| e.truncate(e.len().saturating_sub(ids.len())));
    r
}

/// The display and marker entities the port draws moving structures and seats with
/// (`AbstractContraptionEntity`, which `damageEntities` passes).
fn drawing_entity(kind: &str) -> bool {
    matches!(
        kind,
        "minecraft:block_display"
            | "minecraft:item_display"
            | "minecraft:text_display"
            | "minecraft:interaction"
            | "minecraft:marker"
    )
}

/// `BlockBreakingMovementBehaviour.damageEntities` at block `pos`: every entity in it but items
/// and the structure's own (its drawing, what rides it, its cart) is hurt by `damage` (Create's
/// drill, saw and roller damage types are stood in for by `generic`, `API_GAPS.md`) and
/// thrown by `boost`; a player's own client throws it (`push_player`). A plough throwing an
/// anvil hard enough gives `ANVIL_PLOUGH` to the players within 32 blocks.
fn hit_entities(
    lv: &mut ServerLevel<'_>,
    a: &Actor,
    pos: [i32; 3],
    damage: Option<f32>,
    boost: [f64; 3],
) {
    let lo = pos.map(f64::from);
    let hi = pos.map(|v| f64::from(v) + 1.0);
    let Some(found) = lv.level.entities_in(&*lv.host, lo, hi) else {
        return;
    };
    let exempt = EXEMPT.with_borrow(Clone::clone);
    for e in found {
        let host: &dyn mod_api::Host = &*lv.host;
        if lv.host.item_entity(e).is_some() || exempt.contains(&e.id()) {
            continue;
        }
        let kind = e
            .kind(host)
            .map(|k| k.id(host).to_string())
            .unwrap_or_default();
        if drawing_entity(&kind) {
            continue;
        }
        // A rider of the structure's seats sits on one of its stand-ins.
        let root = e.root_vehicle(host).unwrap_or(e);
        if root != e
            && root
                .kind(host)
                .is_some_and(|k| drawing_entity(&k.id(host).to_string()))
        {
            continue;
        }
        if let Some(d) = damage {
            let _ = e.hurt(&mut *lv.host, "minecraft:generic", d, None, None);
        }
        if kind == "minecraft:player" {
            if let Some(p) = e.uuid(&mut *lv.host).and_then(|u| lv.host.player(u)) {
                let words: Vec<u8> = boost.iter().flat_map(|v| v.to_le_bytes()).collect();
                crate::glue::net::send_to_player(&mut *lv.host, p, THROW_CHANNEL, &words);
            }
            continue;
        }
        let before = e.velocity(&*lv.host).unwrap_or_default();
        e.push(&mut *lv.host, boost);
        // `PloughMovementBehaviour.throwEntity`: an anvil falling at 0.25 or more.
        if a.kind == Kind::Plough && kind == "minecraft:falling_block" {
            let v = [0, 1, 2].map(|i| before[i] + boost[i]);
            let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            let anvil = e
                .nbt(&mut *lv.host)
                .and_then(|n| match n.get(&"BlockState".into()) {
                    // The state as text (`minecraft:anvil[facing=north]`), or a compound.
                    Some(Tag::String(text)) => Some(text.to_string_lossy()),
                    Some(Tag::Compound(b)) => match b.get(&"Name".into()) {
                        Some(Tag::String(name)) => Some(name.to_string_lossy()),
                        _ => None,
                    },
                    _ => None,
                })
                .is_some_and(|n| {
                    // `instanceof AnvilBlock`: the anvil and its chipped and damaged kinds.
                    let name = n.split('[').next().unwrap_or_default();
                    matches!(
                        name,
                        "minecraft:anvil" | "minecraft:chipped_anvil" | "minecraft:damaged_anvil"
                    )
                });
            if anvil && speed >= 0.25 {
                plough_anvil_award(lv, e);
            }
        }
    }
}

/// `AllAdvancements.ANVIL_PLOUGH` for the players in `AABB(entity.blockPosition()).inflate(32)`.
fn plough_anvil_award(lv: &mut ServerLevel<'_>, anvil: mod_api::EntityRef) {
    let Some(b) = anvil.block_position(&*lv.host) else {
        return;
    };
    let lo = b.map(|v| f64::from(v) - 32.0);
    let hi = b.map(|v| f64::from(v) + 33.0);
    for p in lv.players() {
        let Some(at) = p.entity().position(&*lv.host) else {
            continue;
        };
        if (0..3).all(|i| at[i] >= lo[i] && at[i] <= hi[i]) {
            crate::glue::advancements::award(&mut *lv.host, p, "anvil_plough");
        }
    }
}

/// A thrown player's client: the push the structure gave it (`throwEntity` on the client).
pub fn thrown(host: &mut dyn mod_api::ClientHost, data: &[u8]) {
    if data.len() != 24 {
        return;
    }
    let v: Vec<f64> = data
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| f64::from_le_bytes(*c))
        .collect();
    host.push_player([v[0] as f32, v[1] as f32, v[2] as f32]);
}

/// `DeployerMovementBehaviour.shouldActivate` (`DeployerHandler.shouldActivate` with no
/// facing): not with a block item at its own block, nor a bucket at its fluid's source.
fn deployer_should_use(lv: &ServerLevel<'_>, held: Option<&(String, u8)>, at: [i32; 3]) -> bool {
    let Some((item, _)) = held else {
        return true;
    };
    if let Some(block) = item_key_of(lv, item).and_then(|k| lv.host.item_block(k))
        && lv.level.block(&*lv.host, at) == Some(block)
    {
        return false;
    }
    let fluid = match item.as_str() {
        "minecraft:water_bucket" => "minecraft:water",
        "minecraft:lava_bucket" => "minecraft:lava",
        _ => return true,
    };
    lv.block_name(at).as_deref() != Some(fluid)
        || lv.property(at, "level").is_some_and(|l| l != "0")
}

/// `ItemHelper.extract(storage, filter, 1, false)` for a deployer's empty hand: one of the first
/// stack in contraption `key`'s storage that the deployer's filter (`getFilterFromBE`, carried
/// with its block) passes.
fn take_one(lv: &ServerLevel<'_>, key: [i32; 3], local: [i32; 3]) -> Option<ItemStack> {
    let dim = lv.dim;
    let host = &*lv.host;
    let filter = HELD.with_borrow(|m| {
        let (_, d) = m.get(&(dim, key))?.iter().find(|(o, _)| *o == local)?;
        let tag = d
            .engine
            .as_ref()?
            .get(&crate::glue::logistics::carried::FILTER.into())?;
        filters::filter_from_nbt(host, tag)
    });
    STORAGE.with_borrow_mut(|m| {
        let list = m.get_mut(&(dim, key))?;
        for (_, s) in list.iter_mut() {
            for slot in s.iter_mut() {
                let Some(stack) = slot.as_ref() else {
                    continue;
                };
                if !filters::passes(host, filter.as_ref(), stack) {
                    continue;
                }
                let mut one = stack.clone();
                let _ = one.set_count(1);
                *slot = slot.take().and_then(|s| s.shrink(1));
                return Some(one);
            }
        }
        None
    })
}

/// A deployer riding contraption `key` (`DeployerMovementBehaviour`): at a block it reaches
/// (`at`), or (None) at the block its fake player breaks, 20 ticks after its last punch
/// (`tick`). An empty hand takes an item from the contraption's storage (`tryGrabbingItem`); a
/// using deployer does nothing where `shouldActivate` says no; its fake player acts as a
/// standing deployer's from where the deployer is now (`activate`), and what it ended with
/// besides its hand goes into the storage or drops (`tryDisposeOfExcess`). Whether it stalls
/// the contraption (its fake player breaks a block).
fn deploy(lv: &mut ServerLevel<'_>, key: [i32; 3], a: &Actor, at: Option<[i32; 3]>) -> bool {
    use create_engine::deployer::Mode;
    let dim = lv.dim;
    let found = HELD.with_borrow(|m| {
        m.get(&(dim, key))?
            .iter()
            .find(|(o, _)| *o == a.local)?
            .1
            .deployer
            .clone()
    });
    let Some(mut d) = found else {
        return false;
    };
    let Some(pos) = at.or(d.breaking.map(|b| b.0)) else {
        return false;
    };
    if d.held.is_none()
        && let Some(s) = take_one(lv, key, a.local)
    {
        crate::glue::processing::set_deployer_held(&*lv.host, &mut d, Some(&s));
    }
    let facing = actors::posed_facing(a.facing, &a.turns);
    let mut stall = false;
    let mut other = Vec::new();
    if d.mode == Mode::Punch || deployer_should_use(lv, d.held.as_ref(), pos) {
        // The deployer two back from the block it reaches, as `context.position - facing × 2`.
        let o = facing.offset();
        let origin = [0, 1, 2].map(|i| pos[i] - 2 * o[i]);
        let before = d.held.clone();
        other =
            crate::glue::processing::activate_deployer(lv, origin, facing, &mut d, a.breaker_id);
        stall = d.breaking.is_some();
        // `checkForTrackPlacementAdvancement`: on a cart or a train, a rail or track it placed
        // (`placedTracks`) gives its placer `SELF_DEPLOYING`.
        if ON_VEHICLE.get()
            && let Some((item, n)) = &before
            && d.held.as_ref().is_none_or(|(i, m)| i != item || m < n)
            && placed_track(lv, item)
        {
            let owner = HELD.with_borrow(|m| {
                let held = m.get(&(dim, key))?;
                let data = held.iter().find(|(o, _)| *o == a.local)?;
                crate::glue::advancements::carried_owner(data.1.engine.as_ref()?)
            });
            if let Some(p) = owner.and_then(|u| lv.host.player(u)) {
                crate::glue::advancements::award(&mut *lv.host, p, "self_deploying");
            }
        }
        // `tryHarvestBlock`: the broken block's drops go into the fake player's inventory
        // (the game's break drops them where the block was).
        if d.mode == Mode::Punch && !stall {
            let lo = pos.map(|v| f64::from(v) - 0.25);
            let hi = pos.map(|v| f64::from(v) + 1.25);
            for drop in lv.items_in(lo, hi) {
                if drop.item.age(&*lv.host) == Some(0) {
                    other.push(drop.stack.clone());
                    lv.set_dropped(&drop, None, [0.0; 3]);
                }
            }
        }
    }
    HELD.with_borrow_mut(|m| {
        if let Some(e) = m
            .get_mut(&(dim, key))
            .and_then(|l| l.iter_mut().find(|(o, _)| *o == a.local))
        {
            e.1.deployer = Some(d);
        }
    });
    for s in other {
        collect(lv, key, a, s);
    }
    stall
}

/// Whether `item` places a rail or a track (`IBaseRailBlockExtension`, `ITrackBlock`).
fn placed_track(lv: &ServerLevel<'_>, item: &str) -> bool {
    let Some(block) = item_key_of(lv, item).and_then(|k| lv.host.item_block(k)) else {
        return false;
    };
    lv.host.block_in_tag(block, "minecraft:rails") == Some(true)
        || lv.host.block_in_tag(block, "create:tracks") == Some(true)
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
    let Some((name, props, reached)) = reached(lv, p) else {
        return;
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
    interface_connected(lv, pos, Block::PortableStorageInterface)
}

/// The contraption an interface of kind `kind` at `pos` hands its storage to now.
fn interface_connected(lv: &ServerLevel<'_>, pos: [i32; 3], kind: Block) -> Option<[i32; 3]> {
    let state = lv.block(pos);
    create_engine::states::index(state)?;
    let dim = lv.dim;
    with_world(|w| {
        let (b, _) = w.table.get(state)?;
        if b != kind {
            return None;
        }
        w.dims[dim].psis.get(&pos)?.can_transfer()
    })
    .flatten()
}

/// The fluid tanks contraption `key` carries (`MountedStorageManager.getFluids`: its tanks'
/// contents, controller by controller), changed by `f`.
fn with_carried_tanks<R>(
    dim: usize,
    key: [i32; 3],
    f: impl FnOnce(&mut [&mut create_engine::tank::Multi]) -> R,
) -> Option<R> {
    HELD.with_borrow_mut(|m| {
        let list = m.get_mut(&(dim, key))?;
        let mut tanks: Vec<&mut create_engine::tank::Multi> = list
            .iter_mut()
            .filter_map(|(_, d)| d.tank.as_mut())
            .collect();
        Some(f(&mut tanks))
    })
}

/// A portable fluid interface at `pos` as a smart observer's or threshold switch's fluid handler
/// (`PortableFluidInterfaceBlockEntity.capability`): the tanks of the contraption it is connected
/// to (`MountedStorageManager.getFluids`), else one tank of no capacity (`createEmptyHandler`);
/// None for any other block.
pub fn pfi_tanks(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
) -> Option<Vec<create_engine::observe::TankSlot>> {
    use create_engine::observe::TankSlot;
    if lv.create_block(pos) != Some(Block::PortableFluidInterface) {
        return None;
    }
    let empty = || vec![TankSlot::of(None, 0)];
    let Some(key) = interface_connected(lv, pos, Block::PortableFluidInterface) else {
        return Some(empty());
    };
    let tanks = with_carried_tanks(lv.dim, key, |tanks| {
        tanks
            .iter()
            .map(|m| TankSlot::of(m.tank.fluid.map(|f| (f, m.tank.amount)), m.capacity()))
            .collect::<Vec<_>>()
    });
    Some(tanks.unwrap_or_else(empty))
}

/// A portable fluid interface at `pos` as a pump's end (`InterfaceFluidHandler.drain`, simulated):
/// what the tanks of the contraption it is connected to give, up to `amount` of the first fluid
/// in them; Some(None) when it is connected to nothing, None for any other block.
pub fn pfi_offer(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    amount: u32,
    only: Option<create_engine::tank::Fluid>,
) -> Option<Option<(create_engine::tank::Fluid, u32)>> {
    if lv.create_block(pos) != Some(Block::PortableFluidInterface) {
        return None;
    }
    let Some(key) = interface_connected(lv, pos, Block::PortableFluidInterface) else {
        return Some(None);
    };
    let offer = with_carried_tanks(lv.dim, key, |tanks| {
        let fluid = tanks
            .iter()
            .find_map(|t| t.tank.fluid.filter(|f| only.is_none_or(|want| want == *f)))?;
        let have: u32 = tanks
            .iter()
            .filter(|t| t.tank.fluid == Some(fluid))
            .map(|t| t.tank.amount)
            .sum();
        Some((fluid, have.min(amount)))
    });
    Some(offer.flatten())
}

/// FluidNetwork keeps a portable source engaged while its frontier is live.
pub fn pfi_keep_flow(lv: &ServerLevel<'_>, pos: [i32; 3]) {
    if lv.create_block(pos) == Some(Block::PortableFluidInterface)
        && interface_connected(lv, pos, Block::PortableFluidInterface).is_some()
    {
        psi_transferred(lv.dim, pos);
    }
}

/// What a pump drew from a portable fluid interface leaves the contraption's tanks
/// (`InterfaceFluidHandler.drain`): `amount` of their first fluid, tank by tank.
pub fn pfi_drain(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    amount: u32,
    fluid: create_engine::tank::Fluid,
) -> u32 {
    let Some(key) = interface_connected(lv, pos, Block::PortableFluidInterface) else {
        return 0;
    };
    let mut left = amount;
    with_carried_tanks(lv.dim, key, |tanks| {
        for t in tanks.iter_mut().filter(|t| t.tank.fluid == Some(fluid)) {
            if left == 0 {
                break;
            }
            left -= t.tank.drain_amount(left).map_or(0, |d| d.1);
        }
    });
    if left < amount {
        psi_transferred(lv.dim, pos);
    }
    amount - left
}

/// A pump filling a portable fluid interface (`InterfaceFluidHandler.fill`): into the tanks of
/// the contraption it is connected to, tank by tank (`CombinedTankWrapper`); how much went in.
/// None for any other block.
pub fn pfi_fill(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    fluid: create_engine::tank::Fluid,
    amount: u32,
) -> Option<u32> {
    pfi_fill_action(lv, pos, fluid, amount, true)
}

pub fn pfi_fill_action(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    fluid: create_engine::tank::Fluid,
    amount: u32,
    execute: bool,
) -> Option<u32> {
    if lv.create_block(pos) != Some(Block::PortableFluidInterface) {
        return None;
    }
    let Some(key) = interface_connected(lv, pos, Block::PortableFluidInterface) else {
        return Some(0);
    };
    let took = with_carried_tanks(lv.dim, key, |tanks| {
        let mut left = amount;
        for t in tanks.iter_mut() {
            if left == 0 {
                break;
            }
            let capacity = t.capacity();
            left -= if execute {
                t.tank.fill_amount(fluid, left, capacity)
            } else {
                t.tank.clone().fill_amount(fluid, left, capacity)
            };
        }
        amount - left
    })
    .unwrap_or(0);
    if execute && took > 0 {
        psi_transferred(lv.dim, pos);
    }
    Some(took)
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
    crate::glue::advancements::award_later(dim, pos, "portable_storage_interface");
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
            for (_, s) in list.iter_mut().filter(|(_, s)| takes_any(s)) {
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
        crate::glue::trains::cargo_moved(dim, key);
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
        crate::glue::trains::cargo_moved(dim, key);
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
/// The item in the filter slot of the contraption controls or roller at `pos`.
pub(crate) fn mounted_filter(dim: usize, pos: [i32; 3], roller: bool) -> Option<String> {
    with_world(|w| {
        let d = &w.dims[dim];
        if roller {
            d.rollers.get(&pos).and_then(|r| r.0.clone())
        } else {
            d.controls.get(&pos).and_then(|c| c.filter.clone())
        }
    })
    .flatten()
}

/// `FilteringBehaviour.setFilter` on the contraption controls or roller at `pos`: refused when
/// the item fails its predicate (`CONTRAPTION_CONTROLLED`, `isValidMaterial`); a roller's is
/// shared with the rollers in line. Whether it was set.
pub(crate) fn set_mounted_filter(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    roller: bool,
    item: Option<String>,
) -> bool {
    let fits = match item.as_deref() {
        None => true,
        Some(i) if roller => valid_pave_material(lv, i),
        Some(i) => actors::CONTRAPTION_CONTROLLED.contains(&i),
    };
    if !fits {
        return false;
    }
    let dim = lv.dim;
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
    true
}

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

/// A roller's mode picked on its value box (`mode`, `onModeChanged`): shared with the rollers
/// in line.
pub(super) fn set_roller_mode(lv: &mut ServerLevel<'_>, pos: [i32; 3], mode: i32) {
    let Some(m) = actors::RollingMode::ALL
        .get(mode.clamp(0, 2) as usize)
        .copied()
    else {
        return;
    };
    if roller_at(lv, pos).is_none() {
        return;
    }
    let dim = lv.dim;
    with_world(|w| w.dims[dim].rollers.entry(pos).or_default().1 = m);
    share_roller(lv, pos);
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
            super::actor_draw::sync_controls(lv, pos);
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
    let ticked = with_world(|w| {
        let d = &mut w.dims[dim];
        let mut list = std::mem::take(&mut d.psi_running);
        let mut ticked = Vec::new();
        list.retain(|p| {
            let Some(s) = d.psis.get_mut(p) else {
                return false;
            };
            let was = s.is_connected();
            s.tick();
            ticked.push((*p, *s));
            if was != s.is_connected() {
                d.analog_checks.push(*p);
            }
            !s.idle()
        });
        d.psi_running = list;
        ticked
    })
    .unwrap_or_default();
    // Their clients run the timers on, and hear when theirs would be wrong.
    for (p, s) in ticked {
        super::actor_draw::sync_interface(lv, p, &s);
    }
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
    /// with the rollers in line; its mode is set on its value box (`interaction::values`).
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let c = super::clicker(&*host, event.click);
        // The main hand's empty click was the block's (`useWithoutItem` succeeds on the client
        // too): the off hand is not tried after it.
        if c.after_empty_main {
            return Some(InteractionResult::Success);
        }
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
    super::actor_draw::sync_controls(lv, pos);
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
/// carried by the moving contraption keyed at the position (a piston, a gantry carriage's start,
/// a rope pulley, a bearing or windmill, a clockwork bearing, or the block a minecart with a
/// contraption is in) (`ContraptionControlsMovingInteraction.handlePlayerInteraction`; clicking
/// a cart's blocks needs a moving entity). `block` clicks the first contraption controls block
/// it carries, with that block's filter, its status and click sound; a filter names the kind
/// directly.
fn debug_controls(
    lv: &mut ServerLevel<'_>,
    player: Option<mod_api::PlayerRef>,
    key: [i32; 3],
    filter: Option<Option<String>>,
) -> String {
    toggle_controls(lv, player, key, filter, None)
}

/// A player's click on the contraption controls block at offset `local` of the structure the
/// client drew at `anchor` (a piston, rope pulley, bearing or windmill, or the shaft a gantry
/// carriage started on): as `/create_debug controls .. block` on that block
/// (`ContraptionControlsMovingInteraction`); on an elevator it picks the floor its client chose.
pub(super) fn click_moving_controls(
    lv: &mut ServerLevel<'_>,
    player: mod_api::PlayerRef,
    anchor: [i32; 3],
    local: [i32; 3],
    target: Option<i32>,
) {
    if super::lifts::elevator_controls_clicked(lv, player, anchor, local, target) {
        return;
    }
    let dim = lv.dim;
    let gantry = with_world(|w| {
        w.dims[dim]
            .gantries
            .iter()
            .find(|g| add(g.start, g.carriage_facing.opposite().offset()) == anchor)
            .map(|g| g.start)
    })
    .flatten();
    toggle_controls(
        lv,
        Some(player),
        gantry.unwrap_or(anchor),
        None,
        Some(local),
    );
}

/// Where the moving contraption keyed at `key` keeps what it carries.
enum Holder {
    Piston,
    Gantry,
    Bearing,
    Lift,
    Cart(u128),
}

/// Where a block of a moving contraption is in the world now, by its offset.
pub(crate) type Place = Box<dyn Fn([i32; 3]) -> [i32; 3]>;

/// What the moving contraption keyed at `key` carries, taken out to be changed, and where each
/// of its blocks is now; put back with [`put_carried`].
fn take_carried(lv: &mut ServerLevel<'_>, key: [i32; 3]) -> Option<(Holder, Carried, Place)> {
    let dim = lv.dim;
    let world = with_world(|w| {
        let d = &mut w.dims[dim];
        if let Some(m) = d.pistons.get_mut(&key) {
            let origin = m.anchor;
            return Some((Holder::Piston, std::mem::take(&mut m.carried), origin));
        }
        let g = d.gantries.iter_mut().find(|g| g.start == key)?;
        Some((Holder::Gantry, std::mem::take(&mut g.carried), g.start))
    })
    .flatten();
    if let Some((holder, carried, origin)) = world {
        return Some((holder, carried, Box::new(move |o| add(origin, o))));
    }
    if let Some((carried, place)) = super::bearings::take_turning(lv, key) {
        return Some((Holder::Bearing, carried, place));
    }
    if let Some((carried, place)) = super::lifts::take_lift(lv, key) {
        return Some((Holder::Lift, carried, place));
    }
    let (uuid, carried, place) = crate::glue::trains::take_cart_carried(dim, key)?;
    Some((Holder::Cart(uuid), carried, place))
}

/// What [`take_carried`] took goes back.
fn put_carried(lv: &mut ServerLevel<'_>, key: [i32; 3], holder: Holder, carried: Carried) {
    let dim = lv.dim;
    match holder {
        Holder::Piston => {
            with_world(|w| {
                if let Some(m) = w.dims[dim].pistons.get_mut(&key) {
                    m.carried = carried;
                }
            });
            mark_pos_unsaved(dim, key);
        }
        Holder::Gantry => {
            with_world(|w| {
                if let Some(g) = w.dims[dim].gantries.iter_mut().find(|g| g.start == key) {
                    g.carried = carried;
                }
            });
            mark_pos_unsaved(dim, key);
        }
        Holder::Bearing => super::bearings::put_turning(lv, key, carried),
        Holder::Lift => super::lifts::put_lift(lv, key, carried),
        Holder::Cart(uuid) => crate::glue::trains::put_cart_carried(uuid, carried),
    }
}

/// Turns the actors of the moving contraption `key` off or on as contraption controls do:
/// `filter` names the kind (None: the filter of the controls block at offset `which`, or of
/// the first it carries). On an elevator a click on its controls picks a floor instead.
fn toggle_controls(
    lv: &mut ServerLevel<'_>,
    player: Option<mod_api::PlayerRef>,
    key: [i32; 3],
    filter: Option<Option<String>>,
    which: Option<[i32; 3]>,
) -> String {
    let Some((holder, mut carried, place)) = take_carried(lv, key) else {
        return format!("no moving contraption at {} {} {}", key[0], key[1], key[2]);
    };
    let (filter, at) = match filter {
        Some(f) => (f, None),
        None => {
            let found = match which {
                Some(w) => carried.controls.iter().find(|(o, _)| *o == w),
                None => carried.controls.first(),
            };
            let Some((o, c)) = found else {
                put_carried(lv, key, holder, carried);
                return format!(
                    "no contraption controls on the contraption at {} {} {}",
                    key[0], key[1], key[2]
                );
            };
            (c.filter.clone(), Some(place(*o)))
        }
    };
    let mut effects = Vec::new();
    let kinds: Vec<&str> = carried.actors.iter().map(|a| a.kind.item()).collect();
    let disable = actors::toggle(&mut carried.disabled, &filter, &kinds);
    actors::set_active(&mut carried.actors, &filter, !disable, &mut effects);
    let breaker = carried.actors.first().map_or(0, |a| a.breaker_id);
    put_carried(lv, key, holder, carried);
    // The actors turned off stop (`cancelStall`): cracks go, contacts look again.
    for e in effects {
        match e {
            Effect::Crack(p, stage) => {
                lv.level
                    .set_break_progress(&mut *lv.host, breaker, p, stage);
            }
            Effect::ContactLeft(p) => contact_left(lv, p),
            _ => {}
        }
    }
    if let Some(at) = at {
        controls_status(lv, player, at, filter.as_deref(), !disable);
    }
    let what = filter.as_deref().unwrap_or("all actors");
    format!("{what} {}", if disable { "off" } else { "on" })
}

/// `/create_debug carried <pos>`: what the moving structure anchored at `pos` carries for its
/// blocks (each offset, its block, data and the engine's state); for any other block, what a
/// structure would carry for it, taken and put back where it is.
fn debug_carried(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> String {
    let dim = lv.dim;
    fn text(t: &Tag) -> String {
        match t {
            Tag::Compound(c) => {
                let inner: Vec<String> = c
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.to_string_lossy(), text(v)))
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
            Tag::List(l) => format!("[{}]", l.iter().map(text).collect::<Vec<_>>().join(", ")),
            Tag::String(s) => format!("{:?}", s.to_string_lossy()),
            Tag::IntArray(v) => format!("{v:?}"),
            other => format!("{other:?}"),
        }
    }
    let show = |o: &[i32; 3], d: &BlockData| {
        let slots = d.slots.iter().filter(|s| s.is_some()).count();
        format!(
            "{o:?} {:?} slots={slots} own={} engine={}",
            d.block,
            d.own
                .clone()
                .map_or("-".into(), |c| text(&Tag::Compound(c))),
            d.engine
                .clone()
                .map_or("-".into(), |c| text(&Tag::Compound(c))),
        )
    };
    if let Some(list) = HELD.with_borrow(|m| m.get(&(dim, pos)).cloned()) {
        return list
            .iter()
            .map(|(o, d)| show(o, d))
            .collect::<Vec<_>>()
            .join("; ");
    }
    let Some(d) = take_block_data(lv, pos) else {
        return "nothing carried".into();
    };
    let line = show(&[0; 3], &d);
    let key = [i32::MIN + 1, pos[1], i32::MIN + 1];
    HELD.with_borrow_mut(|m| m.insert((dim, key), vec![([0; 3], d)]));
    expect_back(lv, key, |o| add(pos, o));
    // The block stays: it is found again as a loaded one is, taking what waits for it.
    let state = lv.block(pos);
    with_world(|w| w.block_loaded(dim, pos, state));
    release_held(lv, key, |o| add(pos, o));
    line
}

pub(super) fn register_commands(event: &mut RegisterCommandsEvent<'_>) {
    for usage in [
        "create_debug glue <from:block_pos> <to:block_pos>",
        "create_debug roller <pos:block_pos> <mode:word>",
        "create_debug controls <pos:block_pos> <filter:text>",
        "create_debug carried <pos:block_pos>",
    ] {
        event.commands.push(Command::op(usage, 2));
    }
}

/// Create's contraption debug commands (`glue`, `controls`, `roller`).
pub(super) fn run_command(host: &mut dyn ServerHost, event: &mut RunModCommandEvent<'_>) -> Flow {
    let cmd = event.command;
    let lits: Vec<&str> = cmd.literals.iter().map(String::as_str).collect();
    let sub = match lits.as_slice() {
        ["create_debug", s] if matches!(*s, "glue" | "roller" | "controls" | "carried") => *s,
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
            "carried" => match pos("pos") {
                Some(at) => debug_carried(&mut lv, at),
                None => "usage: create_debug carried <x> <y> <z>".into(),
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
