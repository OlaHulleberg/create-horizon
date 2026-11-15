//! The mechanical crafter (`MechanicalCrafterBlockEntity`, `RecipeGridHandler`,
//! `ConnectedInputHandler`): crafters pointing at each other pass their items along as one
//! grid, the last crafts it and puts the result out.

use super::stacks;
use crate::glue::logistics::{insert_into_from, random_angle, upright};
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::crafter::{self, Phase, Pointing};
use create_engine::kinetics::Dir;
use create_engine::{Block, belt, crafter_input};
use mod_api::{
    BlockClick, BlockFace, BlockSide, Direction, HostTransaction, ITEMS, ItemResource, ItemStack,
    LevelRef, PlayerRef, ResourceHandler, ServerHost, SupportType, Transaction,
};

/// A mechanical crafter's facing and pointing; None for any other block.
pub fn crafter_at(lv: &ServerLevel<'_>, p: [i32; 3]) -> Option<(Dir, Pointing)> {
    let facing = lv.facing_of(p, Block::MechanicalCrafter)?;
    Some((facing, Pointing::read(&lv.property(p, "pointing")?)?))
}

/// `RecipeGridHandler.getTargetingCrafter`: the crafter it points at, facing the same way.
fn crafter_target(lv: &ServerLevel<'_>, p: [i32; 3]) -> Option<[i32; 3]> {
    let (f, pointing) = crafter_at(lv, p)?;
    let t = super::step(p, crafter::target(f, pointing), 1);
    (crafter_at(lv, t)?.0 == f).then_some(t)
}

/// `getPrecedingCrafters`: crafters beside it in its plane, facing the same way, pointing at it.
fn crafters_preceding(lv: &ServerLevel<'_>, p: [i32; 3]) -> Vec<[i32; 3]> {
    let Some((f, pointing)) = crafter_at(lv, p) else {
        return Vec::new();
    };
    let own = crafter::target(f, pointing);
    Dir::ALL
        .into_iter()
        .filter(|d| d.axis() != f.axis() && *d != own)
        .filter_map(|d| {
            let n = super::step(p, d, 1);
            let (nf, np) = crafter_at(lv, n)?;
            (nf == f && crafter::target(nf, np) == d.opposite()).then_some(n)
        })
        .collect()
}

/// `getAllCraftersOfChainIf`: the chain through `root`, following targets and preceding
/// crafters; None for a loop, or (unless `all`) when one of them fails `test`, or when all do.
fn crafter_chain(
    lv: &ServerLevel<'_>,
    root: [i32; 3],
    test: impl Fn(&ServerLevel<'_>, [i32; 3]) -> bool,
    all: bool,
) -> Option<Vec<[i32; 3]>> {
    let mut frontier = vec![(root, None)];
    let mut chain: Vec<[i32; 3]> = Vec::new();
    let (mut empty, mut all_empty) = (false, true);
    while !frontier.is_empty() {
        let (current, last) = frontier.remove(0);
        if chain.contains(&current) {
            return None;
        }
        if test(lv, current) {
            all_empty = false;
        } else {
            empty = true;
        }
        chain.push(current);
        if let Some(t) = crafter_target(lv, current)
            && Some(t) != last
        {
            frontier.push((t, Some(current)));
        }
        for p in crafters_preceding(lv, current) {
            if Some(p) != last {
                frontier.push((p, Some(current)));
            }
        }
    }
    (!(empty && !all || all_empty)).then_some(chain)
}

/// The item in a crafter's slot (`inventory`), if any.
pub fn crafter_slot(lv: &ServerLevel<'_>, p: [i32; 3]) -> Option<String> {
    lv.slot(p, 0).map(|s| stacks::id(&*lv.host, &s))
}

/// Whether a crafter has its slot cover on (`covered`).
pub fn crafter_covered(dim: usize, p: [i32; 3]) -> bool {
    with_world(|w| w.dims[dim].crafter_covers.contains(&p)) == Some(true)
}

/// The crafters whose slots make the inventory of the crafter at `pos`
/// (`ConnectedInput.getInventories`): its group's, in order, or itself alone.
pub fn crafter_group(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Vec<[i32; 3]> {
    let dim = lv.dim;
    let Some((facing, _)) = crafter_at(lv, pos) else {
        return Vec::new();
    };
    let list = with_world(|w| crafter_input::inventories(&w.dims[dim].crafter_inputs, pos, facing))
        .unwrap_or_default();
    list.into_iter()
        .filter(|p| crafter_at(lv, *p).is_some())
        .collect()
}

/// The crafter's item handler (`CombinedInvWrapper` over its group's `Inventory`s): one item
/// into each crafter of the group in turn that is idle with an empty slot; what is left of
/// `stack`. Nothing changes when `simulate`.
pub fn crafter_handler_insert(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    stack: ItemStack,
    simulate: bool,
) -> Option<ItemStack> {
    let dim = lv.dim;
    // The grid keeps items by id: a stack with components stays out.
    if stack.components().is_some() || stack.damage() != 0 {
        return Some(stack);
    }
    let mut left = stack.count();
    for p in crafter_group(lv, pos) {
        if left == 0 {
            break;
        }
        if crafter_covered(dim, p) {
            continue;
        }
        let idle = with_world(|w| w.dims[dim].crafters.get(&p).map(|c| c.phase))
            .flatten()
            .is_none_or(|ph| ph == Phase::Idle);
        if !idle || crafter_slot(lv, p).is_some() {
            continue;
        }
        if !simulate {
            let mut one = stack.clone();
            let _ = one.set_count(1);
            lv.set_slots(p, &[(0, Some(one))]);
            with_world(|w| w.dims[dim].crafter_checks.push(p));
        }
        left -= 1;
    }
    if left == 0 {
        return None;
    }
    let mut rest = stack;
    let _ = rest.set_count(left);
    Some(rest)
}

/// A crafter's `ITEMS` handler (`MechanicalCrafterBlockEntity`'s capability: a
/// `CombinedInvWrapper` over its group's `Inventory`s, which
/// funnels, arms, chutes and hoppers put through): one item into each crafter of the group that
/// is idle, uncovered and empty (`Inventory.insertItem`); nothing comes out. None for any other
/// block of the kinetic type.
pub fn provide_items(
    host: &mut dyn ServerHost,
    side: BlockSide,
) -> Option<Box<dyn ResourceHandler<ItemResource>>> {
    let dimension = side.level.dimension().id(&*host)?;
    let lv = ServerLevel::new(host, &dimension)?;
    crafter_at(&lv, side.pos)?;
    let group = crafter_group(&lv, side.pos);
    Some(Box::new(CrafterItems {
        level: side.level,
        dim: lv.dim,
        group,
    }))
}

struct CrafterItems {
    level: LevelRef,
    dim: usize,
    group: Vec<[i32; 3]>,
}

impl CrafterItems {
    fn held(&self, host: &dyn mod_api::Host, index: usize) -> Option<ItemStack> {
        let p = *self.group.get(index)?;
        host.block_entity(self.level, p)?
            .item_slot(host, 0)
            .ok()
            .flatten()
    }
}

impl ResourceHandler<ItemResource> for CrafterItems {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        self.group.len()
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        self.held(&*host, index).map(|s| ItemResource::of(&s))
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        self.held(&*host, index).map_or(0, |s| s.count())
    }
    /// `Inventory`'s slot limit: one item.
    fn capacity(&self, _host: &mut dyn ServerHost, _index: usize, _: &ItemResource) -> u32 {
        1
    }
    fn insert(
        &self,
        host: &mut dyn ServerHost,
        tx: &Transaction,
        index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        let Some(&p) = self.group.get(index) else {
            return 0;
        };
        // `if (phase != Phase.IDLE) return stack; if (covered) return stack;`
        let idle = with_world(|w| w.dims[self.dim].crafters.get(&p).map(|c| c.phase))
            .flatten()
            .is_none_or(|ph| ph == Phase::Idle);
        if amount == 0
            || !idle
            || crafter_covered(self.dim, p)
            || self.held(&*host, index).is_some()
        {
            return 0;
        }
        // The grid keeps items by id: a stack with components stays out.
        let Some(one) = resource.to_stack(1) else {
            return 0;
        };
        if one.components().is_some() || one.damage() != 0 {
            return 0;
        }
        let Some(e) = host.block_entity(self.level, p) else {
            return 0;
        };
        match e.set_item_slot(host, tx, 0, Some(one)) {
            Ok(_) => 1,
            Err(_) => 0,
        }
    }
    fn extract(
        &self,
        _host: &mut dyn ServerHost,
        _tx: &Transaction,
        _index: usize,
        _resource: &ItemResource,
        _amount: u32,
    ) -> u32 {
        0
    }
}

/// `EdgeInteractionHandler` with the wrench on a crafter (not sneaking): a click at `point` on
/// the strip along an edge of its `face` toward a crafter beside it, facing the same way, joins
/// their inputs or parts them (`toggleConnection`); true when it did.
pub fn crafter_edge(lv: &mut ServerLevel<'_>, pos: [i32; 3], face: Dir, point: [f64; 3]) -> bool {
    let dim = lv.dim;
    let Some((facing, _)) = crafter_at(lv, pos) else {
        return false;
    };
    let sturdy = |lv: &ServerLevel<'_>, p: [i32; 3], side: Dir| {
        lv.level
            .block_face_sturdy(&*lv.host, p, block_face(side), SupportType::Full)
            .unwrap_or(false)
    };
    // `getConnectiveSides`: none when the face is covered.
    if sturdy(lv, super::step(pos, face, 1), face.opposite()) {
        return false;
    }
    for d in Dir::ALL {
        if d.axis() == face.axis() {
            continue;
        }
        let neighbour = super::step(pos, d, 1);
        let front = super::step(neighbour, face, 1);
        if sturdy(lv, front, face.opposite()) {
            continue;
        }
        let other = crafter_at(lv, neighbour).map(|c| c.0);
        if !crafter_input::should_connect(facing, face, d, other) {
            continue;
        }
        let (min, max) = crafter_input::edge_box(pos, d);
        if (0..3).all(|k| point[k] >= min[k] && point[k] <= max[k]) {
            with_world(|w| {
                crafter_input::toggle(&mut w.dims[dim].crafter_inputs, pos, neighbour);
                // One alone needs no entry.
                w.dims[dim]
                    .crafter_inputs
                    .retain(|_, i| *i != crafter_input::Input::default());
                w.mark_unsaved(dim, pos);
            });
            return true;
        }
    }
    false
}

fn block_face(d: Dir) -> BlockFace {
    match d {
        Dir::Down => BlockFace::Down,
        Dir::Up => BlockFace::Up,
        Dir::North => BlockFace::North,
        Dir::South => BlockFace::South,
        Dir::West => BlockFace::West,
        Dir::East => BlockFace::East,
    }
}

fn direction(d: Dir) -> Direction {
    Direction::ALL[d as usize]
}

/// `MechanicalCrafterBlock.useItemOn` on its front: at work, it throws its grid out; idle, a
/// slot cover goes on an empty uncovered slot, anything else into its inventory (its group's,
/// as much as it takes); an empty hand takes the slot's item back, or the cover off an empty
/// one. True when the click was the crafter's.
pub fn use_crafter(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    click: &BlockClick<'_>,
    player: PlayerRef,
    main: bool,
) -> bool {
    let dim = lv.dim;
    let Some((facing, _)) = crafter_at(lv, pos) else {
        return false;
    };
    if click.hit.face != facing.offset() || !main {
        return false;
    }
    // Its slot or cover may change: its clients are told.
    SHOW.with_borrow_mut(|s| s.push((dim, pos)));
    let held = click.stack.cloned();
    let held_id = held.as_ref().map(|h| stacks::id(&*lv.host, h));
    if held_id.as_deref() == Some("create:mechanical_arm") {
        return false;
    }
    let wrenched = held_id.as_deref() == Some("create:wrench");
    let is_hand = held.is_none();
    let idle = with_world(|w| w.dims[dim].crafters.get(&pos).map(|c| c.phase))
        .flatten()
        .is_none_or(|ph| ph == Phase::Idle);
    if !idle && !wrenched {
        crafters_eject(lv, pos);
        return true;
    }
    let covered = crafter_covered(dim, pos);
    let slot = crafter_slot(lv, pos);
    if idle && !is_hand && !wrenched {
        let Some(h) = held else {
            return false;
        };
        if held_id.as_deref() == Some("create:crafter_slot_cover") {
            if covered || slot.is_some() {
                return false;
            }
            with_world(|w| {
                w.dims[dim].crafter_covers.insert(pos);
                w.mark_unsaved(dim, pos);
            });
            if !lv.creative(player) {
                lv.take_held(player, 1);
            }
            return true;
        }
        let count = h.count();
        let left = crafter_handler_insert(lv, pos, h, false).map_or(0, |l| l.count());
        lv.take_held(player, count - left);
        return true;
    }
    let Some(item) = slot else {
        if covered && !wrenched {
            with_world(|w| {
                w.dims[dim].crafter_covers.remove(&pos);
                w.mark_unsaved(dim, pos);
            });
            if !lv.creative(player)
                && let Some(cover) = stacks::stack(&*lv.host, "create:crafter_slot_cover", 1)
            {
                lv.give(player, cover);
            }
            return true;
        }
        return false;
    };
    if !is_hand && held_id.as_deref() != Some(item.as_str()) {
        return false;
    }
    if let Some(taken) = lv.slot(pos, 0)
        && lv.set_slots(pos, &[(0, None)])
    {
        let mut one = taken;
        let _ = one.set_count(1);
        lv.give(player, one);
    }
    true
}

/// `RecipeGridHandler.tryToApplyRecipe`: a crafting recipe (the grid in a square of its larger
/// side, up to 3), else a `create:mechanical_crafting` one.
fn crafter_recipe(lv: &ServerLevel<'_>, grid: &[((i32, i32), String)]) -> Option<(String, u8)> {
    let (w, h, cells) = crafter::input(grid);
    let size = w.max(h);
    if (1..=3).contains(&size) {
        let mut square: Vec<Option<String>> = vec![None; size * size];
        for r in 0..h {
            for c in 0..w {
                square[r * size + c] = cells[r * w + c].clone();
            }
        }
        if let Some(out) = super::recipes::craft(lv, &square, size) {
            return Some(out);
        }
    }
    super::recipes::with(|r| {
        r.mechanical_crafting
            .iter()
            .find(|s| s.matches(w, h, &cells))
            .map(|s| s.result.clone())
    })
    .flatten()
}

/// `ejectWholeGrid`: every crafter of the chain throws out its grid in front of it and goes
/// idle, then looks at its slot again (`lazyTick`). Its slot is left: Create's refuses items
/// while it works, so an item a hopper put in meanwhile is the next round's.
fn crafters_eject(lv: &mut ServerLevel<'_>, root: [i32; 3]) {
    let dim = lv.dim;
    let chain = crafter_chain(lv, root, |_, _| true, true).unwrap_or_else(|| vec![root]);
    for p in chain {
        let grid = with_world(|w| {
            let d = &mut w.dims[dim];
            d.crafting.retain(|q| *q != p);
            d.crafters.get_mut(&p).map(|c| {
                c.phase = Phase::Idle;
                std::mem::take(&mut c.grid)
            })
        })
        .flatten()
        .unwrap_or_default();
        with_world(|w| w.dims[dim].crafter_checks.push(p));
        let f = crafter_at(lv, p).map_or([0, 0, 0], |(f, _)| f.offset());
        let at: [f64; 3] = std::array::from_fn(|i| f64::from(p[i]) + 0.5 + f64::from(f[i]) * 0.75);
        for (_, item) in grid {
            lv.spawn_items(at, &item, 1);
        }
    }
}

/// `tryInsert`: the result into the container it points at (as a hopper puts it in); without
/// one, everything is thrown out.
fn crafter_insert(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some((facing, pointing)) = crafter_at(lv, pos) else {
        return;
    };
    let dir = crafter::target(facing, pointing);
    let t = super::step(pos, dir, 1);
    // `isTargetingBelt`: a belt or depot there takes the results from the side
    // (`DirectBeltInputBehaviour.handleInsertion`); else `inserting.hasInventory()`.
    let belt_input = super::direct_input(lv, t, dir);
    let face = direction(dir.opposite());
    let has_inventory = !belt_input
        && lv
            .level
            .capability(&mut *lv.host, &ITEMS, t, Some(face))
            .is_some();
    if !belt_input && !has_inventory {
        crafters_eject(lv, pos);
        return;
    }
    let grid = with_world(|w| w.dims[dim].crafters.get(&pos).map(|c| c.grid.clone()))
        .flatten()
        .unwrap_or_default();
    // Each stack of the grid in turn, whole (the result is one stack of its count).
    let mut stacked: Vec<(String, Vec<(i32, i32)>)> = Vec::new();
    for (at, item) in grid {
        match stacked.iter_mut().find(|(i, _)| *i == item) {
            Some((_, cells)) => cells.push(at),
            None => stacked.push((item, vec![at])),
        }
    }
    let mut left = Vec::new();
    for (item, cells) in stacked {
        let n = cells.len().min(64);
        let kept = if belt_input {
            let angle = random_angle(lv, pos);
            let one = belt::Item::new(&item, n as u8, angle, upright(&*lv.host, &item));
            insert_into_from(lv, t, dir, &one, false).map_or(n, usize::from)
        } else {
            n - insert_item(lv, t, face, &item, n as u32) as usize
        };
        let kept = kept + cells.len() - n;
        left.extend(cells.into_iter().take(kept).map(|at| (at, item.clone())));
    }
    let done = left.is_empty();
    with_world(|w| {
        if let Some(c) = w.dims[dim].crafters.get_mut(&pos) {
            c.phase = Phase::Inserting;
            c.grid = left;
        }
    });
    // All of it out: the whole chain goes idle (`ejectWholeGrid` of empty grids).
    if done {
        crafters_eject(lv, pos);
    }
}

/// `n` of `item` put into the container at `t` through `face`, as a hopper would: how many went.
fn insert_item(lv: &mut ServerLevel<'_>, t: [i32; 3], face: Direction, item: &str, n: u32) -> u32 {
    let Some(stack) = stacks::stack(&*lv.host, item, n) else {
        return 0;
    };
    let resource = ItemResource::of(&stack);
    let level = lv.level;
    lv.host
        .transaction(|host, tx| {
            let Some(cap) = level.capability(host, &ITEMS, t, Some(face)) else {
                return Err(());
            };
            Ok::<u32, ()>(cap.insert_any(host, tx, &resource, n))
        })
        .unwrap_or(0)
}

/// A crafter's grid: each cell's place and item.
type Grid = Vec<((i32, i32), String)>;
/// A phase as told: the phase, the game time it began at and its `countDown` then.
type Told = (Phase, i64, i32);

/// The key of a crafter's block entity data its clients draw it from: `P`, its phase; `C`,
/// whether it is covered; `I`, the item in its slot.
pub const CRAFTER_KEY: &str = "Crafter";

thread_local! {
    /// The grid each crafting crafter held before it crafted (`groupedItemsBeforeCraft`).
    static BEFORE_CRAFT: std::cell::RefCell<std::collections::HashMap<(usize, [i32; 3]), Grid>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    /// Each crafter's phase as its clients were told, with the game time and `countDown` it
    /// began at, from which they run its countdown on.
    static TOLD: std::cell::RefCell<std::collections::HashMap<(usize, [i32; 3]), Told>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    /// Crafters a player clicked, by dimension: their slot or cover may have changed.
    static SHOW: std::cell::RefCell<Vec<(usize, [i32; 3])>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Forgets what was kept for the world.
pub fn clear() {
    BEFORE_CRAFT.with_borrow_mut(std::collections::HashMap::clear);
    TOLD.with_borrow_mut(std::collections::HashMap::clear);
}

/// Mechanical crafters (`run_crafters`), then those looked at or at work tell their clients
/// what they show (`CRAFTER_KEY`).
pub fn tick_crafters(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut touched = with_world(|w| {
        let d = &w.dims[dim];
        if d.crafter_checks.is_empty() && d.crafting.is_empty() {
            Vec::new()
        } else {
            d.crafter_checks
                .iter()
                .chain(&d.crafting)
                .copied()
                .collect()
        }
    })
    .unwrap_or_default();
    SHOW.with_borrow_mut(|s| {
        s.retain(|&(d, p)| {
            if d == dim {
                touched.push(p);
            }
            d != dim
        })
    });
    run_crafters(lv);
    if let Some(now) = with_world(|w| w.dims[dim].crafting.clone()) {
        touched.extend(now);
    }
    touched.sort();
    touched.dedup();
    for pos in touched {
        if crafter_at(lv, pos).is_none() {
            continue;
        }
        let crafter = with_world(|w| w.dims[dim].crafters.get(&pos).cloned())
            .flatten()
            .unwrap_or_default();
        let phase = crafter.phase;
        let now = lv.game_time();
        let (_, began, count_down) = TOLD.with_borrow_mut(|t| {
            let e = t
                .entry((dim, pos))
                .or_insert((phase, now, crafter.count_down));
            if e.0 != phase {
                *e = (phase, now, crafter.count_down);
            }
            *e
        });
        if phase != Phase::Crafting {
            BEFORE_CRAFT.with_borrow_mut(|b| b.remove(&(dim, pos)));
        }
        let grid_tag = |grid: &[((i32, i32), String)]| {
            mod_api::Tag::List(
                grid.iter()
                    .map(|((x, y), item)| {
                        mod_api::Tag::Compound(mod_api::Compound::from([
                            ("x".into(), mod_api::Tag::Int(*x)),
                            ("y".into(), mod_api::Tag::Int(*y)),
                            ("id".into(), mod_api::Tag::String(item.as_str().into())),
                        ]))
                    })
                    .collect(),
            )
        };
        let before = BEFORE_CRAFT
            .with_borrow(|b| b.get(&(dim, pos)).cloned())
            .unwrap_or_default();
        let mut c = mod_api::Compound::from([
            ("G".into(), grid_tag(&crafter.grid)),
            ("B".into(), grid_tag(&before)),
            ("S".into(), mod_api::Tag::Long(began)),
            ("D".into(), mod_api::Tag::Int(count_down)),
            ("P".into(), mod_api::Tag::Int(phase as i32)),
            (
                "C".into(),
                mod_api::Tag::Byte(i8::from(crafter_covered(dim, pos))),
            ),
        ]);
        if let Some(item) = crafter_slot(lv, pos) {
            c.insert("I".into(), mod_api::Tag::String(item.as_str().into()));
        }
        super::basin::set_shown(lv, pos, &[(CRAFTER_KEY, Some(mod_api::Tag::Compound(c)))]);
    }
}

/// Mechanical crafters: those looked at start their chain when every one holds an item
/// (`checkCompletedRecipe`, `begin`); those at work run `tick`'s phases: the grid assembles,
/// is passed on to the crafter pointed at (`EXPORTING`, 1000), the last one crafts (2000) and
/// puts the result out. Nothing runs without one of them.
fn run_crafters(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let Some((mut checks, drops, working, power)) = with_world(|w| {
        let d = &mut w.dims[dim];
        (
            std::mem::take(&mut d.crafter_checks),
            std::mem::take(&mut d.crafter_drops),
            d.crafting.clone(),
            std::mem::take(&mut d.crafter_power_checks),
        )
    }) else {
        return;
    };
    // `wasPoweredBefore`: a crafter newly powered starts its chain with what it has
    // (`checkCompletedRecipe(true)`: gaps are empty cells; covers don't count).
    let mut powered_starts = Vec::new();
    for pos in power {
        if crafter_at(lv, pos).is_none() {
            continue;
        }
        let powered = lv.has_neighbor_signal(pos);
        let rose = with_world(|w| {
            let d = &mut w.dims[dim];
            if powered {
                d.crafter_unpowered.remove(&pos)
            } else {
                d.crafter_unpowered.insert(pos);
                false
            }
        }) == Some(true);
        if rose {
            powered_starts.push(pos);
        }
    }
    for (p, item) in drops {
        lv.spawn_items(p.map(|v| f64::from(v) + 0.5), &item, 1);
    }
    if checks.is_empty() && working.is_empty() && powered_starts.is_empty() {
        return;
    }
    checks.sort();
    checks.dedup();
    let starts: Vec<([i32; 3], bool)> = powered_starts
        .into_iter()
        .map(|p| (p, true))
        .chain(checks.into_iter().map(|p| (p, false)))
        .collect();
    for (pos, powered_start) in starts {
        let idle = with_world(|w| w.dims[dim].crafters.get(&pos).map(|c| c.phase)).flatten()
            == Some(Phase::Idle);
        let rpm = lv.speed(pos);
        // `hasItemOrCover`.
        let filled = |lv: &ServerLevel<'_>, p: [i32; 3]| {
            crafter_slot(lv, p).is_some() || !powered_start && crafter_covered(dim, p)
        };
        if !idle || rpm == 0.0 || !powered_start && !filled(lv, pos) {
            continue;
        }
        let Some(chain) = crafter_chain(lv, pos, filled, powered_start) else {
            continue;
        };
        for p in chain {
            let item = crafter_slot(lv, p);
            if item.is_some() {
                lv.set_slots(p, &[(0, None)]);
            }
            let first = crafters_preceding(lv, p).is_empty();
            with_world(|w| {
                let d = &mut w.dims[dim];
                let c = d.crafters.entry(p).or_default();
                c.grid = item.into_iter().map(|i| ((0, 0), i)).collect();
                c.phase = if first {
                    Phase::Assembling
                } else {
                    Phase::Accepting
                };
                c.count_down = 1;
                if !d.crafting.contains(&p) {
                    d.crafting.push(p);
                }
            });
        }
    }
    let working = with_world(|w| w.dims[dim].crafting.clone()).unwrap_or_default();
    for pos in working {
        let speed = crafter::count_down_speed(lv.speed(pos));
        let Some(c) = with_world(|w| w.dims[dim].crafters.get(&pos).cloned()).flatten() else {
            continue;
        };
        let mut cd = c.count_down;
        match c.phase {
            Phase::Assembling | Phase::Exporting | Phase::Crafting => {
                cd -= speed;
                with_world(|w| {
                    if let Some(x) = w.dims[dim].crafters.get_mut(&pos) {
                        x.count_down = cd.max(0);
                    }
                });
                if cd >= 0 {
                    continue;
                }
            }
            Phase::Inserting => {
                crafter_insert(lv, pos);
                continue;
            }
            _ => continue,
        }
        match c.phase {
            Phase::Assembling => {
                if crafter_target(lv, pos).is_some() {
                    let empty = c.grid.is_empty();
                    with_world(|w| {
                        if let Some(x) = w.dims[dim].crafters.get_mut(&pos) {
                            x.phase = Phase::Exporting;
                            x.count_down = if empty { 0 } else { 1000 };
                        }
                    });
                    continue;
                }
                // What the ingredients leave (`getCraftingRemainingItem`), after the result.
                let host = &*lv.host;
                let containers: Vec<String> = c
                    .grid
                    .iter()
                    .filter_map(|(_, i)| {
                        let s = stacks::stack(host, i, 1)?;
                        host.item_crafting_remainder(&s)
                            .map(|r| stacks::id(host, &r))
                    })
                    .collect();
                let found = crafter_recipe(lv, &c.grid);
                match found {
                    Some((item, n)) => {
                        // `groupedItemsBeforeCraft`, which its clients draw shrinking away.
                        BEFORE_CRAFT.with_borrow_mut(|b| b.insert((dim, pos), c.grid.clone()));
                        with_world(|w| {
                            if let Some(x) = w.dims[dim].crafters.get_mut(&pos) {
                                x.grid =
                                    (0..n).map(|i| ((i32::from(i), 0), item.clone())).collect();
                                x.grid.extend(
                                    containers
                                        .iter()
                                        .enumerate()
                                        .map(|(i, r)| ((i32::from(n) + i as i32, 0), r.clone())),
                                );
                                x.phase = Phase::Crafting;
                                x.count_down = 2000;
                            }
                        });
                    }
                    None => crafters_eject(lv, pos),
                }
            }
            Phase::Exporting => {
                let Some(t) = crafter_target(lv, pos) else {
                    crafters_eject(lv, pos);
                    continue;
                };
                let Some((_, pointing)) = crafter_at(lv, pos) else {
                    continue;
                };
                let (ox, oy) = pointing.merge_offset();
                with_world(|w| {
                    let d = &mut w.dims[dim];
                    let grid = d.crafters.get_mut(&pos).map(|x| {
                        x.phase = Phase::Waiting;
                        x.count_down = 0;
                        std::mem::take(&mut x.grid)
                    });
                    if let Some(tc) = d.crafters.get_mut(&t) {
                        tc.grid.extend(
                            grid.unwrap_or_default()
                                .into_iter()
                                .map(|((x, y), i)| ((x + ox, y + oy), i)),
                        );
                    }
                });
                // `continueIfAllPrecedingFinished` on the target.
                let preceding = crafters_preceding(lv, t);
                let all = with_world(|w| {
                    preceding.iter().all(|p| {
                        w.dims[dim].crafters.get(p).map(|c| c.phase) == Some(Phase::Waiting)
                    })
                })
                .unwrap_or(false);
                if all {
                    with_world(|w| {
                        let d = &mut w.dims[dim];
                        // They wait (taking nothing in) until the chain is done.
                        for p in &preceding {
                            d.crafting.retain(|q| q != p);
                        }
                        if let Some(x) = d.crafters.get_mut(&t) {
                            x.phase = Phase::Assembling;
                            x.count_down = 1;
                        }
                    });
                }
            }
            Phase::Crafting => crafter_insert(lv, pos),
            _ => {}
        }
    }
}
