// The slots of Create's machines, for hoppers and comparators.
fn block_entity_container(
    block: &str,
    container: &mut Option<horizon_client::containers::ModContainer>,
) {
    crate::game::create::block_entity_container(block, container);
}
