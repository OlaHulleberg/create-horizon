// A player right-clicks an entity: an empty blaze burner catches a blaze; a coupling or a wrench
// on a minecart couples, uncouples or picks up its contraption.
fn interact_entity(
    game: &mut crate::game::Game<'_>,
    entity: i32,
    kind: &str,
    stack: Option<&horizon_client::items::ItemStack>,
    claimed: &mut bool,
) {
    crate::game::create_carts::interact_entity(game, entity, kind, stack, claimed);
    if !*claimed {
        crate::game::create_decor::interact_entity(game, entity, kind, stack, claimed);
    }
}
