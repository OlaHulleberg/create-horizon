//! Hand cranks and valve handles turned by hand: their blocks' own use (`HandCrankBlock.useItemOn`,
//! `ValveHandleBlock.useItemOn`), on the server. The client predicts the same answer in
//! `right_click_block` (`board::right_click`), so the click ends there too.

use super::super::items::{self, Item};
use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::super::world::with_world;
use create_engine::Block;
use create_engine::dye::{self, DyeColor};
use mod_api::{
    BlockPart, BlockState, Host, HostBlockStates, InteractionResult, PlayerRef, Registrar,
    ServerHost, UseItemOnEvent,
};

pub(super) fn register(r: &mut dyn Registrar) {
    r.block_part(states::key(Block::HandCrank), Crank);
    for &block in Block::ALL.iter().filter(|&&b| dye::is_valve_handle(b)) {
        r.block_part(states::key(block), Valve);
    }
}

/// What a click on a hand crank or valve handle comes to, the same on both sides: `None` when
/// the block lets it through.
pub(super) fn answers(host: &dyn Host, player: PlayerRef, block: Block) -> Option<Answer> {
    if block == Block::HandCrank {
        // `if (player.isSpectator()) return PASS`.
        return (player.is_spectator(host) != Some(true)).then_some(Answer::Turn);
    }
    if !dye::is_valve_handle(block) {
        return None;
    }
    // `onBlockActivated`: not for a player who may not build, nor a sneaking wrench; a dye of
    // another colour recolours it, else it turns.
    if player.may_build(host) != Some(true) {
        return None;
    }
    let held = items::held(host, player, mod_api::Hand::Main);
    if items::sneaking(host, player) && items::is(held.as_ref(), Item::Wrench) {
        return None;
    }
    let colour = held
        .as_ref()
        .and_then(|s| DyeColor::of_item(&s.item().id(host)));
    Some(match colour.and_then(|c| dye::recoloured(block, c)) {
        Some(to) => Answer::Recolour(to),
        None => Answer::Turn,
    })
}

pub(super) enum Answer {
    Turn,
    Recolour(Block),
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
        answers(&*host, player, Block::HandCrank)?;
        let back = items::sneaking(&*host, player);
        let lv = ServerLevel::new(host, click.dimension.key().id())?;
        let dim = lv.dim;
        if with_world(|w| w.turn_crank(dim, click.pos, back)) != Some(true) {
            return None;
        }
        if !items::creative(&*lv.host, player) {
            player.add_exhaustion(lv.host, create_engine::parts::CRANK_RPM * 0.01);
        }
        Some(InteractionResult::Success)
    }
}

/// The part every valve handle carries.
struct Valve;

#[mod_api::horizon_part]
impl BlockPart for Valve {
    /// `ValveHandleBlock.useItemOn` → `onBlockActivated` → `clicked`: a dye of another colour
    /// recolours it (`DYED_VALVE_HANDLES`, its properties kept), else it turns (`activate`),
    /// backwards while sneaking. The click is taken either way.
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let player = host.player(click.player)?;
        let block = states::id(event.state)
            .and_then(|id| with_world(|w| w.table.get(id).map(|(b, _)| b)).flatten())?;
        let answer = answers(&*host, player, block)?;
        let back = items::sneaking(&*host, player);
        let mut lv = ServerLevel::new(host, click.dimension.key().id())?;
        match answer {
            Answer::Recolour(to) => {
                if let Some(state) = recoloured(&*lv.host, event.state, to) {
                    lv.set_state(click.pos, state, update_all());
                }
            }
            Answer::Turn => {
                let dim = lv.dim;
                with_world(|w| w.use_valve(dim, click.pos, back));
            }
        }
        Some(InteractionResult::Success)
    }
}

/// `state` as block `to` with the same properties.
pub(crate) fn recoloured(host: &dyn Host, state: BlockState, to: Block) -> Option<BlockState> {
    let text = host.block_state_string(state)?;
    let (_, props) = states::split(&text);
    let facts = host.block_states();
    let mut next = host.block_state_default(states::key(to))?;
    for (k, v) in props {
        next = next.with(&facts, k, v).unwrap_or(next);
    }
    Some(next)
}
