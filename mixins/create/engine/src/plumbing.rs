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
    /// Fluid valves' pointers, and the valves whose pointer moves (the only ones that tick).
    pub valves: HashMap<[i32; 3], Pointer>,
    pub valve_moving: Vec<[i32; 3]>,
    /// Smart fluid pipes' filters (`FilteringBehaviour.forFluids`), saved with their column;
    /// those saved for pipes not loaded yet are kept as they are.
    pub filters: HashMap<[i32; 3], Fluid>,
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
    /// A valve placed or loaded: its pointer starts where its block is.
    pub fn valve_placed(&mut self, pos: [i32; 3], open: bool) {
        self.valves.entry(pos).or_insert_with(|| Pointer::at(open));
    }

    /// `FluidValveBlockEntity.onSpeedChanged`.
    pub fn valve_speed(&mut self, pos: [i32; 3], speed: f32) {
        let Some(p) = self.valves.get_mut(&pos) else {
            return;
        };
        p.speed_changed(speed);
        if p.moving() && !self.valve_moving.contains(&pos) {
            self.valve_moving.push(pos);
        }
    }

    /// One tick of the moving valves (`FluidValveBlockEntity.tick`): `open` reads a valve's
    /// `enabled`; the valves whose `enabled` must change, with the new value. A valve whose
    /// pointer stopped leaves the list.
    pub fn tick_valves(
        &mut self,
        open: impl Fn([i32; 3]) -> Option<bool>,
    ) -> Vec<([i32; 3], bool)> {
        let mut out = Vec::new();
        let valves = &mut self.valves;
        self.valve_moving.retain(|pos| {
            let (Some(p), Some(o)) = (valves.get_mut(pos), open(*pos)) else {
                return false;
            };
            if let Some(n) = p.tick(o) {
                out.push((*pos, n));
            }
            p.moving()
        });
        out
    }

    /// A block left `pos`: whatever it kept goes.
    pub fn removed(&mut self, pos: [i32; 3]) -> bool {
        let mut saved = false;
        if self.valves.remove(&pos).is_some() {
            self.valve_moving.retain(|p| *p != pos);
        }
        saved |= self.filters.remove(&pos).is_some();
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
        self.valve_moving.retain(out);
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
    fn only_moving_valves_tick() {
        let mut p = Plumbing::default();
        p.valve_placed([0, 0, 0], false);
        assert!(p.valve_moving.is_empty());
        p.valve_speed([0, 0, 0], 64.0);
        assert_eq!(p.valve_moving, vec![[0, 0, 0]]);
        let mut changes = Vec::new();
        for _ in 0..5 {
            changes.extend(p.tick_valves(|_| Some(false)));
        }
        // 64 RPM: 0.2 a tick, open after 5 ticks; then still.
        assert_eq!(changes, vec![([0, 0, 0], true)]);
        assert!(p.valve_moving.is_empty());
        assert!(!p.removed([0, 0, 0]));
        assert!(p.valves.is_empty());
    }
}
