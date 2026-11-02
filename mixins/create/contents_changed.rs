// A hopper changed a block's slots: a depot's held stack arrives or leaves.
fn contents_changed(game: &mut crate::game::Game<'_>, pos: [i32; 3]) {
    crate::game::create::contents_changed(game, pos);
    crate::game::create_packages::contents_changed(game, pos);
}
