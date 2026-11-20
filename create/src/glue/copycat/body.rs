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
    // `CopycatSpecialCases`: Create's `GlassPaneBlock` variants and vanilla's panes
    // inherit from `IronBarsBlock` but are not the special bar material.
    let create_pane = create.is_some_and(|block| {
        block.name().ends_with("_window_pane")
            || matches!(
                block,
                Block::FramedGlassPane
                    | Block::HorizontalFramedGlassPane
                    | Block::VerticalFramedGlassPane
                    | Block::TiledGlassPane
            )
    });
    Some(Candidate {
        copycat: matches!(create, Some(Block::CopycatPanel | Block::CopycatStep)),
        allow: tagged("create:copycat_allow"),
        deny: tagged("create:copycat_deny"),
        // Create's own blocks can have custom block entities on non-EntityBlock templates.
        entity: class(BlockClass::Entity)? || create.is_some_and(Block::has_block_entity),
        stairs: class(BlockClass::Stair)?,
        full,
        collides,
        bars: class(BlockClass::IronBars)?
            && !class(BlockClass::StainedGlassPane)?
            && id != "minecraft:glass_pane"
            && !create_pane,
        trapdoor: class(BlockClass::TrapDoor)? && has("half") && has("open") && has("facing"),
    })
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
    use mod_api::__private::part_tests::{block_key, install_parts_fixture};
    use mod_api::prelude::*;
    use mod_api::testing::{FakeLevel, with_mods};
    use mod_api::{
        Backend, BackendRef, BlockKey, BlockStateRecord, Hooks, LevelReadError, LevelRef, ModEntry,
        Registrar, RegistrationContext, ServerHost,
    };

    const PANE: &str = "create:oak_window_pane";
    const BARS: &str = "minecraft:iron_bars";
    // Both fixture materials have a narrow, colliding shape, not full-block bounds.
    const SHAPE: [f64; 6] = [0.4375, 0.0, 0.4375, 0.5625, 1.0, 0.5625];

    struct Materials;

    #[mod_api::__subscriptions]
    impl Hooks for Materials {
        fn register(&self, r: &mut dyn Registrar, _: &RegistrationContext) {
            r.block_key(PANE);
            r.block_key(BARS);
        }
    }

    mod_api::__mod_api_impl_host_areas!(Materials, except(tags));
    mod_api::__mod_api_impl_server_host_areas!(Materials);
    mod_api::__mod_api_impl_backend_areas!(Materials, except(block_states, keys, level_collision));
    impl Host for Materials {
        fn backend(&self) -> Option<BackendRef<'_>> {
            Some(mod_api::__private::backend_ref(self))
        }
    }
    impl ServerHost for Materials {}
    impl Backend for Materials {}
    impl TagsHost for Materials {
        fn block_in_tag(&self, _: BlockKey, _: &str) -> Option<bool> {
            Some(false)
        }

        fn state_outline(&self, _: BlockState) -> Option<Vec<[f64; 6]>> {
            Some(vec![SHAPE])
        }
    }
    impl BlockStatesBackend for Materials {
        fn block_state_class(
            &self,
            _: BlockState,
            class: BlockClass,
        ) -> Result<Option<bool>, LevelReadError> {
            Ok(Some(class == BlockClass::IronBars))
        }
    }
    impl KeysBackend for Materials {
        fn block_key_id(&self, key: BlockKey) -> String {
            [PANE, BARS]
                .into_iter()
                .find(|id| block_key(id) == key)
                .expect("only the two registered fixture blocks are used")
                .into()
        }
    }
    impl LevelCollisionBackend for Materials {
        fn level_block_shape(
            &self,
            _: LevelRef,
            _: BlockState,
            _: [i32; 3],
            _: BlockShapeKind,
        ) -> Option<Vec<[f64; 6]>> {
            Some(vec![SHAPE])
        }
    }

    #[test]
    fn a_copycat_panel_accepts_iron_bars_but_not_create_window_panes() {
        with_mods(vec![].into(), || {
            install_parts_fixture(&[ModEntry::new("create", Box::new(Materials))]).unwrap();
            let lv = ServerLevel {
                host: &mut Materials,
                level: FakeLevel::default().level(),
                dim: 0,
                dimension: "minecraft:overworld",
            };
            let panel = Shape::Panel(Dir::Up);
            for (id, accepted) in [(PANE, false), (BARS, true)] {
                let state = BlockStateRecord::new(block_key(id), 0).into();
                let facts = candidate(&lv, state, [0; 3]).unwrap();
                assert_eq!(cc::accepts(panel, &facts), accepted, "{id}");
            }
        });
    }
}
