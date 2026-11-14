// An item used: the use that follows a click Create's blocks took is theirs too; Create's own
// items' uses (the nugget of experience, the wand of symmetry, the worldshaper, the linked
// controller, the clipboard's pages).
fn use_item(
    game: &mut crate::game::Game<'_>,
    stack: &horizon_client::items::ItemStack,
    claimed: &mut bool,
) {
    crate::game::create::use_item(game, stack, claimed);
    if !*claimed {
        crate::game::create_items::use_item(game, stack, claimed);
    }
    if !*claimed {
        crate::game::create_controller::use_item(game, stack);
    }
    crate::game::create_clipboard::use_item(game, stack, claimed);
}
