// World open and close: index Create's block states, or drop the world's state.
fn world_lifecycle(
    game: &mut crate::game::Game<'_>,
    phase: &crate::game::native_lifecycle::WorldPhase,
) {
    crate::game::create::world_lifecycle(game, phase);
    if !matches!(
        phase,
        crate::game::native_lifecycle::WorldPhase::WorldOpenReady
    ) {
        crate::game::create_equipment::world_closed();
    }
}
