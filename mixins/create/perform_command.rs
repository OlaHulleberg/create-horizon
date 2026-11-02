// A player's command: Create answers its own (the package logistics' first).
fn perform_command(game: &mut crate::game::Game<'_>, command: &str, ran: &mut bool) {
    let mut ours = false;
    crate::game::create_packages::perform_command(game, command, &mut ours);
    if ours {
        *ran = true;
    } else {
        crate::game::create::perform_command(game, command, ran);
    }
}
