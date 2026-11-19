//! Create's `ItemUseOverrides` (`foundation/block/ItemUseOverrides.java`): the blocks whose own
//! use takes a click over the held item, sneaking included. `onRegister(ItemUseOverrides::
//! addBlock)` registers the hand crank, every valve handle and the analog lever; on
//! `PlayerInteractEvent.RightClickBlock` — asked before the game gives a sneaking player's click
//! to the item — the class invokes the block's own use (`BlockHelper.invokeUse`) and cancels
//! with its result when that consumes the click, unless the hand holds the wrench, whose own
//! use wins. `right_click_block` is the port's moment for that event.

use super::super::blocks;
use super::super::items::{self, Item};
use super::super::redstone::detectors;
use super::super::server::ServerLevel;
use super::crank;
use create_engine::Block;
use create_engine::dye;
use create_engine::item_use_overrides::{self, ItemOn, invoke_use};
use mod_api::{Flow, Hand, Host, InteractionResult, PlayerRef, RightClickBlockEvent, ServerHost};

/// Whether the block's own use answers `player`'s click, the same on both sides: a hand crank's
/// for anyone but a spectator (`HandCrankBlock.useItemOn`), a valve handle's for a player who
/// may build and without a sneaking wrench (`ValveHandleBlock.onBlockActivated`), and an analog
/// lever's for everyone (`AnalogLeverBlock.useWithoutItem`). False when the block lets the
/// click through.
pub(super) fn answers(host: &dyn Host, player: PlayerRef, block: Block) -> bool {
    if block == Block::HandCrank {
        // `if (player.isSpectator()) return PASS_TO_DEFAULT_BLOCK_INTERACTION`.
        return player.is_spectator(host) != Some(true);
    }
    if dye::is_valve_handle(block) {
        // `onBlockActivated`: not for a player who may not build, nor a sneaking wrench; else
        // it recolours or turns.
        if player.may_build(host) != Some(true) {
            return false;
        }
        let held = items::held(host, player, Hand::Main);
        return !(items::sneaking(host, player) && items::is(held.as_ref(), Item::Wrench));
    }
    block == Block::AnalogLever
}

/// `ItemUseOverrides.onBlockActivated`, on the server: a click on one of the override blocks
/// held with anything but the wrench is the block's own use (`BlockHelper.invokeUse`), and the
/// click is cancelled with its result when that consumes it.
pub(super) fn right_click(host: &mut dyn ServerHost, event: &mut RightClickBlockEvent<'_>) -> Flow {
    let click = event.click;
    let Some(player) = host.player(click.player) else {
        return Flow::Continue;
    };
    let held = click.stack.map(|s| s.item().id(&*host));
    let dimension = click.dimension.key().id().to_owned();
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return Flow::Continue;
    };
    let Some((block, _, state)) = blocks::block_at(&lv, click.pos) else {
        return Flow::Continue;
    };
    if !item_use_overrides::invokes_use(block, held.as_deref()) {
        return Flow::Continue;
    }
    // `BlockHelper.invokeUse`: `useItemOn` first, then `useWithoutItem` for the main hand. None
    // of these uses reads the hit, which Java rebuilds at the block's centre anyway
    // (`VecHelper.getCenterOf(pos)`).
    let back = items::sneaking(&*lv.host, player);
    let main_hand = click.hand == Hand::Main;
    let result = match block {
        // `HandCrankBlock.useItemOn`: the crank turns, tiring the player. A spectator's pass
        // (`answers`) falls through to the plain block's `useWithoutItem`, which answers
        // nothing.
        Block::HandCrank => invoke_use(
            crank::use_crank(&mut lv, player, click.pos, held.as_deref())
                .map_or(ItemOn::PassToDefault, ItemOn::Consumed),
            main_hand,
            || None,
        ),
        // `ValveHandleBlock.clicked`, its own `RightClickBlock` handler at `LOW` priority,
        // which the click reaches once `ItemUseOverrides` has left it: a dye recolours it,
        // else it turns.
        b if dye::is_valve_handle(b) => {
            blocks::use_valve_handle(&mut lv, player, click.pos, b, state, held.as_deref(), back)
        }
        // The lever has no `useItemOn`; `AnalogLeverBlock.useWithoutItem`: one up, or down
        // while sneaking. The wrench never reaches here; `ItemUseOverrides` declines it first.
        Block::AnalogLever => invoke_use(ItemOn::PassToDefault, main_hand, || {
            detectors::use_lever(&mut lv, click.pos, back)
                .then_some(InteractionResult::SuccessServer)
        }),
        _ => None,
    };
    let Some(result) = result else {
        return Flow::Continue;
    };
    *event.cancellation_result = result;
    Flow::Cancel
}
