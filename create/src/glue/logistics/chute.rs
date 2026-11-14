//! Chutes, smart chutes and item vaults on the server: stacks falling through chutes into what
//! lies below (`ChuteBlockEntity.tick`), or blown up them by an encased fan, empty chutes taking
//! from what is above (`handleInputFromAbove`) or below while blown up, a smart chute's filter
//! slot, and vaults' shapes.

use super::super::server::{ServerLevel, UPDATE_CLIENTS};
use super::super::world::with_world;
use super::items;
use super::{info_at, mark_touched};
use crate::glue::contraptions::actors;
use crate::glue::items::filters;
use create_engine::Block;
use create_engine::kinetics::Dir;
use mod_api::{BlockClick, Direction, ItemStack};

/// The chute or smart chute at `pos`, with whether it is powered.
pub fn chute_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(Block, bool)> {
    let (b, i) = info_at(lv, pos)?;
    matches!(b, Block::Chute | Block::SmartChute).then_some((b, i.powered))
}

/// `ChuteBlockEntity.canAcceptItem` for the chute or smart chute `block` at `pos` (its state
/// `powered`): a smart chute also not while powered and only what its filter passes
/// (`SmartChuteBlockEntity.canAcceptItem`). Emptiness is the caller's.
pub fn chute_accepts(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    block: Block,
    powered: bool,
    stack: &ItemStack,
) -> bool {
    let dim = lv.dim;
    match block {
        Block::Chute => true,
        Block::SmartChute => {
            !powered
                && with_world(|w| {
                    filters::passes(&*lv.host, w.dims[dim].chute_filters.get(&pos), stack)
                })
                .unwrap_or(false)
        }
        _ => false,
    }
}

pub fn chute_stack(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<ItemStack> {
    items::slot(lv, pos, 0)
}

pub fn set_chute_stack(lv: &mut ServerLevel<'_>, pos: [i32; 3], stack: Option<ItemStack>) {
    items::set_slot(lv, pos, 0, stack);
    // Arms using the chute look again.
    mark_touched(lv.dim, pos);
}

/// `ChuteBlockEntity` taking an item that landed on it (`canAcceptItem`: only while empty; a
/// smart chute's filter and power): the whole stack, at the top. True when it took it.
pub fn chute_take(lv: &mut ServerLevel<'_>, pos: [i32; 3], stack: &ItemStack) -> bool {
    let Some((block, powered)) = chute_at(lv, pos) else {
        return false;
    };
    if !chute_accepts(lv, pos, block, powered, stack) || chute_stack(lv, pos).is_some() {
        return false;
    }
    if !items::set_slot(lv, pos, 0, Some(stack.clone())) {
        return false;
    }
    let dim = lv.dim;
    with_world(|w| w.dims[dim].chutes.insert(pos, 1.0));
    true
}

/// Whether nothing at `pos` stops what falls: no collision shape there
/// (`Block.canSupportRigidBlock`'s answer for a chute's output).
fn falls_through(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    let cell = [
        f64::from(pos[0]) + 1e-4,
        f64::from(pos[1]) + 1e-4,
        f64::from(pos[2]) + 1e-4,
        f64::from(pos[0]) + 1.0 - 1e-4,
        f64::from(pos[1]) + 1.0 - 1e-4,
        f64::from(pos[2]) + 1.0 - 1e-4,
    ];
    lv.level
        .collision_boxes(&*lv.host, cell)
        .is_none_or(|b| b.is_empty())
}

/// The speed of an encased fan at `pos` blowing `way` (`fan.getSpeed()`), if one is there.
fn fan_blowing(lv: &ServerLevel<'_>, pos: [i32; 3], way: Dir) -> Option<f32> {
    let (block, info) = info_at(lv, pos)?;
    if block != Block::EncasedFan || info.facing != Some(way) {
        return None;
    }
    let dim = lv.dim;
    with_world(|w| w.speed(dim, pos)).flatten()
}

/// `getItemMotion` of the chute at `pos`: its column's `push` (`calculatePush`: a fan under the
/// lowest chute blowing up) and `pull` (`calculatePull`: a fan over the highest one blowing
/// down), in chutes a tick.
pub fn chute_motion(lv: &ServerLevel<'_>, pos: [i32; 3]) -> f32 {
    let end = |step: i32| {
        let mut p = [pos[0], pos[1] + step, pos[2]];
        while chute_at(lv, p).is_some() {
            p[1] += step;
        }
        p
    };
    let push = fan_blowing(lv, end(-1), Dir::Up).unwrap_or(0.0);
    let pull = fan_blowing(lv, end(1), Dir::Down).unwrap_or(0.0);
    create_engine::chute::item_motion(push, pull)
}

/// A stack the fans carry up the chute at `pos` (`tick` with a positive motion,
/// `handleUpwardOutput`): into the container above, else the chute above if it is empty and
/// does not fall (`getInputChutes`, `canAcceptItem`), else out of the top when nothing solid is
/// over it; it waits in the middle while the way is shut. The chute's next position, None when
/// the stack left.
fn rise(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    position: f32,
    motion: f32,
    stack: ItemStack,
    active: bool,
) -> Option<f32> {
    use create_engine::chute::{Fall, step};
    let dim = lv.dim;
    let above = [pos[0], pos[1] + 1, pos[2]];
    let above_chute = chute_at(lv, above);
    let container = above_chute.is_none()
        && (items::items_at(lv, above, Some(Direction::Down)).is_some()
            || actors::is_item_interface(lv, above));
    let can = if !active {
        false
    } else if let Some((b, powered)) = above_chute {
        chute_stack(lv, above).is_none()
            && chute_accepts(lv, above, b, powered, &stack)
            && chute_motion(lv, above) >= 0.0
    } else if container {
        true
    } else {
        let funnel_up = super::funnel::funnel_at(lv, above).is_some_and(|f| f.facing == Dir::Up);
        !funnel_up && falls_through(lv, above)
    };
    match step(position, motion, can) {
        Fall::At(p) => Some(p),
        Fall::Out if above_chute.is_some() => {
            set_chute_stack(lv, above, Some(stack));
            set_chute_stack(lv, pos, None);
            with_world(|w| w.dims[dim].chutes.insert(above, 0.0));
            None
        }
        Fall::Out if container => {
            let count = stack.count();
            let left = items::insert_item(lv, above, Direction::Down, stack);
            if left.as_ref().is_some_and(|l| l.count() == count) {
                Some(0.5)
            } else {
                let done = left.is_none();
                set_chute_stack(lv, pos, left);
                (!done).then_some(0.5)
            }
        }
        Fall::Out => {
            // Out of the top, half a block over the centre, at twice the chute's speed.
            let at = [
                f64::from(pos[0]) + 0.5,
                f64::from(pos[1]) + 1.0,
                f64::from(pos[2]) + 0.5,
            ];
            set_chute_stack(lv, pos, None);
            items::drop_stack(lv, at, [0.0, f64::from(motion) * 2.0, 0.0], stack, 0);
            None
        }
    }
}

/// Chutes holding a stack (`ChuteBlockEntity.tick` falling, `handleDownwardOutput`): into the
/// container below (`insertItemStacked`; a vault's whole inventory), else the chute below if it
/// is empty (and a smart chute's filter passes it), else out of the bottom when nothing solid is
/// under it; it waits in the middle while the way is shut, or while a smart chute is powered
/// (`canActivate`). Then the empty chutes that may take from the inventory above them do.
pub fn tick_chutes(lv: &mut ServerLevel<'_>) {
    use create_engine::chute::{Fall, step};
    let dim = lv.dim;
    let moving: Vec<([i32; 3], f32)> =
        with_world(|w| w.dims[dim].chutes.iter().map(|(p, v)| (*p, *v)).collect())
            .unwrap_or_default();
    for (pos, position) in moving {
        let Some(stack) = chute_stack(lv, pos) else {
            with_world(|w| {
                w.dims[dim].chutes.remove(&pos);
                // Emptied (a hopper below took it): it may take from above again.
                w.dims[dim].chute_pulls.wake(pos);
            });
            continue;
        };
        let active = !matches!(chute_at(lv, pos), Some((_, true)));
        if !active && position <= 0.5 {
            // A powered smart chute holds its stack in the middle; it ticks again once its
            // `powered` changes.
            with_world(|w| w.dims[dim].chutes.remove(&pos));
            continue;
        }
        let motion = chute_motion(lv, pos);
        if motion > 0.0 {
            let next = rise(lv, pos, position, motion, stack, active);
            with_world(|w| match next {
                Some(p) => {
                    w.dims[dim].chutes.insert(pos, p);
                }
                None => {
                    w.dims[dim].chutes.remove(&pos);
                    w.dims[dim].chute_pulls.wake(pos);
                }
            });
            continue;
        }
        let below = [pos[0], pos[1] - 1, pos[2]];
        let below_chute = chute_at(lv, below);
        let container = below_chute.is_none()
            && (items::items_at(lv, below, Some(Direction::Up)).is_some()
                || actors::is_item_interface(lv, below));
        let can = if !active {
            false
        } else if let Some((b, powered)) = below_chute {
            chute_stack(lv, below).is_none() && chute_accepts(lv, below, b, powered, &stack)
        } else if container {
            true
        } else {
            falls_through(lv, below)
        };
        let next = match step(position, motion, can) {
            Fall::At(p) => Some(p),
            Fall::Out if below_chute.is_some() => {
                set_chute_stack(lv, below, Some(stack));
                set_chute_stack(lv, pos, None);
                with_world(|w| w.dims[dim].chutes.insert(below, 1.0));
                None
            }
            Fall::Out if container => {
                let count = stack.count();
                let left = items::insert_item(lv, below, Direction::Up, stack);
                if left.as_ref().is_some_and(|l| l.count() == count) {
                    Some(0.5)
                } else {
                    let done = left.is_none();
                    set_chute_stack(lv, pos, left);
                    (!done).then_some(0.5)
                }
            }
            Fall::Out => {
                // An item 12/16 below the centre, falling at a quarter block a tick.
                let at = [
                    f64::from(pos[0]) + 0.5,
                    f64::from(pos[1]) + 0.5 - 0.75,
                    f64::from(pos[2]) + 0.5,
                ];
                set_chute_stack(lv, pos, None);
                items::drop_stack(lv, at, [0.0, -0.25, 0.0], stack, 0);
                None
            }
        };
        with_world(|w| match next {
            Some(p) => {
                w.dims[dim].chutes.insert(pos, p);
            }
            None => {
                w.dims[dim].chutes.remove(&pos);
                w.dims[dim].chute_pulls.wake(pos);
            }
        });
    }
    tick_chute_pulls(lv);
}

/// `ChuteBlockEntity.handleInputFromAbove` for the empty chutes due to look: up to 16 of the
/// first item of the inventory above (a smart chute: up to 64 of what its filter passes, and
/// nothing while powered) start falling from the top. One that finds nothing sleeps until a
/// container of Create's says it changed (`invVersionTracker`), or looks again `RETRY` ticks
/// later under another container; with no container above it waits for a neighbour change.
fn tick_chute_pulls(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let time = lv.game_time();
    let due = with_world(|w| w.dims[dim].chute_pulls.take_due(time)).unwrap_or_default();
    for pos in due {
        let Some((block, powered)) = chute_at(lv, pos) else {
            continue;
        };
        if powered || chute_stack(lv, pos).is_some() {
            // Powered: it looks again when that changes; holding a stack: when it is empty.
            continue;
        }
        // `handleInputFromAbove`, or `handleInputFromBelow` while the fans blow up.
        let rising = chute_motion(lv, pos) > 0.0;
        let (above, from_side, start) = if rising {
            ([pos[0], pos[1] - 1, pos[2]], Direction::Up, 0.0)
        } else {
            ([pos[0], pos[1] + 1, pos[2]], Direction::Down, 1.0)
        };
        // `grabCapability`: a chute there hands its items on itself.
        if chute_at(lv, above).is_some() {
            continue;
        }
        // A portable storage interface hands down its contraption's storage while connected;
        // it wakes the chute when it connects.
        let psi = actors::is_item_interface(lv, above);
        let from = items::items_at(lv, above, Some(from_side));
        if from.is_none() && !psi {
            continue;
        }
        // Create's own containers say when they change; others are looked at again later.
        let tells = psi
            || matches!(
                info_at(lv, above),
                Some((
                    Block::Depot | Block::WeightedEjector | Block::ItemVault | Block::Chute,
                    _
                )) | Some((Block::SmartChute, _))
            );
        let smart = block == Block::SmartChute;
        let filter = smart
            .then(|| with_world(|w| w.dims[dim].chute_filters.get(&pos).cloned()).flatten())
            .flatten();
        let amount = if smart {
            create_engine::chute::SMART_AMOUNT
        } else {
            create_engine::chute::CHUTE_AMOUNT
        };
        let taken = match from {
            _ if psi => {
                let host = &*lv.host;
                actors::psi_extract(
                    lv,
                    above,
                    |s| filters::passes(host, filter.as_ref(), s),
                    amount,
                )
                .ok()
                .flatten()
            }
            Some(from) => items::extract_matching(
                lv,
                &from,
                |host, r| filters::passes(&*host, filter.as_ref(), r.as_stack()),
                amount,
            ),
            None => None,
        };
        match taken {
            Some(stack) => {
                set_chute_stack(lv, pos, Some(stack));
                with_world(|w| w.dims[dim].chutes.insert(pos, start));
            }
            None => {
                with_world(|w| {
                    let p = &mut w.dims[dim].chute_pulls;
                    if tells {
                        p.sleep(pos);
                    } else {
                        p.retry(pos, time);
                    }
                });
            }
        }
    }
}

/// A player's click on a smart chute's filter slot (`FilteringBehaviour.onShortInteract` through
/// `ValueSettingsInputHandler`): on a side, not sneaking, at the slot 11 px up; the held item
/// becomes its filter (a filter item taken from the hand, the one it held given back), an empty
/// hand clears it (no counts yet). True when the click was the slot's.
pub fn use_smart_chute(lv: &mut ServerLevel<'_>, pos: [i32; 3], click: &BlockClick<'_>) -> bool {
    let Some(p) = lv.host.player(click.player) else {
        return false;
    };
    // `ValueSettingsInputHandler.canInteract`: may build, not sneaking, no clipboard in hand.
    let sneaking = p.is_secondary_use_active(&*lv.host).unwrap_or(false);
    let held_id = click.stack.map(|s| items::id_of(&*lv.host, s));
    if !p.may_build(&*lv.host).unwrap_or(false)
        || sneaking
        || held_id.as_deref() == Some("create:clipboard")
    {
        return false;
    }
    let Some(side) = Dir::from_offset(click.hit.face) else {
        return false;
    };
    let local = [0, 1, 2].map(|i| click.hit.point[i] - f64::from(pos[i]));
    let Some(slot) = create_engine::chute::filter_slot(side) else {
        return false;
    };
    if !create_engine::board::hits_box(slot, local) {
        return false;
    }
    // `canShortInteract`: a wrench or a mechanical arm is used on the block instead.
    if held_id
        .as_deref()
        .is_some_and(|h| matches!(h, "create:wrench" | "create:mechanical_arm"))
    {
        return false;
    }
    // `FilteringBehaviour.onShortInteract`: the hand's stack becomes the filter (a filter item
    // taken from it outside creative); the one it held goes back, with the frame sound.
    let filter = filters::take_filter(lv.host, p, click.hand);
    let dim = lv.dim;
    let shown = filter.clone();
    let old = with_world(|w| w.set_chute_filter(dim, pos, filter)).flatten();
    super::show_filter(lv, pos, shown.as_ref());
    filters::return_filter(lv, p, pos, old);
    true
}

/// Item vaults whose blocks changed form or split (`updateConnectivity`), and their blocks
/// show it (`large`). Nothing when no vault block changed.
pub fn tick_vaults(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let large = with_world(|w| w.tick_vaults(dim)).unwrap_or_default();
    for (pos, l) in large {
        if matches!(info_at(lv, pos), Some((Block::ItemVault, _))) {
            lv.set_property(
                pos,
                "large",
                if l { "true" } else { "false" },
                UPDATE_CLIENTS,
            );
        }
    }
}
