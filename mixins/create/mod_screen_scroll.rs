// The wheel on Create's own screens: the clipboard's pages turn.
fn mod_screen_scroll(name: &str, delta: f32) {
    crate::game::create_clipboard::scroll(name, delta);
}
