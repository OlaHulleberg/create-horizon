// A player's left click on a block: a placard gives its item back, a clipboard is collected;
// the worldshaper picks the block it looks at; a cardboard sword bonks.
fn left_click_block(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    state: horizon_world::registry::BlockStateId,
    canceled: &mut bool,
) {
    crate::game::create_decor::left_click_block(game, pos, state, canceled);
    if !*canceled {
        crate::game::create_items::left_click_block(game, pos, state, canceled);
    }
}
