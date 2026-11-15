//! The millstone (`MillstoneBlockEntity`): its slots are input 0 and outputs 1 to 9.

use super::{recipes, stacks};
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::processing;
use mod_api::{ItemStack, PlayerRef};

/// The millstones that turn: `MillstoneBlockEntity.tick` on their slots. Millstones that don't
/// turn cost nothing.
pub fn tick_millstones(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let turning = with_world(|w| w.turning_millstones(dim)).unwrap_or_default();
    if turning.is_empty() || !recipes::ready() {
        return;
    }
    for (pos, speed) in turning {
        let Some(mut state) = with_world(|w| w.dims[dim].millstones.get(&pos).copied()).flatten()
        else {
            continue;
        };
        let Some(slots) = lv.slots(pos, 10) else {
            continue;
        };
        let host = &*lv.host;
        let input = slots[0].as_ref().map(|s| stacks::id(host, s));
        let outputs: Vec<processing::Slot> = slots[1..10]
            .iter()
            .map(|s| stacks::slot_of(host, s))
            .collect();
        let outputs_full = outputs
            .iter()
            .flatten()
            .any(|(i, n)| *n >= stacks::limit(host, i));
        let seed = lv.seed_at(pos);
        let done = recipes::with(|r| {
            let done = processing::tick_millstone(
                &mut state,
                speed,
                input.as_deref(),
                outputs_full,
                &r.milling,
            )?;
            Some(r.milling.get(done).clone())
        })
        .flatten();
        with_world(|w| {
            if let Some(m) = w.dims[dim].millstones.get_mut(&pos) {
                *m = state;
            }
        });
        let Some(recipe) = done else {
            continue;
        };
        // process(): one input, the rolled results stacked into the outputs.
        let input_left = slots[0].clone().and_then(|s| s.shrink(1));
        let mut outputs = outputs;
        let mut rng = processing::Rng::new(seed);
        for (item, n) in processing::roll(&recipe, &mut rng) {
            let limit = stacks::limit(&*lv.host, &item);
            processing::insert_stacked(&mut outputs, &item, n, limit);
        }
        // The input's crafting remainder goes into the outputs too.
        if let Some(left) = input
            .as_deref()
            .and_then(|i| stacks::remainder(&*lv.host, i))
        {
            let limit = stacks::limit(&*lv.host, &left);
            processing::insert_stacked(&mut outputs, &left, 1, limit);
        }
        let mut changes: Vec<(usize, Option<ItemStack>)> = vec![(0, input_left)];
        for (i, s) in outputs.iter().enumerate() {
            if stacks::slot_of(&*lv.host, &slots[i + 1]) != *s {
                changes.push((i + 1, stacks::from_slot(&*lv.host, s)));
            }
        }
        lv.set_slots(pos, &changes);
        crate::glue::advancements::award_placer(lv, pos, "millstone");
    }
}

/// `MillstoneBlock.updateEntityAfterFallOn`: an item landing on a millstone goes into its input
/// as far as it fits, when it has a milling recipe (`MillstoneInventoryHandler.insertItem`:
/// `canProcess`). `stack` keeps what is left.
pub fn millstone_take(lv: &mut ServerLevel<'_>, pos: [i32; 3], stack: &mut Option<ItemStack>) {
    let Some(s) = stack.as_ref() else {
        return;
    };
    let host = &*lv.host;
    let item = stacks::id(host, s);
    if recipes::with(|r| r.milling.find_single(&item))
        .flatten()
        .is_none()
    {
        return;
    }
    let max = stacks::limit(host, &item);
    let Some(before) = lv.read_slots(pos, 0, 1) else {
        return;
    };
    let mut input = before.clone();
    let count = s.count().min(255) as u8;
    let left = processing::insert_stacked(&mut input, &item, count, max);
    if left == count {
        return;
    }
    let Some(put) = stacks::from_slot(&*lv.host, &input[0]) else {
        return;
    };
    if lv.set_slots(pos, &[(0, Some(put))]) {
        *stack = stack.take().and_then(|s| s.shrink(u32::from(count - left)));
    }
}

/// `MillstoneBlock.useItemOn` with an empty hand: the outputs go to the player (the input when
/// there are none); an item passes to its own use (a block placed against it). True when the
/// click was the millstone's.
pub fn use_millstone(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: PlayerRef,
    stack: Option<&ItemStack>,
) -> bool {
    if stack.is_some() {
        return false;
    }
    let Some(slots) = lv.slots(pos, 10) else {
        return true;
    };
    let outputs: Vec<(usize, ItemStack)> = slots
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(i, s)| Some((i, s.clone()?)))
        .collect();
    let taken = if outputs.is_empty() {
        slots[0].clone().map(|s| (0, s)).into_iter().collect()
    } else {
        outputs
    };
    let cleared: Vec<(usize, Option<ItemStack>)> = taken.iter().map(|(i, _)| (*i, None)).collect();
    if lv.set_slots(pos, &cleared) {
        for (_, s) in taken {
            lv.give(player, s);
        }
    }
    true
}
