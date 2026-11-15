//! The mechanical press (`MechanicalPressBlockEntity`, `PressingBehaviour`) over a depot or
//! items lying on the ground, and
//! what a machine does to a depot's held stack or a belt's (`BeltProcessingBehaviour`): the
//! processing results, a sequenced assembly's steps. The press over a basin is `basin`'s.

use super::{recipes, stacks};
use crate::glue::logistics::{random_angle, upright};
use crate::glue::net;
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::{belt, depot, press, processing, sequenced};
use mod_api::{PlayerRef, ServerHost};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Each idle press's `entityScanCooldown` (`PressingBehaviour.ENTITY_SCAN`).
    static SCAN: RefCell<HashMap<(usize, [i32; 3]), i32>> = RefCell::new(HashMap::new());
}

/// `PressingBehaviour.ENTITY_SCAN`.
const ENTITY_SCAN: i32 = 10;

/// Forgets what was kept for the world.
pub fn clear() {
    SCAN.with_borrow_mut(HashMap::clear);
}

/// Whether a press over `at` (two below it) stands over a belt, depot or ejector
/// (`TransportedItemStackHandlerBehaviour`) or a basin, which it works on instead of the ground.
fn over_handler(lv: &ServerLevel<'_>, at: [i32; 3]) -> bool {
    use create_engine::Block;
    matches!(
        lv.info([at[0], at[1] - 2, at[2]]).map(|(b, _)| b),
        Some(Block::Belt | Block::Depot | Block::WeightedEjector | Block::Basin)
    )
}

/// The item entities on the ground in the block under the press at `at`
/// (`new AABB(below)`, `isAlive() && onGround()`), and whether each has a pressing recipe.
fn grounded_below(lv: &ServerLevel<'_>, at: [i32; 3]) -> Vec<(super::stacks::Dropped, bool)> {
    let below = [at[0], at[1] - 1, at[2]].map(f64::from);
    let hi = below.map(|v| v + 1.0);
    lv.items_in(below, hi)
        .into_iter()
        .filter(|d| d.entity.on_ground(&*lv.host) == Some(true))
        .map(|d| {
            let item = stacks::id(&*lv.host, &d.stack);
            let fits = recipes::with(|r| r.pressing.find_single(&item).is_some()) == Some(true);
            (d, fits)
        })
        .collect()
}

/// `PressingBehaviour.tick` of an idle turning press over neither a belt, depot nor basin:
/// every `ENTITY_SCAN` ticks it looks for an item on the ground under it that a pressing recipe
/// takes, and starts in the world (`start(Mode.WORLD)`).
fn scan_world(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let idle: Vec<[i32; 3]> = with_world(|w| {
        w.dims[dim]
            .presses
            .iter()
            .filter(|(_, p)| !p.running)
            .map(|(k, _)| *k)
            .collect()
    })
    .unwrap_or_default();
    SCAN.with_borrow_mut(|s| s.retain(|(d, p), _| *d != dim || idle.contains(p)));
    for at in idle {
        if lv.speed(at) == 0.0 {
            continue;
        }
        let due = SCAN.with_borrow_mut(|s| {
            let c = s.entry((dim, at)).or_insert(0);
            if *c > 0 {
                *c -= 1;
            }
            if *c > 0 {
                return false;
            }
            *c = ENTITY_SCAN;
            true
        });
        if !due || over_handler(lv, at) {
            continue;
        }
        if grounded_below(lv, at).iter().any(|(_, fits)| *fits) {
            with_world(|w| {
                if let Some(p) = w.dims[dim].presses.get_mut(&at) {
                    p.start(press::Mode::World);
                    w.dims[dim].press_changes.push(at);
                }
            });
        }
    }
}

/// `PressingBehaviour.applyInWorld` with `MechanicalPressBlockEntity.tryProcessInWorld`, the
/// head at its lowest: the first item on the ground under the press (not `bulkPressing`, off by
/// default) is pressed; a single item becomes the recipe's results where it lies
/// (`RecipeApplier.applyRecipeOn(entity)`), a stack of more gives one item's results as new
/// items, moving a little, and shrinks by one.
fn press_in_world(lv: &mut ServerLevel<'_>, at: [i32; 3]) {
    let Some((d, fits)) = grounded_below(lv, at).into_iter().next() else {
        return;
    };
    SCAN.with_borrow_mut(|s| s.insert((lv.dim, at), 0));
    if !fits {
        return;
    }
    let item = stacks::id(&*lv.host, &d.stack);
    let seed = lv.seed_at(at) ^ u64::from(d.id());
    let mut rng = processing::Rng::new(seed);
    let Some(results) = recipes::with(|r| {
        let id = r.pressing.find_single(&item)?;
        Some(processing::roll(r.pressing.get(id), &mut rng))
    })
    .flatten() else {
        return;
    };
    crate::glue::advancements::pressed(lv, at, results.first().map(|(i, n)| (&**i, *n)));
    if d.stack.count() == 1 {
        let mut out = results.into_iter();
        let first = out
            .next()
            .and_then(|(i, n)| stacks::stack(&*lv.host, &i, u32::from(n)));
        for (i, n) in out {
            lv.spawn_moving(d.at, &i, u32::from(n), d.velocity);
        }
        lv.set_dropped(&d, first, d.velocity);
    } else {
        for (i, n) in results {
            let m = [0, 1, 2].map(|_| (f64::from(rng.next_float()) - 0.5) * 0.05 * 2.0);
            lv.spawn_moving(d.at, &i, u32::from(n), m);
        }
        let rest = d.stack.clone().shrink(1);
        lv.set_dropped(&d, rest, d.velocity);
    }
}

/// Depots that ask their press, then running presses: `DepotBehaviour.tick` with
/// `BeltPressingCallbacks`, then `PressingBehaviour.tick`. Nothing runs without either.
pub fn tick_presses(lv: &mut ServerLevel<'_>) {
    use press::{Step, Tick};
    let dim = lv.dim;
    if recipes::ready() {
        scan_world(lv);
    }
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
    if !recipes::ready() {
        return;
    }
    let mut keep = Vec::new();
    for depot in work {
        let above = [depot[0], depot[1] + 2, depot[2]];
        if super::deployer::deployer_on_depot(lv, depot) {
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
        let Some(stack) = lv.slot(depot, 0) else {
            continue;
        };
        let item = stacks::id(&*lv.host, &stack);
        let count = stack.count().min(255) as u8;
        let speed = lv.speed(above);
        let in_way = lv.blocked([depot[0], depot[1] + 1, depot[2]]);
        // A sequenced assembly's pressing step first (`SequencedAssemblyRecipe.getRecipe`).
        let sequenced = sequenced_on_depot(lv, depot, "create:pressing", None);
        let fits = sequenced.is_some()
            || recipes::with(|r| r.pressing.find_single(&item).is_some()) == Some(true);
        match press::depot_step(&mut held, &p, speed, in_way, fits) {
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
                        sequenced_step_on_depot(lv, depot, count, held, a, progress.as_ref())
                    }
                    None => press_on_depot(lv, depot, &item, count, held),
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
        let speed = lv.speed(at);
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
            Some((p.mode, tick))
        })
        .flatten();
        // On the ground: pressed at the bottom (`applyInWorld`).
        if basin_mode == Some((press::Mode::World, Tick::Down)) {
            press_in_world(lv, at);
        }
        let basin_mode = basin_mode
            .filter(|(m, _)| *m == press::Mode::Basin)
            .map(|(_, t)| t);
        // Over a basin: compact at the bottom (`tryProcessInBasin`), look again when done
        // (`onPressingCompleted`).
        match basin_mode {
            Some(Tick::Down) => super::basin::compact_in_basin(lv, at),
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
        let players = lv.players();
        send_presses(lv.host, &players, dim, &changed);
    }
}

/// The presses of `dim` at `presses`, to `players`: their cycles, which their clients run on.
pub fn send_presses(
    host: &mut dyn ServerHost,
    players: &[PlayerRef],
    dim: usize,
    presses: &[[i32; 3]],
) {
    let all: Vec<([i32; 3], press::Press)> = presses
        .iter()
        .map(|&pos| {
            let p = with_world(|w| w.dims[dim].presses.get(&pos).copied())
                .flatten()
                .unwrap_or_default();
            (pos, p)
        })
        .collect();
    for m in press::encode(&all) {
        net::send_to_players(host, players, press::CHANNEL, &m);
    }
}

/// A player joined: the running presses of their level.
pub fn player_joined(host: &mut dyn ServerHost, player: PlayerRef) {
    let Some(dim) = player
        .level(&*host)
        .and_then(|l| l.dimension().id(&*host))
        .and_then(|d| crate::glue::world::level_index(&d))
    else {
        return;
    };
    let presses: Vec<[i32; 3]> = with_world(|w| {
        w.dims[dim]
            .presses
            .iter()
            .filter(|(_, p)| p.running)
            .map(|(k, _)| *k)
            .collect()
    })
    .unwrap_or_default();
    if !presses.is_empty() {
        send_presses(host, &[player], dim, &presses);
    }
}

/// `BeltPressingCallbacks.whenItemHeld` at the head's lowest point: one of the stack (all of
/// it when it is one) becomes the recipe's rolled results (`results_on_depot`). Returns the
/// held stack's motion after.
pub fn press_on_depot(
    lv: &mut ServerLevel<'_>,
    depot: [i32; 3],
    item: &str,
    count: u8,
    held: depot::Held,
) -> depot::Held {
    let seed = lv.seed_at(depot);
    let results = recipes::with(|r| {
        let id = r.pressing.find_single(item)?;
        Some(processing::roll(
            r.pressing.get(id),
            &mut processing::Rng::new(seed),
        ))
    })
    .flatten();
    let Some(results) = results else {
        return held;
    };
    results_on_depot(lv, depot, count, held, results)
}

/// `BeltPressingCallbacks` for a stack on the belt segment at `at` under the press two above
/// (`BeltProcessingBehaviour` of `PressingBehaviour`): None without a press there. An item
/// arriving (`onItemReceived`) passes a blocked press (`isBlocked`) and one standing still, is
/// held while the press runs, and starts it when a recipe takes it (a sequenced assembly's
/// pressing step first); held (`whenItemHeld`), at the head's lowest point one of it becomes the
/// recipe's rolled results, at rest where they lie (locked, a random angle).
pub fn press_on_belt(
    lv: &mut ServerLevel<'_>,
    at: [i32; 3],
    item: &mut belt::Item,
    locked: bool,
) -> Option<belt::Process> {
    use belt::Process;
    use press::{CYCLE, Mode};
    let dim = lv.dim;
    let above = [at[0], at[1] + 2, at[2]];
    let mut p = with_world(|w| w.dims[dim].presses.get(&above).copied()).flatten()?;
    if !locked && lv.blocked([at[0], at[1] + 1, at[2]]) {
        return None;
    }
    if lv.speed(above) == 0.0 {
        return Some(Process::Pass);
    }
    let sequenced = sequenced_for(
        create_engine::arm::item_id(&item.item),
        item.progress.as_ref(),
        "create:pressing",
        None,
    );
    if !locked {
        if p.running {
            return Some(Process::Hold);
        }
        let fits = sequenced.is_some()
            || recipes::with(|r| {
                r.pressing
                    .find_single(create_engine::arm::item_id(&item.item))
                    .is_some()
            }) == Some(true);
        if !fits {
            return Some(Process::Pass);
        }
        p.start(Mode::Belt);
        with_world(|w| {
            w.dims[dim].presses.insert(above, p);
            w.dims[dim].press_changes.push(above);
        });
        return Some(Process::Hold);
    }
    if !p.running {
        return Some(Process::Pass);
    }
    if p.ticks != CYCLE / 2 {
        return Some(Process::Hold);
    }
    let mut outputs = match &sequenced {
        Some((a, _)) => sequenced_on_belt(lv, at, item, a),
        None => {
            let seed = lv.game_time() as u64 ^ (at[0] as u64).wrapping_mul(0x9E37_79B9);
            let results = recipes::with(|r| {
                let id = r
                    .pressing
                    .find_single(create_engine::arm::item_id(&item.item))?;
                Some(processing::roll(
                    r.pressing.get(id),
                    &mut processing::Rng::new(seed),
                ))
            })
            .flatten()?;
            results
                .into_iter()
                .map(|(name, n)| {
                    let mut out = item.clone();
                    out.item = name;
                    out.count = n;
                    out.progress = None;
                    out
                })
                .collect()
        }
    };
    for o in &mut outputs {
        o.locked = true;
        o.angle = random_angle(lv, at);
    }
    crate::glue::advancements::pressed(lv, above, outputs.first().map(|o| (&*o.item, o.count)));
    Some(Process::Convert {
        held: belt_left(item),
        outputs,
    })
}

/// The transitional item of the assembly an item's progress names, with the progress and the
/// fraction of the steps done (`SequencedAssembly.progress`).
pub fn transitional_of(p: &sequenced::Progress) -> Option<(String, &sequenced::Progress, f32)> {
    recipes::with(|r| {
        let a = r.sequenced.iter().find(|a| a.id == p.id)?;
        let total = (a.steps.len().max(1) as u32 * a.loops.max(1)) as f32;
        Some((a.transitional.clone(), p.step as f32 / total))
    })
    .flatten()
    .map(|(t, f)| (t, p, f))
}

/// `SequencedAssemblyRecipe.getRecipe` for the stack held on `depot` and a machine of recipe
/// type `kind` (a deployer holding `tool`): the assembly, the item's progress and the step,
/// when the item is at a step of that type (and the deployer holds what it takes).
pub fn sequenced_on_depot(
    lv: &ServerLevel<'_>,
    depot: [i32; 3],
    kind: &str,
    tool: Option<&str>,
) -> Option<(
    sequenced::Assembly,
    Option<sequenced::Progress>,
    sequenced::Step,
)> {
    let stack = lv.slot(depot, 0)?;
    let progress = stacks::sequenced_progress(&stack);
    let item = stacks::id(&*lv.host, &stack);
    let (a, s) = sequenced_for(&item, progress.as_ref(), kind, tool)?;
    Some((a, progress, s))
}

/// `SequencedAssemblyRecipe.getRecipe` for `item` at `progress` and a machine of recipe type
/// `kind` (a deployer holding `tool`): the assembly and the step, when the item is at a step of
/// that type (and the deployer holds what it takes).
pub fn sequenced_for(
    item: &str,
    progress: Option<&sequenced::Progress>,
    kind: &str,
    tool: Option<&str>,
) -> Option<(sequenced::Assembly, sequenced::Step)> {
    recipes::with(|r| {
        let (a, s) = sequenced::step_for(&r.sequenced, item, progress, kind)?;
        if let Some(with) = &s.with
            && !tool.is_some_and(|t| with.iter().any(|w| w == t))
        {
            return None;
        }
        Some((a.clone(), s.clone()))
    })
    .flatten()
}

/// One of a belt stack through a sequenced step (`advance`): the transitional item a step on,
/// its progress with it, or a result of the pool.
pub fn sequenced_on_belt(
    lv: &ServerLevel<'_>,
    at: [i32; 3],
    item: &belt::Item,
    a: &sequenced::Assembly,
) -> Vec<belt::Item> {
    use sequenced::Advanced;
    let seed = lv.game_time() as u64 ^ (at[0] as u64).wrapping_mul(0x9E37_79B9);
    let roll = processing::Rng::new(seed).next_float();
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
pub fn belt_left(item: &belt::Item) -> Option<belt::Item> {
    (item.count > 1).then(|| {
        let mut left = item.clone();
        left.count -= 1;
        left
    })
}

/// A sequenced step done on the depot's held stack (one of it, `advance`): the transitional item
/// a step on (its progress kept with it), or a result rolled from the pool.
pub fn sequenced_step_on_depot(
    lv: &mut ServerLevel<'_>,
    depot: [i32; 3],
    count: u8,
    held: depot::Held,
    assembly: &sequenced::Assembly,
    progress: Option<&sequenced::Progress>,
) -> depot::Held {
    use sequenced::Advanced;
    let dim = lv.dim;
    let seed = lv.game_time() as u64 ^ (depot[0] as u64).wrapping_mul(0x9E37_79B9);
    let roll = processing::Rng::new(seed).next_float();
    match assembly.advance(progress, roll) {
        Advanced::Done { item, count: n } => {
            results_on_depot(lv, depot, count, held, vec![(item, n)])
        }
        Advanced::Nothing => results_on_depot(lv, depot, count, held, Vec::new()),
        Advanced::Next {
            item,
            progress,
            fraction,
        } => {
            crate::glue::advancements::pressed_on_depot(lv, depot, Some((&item, 1)));
            let Some(mut stack) = stacks::stack(&*lv.host, &item, 1) else {
                return held;
            };
            stacks::set_sequenced_progress(&mut stack, &progress.id, progress.step, fraction);
            // One held: it becomes the transitional item, centred; more: one goes to the first
            // free output, or drops above the depot.
            let angle = random_angle(lv, depot);
            let mut next = held;
            let placed = if count == 1 {
                next = depot::Held {
                    angle,
                    position: 0.5,
                    locked: true,
                    ..held
                };
                lv.set_slots(depot, &[(0, Some(stack.clone()))])
            } else {
                let slots = lv.slots(depot, depot::SLOTS).unwrap_or_default();
                let rest = slots.first().cloned().flatten().and_then(|s| s.shrink(1));
                let free = depot::OUTPUTS
                    .into_iter()
                    .find(|&i| slots.get(i).is_some_and(Option::is_none));
                match free {
                    Some(i) => lv.set_slots(depot, &[(0, rest), (i, Some(stack.clone()))]),
                    None => {
                        lv.set_slots(depot, &[(0, rest)]);
                        false
                    }
                }
            };
            if !placed {
                let at = [
                    f64::from(depot[0]) + 0.5,
                    f64::from(depot[1] + 1),
                    f64::from(depot[2]) + 0.5,
                ];
                lv.spawn_stack(at, stack, [0.0; 3]);
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
pub fn results_on_depot(
    lv: &mut ServerLevel<'_>,
    depot: [i32; 3],
    count: u8,
    held: depot::Held,
    results: Vec<(String, u8)>,
) -> depot::Held {
    crate::glue::advancements::pressed_on_depot(
        lv,
        depot,
        results.first().map(|(i, n)| (&**i, *n)),
    );
    let dim = lv.dim;
    let single = count == 1;
    let angle = random_angle(lv, depot);
    let mut next = held;
    let Some(slots) = lv.slots(depot, depot::SLOTS) else {
        return held;
    };
    let host = &*lv.host;
    let mut rest = results.iter();
    let first = if single {
        match rest.next() {
            Some((first, n)) => {
                next = depot::Held {
                    angle,
                    position: 0.5,
                    locked: true,
                    ..held
                };
                stacks::stack(host, first, u32::from(*n))
            }
            None => None,
        }
    } else {
        slots[0].clone().and_then(|s| s.shrink(1))
    };
    let mut outputs: Vec<processing::Slot> = slots[depot::OUTPUTS]
        .iter()
        .map(|s| stacks::slot_of(host, s))
        .collect();
    let mut dropped = Vec::new();
    for (i, n) in rest {
        let limit = stacks::limit(host, i);
        let left = processing::insert_stacked(&mut outputs, i, *n, limit);
        if left > 0 {
            dropped.push((i.clone(), left));
        }
    }
    let mut changes = vec![(0, first)];
    for (k, s) in outputs.iter().enumerate() {
        let i = depot::OUTPUTS.start + k;
        if stacks::slot_of(host, &slots[i]) != *s {
            changes.push((i, stacks::from_slot(host, s)));
        }
    }
    lv.set_slots(depot, &changes);
    for (i, n) in dropped {
        // Containers.dropItemStack at the depot's centre, half a block up.
        let at = [
            f64::from(depot[0]) + 0.5,
            f64::from(depot[1] + 1),
            f64::from(depot[2]) + 0.5,
        ];
        lv.spawn_items(at, &i, u32::from(n));
    }
    if results.is_empty() && single {
        with_world(|w| w.depot_put(dim, depot, None));
    }
    with_world(|w| w.depot_changed(dim, depot));
    next
}

/// A belt's results as items on it: the angle each comes to rest at (upright items face the
/// belt's way), unlocked (`BeltDeployerCallbacks`).
pub fn on_belt(lv: &ServerLevel<'_>, at: [i32; 3], outputs: Vec<belt::Item>) -> Vec<belt::Item> {
    outputs
        .into_iter()
        .map(|mut o| {
            o.locked = false;
            o.angle = if upright(&*lv.host, &o.item) {
                180
            } else {
                random_angle(lv, at)
            };
            o
        })
        .collect()
}
