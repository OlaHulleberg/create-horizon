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
