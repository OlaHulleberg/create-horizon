// Each tick of every Game: the client's part bookkeeping (the server's Game returns at once).
fn tick(game: &mut crate::game::Game<'_>) {
    crate::game::create::client_tick(game);
}
