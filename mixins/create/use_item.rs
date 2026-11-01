// An item used: the use that follows a click Create's blocks took is theirs too; Create's own
// items' uses (the nugget of experience).
fn use_item(
    game: &mut crate::game::Game<'_>,
    stack: &horizon_client::items::ItemStack,
    claimed: &mut bool,
) {
    crate::game::create::use_item(game, stack, claimed);
}
