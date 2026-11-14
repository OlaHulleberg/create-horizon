// A key in a world on the client (NeoForge's `InputEvent.Key`): a linked controller in use takes
// its buttons' keys, a train's controls in use theirs, the clipboard's screen its editing keys.
fn input_key(
    game: &mut crate::game::Game<'_>,
    key: &str,
    action: horizon_client::debug_keys::KeyAction,
    modifiers: u8,
    screen: bool,
    consumed: &mut bool,
) {
    crate::game::create_controller::input_key(key, action, screen, consumed);
    crate::game::create_train_controls::input_key(key, action, screen, consumed);
    crate::game::create_clipboard::input_key(game, key, action, modifiers, screen, consumed);
}
