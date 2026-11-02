// Create's commands, added to every player's tree.
fn register_commands(commands: &mut Vec<crate::game::native_server::NativeCommand>) {
    crate::game::create::register_commands(commands);
    crate::game::create_packages::register_commands(commands);
    crate::game::create_stock::register_commands(commands);
    crate::game::create_schematics::register_commands(commands);
    crate::game::create_items::register_commands(commands);
    crate::game::create_controller::register_commands(commands);
}
