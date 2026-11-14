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
        let mut changes: Vec<(usize, Option<ItemStack>)> = vec![(0, input_left)];
        for (i, s) in outputs.iter().enumerate() {
            if stacks::slot_of(&*lv.host, &slots[i + 1]) != *s {
                changes.push((i + 1, stacks::from_slot(&*lv.host, s)));
            }
        }
        lv.set_slots(pos, &changes);
    }
}

/// `MillstoneBlock.useItemOn` with an empty hand: the outputs go to the player (the input when
/// there are none); with an item, the block's own use (closed: no menu opens). True when the
/// click was the millstone's.
pub fn use_millstone(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: PlayerRef,
    stack: Option<&ItemStack>,
) -> bool {
    if stack.is_some() {
        return true;
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
