//! The encased fan (`EncasedFanBlockEntity`, `AirCurrent`): its current pushes entities and
//! processes items in it (`FanProcessing`: washing, haunting, smoking, blasting), on the
//! ground and on depots and belts in it; and the nozzle on its front (`NozzleBlockEntity`).
//! Not here (nor in the legacy glue): the processing types' effects on living entities
//! (`affectEntity`: fire, freezing water, haunting).

use super::{recipes, stacks};
use crate::glue::logistics::{random_angle, upright};
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::fan::{self, Catalyst, Current, Heat, Kind, Way};
use create_engine::kinetics::{Axis, Dir};
use create_engine::{Block, belt, depot, nozzle, processing};
use mod_api::{BlockShapeKind, EntityRef, ExplosionInteraction, ExplosionSettings, ItemStack};
use std::collections::HashMap;

/// Encased fans that turn (`EncasedFanBlockEntity.tick`): the current looked at again when due
/// (`AirCurrent.rebuild`), then the depots and belts in it processed (`tickAffectedHandlers`).
/// Stopped fans cost nothing.
pub fn tick_fans(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    check_nozzles(lv);
    let fans = with_world(|w| w.fans_to_tick(dim)).unwrap_or_default();
    if fans.is_empty() {
        return;
    }
    let recipes = recipes::ready();
    // Item entities' progress, kept for those still in a current this tick.
    let mut kept = HashMap::new();
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
                Some(facing) if speed != 0.0 => fan_current(lv, pos, facing, speed),
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
        // The nozzle on its front (`NozzleBlockEntity.tick`): it does something only while its
        // fan turns, so it ticks with it (and once more when it stops, to let go).
        if let Some(f) = facing {
            tick_nozzle(lv, super::step(pos, f, 1), (speed, f));
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
        fan_on_entities(lv, pos, search);
        if !recipes {
            continue;
        }
        fan_on_items(lv, pos, search, &mut old, &mut kept);
        let handlers = with_world(|w| w.dims[dim].fans.get(&pos).map(|f| f.handlers.clone()))
            .flatten()
            .unwrap_or_default();
        for (at, kind) in handlers {
            let Some(kind) = kind else {
                continue;
            };
            fan_on_depot(lv, at, kind);
            fan_on_belt(lv, at, kind);
        }
    }
    with_world(|w| w.dims[dim].item_fan = kept);
}

/// Nozzles whose neighbour changed break, with their drops, when the block behind them is no
/// longer an encased fan facing their way (`NozzleBlock.neighborChanged`, `canSurvive`).
fn check_nozzles(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
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
        let Some(facing) = lv.facing_of(pos, Block::Nozzle) else {
            continue;
        };
        let fan = lv.facing_of(super::step(pos, facing, -1), Block::EncasedFan);
        if !nozzle::survives(facing, fan) {
            lv.destroy_block(pos, true);
        }
    }
}

/// An entity that is not an item, as the fan and the nozzle push it.
struct Pushed {
    entity: EntityRef,
    at: [f64; 3],
    velocity: [f64; 3],
    min: [f64; 3],
    max: [f64; 3],
    sneaking: bool,
    /// `isPlayerCreativeFlying` (and a spectator's flight).
    flying: bool,
    riding: bool,
}

/// The entities but items in `lo`..`hi`.
fn others_in(lv: &mut ServerLevel<'_>, lo: [f64; 3], hi: [f64; 3]) -> Vec<Pushed> {
    let found = lv.level.entities_in(&*lv.host, lo, hi).unwrap_or_default();
    let mut out = Vec::new();
    for entity in found {
        if lv.host.item_entity(entity).is_some() {
            continue;
        }
        let flying = if entity.kind(&*lv.host).as_deref() == Some("minecraft:player") {
            entity
                .uuid(&mut *lv.host)
                .and_then(|u| lv.host.player(u))
                .and_then(|p| p.abilities(&*lv.host))
                .is_some_and(|a| a.flying)
        } else {
            false
        };
        let host = &*lv.host;
        let (Some([min, max]), Some(at)) = (entity.bounding_box(host), entity.position(host))
        else {
            continue;
        };
        out.push(Pushed {
            entity,
            at,
            velocity: entity.velocity(host).unwrap_or_default(),
            min,
            max,
            sneaking: entity.is_shift_key_down(host).unwrap_or(false),
            flying,
            riding: entity.vehicle(host).is_some(),
        });
    }
    out
}

/// `NozzleBlockEntity.tick` for the nozzle at `pos` on the front of a ticking fan (`fan`: its
/// speed and facing): every sixth tick it looks for the entities within half its range that it
/// sees (`lazyTick`, `canSee` through whole blocks); then it takes its range from the fan and
/// pushes (or pulls) each caught one by `(range - distance)` along the line from its centre,
/// 1/128 for items and 1/32 for anything else a tick, forgetting its fall (`fallDistance = 0`).
/// Sneaking players and flyers are let go. Pulling more than 256 discards the items and mobs it
/// holds, after a power 2 explosion that breaks no block (`Level.explode`).
fn tick_nozzle(lv: &mut ServerLevel<'_>, pos: [i32; 3], fan: (f32, Dir)) {
    use nozzle::{Push, Target};
    let dim = lv.dim;
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
    if n.lazy_due() && n.range != 0.0 {
        let (lo, hi) = nozzle::search_box(pos, n.range);
        let mut seen: Vec<(Target, [f64; 3])> = Vec::new();
        let (rlo, rhi) = reach(n.range);
        let mut alive: Vec<Target> = Vec::new();
        let items = lv.items_in(rlo, rhi);
        for d in &items {
            alive.push(Target::Item(d.id()));
            if belt::item_touches(d.at, lo, hi) {
                seen.push((Target::Item(d.id()), d.at));
            }
        }
        let others = others_in(lv, rlo, rhi);
        for e in &others {
            let t = Target::Entity(e.entity.id());
            alive.push(t);
            // Sneaking players and flyers are skipped.
            if e.sneaking || e.flying {
                continue;
            }
            if (0..3).all(|a| e.min[a] < hi[a] && e.max[a] > lo[a]) {
                seen.push((t, e.at));
            }
        }
        for (t, at) in seen {
            n.look_at(pos, t, at, || {
                nozzle::sees(pos, at, |p| p != pos && lv.blocked(p))
            });
        }
        n.entities.retain(|t| alive.contains(t));
        if nozzle::overloaded(n.pushing, n.entities.len()) {
            // `level.explode(.., 2, ExplosionInteraction.NONE)`, then what it held vanishes.
            let _ = lv.level.explode(
                &mut *lv.host,
                None,
                nozzle::center(pos),
                2.0,
                false,
                ExplosionInteraction::None,
                ExplosionSettings::default(),
            );
            let gone = std::mem::take(&mut n.entities);
            for d in &items {
                if gone.contains(&Target::Item(d.id())) {
                    lv.set_dropped(d, None, d.velocity);
                }
            }
            for e in &others {
                if gone.contains(&Target::Entity(e.entity.id())) {
                    e.entity.discard(&mut *lv.host);
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
        for d in lv.items_in(lo, hi) {
            let t = Target::Item(d.id());
            if !n.entities.contains(&t) {
                continue;
            }
            found.push(t);
            match nozzle::push(pos, range, pushing, d.at, true, false) {
                Push::Release => released.push(t),
                Push::Hold => {}
                Push::Add(add) => {
                    let v = d.velocity;
                    let motion = [v[0] + add[0], v[1] + add[1], v[2] + add[2]];
                    lv.set_dropped(&d, Some(d.stack.clone()), motion);
                }
            }
        }
        if n.entities.iter().any(|t| !t.is_item()) {
            for e in others_in(lv, lo, hi) {
                let t = Target::Entity(e.entity.id());
                if !n.entities.contains(&t) {
                    continue;
                }
                found.push(t);
                match nozzle::push(pos, range, pushing, e.at, false, e.sneaking || e.flying) {
                    Push::Release => released.push(t),
                    Push::Hold => {}
                    Push::Add(add) => {
                        let v = e.velocity;
                        e.entity.set_velocity(
                            &mut *lv.host,
                            [v[0] + add[0], v[1] + add[1], v[2] + add[2]],
                        );
                        e.entity.reset_fall_distance(&mut *lv.host);
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

/// `AirCurrent.tickAffectedEntities` for items: those its search caught (every 6th tick) and
/// still in its box are pushed along the current, and processed by the type at their distance
/// (`FanProcessing.applyProcessing`): the first result replaces the stack, the others are
/// dropped beside it with its motion, none discards it.
fn fan_on_items(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    search: bool,
    old: &mut HashMap<u32, fan::Progress>,
    kept: &mut HashMap<u32, fan::Progress>,
) {
    let dim = lv.dim;
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
    let items = lv.items_in(lo, hi);
    let caught: Vec<u32> = if search {
        let ids: Vec<u32> = items.iter().map(|i| i.id()).collect();
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
    let centre = pos.map(|v| f64::from(v) + 0.5);
    let mut rng =
        processing::Rng::new(lv.game_time() as u64 ^ (pos[0] as u64) << 16 ^ pos[2] as u64);
    for d in items {
        if !caught.contains(&d.id()) {
            continue;
        }
        let distance = (0..3)
            .map(|a| (d.at[a] - centre[a]).powi(2))
            .sum::<f64>()
            .sqrt();
        let motion = fan::push(flow, speed, current.max, distance, d.velocity, false);
        let mut next = Some(d.stack.clone());
        if let Some(kind) = current.type_at(fan::aligned_distance(d.at, pos, facing) as f32) {
            let id = d.id();
            let mut p = kept
                .remove(&id)
                .or_else(|| old.remove(&id))
                .unwrap_or_default();
            let host = &*lv.host;
            let item = stacks::id(host, &d.stack);
            let fire_resistant = fire_resistant(lv, &d.stack);
            let count = d.stack.count().min(255) as u8;
            let max = |i: &str| stacks::limit(host, i);
            let done = recipes::with(|r| {
                let can = fan::can_process(kind, &item, fire_resistant, r.fan());
                if !fan::entity_can(&p, kind, can)
                    || !fan::advance_entity(&mut p, kind, u32::from(count))
                {
                    return None;
                }
                fan::process(kind, &item, count, r.fan(), max, &mut rng)
            })
            .flatten();
            kept.insert(id, p);
            if let Some(mut out) = done {
                if out.is_empty() {
                    next = None;
                } else {
                    let (first, n) = out.remove(0);
                    for (i, c) in out {
                        lv.spawn_moving(d.at, &i, u32::from(c), motion);
                    }
                    next = stacks::stack(&*lv.host, &first, u32::from(n));
                }
            }
        }
        lv.set_dropped(&d, next, motion);
    }
}

/// Whether a stack's item resists fire (`DataComponents.DAMAGE_RESISTANT` with `is_fire`).
fn fire_resistant(lv: &ServerLevel<'_>, stack: &ItemStack) -> bool {
    lv.host
        .item_default_component(stack.item(), "minecraft:damage_resistant")
        .is_some()
}

/// `AirCurrent.tickAffectedEntities` for every entity but items (mobs, players, minecarts,
/// boats, armor stands): those its search caught (`search`, every 6th tick) and still in its box
/// are pushed along the current as items are (`setDeltaMovement`) and forget their fall
/// (`fallDistance = 0`). A rider moves with its vehicle (`rideTick` zeroes its motion), so it is
/// left alone. A sneaking player is pushed an eighth as hard (4096 in place of 512); a flying
/// one is let go (`isPlayerCreativeFlying`).
fn fan_on_entities(lv: &mut ServerLevel<'_>, pos: [i32; 3], search: bool) {
    let dim = lv.dim;
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
    let inside = others_in(lv, lo, hi);
    let others: Vec<u32> = if search {
        let ids: Vec<u32> = inside.iter().map(|e| e.entity.id()).collect();
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
        let id = e.entity.id();
        if !others.contains(&id) || e.riding {
            continue;
        }
        // `isPlayerCreativeFlying`: let go until the next search.
        if e.flying {
            with_world(|w| {
                if let Some(f) = w.dims[dim].fans.get_mut(&pos) {
                    f.others.retain(|o| *o != id);
                }
            });
            continue;
        }
        let distance = (0..3)
            .map(|a| (e.at[a] - centre[a]).powi(2))
            .sum::<f64>()
            .sqrt();
        let motion = fan::push(flow, speed, max, distance, e.velocity, e.sneaking);
        e.entity.set_velocity(&mut *lv.host, motion);
        e.entity.reset_fall_distance(&mut *lv.host);
    }
}

/// The depots and belts in a current, with the processing type each is in.
type Handlers = Vec<([i32; 3], Option<Kind>)>;

/// `AirCurrent.rebuild` and `findAffectedHandlers` for the fan at `pos`.
fn fan_current(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    facing: Dir,
    speed: f32,
) -> (Current, Handlers) {
    let dim = lv.dim;
    let at = |i: i32| super::step(pos, facing, i);
    let way = |i: i32| {
        let p = at(i);
        if !lv.is_loaded(p) {
            return Way::Unloaded;
        }
        if lv.block_tagged(p, "create:fan_transparent") {
            return Way::Open;
        }
        let shape = lv.shape(p, BlockShapeKind::Collision);
        if shape.is_empty() {
            Way::Open
        } else if shape.len() == 1 && shape[0] == [0.0, 0.0, 0.0, 1.0, 1.0, 1.0] {
            Way::Full
        } else {
            Way::Depth(max_depth(&shape, facing))
        }
    };
    let max = fan::flow_limit(fan::max_distance(speed), way);
    let current = Current::build(max, fan::pushing(speed, facing), |i| catalyst_at(lv, at(i)));
    let mut handlers = Vec::new();
    for i in 1..=current.limit() {
        let segment = current.type_at((i - 1) as f32);
        for below in 0..2 {
            let p = at(i);
            let p = [p[0], p[1] - below, p[2]];
            if super::takes_from_side(dim, p) {
                handlers.push((p, catalyst_at(lv, p).or(segment)));
            }
            if facing.axis() == Axis::Y {
                break;
            }
        }
    }
    (current, handlers)
}

/// `AirCurrent.findMaxDepth`: how far air gets into a shape (its boxes) going `facing`,
/// sampled at five points of the face; infinite where a sample finds a hole.
fn max_depth(shape: &[[f64; 6]], facing: Dir) -> f32 {
    let axis = match facing.axis() {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    };
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let positive = facing.positive();
    let mut depth = 0.0f64;
    for (a, b) in [
        (0.25, 0.25),
        (0.25, 0.75),
        (0.5, 0.5),
        (0.75, 0.25),
        (0.75, 0.75),
    ] {
        let hit = shape
            .iter()
            .filter(|s| s[u] <= a && a <= s[u + 3] && s[v] <= b && b <= s[v + 3]);
        let d = if positive {
            let min = hit.map(|s| s[axis]).fold(f64::INFINITY, f64::min);
            if min == f64::INFINITY {
                return f32::INFINITY;
            }
            min
        } else {
            let max = hit.map(|s| s[axis + 3]).fold(f64::NEG_INFINITY, f64::max);
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
fn catalyst_at(lv: &ServerLevel<'_>, p: [i32; 3]) -> Option<Kind> {
    let fluid = lv
        .level
        .fluid_state(&*lv.host, p)
        .filter(|f| f.amount > 0)
        .map(|f| f.fluid.id(&*lv.host));
    let mut c = Catalyst {
        water: matches!(
            fluid.as_deref(),
            Some("minecraft:water" | "minecraft:flowing_water")
        ),
        lava: matches!(
            fluid.as_deref(),
            Some("minecraft:lava" | "minecraft:flowing_lava")
        ),
        splashing: lv.block_tagged(p, "create:fan_processing_catalysts/splashing"),
        haunting: lv.block_tagged(p, "create:fan_processing_catalysts/haunting"),
        smoking: lv.block_tagged(p, "create:fan_processing_catalysts/smoking"),
        blasting: lv.block_tagged(p, "create:fan_processing_catalysts/blasting"),
        ..Default::default()
    };
    if !(c.haunting || c.smoking || c.blasting) {
        return fan::kind_at(&c);
    }
    if lv.block_tagged(p, "minecraft:campfires") {
        c.campfire_lit = lv.property(p, "lit").map(|v| v == "true");
    }
    c.soul_flame = lv.property(p, "flame_type").map(|v| v == "soul");
    c.heat = lv.property(p, "blaze").and_then(|v| {
        Some(match v.as_str() {
            "none" => Heat::None,
            "smouldering" => Heat::Smouldering,
            "fading" => Heat::Fading,
            "kindled" => Heat::Kindled,
            "seething" => Heat::Seething,
            _ => return None,
        })
    });
    fan::kind_at(&c)
}

/// `DepotBehaviour.applyToAllItems` with `FanProcessing.applyProcessing`: the held stack's
/// progress, and when done, its results: the first held (centred), the rest in the outputs,
/// what finds no room dropped; none, and the stack is gone.
fn fan_on_depot(lv: &mut ServerLevel<'_>, at: [i32; 3], kind: Kind) {
    let dim = lv.dim;
    let Some(Some(mut held)) = with_world(|w| w.dims[dim].depots.get(&at).copied()).flatten()
    else {
        return;
    };
    let Some(slots) = lv.slots(at, depot::SLOTS) else {
        return;
    };
    let Some(stack) = slots[0].clone() else {
        return;
    };
    let host = &*lv.host;
    let item = stacks::id(host, &stack);
    let count = stack.count().min(255) as u8;
    let fire_resistant = fire_resistant(lv, &stack);
    let can =
        recipes::with(|r| fan::can_process(kind, &item, fire_resistant, r.fan())) == Some(true);
    let done = fan::advance(&mut held.fan, kind, u32::from(count), can);
    if !done {
        with_world(|w| w.dims[dim].depots.insert(at, Some(held)));
        return;
    }
    let mut rng = processing::Rng::new(lv.seed_at(at));
    let max = |i: &str| stacks::limit(host, i);
    let results =
        recipes::with(|r| fan::process(kind, &item, count, r.fan(), max, &mut rng)).flatten();
    let Some(results) = results else {
        with_world(|w| w.dims[dim].depots.insert(at, Some(held)));
        return;
    };
    let mut first = None;
    let mut outputs: Vec<processing::Slot> = slots[depot::OUTPUTS]
        .iter()
        .map(|s| stacks::slot_of(host, s))
        .collect();
    let mut dropped = Vec::new();
    for (i, n) in &results {
        if first.is_none() {
            first = stacks::stack(host, i, u32::from(*n));
            continue;
        }
        let left = processing::insert_stacked(&mut outputs, i, *n, max(i));
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
    lv.set_slots(at, &changes);
    for (i, n) in dropped {
        let p = [
            f64::from(at[0]) + 0.5,
            f64::from(at[1] + 1),
            f64::from(at[2]) + 0.5,
        ];
        lv.spawn_items(p, &i, u32::from(n));
    }
    // getSimilar keeps the motion and the progress (done: -1, never again by this type).
    let next = (!results.is_empty()).then_some(depot::Held {
        position: 0.5,
        ..held
    });
    with_world(|w| {
        w.dims[dim].depots.insert(at, next);
        w.depot_changed(dim, at);
    });
}

/// `BeltBlockEntity.applyToAllItems` with `FanProcessing.applyProcessing`: the items over the
/// segment at `at` (within .51 of its middle).
fn fan_on_belt(lv: &mut ServerLevel<'_>, at: [i32; 3], kind: Kind) {
    let dim = lv.dim;
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
    let mut rng = processing::Rng::new(lv.seed_at(at));
    let mut made = 0;
    let lvr = &*lv;
    let host = &*lvr.host;
    let max = |i: &str| stacks::limit(host, i);
    let dirty = recipes::with(|r| {
        inv.apply_within(index + 0.5, 0.51, |it| {
            let fire_resistant = stacks::key(host, &it.item).is_some_and(|k| {
                host.item_default_component(k, "minecraft:damage_resistant")
                    .is_some()
            });
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
                    let angle = random_angle(lvr, [at[0] + made, at[1], at[2]]);
                    let mut out = belt::Item::new(&item, count, angle, upright(host, &item));
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
    })
    .unwrap_or(false);
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
