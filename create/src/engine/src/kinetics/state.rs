//! Create's base/KineticBlockEntity and GeneratingKineticBlockEntity state.
use super::Context;

/// SmartBlockEntity's initialization and the last engine base-tick pass.
/// Source and fluid subclasses may run this base phase before the final solve.
#[derive(Clone, Copy, Debug, Default)]
pub(super) enum Lifecycle {
    #[default]
    New,
    Ticked(u64),
}
impl Lifecycle {
    pub(super) fn is_new(self) -> bool {
        matches!(self, Self::New)
    }
    pub(super) fn ran(self, pass: u64) -> bool {
        matches!(self, Self::Ticked(last) if last == pass)
    }
}

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
    /// KineticBlockEntity caches, written verbatim by write.
    pub(super) network_size: u32,
    pub(super) last_stress: f32,
    pub(super) last_capacity: f32,
    /// The sequence context it took from what turns it (`copySequenceContextFrom`): its
    /// holder's, and the value per RPM of its own speed (`SequenceContext.relativeValue`).
    pub(super) seq: Option<(Context, f32)>,
    /// `KineticBlockEntity.flickerTally` as of tick `flicker_at`: 5 more each time its speed
    /// starts, stops or turns round (`onSpeedChanged`), one less each tick (`tick`).
    pub(super) flicker: i32,
    pub(super) flicker_at: u64,
    /// KineticBlockEntity.network: identity inherited from the immediate source.
    pub(super) net: Option<i64>,
    /// Added since the last solve (not a replaced part): a block entity that has not ticked yet.
    pub(super) lifecycle: Lifecycle,
    /// KineticBlockEntity.updateSpeed: attach on the next server tick.
    pub(super) update_speed: bool,
    pub(super) source: Option<PartLocation>,
    pub(super) reactivate_source: bool,
    /// ChainGearshiftBlockEntity.signalChanged: input applied after the base tick.
    pub(super) chain_signal: Option<f32>,
}
impl Default for KineticState {
    fn default() -> Self {
        Self {
            speed: 0.0,
            theoretical: 0.0,
            overstressed: false,
            stress: 0.0,
            capacity: 0.0,
            network_size: 0,
            last_stress: 0.0,
            last_capacity: 0.0,
            seq: None,
            flicker: 0,
            flicker_at: 0,
            net: None,
            lifecycle: Lifecycle::New,
            update_speed: true,
            source: None,
            reactivate_source: false,
            chain_signal: None,
        }
    }
}
impl KineticState {
    /// KineticBlockEntity.tick's linear flicker decay, evaluated lazily.
    pub(super) fn flicker(&self, now: u64) -> i32 {
        (self.flicker - now.saturating_sub(self.flicker_at).min(i32::MAX as u64) as i32).max(0)
    }
}

/// A source block position, with the engine port for split shaft halves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartLocation {
    pub pos: [i32; 3],
    pub sub: u8,
}
/// KineticBlockEntity.write/read's Network compound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedNetwork {
    pub id: i64,
    pub stress: f32,
    pub capacity: f32,
    pub size: u32,
    pub added_stress: f32,
    pub added_capacity: f32,
}
/// KineticBlockEntity.write/read's persistent kinetic fields. Displayed speed is derived.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedKineticState {
    pub speed: f32,
    pub sequence: Option<(Context, f32)>,
    pub source: Option<PartLocation>,
    pub network: Option<SavedNetwork>,
    pub needs_speed_update: bool,
}
