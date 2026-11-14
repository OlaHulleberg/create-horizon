//! What copycats read of the game: the block an item places, what
//! `CopycatBlock.getAcceptedBlockState` tests of it (its tags, shapes, block entity and class),
//! what `cycleMaterial` reads of a material, and a block's place sound.

use super::super::server::ServerLevel;
use super::with_prop;
use create_engine::Block;
use create_engine::copycat::{self as cc, Candidate, Shape, Special};
use create_engine::kinetics::{Axis, Dir};
use mod_api::{BlockShapeKind, BlockState, Host, HostBlockStates, ItemStack};

/// The default state of the block `stack` places (`BlockItem.getBlock().defaultBlockState()`).
fn item_block(host: &dyn Host, stack: &ItemStack) -> Option<BlockState> {
    let block = host.item_block(stack.item())?;
    host.block_state_default(block)
}

/// `getAcceptedBlockState`'s tests and `CopycatSpecialCases` of `state`, as a copycat at `pos`
/// reads them.
pub(super) fn candidate(lv: &ServerLevel<'_>, state: BlockState, pos: [i32; 3]) -> Candidate {
    let host = &*lv.host;
    let block = state.block();
    let id = block.id(host);
    let tagged = |t: &str| host.block_in_tag(block, t) == Some(true);
    let create = id.strip_prefix("create:").and_then(Block::from_name);
    // `getShape(level, pos).bounds()` is the whole block.
    let outline = host.state_outline(state).unwrap_or_default();
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for o in &outline {
        for i in 0..3 {
            lo[i] = lo[i].min(o[i]);
            hi[i] = hi[i].max(o[i + 3]);
        }
    }
    let full = !outline.is_empty() && lo == [0.0; 3] && hi == [1.0; 3];
    let collides = lv
        .level
        .block_shape(host, state, pos, BlockShapeKind::Collision)
        .is_some_and(|s| !s.is_empty());
    let facts = host.block_states();
    let has = |k: &str| state.property(&facts, k).ok().flatten().is_some();
    Candidate {
        copycat: matches!(create, Some(Block::CopycatPanel | Block::CopycatStep)),
        allow: tagged("create:copycat_allow"),
        deny: tagged("create:copycat_deny"),
        entity: facts.has_block_entity(state) == Some(true)
            || create.is_some_and(Block::has_block_entity),
        // No API names a block's class: vanilla's tags stand for `StairBlock` and
        // `TrapDoorBlock`, and an `IronBarsBlock` that is no glass pane is a `_bars` block.
        stairs: tagged("minecraft:stairs"),
        full,
        collides,
        bars: id.ends_with("_bars") && !id.ends_with("_pane"),
        trapdoor: tagged("minecraft:trapdoors") && has("half") && has("open") && has("facing"),
    }
}

/// `getAcceptedBlockState` for the item `stack` on a copycat of `shape` at `pos`: the item's
/// block, turned to the clicked `face` when there is one; None when the copycat doesn't take it.
pub(super) fn accepted(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    shape: Shape,
    stack: &ItemStack,
    face: Option<Dir>,
) -> Option<(BlockState, Special)> {
    let mut state = item_block(&*lv.host, stack)?;
    let c = candidate(lv, state, pos);
    if !cc::accepts(shape, &c) {
        return None;
    }
    if let Some(face) = face {
        for (k, v) in cc::oriented(face) {
            if let Some(next) = with_prop(&*lv.host, state, k, v) {
                state = next;
            }
        }
    }
    Some((state, Special::of(&c)))
}

/// What `cycleMaterial` changes of `state`: the property and its new value.
pub(super) fn cycle_of(host: &dyn Host, state: BlockState) -> Option<(&'static str, &'static str)> {
    let facts = host.block_states();
    let prop = |k: &str| state.property(&facts, k).ok().flatten();
    let flag = |k: &str| prop(k).map(|v| v == "true");
    let allows = |k: &str, v: &str| with_prop(host, state, k, v).is_some();
    let m = cc::Material {
        facing: prop("facing")
            .and_then(cc::dir_named)
            .map(|f| (f, allows("facing", "up"))),
        axis: prop("axis")
            .and_then(|a| match a {
                "x" => Some(Axis::X),
                "y" => Some(Axis::Y),
                "z" => Some(Axis::Z),
                _ => None,
            })
            .map(|a| (a, allows("axis", "y"))),
        half_top: prop("half")
            .filter(|h| matches!(*h, "top" | "bottom"))
            .map(|h| h == "top"),
        open: flag("open"),
        lit: flag("lit"),
        powering: flag("powering").filter(|_| state.block().id(host) == "create:rose_quartz_lamp"),
    };
    cc::cycle(&m)
}

/// `getSoundType().getPlaceSound()` of `state`.
pub(super) fn place_sound(host: &dyn Host, state: BlockState) -> Option<String> {
    let place = host.block_states().sound(state)?.place;
    (!place.is_empty()).then(|| place.to_string())
}
