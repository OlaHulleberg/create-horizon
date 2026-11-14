//! Chutes, smart chutes and item vaults on the server: stacks falling through chutes into what
//! lies below (`ChuteBlockEntity.tick`), empty chutes taking from what is above
//! (`handleInputFromAbove`), a smart chute's filter slot, and vaults' shapes.

use super::super::server::{ServerLevel, UPDATE_CLIENTS};
use super::super::world::with_world;
use super::filter;
use super::items;
use super::{info_at, mark_touched};
use create_engine::Block;
use create_engine::kinetics::Dir;
use mod_api::{BlockClick, Direction, Hand, ItemStack, SoundSource};

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
                    filter::passes(&*lv.host, w.dims[dim].chute_filters.get(&pos), stack)
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

/// Chutes holding a stack (`ChuteBlockEntity.tick` falling, `handleDownwardOutput`): into the
/// container below (`insertItemStacked`; a vault's whole inventory), else the chute below if it
/// is empty (and a smart chute's filter passes it), else out of the bottom when nothing solid is
/// under it; it waits in the middle while the way is shut, or while a smart chute is powered
/// (`canActivate`). Then the empty chutes that may take from the inventory above them do.
pub fn tick_chutes(lv: &mut ServerLevel<'_>) {
    use create_engine::chute::{Fall, fall};
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
        let below = [pos[0], pos[1] - 1, pos[2]];
        let below_chute = chute_at(lv, below);
        let container =
            below_chute.is_none() && items::items_at(lv, below, Some(Direction::Up)).is_some();
        let can = if !active {
            false
        } else if let Some((b, powered)) = below_chute {
            chute_stack(lv, below).is_none() && chute_accepts(lv, below, b, powered, &stack)
        } else if container {
            true
        } else {
            falls_through(lv, below)
        };
        let next = match fall(position, can) {
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
        let above = [pos[0], pos[1] + 1, pos[2]];
        // `grabCapability(UP)`: a chute above hands its items down itself.
        if chute_at(lv, above).is_some() {
            continue;
        }
        let Some(from) = items::items_at(lv, above, Some(Direction::Down)) else {
            continue;
        };
        // Create's own containers say when they change; others are looked at again later.
        let tells = matches!(
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
        let taken = items::extract_matching(
            lv,
            &from,
            |host, r| filter::passes(&*host, filter.as_ref(), r.as_stack()),
            amount,
        );
        match taken {
            Some(stack) => {
                set_chute_stack(lv, pos, Some(stack));
                with_world(|w| w.dims[dim].chutes.insert(pos, 1.0));
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
    let creative = p.is_creative(&*lv.host).unwrap_or(false);
    // `FilteringBehaviour.onShortInteract`'s hand: what the slot is set to (None: emptied), a
    // filter item taken from the hand outside creative.
    let filter = click.stack.map(|s| filter::filter_of(&*lv.host, s));
    if let (Some(f), Some(held)) = (&filter, click.stack)
        && f.is_filter_item()
        && !creative
    {
        let rest = held.clone().shrink(1);
        set_hand(lv, p, click.hand, rest);
    }
    let dim = lv.dim;
    let old = with_world(|w| w.set_chute_filter(dim, pos, filter)).flatten();
    // The filter the slot held before, given back when it was a filter item (in creative only
    // when the inventory has none like it), and the frame sound.
    if let Some(old) = old.filter(create_engine::filter::Filter::is_filter_item)
        && let Some(stack) = filter::filter_stack(&*lv.host, &old)
    {
        let has = p
            .inventory(&*lv.host)
            .unwrap_or_default()
            .iter()
            .any(|s| s.same_item(&stack) && s.same_components(&stack));
        if !creative || !has {
            items::give(&mut *lv.host, click.player, stack);
        }
    }
    let center = pos.map(|v| f64::from(v) + 0.5);
    lv.level.play_sound(
        &mut *lv.host,
        "minecraft:entity.item_frame.add_item",
        center,
        SoundSource::Blocks,
        0.25,
        0.1,
    );
    true
}

/// Sets the stack in `player`'s `hand`.
pub fn set_hand(
    lv: &mut ServerLevel<'_>,
    player: mod_api::PlayerRef,
    hand: Hand,
    stack: Option<ItemStack>,
) {
    let slot = match hand {
        Hand::Main => player.selected_slot(&*lv.host).map(|s| s as u32),
        // `Inventory.SLOT_OFFHAND`.
        Hand::Off => Some(40),
    };
    if let Some(slot) = slot {
        let _ = player.set_inventory_slot(&mut *lv.host, slot, stack);
    }
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
