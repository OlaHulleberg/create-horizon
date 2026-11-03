// The window's player collides with and rides the trains' carriages (ContraptionCollider).
fn entity_collisions(
    game: &crate::game::Game<'_>,
    around: horizon_core::geometry::Aabb,
    out: &mut Vec<(horizon_core::geometry::Aabb, horizon_core::geometry::Vec3)>,
) {
    crate::game::create_trains::entity_collisions(game, around, out);
}
