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
    test: impl Fn(&horizon_client::items::ItemStack) -> bool,
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
                if !c.can_take(&view, slot, stack, face) || !test(stack) {
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
