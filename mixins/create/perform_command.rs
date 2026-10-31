// A player's command: Create answers its own.
fn perform_command(game: &mut crate::game::Game<'_>, command: &str, ran: &mut bool) {
    crate::game::create::perform_command(game, command, ran);
}
