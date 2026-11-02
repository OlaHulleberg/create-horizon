// A funnel or another machine taking up to `max` items of one kind from the container at `pos`
// through its side `face` (`IItemHandler.extractItem`): from the first slot a hopper could take
// from through that face, up to its whole stack. None when there is no container or nothing to
// take. Reaches the hopper's own container rules beside them in `machines.rs`.
pub(crate) fn extract_item(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    face: horizon_core::geometry::Direction,
    max: u32,
) -> Option<horizon_client::items::ItemStack> {
    let view = game.containers().view((pos[0], pos[1], pos[2]), false)?;
    for slot in view.slots_for_face(Some(face)) {
        let c = game.containers();
        let Some(stack) = c.get(&view, slot) else {
            continue;
        };
        if !c.can_take(&view, slot, stack, face) {
            continue;
        }
        let current = game.slot_mut_at(&view, slot)?;
        let Some(held) = current.as_mut() else {
            continue;
        };
        let n = held.count.min(max);
        if n == 0 {
            continue;
        }
        let out = horizon_client::items::ItemStack {
            count: n,
            ..held.clone()
        };
        held.count -= n;
        if held.count == 0 {
            *current = None;
        }
        game.view_changed(&view);
        return Some(out);
    }
    None
}

/// A stack compared as Create compares stacks (`isSameItemSameComponents`).
struct Key<'a>(&'a horizon_client::items::ItemStack);

impl PartialEq for Key<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0.item == other.0.item
            && self.0.damage == other.0.damage
            && self.0.extra == other.0.extra
    }
}

// A chute taking up to `amount` of the first item `test` passes from the container at `pos`
// through its side `face` (`ItemHelper.extract`, up to): gathered from the slots a hopper could
// take from through that face, in order. None when there is no container or nothing to take.
pub(crate) fn extract_matching(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    face: horizon_core::geometry::Direction,
    test: impl Fn(&crate::game::Game<'_>, &horizon_client::items::ItemStack) -> bool,
    amount: u32,
) -> Option<horizon_client::items::ItemStack> {
    let view = game.containers().view((pos[0], pos[1], pos[2]), false)?;
    let slots = view.slots_for_face(Some(face));
    let (plan, mut out) = {
        let c = game.containers();
        let items = game.items();
        let keys: Vec<Option<(Key<'_>, u32, u32)>> = slots
            .iter()
            .map(|&slot| {
                let stack = c.get(&view, slot)?;
                if !c.can_take(&view, slot, stack, face) || !test(game, stack) {
                    return None;
                }
                let max = items.get(&stack.item).map_or(64, |i| i.max_stack).max(1);
                Some((Key(stack), stack.count, max))
            })
            .collect();
        let plan = create_engine::chute::extract(&keys, |_| true, false, amount);
        let &(first, _) = plan.first()?;
        let out = keys[first].as_ref()?.0.0.clone();
        (plan, out)
    };
    out.count = 0;
    for (i, n) in plan {
        let current = game.slot_mut_at(&view, slots[i])?;
        let Some(held) = current.as_mut() else {
            continue;
        };
        held.count -= n;
        out.count += n;
        if held.count == 0 {
            *current = None;
        }
    }
    game.view_changed(&view);
    Some(out)
}

// How much of `stack` the container at `pos` would take through its side `face`, as a hopper
// puts in (`insertItem` simulated: `IItemHandler`'s `simulate`), without changing it. None when
// there is no container there.
pub(crate) fn insert_room(
    game: &crate::game::Game<'_>,
    pos: [i32; 3],
    face: horizon_core::geometry::Direction,
    stack: &horizon_client::items::ItemStack,
) -> Option<u32> {
    let view = game.containers().view((pos[0], pos[1], pos[2]), false)?;
    if let super::Kind::Composter(_) = view.kind {
        return Some(0);
    }
    let c = game.containers();
    let slots = if matches!(
        view.kind,
        super::Kind::Furnace | super::Kind::Brewing | super::Kind::Mod
    ) {
        view.slots_for_face(Some(face))
    } else {
        (0..view.len()).collect()
    };
    let max = c.max_stack(stack);
    let mut left = stack.count;
    for slot in slots {
        if left == 0 {
            break;
        }
        if !c.can_place(&view, slot, stack) {
            continue;
        }
        match c.get(&view, slot) {
            // An empty slot takes the whole stack (`tryMoveInItem`).
            None => left = 0,
            Some(current) if current.count <= max && Key(current) == Key(stack) => {
                left -= left.min(max.saturating_sub(current.count));
            }
            Some(_) => {}
        }
    }
    Some(stack.count - left)
}

// Whether the block at `pos` is a container a machine can reach (an item handler): what Create's
// smart observer and threshold switch turn to when placed.
pub(crate) fn is_container(game: &crate::game::Game<'_>, pos: [i32; 3]) -> bool {
    game.containers()
        .view((pos[0], pos[1], pos[2]), false)
        .is_some()
}

// Every slot of the container at `pos`, whatever its sides (`bypassSidedness`): each one's item,
// count and largest stack there (`min(maxStackSize, getSlotLimit)`). None when there is none.
pub(crate) fn container_slots(
    game: &crate::game::Game<'_>,
    pos: [i32; 3],
) -> Option<Vec<create_engine::observe::Slot>> {
    let view = game.containers().view((pos[0], pos[1], pos[2]), false)?;
    let c = game.containers();
    Some(
        (0..view.len())
            .map(|slot| match c.get(&view, slot) {
                Some(s) => create_engine::observe::Slot {
                    item: Some(s.item.to_string()),
                    count: s.count,
                    max_stack: c.max_stack(s),
                },
                None => create_engine::observe::Slot {
                    item: None,
                    count: 0,
                    max_stack: 64,
                },
            })
            .collect(),
    )
}

// The slots of the container at `pos` a machine reaches through its side `face`, in order: each
// slot's index and its stack when a hopper could take it from there (`extractItem` simulated),
// for Create's packager. None when there is no container.
pub(crate) fn takeable_slots(
    game: &crate::game::Game<'_>,
    pos: [i32; 3],
    face: horizon_core::geometry::Direction,
) -> Option<Vec<(usize, Option<horizon_client::items::ItemStack>)>> {
    let view = game.containers().view((pos[0], pos[1], pos[2]), false)?;
    let c = game.containers();
    Some(
        view.slots_for_face(Some(face))
            .into_iter()
            .map(|slot| {
                let stack = c
                    .get(&view, slot)
                    .filter(|s| c.can_take(&view, slot, s, face))
                    .cloned();
                (slot, stack)
            })
            .collect(),
    )
}

// Up to `n` from slot `slot` of the container at `pos` (`extractItem(slot, n)`).
pub(crate) fn take_from_slot(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    slot: usize,
    n: u32,
) -> Option<horizon_client::items::ItemStack> {
    let view = game.containers().view((pos[0], pos[1], pos[2]), false)?;
    let current = game.slot_mut_at(&view, slot)?;
    let held = current.as_mut()?;
    let n = held.count.min(n);
    if n == 0 {
        return None;
    }
    let out = horizon_client::items::ItemStack {
        count: n,
        ..held.clone()
    };
    held.count -= n;
    if held.count == 0 {
        *current = None;
    }
    game.view_changed(&view);
    Some(out)
}

// Every slot of the container at `pos` with its stack, for a package's contents to be fitted
// into (`DefaultUnpackingHandler`'s simulation). None when there is no container.
pub(crate) fn all_slots(
    game: &crate::game::Game<'_>,
    pos: [i32; 3],
) -> Option<Vec<Option<horizon_client::items::ItemStack>>> {
    let view = game.containers().view((pos[0], pos[1], pos[2]), false)?;
    let c = game.containers();
    Some(
        (0..view.len())
            .map(|slot| c.get(&view, slot).cloned())
            .collect(),
    )
}
