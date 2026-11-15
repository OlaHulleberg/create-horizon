//! The block breakers and the saw: the mechanical drill and saw breaking the block in front
//! (`BlockBreakingKineticBlockEntity`), the saw facing up cutting what lands on it
//! (`SawBlockEntity`), and both hurting what walks into them (`entityInside`), with vanilla's
//! `generic` damage for Create's `mechanical_saw` and `mechanical_drill` (`API_GAPS.md`: a
//! native mod's damage types).

use super::{recipes, stacks};
use crate::glue::logistics::{insert_into_from, random_angle, upright};
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::kinetics::{Axis, Dir};
use create_engine::processing::{Recipe, Rng};
use create_engine::saw::{self, Step};
use create_engine::{Block, belt, drill, sequenced, tree};
use mod_api::{Compound, HostBlockStates, ItemStack, Tag};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Entities inside a saw or drill this tick (dimension, block, entity), for the level's tick.
    static BLADES: RefCell<Vec<(usize, [i32; 3], u32)>> = const { RefCell::new(Vec::new()) };
    /// Each saw's `recipeIndex` while it holds nothing (an empty saw keeps no other state).
    static SAW_INDEX: RefCell<HashMap<(usize, [i32; 3]), usize>> = RefCell::new(HashMap::new());
    /// Each cutting saw's `recipeDuration`, which its clients are told.
    static SAW_DURATION: RefCell<HashMap<(usize, [i32; 3]), f32>> = RefCell::new(HashMap::new());
}

/// Forgets what was kept for the world.
pub fn clear() {
    BLADES.with_borrow_mut(Vec::clear);
    SAW_INDEX.with_borrow_mut(HashMap::clear);
    SAW_DURATION.with_borrow_mut(HashMap::clear);
}

/// Turning drills and sideways saws (`BlockBreakingKineticBlockEntity.tick`): the block in front
/// breaks in steps (`isBreakable`: not air, a fluid, unbreakable or `create:non_breakable`; for a
/// saw also `isSawable`), its cracks showing (`destroyBlockProgress`), its drops left in its
/// middle; a saw fells what stood on it (`TreeCutter`). A breaker that broke one looks again at
/// its next lazy tick (every 10 ticks).
pub fn tick_drills(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let drills = with_world(|w| w.dims[dim].drilling.clone()).unwrap_or_default();
    if drills.is_empty() {
        return;
    }
    let lazy = lv.game_time() % 10 == 0;
    for pos in drills {
        let Some((block, facing)) = lv.info(pos).and_then(|(b, i)| Some((b, i.facing?))) else {
            continue;
        };
        let saw = block == Block::MechanicalSaw;
        if saw && facing.axis() == Axis::Y {
            continue;
        }
        let at = super::step(pos, facing, 1);
        let fluid = lv
            .level
            .fluid_state(&*lv.host, at)
            .is_some_and(|f| f.amount > 0)
            && lv.replaceable(at);
        let hardness = if lv.is_air(at)
            || fluid
            || lv.block_tagged(at, "create:non_breakable")
            || (saw && !sawable(lv, at))
        {
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
        let breaker = breaker_id(pos);
        match step {
            drill::Step::Wait => {}
            drill::Step::Progress(stage) => {
                lv.level
                    .set_break_progress(&mut *lv.host, breaker, at, stage);
            }
            drill::Step::Reset => {
                lv.level.set_break_progress(&mut *lv.host, breaker, at, -1);
            }
            drill::Step::Broken => {
                lv.level.set_break_progress(&mut *lv.host, breaker, at, -1);
                let broken = cell(lv, at);
                // `onBlockBroken`: the drops left at the block's middle, standing still.
                let centre = at.map(|v| f64::from(v) + 0.5);
                for s in destroy_collect(lv, at) {
                    lv.spawn_stack(centre, s, [0.0; 3]);
                }
                if saw {
                    fell(lv, pos, at, broken);
                }
            }
        }
    }
}

/// A machine's stable negative breaker id for its cracks (`NEXT_BREAKER_ID` in Create).
fn breaker_id(pos: [i32; 3]) -> i32 {
    let h = (pos[0].wrapping_mul(73_856_093) ^ pos[1].wrapping_mul(19_349_663))
        ^ pos[2].wrapping_mul(83_492_791);
    -((h & 0x3fff_ffff) + 1)
}

/// Breaks the block at `at` with its drops, which come back rather than falling as items.
fn destroy_collect(lv: &mut ServerLevel<'_>, at: [i32; 3]) -> Vec<ItemStack> {
    lv.level
        .destroy_block(&mut *lv.host, at, true, None, true)
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// `SawBlockEntity.isSawable`: logs, leaves, roots, bamboo, cactus, sugar cane, kelp, chorus
/// plants and `create:solid_crops` (pumpkins, melons); not saplings.
fn sawable(lv: &ServerLevel<'_>, at: [i32; 3]) -> bool {
    if lv.block_tagged(at, "minecraft:saplings") {
        return false;
    }
    let c = cell(lv, at);
    c.log
        || c.root
        || c.vertical
        || lv.block_tagged(at, "minecraft:leaves")
        || lv.block_tagged(at, "create:solid_crops")
        || lv.block_name(at).as_deref() == Some("minecraft:chorus_plant")
}

/// What `TreeCutter` reads of the block at `p`.
fn cell(lv: &ServerLevel<'_>, p: [i32; 3]) -> tree::Cell {
    let name = lv.block_name(p).unwrap_or_default();
    let leaf = if name == "minecraft:scaffolding" {
        None
    } else {
        lv.property(p, "distance").and_then(|d| d.parse().ok())
    };
    tree::Cell {
        log: lv.block_tagged(p, "minecraft:logs")
            || lv.block_tagged(p, "create:slimy_logs")
            || name == "minecraft:mushroom_stem",
        root: lv.block_tagged(p, "create:roots"),
        leaf,
        vertical: matches!(
            name.as_str(),
            "minecraft:bamboo"
                | "minecraft:cactus"
                | "minecraft:sugar_cane"
                | "minecraft:kelp_plant"
                | "minecraft:kelp"
        ),
        chorus: matches!(
            name.as_str(),
            "minecraft:chorus_plant" | "minecraft:chorus_flower"
        ),
        attachment: lv.block_tagged(p, "create:tree_attachments"),
        non_decaying: match name.as_str() {
            "minecraft:red_mushroom_block" => 2,
            "minecraft:brown_mushroom_block"
            | "minecraft:weeping_vines"
            | "minecraft:weeping_vines_plant" => 3,
            _ if lv.block_tagged(p, "minecraft:wart_blocks") => 3,
            _ => -1,
        },
    }
}

/// `TreeCutter.findTree(..).destroyBlocks(.., dropItemFromCutTree)`: what stood on the block the
/// saw at `saw` cut at `at` falls, each block's drops flying on from its middle the way the saw
/// faces, the faster the farther it was.
fn fell(lv: &mut ServerLevel<'_>, saw: [i32; 3], at: [i32; 3], broken: tree::Cell) {
    let (min_y, height) = lv.level.height(&*lv.host).unwrap_or((-64, 384));
    let top = min_y + height;
    let found = tree::find(at, broken, height, |p| {
        if p[1] < min_y || p[1] >= top || !lv.is_loaded(p) {
            return tree::Cell {
                non_decaying: -1,
                ..tree::Cell::default()
            };
        }
        cell(lv, p)
    });
    let push = [at[0] - saw[0], at[1] - saw[1], at[2] - saw[2]].map(f64::from);
    for (p, _) in found.blocks() {
        if lv.is_air(p) {
            continue;
        }
        let distance = (0..3)
            .map(|i| f64::from(p[i] - at[i]).powi(2))
            .sum::<f64>()
            .sqrt();
        let centre = p.map(|v| f64::from(v) + 0.5);
        let v = push.map(|c| c * distance / 20.0);
        for s in destroy_collect(lv, p) {
            lv.spawn_stack(centre, s, v);
        }
    }
}

/// `SawBlock.updateEntityAfterFallOn` and `SawBlockEntity.insertItem` for an item landing on a
/// turning saw facing up, while it holds nothing: it takes one (`ProcessingInventory`'s slot
/// limit without `bulkCutting`, which is off by default); `stack` keeps what is left.
pub fn saw_take(lv: &mut ServerLevel<'_>, pos: [i32; 3], stack: &mut Option<ItemStack>) {
    if lv.info(pos).and_then(|(_, i)| i.facing) != Some(Dir::Up) || lv.speed(pos) == 0.0 {
        return;
    }
    let Some(s) = stack.as_ref() else {
        return;
    };
    let item = stacks::id(&*lv.host, s);
    let progress = stacks::sequenced_progress(s);
    if saw_put(lv, pos, &item, progress) {
        *stack = stack.take().and_then(|s| s.shrink(1));
    }
}

/// One `item` (a sequenced assembly's `progress` with it) into the saw at `pos` while it holds
/// nothing (`ProcessingInventory.insertItem` into slot 0). Whether it took it.
pub fn saw_put(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    item: &str,
    progress: Option<create_engine::sequenced::Progress>,
) -> bool {
    let dim = lv.dim;
    let took = with_world(|w| {
        let s = w.dims[dim].saws.entry(pos).or_default();
        if !s.slots.is_empty() {
            return false;
        }
        s.slots = vec![(item.to_owned(), 1)];
        s.remaining = -1.0;
        s.applied = false;
        // `recipeIndex` stays with the saw between items.
        s.index = SAW_INDEX.with_borrow(|m| m.get(&(dim, pos)).copied().unwrap_or(0));
        s.progress = progress;
        true
    })
    .unwrap_or(false);
    if took {
        show_saw(lv, pos);
    }
    took
}

/// Saws holding items (`SawBlockEntity.tick`): their cutting recipe in turn, applied near the
/// end, the results thrown off the way the saw turns (onto a belt or depot there, else as items).
pub fn tick_saws(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    // `destroy`: a broken saw's items drop where it was.
    let drops = with_world(|w| std::mem::take(&mut w.dims[dim].saw_drops)).unwrap_or_default();
    for (pos, item, n) in drops {
        let at = pos.map(|v| f64::from(v) + 0.5);
        lv.spawn_moving(at, &item, u32::from(n), [0.0; 3]);
    }
    let saws: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].saws.keys().copied().collect()).unwrap_or_default();
    if saws.is_empty() || !recipes::ready() {
        return;
    }
    for pos in saws {
        if tick_saw(lv, pos).is_some_and(|s| s != Step::Wait) {
            show_saw(lv, pos);
        }
    }
}

/// One saw's `tick` (what it did; None when it is gone).
fn tick_saw(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> Option<Step> {
    let dim = lv.dim;
    {
        let along_first = lv.info(pos).map(|(_, i)| i.along_first)?;
        let speed = lv.speed(pos);
        let mut saw = with_world(|w| w.dims[dim].saws.get(&pos).cloned()).flatten()?;
        let item = saw.slots.first().map(|s| s.0.clone()).unwrap_or_default();
        let step = saw.tick(speed);
        match step {
            Step::Wait => {}
            Step::Start => {
                let filter = super::filtering::filter_at(dim, pos);
                let host = &*lv.host;
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
                    // `outputMatchesFilter`.
                    let list: Vec<&Recipe> = r
                        .cutting
                        .matching(&item)
                        .into_iter()
                        .map(|i| r.cutting.get(i))
                        .filter(|x| passes(host, filter.as_ref(), x))
                        .collect();
                    saw.start(&list);
                });
            }
            Step::Apply => {
                let mut rng = Rng::new(lv.seed_at(pos));
                let filter = super::filtering::filter_at(dim, pos);
                let host = &*lv.host;
                let applied = recipes::with(|r| {
                    if let Some((a, _)) = sequenced::step_for(
                        &r.sequenced,
                        &item,
                        saw.progress.as_ref(),
                        "create:cutting",
                    ) {
                        saw.apply_assembly(a, &mut rng);
                        return true;
                    }
                    let list: Vec<&Recipe> = r
                        .cutting
                        .matching(&item)
                        .into_iter()
                        .map(|i| r.cutting.get(i))
                        .filter(|x| passes(host, filter.as_ref(), x))
                        .collect();
                    let recipe = list.get(saw.index).copied();
                    // The input's crafting remainder comes out with each roll.
                    let left = stacks::remainder(host, &item);
                    saw.apply(
                        recipe,
                        |i: &str| stacks::limit(host, i),
                        &mut rng,
                        left.as_deref(),
                    );
                    !list.is_empty()
                });
                if applied == Some(true) {
                    crate::glue::advancements::award_placer(lv, pos, "saw_processing");
                }
            }
            Step::Out => {
                let m = saw::movement(along_first, speed);
                // `tryExportingToBeltFunnel`: a pulling belt funnel on top facing against the
                // items' way takes them first, a slot a tick; one that takes none of a slot
                // holds the saw.
                let moving = Dir::ALL.into_iter().find(|d| d.offset() == m);
                if let Some(moving) = moving {
                    for i in 0..saw.slots.len() {
                        let (item, count) = saw.slots[i].clone();
                        let Some(stack) = stacks::stack(&*lv.host, &item, u32::from(count)) else {
                            continue;
                        };
                        let Some(left) =
                            crate::glue::logistics::export_up(lv, pos, moving.opposite(), &stack)
                        else {
                            break;
                        };
                        if left == u32::from(count) {
                            return Some(Step::Wait);
                        }
                        if left == 0 {
                            saw.slots.remove(i);
                        } else {
                            saw.slots[i].1 = left.min(255) as u8;
                        }
                        if saw.slots.is_empty() {
                            saw.clear();
                            SAW_INDEX.with_borrow_mut(|m| m.insert((dim, pos), saw.index));
                        }
                        with_world(|w| {
                            if saw.slots.is_empty() {
                                w.dims[dim].saws.remove(&pos);
                            } else {
                                w.dims[dim].saws.insert(pos, saw);
                            }
                        });
                        return Some(Step::Out);
                    }
                }
                // A belt, depot or machine it turns toward takes the results
                // (`DirectBeltInputBehaviour` from that side); what it doesn't take, or all of
                // them while it takes nothing from that side, waits in the saw.
                let next = [pos[0] + m[0], pos[1], pos[2] + m[2]];
                let facing = Dir::ALL.into_iter().find(|d| d.offset() == m);
                let facing = facing.filter(|_| crate::glue::logistics::direct_input_at(lv, next));
                if let Some(facing) = facing {
                    if !crate::glue::logistics::direct_input_accepts(lv, next, facing) {
                        return Some(Step::Wait);
                    }
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
                    return Some(Step::Out);
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
        if step == Step::Start {
            // `recipeDuration`: what it starts with.
            SAW_DURATION.with_borrow_mut(|m| m.insert((dim, pos), saw.remaining));
        }
        with_world(|w| {
            if saw.slots.is_empty() {
                w.dims[dim].saws.remove(&pos);
            } else {
                w.dims[dim].saws.insert(pos, saw);
            }
        });
        Some(step)
    }
}

/// `RecipeConditions.outputMatchesFilter`: the saw's filter on a recipe's first result.
fn passes(host: &dyn mod_api::Host, f: Option<&create_engine::filter::Filter>, r: &Recipe) -> bool {
    super::filtering::result_passes(host, f, r.results.first().map(|o| o.item.as_str()), None)
}

/// The key of an upward saw's block entity data its clients draw its items from
/// (`SawRenderer.renderItems`): `Items` (`id`, `count`), `R` its remaining time and `D` its
/// duration when told, `A` whether it has cut them, `S` the game time it was told at.
pub const SAW_KEY: &str = "SawItems";

/// Tells the clients of the saw at `pos` what it holds and how far it is (`sendData`).
fn show_saw(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let saw = with_world(|w| w.dims[dim].saws.get(&pos).cloned()).flatten();
    let value = saw.filter(|s| !s.slots.is_empty()).map(|s| {
        let duration = if s.applied {
            20.0
        } else if s.remaining < 0.0 {
            0.0
        } else {
            SAW_DURATION.with_borrow(|m| m.get(&(dim, pos)).copied().unwrap_or(0.0))
        };
        let items = s
            .slots
            .iter()
            .map(|(id, n)| {
                Tag::Compound(Compound::from([
                    ("id".into(), Tag::String(id.as_str().into())),
                    ("count".into(), Tag::Int(i32::from(*n))),
                ]))
            })
            .collect();
        Tag::Compound(Compound::from([
            ("Items".into(), Tag::List(items)),
            ("R".into(), Tag::Float(s.remaining.to_bits())),
            ("D".into(), Tag::Float(duration.to_bits())),
            ("A".into(), Tag::Byte(i8::from(s.applied))),
            ("S".into(), Tag::Long(lv.game_time())),
        ]))
    });
    // An empty saw's is taken away (its clients see it absent: `kinetics::synced`).
    super::basin::set_shown(lv, pos, &[(SAW_KEY, value)]);
}

/// The saw and the drill as blocks: what walks into their blades.
pub struct Blades;

#[mod_api::horizon_part]
impl mod_api::BlockPart for Blades {
    /// `SawBlock.entityInside` and `DrillBlock.entityInside`: the entity (not an item), for the
    /// level's tick (its box can't be read inside its move).
    fn entity_inside(
        &self,
        _level: &mut dyn mod_api::LevelRandomHost,
        event: &mut mod_api::EntityInsideEvent<'_>,
    ) {
        if event.kind == "minecraft:item" {
            return;
        }
        let Some(dim) = crate::glue::world::level_index(event.dimension) else {
            return;
        };
        let pos = [event.pos.x, event.pos.y, event.pos.z];
        BLADES.with_borrow_mut(|b| b.push((dim, pos, event.entity as u32)));
    }
}

/// The entities that walked into a turning saw or drill this tick: hurt by its speed
/// (`DrillBlock.getDamage`) when their box meets the block shrunk by 0.1. Create's
/// `mechanical_saw` and `mechanical_drill` damage types are stood in for by vanilla's `generic`
/// (`API_GAPS.md`: a native mod's damage types).
pub fn hurt_inside(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let inside: Vec<_> = BLADES.with_borrow_mut(|b| {
        let (here, rest): (Vec<_>, Vec<_>) = b.drain(..).partition(|(d, _, _)| *d == dim);
        *b = rest;
        here
    });
    for (_, pos, id) in inside {
        if !matches!(
            lv.info(pos).map(|(b, _)| b),
            Some(Block::MechanicalSaw | Block::MechanicalDrill)
        ) {
            continue;
        }
        let speed = lv.speed(pos);
        if speed == 0.0 {
            continue;
        }
        let Some(entity) = lv.host.entity(id) else {
            continue;
        };
        let Some([min, max]) = entity.bounding_box(&*lv.host) else {
            continue;
        };
        if drill::inside(pos, min, max) {
            let _ = entity.hurt(
                &mut *lv.host,
                "minecraft:generic",
                drill::damage(speed),
                None,
                None,
            );
        }
    }
}
