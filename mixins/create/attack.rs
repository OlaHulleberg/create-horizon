// A player's blow on an entity: a cardboard sword hurts nothing.
fn attack_entity(
    game: &mut crate::game::Game<'_>,
    entity: i32,
    kind: &str,
    pos: [f64; 3],
    canceled: &mut bool,
) {
    crate::game::create_items::attack_entity(game, entity, kind, pos, canceled);
}
