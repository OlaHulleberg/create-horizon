// Stand-in for `mixins/create/clipboard.rs` in the template test: no screens open, and a
// clipboard's pages are neither read nor written.
use super::Game;
use horizon_client::items::ItemStack;

pub(super) fn open(game: &mut Game<'_>, pos: Option<[i32; 3]>, stack: &ItemStack) {
    let _ = (game, pos, stack);
}
pub(super) fn use_item(game: &mut Game<'_>, stack: &ItemStack, claimed: &mut bool) {
    let _ = (game, stack, claimed);
}
pub(super) fn use_item_on(game: &mut Game<'_>, stack: &ItemStack, claimed: &mut bool) {
    let _ = (game, stack, claimed);
}
pub(super) fn net_from_client(game: &mut Game<'_>, player: u128, channel: &str, data: &[u8]) {
    let _ = (game, player, channel, data);
}
pub(super) fn net_from_server(game: &mut Game<'_>, channel: &str, data: &[u8]) {
    let _ = (game, channel, data);
}
pub(super) fn draw(
    game: &mut Game<'_>,
    name: &str,
    batch: &mut horizon_client::hud::HudBatch,
    cursor: [f32; 2],
) {
    let _ = (game, name, batch, cursor);
}
pub(super) fn input_key(
    game: &mut Game<'_>,
    key: &str,
    action: horizon_client::debug_keys::KeyAction,
    modifiers: u8,
    screen: bool,
    consumed: &mut bool,
) {
    let _ = (game, key, action, modifiers, screen, consumed);
}
pub(super) fn text(game: &mut Game<'_>, name: &str, typed: &str) {
    let _ = (game, name, typed);
}
pub(super) fn mouse(game: &mut Game<'_>, name: &str, at: [f32; 2], button: u8, pressed: bool) {
    let _ = (game, name, at, button, pressed);
}
pub(super) fn scroll(name: &str, delta: f32) {
    let _ = (name, delta);
}
pub(super) fn removed(game: &mut Game<'_>, name: &str) {
    let _ = (game, name);
}
