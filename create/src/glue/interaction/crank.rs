//! Hand cranks turned by hand: their block's own use (`HandCrankBlock.useItemOn`), on the server.
//! The client predicts the same answer for a crank and for a valve handle (whose use, a turn or a
//! recolour, is `blocks`') in `right_click_block` (`board::right_click`),
//! so the click ends there too.

use super::super::items::{self, Item};
use super::super::server::ServerLevel;
use super::super::states;
use super::super::world::with_world;
use create_engine::Block;
use create_engine::dye;
use mod_api::{
    BlockPart, Host, InteractionResult, PlayerRef, Registrar, ServerHost, UseItemOnEvent,
};

pub(super) fn register(r: &mut dyn Registrar) {
    r.block_part(states::key(Block::HandCrank), Crank);
}

/// Whether a click on a hand crank or valve handle is the block's, the same on both sides; false
/// when the block lets it through.
pub(super) fn answers(host: &dyn Host, player: PlayerRef, block: Block) -> bool {
    if block == Block::HandCrank {
        // `if (player.isSpectator()) return PASS`.
        return player.is_spectator(host) != Some(true);
    }
    if !dye::is_valve_handle(block) {
        return false;
    }
    // `onBlockActivated`: not for a player who may not build, nor a sneaking wrench; else it
    // recolours or turns.
    if player.may_build(host) != Some(true) {
        return false;
    }
    let held = items::held(host, player, mod_api::Hand::Main);
    !(items::sneaking(host, player) && items::is(held.as_ref(), Item::Wrench))
}

/// The part a hand crank carries.
struct Crank;

#[mod_api::horizon_part]
impl BlockPart for Crank {
    /// `HandCrankBlock.useItemOn`: the crank turns for 10 ticks, backwards while sneaking, and
    /// tires the player (`crankHungerMultiplier` 0.01 per RPM).
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let player = host.player(click.player)?;
        if !answers(&*host, player, Block::HandCrank) {
            return None;
        }
        let back = items::sneaking(&*host, player);
        let mut lv = ServerLevel::new(host, click.dimension.key().id())?;
        let dim = lv.dim;
        if with_world(|w| w.turn_crank(dim, click.pos, back)) != Some(true) {
            return None;
        }
        if !items::creative(&*lv.host, player) {
            player.add_exhaustion(lv.host, create_engine::parts::CRANK_RPM * 0.01);
        }
        // `HandCrankBlock.useItemOn`: turned on an empty stomach.
        if player.food(&*lv.host).is_some_and(|f| f.level == 0) {
            crate::glue::advancements::award(lv.host, player, "hand_crank_000");
        }
        crate::glue::persist::publish(&mut lv);
        Some(InteractionResult::Success)
    }
}
