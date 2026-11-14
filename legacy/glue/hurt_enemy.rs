// A player's hit lands on a mob: a held extendo grip wears.
fn hurt_enemy(
    game: &mut crate::game::Game<'_>,
    stack: &horizon_client::items::ItemStack,
    entity: u32,
    kind: &str,
) {
    let _ = (stack, entity, kind);
    crate::game::create_equipment::wear_extendo(game);
}
