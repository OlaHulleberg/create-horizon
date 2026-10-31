// A right click on a block: Create's hand crank turns.
fn right_click_block(game: &mut crate::game::Game<'_>, pos: [i32; 3], canceled: &mut bool) {
    crate::game::create::right_click_block(game, pos, canceled);
}
