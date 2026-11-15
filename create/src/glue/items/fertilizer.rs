//! `TreeFertilizerItem.useOn` on a sapling (not a hanging propagule): its tree grows now. Create
//! grows it in a simulated world at stage 1; here the sapling is set to stage 1 and bone meal is
//! applied as a dispenser applies it (vanilla's 45% rolls) until it grows, so the tree needs
//! vanilla's room to grow.

use super::super::server::{ServerLevel, UPDATE_CLIENTS};
use super::{creative, shrink_held};
use mod_api::{
    BlockState, Host, HostSide, InteractionResult, ItemPart, LevelRef, ServerHost, UseOnEvent,
};

pub(super) struct Fertilizer;

#[mod_api::horizon_part]
impl ItemPart for Fertilizer {
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Server(host) => grow(host, event),
            HostSide::Client(host) => {
                let level = host.level(host.dimension())?;
                sapling(&*host, level, event.pos)?;
                Some(InteractionResult::Success)
            }
        }
    }
}

/// The sapling at `pos` (`instanceof SaplingBlock`: the `minecraft:saplings` tag, a hanging
/// propagule aside).
fn sapling(host: &dyn Host, level: LevelRef, pos: [i32; 3]) -> Option<BlockState> {
    let state = level.block_state(host, pos)?;
    let hanging = level.block_property(host, pos, "hanging").as_deref() == Some("true");
    (host.block_in_tag(state.block(), "minecraft:saplings") == Some(true) && !hanging)
        .then_some(state)
}

fn grow(host: &mut dyn ServerHost, event: &UseOnEvent<'_>) -> Option<InteractionResult> {
    let player = host.player(event.player)?;
    let dimension = player.dimension(&*host)?.to_string();
    let mut lv = ServerLevel::new(host, &dimension)?;
    let pos = event.pos;
    sapling(&*lv.host, lv.level, pos)?;
    lv.set_property(pos, "stage", "1", UPDATE_CLIENTS);
    let staged = lv.block_state(pos);
    for _ in 0..64 {
        // Not a valid target (no room to grow), or grown.
        if !lv.level.bone_meal(&mut *lv.host, pos) || lv.block_state(pos) != staged {
            break;
        }
    }
    if !creative(&*lv.host, player) {
        shrink_held(lv.host, player, event.hand, 1);
    }
    Some(InteractionResult::Success)
}
