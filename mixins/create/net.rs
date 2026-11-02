// A player's message to Create's server half (a client asking for the parts it shows).
fn net_from_client(game: &mut crate::game::Game<'_>, player: u128, channel: &str, data: &[u8]) {
    crate::game::create::net_from_client(game, player, channel, data);
    crate::game::create_packages::net_from_client(game, player, channel, data);
    crate::game::create_schematics::net_from_client(game, player, channel, data);
}
