// The world is being saved (NeoForge's `LevelEvent.Save`): Create's logistics networks go to the
// world's saved data when they changed.
fn level_save(game: &mut crate::game::Game<'_>, dimension: &str) {
    crate::game::create_stock::level_save(game, dimension);
}
