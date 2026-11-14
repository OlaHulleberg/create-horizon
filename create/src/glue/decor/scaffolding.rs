//! Metal scaffolding (`MetalScaffoldingBlock`): unlike its vanilla template it never falls nor
//! breaks (`tick` does nothing, `canSurvive` is true), so it reaches out sideways as far as it
//! is placed; a scaffolding with neither the same scaffolding nor a sturdy top under it is a
//! horizontal one (`bottom`, `updateShape` from below). The template's own placement already
//! gives `bottom` by that rule (`ScaffoldingBlock.isBottom` with a distance of 7 for a block
//! that is not vanilla scaffolding); a neighbour's change is looked at after the level's tick,
//! from the tick vanilla's `updateShape` schedules.

use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::block_of;
use create_engine::Block;
use mod_api::{
    BlockFace, CanSurviveEvent, Flow, LevelHost, LevelWriteHost, Registrar, SupportType,
    TickBlockEvent,
};

pub(super) const BLOCKS: [Block; 3] = [
    Block::AndesiteScaffolding,
    Block::BrassScaffolding,
    Block::CopperScaffolding,
];

pub(super) fn register(r: &mut dyn Registrar) {
    for block in BLOCKS {
        r.block_part(states::key(block), Scaffolding);
    }
}

struct Scaffolding;

#[mod_api::horizon_part]
impl mod_api::BlockPart for Scaffolding {
    /// `MetalScaffoldingBlock.tick`: nothing (the template would fall or break).
    fn tick_block(&self, _level: &mut dyn LevelWriteHost, _event: &mut TickBlockEvent) -> Flow {
        Flow::Cancel
    }

    /// `MetalScaffoldingBlock.canSurvive`: anywhere.
    fn can_survive(&self, _level: &dyn LevelHost, _event: &mut CanSurviveEvent) -> Option<bool> {
        Some(true)
    }
}

/// `MetalScaffoldingBlock.updateShape` from below: `bottom` unless the same scaffolding or a
/// sturdy top is under it.
pub(super) fn check(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let Some(block) = lv
        .block_state(pos)
        .and_then(states::id)
        .and_then(block_of)
        .map(|(b, _)| b)
        .filter(|b| BLOCKS.contains(b))
    else {
        return;
    };
    let below = [pos[0], pos[1] - 1, pos[2]];
    let same = lv
        .block_state(below)
        .and_then(states::id)
        .and_then(block_of)
        .is_some_and(|(b, _)| b == block);
    let sturdy = lv
        .level
        .block_face_sturdy(&*lv.host, below, BlockFace::Up, SupportType::Full)
        .unwrap_or(false);
    let bottom = if !same && !sturdy { "true" } else { "false" };
    lv.set_property(pos, "bottom", bottom, update_all());
}
