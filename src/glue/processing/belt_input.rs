//! What the machines hand their results to beside them: a belt or a depot taking a stack from a
//! side (`DirectBeltInputBehaviour.handleInsertion`), as the legacy logistics glue's
//! `insert_into_from` did for the saw, the crushing wheels and the crafter; with the two belt
//! facts every placed stack needs (`upright`, `random_angle`).

use super::stacks;
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::kinetics::{Axis, Dir};
use create_engine::{belt, depot, ejector};
use mod_api::{Host, ItemStack};

/// `BeltHelper.isItemUpright`: items of `create:upright_on_belt` stand, unless also in
/// `create:not_upright_on_belt`.
pub fn upright(host: &dyn Host, item: &str) -> bool {
    stacks::item_tagged(host, item, "create:upright_on_belt")
        && !stacks::item_tagged(host, item, "create:not_upright_on_belt")
}

/// `TransportedItemStack`'s `R.nextInt(360)`: Create's shared random; any angle will do.
pub fn random_angle(lv: &ServerLevel<'_>, pos: [i32; 3]) -> i32 {
    let mut x = (lv.game_time() as u64)
        ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (pos[2] as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (pos[1] as u64) << 40;
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x % 360) as i32
}

/// Whether a depot or a belt is at `p` (`DirectBeltInputBehaviour` there).
pub fn takes_from_side(dim: usize, p: [i32; 3]) -> bool {
    with_world(|w| {
        let d = &w.dims[dim];
        d.depots.contains_key(&p) || d.belt_of.contains_key(&p)
    }) == Some(true)
}

/// A belt item as a stack, its sequenced assembly progress with it.
pub fn belt_stack(host: &dyn Host, item: &belt::Item) -> Option<ItemStack> {
    let mut stack = stacks::stack(host, &item.item, u32::from(item.count))?;
    if let Some((_, p, fraction)) = item.progress.as_ref().and_then(super::transitional_of) {
        stacks::set_sequenced_progress(&mut stack, &p.id, p.step, fraction);
    }
    Some(stack)
}

/// The belt segment at `pos`: its controller and its index along the chain.
fn belt_segment(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<([i32; 3], i32, belt::Segment)> {
    let seg = lv.info(pos)?.1.belt?;
    let dim = lv.dim;
    let (c, index) = with_world(|w| {
        let d = &w.dims[dim];
        let c = *d.belt_of.get(&pos)?;
        let index = d.belts.get(&c)?.chain.iter().position(|p| *p == pos)? as i32;
        Some((c, index))
    })
    .flatten()?;
    Some((c, index, seg))
}

/// `item` put into the depot or belt at `next` from its `side` (`from_belt`: Create's smoother
/// hand-over starts it a quarter block back): how many it did not take; None for neither there.
pub fn insert_into_from(
    lv: &mut ServerLevel<'_>,
    next: [i32; 3],
    side: Dir,
    item: &belt::Item,
    from_belt: bool,
) -> Option<u8> {
    let dim = lv.dim;
    let is_depot = with_world(|w| w.dims[dim].depots.contains_key(&next)) == Some(true);
    if is_depot {
        // DepotBehaviour.tryInsertingFromSide: only into an empty depot (an ejector's only
        // while charged, `canAcceptItems`).
        let charged = with_world(|w| {
            w.dims[dim]
                .ejectors
                .get(&next)
                .is_none_or(|e| e.state == ejector::State::Charged)
        })
        .unwrap_or(true);
        let empty = lv
            .slots(next, depot::SLOTS)
            .is_some_and(|s| s.iter().all(Option::is_none));
        if !empty || !charged {
            return Some(item.count);
        }
        let put = belt_stack(&*lv.host, item)?;
        if !lv.set_slots(next, &[(0, Some(put))]) {
            return None;
        }
        let held = depot::Held {
            position: 0.0,
            from: depot::Side::from_dir(side),
            angle: item.angle,
            side_offset: item.side_offset,
            locked: false,
            fan: item.fan,
        };
        with_world(|w| w.depot_put(dim, next, Some(held)));
        return Some(0);
    }
    // BeltBlockEntity.tryInsertingFromSide on the next belt.
    let (c2, index, seg2) = belt_segment(lv, next)?;
    if !seg2.transports() {
        return None;
    }
    let rpm2 = lv.speed(c2);
    let speed2 = belt::direction_aware_speed(rpm2, seg2.facing);
    with_world(|w| {
        let b = w.dims[dim].belts.get_mut(&c2)?;
        let movement2 = belt::movement_facing(rpm2, b.segment.facing);
        if rpm2 == 0.0
            || movement2 == side.opposite()
            || !b.items.can_insert_at_from_side(index, side, movement2)
        {
            return Some(item.count);
        }
        let mut moved = item.clone();
        moved.locked = false;
        moved.position = index as f32 + 0.5 - speed2.signum() / 16.0;
        // From above or below: in the segment's middle.
        if side.axis() == Axis::Y {
        } else if movement2 != side {
            moved.side_offset = if side.positive() { 0.675 } else { -0.675 };
            if side.axis() == Axis::X {
                moved.side_offset = -moved.side_offset;
            }
        } else {
            // A smoother hand-over from the belt behind it.
            let extra = if from_belt { 0.26 } else { 0.0 };
            moved.position = if speed2 > 0.0 {
                index as f32 - extra
            } else {
                index as f32 + 1.0 + extra
            };
        }
        moved.inserted_at = index;
        moved.inserted_from = Some(side);
        b.items.add(moved);
        w.dims[dim].belt_changes.push(c2);
        Some(0)
    })
    .flatten()
}
