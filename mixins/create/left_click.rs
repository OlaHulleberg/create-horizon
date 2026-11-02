// A left click on a block: a placard gives its item back, a clipboard is collected.
fn left_click_block(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    state: horizon_world::registry::BlockStateId,
    canceled: &mut bool,
) {
    crate::game::create_decor::left_click_block(game, pos, state, canceled);
}
