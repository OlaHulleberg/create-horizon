// A hopper takes from a block without a container: the creative crate's supply.
fn item_handler_extract(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    amount: u32,
    taken: &mut Option<horizon_client::items::ItemStack>,
    handled: &mut bool,
) {
    crate::game::create_decor::item_handler_extract(game, pos, amount, taken, handled);
}
