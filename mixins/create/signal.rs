// A block's redstone signal: Create's latches answer from their state (hot: one range test).
fn block_signal(
    state: horizon_world::registry::BlockStateId,
    face: horizon_core::geometry::Direction,
    signal: &mut i32,
) {
    crate::game::create::block_signal(state, face, signal);
}
