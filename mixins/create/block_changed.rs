// The hot hook: Create's glue tests the states first and returns for any other block.
fn block_changed(
    dimension: horizon_world::level::Dimension,
    pos: horizon_core::position::BlockPos,
    old: horizon_world::registry::BlockStateId,
    new: horizon_world::registry::BlockStateId,
) {
    crate::game::create::block_changed(dimension, pos, old, new);
}
