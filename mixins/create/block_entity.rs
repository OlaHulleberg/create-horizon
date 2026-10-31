// A block gets a block entity: Create's machines with slots (the millstone).
fn block_entity(block: &str, entity: &mut Option<(String, horizon_world::nbt::Compound)>) {
    crate::game::create::block_entity(block, entity);
}
