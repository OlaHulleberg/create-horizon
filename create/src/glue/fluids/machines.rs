//! Spouts and item drains, and the `create:filling` / `create:emptying` recipes they run (read
//! with processing's recipes, one store as in the legacy glue). A sequenced assembly's filling
//! step comes first (`SequencedAssemblyRecipe.getRecipe`): the assemblies and the progress on a
//! stack are processing's.

use super::super::processing::{self, recipes};
use super::super::redstone::analog_touched;
use super::super::server::ServerLevel;
use super::super::world::with_world;
use create_engine::spout::{self, Filling};
use create_engine::tank::Fluid;
use create_engine::{belt, sequenced};
use mod_api::{HostTransaction, Identifier, ItemStack, ServerHost};

pub(super) fn stack_of(host: &dyn ServerHost, item: &str, count: u32) -> Option<ItemStack> {
    let key = host.item_key(&Identifier::parse(item).ok()?)?;
    ItemStack::new(key, count)
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
pub(super) fn tick_spouts(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let spouts: Vec<([i32; 3], spout::Spout)> = with_world(|w| {
        w.dims[dim]
            .spouts
            .iter()
            .filter(|(_, s)| s.fluid.is_some() || s.processing >= 0)
            .map(|(p, s)| (*p, *s))
            .collect()
    })
    .unwrap_or_default();
    if spouts.is_empty() || !recipes::ready() {
        return;
    }
    for (pos, mut s) in spouts {
        let before = s.fluid;
        let depot = [pos[0], pos[1] - 2, pos[2]];
        let on_depot = with_world(|w| w.dims[dim].depots.contains_key(&depot)).unwrap_or(false);
        let entity = on_depot
            .then(|| lv.host.block_entity(lv.level, depot))
            .flatten();
        // The depot's held stack (slot 0).
        let held = entity
            .and_then(|e| e.item_slot(&*lv.host, 0).ok().flatten())
            .map(|stack| (stack.item().id(&*lv.host), stack.count()));
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
                    spout::step(&mut s, item, &step)
                }
                None => spout::step(&mut s, item, &r.filling),
            }
        })
        .flatten();
        match (filled, &sequenced, held, entity) {
            (Some(_), Some((a, progress, _, _)), Some((_, count)), _) => {
                let state =
                    with_world(|w| w.dims[dim].depots.get(&depot).copied().flatten()).flatten();
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
                fill_held(lv.host, entity, &item, count, &out);
                with_world(|w| w.depot_changed(dim, depot));
            }
            _ => {}
        }
        with_world(|w| w.dims[dim].spouts.insert(pos, s));
        if s.fluid != before {
            analog_touched(dim, pos);
        }
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
            let outputs = match &sequenced {
                Some((a, _, _)) => {
                    let at = [above[0], above[1] - 2, above[2]];
                    processing::sequenced_on_belt(lv, at, item, a)
                }
                None => {
                    let mut o = item.clone();
                    o.item = out;
                    o.count = 1;
                    o.progress = None;
                    vec![o]
                }
            };
            Process::Convert {
                held: processing::belt_left(item),
                outputs,
            }
        }
    })
}

/// `convertToAndLeaveHeld`: one of the `count` held `item`s becomes `out`; the rest stays held,
/// the filled one goes to the outputs as the depot stacks them (`insertItemStacked`).
fn fill_held(
    host: &mut dyn ServerHost,
    entity: mod_api::BlockEntityRef,
    item: &str,
    count: u32,
    out: &str,
) {
    let Some(filled) = stack_of(&*host, out, 1) else {
        return;
    };
    let rest = if count > 1 {
        stack_of(&*host, item, count - 1)
    } else {
        None
    };
    let _ = host.transaction(|host, tx| {
        match rest {
            None => {
                entity
                    .set_item_slot(host, tx, 0, Some(filled))
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
                    s.as_ref()
                        .is_some_and(|s| s.is_stackable_with(&filled) && s.count() < 64)
                });
                if let Some((i, Some(s))) = stacked {
                    let mut s = s.clone();
                    s.grow(1).map_err(|_| ())?;
                    entity
                        .set_item_slot(host, tx, *i, Some(s))
                        .map_err(|_| ())?;
                } else if let Some((i, _)) = slots.iter().find(|(_, s)| s.is_none()) {
                    entity
                        .set_item_slot(host, tx, *i, Some(filled))
                        .map_err(|_| ())?;
                }
            }
        }
        Ok::<(), ()>(())
    });
}

/// Item drains holding items (`ItemDrainBlockEntity.tick`): a filled bucket is emptied into the
/// tank each cycle while it has room; the empty bucket is thrown off on top; anything else is
/// let go the same way.
pub(super) fn tick_drains(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let drains: Vec<[i32; 3]> = with_world(|w| {
        w.dims[dim]
            .drains
            .iter()
            .filter(|(_, d)| d.held.is_some())
            .map(|(p, _)| *p)
            .collect()
    })
    .unwrap_or_default();
    if drains.is_empty() {
        return;
    }
    for pos in drains {
        let Some(out) = recipes::with(|r| {
            let emptying = &r.emptying[..];
            with_world(|w| {
                let d = w.dims[dim].drains.get_mut(&pos)?;
                let held = d.held.clone();
                if held
                    .as_ref()
                    .is_some_and(|h| create_engine::drain::contents(&h.0, emptying).is_none())
                {
                    d.held = None;
                    return held;
                }
                let before = d.fluid;
                let out = create_engine::drain::tick(d, emptying);
                if d.fluid != before {
                    w.dims[dim].analog_checks.push(pos);
                }
                out?.map(|left| (left, 1))
            })
            .flatten()
        })
        .flatten() else {
            continue;
        };
        let Some(stack) = stack_of(&*lv.host, &out.0, u32::from(out.1)) else {
            continue;
        };
        let (x, y, z) = (
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]) + 1.0,
            f64::from(pos[2]) + 0.5,
        );
        // `summon minecraft:item ~ ~1 ~ {PickupDelay:20}`: no motion.
        let _ = lv
            .level
            .spawn_item_with_motion(&mut *lv.host, x, y, z, stack, [0.0; 3], 20);
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
    let Some((item, count)) =
        with_world(|w| w.dims[dim].drains.get_mut(&pos).and_then(|d| d.held.take())).flatten()
    else {
        return;
    };
    with_world(|w| w.mark_unsaved(dim, pos));
    if let Some(stack) = stack_of(&*lv.host, &item, u32::from(count)) {
        super::super::items::give_back(lv.host, player, stack);
    }
}
