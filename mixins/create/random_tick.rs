// A random tick: Create's copper shingles and tiles weather into Create's next age (hot: one
// range test).
fn random_tick(
    level: &mut crate::game::GameLevel<'_>,
    ticks: &mut Ticks,
    pos: horizon_core::position::BlockPos,
    state: horizon_world::registry::BlockStateId,
    handled: &mut bool,
) {
    crate::game::create_copper_body::random_tick(level, ticks, pos, state, handled);
}
