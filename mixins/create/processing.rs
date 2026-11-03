// Create's processing machines in the game: millstone, press, blaze burner, basin and mixer, drill,
// saw, crushing wheels, encased fan, deployer and mechanical crafter. A sibling of the `create`
// module (`runtime.rs`).
use super::Game;
use super::create::*;
use super::create_logistics::*;
use create_engine::client::{self, Model};
use create_engine::kinetics::{Axis, Dir};
use create_engine::states;
use horizon_world::registry::BlockStateId;
use std::cell::RefCell;
use std::collections::HashMap;

/// The millstones that turn: Create's `MillstoneBlockEntity.tick` on their slots (input 0,
/// outputs 1 to 9). Millstones that don't turn cost nothing.
pub(super) fn tick_millstones(game: &mut Game<'_>, dim: usize) {
    let turning = with_world(|w| w.turning_millstones(dim)).unwrap_or_default();
    if turning.is_empty() {
        return;
    }
    if !ensure_recipes(game) {
        return;
    }
    let seed = game.world.game_time as u64;
    for (pos, speed) in turning {
        let Some(mut state) = with_world(|w| w.dims[dim].millstones.get(&pos).copied()).flatten()
        else {
            continue;
        };
        let max = |item: &str| {
            game.items()
                .get(item)
                .map_or(64, |i| i.max_stack)
                .clamp(1, 64) as u8
        };
        // Stack limits of what the slots hold, read before the block entity is borrowed.
        let limits: Vec<u8> = game
            .world
            .block_entities
            .get(&(pos[0], pos[1], pos[2]))
            .and_then(|b| match b {
                horizon_client::containers::BlockEntity::Mod(m) => Some(
                    m.items
                        .iter()
                        .map(|s| s.as_ref().map_or(64, |s| max(&s.item)))
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_default();
        if limits.len() < 10 {
            continue;
        }
        let result_limits: Vec<(String, u8)> = RECIPES.with_borrow(|r| {
            r.as_ref().map_or_else(Vec::new, |r| {
                r.milling
                    .recipes
                    .iter()
                    .flat_map(|x| x.results.iter())
                    .map(|o| (o.item.clone(), max(&o.item)))
                    .collect()
            })
        });
        game.native_with_mod_block_entity(pos, |be| {
            if be.items.len() < 10 {
                return;
            }
            let input = be.items[0].as_ref().map(|s| s.item.to_string());
            let outputs_full = be.items[1..10]
                .iter()
                .zip(&limits[1..10])
                .any(|(s, l)| s.as_ref().is_some_and(|s| s.count >= u32::from(*l)));
            RECIPES.with_borrow(|r| {
                let Some(r) = r else { return };
                let Some(done) = create_engine::processing::tick_millstone(
                    &mut state,
                    speed,
                    input.as_deref(),
                    outputs_full,
                    &r.milling,
                ) else {
                    return;
                };
                // process(): one input, the rolled results stacked into the outputs.
                if let Some(s) = be.items[0].as_mut() {
                    s.count -= 1;
                    if s.count == 0 {
                        be.items[0] = None;
                    }
                }
                let mut rng = create_engine::processing::Rng::new(
                    seed ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9) ^ (pos[2] as u64) << 20,
                );
                let mut slots: Vec<create_engine::processing::Slot> = be.items[1..10]
                    .iter()
                    .map(|s| {
                        s.as_ref()
                            .map(|s| (s.item.to_string(), s.count.min(255) as u8))
                    })
                    .collect();
                for (item, n) in create_engine::processing::roll(r.milling.get(done), &mut rng) {
                    let limit = result_limits
                        .iter()
                        .find(|(i, _)| *i == item)
                        .map_or(64, |(_, l)| *l);
                    create_engine::processing::insert_stacked(&mut slots, &item, n, limit);
                }
                for (slot, s) in be.items[1..10].iter_mut().zip(slots) {
                    *slot = s.map(|(item, count)| horizon_client::items::ItemStack {
                        item: item.as_str().into(),
                        count: u32::from(count),
                        damage: 0,
                        extra: None,
                    });
                }
            });
        });
        with_world(|w| {
            if let Some(m) = w.dims[dim].millstones.get_mut(&pos) {
                *m = state;
            }
        });
    }
}

/// `MillstoneBlock.useItemOn` with an empty hand: the outputs go to the player (the input when
/// there are none); with an item, the block's own use (closed: no menu opens).
pub(super) fn use_millstone(game: &mut Game<'_>, pos: [i32; 3], canceled: &mut bool) {
    *canceled = true;
    if game.native_held().is_some_and(|h| h.0 != "minecraft:air") {
        return;
    }
    let taken = game
        .native_with_mod_block_entity(pos, |be| {
            let outputs: Vec<_> = be
                .items
                .iter_mut()
                .skip(1)
                .filter_map(Option::take)
                .collect();
            if outputs.is_empty() {
                be.items
                    .first_mut()
                    .and_then(Option::take)
                    .into_iter()
                    .collect()
            } else {
                outputs
            }
        })
        .unwrap_or_default();
    for s in taken {
        game.native_give_items(&s.item, s.count);
    }
}

/// The deployer two above `depot`, facing down, and its speed; None for anything else.
pub(super) fn deployer_above(
    game: &Game<'_>,
    dim: usize,
    depot: [i32; 3],
) -> Option<(create_engine::deployer::Deployer, f32)> {
    let above = [depot[0], depot[1] + 2, depot[2]];
    let state = game.world.block(above[0], above[1], above[2]).0;
    states::index(state)?;
    with_world(|w| {
        let (block, info) = w.table.get(state)?;
        if block != create_engine::Block::Deployer || info.facing != Some(Dir::Down) {
            return None;
        }
        let d = w.dims[dim].deployers.get(&above)?.clone();
        Some((d, w.speed(dim, above).unwrap_or(0.0)))
    })
    .flatten()
}

/// `BeltDeployerCallbacks` for the depot at `depot`: true when a deployer above holds its stack
/// (the depot asks again next tick).
pub(super) fn deployer_on_depot(game: &mut Game<'_>, dim: usize, depot: [i32; 3]) -> bool {
    use create_engine::deployer::{self, Depot};
    let Some((mut d, rpm)) = deployer_above(game, dim, depot) else {
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
    let Some((_, item, count)) = depot_items(game, depot).into_iter().find(|s| s.0 == 0) else {
        return false;
    };
    if !ensure_recipes(game) {
        return false;
    }
    let tool = d.held.as_ref().map(|h| h.0.clone()).unwrap_or_default();
    // A sequenced assembly's deploying step first, with the item the deployer holds.
    let sequenced = sequenced_on_depot(game, depot, "create:deploying", Some(&tool));
    let recipe = match &sequenced {
        Some((a, _, s)) => Some(deployer::Recipe {
            id: a.id.clone(),
            target: Vec::new(),
            tool: s.with.clone().unwrap_or_default(),
            results: Vec::new(),
            keep_held: s.keep_held,
        }),
        None => RECIPES.with_borrow(|r| {
            r.as_ref()
                .and_then(|r| deployer::find(&r.deploying, &item, &tool).cloned())
        }),
    };
    let step = deployer::on_depot(&d, rpm, held.locked, recipe.is_some());
    let hold = match step {
        Depot::Pass => {
            held.locked = false;
            false
        }
        Depot::Hold => {
            held.locked = true;
            true
        }
        Depot::Start => {
            // The ray from the deployer's face to the block two ahead (`start`).
            let state = game.world.block(depot[0], depot[1], depot[2]);
            let top = game
                .res
                .facts()
                .get(state)
                .outline
                .iter()
                .map(|b| b[4])
                .fold(0.0, f64::max);
            d.start((1.0 - top) as f32);
            held.locked = true;
            with_world(|w| {
                let dd = &mut w.dims[dim];
                dd.deployers.insert(above, d.clone());
                if !dd.deploying.contains(&above) {
                    dd.deploying.push(above);
                }
                dd.deployer_changes.push(above);
            });
            true
        }
        Depot::Apply => {
            let Some(r) = recipe else {
                return false;
            };
            if let Some((a, progress, _)) = &sequenced {
                held = sequenced_step_on_depot(game, dim, depot, count, held, a, progress.as_ref());
                held.locked = true;
                deployer_used(game, &mut d, &r, above);
                with_world(|w| w.set_deployer(dim, above, d.clone()));
                with_world(|w| {
                    if let Some(h) = w.dims[dim].depots.get_mut(&depot) {
                        *h = h.map(|_| held);
                    }
                });
                return true;
            }
            // `RecipeApplier.applyRecipeOn` on one of the stack, `getMaxOutputCount` 4.
            let seed = game.world.game_time as u64 ^ (depot[0] as u64).wrapping_mul(0x9E37_79B9);
            let p = create_engine::processing::Recipe {
                id: r.id.clone(),
                ingredients: Vec::new(),
                results: r.results.clone(),
                time: 0,
            };
            let results: Vec<(String, u8)> =
                create_engine::processing::roll(&p, &mut create_engine::processing::Rng::new(seed))
                    .into_iter()
                    .take(4)
                    .collect();
            held = results_on_depot(game, dim, depot, count, held, results);
            held.locked = true;
            deployer_used(game, &mut d, &r, above);
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

/// The deployer at `pos` after applying `recipe` (`activate`): its tool wears, other items are
/// used up (`Deployer::use_for`); a crafting remainder that finds no room in the hand drops at
/// the deployer (the fake player's overflow).
pub(super) fn deployer_used(
    game: &mut Game<'_>,
    d: &mut create_engine::deployer::Deployer,
    recipe: &create_engine::deployer::Recipe,
    pos: [i32; 3],
) {
    let info = d.held.as_ref().and_then(|h| game.items().get(&h.0));
    let max_damage = info.as_ref().map_or(0, |i| i.max_damage);
    let remainder = info.and_then(|i| i.crafting_remainder.clone());
    if let Some(rest) = d.use_for(recipe, max_damage, remainder.as_deref()) {
        game.native_run_command(&format!(
            "summon minecraft:item {}.5 {}.5 {}.5 {{Item:{{id:'{rest}',count:1}}}}",
            pos[0], pos[1], pos[2]
        ));
    }
}

/// Deployers mid-stroke (`DeployerBlockEntity.tick` without a fake player): the stroke runs;
/// one back at rest has the depot below look at it again. Nothing runs without one.
pub(super) fn tick_deployers(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::deployer::{Mode, State, Tick};
    // `redstoneUpdate`: powered, it is locked.
    let power =
        with_world(|w| std::mem::take(&mut w.dims[dim].deployer_power_checks)).unwrap_or_default();
    for pos in power {
        if deployer_facing(game, pos).is_none() {
            continue;
        }
        let powered = super::create_redstone::neighbor_signal(game, pos);
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
    // Deployers at rest, turning, not locked: `tick`'s `WAITING` start
    // (`shouldActivate` when using), else a look again `getTimerSpeed() × 10` later. Using ones
    // over a depot or belt wait for it.
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
        let Some(facing) = deployer_facing(game, pos) else {
            continue;
        };
        let o = facing.offset();
        let below = [pos[0] + o[0] * 2, pos[1] + o[1] * 2, pos[2] + o[2] * 2];
        let over_depot = facing == Dir::Down
            && with_world(|w| {
                w.dims[dim].depots.contains_key(&below) || w.dims[dim].belt_of.contains_key(&below)
            }) == Some(true);
        if over_depot && mode == Mode::Use {
            continue;
        }
        let place = if mode == Mode::Punch {
            Some(below)
        } else {
            deployer_place_target(game, dim, pos, facing).or_else(|| {
                deployer_uses(game, dim, pos, facing).then_some([
                    pos[0] + o[0],
                    pos[1] + o[1],
                    pos[2] + o[2],
                ])
            })
        };
        with_world(|w| {
            let d = &mut w.dims[dim];
            let Some(dp) = d.deployers.get_mut(&pos) else {
                return;
            };
            match place {
                Some(_) => {
                    // `start`: the ray toward the block two ahead sets its reach.
                    let reach = if game_free(game, [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]) {
                        0.75
                    } else {
                        0.0
                    };
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
        deployer_lost_block(game, dim, pos);
        let activated = with_world(|w| {
            let rpm = w.speed(dim, pos).unwrap_or(0.0);
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
            deployer_activate(game, dimension, dim, pos);
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
        super::dispenser::create_use::deployer_crack(
            game,
            create_engine::deployer::breaker_id(pos),
            at,
            -1,
        );
    }
    for (pos, (item, n)) in drops {
        game.native_run_command(&format!(
            "execute in {dimension} run summon minecraft:item {}.5 {}.5 {}.5 {{Item:{{id:'{item}',count:{n}}}}}",
            pos[0], pos[1], pos[2]
        ));
    }
    if !changes.is_empty() {
        let players = players_in(game, dimension);
        send_deployers(game, &players, dim, &changes);
    }
}

/// A deployer's facing; None for any other block.
pub(super) fn deployer_facing(game: &Game<'_>, pos: [i32; 3]) -> Option<Dir> {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    with_world(|w| {
        w.table
            .get(state)
            .filter(|(b, _)| *b == create_engine::Block::Deployer)
            .and_then(|(_, i)| i.facing)
    })
    .flatten()
}

/// Whether the block at `p` can be replaced (air, a fluid, grass).
pub(super) fn game_free(game: &Game<'_>, p: [i32; 3]) -> bool {
    game.res
        .behaviours()
        .is_replaceable(game.world.block(p[0], p[1], p[2]))
}

/// Where the block a deployer holds would go (`placement`), as a position; None when it holds
/// no block, or nowhere fits.
pub(super) fn deployer_place_target(
    game: &Game<'_>,
    dim: usize,
    pos: [i32; 3],
    facing: Dir,
) -> Option<[i32; 3]> {
    let held =
        with_world(|w| w.dims[dim].deployers.get(&pos).and_then(|d| d.held.clone())).flatten()?;
    // A block item (`BlockItem.getBlock`): the block it places.
    let placed = game.items().get(&held.0)?.block?;
    let o = facing.offset();
    let reached = [pos[0] + o[0] * 2, pos[1] + o[1] * 2, pos[2] + o[2] * 2];
    let before = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
    let is_held = game
        .pack
        .assets
        .registry
        .state(game.world.block(reached[0], reached[1], reached[2]))
        .zip(game.pack.assets.registry.state(placed))
        .is_some_and(|(s, p)| s.name == p.name);
    create_engine::deployer::placement(game_free(game, reached), game_free(game, before), is_held)
        .map(|at_reached| if at_reached { reached } else { before })
}

/// `shouldActivate` for a held item that is not a block: always, except a bucket of a fluid
/// whose source is already in the block it reaches.
pub(super) fn deployer_uses(game: &Game<'_>, dim: usize, pos: [i32; 3], facing: Dir) -> bool {
    // `shouldActivate`: an empty hand always (it uses the block it reaches).
    let Some((item, _)) =
        with_world(|w| w.dims[dim].deployers.get(&pos).and_then(|d| d.held.clone())).flatten()
    else {
        return true;
    };
    if game.items().get(&item).is_some_and(|i| i.block.is_some()) {
        return false;
    }
    let o = facing.offset();
    let reached = game
        .world
        .block(pos[0] + o[0] * 2, pos[1] + o[1] * 2, pos[2] + o[2] * 2);
    let fluid = match item.as_str() {
        "minecraft:water_bucket" => "minecraft:water",
        "minecraft:lava_bucket" => "minecraft:lava",
        _ => return true,
    };
    game.pack
        .assets
        .registry
        .state(reached)
        .is_none_or(|s| s.name != fluid || s.properties.get("level").is_some_and(|l| l != "0"))
}

/// `DeployerBlockEntity.activate` for what is ported: nothing over a depot or belt (their
/// callbacks do the work); punching, `deployer_punch`; else a held block item is placed (its
/// default state) where it fits, and one is used up, and another item is used on the block it
/// reaches (`dispenser::create_use`: tools, spawn eggs, buckets, bone meal, shears...).
pub(super) fn deployer_activate(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    let Some(facing) = deployer_facing(game, pos) else {
        return;
    };
    let reached = create_engine::deployer::clicked(pos, facing);
    let over_depot = facing == Dir::Down
        && with_world(|w| {
            w.dims[dim].depots.contains_key(&reached) || w.dims[dim].belt_of.contains_key(&reached)
        }) == Some(true);
    if over_depot {
        return;
    }
    let punching = with_world(|w| {
        w.dims[dim]
            .deployers
            .get(&pos)
            .is_some_and(|d| d.mode == create_engine::deployer::Mode::Punch)
    }) == Some(true);
    if punching {
        super::dispenser::create_use::deployer_punch(game, dimension, dim, pos, facing);
        return;
    }
    // `safeOnUse`: the block it reaches, used first (a lever, a door); it took the action.
    if super::dispenser::create_use::use_block(game, pos, facing) {
        return;
    }
    let Some(target) = deployer_place_target(game, dim, pos, facing) else {
        deployer_use_item(game, dim, pos, facing);
        return;
    };
    let Some((item, _)) =
        with_world(|w| w.dims[dim].deployers.get(&pos).and_then(|d| d.held.clone())).flatten()
    else {
        return;
    };
    let Some(state) = game.items().get(&item).and_then(|i| i.block) else {
        return;
    };
    game.native_set_block(
        dimension,
        target,
        state,
        super::native_hooks::flags::UPDATE_ALL,
    );
    with_world(|w| {
        if let Some(mut d) = w.dims[dim].deployers.get(&pos).cloned() {
            if let Some((_, n)) = d.held.as_mut() {
                *n -= 1;
                if *n == 0 {
                    d.held = None;
                }
            }
            w.set_deployer(dim, pos, d);
        }
    });
}

/// A held item that is not a block, used on the block two ahead (`useOn` from the fake player):
/// the hand keeps what is left of the stack (an emptied bucket, a filled one).
pub(super) fn deployer_use_item(game: &mut Game<'_>, dim: usize, pos: [i32; 3], facing: Dir) {
    if !deployer_uses(game, dim, pos, facing) {
        return;
    }
    let Some((item, count)) =
        with_world(|w| w.dims[dim].deployers.get(&pos).and_then(|d| d.held.clone())).flatten()
    else {
        return;
    };
    let damage = with_world(|w| w.dims[dim].deployers.get(&pos).map(|d| d.held_damage))
        .flatten()
        .unwrap_or(0);
    let stack = horizon_client::items::ItemStack {
        item: item.as_str().into(),
        count: u32::from(count),
        damage,
        extra: None,
    };
    let Some(left) = super::dispenser::create_use::use_item_at(game, pos, facing, stack) else {
        return;
    };
    let worn = left.as_ref().map_or(0, |s| s.damage);
    let held = left.map(|s| (s.item.to_string(), s.count.min(64) as u8));
    with_world(|w| {
        if let Some(mut d) = w.dims[dim].deployers.get(&pos).cloned() {
            if d.held != held || d.held_damage != worn {
                d.held = held;
                d.held_damage = worn;
                w.dims[dim].deployer_changes.push(pos);
                w.set_deployer(dim, pos, d);
            }
        }
    });
}

/// `DeployerBlockEntity.tick`: cracks on a block that is gone since are cleared.
pub(super) fn deployer_lost_block(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    let Some(at) =
        with_world(|w| w.dims[dim].deployers.get(&pos).and_then(|d| d.breaking)).flatten()
    else {
        return;
    };
    let at = at.0;
    if !game
        .res
        .behaviours()
        .is_air(game.world.block(at[0], at[1], at[2]))
    {
        return;
    }
    super::dispenser::create_use::deployer_crack(
        game,
        create_engine::deployer::breaker_id(pos),
        at,
        -1,
    );
    with_world(|w| {
        if let Some(d) = w.dims[dim].deployers.get_mut(&pos) {
            d.breaking = None;
        }
    });
}

/// `DeployerBlock.onWrenched`: the wrench on its front half (more than .75 from its back along
/// the facing) toggles using and punching (`changeMode`); true when it did.
pub(super) fn wrench_deployer(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) -> bool {
    let Some(facing) = deployer_facing(game, pos) else {
        return false;
    };
    let Some(hit) = game.pick() else {
        return false;
    };
    let o = facing.offset();
    let along = (0..3)
        .map(|i| {
            (hit.point[i] - (f64::from(pos[i]) + 0.5 - 0.5 * f64::from(o[i]))) * f64::from(o[i])
        })
        .sum::<f64>();
    if along.abs() <= 0.75 {
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

/// Deployers' strokes to `players`: where the hand is and how fast it moves, what it holds.
pub(super) fn send_deployers(game: &mut Game<'_>, players: &[u128], dim: usize, list: &[[i32; 3]]) {
    let mut shown = Vec::new();
    for &pos in list {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let facing = with_world(|w| w.table.get(state).and_then(|(_, i)| i.facing)).flatten();
        let rpm = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let d = with_world(|w| w.dims[dim].deployers.get(&pos).cloned()).flatten();
        shown.push(match (facing, d) {
            (Some(f), Some(d)) => {
                let (at, v) = d.hand(rpm);
                create_engine::deployer::Shown {
                    pos,
                    facing: f,
                    at,
                    per_tick: v,
                    held: d.held.map(|h| h.0),
                }
            }
            _ => create_engine::deployer::Shown {
                pos,
                facing: Dir::Up,
                at: f32::NAN,
                per_tick: 0.0,
                held: None,
            },
        });
    }
    for chunk in shown.chunks(64) {
        let m = create_engine::deployer::encode(chunk);
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::deployer::CHANNEL, &m);
        }
    }
}

/// `DeployerRenderer`: the pole and the hand (pointing, or holding) at the hand's offset, glided
/// along the facing on the GPU between messages.
pub(super) fn deployers_from_server(game: &mut Game<'_>, data: &[u8]) {
    for s in create_engine::deployer::decode(data) {
        let pole = depot_key(s.pos, 110, 0);
        let hand = depot_key(s.pos, 111, 0);
        game.native_part_clear(pole);
        game.native_part_clear(hand);
        if s.at.is_nan() {
            continue;
        }
        let o = s.facing.offset().map(|v| v as f32);
        let now = game.native_parts_seconds();
        let offset = [o[0] * s.at, o[1] * s.at, o[2] * s.at, now];
        let v = [
            o[0] * s.per_tick * 20.0,
            o[1] * s.per_tick * 20.0,
            o[2] * s.per_tick * 20.0,
        ];
        let facing = dir_name(s.facing);
        let hand_block = if s.held.is_some() {
            "create:part_deployer_hand_holding"
        } else {
            "create:part_deployer_hand_pointing"
        };
        for (key, block) in [(pole, "create:part_deployer_pole"), (hand, hand_block)] {
            let registry = game.pack.assets.registry;
            let id = registry.default_state(block).and_then(|d| {
                let mut st = registry.state(d)?.clone();
                st.properties.insert("facing".to_owned(), facing.to_owned());
                registry.id(&st)
            });
            if let Some(model) = id.and_then(|id| game.native_part_model(id)) {
                game.native_part_glide(key, model, s.pos, 0, 0.0, 0.0, offset, v);
            }
        }
    }
}

/// `DeployerBlock.useItemOn`: a click on its front half swaps the player's hand with the
/// deployer's (plain stacks only here: components would be lost).
pub(super) fn use_deployer(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    facing: Dir,
    canceled: &mut bool,
) {
    if game.native_held().is_some_and(|h| h.0 == "create:wrench") {
        return;
    }
    let Some(hit) = game.pick() else {
        return;
    };
    // The hit along the facing from the back face: less than .75 is the back's own use.
    let o = facing.offset();
    let along = (0..3)
        .map(|i| {
            (hit.point[i] - (f64::from(pos[i]) + 0.5 - 0.5 * f64::from(o[i]))) * f64::from(o[i])
        })
        .sum::<f64>();
    if along.abs() < 0.75 {
        return;
    }
    *canceled = true;
    if game.hand != horizon_client::first_person::Hand::Main {
        return;
    }
    let hand = game.inventory.get(game.selected).cloned().flatten();
    // Its hand keeps an item's id, count and wear, nothing else.
    if hand.as_ref().is_some_and(|s| s.extra.is_some()) {
        return;
    }
    let Some(mut d) = with_world(|w| w.dims[dim].deployers.get(&pos).cloned()).flatten() else {
        return;
    };
    if d.held.is_none() && hand.is_none() {
        return;
    }
    let back = d
        .held
        .take()
        .map(|(item, n)| horizon_client::items::ItemStack {
            item: item.as_str().into(),
            count: u32::from(n),
            damage: d.held_damage,
            extra: None,
        });
    d.held_damage = hand.as_ref().map_or(0, |s| s.damage);
    d.held = hand.map(|s| (s.item.to_string(), s.count.min(64) as u8));
    if let Some(slot) = game.inventory.get_mut(game.selected) {
        *slot = back;
    }
    with_world(|w| {
        w.set_deployer(dim, pos, d);
        w.dims[dim].depot_work.push([pos[0], pos[1] - 2, pos[2]]);
    });
}

/// Depots that ask their press, then running presses: `DepotBehaviour.tick` with
/// `BeltPressingCallbacks`, then `PressingBehaviour.tick`. Nothing runs without either.
pub(super) fn tick_presses(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::press::{self, Step, Tick};
    let Some((work, running)) = with_world(|w| {
        let d = &mut w.dims[dim];
        let running: Vec<[i32; 3]> = d
            .presses
            .iter()
            .filter(|(_, p)| p.running)
            .map(|(k, _)| *k)
            .collect();
        (std::mem::take(&mut d.depot_work), running)
    }) else {
        return;
    };
    if work.is_empty() && running.is_empty() {
        return;
    }
    if !ensure_recipes(game) {
        return;
    }
    let mut keep = Vec::new();
    for depot in work {
        let above = [depot[0], depot[1] + 2, depot[2]];
        if deployer_on_depot(game, dim, depot) {
            keep.push(depot);
            continue;
        }
        let Some(mut p) = with_world(|w| w.dims[dim].presses.get(&above).copied()).flatten() else {
            continue;
        };
        let Some(mut held) =
            with_world(|w| w.dims[dim].depots.get(&depot).copied().flatten()).flatten()
        else {
            continue;
        };
        if held.sliding() && held.position < 0.5 - 1.0 / 16.0 {
            continue;
        }
        let Some((_, item, count)) = depot_items(game, depot).into_iter().find(|s| s.0 == 0) else {
            continue;
        };
        let speed = with_world(|w| w.speed(dim, above)).flatten().unwrap_or(0.0);
        let in_way = blocked(game, [depot[0], depot[1] + 1, depot[2]]);
        // A sequenced assembly's pressing step first (`SequencedAssemblyRecipe.getRecipe`).
        let sequenced = sequenced_on_depot(game, depot, "create:pressing", None);
        let fits = sequenced.is_some()
            || RECIPES.with_borrow(|r| {
                r.as_ref()
                    .is_some_and(|r| r.pressing.find_single(&item).is_some())
            });
        let step = press::depot_step(&mut held, &p, speed, in_way, fits);
        match step {
            Step::Idle => {}
            Step::Wait => keep.push(depot),
            Step::Start => {
                p.start(press::Mode::Belt);
                with_world(|w| {
                    w.dims[dim].presses.insert(above, p);
                    w.dims[dim].press_changes.push(above);
                });
                keep.push(depot);
            }
            Step::Process => {
                held = match &sequenced {
                    Some((a, progress, _)) => {
                        sequenced_step_on_depot(game, dim, depot, count, held, a, progress.as_ref())
                    }
                    None => press_on_depot(game, dim, depot, &item, count, held),
                };
                keep.push(depot);
            }
        }
        with_world(|w| {
            if let Some(h) = w.dims[dim].depots.get_mut(&depot) {
                *h = h.map(|_| held);
            }
        });
    }
    for at in running {
        let speed = with_world(|w| w.speed(dim, at)).flatten().unwrap_or(0.0);
        let basin_mode = with_world(|w| {
            let d = &mut w.dims[dim];
            let p = d.presses.get_mut(&at)?;
            let tick = p.tick(speed);
            match tick {
                Tick::Moving => {}
                Tick::Down => d.press_changes.push(at),
                Tick::Done => {
                    d.press_changes.push(at);
                    let below = [at[0], at[1] - 2, at[2]];
                    if d.depots.contains_key(&below) && !keep.contains(&below) {
                        keep.push(below);
                    }
                }
            }
            (p.mode == press::Mode::Basin).then_some(tick)
        })
        .flatten();
        // Over a basin: compact at the bottom (`tryProcessInBasin`), look again when done
        // (`onPressingCompleted`).
        match basin_mode {
            Some(Tick::Down) => compact_in_basin(game, dim, at),
            Some(Tick::Done) => {
                with_world(|w| w.dims[dim].mixer_checks.push(at));
            }
            _ => {}
        }
    }
    let changed = with_world(|w| {
        let d = &mut w.dims[dim];
        for p in keep {
            if !d.depot_work.contains(&p) {
                d.depot_work.push(p);
            }
        }
        let mut c = std::mem::take(&mut d.press_changes);
        c.sort();
        c.dedup();
        c
    })
    .unwrap_or_default();
    if !changed.is_empty() {
        let players = players_in(game, dimension);
        send_presses(game, &players, dim, &changed);
    }
}

/// `BeltPressingCallbacks.whenItemHeld` at the head's lowest point: one of the stack (all of
/// it when it is one) becomes the recipe's rolled results. A single item is replaced by them
/// (the first held, centred; the rest in the outputs); otherwise the rest stays held and the
/// results go to the outputs. What finds no room drops above the depot. Returns the held
/// stack's motion after.
pub(super) fn press_on_depot(
    game: &mut Game<'_>,
    dim: usize,
    depot: [i32; 3],
    item: &str,
    count: u8,
    held: create_engine::depot::Held,
) -> create_engine::depot::Held {
    use create_engine::processing;
    let seed = game.world.game_time as u64
        ^ (depot[0] as u64).wrapping_mul(0x9E37_79B9)
        ^ (depot[2] as u64) << 20;
    let results = RECIPES.with_borrow(|r| {
        let r = r.as_ref()?;
        let id = r.pressing.find_single(item)?;
        Some(processing::roll(
            r.pressing.get(id),
            &mut processing::Rng::new(seed),
        ))
    });
    let Some(results) = results else {
        return held;
    };
    results_on_depot(game, dim, depot, count, held, results)
}

/// The transitional item of the assembly an item's progress names, with the progress and the
/// fraction of the steps done (`SequencedAssembly.progress`).
pub(super) fn transitional_of(
    p: &create_engine::sequenced::Progress,
) -> Option<(String, &create_engine::sequenced::Progress, f32)> {
    RECIPES.with_borrow(|r| {
        let a = r.as_ref()?.sequenced.iter().find(|a| a.id == p.id)?;
        let total = (a.steps.len().max(1) as u32 * a.loops.max(1)) as f32;
        Some((a.transitional.clone(), p, p.step as f32 / total))
    })
}

/// The depot's held stack with its components.
fn depot_held_stack(game: &Game<'_>, depot: [i32; 3]) -> Option<horizon_client::items::ItemStack> {
    match game
        .world
        .block_entities
        .get(&(depot[0], depot[1], depot[2]))
    {
        Some(horizon_client::containers::BlockEntity::Mod(m)) => m.items.first()?.clone(),
        _ => None,
    }
}

/// `SequencedAssemblyRecipe.getRecipe` for the stack held on `depot` and a machine of recipe
/// type `kind` (a deployer holding `tool`): the assembly, the item's progress and the step,
/// when the item is at a step of that type (and the deployer holds what it takes).
pub(super) fn sequenced_on_depot(
    game: &Game<'_>,
    depot: [i32; 3],
    kind: &str,
    tool: Option<&str>,
) -> Option<(
    create_engine::sequenced::Assembly,
    Option<create_engine::sequenced::Progress>,
    create_engine::sequenced::Step,
)> {
    let stack = depot_held_stack(game, depot)?;
    let progress = stack_progress(&stack);
    let (a, s) = sequenced_for(&stack.item, progress.as_ref(), kind, tool)?;
    Some((a, progress, s))
}

/// `SequencedAssemblyRecipe.getRecipe` for `item` at `progress` and a machine of recipe type
/// `kind` (a deployer holding `tool`): the assembly and the step, when the item is at a step of
/// that type (and the deployer holds what it takes).
pub(super) fn sequenced_for(
    item: &str,
    progress: Option<&create_engine::sequenced::Progress>,
    kind: &str,
    tool: Option<&str>,
) -> Option<(
    create_engine::sequenced::Assembly,
    create_engine::sequenced::Step,
)> {
    RECIPES.with_borrow(|r| {
        let r = r.as_ref()?;
        let (a, s) = create_engine::sequenced::step_for(&r.sequenced, item, progress, kind)?;
        if let Some(with) = &s.with
            && !tool.is_some_and(|t| with.iter().any(|w| w == t))
        {
            return None;
        }
        Some((a.clone(), s.clone()))
    })
}

/// One of a belt stack through a sequenced step (`advance`): the transitional item a step on,
/// its progress with it, or a result of the pool.
pub(super) fn sequenced_on_belt(
    game: &Game<'_>,
    at: [i32; 3],
    item: &create_engine::belt::Item,
    a: &create_engine::sequenced::Assembly,
) -> Vec<create_engine::belt::Item> {
    use create_engine::sequenced::Advanced;
    let seed = game.world.game_time as u64 ^ (at[0] as u64).wrapping_mul(0x9E37_79B9);
    let roll = create_engine::processing::Rng::new(seed).next_float();
    let (name, count, progress) = match a.advance(item.progress.as_ref(), roll) {
        Advanced::Next { item, progress, .. } => (item, 1, Some(progress)),
        Advanced::Done { item, count } => (item, count, None),
        Advanced::Nothing => return Vec::new(),
    };
    let mut out = item.clone();
    out.item = name;
    out.count = count;
    out.progress = progress;
    out.fan = Default::default();
    vec![out]
}

/// The rest of a belt stack after one of it was processed (`convertToAndLeaveHeld`).
fn belt_left(item: &create_engine::belt::Item) -> Option<create_engine::belt::Item> {
    (item.count > 1).then(|| {
        let mut left = item.clone();
        left.count -= 1;
        left
    })
}

/// `BeltDeployerCallbacks` for a stack on the belt at `at` under the deployer `d` (facing down,
/// at `rpm`): the deployer starts its stroke on a stack its held item applies to (a sequenced
/// assembly's deploying step first) and, back at the top of it, turns one of the stack into the
/// results (`activate`).
pub(super) fn deployer_on_belt(
    game: &mut Game<'_>,
    dim: usize,
    at: [i32; 3],
    item: &mut create_engine::belt::Item,
    locked: bool,
    mut d: create_engine::deployer::Deployer,
    rpm: f32,
) -> create_engine::belt::Process {
    use create_engine::belt::Process;
    use create_engine::deployer::{self, Depot};
    let above = [at[0], at[1] + 2, at[2]];
    if !ensure_recipes(game) {
        return Process::Pass;
    }
    let tool = d.held.as_ref().map(|h| h.0.clone()).unwrap_or_default();
    let sequenced = sequenced_for(
        &item.item,
        item.progress.as_ref(),
        "create:deploying",
        Some(&tool),
    );
    let recipe = match &sequenced {
        Some((a, s)) => Some(deployer::Recipe {
            id: a.id.clone(),
            target: Vec::new(),
            tool: s.with.clone().unwrap_or_default(),
            results: Vec::new(),
            keep_held: s.keep_held,
        }),
        None => RECIPES.with_borrow(|r| {
            r.as_ref()
                .and_then(|r| deployer::find(&r.deploying, &item.item, &tool).cloned())
        }),
    };
    match deployer::on_depot(&d, rpm, locked, recipe.is_some()) {
        Depot::Pass => Process::Pass,
        Depot::Hold => Process::Hold,
        Depot::Start => {
            // The ray from the deployer's face to the belt's top (`start`).
            let state = game.world.block(at[0], at[1], at[2]);
            let top = game
                .res
                .facts()
                .get(state)
                .outline
                .iter()
                .map(|b| b[4])
                .fold(0.0, f64::max);
            d.start((1.0 - top) as f32);
            with_world(|w| {
                let dd = &mut w.dims[dim];
                dd.deployers.insert(above, d.clone());
                if !dd.deploying.contains(&above) {
                    dd.deploying.push(above);
                }
                dd.deployer_changes.push(above);
            });
            Process::Hold
        }
        Depot::Apply => {
            let Some(r) = recipe else {
                return Process::Pass;
            };
            let outputs = match &sequenced {
                Some((a, _)) => sequenced_on_belt(game, at, item, a),
                None => {
                    // `RecipeApplier.applyRecipeOn` on one of the stack, `getMaxOutputCount` 4.
                    let seed =
                        game.world.game_time as u64 ^ (at[0] as u64).wrapping_mul(0x9E37_79B9);
                    let p = create_engine::processing::Recipe {
                        id: r.id.clone(),
                        ingredients: Vec::new(),
                        results: r.results.clone(),
                        time: 0,
                    };
                    create_engine::processing::roll(
                        &p,
                        &mut create_engine::processing::Rng::new(seed),
                    )
                    .into_iter()
                    .take(4)
                    .map(|(name, n)| {
                        let mut out = item.clone();
                        out.item = name;
                        out.count = n;
                        out.progress = None;
                        out.fan = Default::default();
                        out
                    })
                    .collect()
                }
            };
            let outputs = outputs
                .into_iter()
                .map(|mut o| {
                    o.locked = false;
                    o.angle = if upright(game, &o.item) {
                        180
                    } else {
                        random_angle(game, at)
                    };
                    o
                })
                .collect();
            deployer_used(game, &mut d, &r, above);
            with_world(|w| w.set_deployer(dim, above, d.clone()));
            Process::Convert {
                held: belt_left(item),
                outputs,
            }
        }
    }
}

/// The spout's belt callbacks for a stack on the belt at `at` under the spout `above`: held
/// while it can be filled, one of it filled at the cycle's end (a sequenced assembly's filling
/// step first, with that step's fluid).
pub(super) fn spout_on_belt(
    game: &mut Game<'_>,
    dim: usize,
    above: [i32; 3],
    item: &mut create_engine::belt::Item,
    locked: bool,
) -> Option<create_engine::belt::Process> {
    use create_engine::belt::Process;
    use create_engine::spout::{self, OnBelt};
    let mut s = with_world(|w| w.dims[dim].spouts.get(&above).copied()).flatten()?;
    if !ensure_recipes(game) {
        return Some(Process::Pass);
    }
    let sequenced = sequenced_for(&item.item, item.progress.as_ref(), "create:filling", None)
        .and_then(|(a, step)| {
            let (fluid, amount) = step.fluid?;
            let fluid = create_engine::tank::Fluid::of_id(&fluid)?;
            Some((a, fluid, amount))
        });
    let before = s.fluid;
    let result = RECIPES.with_borrow(|r| {
        let r = r.as_ref()?;
        Some(match &sequenced {
            Some((a, fluid, amount)) => {
                let step = [spout::Filling {
                    item: item.item.clone(),
                    fluid: *fluid,
                    amount: *amount,
                    result: a.transitional.clone(),
                }];
                spout::on_belt(&mut s, &item.item, &step, locked)
            }
            None => spout::on_belt(&mut s, &item.item, &r.filling, locked),
        })
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
                Some((a, _, _)) => sequenced_on_belt(game, at_below(above), item, a),
                None => {
                    let mut o = item.clone();
                    o.item = out;
                    o.count = 1;
                    o.progress = None;
                    vec![o]
                }
            };
            Process::Convert {
                held: belt_left(item),
                outputs,
            }
        }
    })
}

fn at_below(above: [i32; 3]) -> [i32; 3] {
    [above[0], above[1] - 2, above[2]]
}

/// A sequenced step done on the depot's held stack (one of it, `advance`): the transitional item
/// a step on (its progress kept with it), or a result rolled from the pool.
pub(super) fn sequenced_step_on_depot(
    game: &mut Game<'_>,
    dim: usize,
    depot: [i32; 3],
    count: u8,
    held: create_engine::depot::Held,
    assembly: &create_engine::sequenced::Assembly,
    progress: Option<&create_engine::sequenced::Progress>,
) -> create_engine::depot::Held {
    use create_engine::sequenced::Advanced;
    let seed = game.world.game_time as u64 ^ (depot[0] as u64).wrapping_mul(0x9E37_79B9);
    let roll = create_engine::processing::Rng::new(seed).next_float();
    match assembly.advance(progress, roll) {
        Advanced::Done { item, count: n } => {
            results_on_depot(game, dim, depot, count, held, vec![(item, n)])
        }
        Advanced::Nothing => results_on_depot(game, dim, depot, count, held, Vec::new()),
        Advanced::Next {
            item,
            progress,
            fraction,
        } => {
            let mut stack = horizon_client::items::ItemStack::new(item.as_str(), 1);
            super::create_items_body::set_sequenced_progress(
                &mut stack,
                &progress.id,
                progress.step,
                fraction,
            );
            // One held: it becomes the transitional item, centred; more: one goes to the first
            // free output, or drops above the depot.
            let angle = random_angle(game, depot);
            let mut next = held;
            let placed = game
                .native_with_mod_block_entity(depot, |be| {
                    be.items
                        .resize(be.items.len().max(create_engine::depot::SLOTS), None);
                    if count == 1 {
                        be.items[0] = Some(stack.clone());
                        next = create_engine::depot::Held {
                            angle,
                            position: 0.5,
                            locked: true,
                            ..held
                        };
                        return true;
                    }
                    if let Some(s) = be.items[0].as_mut() {
                        s.count -= 1;
                    }
                    let free = be.items[create_engine::depot::OUTPUTS]
                        .iter_mut()
                        .find(|s| s.is_none());
                    match free {
                        Some(slot) => {
                            *slot = Some(stack.clone());
                            true
                        }
                        None => false,
                    }
                })
                .unwrap_or(false);
            if !placed {
                game.native_run_command(&format!(
                    "summon minecraft:item {}.5 {}.0 {}.5 {{Item:{{id:'{}',count:1}}}}",
                    depot[0],
                    depot[1] + 1,
                    depot[2],
                    stack.item
                ));
            }
            with_world(|w| w.depot_changed(dim, depot));
            next
        }
    }
}

/// A processing result on a depot's held stack (`handleProcessingOnItem`): one of the stack
/// (all of it when it is one) becomes `results`: a single item is replaced by them (the first
/// held, centred; the rest in the outputs); otherwise the rest stays held and the results go to
/// the outputs. What finds no room drops above the depot. The held stack's motion after.
pub(super) fn results_on_depot(
    game: &mut Game<'_>,
    dim: usize,
    depot: [i32; 3],
    count: u8,
    held: create_engine::depot::Held,
    results: Vec<(String, u8)>,
) -> create_engine::depot::Held {
    use create_engine::processing;
    let limits: Vec<u8> = results
        .iter()
        .map(|(i, _)| game.items().get(i).map_or(64, |x| x.max_stack).clamp(1, 64) as u8)
        .collect();
    let stack = |item: &str, count: u8| horizon_client::items::ItemStack {
        item: item.into(),
        count: u32::from(count),
        damage: 0,
        extra: None,
    };
    let single = count == 1;
    let angle = random_angle(game, depot);
    let mut next = held;
    let dropped = game
        .native_with_mod_block_entity(depot, |be| {
            be.items
                .resize(be.items.len().max(create_engine::depot::SLOTS), None);
            let mut rest = results.iter().zip(&limits);
            if single {
                be.items[0] = None;
                if let Some(((first, n), _)) = rest.next() {
                    be.items[0] = Some(stack(first, *n));
                    next = create_engine::depot::Held {
                        angle,
                        position: 0.5,
                        locked: true,
                        ..held
                    };
                }
            } else if let Some(s) = be.items[0].as_mut() {
                s.count -= 1;
            }
            let mut outputs: Vec<processing::Slot> = be.items[create_engine::depot::OUTPUTS]
                .iter()
                .map(slot_of)
                .collect();
            let mut dropped = Vec::new();
            for ((i, n), limit) in rest {
                let left = processing::insert_stacked(&mut outputs, i, *n, *limit);
                if left > 0 {
                    dropped.push((i.clone(), left));
                }
            }
            for (slot, s) in be.items[create_engine::depot::OUTPUTS]
                .iter_mut()
                .zip(outputs)
            {
                *slot = s.map(|(i, n)| stack(&i, n));
            }
            dropped
        })
        .unwrap_or_default();
    for (i, n) in dropped {
        // Containers.dropItemStack at the depot's centre, half a block up.
        game.native_run_command(&format!(
            "summon minecraft:item {}.5 {}.0 {}.5 {{Item:{{id:'{i}',count:{n}}}}}",
            depot[0],
            depot[1] + 1,
            depot[2]
        ));
    }
    if results.is_empty() && single {
        with_world(|w| w.depot_put(dim, depot, None));
    }
    with_world(|w| w.depot_changed(dim, depot));
    next
}

thread_local! {
    /// The face of the last block an item was used on, and when: `native_place_block` doesn't
    /// say which face was clicked, the use before it does.
    pub(super) static CLICKED: std::cell::Cell<Option<(i64, [i32; 3])>> = const { std::cell::Cell::new(None) };
}

/// `BlazeBurnerBlock.useItemOn`: flint and steel lights an empty burner (a lit burner, no blaze);
/// fuel goes to the blaze (`tryInsert` with a player's overflow), one used up outside creative
/// (a lava bucket leaves its bucket).
pub(super) fn use_burner(game: &mut Game<'_>, dim: usize, pos: [i32; 3], canceled: &mut bool) {
    use create_engine::burner::Fuel;
    use create_engine::fan::Heat;
    let Some((held, _, plain)) = game.native_held() else {
        return;
    };
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some((heat, facing)) =
        with_world(|w| w.table.get(state.0).map(|(_, i)| (i.heat, i.facing))).flatten()
    else {
        return;
    };
    if heat == Some(Heat::None) {
        if held == "minecraft:flint_and_steel" {
            let facing = facing.map_or("north", dir_name);
            if let Some(lit) = state_with(
                game.pack.assets.registry,
                "create:lit_blaze_burner",
                &[("facing", facing)],
            ) {
                let dimension = game.dim.id().to_owned();
                game.native_set_block(&dimension, pos, lit, super::native_hooks::flags::UPDATE_ALL);
                *canceled = true;
            }
        }
        return;
    }
    if !plain {
        return;
    }
    let stack = horizon_client::items::ItemStack {
        item: held.as_str().into(),
        count: 1,
        damage: 0,
        extra: None,
    };
    let tagged = |t: &str| {
        game.items()
            .item_tags
            .get(t)
            .is_some_and(|m| m.contains(&held))
    };
    // A data map's superheating fuel (the blaze cake), the special tag; else a furnace fuel's
    // burn time, the regular tag (as coal).
    let (fuel, time) = if held == "create:blaze_cake" || tagged("create:blaze_burner_fuel/special")
    {
        (Fuel::Special, 3200)
    } else {
        let t = game.res.providers().burn_time(game.items(), &stack, false);
        if t > 0 {
            (Fuel::Normal, t)
        } else if tagged("create:blaze_burner_fuel/regular") {
            (Fuel::Normal, 1600)
        } else {
            return;
        }
    };
    let Some(heat) = with_world(|w| {
        w.fuel_burner(dim, pos, fuel, time, true)
            .then(|| w.dims[dim].burners.get(&pos).map(|b| b.heat()))
            .flatten()
    })
    .flatten() else {
        return;
    };
    *canceled = true;
    game.native_set_block_property(pos, "blaze", create_engine::burner::heat_name(heat));
    if !game.creative {
        let remainder = game.items().get(&held).and_then(|i| i.remainder.clone());
        game.native_take_items(&held, 1);
        if let Some(r) = remainder {
            game.native_give_items(&r, 1);
        }
    }
}

/// `BasinBlock.updateEntityAfterFallOn`: an item landing in a basin goes into its inputs as far
/// as it fits (`insertItem`).
pub(super) fn basin_take(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: &mut horizon_client::items::ItemStack,
) {
    let max = game
        .items()
        .get(&stack.item)
        .map_or(64, |x| x.max_stack)
        .clamp(1, 64) as u8;
    let item = stack.item.to_string();
    let count = stack.count.min(255) as u8;
    let left = game
        .native_with_mod_block_entity(pos, |be| {
            be.items
                .resize(be.items.len().max(create_engine::basin::SLOTS), None);
            let mut inputs: Vec<create_engine::processing::Slot> = be.items
                [create_engine::basin::INPUTS]
                .iter()
                .map(slot_of)
                .collect();
            let left = create_engine::processing::insert_stacked(&mut inputs, &item, count, max);
            for (slot, s) in be.items[create_engine::basin::INPUTS]
                .iter_mut()
                .zip(inputs)
            {
                *slot = s.map(|(i, n)| horizon_client::items::ItemStack {
                    item: i.as_str().into(),
                    count: u32::from(n),
                    damage: 0,
                    extra: None,
                });
            }
            left
        })
        .unwrap_or(count);
    if left != count {
        stack.count = u32::from(left);
        with_world(|w| w.dims[dim].mixer_checks.push([pos[0], pos[1] + 2, pos[2]]));
    }
}

/// Turning drills (`BlockBreakingKineticBlockEntity.tick`): the block in front breaks in steps
/// (`isBreakable`: not air, a fluid, unbreakable or `create:non_breakable`), dropping what it
/// drops; a drill that broke one looks again at its next lazy tick (every 10 ticks).
pub(super) fn tick_drills(game: &mut Game<'_>, dim: usize) {
    let drills = with_world(|w| w.dims[dim].drilling.clone()).unwrap_or_default();
    if drills.is_empty() {
        return;
    }
    let lazy = game.world.game_time % 10 == 0;
    for pos in drills {
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let Some(facing) =
            with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten()
        else {
            continue;
        };
        let o = facing.offset();
        let at = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
        let target = game.world.block(at[0], at[1], at[2]);
        let b = game.res.behaviours();
        let fluid = b.fluid(target).kind() != horizon_world::fluid::FluidKind::Empty
            && b.is_replaceable(target);
        let tagged = b
            .tag("create:non_breakable")
            .is_some_and(|t| t.contains(b.block(target)));
        let hardness = if b.is_air(target) || fluid || tagged {
            None
        } else {
            create_engine::drill::breakable(b.facts(target).hardness)
        };
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
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
        if step == create_engine::drill::Step::Broken {
            // Level.destroyBlock with drops (`BlockHelper.destroyBlock`).
            game.native_run_command(&format!(
                "setblock {} {} {} air destroy",
                at[0], at[1], at[2]
            ));
        }
    }
}

/// `SawBlockEntity.insertItem` for an item landing on a saw facing up, while it holds nothing.
pub(super) fn saw_take(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: &mut horizon_client::items::ItemStack,
) {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    if with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten() != Some(Dir::Up) {
        return;
    }
    let took = with_world(|w| {
        let s = w.dims[dim].saws.entry(pos).or_default();
        if !s.slots.is_empty() {
            return false;
        }
        s.slots = vec![(stack.item.to_string(), stack.count.min(64) as u8)];
        s.remaining = -1.0;
        s.applied = false;
        // `recipeIndex` stays with the saw between items.
        s.index = SAW_INDEX.with_borrow(|m| m.get(&(dim, pos)).copied().unwrap_or(0));
        s.progress = super::create_items_body::sequenced_progress(stack)
            .map(|(id, step)| create_engine::sequenced::Progress { id, step });
        true
    })
    .unwrap_or(false);
    if took {
        stack.count = stack.count.saturating_sub(64);
    }
}

/// Saws holding items (`SawBlockEntity.tick`): their cutting recipe in turn, applied near the
/// end, the results thrown off the way the saw turns (onto a belt or depot there, else as items).
pub(super) fn tick_saws(game: &mut Game<'_>, dim: usize) {
    use create_engine::saw::Step;
    let saws: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].saws.keys().copied().collect()).unwrap_or_default();
    if saws.is_empty() || !ensure_recipes(game) {
        return;
    }
    add_saw_stonecutting(game);
    for pos in saws {
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let Some(along_first) =
            with_world(|w| w.table.get(state.0).map(|(_, i)| i.along_first)).flatten()
        else {
            continue;
        };
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let Some(mut saw) = with_world(|w| w.dims[dim].saws.get(&pos).cloned()).flatten() else {
            continue;
        };
        let item = saw.slots.first().map(|s| s.0.clone()).unwrap_or_default();
        match saw.tick(speed) {
            Step::Wait => {}
            Step::Start => RECIPES.with_borrow(|r| {
                let Some(r) = r.as_ref() else { return };
                // A sequenced assembly's cutting step alone, when the item is at one
                // (`getRecipes`), for its processing time.
                if let Some((_, step)) = create_engine::sequenced::step_for(
                    &r.sequenced,
                    &item,
                    saw.progress.as_ref(),
                    "create:cutting",
                ) {
                    let time = create_engine::processing::Recipe {
                        id: String::new(),
                        ingredients: Vec::new(),
                        results: Vec::new(),
                        time: step.time,
                    };
                    saw.start(&[&time]);
                    return;
                }
                let list: Vec<&create_engine::processing::Recipe> = r
                    .cutting
                    .matching(&item)
                    .into_iter()
                    .map(|i| r.cutting.get(i))
                    .collect();
                saw.start(&list);
            }),
            Step::Apply => {
                let mut rng = create_engine::processing::Rng::new(
                    game.world.game_time as u64
                        ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9)
                        ^ (pos[2] as u64) << 20,
                );
                RECIPES.with_borrow(|r| {
                    let Some(r) = r.as_ref() else { return };
                    if let Some((a, _)) = create_engine::sequenced::step_for(
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
                    saw.apply(recipe, |i: &str| stack_limit(game, i), &mut rng);
                });
            }
            Step::Out => {
                let m = create_engine::saw::movement(along_first, speed);
                // A belt or depot it turns toward takes the results (`DirectBeltInputBehaviour`
                // from that side); what it doesn't take waits in the saw.
                let next = [pos[0] + m[0], pos[1], pos[2] + m[2]];
                let facing = Dir::ALL.into_iter().find(|d| d.offset() == m);
                let belt_input = with_world(|w| {
                    let d = &w.dims[dim];
                    d.depots.contains_key(&next) || d.belt_of.contains_key(&next)
                }) == Some(true);
                if let (true, Some(facing)) = (belt_input, facing) {
                    let progress = saw.progress.clone();
                    let transitional = progress.as_ref().and_then(transitional_of).map(|t| t.0);
                    let mut left = Vec::new();
                    for (item, count) in std::mem::take(&mut saw.slots) {
                        let mut one = create_engine::belt::Item::new(
                            &item,
                            count,
                            random_angle(game, pos),
                            upright(game, &item),
                        );
                        if transitional.as_deref() == Some(item.as_str()) {
                            one.progress = progress.clone();
                        }
                        let n =
                            insert_into_from(game, dim, next, facing, &one, false).unwrap_or(count);
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
                let (x, y, z) = (
                    f64::from(pos[0]) + 0.5 + f64::from(m[0]) * 0.5,
                    f64::from(pos[1]) + 1.0,
                    f64::from(pos[2]) + 0.5 + f64::from(m[2]) * 0.5,
                );
                let (vx, vz) = (f64::from(m[0]) * 0.0625, f64::from(m[2]) * 0.0625);
                for (item, count) in &saw.slots {
                    // A transitional item keeps its progress.
                    let components = match saw.progress.as_ref().and_then(transitional_of) {
                        Some((t, p, fraction)) if t == *item => format!(
                            ",components:{{\"minecraft:custom_data\":{{\"create:sequenced_assembly\":{{id:\"{}\",step:{},progress:{}f}}}}}}",
                            p.id, p.step, fraction
                        ),
                        _ => String::new(),
                    };
                    game.native_run_command(&format!(
                        "summon minecraft:item {x} {y} {z} {{Item:{{id:'{item}',count:{count}{components}}},Motion:[{vx}d,0.125d,{vz}d]}}"
                    ));
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

thread_local! {
    /// Each saw's `recipeIndex` while it holds nothing (an empty saw keeps no other state).
    static SAW_INDEX: RefCell<HashMap<(usize, [i32; 3]), usize>> = RefCell::new(HashMap::new());
}

/// The heat under a basin (`BasinBlockEntity.getHeatLevelOf`): a blaze burner's, smouldering for
/// a lit burner or another passive heater, else none.
pub(super) fn basin_heat(game: &Game<'_>, basin: [i32; 3]) -> create_engine::fan::Heat {
    use create_engine::fan::Heat;
    let state = game.world.block(basin[0], basin[1] - 1, basin[2]);
    if let Some(h) = with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.heat)).flatten() {
        return h;
    }
    let b = game.res.behaviours();
    if b.tag("create:passive_boiler_heaters")
        .is_some_and(|t| t.contains(b.block(state)))
    {
        // BlockHelper.isNotUnheated: an unlit campfire is no heater.
        let lit = game
            .pack
            .assets
            .registry
            .state(state)
            .and_then(|s| s.properties.get("lit").cloned());
        if lit.as_deref() != Some("false") {
            return Heat::Smouldering;
        }
    }
    Heat::None
}

/// The slots of the basin at `pos`, all 18.
pub(super) fn basin_slots(
    game: &Game<'_>,
    pos: [i32; 3],
) -> Option<Vec<create_engine::processing::Slot>> {
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(horizon_client::containers::BlockEntity::Mod(m)) => {
            let mut v: Vec<_> = m.items.iter().map(slot_of).collect();
            v.resize(create_engine::basin::SLOTS, None);
            Some(v)
        }
        _ => None,
    }
}

/// Mechanical mixers: those asked to look at their basin (`updateBasin`: turning, not running,
/// a recipe the basin can do), then the running ones' cycle (`tick`), mixing at the bottom
/// (`applyBasinRecipe`) and going on with the same recipe while it still fits.
/// Whether the block at `p` is a basin.
pub(super) fn basin_at(game: &Game<'_>, p: [i32; 3]) -> bool {
    let s = game.world.block(p[0], p[1], p[2]).0;
    with_world(|w| w.table.get(s).map(|(b, _)| b)).flatten() == Some(create_engine::Block::Basin)
}

/// A basin's tanks (none: empty).
pub(super) fn basin_tanks(dim: usize, pos: [i32; 3]) -> create_engine::basin::Tanks {
    with_world(|w| w.dims[dim].basin_fluids.get(&pos).copied())
        .flatten()
        .unwrap_or_default()
}

pub(super) fn tick_mixers(game: &mut Game<'_>, dim: usize) {
    use create_engine::basin::{self, MixStep};
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].mixer_checks)).unwrap_or_default();
    if !checks.is_empty() {
        add_shapeless_mixing(game);
    }
    let running = with_world(|w| w.dims[dim].mixing.clone()).unwrap_or_default();
    if checks.is_empty() && running.is_empty() {
        return;
    }
    if !ensure_recipes(game) {
        return;
    }
    for pos in running {
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let Some(mut m) = with_world(|w| w.dims[dim].mixers.get(&pos).copied()).flatten() else {
            continue;
        };
        let time = RECIPES.with_borrow(|r| {
            r.as_ref()
                .and_then(|r| m.recipe.and_then(|i| r.mixing.get(i)).map(|x| x.time))
                .unwrap_or(0)
        });
        let basin_pos = [pos[0], pos[1] - 2, pos[2]];
        match m.tick(speed, time) {
            MixStep::None => {}
            MixStep::Done => checks.push(pos),
            MixStep::Apply => {
                let heat = basin_heat(game, basin_pos);
                let mut rng = create_engine::processing::Rng::new(
                    game.world.game_time as u64
                        ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9)
                        ^ (pos[2] as u64) << 20,
                );
                if let (Some(mut slots), Some(i)) = (basin_slots(game, basin_pos), m.recipe) {
                    let limit = |i: &str| stack_limit(game, i);
                    let mut tanks = basin_tanks(dim, basin_pos);
                    let done = RECIPES.with_borrow(|r| {
                        r.as_ref()
                            .and_then(|r| r.mixing.get(i))
                            .is_some_and(|recipe| {
                                basin::apply(
                                    &mut slots, &mut tanks, recipe, heat, &limit, &mut rng, false,
                                )
                            })
                    });
                    if done {
                        with_world(|w| w.set_basin_fluids(dim, basin_pos, tanks));
                        let again = RECIPES.with_borrow(|r| {
                            r.as_ref()
                                .and_then(|r| r.mixing.get(i))
                                .is_some_and(|recipe| {
                                    basin::apply(
                                        &mut slots.clone(),
                                        &mut tanks.clone(),
                                        recipe,
                                        heat,
                                        &limit,
                                        &mut rng,
                                        true,
                                    )
                                })
                        });
                        game.native_with_mod_block_entity(basin_pos, |be| {
                            be.items = slots
                                .iter()
                                .map(|s| {
                                    s.as_ref().map(|(i, n)| horizon_client::items::ItemStack {
                                        item: i.as_str().into(),
                                        count: u32::from(*n),
                                        damage: 0,
                                        extra: None,
                                    })
                                })
                                .collect();
                        });
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
            press_on_basin(game, dim, pos);
            continue;
        };
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let basin_pos = [pos[0], pos[1] - 2, pos[2]];
        let is_basin = {
            let s = game.world.block(basin_pos[0], basin_pos[1], basin_pos[2]);
            with_world(|w| w.table.get(s.0).map(|(b, _)| b)).flatten()
                == Some(create_engine::Block::Basin)
        };
        if !is_basin {
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
        let Some(slots) = basin_slots(game, basin_pos) else {
            continue;
        };
        let heat = basin_heat(game, basin_pos);
        let limit = |i: &str| stack_limit(game, i);
        let found = RECIPES.with_borrow(|r| {
            r.as_ref().and_then(|r| {
                basin::matching(
                    &r.mixing,
                    &slots,
                    &basin_tanks(dim, basin_pos),
                    heat,
                    &limit,
                )
                .first()
                .copied()
            })
        });
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
pub(super) fn press_on_basin(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    use create_engine::{basin, press};
    let Some(p) = with_world(|w| w.dims[dim].presses.get(&pos).copied()).flatten() else {
        return;
    };
    let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
    if p.running || speed == 0.0 {
        return;
    }
    let basin_pos = [pos[0], pos[1] - 2, pos[2]];
    let Some(slots) = basin_slots(game, basin_pos).filter(|_| {
        let s = game.world.block(basin_pos[0], basin_pos[1], basin_pos[2]);
        with_world(|w| w.table.get(s.0).map(|(b, _)| b)).flatten()
            == Some(create_engine::Block::Basin)
    }) else {
        return;
    };
    let heat = basin_heat(game, basin_pos);
    let limit = |i: &str| stack_limit(game, i);
    let tanks = basin_tanks(dim, basin_pos);
    let found = RECIPES.with_borrow(|r| {
        r.as_ref().and_then(|r| {
            let i = *basin::matching(&r.compacting, &slots, &tanks, heat, &limit).first()?;
            Some((i, r.compacting[i].ingredients.len()))
        })
    });
    // Square crafting recipes of one item (`canCompress`), with the compacting ones: the one
    // with the most ingredients first (`getMatchingRecipes`' order).
    let crafted = compressible(game, &slots).into_iter().find(|c| {
        !basin::matching(std::slice::from_ref(c), &slots, &tanks, heat, &limit).is_empty()
    });
    let crafted = crafted.filter(|c| found.is_none_or(|(_, n)| c.ingredients.len() > n));
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

/// `SawBlockEntity.getRecipes` with `allowStonecuttingOnSaw` (on by default): vanilla's
/// stonecutting recipes cut on a saw too, after Create's cutting ones, each 50 ticks (`start`:
/// only a cutting recipe has its own time). Read once per recipe table, when a saw first looks.
fn add_saw_stonecutting(game: &mut Game<'_>) {
    use super::native_hooks::NativeRecipeKind;
    if RECIPES.with_borrow(|r| r.as_ref().is_none_or(|r| r.saw_stonecutting)) {
        return;
    }
    let Some(all) = game.native_recipes() else {
        return;
    };
    let id = |i: u32| all.items.get(i as usize).map(|x| x.id.clone());
    let found: Vec<create_engine::processing::Recipe> = all
        .recipes
        .iter()
        .filter(|r| r.kind == NativeRecipeKind::Stonecutting)
        .filter_map(|r| {
            let input: Vec<String> = r.slots.first()?.iter().filter_map(|x| id(x.item)).collect();
            let result = r.results.first()?;
            Some(create_engine::processing::Recipe {
                id: r.id.clone(),
                ingredients: vec![input],
                results: vec![create_engine::processing::Output {
                    item: id(result.item)?,
                    count: result.count.min(64) as u8,
                    chance: 1.0,
                }],
                time: 50,
            })
        })
        .collect();
    RECIPES.with_borrow_mut(|r| {
        if let Some(r) = r.as_mut() {
            let mut all = std::mem::take(&mut r.cutting.recipes);
            all.extend(found);
            r.cutting = create_engine::processing::Book::new(all);
            r.saw_stonecutting = true;
        }
    });
}

/// `MechanicalMixerBlockEntity.matchStaticFilters`: shapeless crafting recipes of more than one
/// ingredient that the press doesn't compress (`canCompress`) mix in a basin too, after Create's
/// own (`allowShapelessInMixer`). Read once per recipe table, when a mixer first looks.
fn add_shapeless_mixing(game: &mut Game<'_>) {
    use super::native_hooks::NativeRecipeKind;
    if RECIPES.with_borrow(|r| r.as_ref().is_none_or(|r| r.shapeless_mixing)) {
        return;
    }
    let Some(all) = game.native_recipes() else {
        return;
    };
    let id = |i: u32| all.items.get(i as usize).map(|x| x.id.clone());
    let mut found = Vec::new();
    for r in &all.recipes {
        if r.kind != NativeRecipeKind::CraftingShapeless || r.slots.len() < 2 {
            continue;
        }
        let ingredients: Vec<Vec<String>> = r
            .slots
            .iter()
            .map(|s| s.iter().filter_map(|x| id(x.item)).collect())
            .collect();
        let same = ingredients.windows(2).all(|w| w[0] == w[1]);
        if same && matches!(ingredients.len(), 4 | 9) {
            continue;
        }
        let Some(result) = r.results.first() else {
            continue;
        };
        let Some(item) = id(result.item) else {
            continue;
        };
        found.push(create_engine::basin::Recipe {
            id: r.id.clone(),
            ingredients,
            results: vec![create_engine::processing::Output {
                item,
                count: result.count.min(64) as u8,
                chance: 1.0,
            }],
            time: 0,
            heat: Default::default(),
            fluids: Vec::new(),
            fluid_results: Vec::new(),
        });
    }
    RECIPES.with_borrow_mut(|r| {
        if let Some(r) = r.as_mut() {
            r.mixing.extend(found);
            r.shapeless_mixing = true;
        }
    });
}

thread_local! {
    /// The square crafting recipe a press is compacting with, by dimension and press.
    static PRESS_CRAFTED: RefCell<HashMap<(usize, [i32; 3]), create_engine::basin::Recipe>> =
        RefCell::new(HashMap::new());
    /// `canCompress` answers by item: its 3×3 and 2×2 crafting recipes of only it (with the
    /// recipe table they were read from).
    static COMPRESSED: RefCell<(usize, HashMap<String, Vec<create_engine::basin::Recipe>>)> =
        RefCell::new((0, HashMap::new()));
}

/// The crafting recipes made of 9, else 4, of one item in the basin's inputs
/// (`MechanicalPressBlockEntity.canCompress`: 4 or 9 ingredients, all the same), as basin
/// recipes, 9 first; looked up once per item.
fn compressible(
    game: &Game<'_>,
    slots: &[create_engine::processing::Slot],
) -> Vec<create_engine::basin::Recipe> {
    let table = game.native_recipes_of_type("create:compacting").as_ptr() as usize;
    let mut items: Vec<&str> = slots[create_engine::basin::INPUTS]
        .iter()
        .flatten()
        .map(|(i, _)| i.as_str())
        .collect();
    items.sort();
    items.dedup();
    COMPRESSED.with_borrow_mut(|(at, cache)| {
        if *at != table {
            cache.clear();
            *at = table;
        }
        let mut out = Vec::new();
        for item in items {
            let found = cache.entry(item.to_string()).or_insert_with(|| {
                [3usize, 2]
                    .into_iter()
                    .filter_map(|size| {
                        let square: Vec<Option<horizon_client::items::ItemStack>> = (0..size
                            * size)
                            .map(|_| Some(horizon_client::items::ItemStack::new(item, 1)))
                            .collect();
                        let made = game.recipes().craft(&square, size)?;
                        Some(create_engine::basin::Recipe {
                            id: format!("crafting:{item}:{size}"),
                            ingredients: vec![vec![item.to_string()]; size * size],
                            results: vec![create_engine::processing::Output {
                                item: made.item.to_string(),
                                count: made.count.min(64) as u8,
                                chance: 1.0,
                            }],
                            time: 0,
                            heat: Default::default(),
                            fluids: Vec::new(),
                            fluid_results: Vec::new(),
                        })
                    })
                    .collect()
            });
            out.extend(found.iter().cloned());
        }
        out.sort_by_key(|r| std::cmp::Reverse(r.ingredients.len()));
        out
    })
}

/// `tryProcessInBasin` at the head's lowest point: the press's compacting recipe on the basin.
pub(super) fn compact_in_basin(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    use create_engine::basin;
    let Some(i) = with_world(|w| w.dims[dim].press_basin.get(&pos).copied()).flatten() else {
        return;
    };
    let basin_pos = [pos[0], pos[1] - 2, pos[2]];
    let Some(mut slots) = basin_slots(game, basin_pos) else {
        return;
    };
    let heat = basin_heat(game, basin_pos);
    let mut rng = create_engine::processing::Rng::new(
        game.world.game_time as u64
            ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9)
            ^ (pos[2] as u64) << 20,
    );
    let limit = |i: &str| stack_limit(game, i);
    let mut tanks = basin_tanks(dim, basin_pos);
    let crafted = PRESS_CRAFTED.with_borrow(|m| m.get(&(dim, pos)).cloned());
    let done = match crafted.filter(|_| i == usize::MAX) {
        Some(recipe) => basin::apply(
            &mut slots, &mut tanks, &recipe, heat, &limit, &mut rng, false,
        ),
        None => RECIPES.with_borrow(|r| {
            r.as_ref()
                .and_then(|r| r.compacting.get(i))
                .is_some_and(|recipe| {
                    basin::apply(
                        &mut slots, &mut tanks, recipe, heat, &limit, &mut rng, false,
                    )
                })
        }),
    };
    if done {
        with_world(|w| w.set_basin_fluids(dim, basin_pos, tanks));
        game.native_with_mod_block_entity(basin_pos, |be| {
            be.items = slots
                .iter()
                .map(|s| {
                    s.as_ref().map(|(i, n)| horizon_client::items::ItemStack {
                        item: i.as_str().into(),
                        count: u32::from(*n),
                        damage: 0,
                        extra: None,
                    })
                })
                .collect();
        });
    }
}

/// Burning blaze burners (`BlazeBurnerBlockEntity.tick`): the fuel burns down; a changed heat is
/// written to the block (`blaze`), which fans and basins read.
pub(super) fn tick_burners(game: &mut Game<'_>, dim: usize) {
    let changed = with_world(|w| {
        if w.dims[dim].burning.is_empty() {
            Vec::new()
        } else {
            w.tick_burners(dim)
        }
    })
    .unwrap_or_default();
    for (pos, heat) in changed {
        game.native_set_block_property(pos, "blaze", create_engine::burner::heat_name(heat));
    }
}

/// Crushing wheels: the controllers between wheels turning against each other
/// (`CrushingWheelBlock.updateControllers`, after a wheel is placed, broken or changes speed),
/// then the turning controllers' `tick`.
pub(super) fn tick_crushers(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].crusher_checks)).unwrap_or_default();
    checks.sort();
    checks.dedup();
    for pos in checks {
        fix_controllers(game, dimension, dim, pos);
    }
    let running = with_world(|w| w.dims[dim].running_crushers.clone()).unwrap_or_default();
    if running.is_empty() || !ensure_recipes(game) {
        return;
    }
    for pos in running {
        tick_crusher(game, dimension, dim, pos);
    }
}

pub(super) fn fix_controllers(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    use create_engine::{Block, crusher};
    let registry = game.pack.assets.registry;
    let at = |p: [i32; 3], side: Dir, n: i32| {
        let o = side.offset();
        [p[0] + o[0] * n, p[1] + o[1] * n, p[2] + o[2] * n]
    };
    let block_of = |game: &Game<'_>, p: [i32; 3]| {
        let s = game.world.block(p[0], p[1], p[2]);
        with_world(|w| w.table.get(s.0).map(|(b, i)| (b, i.axis))).flatten()
    };
    let wheel = block_of(game, pos).filter(|b| b.0 == Block::CrushingWheel);
    let air = BlockStateId(0);
    for side in Dir::ALL {
        let cpos = at(pos, side, 1);
        let existing = block_of(game, cpos).is_some_and(|b| b.0 == Block::CrushingWheelController);
        let want = wheel.and_then(|(_, axis)| {
            let axis = axis?;
            let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
            let opos = at(pos, side, 2);
            let other = block_of(game, opos)
                .filter(|b| b.0 == Block::CrushingWheel)
                .and_then(|b| b.1)
                .map(|a| {
                    (
                        a,
                        with_world(|w| w.speed(dim, opos)).flatten().unwrap_or(0.0),
                    )
                });
            crusher::controller(axis, side, speed, other).map(|c| (c, speed))
        });
        match want {
            // A wheel that went (`onRemove`) or no partner: no controller.
            None => {
                if existing && (wheel.is_none() || wheel.and_then(|w| w.1) != Some(side.axis())) {
                    game.native_set_block(
                        dimension,
                        cpos,
                        air,
                        super::native_hooks::flags::UPDATE_ALL,
                    );
                }
            }
            Some(((valid, facing), speed)) => {
                let here = game.world.block(cpos[0], cpos[1], cpos[2]);
                if !existing && !game.res.behaviours().is_replaceable(here) {
                    continue;
                }
                let Some(state) = state_with(
                    registry,
                    "create:crushing_wheel_controller",
                    &[
                        ("facing", dir_name(facing)),
                        ("valid", if valid { "true" } else { "false" }),
                    ],
                ) else {
                    continue;
                };
                if here != state {
                    game.native_set_block(
                        dimension,
                        cpos,
                        state,
                        super::native_hooks::flags::UPDATE_ALL,
                    );
                }
                let crushing = if valid {
                    crusher::crushing_speed(speed)
                } else {
                    0.0
                };
                with_world(|w| w.set_crusher_speed(dim, cpos, crushing));
            }
        }
    }
}

/// `CrushingWheelControllerBlockEntity.tick` (and the controller block's `entityInside`).
pub(super) fn tick_crusher(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    use create_engine::{crusher, processing};
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some(facing) = with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten()
    else {
        return;
    };
    let Some((mut c, speed)) = with_world(|w| w.dims[dim].crushers.get(&pos).cloned()).flatten()
    else {
        return;
    };
    let cell = |grow: f64| {
        let lo = pos.map(|v| f64::from(v) - grow);
        let hi = pos.map(|v| f64::from(v) + 1.0 + grow);
        (lo, hi)
    };
    let before = c.clone();
    if let Some(id) = c.entity {
        // The held item: drawn to the middle and in; taken a quarter past the middle.
        let (lo, hi) = cell(0.5);
        let mut found = None;
        items_in(game, dimension, lo, hi, |i, stack, at, _| {
            if i == id {
                found = Some((stack.item.to_string(), stack.count.min(255) as u8, at));
            }
        });
        match found {
            None => c.entity = None,
            Some((item, count, at)) => {
                let (motion, taken) = crusher::hold(pos, facing, speed, at);
                if taken {
                    let time = RECIPES.with_borrow(|r| {
                        let r = r.as_ref()?;
                        crusher::recipe(&r.crushing, &r.milling, &item).map(|x| x.time)
                    });
                    crusher::intake(&mut c, &item, count, time);
                }
                items_in(game, dimension, lo, hi, |i, stack, _, v| {
                    if i == id {
                        if taken {
                            stack.count = 0;
                        } else {
                            *v = motion;
                        }
                    }
                });
            }
        }
    } else if c.slots.is_empty() {
        // `checkEntityForProcessing`: the first item inside starts it.
        let (lo, hi) = cell(0.0);
        let mut first = None;
        items_in(game, dimension, lo, hi, |i, _, at, _| {
            if !crusher::bypassed(pos, facing, at) {
                first.get_or_insert(i);
            }
        });
        c.entity = first;
    } else {
        match crusher::tick_inventory(&mut c, speed) {
            crusher::Step::Wait => {}
            crusher::Step::Apply => {
                let max =
                    |i: &str| game.items().get(i).map_or(64, |x| x.max_stack).clamp(1, 64) as u8;
                let mut rng = processing::Rng::new(
                    game.world.game_time as u64
                        ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9)
                        ^ (pos[2] as u64) << 20,
                );
                let item = c.slots[0].0.clone();
                RECIPES.with_borrow(|r| {
                    let recipe = r
                        .as_ref()
                        .and_then(|r| crusher::recipe(&r.crushing, &r.milling, &item));
                    crusher::apply(&mut c, recipe, max, &mut rng);
                });
            }
            crusher::Step::Output => {
                // Facing anywhere but up: a belt or depot below (and on, past a sideways
                // facing) takes the results from that side; what it doesn't take waits.
                let next = [
                    pos[0]
                        + if facing.axis() == Axis::Y {
                            0
                        } else {
                            facing.offset()[0]
                        },
                    pos[1] - 1,
                    pos[2]
                        + if facing.axis() == Axis::Y {
                            0
                        } else {
                            facing.offset()[2]
                        },
                ];
                let belt_input = facing != Dir::Up
                    && with_world(|w| {
                        let d = &w.dims[dim];
                        d.depots.contains_key(&next) || d.belt_of.contains_key(&next)
                    }) == Some(true);
                if belt_input {
                    let mut left = Vec::new();
                    for (item, count) in std::mem::take(&mut c.slots) {
                        let one = create_engine::belt::Item::new(
                            &item,
                            count,
                            random_angle(game, pos),
                            upright(game, &item),
                        );
                        let n =
                            insert_into_from(game, dim, next, facing, &one, false).unwrap_or(count);
                        if n > 0 {
                            left.push((item, n));
                        }
                    }
                    c.slots = left;
                } else {
                    // Results thrown out past the facing side (`outPos`, `outSpeed`).
                    let (at, v) = crusher::output(pos, facing);
                    for (item, count) in std::mem::take(&mut c.slots) {
                        game.native_run_command(&format!(
                        "summon minecraft:item {} {} {} {{Item:{{id:'{item}',count:{count}}},Motion:[{}d,{}d,{}d]}}",
                        at[0], at[1], at[2], v[0], v[1], v[2]
                    ));
                    }
                }
            }
        }
    }
    with_world(|w| {
        if let Some(e) = w.dims[dim].crushers.get_mut(&pos) {
            e.0 = c.clone();
        }
        if c.slots != before.slots {
            w.mark_unsaved(dim, pos);
        }
    });
}

/// Encased fans that turn (`EncasedFanBlockEntity.tick`): the current looked at again when due
/// (`AirCurrent.rebuild`), then the depots and belts in it processed
/// (`tickAffectedHandlers`). Stopped fans cost nothing.
pub(super) fn tick_fans(game: &mut Game<'_>, dimension: &str, dim: usize) {
    check_nozzles(game, dimension, dim);
    let fans = with_world(|w| w.fans_to_tick(dim)).unwrap_or_default();
    if fans.is_empty() {
        return;
    }
    let recipes = ensure_recipes(game);
    // Item entities' progress, kept for those still in a current this tick.
    let mut kept = std::collections::HashMap::new();
    let mut old = with_world(|w| std::mem::take(&mut w.dims[dim].item_fan)).unwrap_or_default();
    for pos in fans {
        let Some((rebuild, speed, facing)) = with_world(|w| {
            let f = w.dims[dim].fans.get(&pos)?;
            Some((f.rebuild, f.speed, f.facing))
        })
        .flatten() else {
            continue;
        };
        if rebuild {
            let (current, handlers) = match facing {
                Some(facing) if speed != 0.0 => fan_current(game, dim, pos, facing, speed),
                _ => Default::default(),
            };
            with_world(|w| {
                if let Some(f) = w.dims[dim].fans.get_mut(&pos) {
                    f.rebuild = false;
                    f.changed |= f.current != current;
                    f.current = current;
                    f.handlers = handlers;
                }
            });
        }
        // The nozzle on its front (`NozzleBlockEntity.tick`): it does something only while
        // its fan turns, so it ticks with it (and once more when it stops, to let go).
        if let Some(f) = facing {
            let o = f.offset();
            tick_nozzle(
                game,
                dimension,
                dim,
                [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]],
                (speed, f),
            );
        }
        if speed == 0.0 {
            continue;
        }
        // `entitySearchCooldown`: the entities in the current are looked for every 6th tick.
        let search = with_world(|w| {
            let f = w.dims[dim].fans.get_mut(&pos)?;
            let search = f.search_cooldown == 0;
            f.search_cooldown = if search { 5 } else { f.search_cooldown - 1 };
            Some(search)
        })
        .flatten()
        .unwrap_or(false);
        fan_on_entities(game, dimension, dim, pos, search);
        if !recipes {
            continue;
        }
        fan_on_items(game, dimension, dim, pos, search, &mut old, &mut kept);
        let handlers = with_world(|w| w.dims[dim].fans.get(&pos).map(|f| f.handlers.clone()))
            .flatten()
            .unwrap_or_default();
        for (at, kind) in handlers {
            let Some(kind) = kind else {
                continue;
            };
            fan_on_depot(game, dim, at, kind);
            fan_on_belt(game, dim, at, kind);
        }
    }
    with_world(|w| w.dims[dim].item_fan = kept);
}

/// Nozzles whose neighbour changed break, with their drops, when the block behind them is no
/// longer an encased fan facing their way (`NozzleBlock.neighborChanged`, `canSurvive`).
fn check_nozzles(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let checks = with_world(|w| {
        let c = &mut w.dims[dim].nozzle_checks;
        if c.is_empty() {
            Vec::new()
        } else {
            std::mem::take(c)
        }
    })
    .unwrap_or_default();
    for pos in checks {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let Some(facing) = with_world(|w| match w.table.get(state) {
            Some((create_engine::Block::Nozzle, i)) => i.facing,
            _ => None,
        })
        .flatten() else {
            continue;
        };
        let o = facing.offset();
        let behind = game
            .world
            .block(pos[0] - o[0], pos[1] - o[1], pos[2] - o[2])
            .0;
        let fan = with_world(|w| match w.table.get(behind) {
            Some((create_engine::Block::EncasedFan, i)) => i.facing,
            _ => None,
        })
        .flatten();
        if !create_engine::nozzle::survives(facing, fan) {
            game.native_run_command(&format!(
                "execute in {dimension} run setblock {} {} {} air destroy",
                pos[0], pos[1], pos[2]
            ));
        }
    }
}

/// `NozzleBlockEntity.tick` for the nozzle at `pos` on the front of a ticking fan (`fan`: its
/// speed and facing): every sixth tick it looks for the entities within half its range that it
/// sees (`lazyTick`, `canSee` through whole blocks); then it takes its range from the fan and
/// pushes (or pulls) each caught one by `(range - distance)` along the line from its centre,
/// 1/128 for items and 1/32 for anything else (mobs, players, carts...) a tick, forgetting its
/// fall (`fallDistance = 0`). Sneaking players and flyers (`isPlayerCreativeFlying`; Horizon's
/// `flying` is a spectator's too) are let go. Pulling more than 256 discards the items and mobs
/// it holds. Not here: the explosion of an overloaded nozzle (`Level.explode`) and its particles.
fn tick_nozzle(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3], fan: (f32, Dir)) {
    use create_engine::nozzle::{self, Push, Target};
    let Some(mut n) = with_world(|w| w.dims[dim].nozzles.remove(&pos)).flatten() else {
        return;
    };
    if n.facing != fan.1 {
        with_world(|w| w.dims[dim].nozzles.insert(pos, n));
        return;
    }
    let reach = |range: f32| {
        let c = nozzle::center(pos);
        let r = f64::from(range);
        (c.map(|v| v - r), c.map(|v| v + r))
    };
    // The entities that aren't items (those are `items_in`'s), in a box.
    let others = |game: &Game<'_>, lo: [f64; 3], hi: [f64; 3]| {
        let mut all = game.native_entities_in(dimension, lo, hi);
        all.retain(|e| e.kind != "minecraft:item");
        all
    };
    if n.lazy_due() && n.range != 0.0 {
        let (lo, hi) = nozzle::search_box(pos, n.range);
        let mut seen: Vec<(Target, [f64; 3])> = Vec::new();
        let (rlo, rhi) = reach(n.range);
        let mut alive: Vec<Target> = Vec::new();
        items_in(game, dimension, rlo, rhi, |id, _, at, _| {
            alive.push(Target::Item(id));
            if create_engine::belt::item_touches(at, lo, hi) {
                seen.push((Target::Item(id), at));
            }
        });
        for e in others(game, rlo, rhi) {
            let t = Target::Entity(e.id);
            alive.push(t);
            // Sneaking players and flyers are skipped.
            if e.sneaking || e.flying {
                continue;
            }
            if (0..3).all(|a| e.min[a] < hi[a] && e.max[a] > lo[a]) {
                seen.push((t, e.pos));
            }
        }
        for (t, at) in seen {
            n.look_at(pos, t, at, || {
                nozzle::sees(pos, at, |p| p != pos && blocked(game, p))
            });
        }
        n.entities.retain(|t| alive.contains(t));
        if nozzle::overloaded(n.pushing, n.entities.len()) {
            let gone = std::mem::take(&mut n.entities);
            items_in(game, dimension, rlo, rhi, |id, stack, _, _| {
                if gone.contains(&Target::Item(id)) {
                    stack.count = 0;
                }
            });
            for t in gone {
                if let Target::Entity(id) = t {
                    game.native_remove_mob(dimension, id);
                }
            }
        }
    }
    n.update_range(Some(fan));
    if !n.entities.is_empty() {
        let (range, pushing) = (n.range, n.pushing);
        let (lo, hi) = reach(range);
        let mut released: Vec<Target> = Vec::new();
        let mut found: Vec<Target> = Vec::new();
        let caught = &n.entities;
        items_in(game, dimension, lo, hi, |id, _, at, v| {
            let t = Target::Item(id);
            if !caught.contains(&t) {
                return;
            }
            found.push(t);
            match nozzle::push(pos, range, pushing, at, true, false) {
                Push::Release => released.push(t),
                Push::Hold => {}
                Push::Add(add) => *v = [v[0] + add[0], v[1] + add[1], v[2] + add[2]],
            }
        });
        if caught.iter().any(|t| !t.is_item()) {
            for e in others(game, lo, hi) {
                let t = Target::Entity(e.id);
                if !n.entities.contains(&t) {
                    continue;
                }
                found.push(t);
                match nozzle::push(pos, range, pushing, e.pos, false, e.sneaking || e.flying) {
                    Push::Release => released.push(t),
                    Push::Hold => {}
                    Push::Add(add) => {
                        let v = e.velocity;
                        game.native_set_delta_movement(
                            dimension,
                            e.id,
                            [v[0] + add[0], v[1] + add[1], v[2] + add[2]],
                        );
                        game.native_reset_fall_distance(dimension, e.id);
                    }
                }
            }
        }
        // Gone, or out of the box and so out of range: let go.
        n.entities
            .retain(|t| found.contains(t) && !released.contains(t));
    }
    with_world(|w| w.dims[dim].nozzles.insert(pos, n));
}

/// `AirCurrent.tickAffectedEntities` for items: those its search caught (every 5 ticks) and still
/// in its box are pushed along the current, and processed by the type at their distance
/// (`FanProcessing.applyProcessing`): the first result replaces the stack, the others are
/// dropped beside it with its motion, none discards it.
pub(super) fn fan_on_items(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    search: bool,
    old: &mut std::collections::HashMap<u32, create_engine::fan::Progress>,
    kept: &mut std::collections::HashMap<u32, create_engine::fan::Progress>,
) {
    use create_engine::{fan, processing};
    let Some((facing, speed, current)) = with_world(|w| {
        let f = w.dims[dim].fans.get(&pos)?;
        Some((f.facing?, f.speed, f.current.clone()))
    })
    .flatten() else {
        return;
    };
    let Some((lo, hi)) = fan::bounds(pos, facing, current.max) else {
        return;
    };
    let mut items = Vec::new();
    items_in(game, dimension, lo, hi, |id, stack, at, v| {
        items.push((id, stack.item.to_string(), stack.count, at, *v));
    });
    let caught: Vec<u32> = if search {
        let ids: Vec<u32> = items.iter().map(|i| i.0).collect();
        with_world(|w| {
            if let Some(f) = w.dims[dim].fans.get_mut(&pos) {
                f.caught = ids.clone();
            }
        });
        ids
    } else {
        with_world(|w| w.dims[dim].fans.get(&pos).map(|f| f.caught.clone()))
            .flatten()
            .unwrap_or_default()
    };
    if items.is_empty() {
        return;
    }
    let flow = if current.pushing {
        facing
    } else {
        facing.opposite()
    };
    let centre = [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + 0.5,
        f64::from(pos[2]) + 0.5,
    ];
    let max = |i: &str| game.items().get(i).map_or(64, |x| x.max_stack).clamp(1, 64) as u8;
    let mut rng =
        processing::Rng::new(game.world.game_time as u64 ^ (pos[0] as u64) << 16 ^ pos[2] as u64);
    // id → (motion, the stack it becomes when processed: None to discard).
    let mut changes: std::collections::HashMap<u32, ([f64; 3], Option<Option<(String, u8)>>)> =
        Default::default();
    let mut spawn = Vec::new();
    for (id, item, count, at, v) in items {
        if !caught.contains(&id) {
            continue;
        }
        let distance = ((at[0] - centre[0]).powi(2)
            + (at[1] - centre[1]).powi(2)
            + (at[2] - centre[2]).powi(2))
        .sqrt();
        let motion = fan::push(flow, speed, current.max, distance, v, false);
        let mut result = None;
        if let Some(kind) = current.type_at(fan::aligned_distance(at, pos, facing) as f32) {
            let mut p = kept
                .remove(&id)
                .or_else(|| old.remove(&id))
                .unwrap_or_default();
            let fire_resistant = game.items().get(&item).is_some_and(|i| i.fire_resistant);
            let count = count.min(255) as u8;
            let done = RECIPES.with_borrow(|r| {
                let r = r.as_ref()?;
                let can = fan::can_process(kind, &item, fire_resistant, r.fan());
                if !fan::entity_can(&p, kind, can)
                    || !fan::advance_entity(&mut p, kind, u32::from(count))
                {
                    return None;
                }
                fan::process(kind, &item, count, r.fan(), max, &mut rng)
            });
            kept.insert(id, p);
            if let Some(mut stacks) = done {
                if stacks.is_empty() {
                    result = Some(None);
                } else {
                    let first = stacks.remove(0);
                    for s in stacks {
                        spawn.push((at, motion, s));
                    }
                    result = Some(Some(first));
                }
            }
        }
        changes.insert(id, (motion, result));
    }
    items_in(game, dimension, lo, hi, |id, stack, _, v| {
        let Some((motion, result)) = changes.get(&id) else {
            return;
        };
        *v = *motion;
        match result {
            Some(Some((item, count))) => {
                stack.item = item.as_str().into();
                stack.count = u32::from(*count);
                stack.damage = 0;
                stack.extra = None;
            }
            Some(None) => stack.count = 0,
            None => {}
        }
    });
    for (at, v, (item, count)) in spawn {
        game.native_run_command(&format!(
            "summon minecraft:item {} {} {} {{Item:{{id:'{item}',count:{count}}},Motion:[{}d,{}d,{}d]}}",
            at[0], at[1], at[2], v[0], v[1], v[2]
        ));
    }
}

/// `AirCurrent.tickAffectedEntities` for every entity but items (mobs, players, minecarts,
/// boats, armor stands): those its search caught (`search`, every 6th tick) and still in its box
/// are pushed along the current as items are (`setDeltaMovement`, a player's client told) and
/// forget their fall (`fallDistance = 0`). A rider moves with its vehicle (`rideTick` zeroes its
/// motion), so it is left alone. A sneaking player is pushed an eighth as hard (4096 in place
/// of 512); a flying one is let go (`isPlayerCreativeFlying`; Horizon's `flying` is a
/// spectator's too). Not here: the processing types' effects on living entities
/// (`affectEntity`: fire, freezing water, haunting).
fn fan_on_entities(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3], search: bool) {
    use create_engine::fan;
    let Some((facing, speed, max, pushing, none)) = with_world(|w| {
        let f = w.dims[dim].fans.get(&pos)?;
        Some((
            f.facing?,
            f.speed,
            f.current.max,
            f.current.pushing,
            f.others.is_empty(),
        ))
    })
    .flatten() else {
        return;
    };
    // Nothing caught and no search this tick: nothing to look at.
    if none && !search {
        return;
    }
    let Some((lo, hi)) = fan::bounds(pos, facing, max) else {
        return;
    };
    let mut inside = game.native_entities_in(dimension, lo, hi);
    inside.retain(|e| e.kind != "minecraft:item");
    let others: Vec<u32> = if search {
        let ids: Vec<u32> = inside.iter().map(|e| e.id).collect();
        with_world(|w| {
            if let Some(f) = w.dims[dim].fans.get_mut(&pos) {
                f.others = ids.clone();
            }
        });
        ids
    } else {
        with_world(|w| w.dims[dim].fans.get(&pos).map(|f| f.others.clone()))
            .flatten()
            .unwrap_or_default()
    };
    let flow = if pushing { facing } else { facing.opposite() };
    let centre = pos.map(|v| f64::from(v) + 0.5);
    for e in inside {
        if !others.contains(&e.id) || e.vehicle.is_some() {
            continue;
        }
        // `isPlayerCreativeFlying`: let go until the next search.
        if e.flying {
            with_world(|w| {
                if let Some(f) = w.dims[dim].fans.get_mut(&pos) {
                    f.others.retain(|id| *id != e.id);
                }
            });
            continue;
        }
        let distance = (0..3)
            .map(|a| (e.pos[a] - centre[a]).powi(2))
            .sum::<f64>()
            .sqrt();
        let motion = fan::push(flow, speed, max, distance, e.velocity, e.sneaking);
        game.native_set_delta_movement(dimension, e.id, motion);
        game.native_reset_fall_distance(dimension, e.id);
    }
}

/// `AirCurrent.rebuild` and `findAffectedHandlers` for the fan at `pos`.
pub(super) fn fan_current(
    game: &Game<'_>,
    dim: usize,
    pos: [i32; 3],
    facing: Dir,
    speed: f32,
) -> (
    create_engine::fan::Current,
    Vec<([i32; 3], Option<create_engine::fan::Kind>)>,
) {
    use create_engine::fan::{self, Current, Way};
    let o = facing.offset();
    let at = |i: i32| [pos[0] + o[0] * i, pos[1] + o[1] * i, pos[2] + o[2] * i];
    let b = game.res.behaviours();
    let transparent = b.tag("create:fan_transparent");
    let way = |i: i32| {
        let p = at(i);
        if !game.world.loaded(p[0], p[2]) {
            return Way::Unloaded;
        }
        let state = game.world.block(p[0], p[1], p[2]);
        if transparent.is_some_and(|t| t.contains(b.block(state))) {
            return Way::Open;
        }
        match b.collision_shape(state) {
            Some(shape) if shape.is_empty() => Way::Open,
            Some(shape) if shape.is_block() => Way::Full,
            Some(shape) => Way::Depth(max_depth(shape, facing)),
            None if b.facts(state).flags
                & horizon_world::level::data::state_flags::COLLISION_EMPTY
                != 0 =>
            {
                Way::Open
            }
            None => Way::Full,
        }
    };
    let max = fan::flow_limit(fan::max_distance(speed), way);
    let current = Current::build(max, fan::pushing(speed, facing), |i| {
        catalyst_at(game, at(i))
    });
    let mut handlers = Vec::new();
    for i in 1..=current.limit() {
        let segment = current.type_at((i - 1) as f32);
        for below in 0..2 {
            let p = at(i);
            let p = [p[0], p[1] - below, p[2]];
            let held = with_world(|w| {
                let d = &w.dims[dim];
                d.depots.contains_key(&p) || d.belt_of.contains_key(&p)
            });
            if held == Some(true) {
                handlers.push((p, catalyst_at(game, p).or(segment)));
            }
            if facing.axis() == Axis::Y {
                break;
            }
        }
    }
    (current, handlers)
}

/// `AirCurrent.findMaxDepth`: how far air gets into `shape` going `facing`, sampled at five
/// points of the face; infinite where a sample finds a hole.
pub(super) fn max_depth(shape: &horizon_core::geometry::VoxelShape, facing: Dir) -> f32 {
    use horizon_core::geometry::Axis as A;
    let axis = match facing.axis() {
        Axis::X => A::X,
        Axis::Y => A::Y,
        Axis::Z => A::Z,
    };
    let positive = matches!(facing, Dir::East | Dir::Up | Dir::South);
    let mut depth = 0.0f64;
    for (a, b) in [
        (0.25, 0.25),
        (0.25, 0.75),
        (0.5, 0.5),
        (0.75, 0.25),
        (0.75, 0.75),
    ] {
        let d = if positive {
            let min = shape.min_at(axis, a, b);
            if min == f64::INFINITY {
                return f32::INFINITY;
            }
            min
        } else {
            let max = shape.max_at(axis, a, b);
            if max == f64::NEG_INFINITY {
                return f32::INFINITY;
            }
            1.0 - max
        };
        depth = depth.max(d);
    }
    depth as f32
}

/// `FanProcessingType.getAt` at a block: its fluid and the catalyst block tags with the state
/// properties they check (a campfire's `lit`, a blaze burner's `blaze` and `flame_type`).
pub(super) fn catalyst_at(game: &Game<'_>, p: [i32; 3]) -> Option<create_engine::fan::Kind> {
    use create_engine::fan::{Catalyst, Heat, kind_at};
    use horizon_world::fluid::FluidKind;
    let b = game.res.behaviours();
    let state = game.world.block(p[0], p[1], p[2]);
    let fluid = b.fluid(state).kind();
    let block = b.block(state);
    let tagged = |t: &str| b.tag(t).is_some_and(|t| t.contains(block));
    let mut c = Catalyst {
        water: matches!(fluid, FluidKind::Water | FluidKind::FlowingWater),
        lava: matches!(fluid, FluidKind::Lava | FluidKind::FlowingLava),
        splashing: tagged("create:fan_processing_catalysts/splashing"),
        haunting: tagged("create:fan_processing_catalysts/haunting"),
        smoking: tagged("create:fan_processing_catalysts/smoking"),
        blasting: tagged("create:fan_processing_catalysts/blasting"),
        ..Default::default()
    };
    if !(c.haunting || c.smoking || c.blasting) {
        return kind_at(&c);
    }
    let Some(s) = game.pack.assets.registry.state(state) else {
        return kind_at(&c);
    };
    let prop = |k: &str| s.properties.get(k).map(String::as_str);
    if tagged("minecraft:campfires") {
        c.campfire_lit = prop("lit").map(|v| v == "true");
    }
    c.soul_flame = prop("flame_type").map(|v| v == "soul");
    c.heat = prop("blaze").and_then(|v| {
        Some(match v {
            "none" => Heat::None,
            "smouldering" => Heat::Smouldering,
            "fading" => Heat::Fading,
            "kindled" => Heat::Kindled,
            "seething" => Heat::Seething,
            _ => return None,
        })
    });
    kind_at(&c)
}

/// `DepotBehaviour.applyToAllItems` with `FanProcessing.applyProcessing`: the held stack's
/// progress, and when done, its results: the first held (centred), the rest in the outputs,
/// what finds no room dropped; none, and the stack is gone.
pub(super) fn fan_on_depot(
    game: &mut Game<'_>,
    dim: usize,
    depot: [i32; 3],
    kind: create_engine::fan::Kind,
) {
    use create_engine::{fan, processing};
    let Some(Some(mut held)) = with_world(|w| w.dims[dim].depots.get(&depot).copied()).flatten()
    else {
        return;
    };
    let Some((_, item, count)) = depot_items(game, depot).into_iter().find(|s| s.0 == 0) else {
        return;
    };
    let fire_resistant = game.items().get(&item).is_some_and(|i| i.fire_resistant);
    let can = RECIPES.with_borrow(|r| {
        r.as_ref()
            .is_some_and(|r| fan::can_process(kind, &item, fire_resistant, r.fan()))
    });
    let done = fan::advance(&mut held.fan, kind, u32::from(count), can);
    if !done {
        with_world(|w| w.dims[dim].depots.insert(depot, Some(held)));
        return;
    }
    let mut rng = processing::Rng::new(
        game.world.game_time as u64
            ^ (depot[0] as u64).wrapping_mul(0x9E37_79B9)
            ^ (depot[2] as u64) << 20,
    );
    let max = |i: &str| game.items().get(i).map_or(64, |x| x.max_stack).clamp(1, 64) as u8;
    let results =
        RECIPES.with_borrow(|r| fan::process(kind, &item, count, r.as_ref()?.fan(), max, &mut rng));
    let Some(results) = results else {
        with_world(|w| w.dims[dim].depots.insert(depot, Some(held)));
        return;
    };
    let limits: Vec<u8> = results.iter().map(|(i, _)| max(i)).collect();
    let stack = |item: &str, count: u8| horizon_client::items::ItemStack {
        item: item.into(),
        count: u32::from(count),
        damage: 0,
        extra: None,
    };
    let dropped = game
        .native_with_mod_block_entity(depot, |be| {
            be.items
                .resize(be.items.len().max(create_engine::depot::SLOTS), None);
            be.items[0] = None;
            let mut outputs: Vec<processing::Slot> = be.items[create_engine::depot::OUTPUTS]
                .iter()
                .map(slot_of)
                .collect();
            let mut dropped = Vec::new();
            for ((i, n), limit) in results.iter().zip(&limits) {
                if be.items[0].is_none() {
                    be.items[0] = Some(stack(i, *n));
                    continue;
                }
                let left = processing::insert_stacked(&mut outputs, i, *n, *limit);
                if left > 0 {
                    dropped.push((i.clone(), left));
                }
            }
            for (slot, s) in be.items[create_engine::depot::OUTPUTS]
                .iter_mut()
                .zip(outputs)
            {
                *slot = s.map(|(i, n)| stack(&i, n));
            }
            dropped
        })
        .unwrap_or_default();
    for (i, n) in dropped {
        game.native_run_command(&format!(
            "summon minecraft:item {}.5 {}.0 {}.5 {{Item:{{id:'{i}',count:{n}}}}}",
            depot[0],
            depot[1] + 1,
            depot[2]
        ));
    }
    // getSimilar keeps the motion and the progress (done: -1, never again by this type).
    let next = (!results.is_empty()).then_some(create_engine::depot::Held {
        position: 0.5,
        ..held
    });
    with_world(|w| {
        w.dims[dim].depots.insert(depot, next);
        w.depot_changed(dim, depot);
    });
}

/// `BeltBlockEntity.applyToAllItems` with `FanProcessing.applyProcessing`: the items over the
/// segment at `at` (within .51 of its middle).
pub(super) fn fan_on_belt(
    game: &mut Game<'_>,
    dim: usize,
    at: [i32; 3],
    kind: create_engine::fan::Kind,
) {
    use create_engine::{belt, fan, processing};
    let Some((c, index)) = with_world(|w| {
        let d = &w.dims[dim];
        let c = *d.belt_of.get(&at)?;
        let index = d.belts.get(&c)?.chain.iter().position(|p| *p == at)? as f32;
        let any = d.belts[&c]
            .items
            .items
            .iter()
            .any(|i| (index + 0.5 - i.position).abs() < 0.51);
        any.then_some((c, index))
    })
    .flatten() else {
        return;
    };
    let Some(mut inv) = with_world(|w| {
        w.dims[dim]
            .belts
            .get_mut(&c)
            .map(|b| std::mem::take(&mut b.items))
    })
    .flatten() else {
        return;
    };
    let mut rng = processing::Rng::new(
        game.world.game_time as u64
            ^ (at[0] as u64).wrapping_mul(0x9E37_79B9)
            ^ (at[2] as u64) << 20,
    );
    let max = |i: &str| game.items().get(i).map_or(64, |x| x.max_stack).clamp(1, 64) as u8;
    let mut made = 0;
    let dirty = RECIPES.with_borrow(|r| {
        let Some(r) = r.as_ref() else {
            return false;
        };
        inv.apply_within(index + 0.5, 0.51, |it| {
            let fire_resistant = game.items().get(&it.item).is_some_and(|i| i.fire_resistant);
            let can = fan::can_process(kind, &it.item, fire_resistant, r.fan());
            if !fan::advance(&mut it.fan, kind, u32::from(it.count), can) {
                return belt::Process::Pass;
            }
            let Some(results) = fan::process(kind, &it.item, it.count, r.fan(), max, &mut rng)
            else {
                return belt::Process::Pass;
            };
            let outputs = results
                .into_iter()
                .map(|(item, count)| {
                    // getSimilar: a new stack's angle (`R.nextInt(360)`), the old one's place
                    // and progress.
                    made += 1;
                    let angle = random_angle(game, [at[0] + made, at[1], at[2]]);
                    let mut out = belt::Item::new(&item, count, angle, upright(game, &item));
                    out.position = it.position;
                    out.inserted_at = it.inserted_at;
                    out.inserted_from = it.inserted_from;
                    out.fan = it.fan;
                    out
                })
                .collect();
            belt::Process::Convert {
                held: None,
                outputs,
            }
        })
    });
    with_world(|w| {
        if let Some(b) = w.dims[dim].belts.get_mut(&c) {
            b.items = inv;
        }
        if dirty {
            w.dims[dim].belt_changes.push(c);
            w.mark_unsaved(dim, c);
        }
    });
}

pub(super) fn send_presses(
    game: &mut Game<'_>,
    players: &[u128],
    dim: usize,
    presses: &[[i32; 3]],
) {
    let all: Vec<([i32; 3], create_engine::press::Press)> = presses
        .iter()
        .map(|&pos| {
            let p = with_world(|w| w.dims[dim].presses.get(&pos).copied())
                .flatten()
                .unwrap_or_default();
            (pos, p)
        })
        .collect();
    for m in create_engine::press::encode(&all) {
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::press::CHANNEL, &m);
        }
    }
}

/// Moves the press head at `pos` to where `p` has it, gliding to where it will be a tick on.
pub(super) fn draw_press_head(
    game: &mut Game<'_>,
    pos: [i32; 3],
    p: &create_engine::press::Press,
    next: f32,
) {
    use create_engine::press::head_offset;
    let Some(model) = CLIENT.with_borrow(|c| {
        let c = c.as_ref()?;
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let (_, info) = c.table.get(state.0)?;
        let f = info.facing?;
        c.models.get(&(state.0, Model::PressHead(f))).copied()
    }) else {
        return;
    };
    let depth = p.mode.head_offset();
    let (now, to) = if p.running {
        (
            head_offset(p.ticks as f32) * depth,
            head_offset(next) * depth,
        )
    } else {
        (0.0, 0.0)
    };
    game.native_part_glide(
        client::key(pos, client::PRESS_HEAD_SLOT),
        model,
        pos,
        1,
        0.0,
        0.0,
        [0.0, -now, 0.0, game.native_parts_seconds()],
        [0.0, -(to - now) * 20.0, 0.0],
    );
}

/// The client's next cycle tick for a press at `speed`: `PressingBehaviour.tick` on the client,
/// which waits at the lowest point for the server.
pub(super) fn client_press_step(p: &create_engine::press::Press, speed: f32, waiting: bool) -> i32 {
    use create_engine::press::CYCLE;
    if waiting && p.ticks >= CYCLE / 2 {
        return CYCLE / 2;
    }
    let next = p.ticks + create_engine::press::tick_speed(speed);
    if p.ticks < CYCLE / 2 && next >= CYCLE / 2 {
        return CYCLE / 2;
    }
    next
}

pub(super) fn presses_from_server(game: &mut Game<'_>, data: &[u8]) {
    use create_engine::press::CYCLE;
    for (pos, p) in create_engine::press::decode(data) {
        // At the lowest point the server has pressed: the head goes on up.
        let waiting = p.running && p.ticks < CYCLE / 2;
        PRESSES.with_borrow_mut(|c| {
            if p.running {
                c.running.insert(pos, (p, waiting));
            } else {
                c.running.remove(&pos);
            }
        });
        let speed = press_speed(pos);
        let next = client_press_step(&p, speed, waiting) as f32;
        draw_press_head(game, pos, &p, next);
    }
}

pub(super) fn press_speed(pos: [i32; 3]) -> f32 {
    CLIENT.with_borrow(|c| {
        c.as_ref()
            .and_then(|c| c.speeds.get(&pos))
            .map_or(0.0, |s| s[0])
    })
}

/// Each client tick: running presses move on a cycle step, drawn gliding to the next.
pub(super) fn tick_client_presses(game: &mut Game<'_>) {
    let running: Vec<([i32; 3], create_engine::press::Press, bool)> =
        PRESSES.with_borrow(|c| c.running.iter().map(|(k, v)| (*k, v.0, v.1)).collect());
    for (pos, mut p, waiting) in running {
        let speed = press_speed(pos);
        p.ticks = client_press_step(&p, speed, waiting);
        if p.ticks > create_engine::press::CYCLE {
            p.running = false;
        }
        let next = client_press_step(&p, speed, waiting) as f32;
        draw_press_head(game, pos, &p, next);
        PRESSES.with_borrow_mut(|c| {
            if p.running {
                c.running.insert(pos, (p, waiting));
            } else {
                c.running.remove(&pos);
            }
        });
    }
}

/// A mechanical crafter's facing and pointing; None for any other block.
pub(super) fn crafter_at(
    game: &Game<'_>,
    p: [i32; 3],
) -> Option<(Dir, create_engine::crafter::Pointing)> {
    let s = game
        .pack
        .assets
        .registry
        .state(game.world.block(p[0], p[1], p[2]))?;
    if s.name != "create:mechanical_crafter" {
        return None;
    }
    let facing = s
        .properties
        .get("facing")
        .and_then(|v| Dir::ALL.into_iter().find(|d| dir_name(*d) == v))?;
    Some((
        facing,
        create_engine::crafter::Pointing::read(s.properties.get("pointing")?)?,
    ))
}

/// `RecipeGridHandler.getTargetingCrafter`: the crafter it points at, facing the same way.
pub(super) fn crafter_target(game: &Game<'_>, p: [i32; 3]) -> Option<[i32; 3]> {
    let (f, pointing) = crafter_at(game, p)?;
    let o = create_engine::crafter::target(f, pointing).offset();
    let t = [p[0] + o[0], p[1] + o[1], p[2] + o[2]];
    (crafter_at(game, t)?.0 == f).then_some(t)
}

/// `getPrecedingCrafters`: crafters beside it in its plane, facing the same way, pointing at it.
pub(super) fn crafters_preceding(game: &Game<'_>, p: [i32; 3]) -> Vec<[i32; 3]> {
    let Some((f, pointing)) = crafter_at(game, p) else {
        return Vec::new();
    };
    let own = create_engine::crafter::target(f, pointing);
    Dir::ALL
        .into_iter()
        .filter(|d| d.axis() != f.axis() && *d != own)
        .filter_map(|d| {
            let o = d.offset();
            let n = [p[0] + o[0], p[1] + o[1], p[2] + o[2]];
            let (nf, np) = crafter_at(game, n)?;
            (nf == f && create_engine::crafter::target(nf, np) == d.opposite()).then_some(n)
        })
        .collect()
}

/// `getAllCraftersOfChainIf`: the chain through `root`, following targets and preceding
/// crafters; None for a loop, or (unless `all`) when one of them fails `test`, or when all do.
pub(super) fn crafter_chain(
    game: &Game<'_>,
    root: [i32; 3],
    test: impl Fn(&Game<'_>, [i32; 3]) -> bool,
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
        if test(game, current) {
            all_empty = false;
        } else {
            empty = true;
        }
        chain.push(current);
        if let Some(t) = crafter_target(game, current)
            && Some(t) != last
        {
            frontier.push((t, Some(current)));
        }
        for p in crafters_preceding(game, current) {
            if Some(p) != last {
                frontier.push((p, Some(current)));
            }
        }
    }
    (!(empty && !all || all_empty)).then_some(chain)
}

/// The item in a crafter's slot (`inventory`), if any.
pub(super) fn crafter_slot(game: &Game<'_>, p: [i32; 3]) -> Option<String> {
    match game.world.block_entities.get(&(p[0], p[1], p[2])) {
        Some(horizon_client::containers::BlockEntity::Mod(m)) => m
            .items
            .first()
            .and_then(|s| s.as_ref())
            .map(|s| s.item.to_string()),
        _ => None,
    }
}

/// Whether a crafter has its slot cover on (`covered`).
pub(super) fn crafter_covered(dim: usize, p: [i32; 3]) -> bool {
    with_world(|w| w.dims[dim].crafter_covers.contains(&p)) == Some(true)
}

/// The crafters whose slots make the inventory of the crafter at `pos`
/// (`ConnectedInput.getInventories`): its group's, in order, or itself alone.
pub(super) fn crafter_group(game: &Game<'_>, dim: usize, pos: [i32; 3]) -> Vec<[i32; 3]> {
    let Some((facing, _)) = crafter_at(game, pos) else {
        return Vec::new();
    };
    let list = with_world(|w| {
        create_engine::crafter_input::inventories(&w.dims[dim].crafter_inputs, pos, facing)
    })
    .unwrap_or_default();
    list.into_iter()
        .filter(|p| crafter_at(game, *p).is_some())
        .collect()
}

/// The crafter's item handler (`CombinedInvWrapper` over its group's `Inventory`s): one item
/// into each crafter of the group in turn that is idle with an empty slot; what is left of
/// `stack`. Nothing changes when `simulate`.
pub(super) fn crafter_handler_insert(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    mut stack: horizon_client::items::ItemStack,
    simulate: bool,
) -> Option<horizon_client::items::ItemStack> {
    // The grid keeps items by id: a stack with components stays out.
    if stack.extra.is_some() || stack.damage != 0 {
        return Some(stack);
    }
    for p in crafter_group(game, dim, pos) {
        if stack.count == 0 {
            break;
        }
        if crafter_covered(dim, p) {
            continue;
        }
        let idle = with_world(|w| w.dims[dim].crafters.get(&p).map(|c| c.phase))
            .flatten()
            .is_none_or(|ph| ph == create_engine::crafter::Phase::Idle);
        if !idle || crafter_slot(game, p).is_some() {
            continue;
        }
        if !simulate {
            let one = horizon_client::items::ItemStack {
                count: 1,
                ..stack.clone()
            };
            game.native_with_mod_block_entity(p, |be| {
                be.items.resize(be.items.len().max(1), None);
                be.items[0] = Some(one);
            });
            with_world(|w| w.dims[dim].crafter_checks.push(p));
        }
        stack.count -= 1;
    }
    (stack.count > 0).then_some(stack)
}

/// `EdgeInteractionHandler` with the wrench on a crafter (not sneaking): a click on the strip
/// along an edge toward a crafter beside it, facing the same way, joins their inputs or parts
/// them (`toggleConnection`); true when it did.
pub(super) fn crafter_edge(game: &mut Game<'_>, dim: usize, pos: [i32; 3], face: Dir) -> bool {
    use create_engine::crafter_input;
    let Some((facing, _)) = crafter_at(game, pos) else {
        return false;
    };
    let Some(hit) = game.pick().map(|h| h.point) else {
        return false;
    };
    let sturdy = |game: &Game<'_>, p: [i32; 3], side: Dir| {
        let state = game.world.block(p[0], p[1], p[2]);
        game.res.facts().get(state).sturdy_full & (1 << side as u8) != 0
    };
    let o = face.offset();
    // `getConnectiveSides`: none when the face is covered.
    if sturdy(
        game,
        [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]],
        face.opposite(),
    ) {
        return false;
    }
    for d in Dir::ALL {
        if d.axis() == face.axis() {
            continue;
        }
        let n = d.offset();
        let neighbour = [pos[0] + n[0], pos[1] + n[1], pos[2] + n[2]];
        let front = [
            neighbour[0] + o[0],
            neighbour[1] + o[1],
            neighbour[2] + o[2],
        ];
        if sturdy(game, front, face.opposite()) {
            continue;
        }
        let other = crafter_at(game, neighbour).map(|c| c.0);
        if !crafter_input::should_connect(facing, face, d, other) {
            continue;
        }
        let (min, max) = crafter_input::edge_box(pos, d);
        if (0..3).all(|k| hit[k] >= min[k] && hit[k] <= max[k]) {
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

/// `MechanicalCrafterBlock.useItemOn` on its front: at work, it throws its grid out; idle, a slot
/// cover goes on an empty uncovered slot, anything else into its inventory (its group's, as much
/// as it takes); an empty hand takes the slot's item back, or the cover off an empty one.
pub(super) fn use_crafter(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    let Some((facing, _)) = crafter_at(game, pos) else {
        return;
    };
    if face != facing.offset() || game.hand != horizon_client::first_person::Hand::Main {
        return;
    }
    let held = game.inventory.get(game.selected).cloned().flatten();
    if held
        .as_ref()
        .is_some_and(|h| &*h.item == "create:mechanical_arm")
    {
        return;
    }
    let wrenched = held.as_ref().is_some_and(|h| &*h.item == "create:wrench");
    let is_hand = held.is_none();
    let idle = with_world(|w| w.dims[dim].crafters.get(&pos).map(|c| c.phase))
        .flatten()
        .is_none_or(|ph| ph == create_engine::crafter::Phase::Idle);
    if !idle && !wrenched {
        *canceled = true;
        let dimension = game.dim.id().to_owned();
        crafters_eject(game, &dimension, dim, pos);
        return;
    }
    let covered = crafter_covered(dim, pos);
    let slot = crafter_slot(game, pos);
    if idle && !is_hand && !wrenched {
        let Some(h) = held else { return };
        if &*h.item == "create:crafter_slot_cover" {
            if covered || slot.is_some() {
                return;
            }
            *canceled = true;
            with_world(|w| {
                w.dims[dim].crafter_covers.insert(pos);
                w.mark_unsaved(dim, pos);
            });
            if !game.creative {
                take_held(game, 1);
            }
            return;
        }
        *canceled = true;
        let count = h.count;
        let left = crafter_handler_insert(game, dim, pos, h, false).map_or(0, |l| l.count);
        take_held(game, count - left);
        return;
    }
    let Some(item) = slot else {
        if covered && !wrenched {
            *canceled = true;
            with_world(|w| {
                w.dims[dim].crafter_covers.remove(&pos);
                w.mark_unsaved(dim, pos);
            });
            if !game.creative {
                game.native_give_items("create:crafter_slot_cover", 1);
            }
        }
        return;
    };
    if !is_hand && held.as_ref().is_none_or(|h| *h.item != *item) {
        return;
    }
    *canceled = true;
    game.native_with_mod_block_entity(pos, |be| {
        if let Some(s) = be.items.first_mut() {
            *s = None;
        }
    });
    game.native_give_items(&item, 1);
}

/// `n` of the selected stack used up.
fn take_held(game: &mut Game<'_>, n: u32) {
    let selected = game.selected;
    if let Some(Some(s)) = game.inventory.get_mut(selected) {
        s.count = s.count.saturating_sub(n);
        if s.count == 0 {
            game.inventory[selected] = None;
        }
    }
}

/// `RecipeGridHandler.tryToApplyRecipe`: a crafting recipe (the grid in a square of its larger
/// side, up to 3), else a `create:mechanical_crafting` one.
pub(super) fn crafter_recipe(
    game: &Game<'_>,
    grid: &[((i32, i32), String)],
) -> Option<(String, u8)> {
    let (w, h, cells) = create_engine::crafter::input(grid);
    let size = w.max(h);
    if (1..=3).contains(&size) {
        let mut square: Vec<Option<horizon_client::items::ItemStack>> = vec![None; size * size];
        for r in 0..h {
            for c in 0..w {
                square[r * size + c] = cells[r * w + c]
                    .as_ref()
                    .map(|i| horizon_client::items::ItemStack::new(i, 1));
            }
        }
        if let Some(out) = game.recipes().craft(&square, size) {
            return Some((out.item.to_string(), out.count.min(64) as u8));
        }
    }
    game.native_recipes_of_type("create:mechanical_crafting")
        .iter()
        .filter_map(|r| {
            let json = &*r.json;
            let pattern: Vec<&str> = json["pattern"]
                .as_array()?
                .iter()
                .filter_map(|l| l.as_str())
                .collect();
            let height = pattern.len();
            let width = pattern.iter().map(|l| l.chars().count()).max()?;
            let mut cells = Vec::with_capacity(width * height);
            for line in &pattern {
                let chars: Vec<char> = line.chars().collect();
                for c in 0..width {
                    let ch = chars.get(c).copied().unwrap_or(' ');
                    cells.push(if ch == ' ' {
                        None
                    } else {
                        let key = &json["key"][ch.to_string()];
                        Some(
                            game.native_recipe_ingredient(key)?
                                .1
                                .iter()
                                .map(|s| s.to_string())
                                .collect(),
                        )
                    });
                }
            }
            let out = game.native_recipe_result(&json["result"])?;
            Some(create_engine::crafter::Shaped {
                width,
                height,
                cells,
                mirrored: json["accept_mirrored"].as_bool().unwrap_or(true),
                result: (out.item.to_string(), out.count.min(64) as u8),
            })
        })
        .find(|s| s.matches(w, h, &cells))
        .map(|s| s.result)
}

/// `ejectWholeGrid`: every crafter of the chain throws out its grid in front of it and goes
/// idle, then looks at its slot again (`lazyTick`). Its slot is left: Create's refuses items
/// while it works, so an item a hopper put in meanwhile is the next round's.
pub(super) fn crafters_eject(game: &mut Game<'_>, dimension: &str, dim: usize, root: [i32; 3]) {
    let chain = crafter_chain(game, root, |_, _| true, true).unwrap_or_else(|| vec![root]);
    for p in chain {
        let grid = with_world(|w| {
            let d = &mut w.dims[dim];
            d.crafting.retain(|q| *q != p);
            d.crafters.get_mut(&p).map(|c| {
                c.phase = create_engine::crafter::Phase::Idle;
                std::mem::take(&mut c.grid)
            })
        })
        .flatten()
        .unwrap_or_default();
        let items: Vec<String> = grid.into_iter().map(|(_, i)| i).collect();
        with_world(|w| w.dims[dim].crafter_checks.push(p));
        let f = crafter_at(game, p).map_or([0, 0, 0], |(f, _)| f.offset());
        for item in items {
            game.native_run_command(&format!(
                "execute in {dimension} run summon minecraft:item {} {} {} {{Item:{{id:'{item}',count:1}}}}",
                p[0] as f64 + 0.5 + f[0] as f64 * 0.75,
                p[1] as f64 + 0.5 + f[1] as f64 * 0.75,
                p[2] as f64 + 0.5 + f[2] as f64 * 0.75
            ));
        }
    }
}

/// `tryInsert`: the result into the container it points at (as a hopper puts it in); without
/// one, everything is thrown out.
pub(super) fn crafter_insert(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    use create_engine::crafter::Phase;
    let Some((facing, pointing)) = crafter_at(game, pos) else {
        return;
    };
    let dir = create_engine::crafter::target(facing, pointing);
    let o = dir.offset();
    let t = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
    // `isTargetingBelt`: a belt or depot there takes the results from the side
    // (`DirectBeltInputBehaviour.handleInsertion`); else `inserting.hasInventory()`.
    let belt_input = with_world(|w| {
        let d = &w.dims[dim];
        d.depots.contains_key(&t) || d.belt_of.contains_key(&t)
    }) == Some(true);
    let face = horizon_core::geometry::Direction::ALL[dir.opposite() as usize];
    if !belt_input && !game.world.block_entities.contains_key(&(t[0], t[1], t[2])) {
        crafters_eject(game, dimension, dim, pos);
        return;
    }
    let grid = with_world(|w| w.dims[dim].crafters.get(&pos).map(|c| c.grid.clone()))
        .flatten()
        .unwrap_or_default();
    // Each stack of the grid in turn, whole (the result is one stack of its count).
    let mut stacks: Vec<(String, Vec<(i32, i32)>)> = Vec::new();
    for (at, item) in grid {
        match stacks.iter_mut().find(|(i, _)| *i == item) {
            Some((_, cells)) => cells.push(at),
            None => stacks.push((item, vec![at])),
        }
    }
    let mut left = Vec::new();
    for (item, cells) in stacks {
        let n = cells.len().min(64);
        let kept = if belt_input {
            let angle = random_angle(game, pos);
            let one = create_engine::belt::Item::new(&item, n as u8, angle, upright(game, &item));
            insert_into_from(game, dim, t, dir, &one, false).map_or(n, usize::from)
        } else {
            let stack = horizon_client::items::ItemStack::new(&item, n as u32);
            insert_item(game, dim, t, face, stack).map_or(0, |l| l.count as usize)
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
        crafters_eject(game, dimension, dim, pos);
    }
}

/// Mechanical crafters: those looked at start their chain when every one holds an item
/// (`checkCompletedRecipe`, `begin`); those at work run `tick`'s phases: the grid assembles,
/// is passed on to the crafter pointed at (`EXPORTING`, 1000), the last one crafts (2000) and
/// puts the result out. Nothing runs without one of them.
pub(super) fn tick_crafters(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::crafter::{self, Phase};
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
        if crafter_at(game, pos).is_none() {
            continue;
        }
        let powered = super::create_redstone::neighbor_signal(game, pos);
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
        game.native_run_command(&format!(
            "execute in {dimension} run summon minecraft:item {}.5 {}.5 {}.5 {{Item:{{id:'{item}',count:1}}}}",
            p[0], p[1], p[2]
        ));
    }
    if checks.is_empty() && working.is_empty() && powered_starts.is_empty() {
        return;
    }
    checks.sort();
    checks.dedup();
    let starts = powered_starts
        .into_iter()
        .map(|p| (p, true))
        .chain(checks.into_iter().map(|p| (p, false)));
    for (pos, powered_start) in starts {
        let idle = with_world(|w| w.dims[dim].crafters.get(&pos).map(|c| c.phase)).flatten()
            == Some(Phase::Idle);
        let rpm = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        // `hasItemOrCover`.
        let filled = |g: &Game<'_>, p: [i32; 3]| {
            crafter_slot(g, p).is_some() || !powered_start && crafter_covered(dim, p)
        };
        if !idle || rpm == 0.0 || !powered_start && !filled(game, pos) {
            continue;
        }
        let Some(chain) = crafter_chain(game, pos, filled, powered_start) else {
            continue;
        };
        for p in chain {
            let item = crafter_slot(game, p);
            game.native_with_mod_block_entity(p, |be| {
                if let Some(x) = be.items.first_mut() {
                    *x = None;
                }
            });
            let first = crafters_preceding(game, p).is_empty();
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
        let rpm = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let speed = crafter::count_down_speed(rpm);
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
                crafter_insert(game, dimension, dim, pos);
                continue;
            }
            _ => continue,
        }
        match c.phase {
            Phase::Assembling => {
                if crafter_target(game, pos).is_some() {
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
                let containers: Vec<String> = c
                    .grid
                    .iter()
                    .filter_map(|(_, i)| game.items().get(i)?.crafting_remainder.clone())
                    .collect();
                match crafter_recipe(game, &c.grid) {
                    Some((item, n)) => with_world(|w| {
                        if let Some(x) = w.dims[dim].crafters.get_mut(&pos) {
                            x.grid = (0..n).map(|i| ((i32::from(i), 0), item.clone())).collect();
                            x.grid.extend(
                                containers
                                    .iter()
                                    .enumerate()
                                    .map(|(i, r)| ((i32::from(n) + i as i32, 0), r.clone())),
                            );
                            x.phase = Phase::Crafting;
                            x.count_down = 2000;
                        }
                    })
                    .unwrap_or(()),
                    None => crafters_eject(game, dimension, dim, pos),
                }
            }
            Phase::Exporting => {
                let Some(t) = crafter_target(game, pos) else {
                    crafters_eject(game, dimension, dim, pos);
                    continue;
                };
                let Some((_, pointing)) = crafter_at(game, pos) else {
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
                let preceding = crafters_preceding(game, t);
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
            Phase::Crafting => crafter_insert(game, dimension, dim, pos),
            _ => {}
        }
    }
}
