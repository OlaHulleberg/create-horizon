// Stand-in for `mixins/create/copycat_body.rs` in the template test: an item places the state of
// its own name; the fixture's blocks have no classes or tags, their shapes and collision are the
// fixture's facts; no sounds or particles.
use super::Game;
use create_engine::Block;
use create_engine::copycat::Candidate;
use horizon_world::registry::BlockStateId;

pub(super) fn item_block(game: &Game<'_>, item: &str) -> Option<BlockStateId> {
    game.pack.assets.registry.default_state(item)
}

pub(super) fn candidate(game: &Game<'_>, state: BlockStateId) -> Candidate {
    let Some(s) = game.pack.assets.registry.state(state) else {
        return Candidate::default();
    };
    let create = s.name.strip_prefix("create:").and_then(Block::from_name);
    let outline = &game.res.facts().get(state).outline;
    let collides = game.res.behaviours().facts(state).flags
        & horizon_world::level::data::state_flags::COLLISION_EMPTY
        == 0;
    Candidate {
        copycat: matches!(create, Some(Block::CopycatPanel | Block::CopycatStep)),
        entity: create.is_some_and(Block::has_block_entity),
        full: outline.first() == Some(&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
        collides,
        ..Candidate::default()
    }
}

pub(super) fn place_sound(game: &Game<'_>, state: BlockStateId) -> Option<String> {
    let _ = (game, state);
    None
}

pub(super) fn break_particles(game: &mut Game<'_>, pos: [i32; 3], state: BlockStateId) {
    let _ = (game, pos, state);
}
