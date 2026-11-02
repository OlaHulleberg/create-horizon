// Each tick of every Game: the client's part bookkeeping and diving boots (the server's Game
// returns at once).
fn tick(game: &mut crate::game::Game<'_>) {
    crate::game::create::client_tick(game);
    crate::game::create_equipment::client_tick(game);
    crate::game::create_schematics::client_tick(game);
}
