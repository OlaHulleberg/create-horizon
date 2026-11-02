// A block placed from an item: Create's directional blocks take the facing Create gives them, a
// backtank the stack's air; a held extendo grip wears; wands of symmetry in the hotbar copy it.
fn place_block(
    game: &mut crate::game::Game<'_>,
    stack: &horizon_client::items::ItemStack,
    pos: [i32; 3],
    state: horizon_world::registry::BlockStateId,
) {
    crate::game::create::place_block(game, pos, state);
    crate::game::create_schematics::place_block(game, pos, state);
    crate::game::create_equipment::place_block(game, stack, pos, state);
    crate::game::create_decor::place_block(game, stack, pos, state);
    crate::game::create_items::place_block(game, pos, state);
}
