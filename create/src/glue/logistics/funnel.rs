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
    // With its amount board (`acceptsValueSettings`), the client holds the press: its release
    // (a short click) or the board comes on `create:value_settings` (`amount`).
    if super::amount::shown(lv, pos).is_some() {
        return true;
    }
    set_funnel_filter(lv, pos, p, click.hand);
    true
}

/// `FilteringBehaviour.onShortInteract` on the brass funnel at `pos`: the stack in `hand`
/// becomes its filter (a filter item taken from the hand, the one it held given back), an empty
/// hand clears it; an exact amount fits the new filter's stack size.
pub(crate) fn set_funnel_filter(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    p: mod_api::PlayerRef,
    hand: mod_api::Hand,
) {
    use crate::glue::items::filters;
    let filter = filters::take_filter(lv.host, p, hand);
    let max = super::amount::filter_max(&*lv.host, filter.as_ref());
    let dim = lv.dim;
    let old = with_world(|w| {
        let d = &mut w.dims[dim];
        let old = match filter {
            Some(f) => d.chute_filters.insert(pos, f),
            None => d.chute_filters.remove(&pos),
        };
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        if let Some(a) = d.filter_amounts.get_mut(&pos) {
            *a = a.fitted(max);
        }
        old
    })
    .flatten();
    wake_funnel(lv.dim, pos);
    let now = with_world(|w| w.dims[dim].chute_filters.get(&pos).cloned()).flatten();
    super::show_filter(lv, pos, now.as_ref());
    filters::return_filter(lv, p, pos, old);
}

/// `withCallback(invVersionTracker.reset)`: the funnel at `pos` looks at its inventory again.
pub(crate) fn wake_funnel(dim: usize, pos: [i32; 3]) {
    with_world(|w| {
        let d = &mut w.dims[dim];
        if let Some(due) = d.funnels.get_mut(&pos) {
            *due = 0;
            d.funnel_timers.at(0, pos);
        }
    });
}

/// What the inventory `behind` gives on `side` to the funnel at `pos`
/// (`invManipulation.extract(getModeToExtract(), getAmountToExtract())`): what its filter
/// passes (a brass funnel's), `amount` of it, exactly when `exact`.
fn take_filtered(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    behind: [i32; 3],
    side: Direction,
    (amount, exact): (u32, bool),
) -> Option<mod_api::ItemStack> {
    let dim = lv.dim;
    let filter = with_world(|w| w.dims[dim].chute_filters.get(&pos).cloned()).flatten();
    let Some(filter) = filter else {
        return items::take_from(lv, behind, side, |_| true, amount, exact);
    };
    let from = items::items_at(lv, behind, Some(side))?;
    items::extract_matching(
        lv,
        &from,
        |host, r| crate::glue::items::filters::passes(&*host, Some(&filter), r.as_stack()),
        amount,
        exact,
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

/// The facing of a belt that carries items below `pos` (`BeltFunnelBlock.isOnValidBelt`).
fn belt_facing_below(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Dir> {
    let (_, info) = info_at(lv, [pos[0], pos[1] - 1, pos[2]])?;
    let seg = info.belt?;
    seg.transports().then_some(seg.facing)
}

/// Whether a depot or a weighted ejector is below `pos` (`DepotBehaviour`'s input takes belt
/// funnels, `allowingBeltFunnels`).
fn depot_below(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    info_at(lv, [pos[0], pos[1] - 1, pos[2]])
        .is_some_and(|(b, _)| matches!(b, Block::Depot | Block::WeightedEjector))
}

/// Whether an item drain or an upward saw is below `pos`: their inputs take belt funnels too
/// (`ItemDrainBlockEntity`'s `allowingBeltFunnels`, `SawBlockEntity`'s
/// `allowingBeltFunnelsWhen(canProcess)`).
fn machine_below(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    info_at(lv, [pos[0], pos[1] - 1, pos[2]]).is_some_and(|(b, i)| match b {
        Block::ItemDrain => true,
        Block::MechanicalSaw => i.facing == Some(Dir::Up),
        _ => false,
    })
}

/// `DirectBeltInputBehaviour.tryExportingToBeltFunnel(stack, side, false)` of the saw or drain
/// at `pos`: None unless a pulling belt funnel on top faces `side`; else what is left of `stack`
/// once its inventory took what it takes and its filter passes.
pub(crate) fn export_up(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    side: Dir,
    stack: &mod_api::ItemStack,
) -> Option<u32> {
    let above = [pos[0], pos[1] + 1, pos[2]];
    let f = funnel_at(lv, above)?;
    if f.facing != side {
        return None;
    }
    super::arms::export_to_belt_funnel(lv, above, f, stack, false)
}

/// `activateExtractingBeltFunnel` over an item drain or upward saw: up to `max` of the inventory
/// behind into it from the funnel's side (`handleInsertion(stack, facing)`), only what it takes
/// whole (Create extracts only an amount its simulated insertion takes in full). Answers as
/// `extract_in_front`.
fn push_into_machine(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    facing: Dir,
    side: Direction,
    behind: [i32; 3],
    (amount, exact): (u32, bool),
) -> Option<Option<bool>> {
    let below = [pos[0], pos[1] - 1, pos[2]];
    let dim = lv.dim;
    // A saw takes one item and a drain one stack, each only while empty (`isOccupied`).
    let (busy, most) = with_world(|w| {
        let d = &w.dims[dim];
        match d.drains.get(&below) {
            Some(drain) => (drain.held.is_some(), amount),
            None => (d.saws.contains_key(&below), 1),
        }
    })
    .unwrap_or((true, 0));
    if busy || exact && amount > most {
        return Some(Some(false));
    }
    let Some(stack) = take_filtered(lv, pos, behind, side, (amount.min(most), exact)) else {
        return Some(None);
    };
    let count = stack.count().min(64) as u8;
    let moved = stack.clone();
    let (id, progress) = super::belt::belt_name(&*lv.host, &stack);
    let mut item =
        create_engine::belt::Item::new(&id, count, random_angle(lv, pos), upright(&*lv.host, &id));
    item.progress = progress;
    let left =
        super::belt::handle_insertion(lv, below, facing, &item, false, false).unwrap_or(count);
    if left > 0
        && let Some(back) = items::with_count(&stack, u32::from(left))
        && let Some(rest) = items::insert_item(lv, behind, side, back)
    {
        let at = pos.map(|v| f64::from(v) + 0.5);
        items::drop_stack(lv, at, [0.0; 3], rest, 0);
    }
    if left == count {
        return Some(Some(false));
    }
    super::flaps::funnel(lv, pos, false);
    crate::glue::redstone::detectors::funnel_transfer(lv, pos, &moved);
    Some(Some(true))
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
    max: (u32, bool),
) -> Option<Option<bool>> {
    let below = [pos[0], pos[1] - 1, pos[2]];
    if super::arms::is_ejector(lv.dim, below) {
        // An ejector: its own insertion (charged only, merging up to its stack size).
        let Some(stack) = take_filtered(lv, pos, behind, side, max) else {
            return Some(None);
        };
        let moved = stack.clone();
        let (id, progress) = super::belt::belt_name(&*lv.host, &stack);
        let count = stack.count().min(64) as u8;
        let mut item = create_engine::belt::Item::new(&id, count, random_angle(lv, below), false);
        item.progress = progress;
        let left =
            super::belt::handle_insertion(lv, below, facing, &item, false, false).unwrap_or(count);
        if left > 0
            && let Some(back) = items::with_count(&stack, u32::from(left))
            && let Some(rest) = items::insert_item(lv, behind, side, back)
        {
            let at = pos.map(|v| f64::from(v) + 0.5);
            items::drop_stack(lv, at, [0.0; 3], rest, 0);
        }
        if left == count {
            return Some(Some(false));
        }
        super::flaps::funnel(lv, pos, false);
        crate::glue::redstone::detectors::funnel_transfer(lv, pos, &moved);
        return Some(Some(true));
    }
    if !super::depot::depot_items(lv, below).is_empty() {
        return Some(Some(false));
    }
    let Some(stack) = take_filtered(lv, pos, behind, side, max) else {
        return Some(None);
    };
    let id = items::id_of(&*lv.host, &stack);
    let moved = stack.clone();
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
    super::flaps::funnel(lv, pos, false);
    crate::glue::redstone::detectors::funnel_transfer(lv, pos, &moved);
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
    // An ejector keeps the funnel facing its back for its launches (`canFunnelsPullFrom`).
    if let Some((Block::WeightedEjector, info)) = info_at(lv, below)
        && let Some(f) = funnel_at(lv, pos)
        && info.facing == Some(f.facing.opposite())
    {
        return Some(None);
    }
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
        let moved = stack.clone();
        let left = items::insert_item(lv, behind, side, stack);
        if left.as_ref().is_some_and(|l| l.count() == count) {
            continue;
        }
        let emptied = left.is_none();
        items::set_slot(lv, below, slot, left);
        if slot == 0 && emptied {
            with_world(|w| w.depot_put(dim, below, None));
        }
        super::flaps::funnel(lv, pos, true);
        crate::glue::redstone::detectors::funnel_transfer(lv, pos, &moved);
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
        // What carries a belt funnel below: a belt (its facing), a depot, a drain or an upward saw.
        let belt = belt_facing_below(lv, pos)
            .map(Some)
            .or_else(|| (depot_below(lv, pos) || machine_below(lv, pos)).then_some(None));
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
            // The filter stays with a brass funnel on and off a belt: its block entity shows it
            // again.
            if f.brass {
                let dim = lv.dim;
                let filter = with_world(|w| w.dims[dim].chute_filters.get(&pos).cloned()).flatten();
                if filter.is_some() {
                    super::show_filter(lv, pos, filter.as_ref());
                }
            }
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
    use create_engine::funnel::{EXTRACTION_COOLDOWN, Mode, belt_mode};
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
        let max = super::amount::funnel_extraction(lv, pos, &f);
        let next = match mode {
            Mode::Extract => extract_in_front(lv, pos, f.facing, side, behind, max),
            Mode::PushingToBelt if depot_below(lv, pos) => {
                push_onto_depot(lv, pos, f.facing, side, behind, max)
            }
            Mode::PushingToBelt if machine_below(lv, pos) => {
                push_into_machine(lv, pos, f.facing, side, behind, max)
            }
            Mode::PushingToBelt => push_onto_belt(lv, pos, f.facing, side, behind, max),
            Mode::TakingFromBelt if depot_below(lv, pos) => {
                // The depot hands its stack over each tick it holds one
                // (`DepotBehaviour.tick`, `handleBeltFunnelOutput`).
                pull_from_depot(lv, pos, side, behind);
                Some(Some(false))
            }
            // Inserting, pulling from a belt (the belt does it), paused or invalid: dormant.
            _ => None,
        };
        // `startCooldown` after a stack left: counted down over 8 ticks, the funnel acts on the
        // 9th. Blocked, or the inventory empty (`awaitNewVersion`, a check each tick), it tries
        // again the next tick.
        schedule(
            pos,
            match next {
                Some(Some(true)) => now + u64::from(EXTRACTION_COOLDOWN) + 1,
                Some(_) => now + 1,
                // A belt funnel looks at the belt's way each tick (Create's mode is read each
                // tick): the belt reversed or started, it may push.
                None if f.belt.is_some() => now + 1,
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
    max: (u32, bool),
) -> Option<Option<bool>> {
    let (at, v) = create_engine::funnel::output(pos, facing);
    // An item still in the way: wait (`lastObserved`, `getEntityOverflowScanningArea`: the
    // funnel's own block, and the block below for any but a floor funnel).
    let low = if facing == Dir::Up { 0.0 } else { 1.0 };
    let near = items::items_in(
        lv,
        [
            f64::from(pos[0]),
            f64::from(pos[1]) - low,
            f64::from(pos[2]),
        ],
        pos.map(|v| f64::from(v) + 1.0),
    );
    if !near.is_empty() {
        return Some(Some(false));
    }
    let Some(stack) = take_filtered(lv, pos, behind, side, max) else {
        return Some(None);
    };
    let moved = stack.clone();
    items::drop_stack(lv, at, v, stack, 0);
    super::flaps::funnel(lv, pos, false);
    crate::glue::redstone::detectors::funnel_transfer(lv, pos, &moved);
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
    max: (u32, bool),
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
    let Some(stack) = take_filtered(lv, pos, behind, side, max) else {
        return Some(None);
    };
    let count = stack.count().min(64) as u8;
    let moved = stack.clone();
    let (id, progress) = super::belt::belt_name(&*lv.host, &stack);
    let mut item =
        create_engine::belt::Item::new(&id, count, random_angle(lv, pos), upright(&*lv.host, &id));
    item.progress = progress;
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
    super::flaps::funnel(lv, pos, false);
    crate::glue::redstone::detectors::funnel_transfer(lv, pos, &moved);
    Some(Some(true))
}

thread_local! {
    /// Item entities inside a funnel this tick (`FunnelBlock.entityInside`): level, funnel and
    /// entity, taken at the level's tick, where the entity can be changed.
    static INSIDE: std::cell::RefCell<Vec<(usize, [i32; 3], u32)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// An item entity `entity` is inside the funnel at `pos` of level `dim`.
pub fn item_inside(dim: usize, pos: [i32; 3], entity: u32) {
    INSIDE.with_borrow_mut(|v| {
        if !v.contains(&(dim, pos, entity)) {
            v.push((dim, pos, entity));
        }
    });
}

/// `FunnelBlock.entityInside` for the items inside funnels this tick: an inserting, unpowered
/// funnel takes an item past its opening (a quarter block in from its centre, toward its back)
/// into the inventory behind it, as much as fits and its filter passes.
pub fn collect_inside(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mine: Vec<([i32; 3], u32)> = INSIDE.with_borrow_mut(|v| {
        let mine = v
            .iter()
            .filter(|e| e.0 == dim)
            .map(|e| (e.1, e.2))
            .collect();
        v.retain(|e| e.0 != dim);
        mine
    });
    for (pos, id) in mine {
        let Some(f) = funnel_at(lv, pos) else {
            continue;
        };
        if f.belt.is_some() || f.powered || f.extracting {
            continue;
        }
        let Some(e) = lv.host.entity(id) else {
            continue;
        };
        let Some(item) = lv.host.item_entity(e) else {
            continue;
        };
        let (Some(stack), Some(at)) = (item.stack(&*lv.host), e.position(&*lv.host)) else {
            continue;
        };
        if !create_engine::funnel::past_opening(pos, f.facing, at) {
            continue;
        }
        if !funnel_passes(lv, pos, &stack) {
            continue;
        }
        let behind = create_engine::funnel::behind(pos, f.facing);
        let count = stack.count();
        let moved = stack.clone();
        let left = items::insert_item(lv, behind, face(f.facing), stack);
        match left {
            None => {
                let _ = e.discard(&mut *lv.host);
            }
            Some(l) if l.count() < count => {
                let _ = item.set_stack(&mut *lv.host, l);
            }
            _ => continue,
        }
        super::flaps::funnel(lv, pos, true);
        crate::glue::redstone::detectors::funnel_transfer(lv, pos, &moved);
    }
}
