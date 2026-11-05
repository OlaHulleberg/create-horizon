// Stand-in for `mixins/create/copper_body.rs` in the template test: the fixture's level has no
// block data or random, so Create's copper only keeps vanilla's random tick from running.
use super::create::with_world;
use create_engine::states;
use horizon_world::registry::BlockStateId;

pub(super) fn random_tick(
    level: &mut super::GameLevel<'_>,
    ticks: &mut super::Ticks,
    pos: horizon_core::position::BlockPos,
    state: BlockStateId,
    handled: &mut bool,
) {
    let _ = (level, ticks, pos);
    if states::index(state.0).is_none() {
        return;
    }
    let block = with_world(|w| w.table.get(state.0).map(|(b, _)| b)).flatten();
    if block.and_then(create_engine::decor::copper_next).is_some() {
        *handled = true;
    }
}

pub(super) fn use_item_on(
    game: &mut super::Game<'_>,
    stack: &horizon_client::items::ItemStack,
    pos: [i32; 3],
    claimed: &mut bool,
) -> bool {
    let _ = (game, stack, pos, claimed);
    false
}
