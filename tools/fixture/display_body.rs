// Stand-in for `mixins/create/display_body.rs` in the template test: no language is loaded (the
// engine's en_us answers), and signs are what the test puts in `SIGNS`.
use super::Game;

thread_local! {
    /// Signs by position: (hanging, front, back).
    pub(crate) static SIGNS: std::cell::RefCell<std::collections::HashMap<[i32; 3], (bool, [String; 4], [String; 4])>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

pub(super) fn lang_get(key: &str) -> Option<String> {
    let _ = key;
    None
}

pub(super) fn sign_at(game: &Game<'_>, pos: [i32; 3]) -> Option<(bool, [String; 4], [String; 4])> {
    let _ = game;
    SIGNS.with_borrow(|s| s.get(&pos).cloned())
}
