// What Create's blasts reach of the game (`Level.explode` on the server); the template test
// swaps it for `tools/fixture/blast_body.rs`.
use super::Game;

/// `Level.explode(null, x, y, z, power, ExplosionInteraction.NONE)`: entities hurt and pushed,
/// no block broken, no fire; the explosion's sound and particles.
pub(super) fn explode_sparing_blocks(game: &mut Game<'_>, center: [f64; 3], power: f32) {
    if let Err(e) = game.server_explode(
        center,
        power,
        super::explosion::BlockInteraction::Keep,
        false,
    ) {
        eprintln!("[create] an explosion at {center:?} failed: {e}");
    }
}
