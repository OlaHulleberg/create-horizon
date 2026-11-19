//! Hand cranks turned by hand: their block's own use (`HandCrankBlock.useItemOn`), on the server,
//! invoked both by the game's use of the block and by `ItemUseOverrides` for a click the game
//! would give to the held item (`item_use_overrides`). The client predicts the same answer for
//! a crank and for a valve handle (whose use, a turn or a recolour, is `blocks`') in
//! `right_click_block` (`board::right_click`), so the click ends there too.

use super::super::items;
use super::super::server::ServerLevel;
use super::super::states;
use super::super::world::with_world;
use super::item_use_overrides;
use create_engine::Block;
use mod_api::{BlockPart, InteractionResult, PlayerRef, Registrar, ServerHost, UseItemOnEvent};

pub(super) fn register(r: &mut dyn Registrar) {
    r.block_part(states::key(Block::HandCrank), Crank);
}

/// The part a hand crank carries.
struct Crank;

#[mod_api::sunrise_part]
impl BlockPart for Crank {
    /// `HandCrankBlock.useItemOn`, the game's own use of the block.
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let player = host.player(click.player)?;
        let held = click.stack.map(|s| s.item().id(&*host));
        let mut lv = ServerLevel::new(host, click.dimension.key().id())?;
        use_crank(&mut lv, player, click.pos, held.as_deref())
    }
}

/// `HandCrankBlock.useItemOn`: the crank turns for 10 ticks, backwards while sneaking, and
/// tires the player (`crankHungerMultiplier` 0.01 per RPM, and never for an extendo grip).
/// `InteractionResult::Success`, or None when the crank's use does not answer (a spectator's
/// pass, whose `useWithoutItem` is the plain block's).
pub(super) fn use_crank(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    pos: [i32; 3],
    held: Option<&str>,
) -> Option<InteractionResult> {
    if !item_use_overrides::answers(&*lv.host, player, Block::HandCrank) {
        return None;
    }
    let back = items::sneaking(&*lv.host, player);
    let dim = lv.dim;
    if with_world(|w| w.turn_crank(dim, pos, back)) != Some(true) {
        return None;
    }
    // `if (!stack.is(AllItems.EXTENDO_GRIP.get())) player.causeFoodExhaustion(...)`.
    if held != Some("create:extendo_grip") && !items::creative(&*lv.host, player) {
        player.add_exhaustion(lv.host, create_engine::parts::CRANK_RPM * 0.01);
    }
    // `HandCrankBlock.useItemOn`: turned on an empty stomach.
    if player.food(&*lv.host).is_some_and(|f| f.level == 0) {
        crate::glue::advancements::award(lv.host, player, "hand_crank_000");
    }
    crate::glue::persist::publish(lv);
    Some(InteractionResult::Success)
}
