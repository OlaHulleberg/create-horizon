// An entity is inside a block with `entity_inside` (`BlockBehaviour.entityInside`): Create's
// seats seat the mobs that land on them, turning turntables turn what stands on them.
fn entity_inside(
    game: &mut crate::game::Game<'_>,
    dimension: &str,
    pos: horizon_core::position::BlockPos,
    state: horizon_world::registry::BlockStateId,
    entity: &crate::game::native_entities::NativeEntity,
) {
    crate::game::create_seats::entity_inside(game, dimension, pos, state, entity);
}
