//! The basin and what works on it: the mechanical mixer (`MechanicalMixerBlockEntity`), the
//! press compacting (`MechanicalPressBlockEntity` as a basin operator), and the blaze burner
//! heating it (`BlazeBurnerBlockEntity`).

use super::{recipes, stacks};
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::basin::{self, MixStep};
use create_engine::fan::Heat;
use create_engine::processing::{Output, Rng, Slot};
use create_engine::{Block, burner, press};
use mod_api::{Compound, ItemStack, PlayerRef, Tag};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// The square crafting recipe a press is compacting with, by dimension and press.
    static PRESS_CRAFTED: RefCell<HashMap<(usize, [i32; 3]), basin::Recipe>> =
        RefCell::new(HashMap::new());
    /// `canCompress` answers by item: its 3×3 and 2×2 crafting recipes of only it. Forgotten
    /// with the recipes (a data reload or the world closing).
    static COMPRESSED: RefCell<HashMap<String, Vec<basin::Recipe>>> = RefCell::new(HashMap::new());
}

/// Forgets what was kept for the world.
pub fn clear() {
    PRESS_CRAFTED.with_borrow_mut(HashMap::clear);
    COMPRESSED.with_borrow_mut(HashMap::clear);
    MIXER_SHOWN.with_borrow_mut(HashMap::clear);
}

/// The recipes were read again: what was worked out from them goes.
pub fn recipes_changed() {
    COMPRESSED.with_borrow_mut(HashMap::clear);
}

/// The heat under a basin (`BasinBlockEntity.getHeatLevelOf`): a blaze burner's, smouldering for
/// a lit burner or another passive heater, else none.
pub fn basin_heat(lv: &ServerLevel<'_>, basin: [i32; 3]) -> Heat {
    let under = [basin[0], basin[1] - 1, basin[2]];
    if let Some(h) = lv.info(under).and_then(|(_, i)| i.heat) {
        return h;
    }
    if lv.block_tagged(under, "create:passive_boiler_heaters") {
        // BlockHelper.isNotUnheated: an unlit campfire is no heater.
        if lv.property(under, "lit").as_deref() != Some("false") {
            return Heat::Smouldering;
        }
    }
    Heat::None
}

/// Whether the block at `p` is a basin.
pub fn basin_at(lv: &ServerLevel<'_>, p: [i32; 3]) -> bool {
    lv.info(p).is_some_and(|(b, _)| b == Block::Basin)
}

/// A basin's tanks (none: empty).
pub fn basin_tanks(dim: usize, pos: [i32; 3]) -> basin::Tanks {
    with_world(|w| w.dims[dim].basin_fluids.get(&pos).copied())
        .flatten()
        .unwrap_or_default()
}

/// The key of a basin's block entity data its clients read its tanks from: fluid index (-1 for
/// none) and amount of the inputs, then the outputs.
pub const FLUIDS_KEY: &str = "BasinFluids";
/// The keys of the potions `FLUIDS_KEY` numbers (`fluids::fluid_keys`).
pub const FLUIDS_KEYS_KEY: &str = "BasinFluids_fluids";
/// The key of a basin's block entity data its clients read its stacks from (`id`, `count`), in
/// slot order: a client's block entity has no slots.
pub const ITEMS_KEY: &str = "BasinItems";
/// The key of a mixer's block entity data its clients run its head from: `T`, its running ticks
/// when told, and `S`, the game time it was told at; none when it rests.
pub const MIXER_KEY: &str = "Mixer";
/// Ticks between a basin's `recipeBackupCheck`s (a lazy tick of 10, counted down from 20).
const BACKUP_CHECK: i64 = 200;
/// A blaze burner's blaze wears goggles (`BlazeBurnerBlockEntity.goggles`).
pub const GOGGLES_KEY: &str = "Goggles";

/// A running mixer's running ticks as its clients were last told, and the game time then.
type Told = (i32, i64);

thread_local! {
    /// What each running mixer's clients were last told, by dimension and mixer.
    static MIXER_SHOWN: RefCell<HashMap<(usize, [i32; 3]), Told>> = RefCell::new(HashMap::new());
}

/// Puts each value under its key in the data of the block entity at `pos` (none: takes it
/// away), which sends it to its clients; nothing when it holds them already.
pub(super) fn set_shown(lv: &mut ServerLevel<'_>, pos: [i32; 3], values: &[(&str, Option<Tag>)]) {
    let Some(entity) = lv.entity_at(pos) else {
        return;
    };
    let Some(mut data) = entity.data(&*lv.host) else {
        return;
    };
    let mut changed = false;
    for (key, value) in values {
        if data.get(&(*key).into()) == value.as_ref() {
            continue;
        }
        changed = true;
        match value {
            Some(v) => {
                data.insert((*key).into(), v.clone());
            }
            None => {
                data.remove(&(*key).into());
            }
        }
    }
    if changed {
        let _ = entity.replace_data(&mut *lv.host, data);
    }
}

/// The basins whose tanks or slots changed this tick tell their clients (`FLUIDS_KEY`,
/// `ITEMS_KEY`).
pub fn show_basins(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let changed = with_world(|w| {
        let c = &mut w.dims[dim].basin_changes;
        if c.is_empty() {
            Vec::new()
        } else {
            std::mem::take(c)
        }
    })
    .unwrap_or_default();
    let mut changed = changed;
    changed.sort();
    changed.dedup();
    for pos in changed {
        if !lv.is_loaded(pos) || !basin_at(lv, pos) {
            continue;
        }
        let t = basin_tanks(dim, pos);
        let mut codes = create_engine::tank::Codes::default();
        let fluids = (t != basin::Tanks::default()).then(|| {
            Tag::IntArray(
                t.input
                    .iter()
                    .chain(t.output.iter())
                    .flat_map(|s| match s {
                        Some((f, n)) => [codes.code(Some(*f)), *n as i32],
                        None => [-1, 0],
                    })
                    .collect(),
            )
        });
        // The potions' keys (`fluids::fluid_keys`), none taken away.
        let keys = (!codes.keys.is_empty()).then(|| {
            Tag::List(
                codes
                    .keys
                    .iter()
                    .map(|k| Tag::String(k.as_str().into()))
                    .collect(),
            )
        });
        let host = &*lv.host;
        let items: Vec<Tag> = lv
            .slots(pos, basin::SLOTS)
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .map(|s| {
                Tag::Compound(Compound::from([
                    (
                        "id".into(),
                        Tag::String(stacks::id(host, &s).as_str().into()),
                    ),
                    ("count".into(), Tag::Int(s.count() as i32)),
                ]))
            })
            .collect();
        // Empty ones are taken away (their clients see them absent: `kinetics::synced`).
        let items = (!items.is_empty()).then_some(Tag::List(items));
        set_shown(
            lv,
            pos,
            &[
                (FLUIDS_KEY, fluids),
                (FLUIDS_KEYS_KEY, keys),
                (ITEMS_KEY, items),
            ],
        );
    }
}

/// Its basin's slots changed (taken from or put in): its clients are told.
pub fn basin_changed(dim: usize, pos: [i32; 3]) {
    with_world(|w| w.dims[dim].basin_changes.push(pos));
}

/// Tells a mixer's clients its cycle (`MIXER_KEY`) when they would work out another one than
/// the server's (`basin::shown_ticks`): when it starts, leaves the bottom, stays down for the
/// next recipe or stops.
fn show_mixer(lv: &mut ServerLevel<'_>, pos: [i32; 3], m: &basin::Mixer) {
    let dim = lv.dim;
    let now = lv.game_time();
    let told = MIXER_SHOWN.with_borrow(|s| s.get(&(dim, pos)).copied());
    let value = if !m.running {
        if told.is_none() {
            return;
        }
        MIXER_SHOWN.with_borrow_mut(|s| s.remove(&(dim, pos)));
        // At rest: none.
        None
    } else {
        if told.is_some_and(|(t, at)| basin::shown_ticks(t, now - at) == m.running_ticks) {
            return;
        }
        MIXER_SHOWN.with_borrow_mut(|s| s.insert((dim, pos), (m.running_ticks, now)));
        Some(Tag::Compound(Compound::from([
            ("T".into(), Tag::Int(m.running_ticks)),
            ("S".into(), Tag::Long(now)),
        ])))
    };
    set_shown(lv, pos, &[(MIXER_KEY, value)]);
}

/// The slots of the basin at `pos`, all 18.
fn basin_slots(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Vec<Slot>> {
    lv.read_slots(pos, 0, basin::SLOTS)
}

/// Writes the basin's slots back, those that changed.
fn write_basin(lv: &mut ServerLevel<'_>, pos: [i32; 3], before: &[Slot], after: &[Slot]) {
    let host = &*lv.host;
    let changes: Vec<(usize, Option<ItemStack>)> = after
        .iter()
        .enumerate()
        .filter(|(i, s)| before.get(*i) != Some(*s))
        .map(|(i, s)| (i, stacks::from_slot(host, s)))
        .collect();
    if !changes.is_empty() {
        lv.set_slots(pos, &changes);
        basin_changed(lv.dim, pos);
    }
}

/// Plays `sound` at the centre of `pos`.
fn sound(
    lv: &mut ServerLevel<'_>,
    sound: &str,
    pos: [i32; 3],
    source: mod_api::SoundSource,
    volume: f32,
    pitch: f32,
) {
    let at = pos.map(|v| f64::from(v) + 0.5);
    let _ = lv
        .level
        .play_sound(&mut *lv.host, sound, at, source, volume, pitch);
}

/// `BasinBlock.updateEntityAfterFallOn`: an item landing in a basin goes into its inputs as far
/// as it fits (`insertItem`). `stack` keeps what is left.
pub fn basin_take(lv: &mut ServerLevel<'_>, pos: [i32; 3], stack: &mut Option<ItemStack>) {
    let Some(s) = stack.as_ref() else {
        return;
    };
    let item = stacks::id(&*lv.host, s);
    let max = stacks::limit(&*lv.host, &item);
    let count = s.count().min(255) as u8;
    let Some(before) = lv.read_slots(pos, 0, basin::INPUTS.end) else {
        return;
    };
    let mut inputs = before.clone();
    let left = create_engine::processing::insert_stacked(&mut inputs, &item, count, max);
    if left == count {
        return;
    }
    write_basin(lv, pos, &before, &inputs);
    *stack = stack.take().and_then(|s| s.shrink(u32::from(count - left)));
    let dim = lv.dim;
    with_world(|w| w.dims[dim].mixer_checks.push([pos[0], pos[1] + 2, pos[2]]));
}

/// `BasinBlock.useItemOn`: an empty hand takes every stack out (inputs and outputs, into the
/// inventory); an item holding a fluid (a bucket, a potion with its whole contents, an emptying
/// recipe's) empties into an input tank when all of it fits (`FluidHelper.tryEmptyItemIntoBE`);
/// an item that can be filled (an empty bucket, a glass bottle) takes what it needs from the
/// first tank holding enough, the outputs first (`tryFillItemFromBE` on
/// `CombinedTankWrapper(output, input)`). A creative player keeps the item and gets none back.
/// True when the click was the basin's.
pub fn use_basin(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: PlayerRef,
    stack: Option<&ItemStack>,
) -> bool {
    let dim = lv.dim;
    let Some(stack) = stack else {
        let Some(slots) = lv.slots(pos, basin::SLOTS) else {
            return false;
        };
        let taken: Vec<(usize, Option<ItemStack>)> = slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_some())
            .map(|(i, _)| (i, None))
            .collect();
        if !taken.is_empty() && lv.set_slots(pos, &taken) {
            basin_changed(dim, pos);
            for s in slots.into_iter().flatten() {
                lv.give(player, s);
            }
            sound(
                lv,
                "minecraft:entity.item.pickup",
                pos,
                mod_api::SoundSource::Players,
                0.2,
                1.5,
            );
        }
        return true;
    };
    let creative = lv.creative(player);
    // A potion by its whole contents (`PotionFluidHandler.getFluidFromPotionItem`).
    let held = crate::glue::logistics::items::potion_name(&*lv.host, stack)
        .unwrap_or_else(|| stacks::id(&*lv.host, stack));
    let mut tanks = basin_tanks(dim, pos);
    let emptying = crate::glue::fluids::clicks::emptying(&held);
    // `tryEmptyItemIntoBE`: all of it into an input tank, or nothing.
    if let Some(e) = &emptying {
        let mut inputs = tanks.input;
        if basin::fill(&mut inputs, e.fluid, e.amount) == e.amount {
            tanks.input = inputs;
            with_world(|w| w.set_basin_fluids(dim, pos, tanks));
            if !creative {
                lv.take_held(player, 1);
                if let Some(left) = e
                    .leftover
                    .as_deref()
                    .and_then(|l| crate::glue::logistics::items::stack_of(&*lv.host, l, 1))
                {
                    lv.give(player, left);
                }
            }
            return true;
        }
    }
    // `tryFillItemFromBE` on `CombinedTankWrapper(output, input)`: the first tank holding what
    // filling the item takes.
    for i in 0..4 {
        let slot = if i < 2 {
            &mut tanks.output[i]
        } else {
            &mut tanks.input[i - 2]
        };
        let Some((fluid, have)) = *slot else {
            continue;
        };
        let Some((need, out)) = crate::glue::fluids::clicks::filling(&held, fluid) else {
            continue;
        };
        if need > have {
            continue;
        }
        *slot = (have > need).then_some((fluid, have - need));
        with_world(|w| w.set_basin_fluids(dim, pos, tanks));
        if !creative {
            lv.take_held(player, 1);
            if let Some(out) = crate::glue::logistics::items::stack_of(&*lv.host, &out, 1) {
                lv.give(player, out);
            }
        }
        return true;
    }
    // An item that could be emptied or filled is used up all the same.
    emptying.is_some()
        || held == "minecraft:bucket"
        || held == "minecraft:glass_bottle"
        || recipes::with(|r| create_engine::spout::fillable(&held, &r.filling)).unwrap_or(false)
}

/// Mechanical mixers: those asked to look at their basin (`updateBasin`: turning, not running,
/// a recipe the basin can do), then the running ones' cycle (`tick`), mixing at the bottom
/// (`applyBasinRecipe`) and going on with the same recipe while it still fits.
pub fn tick_mixers(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].mixer_checks)).unwrap_or_default();
    // `BasinBlockEntity.lazyTick`'s `recipeBackupCheck`: every 200 ticks a basin that holds
    // something has its mixer or press look for a recipe again, so a burner lit (or a recipe
    // reloaded) after the items went in still gets them going.
    if lv.game_time() % BACKUP_CHECK == 0 {
        let machines: Vec<[i32; 3]> = with_world(|w| {
            let d = &w.dims[dim];
            d.mixers
                .iter()
                .filter(|(_, m)| !m.running)
                .map(|(p, _)| *p)
                .chain(d.presses.keys().copied())
                .collect()
        })
        .unwrap_or_default();
        for pos in machines {
            let basin_pos = [pos[0], pos[1] - 2, pos[2]];
            let tanks = basin_tanks(dim, basin_pos);
            let filled = basin_slots(lv, basin_pos).is_some_and(|s| s.iter().any(Option::is_some))
                || tanks.input.iter().chain(&tanks.output).any(Option::is_some);
            if filled {
                checks.push(pos);
            }
        }
    }
    let running = with_world(|w| w.dims[dim].mixing.clone()).unwrap_or_default();
    if checks.is_empty() && running.is_empty() {
        return;
    }
    if !recipes::ready() {
        return;
    }
    for pos in running {
        let speed = lv.speed(pos);
        let Some(mut m) = with_world(|w| w.dims[dim].mixers.get(&pos).copied()).flatten() else {
            continue;
        };
        let time = recipes::with(|r| m.recipe.and_then(|i| r.mixing.get(i)).map(|x| x.time))
            .flatten()
            .unwrap_or(0);
        let basin_pos = [pos[0], pos[1] - 2, pos[2]];
        match m.tick(speed, time) {
            MixStep::None => {}
            MixStep::Done => checks.push(pos),
            MixStep::Apply => {
                let heat = basin_heat(lv, basin_pos);
                let mut rng = Rng::new(lv.seed_at(pos));
                // `acceptOutputs(.., true)`: a basin spouting to a side needs a taker there.
                let gives_items = recipes::with(|r| {
                    m.recipe
                        .and_then(|i| r.mixing.get(i))
                        .is_some_and(|x| !x.results.is_empty())
                })
                .unwrap_or(false);
                let can_output = !gives_items || super::spoutput::accepts(lv, basin_pos);
                if let (Some(before), Some(i), true) =
                    (basin_slots(lv, basin_pos), m.recipe, can_output)
                {
                    let host = &*lv.host;
                    let limit = |i: &str| stacks::limit(host, i);
                    let mut slots = before.clone();
                    let mut tanks = basin_tanks(dim, basin_pos);
                    let (done, again) = recipes::with(|r| {
                        let Some(recipe) = r.mixing.get(i) else {
                            return (false, false);
                        };
                        let done = basin::apply(
                            &mut slots, &mut tanks, recipe, heat, limit, &mut rng, false,
                        );
                        let again = done
                            && basin::apply(
                                &mut slots.clone(),
                                &mut tanks.clone(),
                                recipe,
                                heat,
                                limit,
                                &mut rng,
                                true,
                            );
                        (done, again)
                    })
                    .unwrap_or_default();
                    if done {
                        let fluids_before = basin_tanks(dim, basin_pos);
                        super::spoutput::route_fluids(lv, basin_pos, &fluids_before, &mut tanks);
                        crate::glue::advancements::award_placer(lv, pos, "mechanical_mixer");
                        with_world(|w| w.set_basin_fluids(dim, basin_pos, tanks));
                        super::spoutput::route(lv, basin_pos, &before, &mut slots);
                        write_basin(lv, basin_pos, &before, &slots);
                        // `continueWithPreviousRecipe`: stays down for the next one.
                        if again {
                            m.running_ticks = 20;
                        }
                    }
                }
            }
        }
        with_world(|w| {
            let d = &mut w.dims[dim];
            d.mixers.insert(pos, m);
            if !m.running {
                d.mixing.retain(|p| *p != pos);
            }
        });
        show_mixer(lv, pos, &m);
    }
    checks.sort();
    checks.dedup();
    for pos in checks {
        let Some(mut m) = with_world(|w| w.dims[dim].mixers.get(&pos).copied()).flatten() else {
            press_on_basin(lv, pos);
            continue;
        };
        let speed = lv.speed(pos);
        let basin_pos = [pos[0], pos[1] - 2, pos[2]];
        if !basin_at(lv, basin_pos) {
            // `onBasinRemoved`.
            if m.running {
                m.running = false;
                m.running_ticks = 0;
                with_world(|w| {
                    w.dims[dim].mixers.insert(pos, m);
                    w.dims[dim].mixing.retain(|p| *p != pos);
                });
                show_mixer(lv, pos, &m);
            }
            continue;
        }
        // `canContinueProcessing`: not while results wait at the spout.
        // `updateBasin`: `isSpeedRequirementFulfilled` (medium speed) first.
        if speed.abs() < basin::MIXER_MIN_SPEED
            || m.running
            || !super::spoutput::can_continue(lv, basin_pos)
        {
            continue;
        }
        let Some(slots) = basin_slots(lv, basin_pos) else {
            continue;
        };
        let heat = basin_heat(lv, basin_pos);
        let host = &*lv.host;
        let limit = |i: &str| stacks::limit(host, i);
        let tanks = basin_tanks(dim, basin_pos);
        // `matchBasinRecipe`: the basin's filter on the recipe's result.
        let filter = super::filtering::filter_at(dim, basin_pos);
        let found = recipes::with(|r| {
            basin::matching(&r.mixing, &slots, &tanks, heat, limit)
                .into_iter()
                .find(|&i| {
                    super::filtering::basin_recipe_passes(host, filter.as_ref(), &r.mixing[i])
                })
        })
        .flatten();
        if let Some(i) = found {
            m.start(i);
            with_world(|w| {
                let d = &mut w.dims[dim];
                d.mixers.insert(pos, m);
                if !d.mixing.contains(&pos) {
                    d.mixing.push(pos);
                }
            });
            show_mixer(lv, pos, &m);
        }
    }
}

/// `MechanicalPressBlockEntity` as a basin operator (`updateBasin`): a turning press with a
/// basin two below starts a cycle over it (`Mode.BASIN`) for the compacting recipe with the most
/// ingredients the basin can do.
fn press_on_basin(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(p) = with_world(|w| w.dims[dim].presses.get(&pos).copied()).flatten() else {
        return;
    };
    let speed = lv.speed(pos);
    if p.running || speed == 0.0 {
        return;
    }
    let basin_pos = [pos[0], pos[1] - 2, pos[2]];
    if !basin_at(lv, basin_pos) || !super::spoutput::can_continue(lv, basin_pos) {
        return;
    }
    let Some(slots) = basin_slots(lv, basin_pos) else {
        return;
    };
    let heat = basin_heat(lv, basin_pos);
    let tanks = basin_tanks(dim, basin_pos);
    let crafted = compressible(lv, &slots);
    let host = &*lv.host;
    let limit = |i: &str| stacks::limit(host, i);
    // `matchBasinRecipe`: the basin's filter on the recipe's result.
    let filter = super::filtering::filter_at(dim, basin_pos);
    let passes =
        |c: &basin::Recipe| super::filtering::basin_recipe_passes(host, filter.as_ref(), c);
    let found = recipes::with(|r| {
        let i = basin::matching(&r.compacting, &slots, &tanks, heat, limit)
            .into_iter()
            .find(|&i| passes(&r.compacting[i]))?;
        Some((i, r.compacting[i].ingredients.len()))
    })
    .flatten();
    // Square crafting recipes of one item (`canCompress`), with the compacting ones: the one
    // with the most ingredients first (`getMatchingRecipes`' order).
    let crafted = crafted
        .into_iter()
        .filter(|c| passes(c))
        .find(|c| !basin::matching(std::slice::from_ref(c), &slots, &tanks, heat, limit).is_empty())
        .filter(|c| found.is_none_or(|(_, n)| c.ingredients.len() > n));
    PRESS_CRAFTED.with_borrow_mut(|m| match &crafted {
        Some(c) => {
            m.insert((dim, pos), c.clone());
        }
        None => {
            m.remove(&(dim, pos));
        }
    });
    let found = found.map(|f| f.0).or(crafted.as_ref().map(|_| usize::MAX));
    if let Some(i) = found {
        with_world(|w| {
            let d = &mut w.dims[dim];
            if let Some(p) = d.presses.get_mut(&pos) {
                p.start(press::Mode::Basin);
            }
            d.press_basin.insert(pos, i);
            d.press_changes.push(pos);
        });
    }
}

/// The crafting recipes made of 9, else 4, of one item in the basin's inputs
/// (`MechanicalPressBlockEntity.canCompress`: 4 or 9 ingredients, all the same), as basin
/// recipes, 9 first; looked up once per item.
fn compressible(lv: &ServerLevel<'_>, slots: &[Slot]) -> Vec<basin::Recipe> {
    let mut items: Vec<&str> = slots[basin::INPUTS]
        .iter()
        .flatten()
        .map(|(i, _)| i.as_str())
        .collect();
    items.sort();
    items.dedup();
    let mut out = Vec::new();
    for item in items {
        let known = COMPRESSED.with_borrow(|c| c.get(item).cloned());
        let found = known.unwrap_or_else(|| {
            let found: Vec<basin::Recipe> = [3usize, 2]
                .into_iter()
                .filter_map(|size| {
                    let square = vec![Some(item.to_string()); size * size];
                    let (made, n) = recipes::craft(lv, &square, size)?;
                    Some(basin::Recipe {
                        id: format!("crafting:{item}:{size}"),
                        ingredients: vec![vec![item.to_string()]; size * size],
                        results: vec![Output {
                            item: made,
                            count: n,
                            chance: 1.0,
                        }],
                        time: 0,
                        heat: Default::default(),
                        fluids: Vec::new(),
                        fluid_results: Vec::new(),
                    })
                })
                .collect();
            COMPRESSED.with_borrow_mut(|c| c.insert(item.to_string(), found.clone()));
            found
        });
        out.extend(found);
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.ingredients.len()));
    out
}

/// `tryProcessInBasin` at the head's lowest point: the press's compacting recipe on the basin.
pub fn compact_in_basin(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(i) = with_world(|w| w.dims[dim].press_basin.get(&pos).copied()).flatten() else {
        return;
    };
    let basin_pos = [pos[0], pos[1] - 2, pos[2]];
    // `acceptOutputs(.., true)`: compacting always gives items.
    if !super::spoutput::accepts(lv, basin_pos) {
        return;
    }
    let Some(before) = basin_slots(lv, basin_pos) else {
        return;
    };
    let mut slots = before.clone();
    let heat = basin_heat(lv, basin_pos);
    let mut rng = Rng::new(lv.seed_at(pos));
    let mut tanks = basin_tanks(dim, basin_pos);
    let crafted = PRESS_CRAFTED.with_borrow(|m| m.get(&(dim, pos)).cloned());
    let host = &*lv.host;
    let limit = |i: &str| stacks::limit(host, i);
    let done = match crafted.filter(|_| i == usize::MAX) {
        Some(recipe) => basin::apply(
            &mut slots, &mut tanks, &recipe, heat, limit, &mut rng, false,
        ),
        None => recipes::with(|r| {
            r.compacting.get(i).is_some_and(|recipe| {
                basin::apply(&mut slots, &mut tanks, recipe, heat, limit, &mut rng, false)
            })
        })
        .unwrap_or(false),
    };
    if done {
        let fluids_before = basin_tanks(dim, basin_pos);
        super::spoutput::route_fluids(lv, basin_pos, &fluids_before, &mut tanks);
        crate::glue::advancements::award_placer(lv, pos, "compacting");
        with_world(|w| w.set_basin_fluids(dim, basin_pos, tanks));
        super::spoutput::route(lv, basin_pos, &before, &mut slots);
        write_basin(lv, basin_pos, &before, &slots);
    }
}

/// Burning blaze burners (`BlazeBurnerBlockEntity.tick`): the fuel burns down; a changed heat is
/// written to the block (`blaze`), which fans and basins read.
pub fn tick_burners(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    // `updateBlockState` of a burner whose block changed: a burner set by a command (or turned
    // by a structure) takes the heat its fuel gives, smouldering without any, in its next tick.
    let checks = with_world(|w| std::mem::take(&mut w.dims[dim].burner_checks)).unwrap_or_default();
    for pos in checks {
        let Some((Block::BlazeBurner, info)) = lv.info(pos) else {
            continue;
        };
        // `if (isCreative) return;`: a creative burner keeps whatever its block shows.
        let burner = with_world(|w| w.dims[dim].burners.get(&pos).copied()).flatten();
        if burner.is_some_and(|b| b.creative.is_some()) {
            continue;
        }
        let heat = burner.map_or(Heat::Smouldering, |b| b.heat());
        if info.heat != Some(heat) {
            lv.set_property(
                pos,
                "blaze",
                burner::heat_name(heat),
                crate::glue::server::update_all(),
            );
        }
    }
    let changed = with_world(|w| {
        if w.dims[dim].burning.is_empty() {
            Vec::new()
        } else {
            w.tick_burners(dim)
        }
    })
    .unwrap_or_default();
    for (pos, heat) in changed {
        lv.set_property(
            pos,
            "blaze",
            burner::heat_name(heat),
            crate::glue::server::update_all(),
        );
    }
}

/// `LivingEntity.getSlotForHand`.
fn slot_of(hand: mod_api::Hand) -> mod_api::EquipmentSlot {
    match hand {
        mod_api::Hand::Main => mod_api::EquipmentSlot::MainHand,
        mod_api::Hand::Off => mod_api::EquipmentSlot::OffHand,
    }
}

/// `LitBlazeBurnerBlock.useItemOn`: a shovel puts the flame out (an empty burner, one use of the
/// shovel), and a soul fire base block (soul sand, soul soil) makes a regular flame a soul flame.
/// True when the click was the burner's.
pub fn use_lit_burner(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: PlayerRef,
    hand: mod_api::Hand,
    stack: Option<&ItemStack>,
) -> bool {
    let Some(stack) = stack else {
        return false;
    };
    let held = stacks::id(&*lv.host, stack);
    if held.ends_with("_shovel") {
        lv.play_block_sound("minecraft:block.fire.extinguish", pos, 0.5, 2.0);
        if let Some(living) = lv.host.living(player.entity()) {
            living.hurt_and_break(lv.host, slot_of(hand), 1);
        }
        if let Some(empty) = lv.host.block_state_id("create:blaze_burner") {
            lv.set_state(pos, empty, crate::glue::server::update_all());
        }
        return true;
    }
    let regular = lv
        .block_state(pos)
        .is_some_and(|s| lv.state_property(s, "flame_type").as_deref() == Some("regular"));
    if regular && matches!(held.as_str(), "minecraft:soul_sand" | "minecraft:soul_soil") {
        let pitch = Rng::new(lv.seed_at(pos)).next_float() * 0.4 + 0.8;
        lv.play_block_sound("minecraft:block.soul_sand.place", pos, 1.0, pitch);
        if let Some(soul) = lv
            .host
            .block_state_id("create:lit_blaze_burner[flame_type=soul]")
        {
            lv.set_state(pos, soul, crate::glue::server::update_all());
        }
        return true;
    }
    false
}

/// `BlazeBurnerBlock.useItemOn`: flint and steel lights an empty burner (a lit burner, no
/// blaze); fuel goes to the blaze (`tryInsert` with a player's overflow), one used up outside
/// creative (a lava bucket leaves its bucket). True when the click was the burner's.
pub fn use_burner(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: PlayerRef,
    hand: mod_api::Hand,
    stack: Option<&ItemStack>,
) -> bool {
    let Some((_, info)) = lv.info(pos) else {
        return false;
    };
    // Goggles go on a blaze, and come off with an empty hand (the goggles stay in hand).
    let lit = info.heat.is_some_and(|h| h != Heat::None);
    let held_goggles = stack.is_some_and(|s| stacks::id(&*lv.host, s) == "create:goggles");
    if lit && (held_goggles || stack.is_none()) {
        let wearing = lv
            .entity_at(pos)
            .and_then(|e| e.data(&*lv.host))
            .is_some_and(|d| d.get(&GOGGLES_KEY.into()) == Some(&Tag::Byte(1)));
        if wearing == held_goggles {
            return false;
        }
        let value = held_goggles.then_some(Tag::Byte(1));
        set_shown(lv, pos, &[(GOGGLES_KEY, value)]);
        return true;
    }
    let Some(stack) = stack else {
        return false;
    };
    let held = stacks::id(&*lv.host, stack);
    if info.heat == Some(Heat::None) {
        if held == "minecraft:flint_and_steel" {
            // Flint and steel's sound, one use of it, and `LIT_BLAZE_BURNER.getDefaultState()`.
            let pitch = Rng::new(lv.seed_at(pos)).next_float() * 0.4 + 0.8;
            lv.play_block_sound("minecraft:item.flintandsteel.use", pos, 1.0, pitch);
            if let Some(living) = lv.host.living(player.entity()) {
                living.hurt_and_break(lv.host, slot_of(hand), 1);
            }
            if let Some(lit) = lv.host.block_state_id("create:lit_blaze_burner") {
                lv.set_state(pos, lit, crate::glue::server::update_all());
            }
            return true;
        }
        return false;
    }
    let Some(remainder) = feed_burner(lv, pos, stack, true, false) else {
        return false;
    };
    if !lv.creative(player) && held != "create:creative_blaze_cake" {
        lv.take_held(player, 1);
        if let Some(r) = remainder {
            lv.give(player, r);
        }
    }
    true
}

/// `BlazeBurnerBlock.tryInsert` of one of `stack` into the burner at `pos` (a lit burner or an
/// empty one takes nothing): the creative blaze cake sets the next heat for good, never used
/// up; other fuel goes to the blaze (`tryUpdateFuel`, `overflow` as a player's click). Only
/// looks with `simulate`. None when it is not taken, else what is left of the one used (a lava
/// bucket's bucket).
pub fn feed_burner(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    stack: &ItemStack,
    overflow: bool,
    simulate: bool,
) -> Option<Option<ItemStack>> {
    use burner::Fuel;
    let dim = lv.dim;
    let (_, info) = lv.info(pos)?;
    if info.heat == Some(Heat::None) || info.heat.is_none() {
        return None;
    }
    let held = stacks::id(&*lv.host, stack);
    // `isCreativeFuel`.
    if held == "create:creative_blaze_cake" {
        if simulate {
            return Some(None);
        }
        let heat = with_world(|w| {
            let next = w.dims[dim]
                .burners
                .get_mut(&pos)?
                .creative_fuel(info.heat.unwrap_or(Heat::Smouldering));
            w.mark_unsaved(dim, pos);
            Some(next)
        })
        .flatten()?;
        lv.set_property(
            pos,
            "blaze",
            burner::heat_name(heat),
            crate::glue::server::update_all(),
        );
        sound(
            lv,
            "minecraft:entity.blaze.shoot",
            pos,
            mod_api::SoundSource::Blocks,
            0.2,
            0.6,
        );
        return Some(None);
    }
    if stack.components().is_some() {
        return None;
    }
    // A data map's superheating fuel (the blaze cake), the special tag; else a furnace fuel's
    // burn time, the regular tag (as coal).
    let host = &*lv.host;
    let (fuel, time) = if held == "create:blaze_cake"
        || stacks::item_tagged(host, &held, "create:blaze_burner_fuel/special")
    {
        (Fuel::Special, 3200)
    } else {
        match host.item_burn_time(stack.item()) {
            Some(t) if t > 0 => (Fuel::Normal, t),
            _ if stacks::item_tagged(host, &held, "create:blaze_burner_fuel/regular") => {
                (Fuel::Normal, 1600)
            }
            _ => return None,
        }
    };
    let remainder = host.item_crafting_remainder(stack);
    if simulate {
        let takes = with_world(|w| {
            let mut b = *w.dims[dim].burners.get(&pos)?;
            Some(b.try_fuel(fuel, time, overflow))
        })
        .flatten()?;
        return takes.then_some(remainder);
    }
    let heat = with_world(|w| {
        w.fuel_burner(dim, pos, fuel, time, overflow)
            .then(|| w.dims[dim].burners.get(&pos).map(|b| b.heat()))
            .flatten()
    })
    .flatten()?;
    lv.set_property(
        pos,
        "blaze",
        burner::heat_name(heat),
        crate::glue::server::update_all(),
    );
    Some(remainder)
}
