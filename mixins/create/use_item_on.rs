// An item used on a block: Create's wrench turns or picks up Create's blocks; the wand of
// symmetry moves its mirror; tree fertilizer grows a sapling; a linked controller binds to a link
// or goes on a lectern; a copycat's item notes where it was clicked, for its placement; a
// clipboard opens its pages.
fn use_item_on(
    game: &mut crate::game::Game<'_>,
    stack: &horizon_client::items::ItemStack,
    pos: [i32; 3],
    face: [i32; 3],
    claimed: &mut bool,
) {
    crate::game::create_copycat::use_item_on(game, stack, pos, face);
    crate::game::create::use_item_on(game, stack, pos, face, claimed);
    if !*claimed {
        crate::game::create_items::use_item_on(game, stack, pos, face, claimed);
    }
    if !*claimed {
        crate::game::create_controller::use_item_on(game, stack, pos, claimed);
    }
    crate::game::create_clipboard::use_item_on(game, stack, claimed);
}
