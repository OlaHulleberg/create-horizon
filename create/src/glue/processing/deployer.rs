//! The deployer (`DeployerBlockEntity`, `DeployerHandler`): its stroke, its recipes over a
//! depot or a belt (`BeltDeployerCallbacks`), and its fake player (`DeployerFakePlayer`) using
//! what it holds on the block it reaches, or punching what is there.
//!
//! The fake player is the game's (`ServerHost::fake_player`), acting with the deployer's held
//! stack for one action at a time (`perform_action_scoped`): an entity in the block it reaches is
//! used first (`interactOn`: shears on a sheep, a bucket on a cow, a name tag), then its click is
//! the game's own path (`ServerPlayerGameMode.useItemOn`: the block's use, then the item's
//! `useOn`, which places a block item, tills with a hoe...), then the item's use in the air
//! (`item.use`: a bucket fills or empties, a bottle fills, a snowball is thrown); its punch the
//! game's attack and block break. Sand paper polishes what lies under it. A player in front
//! whose interaction passed is fed what it holds, if it is a food they can eat or a deployable
//! drink (`finishUsingItem` on them, `finish_using`). Left out: the slower digging of a block whose drops need another tool
//! (`requiresCorrectToolForDrops`), the API having no query for it, so every block digs at its
//! harvestable speed.

use super::takes_from_side;
use super::{press, recipes, stacks};
use crate::glue::net;
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::Block;
use create_engine::deployer::{self, Deployer};
use create_engine::kinetics::Dir;
use create_engine::{belt, processing};
use mod_api::{
    BlockClick, BlockHit, Hand, ItemStack, PlayerAction, PlayerActionSetup, PlayerRef,
    RandomSource, ServerHost,
};

/// `DeployerFakePlayer`'s fallback profile: `9e2faded-cafe-4ec2-c314-dad129ae971d`, "Deployer".
const FAKE_UUID: u128 = 0x9e2f_aded_cafe_4ec2_c314_dad1_29ae_971d;
const FAKE_NAME: &str = "Deployer";

/// `minecraft:item`, for the crumbs of what a deployer feeds.
pub(crate) static ITEM_PARTICLE: std::sync::OnceLock<mod_api::ParticleKey> =
    std::sync::OnceLock::new();

/// The deployer two above `depot`, facing down, and its speed; None for anything else.
pub fn deployer_above(lv: &ServerLevel<'_>, depot: [i32; 3]) -> Option<(Deployer, f32)> {
    let above = [depot[0], depot[1] + 2, depot[2]];
    if lv.facing_of(above, Block::Deployer) != Some(Dir::Down) {
        return None;
    }
    let dim = lv.dim;
    let d = with_world(|w| w.dims[dim].deployers.get(&above).cloned()).flatten()?;
    Some((d, lv.speed(above)))
}

/// The deployer's recipe for `item` with what it holds (`DeployerBlockEntity.getRecipe`): sand
/// paper polishes first (`SANDPAPER_POLISHING`), then a sequenced assembly's deploying step,
/// else a deploying or item application recipe.
fn recipe_for(
    item: &str,
    progress: Option<&create_engine::sequenced::Progress>,
    tool: &str,
) -> (
    Option<create_engine::sequenced::Assembly>,
    Option<deployer::Recipe>,
) {
    if matches!(tool, "create:sand_paper" | "create:red_sand_paper") {
        let polishing = recipes::with(|r| {
            let id = r.polishing.find_single(item)?;
            let p = r.polishing.get(id);
            Some(deployer::Recipe {
                id: p.id.clone(),
                target: p.ingredients.first().cloned().unwrap_or_default(),
                tool: vec![tool.to_owned()],
                results: p.results.clone(),
                keep_held: false,
            })
        })
        .flatten();
        if polishing.is_some() {
            return (None, polishing);
        }
    }
    let sequenced = press::sequenced_for(item, progress, "create:deploying", Some(tool));
    let recipe = match &sequenced {
        Some((a, s)) => Some(deployer::Recipe {
            id: a.id.clone(),
            target: Vec::new(),
            tool: s.with.clone().unwrap_or_default(),
            results: Vec::new(),
            keep_held: s.keep_held,
        }),
        None => recipes::with(|r| deployer::find(&r.deploying, item, tool).cloned()).flatten(),
    };
    (sequenced.map(|(a, _)| a), recipe)
}

/// `RecipeApplier.applyRecipeOn` on one of the stack, `getMaxOutputCount` 4.
fn rolled(lv: &ServerLevel<'_>, at: [i32; 3], r: &deployer::Recipe) -> Vec<(String, u8)> {
    let seed = lv.game_time() as u64 ^ (at[0] as u64).wrapping_mul(0x9E37_79B9);
    let p = processing::Recipe {
        id: r.id.clone(),
        ingredients: Vec::new(),
        results: r.results.clone(),
        time: 0,
    };
    processing::roll(&p, &mut processing::Rng::new(seed))
        .into_iter()
        .take(4)
        .collect()
}

/// The deployer's stroke starting toward a depot or belt: the ray from its face to the top of
/// the block two ahead (`start`).
fn start_over(lv: &ServerLevel<'_>, d: &mut Deployer, at: [i32; 3]) {
    let top = lv.outline_top(at);
    d.start((1.0 - top) as f32);
    let above = [at[0], at[1] + 2, at[2]];
    let dim = lv.dim;
    with_world(|w| {
        let dd = &mut w.dims[dim];
        dd.deployers.insert(above, d.clone());
        if !dd.deploying.contains(&above) {
            dd.deploying.push(above);
        }
        dd.deployer_changes.push(above);
    });
}

/// `BeltDeployerCallbacks` for the depot at `depot`: true when a deployer above holds its stack
/// (the depot asks again next tick).
pub fn deployer_on_depot(lv: &mut ServerLevel<'_>, depot: [i32; 3]) -> bool {
    use deployer::Depot;
    let dim = lv.dim;
    let Some((mut d, rpm)) = deployer_above(lv, depot) else {
        return false;
    };
    let above = [depot[0], depot[1] + 2, depot[2]];
    let Some(mut held) =
        with_world(|w| w.dims[dim].depots.get(&depot).copied().flatten()).flatten()
    else {
        return false;
    };
    if held.sliding() && held.position < 0.5 - 1.0 / 16.0 {
        return false;
    }
    let Some(stack) = lv.slot(depot, 0) else {
        return false;
    };
    let item = stacks::id(&*lv.host, &stack);
    let count = stack.count().min(255) as u8;
    if !recipes::ready() {
        return false;
    }
    let tool = d.held.as_ref().map(|h| h.0.clone()).unwrap_or_default();
    let sequenced = press::sequenced_on_depot(lv, depot, "create:deploying", Some(&tool));
    let (_, recipe) = recipe_for(&item, stacks::sequenced_progress(&stack).as_ref(), &tool);
    let hold = match deployer::on_depot(&d, rpm, held.locked, recipe.is_some()) {
        Depot::Pass => {
            held.locked = false;
            false
        }
        Depot::Hold => {
            held.locked = true;
            true
        }
        Depot::Start => {
            start_over(lv, &mut d, depot);
            held.locked = true;
            true
        }
        Depot::Apply => {
            let Some(r) = recipe else {
                return false;
            };
            held = match &sequenced {
                Some((a, progress, _)) => {
                    press::sequenced_step_on_depot(lv, depot, count, held, a, progress.as_ref())
                }
                None => {
                    let results = rolled(lv, depot, &r);
                    press::results_on_depot(lv, depot, count, held, results)
                }
            };
            held.locked = true;
            deployer_used(lv, &mut d, &r, above);
            with_world(|w| w.set_deployer(dim, above, d.clone()));
            true
        }
    };
    with_world(|w| {
        if let Some(h) = w.dims[dim].depots.get_mut(&depot) {
            *h = h.map(|_| held);
        }
    });
    hold
}

/// `BeltDeployerCallbacks` for a stack on the belt at `at` under the deployer `d` (facing down,
/// at `rpm`): the deployer starts its stroke on a stack its held item applies to (a sequenced
/// assembly's deploying step first) and, back at the top of it, turns one of the stack into the
/// results (`activate`).
pub fn deployer_on_belt(
    lv: &mut ServerLevel<'_>,
    at: [i32; 3],
    item: &mut belt::Item,
    locked: bool,
    mut d: Deployer,
    rpm: f32,
) -> belt::Process {
    use belt::Process;
    use deployer::Depot;
    let dim = lv.dim;
    let above = [at[0], at[1] + 2, at[2]];
    if !recipes::ready() {
        return Process::Pass;
    }
    let tool = d.held.as_ref().map(|h| h.0.clone()).unwrap_or_default();
    let (sequenced, recipe) = recipe_for(
        create_engine::arm::item_id(&item.item),
        item.progress.as_ref(),
        &tool,
    );
    match deployer::on_depot(&d, rpm, locked, recipe.is_some()) {
        Depot::Pass => Process::Pass,
        Depot::Hold => Process::Hold,
        Depot::Start => {
            start_over(lv, &mut d, at);
            Process::Hold
        }
        Depot::Apply => {
            let Some(r) = recipe else {
                return Process::Pass;
            };
            let outputs = match &sequenced {
                Some(a) => press::sequenced_on_belt(lv, at, item, a),
                None => rolled(lv, at, &r)
                    .into_iter()
                    .map(|(name, n)| {
                        let mut out = item.clone();
                        out.item = name;
                        out.count = n;
                        out.progress = None;
                        out.fan = Default::default();
                        out
                    })
                    .collect(),
            };
            let outputs = press::on_belt(lv, at, outputs);
            deployer_used(lv, &mut d, &r, above);
            with_world(|w| w.set_deployer(dim, above, d.clone()));
            Process::Convert {
                held: press::belt_left(item),
                outputs,
            }
        }
    }
}

/// The deployer at `pos` after applying `recipe` (`activate`): its tool wears, other items are
/// used up (`Deployer::use_for`); a crafting remainder that finds no room in the hand goes into
/// its overflow (`overflowItems`).
fn deployer_used(
    lv: &mut ServerLevel<'_>,
    d: &mut Deployer,
    recipe: &deployer::Recipe,
    pos: [i32; 3],
) {
    crate::glue::advancements::deployer_applied(lv, pos, recipe.results.first().map(|o| &*o.item));
    let held = held_of(lv, d);
    let host = &*lv.host;
    let max_damage = held
        .as_ref()
        .and_then(|s| host.max_damage(s.item()))
        .unwrap_or(0);
    let remainder = held
        .as_ref()
        .and_then(|s| host.item_crafting_remainder(s))
        .map(|s| stacks::id(host, &s));
    if let Some(rest) = d.use_for(recipe, max_damage, remainder.as_deref())
        && let Some(s) = stacks::stack(&*lv.host, &rest, 1)
    {
        keep_overflow(lv, pos, vec![s]);
    }
    // `BeltDeployerCallbacks.activate`: the held item breaking, an item picked up, and sand
    // paper's short sanding.
    if d.held.is_none() {
        lv.play_block_sound("minecraft:entity.item.break", pos, 0.25, 1.0);
    }
    lv.play_block_sound("minecraft:entity.item.pickup", pos, 0.25, 0.75);
    if recipe
        .tool
        .iter()
        .any(|t| matches!(t.as_str(), "create:sand_paper" | "create:red_sand_paper"))
    {
        lv.play_block_sound("create:sanding_short", pos, 0.35, 1.0);
    }
}

fn centre(pos: [i32; 3]) -> [f64; 3] {
    pos.map(|v| f64::from(v) + 0.5)
}

/// The deployer's held stack as an item stack, with its components and its wear.
fn held_of(lv: &ServerLevel<'_>, d: &Deployer) -> Option<ItemStack> {
    let held = d.held.as_ref()?;
    let mut s = stacks::stack(&*lv.host, &held.0, u32::from(held.1))?;
    if !d.held_components.is_empty()
        && let Ok((mod_api::Tag::Compound(c), _)) =
            mod_api::Tag::from_network_bytes(&d.held_components, None)
    {
        let _ = s.set_components(c);
    }
    if d.held_damage > 0 {
        s.set_damage(d.held_damage);
    }
    Some(s)
}

/// The deployer's hand takes `stack` (its id, count, wear and the rest of its components).
pub(crate) fn set_held_stack(
    host: &dyn mod_api::Host,
    d: &mut Deployer,
    stack: Option<&ItemStack>,
) {
    d.held = stack.map(|s| (stacks::id(host, s), s.count().min(64) as u8));
    d.held_damage = stack.map_or(0, ItemStack::damage);
    d.held_components = stack
        .and_then(|s| s.components())
        .and_then(|c| {
            mod_api::Tag::Compound(c.as_nbt().clone())
                .to_network_bytes()
                .ok()
        })
        .unwrap_or_default();
}

/// Deployers mid-stroke (`DeployerBlockEntity.tick`): the stroke runs; one back at rest has the
/// depot below look at it again. Nothing runs without one.
pub fn tick_deployers(lv: &mut ServerLevel<'_>) {
    use deployer::{Mode, State, Tick};
    let dim = lv.dim;
    // `redstoneUpdate`: powered, it is locked.
    let power =
        with_world(|w| std::mem::take(&mut w.dims[dim].deployer_power_checks)).unwrap_or_default();
    for pos in power {
        if lv.facing_of(pos, Block::Deployer).is_none() {
            continue;
        }
        let powered = lv.has_neighbor_signal(pos);
        with_world(|w| {
            if let Some(d) = w.dims[dim].deployers.get_mut(&pos)
                && d.locked != powered
            {
                d.locked = powered;
                w.mark_unsaved(dim, pos);
                // A depot below looks at it again.
                w.dims[dim].depot_work.push([pos[0], pos[1] - 2, pos[2]]);
            }
        });
    }
    // Deployers at rest, turning, not locked: `tick`'s `WAITING` start (`shouldActivate` when
    // using), else a look again `getTimerSpeed() × 10` later. Using ones over a depot or belt
    // wait for it.
    let idle: Vec<([i32; 3], f32, Mode)> = with_world(|w| {
        let d = &w.dims[dim];
        d.deployers
            .iter()
            .filter(|(p, dp)| !dp.busy() && !dp.locked && !d.deploying.contains(p))
            .map(|(p, dp)| (*p, w.speed(dim, *p).unwrap_or(0.0), dp.mode))
            .filter(|(_, rpm, _)| *rpm != 0.0)
            .collect()
    })
    .unwrap_or_default();
    for (pos, rpm, mode) in idle {
        let Some(facing) = lv.facing_of(pos, Block::Deployer) else {
            continue;
        };
        // `WAITING`: while it holds overflow it waits for a funnel or hopper to take it.
        if has_overflow(lv, pos) {
            with_world(|w| {
                let d = &mut w.dims[dim];
                if let Some(dp) = d.deployers.get_mut(&pos) {
                    dp.wait(rpm);
                    d.deploying.push(pos);
                }
            });
            continue;
        }
        // A held stack its filter doesn't let through leaves its hand into the overflow.
        if let Some(f) = super::filtering::filter_at(dim, pos)
            && let Some(mut d) = with_world(|w| w.dims[dim].deployers.get(&pos).cloned()).flatten()
            && let Some(held) = d.held.clone()
            && !crate::glue::items::filters::passes_item(&*lv.host, Some(&f), &held.0)
        {
            let stack = held_of(lv, &d);
            set_held_stack(&*lv.host, &mut d, None);
            with_world(|w| w.set_deployer(dim, pos, d));
            keep_overflow(lv, pos, stack.into_iter().collect());
            continue;
        }
        if mode == Mode::Punch && super::fist::start(lv, pos, facing) {
            continue;
        }
        let below = super::step(pos, facing, 2);
        if facing == Dir::Down && takes_from_side(dim, below) && mode == Mode::Use {
            continue;
        }
        let place = if mode == Mode::Punch {
            Some(below)
        } else {
            deployer_place_target(lv, pos, facing)
                .or_else(|| deployer_uses(lv, pos, facing).then_some(super::step(pos, facing, 1)))
        };
        let reach = if lv.replaceable(super::step(pos, facing, 1)) {
            0.75
        } else {
            0.0
        };
        with_world(|w| {
            let d = &mut w.dims[dim];
            let Some(dp) = d.deployers.get_mut(&pos) else {
                return;
            };
            match place {
                // `start`: the ray toward the block two ahead sets its reach.
                Some(_) => {
                    dp.start(reach);
                    d.deployer_changes.push(pos);
                }
                None => dp.wait(rpm),
            }
            d.deploying.push(pos);
        });
    }
    let Some(list) = with_world(|w| w.dims[dim].deploying.clone()) else {
        return;
    };
    for pos in list {
        deployer_lost_block(lv, pos);
        let rpm = lv.speed(pos);
        let activated = with_world(|w| {
            let d = &mut w.dims[dim];
            let Some(dp) = d.deployers.get_mut(&pos) else {
                d.deploying.retain(|p| *p != pos);
                return false;
            };
            let tick = dp.tick(rpm);
            if tick != Tick::None {
                d.deployer_changes.push(pos);
                if dp.state == State::Waiting {
                    d.depot_work.push([pos[0], pos[1] - 2, pos[2]]);
                }
            }
            if !dp.busy() {
                d.deploying.retain(|p| *p != pos);
            }
            tick == Tick::Activate
        })
        .unwrap_or(false);
        if activated {
            super::fist::trigger(lv, pos);
            deployer_activate(lv, pos);
        }
    }
    let Some((changes, drops, cracks)) = with_world(|w| {
        let d = &mut w.dims[dim];
        let mut c = std::mem::take(&mut d.deployer_changes);
        c.sort();
        c.dedup();
        (
            c,
            std::mem::take(&mut d.deployer_drops),
            std::mem::take(&mut d.deployer_cracks),
        )
    }) else {
        return;
    };
    // `DeployerFakePlayer.remove`: a removed deployer's cracks go.
    for (pos, at) in cracks {
        let _ = lv
            .level
            .set_break_progress(&mut *lv.host, deployer::breaker_id(pos), at, -1);
    }
    for (pos, (item, n)) in drops {
        lv.spawn_items(centre(pos), &item, u32::from(n));
    }
    if !changes.is_empty() {
        let players = lv.players();
        send_deployers(lv, &players, &changes);
    }
}

/// Where the block a deployer holds would go (`placement`), as a position; None when it holds
/// no block, or nowhere fits.
fn deployer_place_target(lv: &ServerLevel<'_>, pos: [i32; 3], facing: Dir) -> Option<[i32; 3]> {
    let dim = lv.dim;
    let held =
        with_world(|w| w.dims[dim].deployers.get(&pos).and_then(|d| d.held.clone())).flatten()?;
    // A block item (`BlockItem.getBlock`): the block it places.
    let placed = lv.host.item_block(stacks::key(&*lv.host, &held.0)?)?;
    let reached = super::step(pos, facing, 2);
    let before = super::step(pos, facing, 1);
    let is_held = lv.level.block(&*lv.host, reached) == Some(placed);
    deployer::placement(lv.replaceable(reached), lv.replaceable(before), is_held)
        .map(|at_reached| if at_reached { reached } else { before })
}

/// `shouldActivate` for a held item that is not a block: always, except a bucket of a fluid
/// whose source is already in the block it reaches.
fn deployer_uses(lv: &ServerLevel<'_>, pos: [i32; 3], facing: Dir) -> bool {
    let dim = lv.dim;
    // An empty hand always (it uses the block it reaches).
    let Some((item, _)) =
        with_world(|w| w.dims[dim].deployers.get(&pos).and_then(|d| d.held.clone())).flatten()
    else {
        return true;
    };
    if stacks::key(&*lv.host, &item).is_some_and(|k| lv.host.item_block(k).is_some()) {
        return false;
    }
    let reached = super::step(pos, facing, 2);
    let fluid = match item.as_str() {
        "minecraft:water_bucket" => "minecraft:water",
        "minecraft:lava_bucket" => "minecraft:lava",
        _ => return true,
    };
    lv.block_name(reached).as_deref() != Some(fluid)
        || lv.property(reached, "level").is_some_and(|l| l != "0")
}

/// `DeployerBlockEntity.activate`: nothing over a depot or belt (their callbacks do the work);
/// punching, `punch`; else the fake player clicks the block it reaches with what it holds
/// (`activateInner` in `Mode.USE`), and the hand keeps what is left of the stack.
fn deployer_activate(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(facing) = lv.facing_of(pos, Block::Deployer) else {
        return;
    };
    let reached = deployer::clicked(pos, facing);
    if facing == Dir::Down && takes_from_side(dim, reached) {
        return;
    }
    let Some(mut d) = with_world(|w| w.dims[dim].deployers.get(&pos).cloned()).flatten() else {
        return;
    };
    let before = d.clone();
    let other = activate(lv, pos, facing, &mut d, deployer::breaker_id(pos));
    crate::glue::advancements::award_placer(lv, pos, "deployer");
    if d != before {
        with_world(|w| w.set_deployer(dim, pos, d));
    }
    keep_overflow(lv, pos, other);
}

/// The slots a deployer keeps its fake player's other items in (`overflowItems`), for funnels
/// and hoppers to take; it waits while they hold any.
pub const OVERFLOW_SLOTS: usize = 11;

/// Whether the deployer at `pos` holds overflow.
fn has_overflow(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    lv.slots(pos, OVERFLOW_SLOTS)
        .is_some_and(|s| s.iter().any(Option::is_some))
}

/// `overflowItems.add`: `stacks` go into the deployer at `pos`'s overflow slots, onto stacks
/// of their kind first; what finds no slot drops at the deployer.
fn keep_overflow(lv: &mut ServerLevel<'_>, pos: [i32; 3], stacks_in: Vec<ItemStack>) {
    if stacks_in.is_empty() {
        return;
    }
    let Some(mut slots) = lv.slots(pos, OVERFLOW_SLOTS) else {
        for s in stacks_in {
            lv.spawn_stack(centre(pos), s, [0.0; 3]);
        }
        return;
    };
    let before = slots.clone();
    for s in stacks_in {
        let mut left = Some(s);
        for slot in slots.iter_mut() {
            let Some(l) = left.take() else { break };
            match slot {
                None => {
                    *slot = Some(l);
                }
                Some(have) if mod_api::ItemResource::of(have).matches(&l) => {
                    let max = lv.host.max_stack_size(have.item()).unwrap_or(64);
                    let n = l.count().min(max.saturating_sub(have.count()));
                    if n > 0 {
                        have.set_count(have.count() + n).ok();
                    }
                    left = l.shrink(n);
                }
                Some(_) => left = Some(l),
            }
        }
        if let Some(l) = left {
            lv.spawn_stack(centre(pos), l, [0.0; 3]);
        }
    }
    let changes: Vec<(usize, Option<ItemStack>)> = slots
        .into_iter()
        .enumerate()
        .filter(|(i, s)| before[*i] != *s)
        .collect();
    if !changes.is_empty() {
        lv.set_slots(pos, &changes);
    }
}

/// `DeployerHandler.activate` for the deployer `d` at `pos` facing `facing` (one standing in the
/// world, or riding a contraption there now): punching, `punch`; else its fake player clicks
/// the block two ahead with what it holds (`activateInner` in `Mode.USE`), and the hand keeps
/// what is left of the stack. `breaker` names its cracks. What else the fake ended with
/// (Create's `overflowItems`).
pub fn activate(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    facing: Dir,
    d: &mut Deployer,
    breaker: i32,
) -> Vec<ItemStack> {
    if d.mode == deployer::Mode::Punch {
        return punch(lv, pos, facing, d, breaker);
    }
    // `safeOnUse` and the item's use, clicked at the middle of the face toward the deployer.
    let reached = deployer::clicked(pos, facing);
    let o = facing.offset();
    let hit = BlockHit::new(
        std::array::from_fn(|i| f64::from(reached[i]) + 0.5 - f64::from(o[i]) * 0.5),
        deployer::clicked_face(facing).offset(),
        false,
    );
    // An entity in the block it reaches is used first (`Player.interactOn`), one at random but
    // a contraption's.
    let targets = entities_at(lv, reached);
    if !targets.is_empty() {
        let target = targets[random_index(lv, targets.len())];
        let location = [0.0, 0.5, 0.0];
        let (outcome, other) = act_with(
            lv,
            pos,
            facing,
            d,
            PlayerAction::InteractOn {
                hand: Hand::Main,
                target,
                location,
            },
        );
        if outcome == Some(mod_api::ActionOutcome::Consumed) {
            return other;
        }
        for s in other {
            lv.spawn_stack(centre(pos), s, [0.0; 3]);
        }
        // `entity instanceof Player`: food it can eat, or a deployable drink, is finished on it.
        let player = lv
            .level
            .online_players(&*lv.host)
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.entity() == target);
        if let Some(player) = player
            && feed(lv, pos, facing, d, player)
        {
            return Vec::new();
        }
    }
    let action = PlayerAction::UseOn {
        hand: Hand::Main,
        pos: reached,
        hit,
    };
    let (outcome, mut other) = act_with(lv, pos, facing, d, action);
    // Nothing used it: the held item's own use (`item.use`: buckets, bottles, snowballs...),
    // but an ender pearl's.
    let used = matches!(
        outcome,
        Some(mod_api::ActionOutcome::Consumed | mod_api::ActionOutcome::EffectBlocked(_))
    );
    let pearl = d
        .held
        .as_ref()
        .is_some_and(|(i, _)| i == "minecraft:ender_pearl");
    if !used && d.held.is_some() && !pearl {
        let (_, more) = act_with(
            lv,
            pos,
            facing,
            d,
            PlayerAction::UseItem { hand: Hand::Main },
        );
        other.extend(more);
    }
    other
}

/// A component of `stack`: its own (the patch), else its item's default.
fn component(host: &dyn mod_api::Host, stack: &ItemStack, name: &str) -> Option<mod_api::Tag> {
    stack
        .components()
        .and_then(|c| c.as_nbt().get(&name.into()).cloned())
        .or_else(|| host.item_default_component(stack.item(), name))
}

fn tag_number(t: Option<&mod_api::Tag>) -> Option<f64> {
    use mod_api::Tag;
    match t? {
        Tag::Byte(v) => Some(f64::from(*v)),
        Tag::Short(v) => Some(f64::from(*v)),
        Tag::Int(v) => Some(f64::from(*v)),
        Tag::Long(v) => Some(*v as f64),
        Tag::Float(v) => Some(f64::from(f32::from_bits(*v))),
        Tag::Double(v) => Some(f64::from_bits(*v)),
        _ => None,
    }
}

/// `activateInner` in `Mode.USE` on a player whose interaction passed: a held food the player
/// can eat (`canEat(canAlwaysEat)`), or a deployable drink (`create:deployable_drink`: milk,
/// potions), is finished on them (`stack.finishUsingItem(level, playerEntity)`), the hand keeping
/// what is left, and its crumbs fly at the hand (`spawnedItemEffects`). True when it was.
fn feed(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    facing: Dir,
    d: &mut Deployer,
    target: PlayerRef,
) -> bool {
    let Some(stack) = held_of(lv, d) else {
        return false;
    };
    let host = &*lv.host;
    let food = match component(host, &stack, "minecraft:food") {
        Some(mod_api::Tag::Compound(c)) => Some((
            tag_number(c.get(&"nutrition".into())).unwrap_or(0.0) as i32,
            tag_number(c.get(&"saturation".into())).unwrap_or(0.0) as f32,
            tag_number(c.get(&"can_always_eat".into())).is_some_and(|v| v != 0.0),
        )),
        _ => None,
    };
    let eats = food.is_some_and(|(_, _, always)| target.can_eat(host, always) == Some(true));
    let drink = stacks::item_tagged(host, &stacks::id(host, &stack), "create:deployable_drink");
    if !eats && !drink {
        return false;
    }
    let left = finish_using(lv, &stack, target, food.filter(|_| eats));
    set_held_stack(&*lv.host, d, left.as_ref());
    // `SandPaperItem.spawnParticles` at the hand (`getMovementVector().scale(reach + 1)`): 20
    // crumbs of the stack as it was, each thrown up to 1/8 along each axis.
    if let Some(key) = ITEM_PARTICLE.get() {
        let o = facing.offset();
        let reach = f64::from(d.reach) + 1.0;
        let at = std::array::from_fn(|i| f64::from(pos[i]) + 0.5 + f64::from(o[i]) * reach);
        let options = mod_api::ParticleOptions::item_stack(*key, stack);
        lv.level.send_particles(
            &mut *lv.host,
            &options,
            None,
            at,
            [0.0; 3],
            0.125 / 3f64.sqrt(),
            20,
        );
    }
    true
}

/// `ItemStack.finishUsingItem(level, target)` for a consumable `stack` (`Consumable.onConsume`):
/// its sound, the food's listener (`FoodProperties.onConsume`: the eating sound, the food, a
/// burp), a potion's (`PotionContents.onConsume`) and a suspicious stew's effects, its consume
/// effects (milk's clearing), then one is used up unless the target has infinite materials, and
/// its `use_remainder` takes the emptied hand or goes to the target. What the hand keeps.
fn finish_using(
    lv: &mut ServerLevel<'_>,
    stack: &ItemStack,
    target: PlayerRef,
    food: Option<(i32, f32, bool)>,
) -> Option<ItemStack> {
    use mod_api::{RandomSource, SoundSource, Tag};
    let host = &*lv.host;
    let Some(Tag::Compound(consumable)) = component(host, stack, "minecraft:consumable") else {
        return Some(stack.clone());
    };
    let Some(living) = lv.host.living(target.entity()) else {
        return Some(stack.clone());
    };
    let at = target.entity().position(host).unwrap_or([0.0; 3]);
    let drinks = matches!(
        consumable.get(&"animation".into()),
        Some(Tag::String(a)) if a.to_string_lossy() == "drink"
    );
    let sound = match consumable.get(&"sound".into()) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        Some(Tag::Compound(c)) => match c.get(&"sound_id".into()) {
            Some(Tag::String(s)) => s.to_string_lossy(),
            _ => String::new(),
        },
        _ => "minecraft:entity.generic.eat".into(),
    };
    let effects = match consumable.get(&"on_consume_effects".into()) {
        Some(Tag::List(l)) => l.len(),
        _ => 0,
    };
    // `Consumable.emit`: the consume sound at the target, from its own source.
    let (volume, pitch) = lv
        .level
        .with_random(&mut *lv.host, |r| {
            let eat_volume = if r.next_boolean() { 0.5 } else { 1.0 };
            let eat_pitch = r.triangle_float(1.0, 0.2);
            let drink_pitch = 0.9 + r.next_float() * 0.1;
            if drinks {
                (0.5, drink_pitch)
            } else {
                (eat_volume, eat_pitch)
            }
        })
        .unwrap_or((1.0, 1.0));
    if !sound.is_empty() {
        lv.level.play_sound(
            &mut *lv.host,
            &sound,
            at,
            SoundSource::Players,
            volume,
            pitch,
        );
    }
    // `FoodProperties.onConsume`.
    if let Some((nutrition, saturation, _)) = food {
        let pitch = lv
            .level
            .with_random(&mut *lv.host, |r| r.triangle_float(1.0, 0.4))
            .unwrap_or(1.0);
        if !sound.is_empty() {
            lv.level
                .play_sound(&mut *lv.host, &sound, at, SoundSource::Neutral, 1.0, pitch);
        }
        // `FoodData.eat(nutrition, saturation)`, given as its modifier
        // (`saturation = nutrition * modifier * 2`).
        let modifier = if nutrition == 0 {
            0.0
        } else {
            saturation / (nutrition as f32 * 2.0)
        };
        target.feed(&mut *lv.host, nutrition, modifier);
        let pitch = lv
            .level
            .with_random(&mut *lv.host, |r| 0.9 + r.next_float() * 0.1)
            .unwrap_or(1.0);
        lv.level.play_sound(
            &mut *lv.host,
            "minecraft:entity.player.burp",
            at,
            SoundSource::Players,
            0.5,
            pitch,
        );
    }
    // `PotionContents.onConsume`.
    if let Some(t) = component(
        &*lv.host,
        stack,
        crate::glue::logistics::items::POTION_CONTENTS,
    ) {
        let contents = crate::glue::logistics::items::contents_of_tag(&t);
        crate::glue::fluids::drink_potion(lv, target.entity(), &contents);
    }
    // `SuspiciousStewEffects.onConsume`: each entry at amplifier 0.
    if let Some(Tag::List(stew)) = component(&*lv.host, stack, "minecraft:suspicious_stew_effects")
    {
        for e in stew {
            let Tag::Compound(e) = e else { continue };
            let Some(Tag::String(id)) = e.get(&"id".into()) else {
                continue;
            };
            let ticks = tag_number(e.get(&"duration".into())).unwrap_or(160.0) as i32;
            if let Some(key) = lv.host.effect_key(&id.to_string_lossy()) {
                let effect = mod_api::ActiveEffect::new(key, ticks, 0, false, true, true);
                living.add_effect(&mut *lv.host, &effect);
            }
        }
    }
    for i in 0..effects {
        let _ = living.apply_consume_effect(&mut *lv.host, stack, i);
    }
    // `stack.consume(1, entity)` and `UseRemainder.convertIntoRemainder`.
    if target.has_infinite_materials(&*lv.host) == Some(true) {
        return Some(stack.clone());
    }
    let left = stack.clone().shrink(1);
    let remainder = match component(&*lv.host, stack, "minecraft:use_remainder") {
        Some(Tag::Compound(c)) => match c.get(&"id".into()) {
            Some(Tag::String(id)) => stacks::stack(
                &*lv.host,
                &id.to_string_lossy(),
                tag_number(c.get(&"count".into())).unwrap_or(1.0) as u32,
            ),
            _ => None,
        },
        _ => None,
    };
    match (left, remainder) {
        (None, r) => r,
        (Some(left), Some(r)) => {
            let _ = target.give_stack(&mut *lv.host, r);
            Some(left)
        }
        (left, None) => left,
    }
}

/// The entities in the block `at`, but a contraption (`AbstractContraptionEntity`).
fn entities_at(lv: &ServerLevel<'_>, at: [i32; 3]) -> Vec<mod_api::EntityRef> {
    let lo = at.map(f64::from);
    let hi = at.map(|v| f64::from(v) + 1.0);
    lv.level
        .entities_in(&*lv.host, lo, hi)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            e.kind(&*lv.host)
                .map(|k| k.id(&*lv.host))
                .is_none_or(|k| !(k.starts_with("create:") && k.ends_with("contraption")))
        })
        .collect()
}

/// `level.random.nextInt(n)`.
fn random_index(lv: &mut ServerLevel<'_>, n: usize) -> usize {
    let n = n as i32;
    let pick = lv
        .level
        .with_random(&mut *lv.host, |r| r.next_int_bound(n).unwrap_or(0))
        .unwrap_or(0) as usize;
    pick.min(n as usize - 1)
}

/// The fake player at the deployer `pos` facing `facing` does `action` with what `d` holds;
/// the deployer keeps what is left in the hand, and what else the fake ended with is returned.
fn act(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    facing: Dir,
    d: &mut Deployer,
    action: PlayerAction,
) -> Vec<ItemStack> {
    act_with(lv, pos, facing, d, action).1
}

/// `act`, with what the action came to (None when the fake could not act).
fn act_with(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    facing: Dir,
    d: &mut Deployer,
    action: PlayerAction,
) -> (Option<mod_api::ActionOutcome>, Vec<ItemStack>) {
    let Some(fake) = lv.host.fake_player(lv.level, FAKE_UUID, FAKE_NAME) else {
        return (None, Vec::new());
    };
    let stack = held_of(lv, d);
    // Create's fake player stands at the ray's origin with no eye height and is not in the
    // world; the game's has a standing body there (1.62 to its eyes), which would be in the
    // way of the block it places. It stands in the deployer instead: its eyes in the middle,
    // facing down above it.
    let mut eye = pos.map(|v| f64::from(v) + 0.5);
    if matches!(action, PlayerAction::UseItem { .. }) {
        // The item's own ray (a bucket's, a bottle's) starts where Create's does
        // (`activateInner`'s ray origin, `3/2 + 1/64` ahead, eyes on its feet), so it reaches the
        // block two ahead and not the deployer's own pole.
        let o = facing.offset();
        eye = std::array::from_fn(|i| eye[i] + f64::from(o[i]) * (1.5 + 1.0 / 64.0));
    } else if facing == Dir::Down {
        eye[1] = f64::from(pos[1]) + 1.63;
    }
    // The fake player looks the way the deployer faces.
    let (yaw, pitch) = match facing {
        Dir::Down => (0.0, 90.0),
        Dir::Up => (0.0, -90.0),
        Dir::North => (180.0, 0.0),
        Dir::South => (0.0, 0.0),
        Dir::West => (90.0, 0.0),
        Dir::East => (-90.0, 0.0),
    };
    let setup = PlayerActionSetup::new(stack, eye, yaw, pitch, false);
    let Ok(result) = fake.perform_action_scoped(&mut *lv.host, action, setup) else {
        return (None, Vec::new());
    };
    set_held_stack(&*lv.host, d, result.held.as_ref());
    (Some(result.outcome), result.other)
}

/// `activateInner` in `Mode.PUNCH`: an entity in the block it reaches takes the hit (one at
/// random, any but a contraption); else, with a held stack, the block there breaks in strokes
/// (`Deployer::punch`), the fake player's cracks on it, a fire in front of it goes out; a
/// broken block is broken by the fake player with the held tool (its drops, the tool's wear).
fn punch(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    facing: Dir,
    d: &mut Deployer,
    breaker: i32,
) -> Vec<ItemStack> {
    let at = deployer::clicked(pos, facing);
    let targets = entities_at(lv, at);
    if !targets.is_empty() {
        let target = targets[random_index(lv, targets.len())];
        return act(lv, pos, facing, d, PlayerAction::Attack { target });
    }
    // `getShape(..).isEmpty()`: nothing to punch, and the breaking is forgotten.
    if lv.shape(at, mod_api::BlockShapeKind::Outline).is_empty() {
        d.breaking = None;
        return Vec::new();
    }
    // `BlockHelper.extinguishFire`: fire on the face toward the deployer goes out.
    let front = super::step(at, deployer::clicked_face(facing), 1);
    if lv.block_name(front).as_deref() == Some("minecraft:fire") {
        let _ = lv.level.level_event(&mut *lv.host, 1009, front, 0);
        let _ = lv.level.remove_block(&mut *lv.host, front, false);
        return Vec::new();
    }
    let Some(held) = d.held.clone() else {
        return Vec::new();
    };
    let Some(state) = lv.block_state(at) else {
        return Vec::new();
    };
    let hardness = mod_api::HostBlockStates::block_states(&*lv.host)
        .destroy_speed(state)
        .unwrap_or(0.0);
    let Some(tool) = stacks::stack(&*lv.host, &held.0, 1) else {
        return Vec::new();
    };
    let per_hit = deployer::destroy_progress(deployer::Dig {
        hardness,
        tool_speed: lv
            .level
            .block_break_speed(&*lv.host, at, Some(&tool))
            .unwrap_or(1.0),
        // Held stacks carry no enchantments here.
        efficiency: 0.0,
        // `requiresCorrectToolForDrops` can't be asked (the module doc).
        harvest: true,
    });
    // `level.playSound(.., getHitSound(), NEUTRAL, .25, 1)`.
    if let Some(sound) = mod_api::HostBlockStates::block_states(&*lv.host).sound(state) {
        let _ = lv.level.play_sound(
            &mut *lv.host,
            &sound.hit,
            centre(at),
            mod_api::SoundSource::Neutral,
            0.25,
            1.0,
        );
    }
    match d.punch(at, per_hit) {
        deployer::Punch::Broken => {
            let _ = lv.level.set_break_progress(&mut *lv.host, breaker, at, -1);
            return act(lv, pos, facing, d, PlayerAction::BreakBlock { pos: at });
        }
        deployer::Punch::Crack(Some(stage)) => {
            let _ = lv
                .level
                .set_break_progress(&mut *lv.host, breaker, at, stage);
        }
        deployer::Punch::Crack(None) | deployer::Punch::Stop => {}
    }
    Vec::new()
}

/// `DeployerBlockEntity.tick`: cracks on a block that is gone since are cleared.
fn deployer_lost_block(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(at) =
        with_world(|w| w.dims[dim].deployers.get(&pos).and_then(|d| d.breaking)).flatten()
    else {
        return;
    };
    let at = at.0;
    if !lv.is_air(at) {
        return;
    }
    let _ = lv
        .level
        .set_break_progress(&mut *lv.host, deployer::breaker_id(pos), at, -1);
    with_world(|w| {
        if let Some(d) = w.dims[dim].deployers.get_mut(&pos) {
            d.breaking = None;
        }
    });
}

/// How far along its facing a click at `point` is from the deployer's back face.
fn along(pos: [i32; 3], facing: Dir, point: [f64; 3]) -> f64 {
    let o = facing.offset();
    (0..3)
        .map(|i| (point[i] - (f64::from(pos[i]) + 0.5 - 0.5 * f64::from(o[i]))) * f64::from(o[i]))
        .sum::<f64>()
}

/// `DeployerBlock.onWrenched`: the wrench on its front half (more than .75 from its back along
/// the facing, at `point`) toggles using and punching (`changeMode`); true when it did.
pub fn wrench_deployer(lv: &mut ServerLevel<'_>, pos: [i32; 3], point: [f64; 3]) -> bool {
    let dim = lv.dim;
    let Some(facing) = lv.facing_of(pos, Block::Deployer) else {
        return false;
    };
    if along(pos, facing, point).abs() <= 0.75 {
        return false;
    }
    with_world(|w| {
        if let Some(mut d) = w.dims[dim].deployers.get(&pos).cloned() {
            d.mode = d.mode.toggled();
            w.set_deployer(dim, pos, d);
        }
    });
    true
}

/// `DeployerBlock.useItemOn`: a click on its front half swaps the player's hand with the
/// deployer's. True when the click was the deployer's.
pub fn use_deployer(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    facing: Dir,
    click: &BlockClick<'_>,
    player: PlayerRef,
    main: bool,
) -> bool {
    let dim = lv.dim;
    let host = &*lv.host;
    if click
        .stack
        .is_some_and(|h| stacks::id(host, h) == "create:wrench")
    {
        return false;
    }
    // The hit along the facing from the back face: less than .75 is the back's own use.
    if along(pos, facing, click.hit.point).abs() < 0.75 {
        return false;
    }
    if !main {
        return true;
    }
    let hand = lv.held(player);
    let Some(mut d) = with_world(|w| w.dims[dim].deployers.get(&pos).cloned()).flatten() else {
        return true;
    };
    if d.held.is_none() && hand.is_none() {
        return true;
    }
    let back = held_of(lv, &d);
    set_held_stack(&*lv.host, &mut d, hand.as_ref());
    lv.set_held(player, back);
    with_world(|w| {
        w.set_deployer(dim, pos, d);
        w.dims[dim].depot_work.push([pos[0], pos[1] - 2, pos[2]]);
    });
    true
}

/// Deployers' strokes to `players`: where the hand is and how fast it moves, what it holds.
fn send_deployers(lv: &mut ServerLevel<'_>, players: &[PlayerRef], list: &[[i32; 3]]) {
    let dim = lv.dim;
    let mut shown = Vec::new();
    for &pos in list {
        let facing = lv.facing_of(pos, Block::Deployer);
        let rpm = lv.speed(pos);
        let d = with_world(|w| w.dims[dim].deployers.get(&pos).cloned()).flatten();
        shown.push(match (facing, d) {
            (Some(f), Some(d)) => {
                let (at, v) = d.hand(rpm);
                deployer::Shown {
                    pos,
                    facing: f,
                    at,
                    per_tick: v,
                    punch: d.mode == deployer::Mode::Punch,
                    held: d.held.map(|h| h.0),
                }
            }
            _ => deployer::Shown {
                pos,
                facing: Dir::Up,
                at: f32::NAN,
                per_tick: 0.0,
                held: None,
                punch: false,
            },
        });
    }
    for chunk in shown.chunks(64) {
        let m = deployer::encode(chunk);
        net::send_to_players(lv.host, players, deployer::CHANNEL, &m);
    }
}

/// A player joined: every deployer of their level.
pub fn player_joined(host: &mut dyn ServerHost, player: PlayerRef) {
    let Some(dimension) = player.level(&*host).and_then(|l| l.dimension().id(&*host)) else {
        return;
    };
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    let dim = lv.dim;
    let deployers: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].deployers.keys().copied().collect()).unwrap_or_default();
    if !deployers.is_empty() {
        send_deployers(&mut lv, &[player], &deployers);
    }
}
