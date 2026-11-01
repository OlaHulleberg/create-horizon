// The end of a player's server tick: Create's diving helmet and extendo grip.
fn player_tick(game: &mut crate::game::Game<'_>) {
    crate::game::create_equipment::player_tick(game);
}
