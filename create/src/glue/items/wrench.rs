//! `WrenchItem.useOn` on a Create block: sneaking, `onSneakWrenched` (its drops to the player
//! unless creative, the block gone without drops); else `onWrenched`, turned about the axis of
//! the face clicked. Blocks not Create's are left alone; a block a subsystem wrenches its own way
//! answers first (`Subsystem::wrench_used`: a copycat's material, a pipe's glass, a whistle's
//! size, a belt's casing, a funnel's mode). What takes the click before the wrench's use (the legacy
//! `use_item_on` order) is each block's own: a crafter's edge and a deployer's mode
//! (processing's `right_click_block`), a bracket (decor's part), a cart assembler (trains'
//! part), a redstone link (redstone's part).

use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::super::world::with_world;
use super::{give_back, sneaking};
use create_engine::Block;
use create_engine::kinetics::Dir;
use mod_api::{
    ClientHost, HostBlockStates, HostSide, InteractionResult, ItemPart, ServerHost, UseOnEvent,
};
use std::collections::BTreeMap;

pub(super) struct Wrench;

#[mod_api::horizon_part]
impl ItemPart for Wrench {
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Server(host) => use_on(host, event),
            HostSide::Client(host) => predict(host, event),
        }
    }
}

/// The client's answer, as the server's will be: `IWrenchable`'s `SUCCESS` on a Create block,
/// for a player who may build.
fn predict(host: &mut dyn ClientHost, event: &UseOnEvent<'_>) -> Option<InteractionResult> {
    let player = host.player(event.player)?;
    if player.may_build(&*host) != Some(true) {
        return None;
    }
    let state = host
        .level(host.dimension())?
        .block_state(&*host, event.pos)
        .and_then(states::id)?;
    let block = super::super::client_table(&*host, |t| t.get(state).map(|(b, _)| b)).flatten()?;
    Some(if block == Block::Nozzle && !sneaking(&*host, player) {
        InteractionResult::Fail
    } else {
        InteractionResult::Success
    })
}

fn use_on(host: &mut dyn ServerHost, event: &UseOnEvent<'_>) -> Option<InteractionResult> {
    let player = host.player(event.player)?;
    // `if (player == null || !player.mayBuild()) return super.useOn(context)`.
    if player.may_build(&*host) != Some(true) {
        return None;
    }
    let dimension = player.dimension(&*host)?.to_string();
    let mut lv = ServerLevel::new(host, &dimension)?;
    let pos = event.pos;
    let id = lv.block(pos);
    if id == 0 {
        return None;
    }
    let block = with_world(|w| w.table.get(id).map(|(b, _)| b)).flatten()?;
    let clicked = Dir::from_offset(event.face)?;
    let back = sneaking(&*lv.host, player);
    for s in super::super::SUBSYSTEMS {
        if s.wrench_used(&mut lv, player, pos, block, clicked, event.hit.point, back) {
            return Some(InteractionResult::Success);
        }
    }
    if back {
        // `onSneakWrenched`: `Block.getDrops` with the wrench, back into the inventory outside
        // creative, then `destroyBlock(pos, false)`.
        if !super::creative(&*lv.host, player) {
            let drops = lv
                .level
                .block_drops(&mut *lv.host, pos, Some(event.stack), Some(player.entity()))
                .ok()
                .flatten()
                .unwrap_or_default();
            for d in drops {
                give_back(lv.host, player, d);
            }
        }
        lv.destroy_block(pos, false);
        return Some(InteractionResult::Success);
    }
    // `NozzleBlock.onWrenched`: FAIL (it is only picked up).
    if block == Block::Nozzle {
        return Some(InteractionResult::Fail);
    }
    if let Some(next) = turned(&lv, pos, clicked) {
        lv.set_state(pos, next, update_all());
    }
    Some(InteractionResult::Success)
}

/// `IWrenchable.getRotatedBlockState`: the state at `pos` turned about the clicked face's axis,
/// or None when it doesn't turn.
fn turned(lv: &ServerLevel<'_>, pos: [i32; 3], clicked: Dir) -> Option<mod_api::BlockState> {
    let state = lv.block_state(pos)?;
    let text = lv.host.block_state_string(state)?;
    let (_, own) = states::split(&text);
    let props: BTreeMap<String, String> = own
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let facts = lv.host.block_states();
    // A `facing` that never points up is a horizontal one (HORIZONTAL_FACING).
    let horizontal = props.contains_key("facing") && state.with(&facts, "facing", "up").is_err();
    let turned = create_engine::wrench::rotated(&props, horizontal, clicked);
    if turned == props {
        return None;
    }
    let mut next = state;
    for (k, v) in &turned {
        next = next.with(&facts, k, v).ok()?;
    }
    Some(next)
}
