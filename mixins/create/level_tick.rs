// After a server level's tick: Create re-solves what changed there.
fn level_tick_post(game: &mut crate::game::Game<'_>, dimension: &str) {
    crate::game::create::level_tick_post(game, dimension);
}
