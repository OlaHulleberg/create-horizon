//! One dimension's fluid valves, smart pipe filters, hose pulleys, creative fluid tanks and
//! steam whistles: what each keeps, and the short lists of those with something to do (the only
//! ones the glue looks at in a tick).

use crate::fluids::Pointer;
use crate::hose::Hose;
use crate::tank::{Fluid, Tank};
use std::collections::HashMap;

/// A steam whistle (`WhistleBlockEntity`): its pitch, and while it sounds the ticks until its
/// loop is started again (0: silent).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Whistle {
    pub pitch: i32,
    pub sounding: bool,
    pub replay_in: u32,
}

#[derive(Default)]
pub struct Plumbing {
    /// FluidValveBlockEntity pointers: every valve ticks, including stationary ones.
    pub valves: HashMap<[i32; 3], Pointer>,
    /// Smart fluid pipes' filters (`FilteringBehaviour.forFluids`), saved with their column;
    /// those saved for pipes not loaded yet are kept as they are.
    pub filters: HashMap<[i32; 3], crate::fluids::SmartFilter>,
    /// Smart pipes whose filter clients must draw again.
    pub filter_changes: Vec<[i32; 3]>,
    /// Hose pulleys, those moving or searching (the only ones that tick), the speeds they were
    /// given in a solve (their `onSpeedChanged` reads the world: the glue's), and the blocks
    /// under still hoses turning forwards that wake them when they change (`lazyTick`).
    pub hoses: HashMap<[i32; 3], Hose>,
    pub hose_busy: Vec<[i32; 3]>,
    pub hose_speeds: Vec<([i32; 3], f32)>,
    pub hose_waits: HashMap<[i32; 3], [i32; 3]>,
    /// What was saved for hoses not loaded yet: offset and tank.
    pub hose_saved: HashMap<[i32; 3], (f32, Tank)>,
    /// Hoses whose hose clients must draw again.
    pub hose_changes: Vec<[i32; 3]>,
    /// Creative fluid tanks and the fluid each gives without end (saved), and those clients must
    /// draw again.
    pub creative: HashMap<[i32; 3], Option<Fluid>>,
    pub creative_saved: HashMap<[i32; 3], Option<Fluid>>,
    pub creative_changes: Vec<[i32; 3]>,
    /// Steam whistles, those whose pitch is counted again (`queuePitchUpdate`), and those
    /// sounding (the only ones that tick).
    pub whistles: HashMap<[i32; 3], Whistle>,
    pub whistle_checks: Vec<[i32; 3]>,
    pub whistling: Vec<[i32; 3]>,
    /// Whistles that may start or stop sounding (placed or powered, their boiler changed).
    pub sound_checks: Vec<[i32; 3]>,
    /// Whistle extensions placed, changed or beside a change: their root's pitch, their shape
    /// and whether they still stand are looked at (`onPlace`, `updateShape`).
    pub extension_checks: Vec<[i32; 3]>,
    /// Whistles whose tank may be gone (`updateShape`: they break).
    pub whistle_support: Vec<[i32; 3]>,
}

impl Plumbing {
    /// FluidValveBlockEntity's constructor starts at zero, chasing zero at zero speed.
    /// Changes of block state preserve the entity; ENABLED follows the pointer in tick.
    pub fn valve_placed(&mut self, pos: [i32; 3]) {
        self.valves.entry(pos).or_default();
    }

    /// `FluidValveBlockEntity.onSpeedChanged`.
    pub fn valve_speed(&mut self, pos: [i32; 3], speed: f32) {
        let Some(p) = self.valves.get_mut(&pos) else {
            return;
        };
        p.speed_changed(speed);
    }

    /// FluidValveBlockEntity.tick: tickChaser, then reconcile ENABLED at both endpoints,
    /// even when stationary. Intermediate pointer values leave ENABLED unchanged.
    pub fn tick_valves(
        &mut self,
        open: impl Fn([i32; 3]) -> Option<bool>,
        mut changed: impl FnMut([i32; 3]),
    ) -> Vec<([i32; 3], bool)> {
        let mut out = Vec::new();
        for (pos, pointer) in &mut self.valves {
            if let Some(enabled) = open(*pos) {
                let before = *pointer;
                if let Some(next) = pointer.tick(enabled) {
                    out.push((*pos, next));
                }
                if *pointer != before {
                    changed(*pos);
                }
            }
        }
        out
    }

    /// A block left `pos`: whatever it kept goes.
    pub fn removed(&mut self, pos: [i32; 3]) -> bool {
        let mut saved = false;
        self.valves.remove(&pos);
        if self.filters.remove(&pos).is_some() {
            self.filter_changes.push(pos);
            saved = true;
        }
        if self.hoses.remove(&pos).is_some() {
            self.hose_busy.retain(|p| *p != pos);
            self.hose_waits.retain(|_, h| *h != pos);
            self.hose_changes.push(pos);
            saved = true;
        }
        if self.creative.remove(&pos).is_some() {
            self.creative_changes.push(pos);
            saved = true;
        }
        if self.whistles.remove(&pos).is_some() {
            self.whistling.retain(|p| *p != pos);
        }
        saved
    }

    /// A column unloaded: what is kept for its blocks is saved already.
    pub fn column_unloaded(&mut self, cx: i32, cz: i32) {
        let out = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) != (cx, cz);
        self.valves.retain(|p, _| out(p));
        self.filters.retain(|p, _| out(p));
        self.hose_saved.retain(|p, _| out(p));
        self.creative_saved.retain(|p, _| out(p));
        self.hoses.retain(|p, _| out(p));
        self.hose_busy.retain(out);
        self.hose_waits.retain(|_, h| out(h));
        self.creative.retain(|p, _| out(p));
        self.whistles.retain(|p, _| out(p));
        self.whistling.retain(out);
    }

    /// The fluid a creative tank at `pos` gives, if it is one.
    pub fn creative_fluid(&self, pos: [i32; 3]) -> Option<Option<Fluid>> {
        self.creative.get(&pos).copied()
    }

    /// A hose that has something to do ticks.
    pub fn hose_busy(&mut self, pos: [i32; 3]) {
        if self.hoses.get(&pos).is_some_and(Hose::busy) && !self.hose_busy.contains(&pos) {
            self.hose_busy.push(pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_valve_starts_at_zero_and_closes_without_rotation() {
        let pos = [0, 0, 0];
        let mut p = Plumbing::default();
        p.valve_placed(pos);
        assert_eq!(p.valves[&pos], Pointer::default());
        assert_eq!(p.tick_valves(|_| Some(true), |_| {}), [(pos, false)]);
        assert!(p.tick_valves(|_| Some(false), |_| {}).is_empty());
    }

    #[test]
    fn stationary_valve_endpoints_drive_enabled_after_state_changes() {
        let pos = [0, 0, 0];
        let mut p = Plumbing::default();
        p.valve_placed(pos);
        p.valve_speed(pos, 64.0);
        for expected in [0.2, 0.4, 0.6, 0.8] {
            assert!(p.tick_valves(|_| Some(false), |_| {}).is_empty());
            assert!((p.valves[&pos].value - expected).abs() < 1e-6);
        }
        assert_eq!(p.tick_valves(|_| Some(false), |_| {}), [(pos, true)]);
        p.valve_speed(pos, 0.0);
        p.valve_placed(pos);
        assert_eq!(p.valves[&pos].value, 1.0);
        assert_eq!(p.tick_valves(|_| Some(false), |_| {}), [(pos, true)]);
        p.valve_speed(pos, -320.0);
        assert_eq!(p.tick_valves(|_| Some(true), |_| {}), [(pos, false)]);
        assert!(!p.removed(pos));
        assert!(p.valves.is_empty());
    }
}
