// A hopper gives to a block without a container: the creative crate voids it.
fn item_handler_insert(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    stack: &mut Option<horizon_client::items::ItemStack>,
    handled: &mut bool,
) {
    crate::game::create_decor::item_handler_insert(game, pos, stack, handled);
}
