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

/// The mysterious cuckoo clock's surprise at its progress 50: `level.destroyBlock(pos, false)`
/// (the break effects, no drops), then `level.explode(.., 3, false, ExplosionInteraction.BLOCK)`
/// at its centre (blocks broken as the `block_explosion_drop_decay` rule says). The damage is
/// vanilla's explosion: Create's `cuckoo_surprise` damage type is not carried.
pub(super) fn cuckoo_surprise(game: &mut Game<'_>, pos: [i32; 3]) {
    let state = game.world.block(pos[0], pos[1], pos[2]);
    game.level_event_at(2001, pos, state.0 as i32);
    let air = horizon_world::registry::BlockStateId(0);
    if let Err(e) = game.level_set(pos, air) {
        eprintln!("[create] the cuckoo clock at {pos:?} could not be broken: {e}");
        return;
    }
    let interaction = game.destroy_type("block_explosion_drop_decay");
    let center = pos.map(|v| f64::from(v) + 0.5);
    if let Err(e) = game.server_explode(center, 3.0, interaction, false) {
        eprintln!("[create] an explosion at {center:?} failed: {e}");
    }
}

/// The surprise's smoke on a client (`ParticleTypes.LARGE_SMOKE` within half a block of the
/// clock's centre, `VecHelper.offsetRandomly`).
pub(super) fn cuckoo_smoke(game: &mut Game<'_>, center: [f64; 3]) {
    use horizon_client::particles::{ParticleOptions, ParticleType};
    use horizon_core::random::RandomSource;
    let mut at = center;
    for v in &mut at {
        *v += f64::from((game.world.random.next_float() - 0.5) * 2.0 * 0.5);
    }
    game.particles.add(
        &ParticleOptions::simple(ParticleType::LargeSmoke),
        at,
        [0.0; 3],
    );
}
