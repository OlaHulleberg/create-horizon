// A player gets on or sneaks off a vehicle (NeoForge's `EntityMountEvent`): one getting off a
// Create seat, in the world or on a train's carriage, is let off where Create lets them; a cart
// carrying a contraption takes no rider.
fn entity_mount(
    game: &mut crate::game::Game<'_>,
    dimension: &str,
    vehicle: u32,
    mounting: bool,
    canceled: &mut bool,
) {
    crate::game::create_carts::entity_mount(dimension, vehicle, mounting, canceled);
    if !*canceled {
        crate::game::create_seats::entity_mount(game, dimension, vehicle, mounting);
        crate::game::create_train_controls::entity_mount(game, dimension, vehicle, mounting);
    }
}
