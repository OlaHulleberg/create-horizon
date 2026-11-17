//! `WrenchItem.useOn` on an `IWrenchable` Create block: sneaking, `onSneakWrenched` (its drops to
//! the player unless creative, the block gone without drops); else `onWrenched`, turned about the
//! axis of the face clicked, with Create's sounds. Any other block in `create:wrench_pickup`
//! (bars, rails, redstone parts...) is picked up the same way when sneaking
//! (`onItemUseOnOther`); the rest are left alone. A block a subsystem wrenches its own way
//! answers first (`Subsystem::wrench_used`: a copycat's material, a pipe's glass, a whistle's
//! size, a belt's casing, a funnel's mode). What takes the click before the wrench's use
//! is each block's own: a crafter's edge and a deployer's mode (processing's
//! `right_click_block`), a bracket (decor's part), a cart assembler (trains' part), a redstone
//! link (redstone's part).

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
    let block = super::super::client_table(&*host, |t| t.get(state).map(|(b, _)| b)).flatten();
    let Some(block) = block.filter(|b| create_engine::wrench::wrenchable(b.name())) else {
        let level = host.level(host.dimension())?;
        let state = level.block_state(&*host, event.pos)?;
        return (sneaking(&*host, player) && pickup(&*host, state))
            .then_some(InteractionResult::Success);
    };
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
    let block = (id != 0)
        .then(|| with_world(|w| w.table.get(id).map(|(b, _)| b)).flatten())
        .flatten()
        .filter(|b| create_engine::wrench::wrenchable(b.name()));
    let back = sneaking(&*lv.host, player);
    let Some(block) = block else {
        // `onItemUseOnOther`: any block in `create:wrench_pickup`, sneaking.
        let state = lv.block_state(pos)?;
        if !back || !pickup(&*lv.host, state) {
            return None;
        }
        remove(&mut lv, player, pos, event.stack);
        return Some(InteractionResult::Success);
    };
    let clicked = Dir::from_offset(event.face)?;
    for s in super::super::SUBSYSTEMS {
        if s.wrench_used(&mut lv, player, pos, block, clicked, event.hit.point, back) {
            return Some(InteractionResult::Success);
        }
    }
    if back {
        remove(&mut lv, player, pos, event.stack);
        return Some(InteractionResult::Success);
    }
    // `NozzleBlock.onWrenched`: FAIL (it is only picked up).
    if block == Block::Nozzle {
        return Some(InteractionResult::Fail);
    }
    if let Some(next) = turned(&lv, pos, clicked) {
        switch_state(&mut lv, pos, next);
        rotate_sound(&mut lv, pos);
        if matches!(block, Block::RedstoneContact | Block::ElevatorContact) {
            crate::glue::contraptions::contact_wrenched(&mut lv, pos);
        }
    }
    Some(InteractionResult::Success)
}

/// `KineticBlockEntity.switchToBlockState`, shared by block interactions and wrench overrides.
pub(crate) fn switch_state(lv: &mut ServerLevel<'_>, pos: [i32; 3], next: mod_api::BlockState) {
    if lv.block_state(pos) == Some(next) {
        return;
    }
    if let Some(id) = states::id(next) {
        with_world(|w| w.switching_block_state(lv.dim, pos, id));
    }
    lv.set_state(pos, next, update_all());
}

/// Switch one property through `KineticBlockEntity.switchToBlockState`.
pub(crate) fn switch_property(lv: &mut ServerLevel<'_>, pos: [i32; 3], key: &str, value: &str) {
    if let Some(state) = lv.block_state(pos)
        && let Ok(next) = state.with(&lv.host.block_states(), key, value)
    {
        switch_state(lv, pos, next);
    }
}

/// `IWrenchable.playRotateSound`: `WRENCH_ROTATE` (the wooden trapdoor's close at .25 and 1.25
/// times) at a pitch of `nextFloat() + .5`.
pub(crate) fn rotate_sound(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let pitch = random(lv) + 0.5;
    sound(lv, "create:wrench_rotate", pos, 0.25, 1.25 * pitch);
}

/// Whether `state` is in `create:wrench_pickup` (`WrenchItem.canWrenchPickup`).
fn pickup(host: &dyn mod_api::Host, state: mod_api::BlockState) -> bool {
    host.block_in_tag(state.block(), "create:wrench_pickup") == Some(true)
}

/// `onSneakWrenched` and `onItemUseOnOther`: `Block.getDrops` with the wrench, back into the
/// inventory outside creative, `destroyBlock(pos, false)`, then `WRENCH_REMOVE` (an item pickup
/// and a netherite block's hit, each at .25 and .75 times) at a pitch of `nextFloat() * .5 + .5`.
fn remove(
    lv: &mut ServerLevel<'_>,
    player: mod_api::PlayerRef,
    pos: [i32; 3],
    wrench: &mod_api::ItemStack,
) {
    if !super::creative(&*lv.host, player) {
        let drops = lv
            .level
            .block_drops(&mut *lv.host, pos, Some(wrench), Some(player.entity()))
            .ok()
            .flatten()
            .unwrap_or_default();
        for d in drops {
            give_back(lv.host, player, d);
        }
    }
    lv.destroy_block(pos, false);
    let pitch = random(lv) * 0.5 + 0.5;
    for name in ["create:wrench_remove", "create:wrench_remove_compounded_1"] {
        sound(lv, name, pos, 0.25, 0.75 * pitch);
    }
}

/// `Create.RANDOM.nextFloat()`, from the level's random.
fn random(lv: &mut ServerLevel<'_>) -> f32 {
    use mod_api::RandomSource;
    lv.level
        .with_random(&mut *lv.host, |r| r.next_float())
        .unwrap_or(0.5)
}

fn sound(lv: &mut ServerLevel<'_>, name: &str, pos: [i32; 3], volume: f32, pitch: f32) {
    let at = pos.map(|v| f64::from(v) + 0.5);
    lv.level.play_sound(
        &mut *lv.host,
        name,
        at,
        mod_api::SoundSource::Blocks,
        volume,
        pitch,
    );
}

/// `IWrenchable.getRotatedBlockState`: the state at `pos` turned about the clicked face's axis,
/// or None when it doesn't turn.
fn turned(lv: &ServerLevel<'_>, pos: [i32; 3], clicked: Dir) -> Option<mod_api::BlockState> {
    let state = lv.block_state(pos)?;
    let text = lv.host.block_state_string(state)?;
    let (name, own) = states::split(&text);
    let props: BTreeMap<String, String> = own
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let facts = lv.host.block_states();
    // A `facing` that never points up is a horizontal one (HORIZONTAL_FACING).
    let horizontal = props.contains_key("facing") && state.with(&facts, "facing", "up").is_err();
    let turned = if name == "create:mechanical_pump" {
        // `PumpBlock.getRotatedBlockState`: a pump turns around, whichever face is clicked.
        create_engine::wrench::reversed(&props)
    } else {
        create_engine::wrench::rotated(&props, horizontal, clicked)
    };
    if turned == props {
        return None;
    }
    let mut next = state;
    for (k, v) in &turned {
        next = next.with(&facts, k, v).ok()?;
    }
    Some(next)
}
