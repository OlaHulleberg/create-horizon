//! Create's base/KineticBlockEntity and GeneratingKineticBlockEntity state.
use super::{Context, NONE};

/// Runtime information belonging to one kinetic block entity, independent of its block part.
#[derive(Clone, Debug)]
pub(super) struct KineticState {
    /// The speed it shows now (0 when overstressed).
    pub(super) speed: f32,
    /// The speed its network turns it at, overstressed or not (`getTheoreticalSpeed`).
    pub(super) theoretical: f32,
    pub(super) overstressed: bool,
    /// The network's stress and capacity at the last solve (`KineticBlockEntity.stress`,
    /// `capacity`).
    pub(super) stress: f32,
    pub(super) capacity: f32,
    /// The sequence context it took from what turns it (`copySequenceContextFrom`): its
    /// holder's, and the value per RPM of its own speed (`SequenceContext.relativeValue`).
    pub(super) seq: Option<(Context, f32)>,
    /// `KineticBlockEntity.flickerTally` as of tick `flicker_at`: 5 more each time its speed
    /// starts, stops or turns round (`onSpeedChanged`), one less each tick (`tick`).
    pub(super) flicker: i32,
    pub(super) flicker_at: u64,
    /// KineticBlockEntity.network: identity inherited from the immediate source.
    pub(super) net: u32,
    /// Added since the last solve (not a replaced part): a block entity that has not ticked yet.
    pub(super) new_part: bool,
    pub(super) source: u32,
    pub(super) reactivate_source: bool,
}
impl Default for KineticState {
    fn default() -> Self {
        Self {
            speed: 0.0,
            theoretical: 0.0,
            overstressed: false,
            stress: 0.0,
            capacity: 0.0,
            seq: None,
            flicker: 0,
            flicker_at: 0,
            net: NONE,
            new_part: true,
            source: NONE,
            reactivate_source: false,
        }
    }
}
impl KineticState {
    /// KineticBlockEntity.tick's linear flicker decay, evaluated lazily.
    pub(super) fn flicker(&self, now: u64) -> i32 {
        (self.flicker - now.saturating_sub(self.flicker_at).min(i32::MAX as u64) as i32).max(0)
    }
}
