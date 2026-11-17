//! Scheduled server block entity work from KineticBlockEntity.tick.
use std::collections::BTreeMap;
/// A validation ticket includes the incarnation stamp so reused node slots are ignored.
#[derive(Default)]
pub(super) struct TickScheduler {
    validation: BTreeMap<u64, Vec<(u32, u64)>>,
    reactivation: BTreeMap<u64, Vec<(u32, u64)>>,
}
impl TickScheduler {
    /// GeneratingKineticBlockEntity.tick retries a source on its next server tick.
    pub(super) fn reactivate_later(&mut self, now: u64, id: u32, stamp: u64) {
        self.reactivation
            .entry(now + 1)
            .or_default()
            .push((id, stamp));
    }
    pub(super) fn take_reactivation(&mut self, now: u64) -> Option<Vec<(u32, u64)>> {
        if self
            .reactivation
            .first_key_value()
            .is_some_and(|(&at, _)| at <= now)
        {
            self.reactivation.pop_first().map(|(_, v)| v)
        } else {
            None
        }
    }
    /// KineticBlockEntity's post-decrement countdown repeats every 61 server ticks.
    pub(super) fn validate_later(&mut self, now: u64, id: u32, stamp: u64) {
        self.validation
            .entry(now + 61)
            .or_default()
            .push((id, stamp));
    }
    /// Idle worlds inspect only the earliest deadline and allocate nothing.
    pub(super) fn take_due(&mut self, now: u64) -> Option<Vec<(u32, u64)>> {
        if self
            .validation
            .first_key_value()
            .is_some_and(|(&at, _)| at <= now)
        {
            self.validation.pop_first().map(|(_, v)| v)
        } else {
            None
        }
    }
}
