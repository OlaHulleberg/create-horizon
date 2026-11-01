// A block placed from an item: Create's directional blocks take the facing Create gives them.
fn place_block(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    state: horizon_world::registry::BlockStateId,
) {
    crate::game::create::place_block(game, pos, state);
}
