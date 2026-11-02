// The world is being saved (NeoForge's `LevelEvent.Save`): Create's logistics networks, minecart
// contraptions and couplings go to the world's saved data when they changed.
fn level_save(game: &mut crate::game::Game<'_>, dimension: &str) {
    crate::game::create_stock::level_save(game, dimension);
    crate::game::create_carts::level_save(game, dimension);
}
