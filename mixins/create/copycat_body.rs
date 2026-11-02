// What copycats read of the game beyond the hook helpers: the block an item places, what
// `CopycatBlock.getAcceptedBlockState` tests of it (its classes, tags, shapes and block entity),
// its place sound, and a block's break particles. Compiled against Horizon by `horizon.py check`;
// the template test swaps it for `tools/fixture/copycat_body.rs`.
use super::Game;
use create_engine::Block;
use create_engine::copycat::Candidate;
use horizon_world::level::classes::Class;
use horizon_world::level::data::state_flags;
use horizon_world::registry::BlockStateId;

/// The default state of the block `item` places (`BlockItem.getBlock().defaultBlockState()`).
pub(super) fn item_block(game: &Game<'_>, item: &str) -> Option<BlockStateId> {
    let state = game.items().get(item)?.block?;
    let registry = game.pack.assets.registry;
    registry.default_state(&registry.state(state)?.name)
}

/// `getAcceptedBlockState`'s tests and `CopycatSpecialCases` of `state`.
pub(super) fn candidate(game: &Game<'_>, state: BlockStateId) -> Candidate {
    let registry = game.pack.assets.registry;
    let Some(s) = registry.state(state) else {
        return Candidate::default();
    };
    let b = game.res.behaviours();
    let classes = b.data(state).classes;
    let flags = b.facts(state).flags;
    let tagged = |t: &str| b.tag(t).is_some_and(|set| set.contains(b.block(state)));
    let create = s.name.strip_prefix("create:").and_then(Block::from_name);
    // `getShape(level, pos).bounds()` is the whole block.
    let outline = &game.res.facts().get(state).outline;
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for o in outline.iter() {
        for i in 0..3 {
            lo[i] = lo[i].min(o[i]);
            hi[i] = hi[i].max(o[i + 3]);
        }
    }
    let full = !outline.is_empty() && lo == [0.0; 3] && hi == [1.0; 3];
    let collides = match b.collision_shape(state) {
        Some(shape) => !shape.is_empty(),
        None => flags & state_flags::COLLISION_EMPTY == 0,
    };
    let has = |k: &str| s.properties.contains_key(k);
    Candidate {
        copycat: matches!(create, Some(Block::CopycatPanel | Block::CopycatStep)),
        allow: tagged("create:copycat_allow"),
        deny: tagged("create:copycat_deny"),
        entity: flags & state_flags::BLOCK_ENTITY != 0
            || create.is_some_and(Block::has_block_entity),
        stairs: classes.contains(Class::Stair),
        full,
        collides,
        // An `IronBarsBlock` but no glass pane (vanilla's, stained or Create's).
        bars: classes.contains(Class::IronBars)
            && !classes.contains(Class::StainedGlassPane)
            && !s.name.ends_with("_pane"),
        trapdoor: classes.contains(Class::TrapDoor) && has("half") && has("open") && has("facing"),
    }
}

/// `getSoundType().getPlaceSound()` of `state`.
pub(super) fn place_sound(game: &Game<'_>, state: BlockStateId) -> Option<String> {
    let place = &game.res.facts().get(state).sounds.place;
    (!place.is_empty()).then(|| place.to_string())
}

/// `LevelEvent.PARTICLES_DESTROY_BLOCK` (2001) of `state` at `pos`.
pub(super) fn break_particles(game: &mut Game<'_>, pos: [i32; 3], state: BlockStateId) {
    game.level_event_at(2001, pos, state.0 as i32);
}
