// A key in a world on the client (NeoForge's `InputEvent.Key`): a linked controller in use takes
// its buttons' keys, a train's controls in use theirs.
fn input_key(
    key: &str,
    action: horizon_client::debug_keys::KeyAction,
    screen: bool,
    consumed: &mut bool,
) {
    crate::game::create_controller::input_key(key, action, screen, consumed);
    crate::game::create_train_controls::input_key(key, action, screen, consumed);
}
