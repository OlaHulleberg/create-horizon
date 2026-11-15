//! Crushing wheels (`CrushingWheelBlock`) and the controller between two of them
//! (`CrushingWheelControllerBlockEntity`): the item it draws in, crushes and lets out.

use super::{recipes, stacks};
use crate::glue::kinetics::dir_name;
use crate::glue::logistics::{insert_into_from, random_angle, upright};
use crate::glue::server::{ServerLevel, update_all};
use crate::glue::world::with_world;
use create_engine::kinetics::{Axis, Dir};
use create_engine::{Block, belt, crusher, processing};

/// Crushing wheels: the controllers between wheels turning against each other
/// (`CrushingWheelBlock.updateControllers`, after a wheel is placed, broken or changes speed),
/// then the turning controllers' `tick`.
pub fn tick_crushers(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].crusher_checks)).unwrap_or_default();
    checks.sort();
    checks.dedup();
    for pos in checks {
        fix_controllers(lv, pos);
    }
    let running = with_world(|w| w.dims[dim].running_crushers.clone()).unwrap_or_default();
    if running.is_empty() || !recipes::ready() {
        return;
    }
    for pos in running {
        tick_crusher(lv, pos);
    }
}

/// `updateControllers` for the wheel (or the place a wheel left) at `pos`.
fn fix_controllers(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let wheel = lv
        .info(pos)
        .filter(|(b, _)| *b == Block::CrushingWheel)
        .map(|(_, i)| i.axis);
    for side in Dir::ALL {
        let cpos = super::step(pos, side, 1);
        let existing = lv
            .info(cpos)
            .is_some_and(|(b, _)| b == Block::CrushingWheelController);
        let want = wheel.and_then(|axis| {
            let axis = axis?;
            let speed = lv.speed(pos);
            let opos = super::step(pos, side, 2);
            let other = lv
                .info(opos)
                .filter(|(b, _)| *b == Block::CrushingWheel)
                .and_then(|(_, i)| i.axis)
                .map(|a| (a, lv.speed(opos)));
            crusher::controller(axis, side, speed, other).map(|c| (c, speed))
        });
        match want {
            // A wheel that went (`onRemove`) or no partner: no controller.
            None => {
                if existing && (wheel.is_none() || wheel.flatten() != Some(side.axis())) {
                    let _ = lv.level.remove_block(&mut *lv.host, cpos, false);
                }
            }
            Some(((valid, facing), speed)) => {
                if !existing && !lv.replaceable(cpos) {
                    continue;
                }
                let Some(state) = lv.host.block_state_id(&format!(
                    "create:crushing_wheel_controller[facing={},valid={valid}]",
                    dir_name(facing)
                )) else {
                    continue;
                };
                if lv.block_state(cpos) != Some(state) {
                    lv.set_state(cpos, state, update_all());
                }
                let crushing = if valid {
                    crate::glue::advancements::crushing(lv, pos, side);
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
fn tick_crusher(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(facing) = lv.facing_of(pos, Block::CrushingWheelController) else {
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
        let found = lv.items_in(lo, hi).into_iter().find(|d| d.id() == id);
        match found {
            None if crush_entity(lv, pos, facing, speed, id) => {}
            None => c.entity = None,
            Some(d) => {
                let item = stacks::id(&*lv.host, &d.stack);
                let count = d.stack.count().min(255) as u8;
                let (motion, taken) = crusher::hold(pos, facing, speed, d.at);
                if taken || crusher::on_shelf(pos, facing, d.at) {
                    let time = recipes::with(|r| {
                        crusher::recipe(&r.crushing, &r.milling, &item).map(|x| x.time)
                    })
                    .flatten();
                    crusher::intake(&mut c, &item, count, time);
                    lv.set_dropped(&d, None, d.velocity);
                } else {
                    lv.set_dropped(&d, Some(d.stack.clone()), motion);
                }
            }
        }
    } else if c.slots.is_empty() {
        // `checkEntityForProcessing`: the first item inside starts it.
        let (lo, hi) = cell(0.0);
        c.entity = lv
            .items_in(lo, hi)
            .into_iter()
            .find(|d| !crusher::bypassed(pos, facing, d.at))
            .map(|d| d.id())
            .or_else(|| crushable_in(lv, lo, hi));
    } else {
        match crusher::tick_inventory(&mut c, speed) {
            crusher::Step::Wait => {}
            crusher::Step::Apply => {
                let host = &*lv.host;
                let max = |i: &str| stacks::limit(host, i);
                let mut rng = processing::Rng::new(lv.seed_at(pos));
                let item = c.slots[0].0.clone();
                // The input's crafting remainder comes out with the results.
                let left = stacks::remainder(host, &item);
                recipes::with(|r| {
                    let recipe = crusher::recipe(&r.crushing, &r.milling, &item);
                    crusher::apply(&mut c, recipe, max, &mut rng, left.as_deref());
                });
            }
            crusher::Step::Output => {
                // Facing anywhere but up: a belt or depot below (and on, past a sideways
                // facing) takes the results from that side; what it doesn't take waits.
                let side = if facing.axis() == Axis::Y {
                    [0, 0, 0]
                } else {
                    facing.offset()
                };
                let next = [pos[0] + side[0], pos[1] - 1, pos[2] + side[2]];
                if facing != Dir::Up && crate::glue::logistics::direct_input_at(lv, next) {
                    // `canInsertFromSide`: what it takes nothing from this side waits.
                    if !crate::glue::logistics::direct_input_accepts(lv, next, facing) {
                        return;
                    }
                    let mut left = Vec::new();
                    for (item, count) in std::mem::take(&mut c.slots) {
                        let up = upright(&*lv.host, &item);
                        let one = belt::Item::new(&item, count, random_angle(lv, pos), up);
                        let n = insert_into_from(lv, next, facing, &one, false).unwrap_or(count);
                        if n > 0 {
                            left.push((item, n));
                        }
                    }
                    c.slots = left;
                } else {
                    // Results thrown out past the facing side (`outPos`, `outSpeed`).
                    let (at, v) = crusher::output(pos, facing);
                    for (item, count) in std::mem::take(&mut c.slots) {
                        lv.spawn_moving(at, &item, u32::from(count), v);
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

/// The controller's inventory taking `count` of `item` from a belt, a saw or another crusher
/// (`ProcessingInventory.insertItem` through its handler): only while it holds nothing and
/// draws in no entity. Whether it took them.
pub fn crusher_put(lv: &mut ServerLevel<'_>, pos: [i32; 3], item: &str, count: u8) -> bool {
    let dim = lv.dim;
    let time =
        recipes::with(|r| crusher::recipe(&r.crushing, &r.milling, item).map(|x| x.time)).flatten();
    with_world(|w| {
        let (c, _) = w.dims[dim].crushers.get_mut(&pos)?;
        if c.occupied() {
            return None;
        }
        crusher::intake(c, item, count, time);
        w.mark_unsaved(dim, pos);
        Some(())
    })
    .flatten()
    .is_some()
}

/// `CrushingWheelControllerBlockEntity.CRUSHER_DAMAGE` (`crushingDamage`, 4 by default).
const CRUSHING_DAMAGE: f32 = 4.0;

/// The damage the wheels deal. Create's own `create:crush` (through armour, `bypasses_armor`)
/// is not among the world's damage types: a native mod's `data/<ns>/damage_type` files are not
/// read (API_GAPS.md), so vanilla's cramming, which also goes through armour, stands in.
const CRUSH: &str = "minecraft:cramming";

/// `CrushingWheelControllerBlock.checkEntityForProcessing` for what is not an item: the first
/// living entity in the controller's cell, but a creative player or any player on peaceful.
fn crushable_in(lv: &mut ServerLevel<'_>, lo: [f64; 3], hi: [f64; 3]) -> Option<u32> {
    let peaceful = lv.level.difficulty(&*lv.host) == Some("peaceful");
    let found = lv.level.entities_in(&*lv.host, lo, hi).unwrap_or_default();
    found
        .into_iter()
        .find(|&e| {
            if lv.host.item_entity(e).is_some() || lv.host.living(e).is_none() {
                return false;
            }
            if e.kind(&*lv.host).map(|k| k.id(&*lv.host)).as_deref() == Some("minecraft:player") {
                let creative = e
                    .uuid(&mut *lv.host)
                    .and_then(|u| lv.host.player(u))
                    .and_then(|p| p.is_creative(&*lv.host))
                    .unwrap_or(false);
                return !creative && !peaceful;
            }
            true
        })
        .map(|e| e.id())
}

/// `CrushingWheelControllerBlockEntity.tick` for a held entity that is not an item: drawn to
/// the middle and along the facing (held still across it when sneaking), hurt by the crushing
/// (`CreateDamageSources.crush`) every tick and moved out past the wheels when it dies (just
/// before the blow that would kill it, so its drops fall there). False when it is gone or out of
/// the cell: the controller lets go.
fn crush_entity(lv: &mut ServerLevel<'_>, pos: [i32; 3], facing: Dir, speed: f32, id: u32) -> bool {
    let lo = pos.map(|v| f64::from(v) - 0.5);
    let hi = pos.map(|v| f64::from(v) + 1.5);
    let Some(entity) = lv
        .level
        .entities_in(&*lv.host, lo, hi)
        .unwrap_or_default()
        .into_iter()
        .find(|e| e.id() == id)
    else {
        return false;
    };
    let Some(living) = lv.host.living(entity) else {
        return false;
    };
    if living.alive(&*lv.host) != Some(true) {
        return false;
    }
    let Some(at) = entity.position(&*lv.host) else {
        return false;
    };
    let (mut motion, _) = crusher::hold(pos, facing, speed, at);
    if entity.is_shift_key_down(&*lv.host) == Some(true) {
        let a = match facing.axis() {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        };
        for (i, m) in motion.iter_mut().enumerate() {
            if i != a {
                *m = 0.0;
            }
        }
    }
    entity.set_velocity(&mut *lv.host, motion);
    // `entityOutPos`: half a block past where items leave.
    let (out, _) = crusher::output(pos, facing);
    let o = facing.offset();
    let out = [0, 1, 2].map(|i| out[i] + 0.5 * f64::from(o[i]));
    if living
        .health(&*lv.host)
        .is_some_and(|h| h - CRUSHING_DAMAGE <= 0.0)
    {
        entity.teleport_to(&mut *lv.host, out, None);
    }
    let _ = entity.hurt(&mut *lv.host, CRUSH, CRUSHING_DAMAGE, None, None);
    if living.alive(&*lv.host) != Some(true) {
        entity.teleport_to(&mut *lv.host, out, None);
    }
    true
}
