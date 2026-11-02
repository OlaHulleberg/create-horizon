// A mod seat came back from a save with its riders (`EntityJoinLevelEvent` loaded from disk):
// Create's seat stand-ins are taken back by their seat block or their carriage seat at the
// level's next tick.
fn seat_loaded(dimension: &str, seat: u32, pos: [f64; 3]) {
    crate::game::create_seats::seat_loaded(dimension, seat, pos);
}
