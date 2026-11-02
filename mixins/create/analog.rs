// What a comparator reads from a block: Create's vaults, tanks, depots, backtanks, gauges,
// burners, packagers and ports answer from their state (one range test for any other block).
fn analog_output_signal(
    level: &crate::game::GameLevel<'_>,
    dimension: horizon_world::level::Dimension,
    pos: horizon_core::position::BlockPos,
    state: horizon_world::registry::BlockStateId,
    direction: horizon_core::geometry::Direction,
    signal: &mut i32,
) {
    crate::game::create::analog_output_signal(
        &*level.world,
        level.items,
        dimension,
        pos,
        state,
        direction,
        signal,
    );
}
