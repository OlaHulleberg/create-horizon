// A scheduled block tick: Create's latches and brass diodes run their own, not their repeater
// template's.
fn tick_block(state: horizon_world::registry::BlockStateId, handled: &mut bool) {
    crate::game::create::tick_block(state, handled);
}
