// The HUD: Create's goggles show what they look at.
fn hud(game: &mut crate::game::Game<'_>, batch: &mut horizon_client::hud::HudBatch) {
    crate::game::create::hud(game, batch);
}
