// The world opened on its server (NeoForge's `LevelEvent.Load`): Create's logistics networks,
// minecart contraptions and couplings come back from the world's saved data.
fn level_load(game: &mut crate::game::Game<'_>, dimension: &str) {
    crate::game::create_stock::level_load(game, dimension);
    crate::game::create_carts::level_load(game, dimension);
}
