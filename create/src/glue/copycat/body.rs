//! What copycats read of the game: the block an item places, what
//! `CopycatBlock.getAcceptedBlockState` tests of it (its tags, shapes, block entity and class),
//! what `cycleMaterial` reads of a material, and a block's place sound.

use super::super::server::ServerLevel;
use super::with_prop;
use create_engine::Block;
use create_engine::copycat::{self as cc, Candidate, Shape, Special};
use create_engine::kinetics::{Axis, Dir};
use mod_api::{BlockClass, BlockShapeKind, BlockState, Host, HostBlockStates, ItemStack};

/// The default state of the block `stack` places (`BlockItem.getBlock().defaultBlockState()`).
fn item_block(host: &dyn Host, stack: &ItemStack) -> Option<BlockState> {
    let block = host.item_block(stack.item())?;
    host.block_state_default(block)
}

/// `getAcceptedBlockState`'s tests and `CopycatSpecialCases` of `state`, as a copycat at `pos`
/// reads them.
pub(super) fn candidate(
    lv: &ServerLevel<'_>,
    state: BlockState,
    pos: [i32; 3],
) -> Option<Candidate> {
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
    let class = |class| facts.block_class(state, class).ok().flatten();
    Some(Candidate {
        copycat: matches!(create, Some(Block::CopycatPanel | Block::CopycatStep)),
        allow: tagged("create:copycat_allow"),
        deny: tagged("create:copycat_deny"),
        // Create's own blocks can have custom block entities on non-EntityBlock templates.
        entity: class(BlockClass::Entity)? || create.is_some_and(Block::has_block_entity),
        stairs: class(BlockClass::Stair)?,
        full,
        collides,
        bars: special_bars(&id, create, class)?,
        trapdoor: class(BlockClass::TrapDoor)? && has("half") && has("open") && has("facing"),
    })
}

/// `CopycatSpecialCases`: glass panes inherit from `IronBarsBlock` without being bar material.
/// Stained panes have their own class check; these are vanilla's clear pane and Create's panes.
fn special_bars(
    id: &str,
    create: Option<Block>,
    class: impl Fn(BlockClass) -> Option<bool>,
) -> Option<bool> {
    Some(
        class(BlockClass::IronBars)?
            && !class(BlockClass::StainedGlassPane)?
            && id != "minecraft:glass_pane"
            && !create.is_some_and(|block| {
                block.name().ends_with("_window_pane")
                    || matches!(
                        block,
                        Block::FramedGlassPane
                            | Block::HorizontalFramedGlassPane
                            | Block::VerticalFramedGlassPane
                            | Block::TiledGlassPane
                    )
            }),
    )
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
    let c = candidate(lv, state, pos)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copycat_panel_accepts_iron_bars_but_not_create_window_panes() {
        let panel = Shape::Panel(Dir::Up);
        for (id, accepted) in [
            ("create:oak_window_pane", false),
            ("minecraft:iron_bars", true),
        ] {
            // Both inherit IronBarsBlock but neither has full-block bounds in this test.
            let create = id.strip_prefix("create:").and_then(Block::from_name);
            let facts = Candidate {
                bars: special_bars(id, create, |class| match class {
                    BlockClass::IronBars => Some(true),
                    BlockClass::StainedGlassPane => Some(false),
                    _ => None,
                })
                .unwrap(),
                collides: true,
                ..Candidate::default()
            };
            assert_eq!(cc::accepts(panel, &facts), accepted, "{id}");
        }
    }
}
