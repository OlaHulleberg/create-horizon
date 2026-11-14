// The mouse on Create's own screens: the clipboard's buttons, checkboxes and text.
fn mod_screen_mouse(
    game: &mut crate::game::Game<'_>,
    name: &str,
    at: [f32; 2],
    button: u8,
    pressed: bool,
) {
    crate::game::create_clipboard::mouse(game, name, at, button, pressed);
}
