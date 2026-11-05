// Stand-in for `mixins/create/blast_body.rs` in the template test: the fixture has no
// explosions.
use super::Game;

pub(super) fn explode_sparing_blocks(game: &mut Game<'_>, center: [f64; 3], power: f32) {
    let _ = (game, center, power);
}
