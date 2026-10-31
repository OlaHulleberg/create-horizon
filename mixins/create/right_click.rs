// A right click on a block: Create's hand crank turns, a depot takes or gives its stacks.
fn right_click_block(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    crate::game::create::right_click_block(game, pos, face, canceled);
}
