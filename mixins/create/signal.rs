// A block's redstone signal: Create's latches, brass diodes, rose quartz lamps and redstone links
// answer from their state (hot: one range test).
fn block_signal(
    dimension: horizon_world::level::Dimension,
    pos: horizon_core::position::BlockPos,
    state: horizon_world::registry::BlockStateId,
    face: horizon_core::geometry::Direction,
    direct: bool,
    signal: &mut i32,
) {
    crate::game::create::block_signal(dimension, pos, state, face, direct, signal);
}
