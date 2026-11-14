// Create's own screens on the client (Horizon API 71): the clipboard's pages.
fn mod_screen_draw(
    game: &mut crate::game::Game<'_>,
    name: &str,
    batch: &mut horizon_client::hud::HudBatch,
    cursor: [f32; 2],
) {
    crate::game::create_clipboard::draw(game, name, batch, cursor);
}
