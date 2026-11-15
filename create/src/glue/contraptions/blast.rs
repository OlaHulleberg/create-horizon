//! Create's blasts (`Level.explode` on the server), for the nozzle and the mysterious cuckoo
//! clock. The surprise's smoke on clients (`LARGE_SMOKE` about the clock) is the cuckoo clock's
//! own client effect, drawn by its glue.

use super::super::server::ServerLevel;
use mod_api::{ExplosionInteraction, ExplosionSettings};

impl ServerLevel<'_> {
    /// `Level.explode(null, x, y, z, power, ExplosionInteraction.NONE)`: entities hurt and
    /// pushed, no block broken, no fire; the explosion's sound and particles.
    pub fn explode_sparing_blocks(&mut self, center: [f64; 3], power: f32) {
        let done = self.level.explode(
            &mut *self.host,
            None,
            center,
            power,
            false,
            ExplosionInteraction::None,
            ExplosionSettings::default(),
        );
        if let Err(e) = done {
            crate::create_log!("[create] an explosion at {center:?} failed: {e:?}");
        }
    }

    /// The mysterious cuckoo clock's surprise at its progress 50: `level.destroyBlock(pos,
    /// false)` (the break effects, no drops), then `level.explode(.., 3, false,
    /// ExplosionInteraction.BLOCK)` at its centre (blocks broken as the
    /// `block_explosion_drop_decay` rule says). The damage is vanilla's explosion: Create's
    /// `cuckoo_surprise` damage type is not carried.
    pub fn cuckoo_surprise(&mut self, pos: [i32; 3]) {
        if !self.destroy_block(pos, false) {
            crate::create_log!("[create] the cuckoo clock at {pos:?} could not be broken");
            return;
        }
        let center = pos.map(|v| f64::from(v) + 0.5);
        let done = self.level.explode(
            &mut *self.host,
            None,
            center,
            3.0,
            false,
            ExplosionInteraction::Block,
            ExplosionSettings::default(),
        );
        if let Err(e) = done {
            crate::create_log!("[create] an explosion at {center:?} failed: {e:?}");
        }
    }
}
