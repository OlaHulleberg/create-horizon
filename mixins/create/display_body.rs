// What the display links read of the game beyond the hook helpers: the game's language and a
// vanilla sign's lines. Compiled against Horizon by `horizon.py check`; the template test swaps
// it for `tools/fixture/display_body.rs`.
use super::Game;

/// A lang key's text in the game's language (`Language.getOrDefault`), if it has one.
pub(super) fn lang_get(key: &str) -> Option<String> {
    horizon_client::lang::has(key).then(|| horizon_client::lang::get(key))
}

/// A sign's (`SignBlockEntity`) kind and lines: whether it hangs, its front and back.
pub(super) fn sign_at(game: &Game<'_>, pos: [i32; 3]) -> Option<(bool, [String; 4], [String; 4])> {
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(horizon_client::containers::BlockEntity::Sign(s)) => {
            Some((s.hanging, s.front.lines.clone(), s.back.lines.clone()))
        }
        _ => None,
    }
}
