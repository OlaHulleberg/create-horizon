// Game glue for the Create port, injected as `crate::game::create`. Rules live in the
// `create_engine` crate; this module only turns the game's hooks into engine calls.

use super::Game;
use super::native_lifecycle::WorldPhase;

/// World open: find Create's block states so hot hooks can test them with one compare.
/// World close: forget them, so a world without Create pays nothing.
pub(crate) fn world_lifecycle(game: &mut Game<'_>, phase: &WorldPhase) {
    match phase {
        WorldPhase::WorldOpenReady => {
            let registry = game.pack.assets.registry;
            let names = (0..registry.len()).filter_map(|i| {
                registry
                    .state(horizon_world::registry::BlockStateId(i as u32))
                    .map(|s| s.name.as_str())
            });
            match create_engine::states::find_range(names) {
                Some((first, len)) => {
                    create_engine::states::set_range(first, len);
                    eprintln!("[create] {len} Create block states from id {first}");
                }
                None => {
                    create_engine::states::clear_range();
                    eprintln!("[create] no Create block states: the mod stays idle");
                }
            }
        }
        WorldPhase::WorldCloseFinal | WorldPhase::WorldOpenFailed | WorldPhase::WorldCloseFailed => {
            create_engine::states::clear_range();
        }
    }
}
