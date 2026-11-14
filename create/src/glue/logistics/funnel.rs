//! Funnels on the server: a horizontal funnel over a belt becoming its belt funnel and back
//! (`FunnelBlock.updateShape`, `BeltFunnelBlock.updateShape`), and extracting funnels taking
//! from the inventory behind them (`FunnelBlockEntity.tick`).

use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::super::world::with_world;
use super::belt::{belt_segment, insert_into_from};
use super::info_at;
use super::items::{self, face, random_angle, upright};
use create_engine::Block;
use create_engine::kinetics::{Axis, Dir};
use mod_api::{Direction, HostBlockStates, StateConstraint};

/// A funnel's state as its rules read it: its kind (brass or not), facing, `powered`,
/// `extracting`, and a belt funnel's shape (None for a funnel on an inventory).
#[derive(Clone, Copy, Debug)]
pub struct FunnelState {
    pub brass: bool,
    pub facing: Dir,
    pub powered: bool,
    pub extracting: bool,
    pub belt: Option<create_engine::funnel::BeltShape>,
}

/// Whether the funnel at `pos` lets `stack` through: a brass funnel's filter slot
/// (`FilteringBehaviour.test`); an andesite funnel's always does.
pub fn funnel_passes(lv: &ServerLevel<'_>, pos: [i32; 3], stack: &mod_api::ItemStack) -> bool {
    let dim = lv.dim;
    with_world(|w| {
        crate::glue::items::filters::passes(&*lv.host, w.dims[dim].chute_filters.get(&pos), stack)
    })
    .unwrap_or(true)
}

/// A player's click on a brass funnel's filter slot (`FilteringBehaviour.onShortInteract`
/// through `ValueSettingsInputHandler`, at `FunnelFilterSlotPositioning`): the held item
/// becomes its filter (a filter item taken from the hand, the one it held given back), an empty
/// hand clears it. True when the click was the slot's.
pub fn use_funnel_filter(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    click: &mod_api::BlockClick<'_>,
) -> bool {
    use crate::glue::items::filters;
    let Some(f) = funnel_at(lv, pos) else {
        return false;
    };
    if !f.brass {
        return false;
    }
    let Some(p) = lv.host.player(click.player) else {
        return false;
    };
    // `ValueSettingsInputHandler.canInteract`, then `canShortInteract` (a wrench or an arm is
    // used on the block instead).
    if !crate::glue::interaction::values::may_use_value_box(&*lv.host, p) {
        return false;
    }
    let held = click.stack.map(|s| items::id_of(&*lv.host, s));
    if held
        .as_deref()
        .is_some_and(|h| matches!(h, "create:wrench" | "create:mechanical_arm"))
    {
        return false;
    }
    let Some(side) = Dir::from_offset(click.hit.face) else {
        return false;
    };
    let Some(slot) = create_engine::funnel::filter_slot(f.facing, f.belt, side) else {
        return false;
    };
    let local = [0, 1, 2].map(|i| click.hit.point[i] - f64::from(pos[i]));
    if !create_engine::board::hits_box(slot, local) {
        return false;
    }
    let filter = filters::take_filter(lv.host, p, click.hand);
    let dim = lv.dim;
    let old = with_world(|w| {
        let d = &mut w.dims[dim];
        let old = match filter {
            Some(f) => d.chute_filters.insert(pos, f),
            None => d.chute_filters.remove(&pos),
        };
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        // `withCallback(invVersionTracker.reset)`: it looks at its inventory again.
        if let Some(due) = d.funnels.get_mut(&pos) {
            *due = 0;
            d.funnel_timers.at(0, pos);
        }
        old
    })
    .flatten();
    filters::return_filter(lv, p, pos, old);
    true
}

/// Up to `max` of what the inventory `behind` gives on `side` to the funnel at `pos`: what its
/// filter passes (a brass funnel's), and with `plain` only stacks without components (what
/// belts carry).
fn take_filtered(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    behind: [i32; 3],
    side: Direction,
    plain: bool,
    max: u32,
) -> Option<mod_api::ItemStack> {
    let dim = lv.dim;
    let filter = with_world(|w| w.dims[dim].chute_filters.get(&pos).cloned()).flatten();
    let Some(filter) = filter else {
        return items::take_from(
            lv,
            behind,
            side,
            |s| !plain || !items::has_components(s),
            max,
        );
    };
    let from = items::items_at(lv, behind, Some(side))?;
    items::extract_matching(
        lv,
        &from,
        |host, r| {
            let s = r.as_stack();
            (!plain || !items::has_components(s))
                && crate::glue::items::filters::passes(&*host, Some(&filter), s)
        },
        max,
    )
}

/// Whether `block` is a funnel (`AbstractFunnelBlock`).
pub fn is_funnel(block: Block) -> bool {
    matches!(
        block,
        Block::AndesiteFunnel
            | Block::BrassFunnel
            | Block::AndesiteBeltFunnel
            | Block::BrassBeltFunnel
    )
}

/// The funnel at `pos`, if one is there (a miss is a state test).
pub fn funnel_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<FunnelState> {
    let (block, info) = info_at(lv, pos)?;
    let (brass, belt) = match block {
        Block::AndesiteFunnel => (false, false),
        Block::BrassFunnel => (true, false),
        Block::AndesiteBeltFunnel => (false, true),
        Block::BrassBeltFunnel => (true, true),
        _ => return None,
    };
    Some(FunnelState {
        brass,
        facing: info.facing?,
        powered: info.powered,
        extracting: info.extracting,
        belt: if belt {
            Some(info.belt_funnel.unwrap_or_default())
        } else {
            None
        },
    })
}

/// The way the belt below `pos` moves (`getMovementFacing`), when a belt that carries items is
/// there.
pub fn belt_movement_below(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Dir> {
    let below = [pos[0], pos[1] - 1, pos[2]];
    let (c, _, seg) = belt_segment(lv, below)?;
    if !seg.transports() {
        return None;
    }
    let dim = lv.dim;
    let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
    Some(create_engine::belt::movement_facing(rpm, seg.facing))
}

/// The facing of a belt that carries items below `pos` (`BeltFunnelBlock.isOnValidBelt`; Create
/// also lets saws and item drains carry belt funnels: not yet).
fn belt_facing_below(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Dir> {
    let (_, info) = info_at(lv, [pos[0], pos[1] - 1, pos[2]])?;
    let seg = info.belt?;
    seg.transports().then_some(seg.facing)
}

/// Whether a depot is below `pos` (`DepotBehaviour`'s input takes belt funnels).
fn depot_below(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    info_at(lv, [pos[0], pos[1] - 1, pos[2]]).is_some_and(|(b, _)| b == Block::Depot)
}

/// `activateExtractingBeltFunnel` over a depot: up to `max` of the inventory behind onto the
/// depot below while it holds nothing, sliding in from the funnel's side. Answers as
/// `extract_in_front`.
fn push_onto_depot(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    facing: Dir,
    side: Direction,
    behind: [i32; 3],
    max: u32,
) -> Option<Option<bool>> {
    let below = [pos[0], pos[1] - 1, pos[2]];
    if !super::depot::depot_items(lv, below).is_empty() {
        return Some(Some(false));
    }
    let Some(stack) = take_filtered(lv, pos, behind, side, false, max) else {
        return Some(None);
    };
    let id = items::id_of(&*lv.host, &stack);
    if !items::set_slot(lv, below, 0, Some(stack)) {
        return Some(None);
    }
    // `handleInsertion(stack, facing)`: it comes in from the funnel's side.
    let held = create_engine::depot::Held {
        position: 0.0,
        from: create_engine::depot::Side::from_dir(facing),
        ..create_engine::depot::Held::new(random_angle(lv, below), upright(&*lv.host, &id))
    };
    let dim = lv.dim;
    with_world(|w| w.depot_put(dim, below, Some(held)));
    Some(Some(true))
}

/// `DepotBehaviour.handleBeltFunnelOutput` for a pulling belt funnel over a depot: the depot's
/// outputs, then its held stack once it has come to rest, go into the inventory behind the
/// funnel as far as they fit and its filter passes them (`tryExportingToBeltFunnel`). Answers as
/// `extract_in_front`.
fn pull_from_depot(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    side: Direction,
    behind: [i32; 3],
) -> Option<Option<bool>> {
    let below = [pos[0], pos[1] - 1, pos[2]];
    let dim = lv.dim;
    let resting = with_world(|w| {
        w.dims[dim]
            .depots
            .get(&below)
            .is_some_and(|h| h.is_none_or(|h| !h.sliding()))
    })
    .unwrap_or(false);
    let mut slots: Vec<u32> = (1..create_engine::depot::SLOTS as u32).collect();
    if resting {
        slots.push(0);
    }
    for slot in slots {
        let Some(stack) = items::slot(lv, below, slot) else {
            continue;
        };
        if !funnel_passes(lv, pos, &stack) {
            continue;
        }
        let count = stack.count();
        let left = items::insert_item(lv, behind, side, stack);
        if left.as_ref().is_some_and(|l| l.count() == count) {
            continue;
        }
        let emptied = left.is_none();
        items::set_slot(lv, below, slot, left);
        if slot == 0 && emptied {
            with_world(|w| w.depot_put(dim, below, None));
        }
        return Some(Some(true));
    }
    Some(None)
}

/// Funnels whose block below changed (`FunnelBlock.updateShape`, `BeltFunnelBlock.updateShape`):
/// a horizontal funnel over a belt becomes its belt funnel (`getEquivalentBeltFunnel`, shaped by
/// `getShapeForPosition`), a belt funnel no longer on a belt becomes its funnel again (extracting
/// when it pushed), and a belt funnel's shape follows the belt below (wrenched shapes kept).
pub fn tick_funnel_shapes(lv: &mut ServerLevel<'_>) {
    use create_engine::funnel::{BeltShape, shape_for_position, updated_shape};
    let dim = lv.dim;
    let checks = with_world(|w| std::mem::take(&mut w.dims[dim].funnel_shapes)).unwrap_or_default();
    for pos in checks {
        let Some(f) = funnel_at(lv, pos) else {
            continue;
        };
        if f.facing.axis() == Axis::Y {
            continue;
        }
        let Some(state) = lv.block_state(pos) else {
            continue;
        };
        let water = {
            let facts = lv.host.block_states();
            state
                .property(&facts, "waterlogged")
                .ok()
                .flatten()
                .unwrap_or("false")
                .to_owned()
        };
        // What carries a belt funnel below: a belt (its facing), or a depot.
        let belt = belt_facing_below(lv, pos)
            .map(Some)
            .or_else(|| depot_below(lv, pos).then_some(None));
        let flag = |b: bool| if b { "true" } else { "false" };
        let (block, shape) = match (f.belt, belt) {
            (None, Some(b)) => (
                if f.brass {
                    Block::BrassBeltFunnel
                } else {
                    Block::AndesiteBeltFunnel
                },
                shape_for_position(b, f.facing, f.extracting),
            ),
            (Some(shape), None) => (
                if f.brass {
                    Block::BrassFunnel
                } else {
                    Block::AndesiteFunnel
                },
                shape,
            ),
            (Some(shape), Some(b)) => {
                let fresh = shape_for_position(b, f.facing, shape == BeltShape::Pushing);
                let now = updated_shape(shape, fresh);
                if now == shape {
                    continue;
                }
                (
                    if f.brass {
                        Block::BrassBeltFunnel
                    } else {
                        Block::AndesiteBeltFunnel
                    },
                    now,
                )
            }
            (None, None) => continue,
        };
        let facing = super::super::kinetics::dir_name(f.facing);
        let props: [(&str, &str); 4] = if f.belt.is_some() && belt.is_none() {
            [
                ("facing", facing),
                ("powered", flag(f.powered)),
                ("extracting", flag(shape == BeltShape::Pushing)),
                ("waterlogged", &water),
            ]
        } else {
            [
                ("facing", facing),
                ("powered", flag(f.powered)),
                ("shape", shape.name()),
                ("waterlogged", &water),
            ]
        };
        let constraints: Vec<StateConstraint> = props
            .iter()
            .map(|(k, v)| StateConstraint::new(*k, *v))
            .collect();
        let next = {
            let facts = lv.host.block_states();
            mod_api::BlockState::of(&facts, states::key(block), &constraints)
        };
        if let Ok(next) = next
            && next != state
        {
            lv.set_state(pos, next, update_all());
        }
    }
}

/// Funnels due to look again (`FunnelBlockEntity.tick` once its cooldown has run out): an
/// extracting funnel on an inventory drops a stack in front while nothing lies there
/// (`activateExtractor`); a belt funnel pushing onto its belt puts one there
/// (`activateExtractingBeltFunnel`); either looks again 8 ticks after it moved items, the next
/// tick while blocked, and 8 ticks later when the inventory gave nothing (Create looks every
/// tick then). An inserting, pulling or powered funnel is dormant until its state changes; a
/// tick with no funnel due costs one look at the earliest.
pub fn tick_funnels(lv: &mut ServerLevel<'_>) {
    use create_engine::funnel::{EXTRACTION_COOLDOWN, Mode, amount, belt_mode};
    let dim = lv.dim;
    let now = lv.game_time().max(0) as u64;
    let due = with_world(|w| {
        let d = &mut w.dims[dim];
        if d.funnel_timers.is_empty() {
            return Vec::new();
        }
        let funnels = &d.funnels;
        d.funnel_timers
            .due(now, |p, t| funnels.get(&p).is_some_and(|due| *due == t))
    })
    .unwrap_or_default();
    let schedule = |pos: [i32; 3], at: u64| {
        with_world(|w| {
            let d = &mut w.dims[dim];
            if let Some(due) = d.funnels.get_mut(&pos) {
                *due = at;
                if at != u64::MAX {
                    d.funnel_timers.at(at, pos);
                }
            }
        });
    };
    for pos in due {
        let Some(f) = funnel_at(lv, pos) else {
            schedule(pos, u64::MAX);
            continue;
        };
        let mode = match f.belt {
            Some(shape) => belt_mode(f.powered, shape, belt_movement_below(lv, pos), f.facing),
            None if f.powered => Mode::Paused,
            None if f.extracting => Mode::Extract,
            None => Mode::Collect,
        };
        let side = face(f.facing);
        let behind = create_engine::funnel::behind(pos, f.facing);
        let max = amount(f.brass);
        let next = match mode {
            Mode::Extract => extract_in_front(lv, pos, f.facing, side, behind, max),
            Mode::PushingToBelt if depot_below(lv, pos) => {
                push_onto_depot(lv, pos, f.facing, side, behind, max)
            }
            Mode::PushingToBelt => push_onto_belt(lv, pos, f.facing, side, behind, max),
            Mode::TakingFromBelt if depot_below(lv, pos) => pull_from_depot(lv, pos, side, behind),
            // Inserting, pulling from a belt (the belt does it), paused or invalid: dormant.
            _ => None,
        };
        schedule(
            pos,
            match next {
                Some(Some(false)) => now + 1,
                Some(_) => now + u64::from(EXTRACTION_COOLDOWN),
                None => u64::MAX,
            },
        );
    }
}

/// `activateExtractor`: Some(Some(true)) when a stack left, Some(Some(false)) when an item in
/// front blocks it, Some(None) when the inventory gave nothing.
fn extract_in_front(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    facing: Dir,
    side: Direction,
    behind: [i32; 3],
    max: u32,
) -> Option<Option<bool>> {
    let (at, v) = create_engine::funnel::output(pos, facing);
    // Something still in front: wait (`lastObserved`, the overflow area).
    let near = items::items_in(
        lv,
        [at[0] - 0.5, at[1] - 0.5, at[2] - 0.5],
        [at[0] + 0.5, at[1] + 0.5, at[2] + 0.5],
    );
    if !near.is_empty() {
        return Some(Some(false));
    }
    let Some(stack) = take_filtered(lv, pos, behind, side, false, max) else {
        return Some(None);
    };
    items::drop_stack(lv, at, v, stack, 0);
    Some(Some(true))
}

/// `activateExtractingBeltFunnel`: up to `max` of the inventory behind onto the belt below,
/// inserted from the funnel's side (`handleInsertion(stack, facing)`) while the belt takes items
/// there (`canInsertFromSide`, `isOccupied`). Same answers as `extract_in_front`.
fn push_onto_belt(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    facing: Dir,
    side: Direction,
    behind: [i32; 3],
    max: u32,
) -> Option<Option<bool>> {
    let dim = lv.dim;
    let below = [pos[0], pos[1] - 1, pos[2]];
    let (c, index, _) = belt_segment(lv, below)?;
    let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
    let free = with_world(|w| {
        let b = w.dims[dim].belts.get(&c)?;
        let movement = create_engine::belt::movement_facing(rpm, b.segment.facing);
        Some(
            rpm != 0.0
                && movement != facing.opposite()
                && b.items.can_insert_at_from_side(index, facing, movement),
        )
    })
    .flatten()?;
    if !free {
        return Some(Some(false));
    }
    // The belts carry items without components: only those leave.
    let Some(stack) = take_filtered(lv, pos, behind, side, true, max) else {
        return Some(None);
    };
    let count = stack.count().min(64) as u8;
    let id = items::id_of(&*lv.host, &stack);
    let item =
        create_engine::belt::Item::new(&id, count, random_angle(lv, pos), upright(&*lv.host, &id));
    let left = insert_into_from(lv, below, facing, &item, false).unwrap_or(count);
    if left > 0 {
        // The belt took less after all: the rest goes back where it came from.
        if let Some(back) = items::with_count(&stack, u32::from(left))
            && let Some(rest) = items::insert_item(lv, behind, side, back)
        {
            let at = pos.map(|v| f64::from(v) + 0.5);
            items::drop_stack(lv, at, [0.0; 3], rest, 0);
        }
    }
    Some(Some(true))
}
