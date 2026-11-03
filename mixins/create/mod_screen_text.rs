// Text typed on Create's own screens: into the clipboard's entry being edited.
fn mod_screen_text(game: &mut crate::game::Game<'_>, name: &str, text: &str) {
    crate::game::create_clipboard::text(game, name, text);
}
