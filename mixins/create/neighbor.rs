// A neighbour changed: Create's split shafts check their redstone power (hot: one range test).
fn neighbor_changed(
    dimension: horizon_world::level::Dimension,
    pos: horizon_core::position::BlockPos,
    state: horizon_world::registry::BlockStateId,
) {
    crate::game::create::neighbor_changed(dimension, pos, state);
}
