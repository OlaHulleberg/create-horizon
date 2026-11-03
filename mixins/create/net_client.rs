// The server's speeds, shown as turning parts on the client.
fn net_from_server(game: &mut crate::game::Game<'_>, channel: &str, data: &[u8]) {
    crate::game::create::net_from_server(game, channel, data);
    crate::game::create_packages::net_from_server(game, channel, data);
    crate::game::create_schematics::net_from_server(game, channel, data);
    crate::game::create_controller::net_from_server(game, channel, data);
    crate::game::create_copycat::net_from_server(game, channel, data);
    crate::game::create_train_controls::net_from_server(game, channel, data);
    crate::game::create_clipboard::net_from_server(game, channel, data);
}
