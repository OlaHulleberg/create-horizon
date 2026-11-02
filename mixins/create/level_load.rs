// The world opened on its server (NeoForge's `LevelEvent.Load`): Create's logistics networks
// come back from the world's saved data.
fn level_load(game: &mut crate::game::Game<'_>, dimension: &str) {
    crate::game::create_stock::level_load(game, dimension);
}
