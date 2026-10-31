// An item used: the use that follows a click Create's blocks took is theirs too.
fn use_item(game: &mut crate::game::Game<'_>, claimed: &mut bool) {
    crate::game::create::use_item(game, claimed);
}
