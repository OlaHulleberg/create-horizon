//! One dimension's clockwork bearings, rope pulleys and elevators: the running ones, what waits
//! for the next tick (`assembleNextTick`), the elevator columns and their contacts, structures
//! saved with a column until their block is back, and structures whose block broke. Nothing
//! here runs per tick for a block with nothing to do.

use crate::Block;
use crate::clock::Clock;
use crate::elevator::{Column, ColumnKey, Elevator};
use crate::parts::StateInfo;
use crate::pulley::Pulley;
use std::collections::{HashMap, HashSet};

/// A running clockwork bearing: its hands' angles and structures, and the speed it turns at.
#[derive(Clone, Debug, PartialEq)]
pub struct RunningClock {
    pub clock: Clock,
    pub facing: crate::kinetics::Dir,
    pub rpm: f32,
    pub hour: crate::clock::Blocks,
    pub minute: Option<(i32, crate::clock::Blocks)>,
}

/// A structure one of these blocks holds out of the world.
#[derive(Clone, Debug, PartialEq)]
pub enum Held {
    Clock(RunningClock),
    Pulley(Pulley),
    Elevator(Elevator),
}

impl Held {
    /// Where its blocks go back to (absolute positions and states): a clock's at angle 0
    /// (`disassemble` turns the hands back first), a pulley's and an elevator's at the nearest
    /// whole block.
    pub fn put_back(&self, pos: [i32; 3]) -> Vec<([i32; 3], u32)> {
        let at = |base: [i32; 3], blocks: &[([i32; 3], u32)]| -> Vec<([i32; 3], u32)> {
            blocks
                .iter()
                .map(|(o, s)| ([base[0] + o[0], base[1] + o[1], base[2] + o[2]], *s))
                .collect()
        };
        match self {
            Held::Clock(c) => {
                let mut v = at(pos, &c.hour);
                if let Some((_, m)) = &c.minute {
                    v.extend(at(pos, m));
                }
                v
            }
            Held::Pulley(p) => at(
                p.placed_at(crate::pulley::grid_offset(p.offset, p.range)),
                &p.blocks,
            ),
            Held::Elevator(e) => at(e.placed_at(), &e.blocks),
        }
    }
}

#[derive(Default)]
pub struct Lifts {
    pub clocks: HashMap<[i32; 3], RunningClock>,
    /// Bearings and pulleys to look at next tick (`assembleNextTick`).
    pub clock_checks: Vec<[i32; 3]>,
    pub pulleys: HashMap<[i32; 3], Pulley>,
    pub pulley_checks: Vec<[i32; 3]>,
    pub elevators: HashMap<[i32; 3], Elevator>,
    /// Elevators on the move (the others wait for a call), and right-clicked elevator pulleys.
    pub travelling: HashSet<[i32; 3]>,
    pub elevator_clicks: Vec<[i32; 3]>,
    pub columns: HashMap<ColumnKey, Column>,
    /// Contacts whose neighbours changed (`neighborChanged`, `updateShape`), their activations
    /// due (`scheduleActivation`: game time), and those an elevator lit (`activateBlock`).
    pub contact_checks: Vec<[i32; 3]>,
    pub contact_ticks: Vec<([i32; 3], i64)>,
    pub contact_lit: HashSet<[i32; 3]>,
    /// Structures whose block broke, to put back; blocks clients must see again.
    pub orphans: Vec<([i32; 3], Held)>,
    pub changes: Vec<[i32; 3]>,
    saved: HashMap<[i32; 3], Held>,
}

impl Lifts {
    /// A block placed, loaded or changed (`place`): a saved structure is back with its block; a
    /// contact joins its column and looks at its neighbours.
    pub fn placed(&mut self, pos: [i32; 3], block: Block, info: &StateInfo) {
        match block {
            Block::ClockworkBearing => {
                if !self.clocks.contains_key(&pos)
                    && let Some(Held::Clock(c)) = self.saved.remove(&pos)
                {
                    self.clocks.insert(pos, c);
                    self.changes.push(pos);
                    self.clock_checks.push(pos);
                }
            }
            Block::RopePulley => {
                if !self.pulleys.contains_key(&pos)
                    && let Some(Held::Pulley(p)) = self.saved.remove(&pos)
                {
                    self.pulleys.insert(pos, p);
                    self.changes.push(pos);
                }
            }
            Block::ElevatorPulley => {
                if !self.elevators.contains_key(&pos)
                    && let Some(Held::Elevator(e)) = self.saved.remove(&pos)
                {
                    let c = self.columns.entry(e.column).or_default();
                    c.active = true;
                    self.travelling.insert(pos);
                    self.elevators.insert(pos, e);
                    self.changes.push(pos);
                }
            }
            Block::ElevatorContact => {
                if let Some(f) = info.facing {
                    // `ElevatorContactBlockEntity.initialize`.
                    self.columns
                        .entry(crate::elevator::column_of(pos, f))
                        .or_default()
                        .add(pos[1]);
                }
                self.contact_checks.push(pos);
            }
            Block::RedstoneContact => self.contact_checks.push(pos),
            _ => {}
        }
    }

    /// A block changed from `old` to `new` (another block, or gone): what it held out of the
    /// world goes back (`remove` → `disassemble`); a contact leaves its column.
    pub fn changed(
        &mut self,
        pos: [i32; 3],
        old: Option<(Block, &StateInfo)>,
        new: Option<(Block, &StateInfo)>,
    ) {
        let Some((was, info)) = old else {
            return;
        };
        if new.map(|(b, _)| b) == Some(was) {
            if was == Block::ElevatorContact {
                let facing = new.and_then(|(_, i)| i.facing);
                if facing != info.facing
                    && let Some(f) = info.facing
                {
                    self.leave_column(pos, f);
                }
            }
            return;
        }
        let held = match was {
            Block::ClockworkBearing => self.clocks.remove(&pos).map(Held::Clock),
            Block::RopePulley => self.pulleys.remove(&pos).map(Held::Pulley),
            Block::ElevatorPulley => {
                self.travelling.remove(&pos);
                self.elevators.remove(&pos).map(|e| {
                    self.set_active(e.column, false);
                    Held::Elevator(e)
                })
            }
            Block::ElevatorContact => {
                if let Some(f) = info.facing {
                    self.leave_column(pos, f);
                }
                None
            }
            _ => None,
        };
        if let Some(h) = held {
            self.orphans.push((pos, h));
            self.changes.push(pos);
        }
    }

    fn leave_column(&mut self, pos: [i32; 3], facing: crate::kinetics::Dir) {
        let key = crate::elevator::column_of(pos, facing);
        if let Some(c) = self.columns.get_mut(&key) {
            c.remove(pos[1]);
            if c.is_empty() {
                self.columns.remove(&key);
            }
        }
    }

    /// `ElevatorColumn.setActive`.
    pub fn set_active(&mut self, key: ColumnKey, active: bool) {
        let c = self.columns.entry(key).or_default();
        c.active = active;
        if c.is_empty() {
            self.columns.remove(&key);
        }
    }

    /// `ElevatorColumn.target` from a call: the elevators of that column set off.
    pub fn call(&mut self, key: ColumnKey, y: i32) {
        self.columns.entry(key).or_default().target = Some(y);
        for (pos, e) in &self.elevators {
            if e.column == key {
                self.travelling.insert(*pos);
                self.changes.push(*pos);
            }
        }
    }

    /// `scheduleActivation`: a tick next game tick unless one is due.
    pub fn schedule(&mut self, pos: [i32; 3], now: i64) {
        if !self.contact_ticks.iter().any(|t| t.0 == pos) {
            self.contact_ticks.push((pos, now + 1));
        }
    }

    /// The bearing's or pulley's speed changed (`onSpeedChanged`).
    pub fn speed_changed(&mut self, pos: [i32; 3], block: Block) {
        match block {
            Block::ClockworkBearing => self.clock_checks.push(pos),
            Block::RopePulley => self.pulley_checks.push(pos),
            Block::ElevatorPulley if self.elevators.contains_key(&pos) => {
                self.travelling.insert(pos);
                self.changes.push(pos);
            }
            _ => {}
        }
    }

    /// Structures saved with a column, for when their blocks are found.
    pub fn load(&mut self, saved: impl Iterator<Item = ([i32; 3], Held)>) {
        self.saved.extend(saved);
    }

    /// The running structures of a column, to save.
    pub fn column(&self, cx: i32, cz: i32) -> Vec<([i32; 3], Held)> {
        let here = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (cx, cz);
        let mut v: Vec<([i32; 3], Held)> = self
            .clocks
            .iter()
            .filter(|(p, _)| here(p))
            .map(|(p, c)| (*p, Held::Clock(c.clone())))
            .chain(
                self.pulleys
                    .iter()
                    .filter(|(p, _)| here(p))
                    .map(|(p, x)| (*p, Held::Pulley(x.clone()))),
            )
            .chain(
                self.elevators
                    .iter()
                    .filter(|(p, _)| here(p))
                    .map(|(p, e)| (*p, Held::Elevator(e.clone()))),
            )
            .chain(
                self.saved
                    .iter()
                    .filter(|(p, _)| here(p))
                    .map(|(p, h)| (*p, h.clone())),
            )
            .collect();
        v.sort_by_key(|(p, _)| *p);
        v
    }

    /// A column left the level: what ran there leaves memory (saved with it).
    pub fn column_unloaded(&mut self, cx: i32, cz: i32) {
        let away = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) != (cx, cz);
        self.clocks.retain(|p, _| away(p));
        self.pulleys.retain(|p, _| away(p));
        let gone: Vec<ColumnKey> = self
            .elevators
            .iter()
            .filter(|(p, _)| !away(p))
            .map(|(_, e)| e.column)
            .collect();
        for key in gone {
            self.set_active(key, false);
        }
        self.elevators.retain(|p, _| away(p));
        self.travelling.retain(away);
        self.saved.retain(|p, _| away(p));
        self.clock_checks.retain(away);
        self.pulley_checks.retain(away);
        self.elevator_clicks.retain(away);
        self.contact_checks.retain(away);
        self.contact_ticks.retain(|t| away(&t.0));
        self.contact_lit.retain(away);
        // A column's contacts all lie in one column of blocks: they come back with it.
        self.columns.retain(|k, c| {
            if (k.x >> 4, k.z >> 4) == (cx, cz) {
                c.contacts.clear();
            }
            !c.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinetics::Dir;

    #[test]
    fn a_saved_structure_comes_back_with_its_block_and_a_broken_one_is_put_back() {
        let mut l = Lifts::default();
        let p = Pulley::new([0, 5, 0], 2, 50, vec![([0, 0, 0], 9)]);
        l.load([([0, 10, 0], Held::Pulley(p.clone()))].into_iter());
        assert_eq!(l.column(0, 0).len(), 1, "saved again while it waits");
        l.placed([0, 10, 0], Block::RopePulley, &StateInfo::default());
        assert_eq!(l.pulleys.get(&[0, 10, 0]), Some(&p));
        let info = StateInfo::default();
        l.changed([0, 10, 0], Some((Block::RopePulley, &info)), None);
        assert!(l.pulleys.is_empty());
        let (at, held) = &l.orphans[0];
        assert_eq!(*at, [0, 10, 0]);
        assert_eq!(held.put_back(*at), vec![([0, 5, 0], 9)]);
    }

    #[test]
    fn contacts_join_their_column_and_a_call_sets_its_elevator_off() {
        let mut l = Lifts::default();
        let west = StateInfo {
            facing: Some(Dir::West),
            ..StateInfo::default()
        };
        for y in [3, 14] {
            l.placed([1, y, 0], Block::ElevatorContact, &west);
        }
        let key = crate::elevator::column_of([1, 0, 0], Dir::West);
        assert_eq!(l.columns[&key].contacts, vec![3, 14]);
        let e = Elevator::new(20, [0, 15, 0], 4, 80, key, -1, vec![([0, 0, 0], 1)]);
        l.elevators.insert([0, 20, 0], e);
        l.call(key, 3);
        assert_eq!(l.columns[&key].target, Some(3));
        assert!(l.travelling.contains(&[0, 20, 0]));
        l.changed([1, 14, 0], Some((Block::ElevatorContact, &west)), None);
        assert_eq!(l.columns[&key].contacts, vec![3]);
    }
}
