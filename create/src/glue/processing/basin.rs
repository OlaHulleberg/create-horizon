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
use mod_api::{ItemStack, PlayerRef};
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
/// inventory); a fluid's bucket empties into an input tank when all of it fits
/// (`FluidHelper.tryEmptyItemIntoBE`); an empty bucket takes a bucket's worth from the first tank
/// holding one, the outputs first (`tryFillItemFromBE` on `CombinedTankWrapper(output, input)`).
/// A creative player keeps their bucket and gets none. True when the click was the basin's.
pub fn use_basin(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: PlayerRef,
    stack: Option<&ItemStack>,
) -> bool {
    use create_engine::tank::{BUCKET, Fluid};
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
    let held = stacks::id(&*lv.host, stack);
    let mut tanks = basin_tanks(dim, pos);
    if let Some(fluid) = Fluid::of_bucket(&held) {
        let mut inputs = tanks.input;
        if basin::fill(&mut inputs, fluid, BUCKET) != BUCKET {
            return false;
        }
        tanks.input = inputs;
        with_world(|w| w.set_basin_fluids(dim, pos, tanks));
        if !creative {
            lv.take_held(player, 1);
            if let Some(b) = stacks::stack(&*lv.host, "minecraft:bucket", 1) {
                lv.give(player, b);
            }
        }
        return true;
    }
    if held != "minecraft:bucket" {
        return false;
    }
    let full =
        |t: &[Option<(Fluid, u32)>; 2]| t.iter().position(|s| s.is_some_and(|(_, n)| n >= BUCKET));
    let take = |t: &mut [Option<(Fluid, u32)>; 2]| {
        let i = full(t)?;
        let (f, n) = t[i]?;
        t[i] = (n > BUCKET).then_some((f, n - BUCKET));
        Some(f)
    };
    let Some(fluid) = take(&mut tanks.output).or_else(|| take(&mut tanks.input)) else {
        return false;
    };
    with_world(|w| w.set_basin_fluids(dim, pos, tanks));
    if !creative {
        lv.take_held(player, 1);
        if let Some(b) = stacks::stack(&*lv.host, fluid.bucket(), 1) {
            lv.give(player, b);
        }
    }
    true
}

/// Mechanical mixers: those asked to look at their basin (`updateBasin`: turning, not running,
/// a recipe the basin can do), then the running ones' cycle (`tick`), mixing at the bottom
/// (`applyBasinRecipe`) and going on with the same recipe while it still fits.
pub fn tick_mixers(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].mixer_checks)).unwrap_or_default();
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
                if let (Some(before), Some(i)) = (basin_slots(lv, basin_pos), m.recipe) {
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
                        with_world(|w| w.set_basin_fluids(dim, basin_pos, tanks));
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
            }
            continue;
        }
        if speed == 0.0 || m.running {
            continue;
        }
        let Some(slots) = basin_slots(lv, basin_pos) else {
            continue;
        };
        let heat = basin_heat(lv, basin_pos);
        let host = &*lv.host;
        let limit = |i: &str| stacks::limit(host, i);
        let tanks = basin_tanks(dim, basin_pos);
        let found = recipes::with(|r| {
            basin::matching(&r.mixing, &slots, &tanks, heat, limit)
                .first()
                .copied()
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
    if !basin_at(lv, basin_pos) {
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
    let found = recipes::with(|r| {
        let i = *basin::matching(&r.compacting, &slots, &tanks, heat, limit).first()?;
        Some((i, r.compacting[i].ingredients.len()))
    })
    .flatten();
    // Square crafting recipes of one item (`canCompress`), with the compacting ones: the one
    // with the most ingredients first (`getMatchingRecipes`' order).
    let crafted = crafted
        .into_iter()
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
        with_world(|w| w.set_basin_fluids(dim, basin_pos, tanks));
        write_basin(lv, basin_pos, &before, &slots);
    }
}

/// Burning blaze burners (`BlazeBurnerBlockEntity.tick`): the fuel burns down; a changed heat is
/// written to the block (`blaze`), which fans and basins read.
pub fn tick_burners(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
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

/// `BlazeBurnerBlock.useItemOn`: flint and steel lights an empty burner (a lit burner, no
/// blaze); fuel goes to the blaze (`tryInsert` with a player's overflow), one used up outside
/// creative (a lava bucket leaves its bucket). True when the click was the burner's.
pub fn use_burner(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: PlayerRef,
    stack: Option<&ItemStack>,
) -> bool {
    use burner::Fuel;
    let dim = lv.dim;
    let Some(stack) = stack else {
        return false;
    };
    let Some((_, info)) = lv.info(pos) else {
        return false;
    };
    let held = stacks::id(&*lv.host, stack);
    if info.heat == Some(Heat::None) {
        if held == "minecraft:flint_and_steel" {
            let facing = info.facing.map_or("north", crate::glue::kinetics::dir_name);
            let lit = lv
                .host
                .block_state_id(&format!("create:lit_blaze_burner[facing={facing}]"));
            if let Some(lit) = lit {
                lv.set_state(pos, lit, crate::glue::server::update_all());
                return true;
            }
        }
        return false;
    }
    // `isCreativeFuel`: the creative blaze cake sets the next heat for good, never used up.
    if held == "create:creative_blaze_cake" {
        let Some(heat) = with_world(|w| {
            let next = w.dims[dim].burners.get_mut(&pos)?.creative_fuel();
            w.mark_unsaved(dim, pos);
            Some(next)
        })
        .flatten() else {
            return false;
        };
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
        return true;
    }
    if stack.components().is_some() {
        return false;
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
            _ => return false,
        }
    };
    let remainder = host.item_crafting_remainder(stack);
    let Some(heat) = with_world(|w| {
        w.fuel_burner(dim, pos, fuel, time, true)
            .then(|| w.dims[dim].burners.get(&pos).map(|b| b.heat()))
            .flatten()
    })
    .flatten() else {
        return false;
    };
    lv.set_property(
        pos,
        "blaze",
        burner::heat_name(heat),
        crate::glue::server::update_all(),
    );
    if !lv.creative(player) {
        lv.take_held(player, 1);
        if let Some(r) = remainder {
            lv.give(player, r);
        }
    }
    true
}
