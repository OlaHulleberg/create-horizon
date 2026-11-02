// A player gets on or sneaks off a vehicle (NeoForge's `EntityMountEvent`): one getting off a
// Create seat is let off where Create lets them.
fn entity_mount(game: &mut crate::game::Game<'_>, dimension: &str, vehicle: u32, mounting: bool) {
    crate::game::create_seats::entity_mount(game, dimension, vehicle, mounting);
}
