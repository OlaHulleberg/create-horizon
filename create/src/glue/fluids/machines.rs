//! Spouts and item drains, and the `create:filling` / `create:emptying` recipes they run (read
//! with processing's recipes, from the same store). A sequenced assembly's filling
//! step comes first (`SequencedAssemblyRecipe.getRecipe`): the assemblies and the progress on a
//! stack are processing's.

use super::super::processing::{self, recipes};
use super::super::redstone::analog_touched;
use super::super::server::ServerLevel;
use super::super::world::with_world;
use create_engine::spout::{self, Filling};
use create_engine::tank::Fluid;
use create_engine::{belt, sequenced};
use mod_api::{Compound, HostTransaction, ItemStack, ServerHost, Tag};

/// `count` of `item`; a potion named with its contents (`tank::potion_item`, as a filled water
/// bottle, `spout::WATER_BOTTLE`) holds them (`PotionContents` of its potion), and a stack a
/// belt named with its components (a package carried across a drain) keeps them.
pub(super) fn stack_of(host: &dyn ServerHost, item: &str, count: u32) -> Option<ItemStack> {
    crate::glue::logistics::items::stack_of(host, item, count)
}

/// The item a stack is to spouts, drains and tanks: its id, and a potion with its whole
/// contents (`tank::potion_item`; its other components, a name or enchantments, aside, as
/// `PotionFluidHandler.getFluidFromPotionItem` reads only the contents).
pub(super) fn item_name(host: &dyn mod_api::Host, stack: &ItemStack) -> String {
    crate::glue::logistics::items::potion_name(host, stack).unwrap_or_else(|| stack.item().id(host))
}

/// A sequenced assembly's filling step for `item` at `progress`: the assembly, the step's
/// fluid and amount.
fn sequenced_filling(
    item: &str,
    progress: Option<&sequenced::Progress>,
) -> Option<(sequenced::Assembly, Fluid, u32)> {
    let (a, step) = processing::sequenced_for(item, progress, "create:filling", None)?;
    let (fluid, amount) = step.fluid?;
    Some((a, Fluid::of_id(&fluid)?, amount))
}

/// The step as the one filling recipe the spout may run: `item` with the step's fluid becomes
/// the assembly's transitional item.
fn step_filling(item: &str, a: &sequenced::Assembly, fluid: Fluid, amount: u32) -> [Filling; 1] {
    [Filling {
        item: item.to_string(),
        fluid,
        amount,
        result: a.transitional.clone(),
    }]
}

/// Spouts holding fluid (`SpoutBlockEntity.whenItemHeld` on the depot two below, `tick`): the
/// held item is filled after the 20-tick cycle (at its 5th-last tick); a sequenced assembly's
/// filling step first, when the spout holds that step's fluid, the step done as processing does
/// it on a depot.
pub(crate) fn spout_on_depot(lv: &mut ServerLevel<'_>, depot: [i32; 3]) {
    let dim = lv.dim;
    let pos = [depot[0], depot[1] + 2, depot[2]];
    let Some(mut s) = with_world(|w| w.dims[dim].spouts.get(&pos).copied()).flatten() else {
        return;
    };
    let Some(mut held_state) =
        with_world(|w| w.dims[dim].depots.get(&depot).copied().flatten()).flatten()
    else {
        return;
    };
    // DepotBehaviour.tick tests arrival before dispatching received/held callbacks.
    if !recipes::ready() || (!held_state.locked && lv.blocked([depot[0], depot[1] + 1, depot[2]])) {
        return;
    }
    let before = s.fluid;
    let was_locked = held_state.locked;
    let entity = lv.host.block_entity(lv.level, depot);
    // The depot's held stack (slot 0).
    let held = entity
        .and_then(|e| e.item_slot(&*lv.host, 0).ok().flatten())
        .map(|stack| (item_name(&*lv.host, &stack), stack.count()));
    let sequenced = held
        .as_ref()
        .and_then(|_| processing::sequenced_on_depot(lv, depot, "create:filling", None))
        .and_then(|(a, progress, step)| {
            let (fluid, amount) = step.fluid?;
            let fluid = Fluid::of_id(&fluid)?;
            (s.fluid.map(|f| f.0) == Some(fluid)).then_some((a, progress, fluid, amount))
        });
    let filled = recipes::with(|r| {
        let item = held.as_ref().map(|h| h.0.as_str());
        match &sequenced {
            Some((a, _, fluid, amount)) => {
                let step = step_filling(item?, a, *fluid, *amount);
                item.map(|item| spout::on_depot(&mut s, &mut held_state, item, &step))
            }
            None => item.map(|item| spout::on_depot(&mut s, &mut held_state, item, &r.filling)),
        }
    })
    .flatten();
    let locked = held_state.locked;
    let filled = match filled {
        Some(spout::OnBelt::Fill(out)) => Some(out),
        _ => None,
    };
    if let (Some(out), Some(_)) = (&filled, &held) {
        crate::glue::advancements::spouted(lv, pos, out);
    }
    match (filled, &sequenced, held, entity) {
        (Some(_), Some((a, progress, _, _)), Some((_, count)), _) => {
            let state = with_world(|w| w.dims[dim].depots.get(&depot).copied().flatten()).flatten();
            if let Some(state) = state {
                let count = count.min(255) as u8;
                let next = processing::sequenced_step_on_depot(
                    lv,
                    depot,
                    count,
                    state,
                    a,
                    progress.as_ref(),
                );
                with_world(|w| {
                    if let Some(h) = w.dims[dim].depots.get_mut(&depot) {
                        *h = Some(next);
                    }
                });
            }
        }
        (Some(out), None, Some((item, count)), Some(entity)) => {
            if let Some(over) = fill_held(lv.host, entity, &item, count, &out) {
                // `applyToAllItems`: what the outputs can't take is dropped on the depot.
                let c = depot.map(f64::from);
                let _ = lv.level.spawn_item(
                    &mut *lv.host,
                    c[0] + 0.5,
                    c[1] + 1.0,
                    c[2] + 0.5,
                    over,
                    10,
                );
            }
            with_world(|w| w.depot_changed(dim, depot));
        }
        _ => {}
    }
    with_world(|w| {
        if let Some(Some(h)) = w.dims[dim].depots.get_mut(&depot) {
            h.locked = locked;
        }
        if locked != was_locked {
            w.depot_changed(dim, depot);
        }
    });
    with_world(|w| w.dims[dim].spouts.insert(pos, s));
    if s.fluid != before {
        analog_touched(dim, pos);
    }
}

/// The spout's belt callbacks (`SpoutBlockEntity`'s `BeltProcessingBehaviour`) for a stack on
/// the belt segment two under the spout `above`: None without a spout there; held while it can
/// be filled, one of it filled at the cycle's end (a sequenced assembly's filling step first,
/// with that step's fluid).
pub(super) fn spout_on_belt(
    lv: &mut ServerLevel<'_>,
    above: [i32; 3],
    item: &mut belt::Item,
    locked: bool,
) -> Option<belt::Process> {
    use belt::Process;
    use spout::OnBelt;
    let dim = lv.dim;
    let mut s = with_world(|w| w.dims[dim].spouts.get(&above).copied()).flatten()?;
    if !recipes::ready() {
        return Some(Process::Pass);
    }
    let sequenced = sequenced_filling(&item.item, item.progress.as_ref());
    let before = s.fluid;
    let result = recipes::with(|r| match &sequenced {
        Some((a, fluid, amount)) => {
            let step = step_filling(&item.item, a, *fluid, *amount);
            spout::on_belt(&mut s, &item.item, &step, locked)
        }
        None => spout::on_belt(&mut s, &item.item, &r.filling, locked),
    })?;
    with_world(|w| w.dims[dim].spouts.insert(above, s));
    if s.fluid != before {
        analog_touched(dim, above);
    }
    Some(match result {
        OnBelt::Pass => Process::Pass,
        OnBelt::Hold => Process::Hold,
        OnBelt::Fill(out) => {
            crate::glue::advancements::spouted(lv, above, &out);
            let outputs = match &sequenced {
                Some((a, _, _)) => {
                    let at = [above[0], above[1] - 2, above[2]];
                    processing::sequenced_on_belt(lv, at, item, a)
                }
                None => {
                    let mut o = item.processing_copy();
                    o.item = out;
                    o.count = 1;
                    o.progress = None;
                    vec![o]
                }
            };
            let held = processing::belt_left(item).map(|h| h.processing_copy());
            Process::Convert { held, outputs }
        }
    })
}

/// `convertToAndLeaveHeld`: one of the `count` held `item`s becomes `out`; the rest stays held,
/// the filled one goes to the outputs as the depot stacks them (`insertItemStacked`). The filled
/// stack when the outputs are full, for the caller to drop.
fn fill_held(
    host: &mut dyn ServerHost,
    entity: mod_api::BlockEntityRef,
    item: &str,
    count: u32,
    out: &str,
) -> Option<ItemStack> {
    let filled = stack_of(&*host, out, 1)?;
    let rest = if count > 1 {
        stack_of(&*host, item, count - 1)
    } else {
        None
    };
    let mut over = None;
    let _ = host.transaction(|host, tx| {
        match rest {
            None => {
                entity
                    .set_item_slot(host, tx, 0, Some(filled.clone()))
                    .map_err(|_| ())?;
            }
            Some(rest) => {
                entity
                    .set_item_slot(host, tx, 0, Some(rest))
                    .map_err(|_| ())?;
                let outputs = create_engine::depot::OUTPUTS;
                let slots: Vec<(u32, Option<ItemStack>)> = outputs
                    .map(|i| (i as u32, entity.item_slot(&*host, i as u32).ok().flatten()))
                    .collect();
                let stacked = slots.iter().find(|(_, s)| {
                    s.as_ref().is_some_and(|s| {
                        let max = host.max_stack_size(s.item()).unwrap_or(64);
                        s.is_stackable_with(&filled) && s.count() < max
                    })
                });
                if let Some((i, Some(s))) = stacked {
                    let mut s = s.clone();
                    s.grow(1).map_err(|_| ())?;
                    entity
                        .set_item_slot(host, tx, *i, Some(s))
                        .map_err(|_| ())?;
                } else if let Some((i, _)) = slots.iter().find(|(_, s)| s.is_none()) {
                    entity
                        .set_item_slot(host, tx, *i, Some(filled.clone()))
                        .map_err(|_| ())?;
                } else {
                    over = Some(filled.clone());
                }
            }
        }
        Ok::<(), ()>(())
    });
    over
}

/// Item drains holding a stack (`ItemDrainBlockEntity.tick`): it moves across, is emptied in
/// the middle, and at the far side is handed on to a belt, depot or drain there
/// (`DirectBeltInputBehaviour`), or thrown off when nothing solid is in its way; else it waits.
pub(crate) fn tick_drain(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    use create_engine::drain::Tick;
    let dim = lv.dim;
    let emptying = recipes::with(|r| r.emptying.clone()).unwrap_or_default();
    let Some(t) = with_world(|w| {
        let d = w.dims[dim].drains.get_mut(&pos)?;
        let t = create_engine::drain::tick(d, &emptying);
        if t == Tick::Emptied {
            w.dims[dim].analog_checks.push(pos);
            w.mark_unsaved(dim, pos);
        }
        Some(t)
    })
    .flatten() else {
        return;
    };
    if t == Tick::Emptied {
        crate::glue::advancements::award_placer(lv, pos, "drain");
    }
    let Tick::Out(side) = t else {
        return;
    };
    let Some(held) =
        with_world(|w| w.dims[dim].drains.get(&pos).and_then(|d| d.held.clone())).flatten()
    else {
        return;
    };
    // `tryExportingToBeltFunnel`: a pulling belt funnel on top facing against the item's way
    // takes it first; one that takes none of it holds it.
    if let Some(stack) = stack_of(&*lv.host, &held.item, u32::from(held.count))
        && let Some(left) = crate::glue::logistics::export_up(lv, pos, side.opposite(), &stack)
    {
        let left = left.min(u32::from(held.count)) as u8;
        if left != held.count {
            with_world(|w| {
                if let Some(d) = w.dims[dim].drains.get_mut(&pos) {
                    match d.held.as_mut() {
                        Some(h) if left > 0 => h.count = left,
                        _ => d.held = None,
                    }
                }
                w.mark_unsaved(dim, pos);
            });
        }
        return;
    }
    let next = super::offset(pos, side);
    let left = match insert_into_drain(lv, next, side, &held.item, held.count) {
        Some(left) => {
            if left == 0 {
                crate::glue::advancements::award_placer(lv, pos, "chained_drain");
            }
            Some(left)
        }
        None => {
            let item = create_engine::belt::Item::new(&held.item, held.count, 0, false);
            crate::glue::logistics::insert_into_from(lv, next, side, &item, false)
        }
    };
    let left = match left {
        Some(left) => left,
        // No input there: thrown off over an open side, else it waits.
        None if !crate::glue::logistics::has_collision(lv, next) => {
            if let Some(stack) = stack_of(&*lv.host, &held.item, u32::from(held.count)) {
                let o = side.offset().map(f64::from);
                let at = [
                    f64::from(pos[0]) + 0.5 + o[0] * 0.75,
                    f64::from(pos[1]) + 0.5 + o[1] * 0.75 + 6.0 / 16.0,
                    f64::from(pos[2]) + 0.5 + o[2] * 0.75,
                ];
                let motion = [o[0] / 8.0, o[1] / 8.0 + 1.0 / 8.0, o[2] / 8.0];
                let _ = lv.level.spawn_item_with_motion(
                    &mut *lv.host,
                    at[0],
                    at[1],
                    at[2],
                    stack,
                    motion,
                    10,
                );
            }
            0
        }
        None => return,
    };
    if left != held.count {
        with_world(|w| {
            if let Some(d) = w.dims[dim].drains.get_mut(&pos) {
                match d.held.as_mut() {
                    Some(h) if left > 0 => h.count = left,
                    _ => d.held = None,
                }
            }
            w.mark_unsaved(dim, pos);
        });
    }
}

/// `insert_into_drain`, or (`simulate`) what it would leave without taking any.
pub(crate) fn insert_into_drain_sim(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    side: create_engine::kinetics::Dir,
    item: &str,
    count: u8,
    simulate: bool,
) -> Option<u8> {
    if !simulate {
        return insert_into_drain(lv, pos, side, item, count);
    }
    let dim = lv.dim;
    let emptying = recipes::with(|r| r.emptying.clone()).unwrap_or_default();
    let mut d = with_world(|w| w.dims[dim].drains.get(&pos).cloned()).flatten()?;
    let took = create_engine::drain::insert(&mut d, item, count, side, &emptying);
    Some(count - took)
}

/// `count` of `item` coming into the drain at `pos` travelling toward `side`
/// (`tryInsertingFromSide`): how many are left over. None when no drain is there.
pub(crate) fn insert_into_drain(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    side: create_engine::kinetics::Dir,
    item: &str,
    count: u8,
) -> Option<u8> {
    let dim = lv.dim;
    let emptying = recipes::with(|r| r.emptying.clone()).unwrap_or_default();
    with_world(|w| {
        let d = w.dims[dim].drains.get_mut(&pos)?;
        let took = create_engine::drain::insert(d, item, count, side, &emptying);
        if took > 0 {
            w.mark_unsaved(dim, pos);
        }
        Some(count - took)
    })
    .flatten()
}

/// `ItemDrainBlock.updateEntityAfterFallOn`: a stack landing on a drain goes in travelling the
/// way the item moved, north for one that fell straight (`Direction.getNearest` of no motion,
/// which is what the landing event gives); what it can't take stays an item.
pub(super) fn drain_landed(lv: &mut ServerLevel<'_>, pos: [i32; 3], stack: &mut Option<ItemStack>) {
    let Some(s) = stack.as_ref() else {
        return;
    };
    // Any stack, named with its components as the belts name it (a potion with its contents).
    let id = crate::glue::logistics::items::key_of(&*lv.host, s);
    let side = create_engine::kinetics::Dir::North;
    let count = s.count().min(64) as u8;
    let Some(left) = insert_into_drain(lv, pos, side, &id, count) else {
        return;
    };
    if left != count {
        *stack = s.clone().shrink(u32::from(count - left));
    }
}

/// `ItemDrainBlock.useItemOn` with nothing to empty: the drain's held stack goes back to the
/// player (`placeItemBackInInventory`).
pub(super) fn give_back_drained(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: mod_api::PlayerRef,
) {
    let dim = lv.dim;
    let Some(held) =
        with_world(|w| w.dims[dim].drains.get_mut(&pos).and_then(|d| d.held.take())).flatten()
    else {
        return;
    };
    with_world(|w| w.mark_unsaved(dim, pos));
    if let Some(stack) = stack_of(&*lv.host, &held.item, u32::from(held.count)) {
        super::super::items::give_back(lv.host, player, stack);
    }
}

/// Spouts' and drains' tanks and drains' stacks saved with a column, under `create_machines`
/// (`SmartFluidTankBehaviour`'s and `ItemDrainBlockEntity`'s `HeldItem`, `ProcessingTicks`):
/// `spouts` and `drains` hold x, y, z, fluid (255: none), amount each; `drain_items` a compound
/// per held stack.
const KEY: &str = "create_machines";

pub(super) fn chunk_data_save(dim: usize, chunk: [i32; 2], data: &mut Compound) {
    let here = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]);
    let codes = std::cell::RefCell::new(create_engine::tank::Codes::default());
    let tank = |p: &[i32; 3], f: Option<(Fluid, u32)>| {
        let n = f.map_or(0, |(_, n)| n as i32);
        let fluid = codes.borrow_mut().code(f.map(|f| f.0));
        [p[0], p[1], p[2], fluid, n]
    };
    let Some((spouts, drains, items)) = with_world(|w| {
        let d = &w.dims[dim];
        let mut spouts: Vec<_> = d.spouts.iter().filter(|(p, _)| here(p)).collect();
        spouts.sort_by_key(|e| e.0);
        let mut drains: Vec<_> = d.drains.iter().filter(|(p, _)| here(p)).collect();
        drains.sort_by_key(|e| e.0);
        let items: Vec<Tag> = drains
            .iter()
            .filter_map(|(p, dr)| {
                let h = dr.held.as_ref()?;
                Some(Tag::Compound(Compound::from([
                    ("Pos".into(), Tag::IntArray(p.to_vec())),
                    ("Item".into(), Tag::String(h.item.as_str().into())),
                    ("Count".into(), Tag::Int(i32::from(h.count))),
                    ("BeltPosition".into(), Tag::Float(h.position.to_bits())),
                    ("From".into(), Tag::Int(h.from as i32)),
                    ("ProcessingTicks".into(), Tag::Int(dr.processing)),
                ])))
            })
            .collect();
        let spouts: Vec<i32> = spouts.iter().flat_map(|(p, s)| tank(p, s.fluid)).collect();
        let drains: Vec<i32> = drains.iter().flat_map(|(p, s)| tank(p, s.fluid)).collect();
        Some((spouts, drains, items))
    })
    .flatten() else {
        return;
    };
    if spouts.is_empty() && drains.is_empty() {
        return;
    }
    let mut own = Compound::from([
        ("spouts".into(), Tag::IntArray(spouts)),
        ("drains".into(), Tag::IntArray(drains)),
        ("drain_items".into(), Tag::List(items)),
    ]);
    super::put_fluid_keys(&mut own, KEY, codes.into_inner());
    data.insert(KEY.into(), Tag::Compound(own));
}

/// What a column saved of its spouts and drains, put back (the blocks found later keep it).
pub(super) fn chunk_load(dim: usize, data: &Compound) {
    let Some(Tag::Compound(own)) = data.get(&KEY.into()) else {
        return;
    };
    let ints = |k: &str| match own.get(&k.into()) {
        Some(Tag::IntArray(v)) => v.clone(),
        _ => Vec::new(),
    };
    let keys = super::fluid_keys(own, KEY);
    let fluid =
        |c: &[i32]| create_engine::tank::Codes::fluid(&keys, c[3]).map(|f| (f, c[4].max(0) as u32));
    let (spouts, drains) = (ints("spouts"), ints("drains"));
    let items = match own.get(&"drain_items".into()) {
        Some(Tag::List(l)) => l.clone(),
        _ => Vec::new(),
    };
    with_world(|w| {
        let d = &mut w.dims[dim];
        for c in spouts.as_chunks::<5>().0 {
            d.spouts.entry([c[0], c[1], c[2]]).or_default().fluid = fluid(c);
        }
        for c in drains.as_chunks::<5>().0 {
            d.drains.entry([c[0], c[1], c[2]]).or_default().fluid = fluid(c);
        }
        for t in items {
            let Tag::Compound(c) = t else {
                continue;
            };
            let get = |k: &str| c.get(&k.into());
            let (Some(Tag::IntArray(p)), Some(Tag::String(item))) = (get("Pos"), get("Item"))
            else {
                continue;
            };
            let &[x, y, z] = p.as_slice() else {
                continue;
            };
            let int = |k: &str| match get(k) {
                Some(Tag::Int(v)) => *v,
                _ => 0,
            };
            let position = match get("BeltPosition") {
                Some(Tag::Float(b)) => f32::from_bits(*b),
                _ => 0.0,
            };
            let from = create_engine::kinetics::Dir::ALL[int("From").clamp(0, 5) as usize];
            let dr = d.drains.entry([x, y, z]).or_default();
            dr.processing = int("ProcessingTicks");
            dr.held = Some(create_engine::drain::Held {
                item: item.to_string_lossy(),
                count: int("Count").clamp(1, 64) as u8,
                position,
                prev: position,
                from,
            });
        }
    });
}
