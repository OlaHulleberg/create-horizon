//! The block breakers and the saw: the mechanical drill and saw breaking the block in front
//! (`BlockBreakingKineticBlockEntity`), and the saw facing up cutting what lands on it
//! (`SawBlockEntity`).

use super::takes_from_side;
use super::{recipes, stacks};
use crate::glue::logistics::{insert_into_from, random_angle, upright};
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::kinetics::Dir;
use create_engine::processing::{Recipe, Rng};
use create_engine::saw::{self, Step};
use create_engine::{belt, drill, sequenced};
use mod_api::{HostBlockStates, ItemStack};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Each saw's `recipeIndex` while it holds nothing (an empty saw keeps no other state).
    static SAW_INDEX: RefCell<HashMap<(usize, [i32; 3]), usize>> = RefCell::new(HashMap::new());
}

/// Forgets what was kept for the world.
pub fn clear() {
    SAW_INDEX.with_borrow_mut(HashMap::clear);
}

/// Turning drills (`BlockBreakingKineticBlockEntity.tick`): the block in front breaks in steps
/// (`isBreakable`: not air, a fluid, unbreakable or `create:non_breakable`), dropping what it
/// drops; a drill that broke one looks again at its next lazy tick (every 10 ticks).
pub fn tick_drills(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let drills = with_world(|w| w.dims[dim].drilling.clone()).unwrap_or_default();
    if drills.is_empty() {
        return;
    }
    let lazy = lv.game_time() % 10 == 0;
    for pos in drills {
        let Some(facing) = lv.info(pos).and_then(|(_, i)| i.facing) else {
            continue;
        };
        let at = super::step(pos, facing, 1);
        let fluid = lv
            .level
            .fluid_state(&*lv.host, at)
            .is_some_and(|f| f.amount > 0)
            && lv.replaceable(at);
        let hardness = if lv.is_air(at) || fluid || lv.block_tagged(at, "create:non_breakable") {
            None
        } else {
            lv.block_state(at)
                .and_then(|s| lv.host.block_states().destroy_speed(s))
                .and_then(drill::breakable)
        };
        let speed = lv.speed(pos);
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
        if step == drill::Step::Broken {
            // Level.destroyBlock with drops (`BlockHelper.destroyBlock`).
            lv.destroy_block(at, true);
        }
    }
}

/// `SawBlockEntity.insertItem` for an item landing on a saw facing up, while it holds nothing:
/// it takes up to a stack; `stack` keeps what is left.
pub fn saw_take(lv: &mut ServerLevel<'_>, pos: [i32; 3], stack: &mut Option<ItemStack>) {
    let dim = lv.dim;
    if lv.info(pos).and_then(|(_, i)| i.facing) != Some(Dir::Up) {
        return;
    }
    let Some(s) = stack.as_ref() else {
        return;
    };
    let item = stacks::id(&*lv.host, s);
    let count = s.count().min(64) as u8;
    let progress = stacks::sequenced_progress(s);
    let took = with_world(|w| {
        let s = w.dims[dim].saws.entry(pos).or_default();
        if !s.slots.is_empty() {
            return false;
        }
        s.slots = vec![(item, count)];
        s.remaining = -1.0;
        s.applied = false;
        // `recipeIndex` stays with the saw between items.
        s.index = SAW_INDEX.with_borrow(|m| m.get(&(dim, pos)).copied().unwrap_or(0));
        s.progress = progress;
        true
    })
    .unwrap_or(false);
    if took {
        *stack = stack.take().and_then(|s| s.shrink(64));
    }
}

/// Saws holding items (`SawBlockEntity.tick`): their cutting recipe in turn, applied near the
/// end, the results thrown off the way the saw turns (onto a belt or depot there, else as items).
pub fn tick_saws(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let saws: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].saws.keys().copied().collect()).unwrap_or_default();
    if saws.is_empty() || !recipes::ready() {
        return;
    }
    for pos in saws {
        let Some(along_first) = lv.info(pos).map(|(_, i)| i.along_first) else {
            continue;
        };
        let speed = lv.speed(pos);
        let Some(mut saw) = with_world(|w| w.dims[dim].saws.get(&pos).cloned()).flatten() else {
            continue;
        };
        let item = saw.slots.first().map(|s| s.0.clone()).unwrap_or_default();
        match saw.tick(speed) {
            Step::Wait => {}
            Step::Start => {
                recipes::with(|r| {
                    // A sequenced assembly's cutting step alone, when the item is at one
                    // (`getRecipes`), for its processing time.
                    if let Some((_, step)) = sequenced::step_for(
                        &r.sequenced,
                        &item,
                        saw.progress.as_ref(),
                        "create:cutting",
                    ) {
                        let time = Recipe {
                            id: String::new(),
                            ingredients: Vec::new(),
                            results: Vec::new(),
                            time: step.time,
                        };
                        saw.start(&[&time]);
                        return;
                    }
                    let list: Vec<&Recipe> = r
                        .cutting
                        .matching(&item)
                        .into_iter()
                        .map(|i| r.cutting.get(i))
                        .collect();
                    saw.start(&list);
                });
            }
            Step::Apply => {
                let mut rng = Rng::new(lv.seed_at(pos));
                let host = &*lv.host;
                recipes::with(|r| {
                    if let Some((a, _)) = sequenced::step_for(
                        &r.sequenced,
                        &item,
                        saw.progress.as_ref(),
                        "create:cutting",
                    ) {
                        saw.apply_assembly(a, &mut rng);
                        return;
                    }
                    let list = r.cutting.matching(&item);
                    let recipe = list.get(saw.index).map(|&i| r.cutting.get(i));
                    saw.apply(recipe, |i: &str| stacks::limit(host, i), &mut rng);
                });
            }
            Step::Out => {
                let m = saw::movement(along_first, speed);
                // A belt or depot it turns toward takes the results (`DirectBeltInputBehaviour`
                // from that side); what it doesn't take waits in the saw.
                let next = [pos[0] + m[0], pos[1], pos[2] + m[2]];
                let facing = Dir::ALL.into_iter().find(|d| d.offset() == m);
                if let (true, Some(facing)) = (takes_from_side(dim, next), facing) {
                    let progress = saw.progress.clone();
                    let transitional = progress
                        .as_ref()
                        .and_then(super::transitional_of)
                        .map(|t| t.0);
                    let mut left = Vec::new();
                    for (item, count) in std::mem::take(&mut saw.slots) {
                        let up = upright(&*lv.host, &item);
                        let mut one = belt::Item::new(&item, count, random_angle(lv, pos), up);
                        if transitional.as_deref() == Some(item.as_str()) {
                            one.progress = progress.clone();
                        }
                        let n = insert_into_from(lv, next, facing, &one, false).unwrap_or(count);
                        if n > 0 {
                            left.push((item, n));
                        }
                    }
                    if left.is_empty() {
                        saw.clear();
                        SAW_INDEX.with_borrow_mut(|m| m.insert((dim, pos), saw.index));
                    } else {
                        saw.slots = left;
                    }
                    with_world(|w| {
                        if saw.slots.is_empty() {
                            w.dims[dim].saws.remove(&pos);
                        } else {
                            w.dims[dim].saws.insert(pos, saw);
                        }
                    });
                    continue;
                }
                let at = [
                    f64::from(pos[0]) + 0.5 + f64::from(m[0]) * 0.5,
                    f64::from(pos[1]) + 1.0,
                    f64::from(pos[2]) + 0.5 + f64::from(m[2]) * 0.5,
                ];
                let v = [f64::from(m[0]) * 0.0625, 0.125, f64::from(m[2]) * 0.0625];
                let transitional = saw.progress.as_ref().and_then(super::transitional_of);
                for (item, count) in &saw.slots {
                    let Some(mut stack) = stacks::stack(&*lv.host, item, u32::from(*count)) else {
                        continue;
                    };
                    // A transitional item keeps its progress.
                    if let Some((t, p, fraction)) = &transitional
                        && t == item
                    {
                        stacks::set_sequenced_progress(&mut stack, &p.id, p.step, *fraction);
                    }
                    lv.spawn_stack(at, stack, v);
                }
                saw.clear();
            }
        }
        if saw.slots.is_empty() {
            SAW_INDEX.with_borrow_mut(|m| m.insert((dim, pos), saw.index));
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
