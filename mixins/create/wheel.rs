// The mouse wheel: Create's scroll values (a creative motor's speed) take it over a block.
fn wheel(game: &mut crate::game::Game<'_>, steps: f32, claimed: &mut bool) {
    crate::game::create::wheel(game, steps, claimed);
}
