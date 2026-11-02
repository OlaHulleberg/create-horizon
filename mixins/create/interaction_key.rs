// An attack, use or pick-block click on the client (NeoForge's
// `InputEvent.InteractionKeyMappingTriggered`): a use click on a train's carriage is the
// carriage's.
fn interaction_key_mapping_triggered(
    game: &mut crate::game::Game<'_>,
    button: horizon_client::interaction_input::Button,
    canceled: &mut bool,
    swing: &mut bool,
) {
    crate::game::create_train_controls::interaction(game, button, canceled, swing);
}
