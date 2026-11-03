// One of Create's own screens closed: the clipboard's pages go to the server.
fn mod_screen_removed(game: &mut crate::game::Game<'_>, name: &str) {
    crate::game::create_clipboard::removed(game, name);
}
