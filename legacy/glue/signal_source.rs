// Whether a block gives a signal: Create's latches, brass diodes and rose quartz lamps do.
fn is_signal_source(state: horizon_world::registry::BlockStateId, source: &mut bool) {
    crate::game::create::is_signal_source(state, source);
}
