// A player breaks a block: a held extendo grip wears, a backtank's drop keeps its air.
fn destroy_block(
    game: &mut crate::game::Game<'_>,
    pos: [i32; 3],
    state: horizon_world::registry::BlockStateId,
    canceled: &mut bool,
) {
    crate::game::create_equipment::destroy_block(game, pos, state, *canceled);
}
