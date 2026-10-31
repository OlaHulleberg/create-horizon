// An item used on a block: Create's wrench turns or picks up Create's blocks.
fn use_item_on(
    game: &mut crate::game::Game<'_>,
    stack: &horizon_client::items::ItemStack,
    pos: [i32; 3],
    face: [i32; 3],
    claimed: &mut bool,
) {
    crate::game::create::use_item_on(game, stack, pos, face, claimed);
}
