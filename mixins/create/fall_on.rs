// An item lies on a block: Create's belts and depots take it.
fn update_entity_after_fall_on(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    stack: &mut horizon_client::items::ItemStack,
    at: [f64; 3],
) {
    crate::game::create::item_landed(game, pos, stack, at);
}
