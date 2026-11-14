// A player's message to Create's server half (a client asking for the parts it shows, a linked
// controller's buttons, a click on a train's carriage and its controls' keys).
fn net_from_client(game: &mut crate::game::Game<'_>, player: u128, channel: &str, data: &[u8]) {
    crate::game::create::net_from_client(game, player, channel, data);
    crate::game::create_packages::net_from_client(game, player, channel, data);
    crate::game::create_schematics::net_from_client(game, player, channel, data);
    crate::game::create_controller::net_from_client(game, player, channel, data);
    crate::game::create_copycat::net_from_client(game, player, channel, data);
    crate::game::create_train_controls::net_from_client(game, player, channel, data);
    crate::game::create_clipboard::net_from_client(game, player, channel, data);
}
