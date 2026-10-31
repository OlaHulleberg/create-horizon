// Create's commands, added to every player's tree.
fn register_commands(commands: &mut Vec<crate::game::native_server::NativeCommand>) {
    crate::game::create::register_commands(commands);
}
