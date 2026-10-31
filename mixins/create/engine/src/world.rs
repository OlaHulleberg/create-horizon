//! Create's state for one open world: what each Create block state is (decoded once), and each
//! dimension's kinetic network. The game glue feeds it block changes and chunk loads; nothing here
//! runs per tick unless something changed.

use crate::Block;
use crate::kinetics::{Event, Kinetics, SolveStats};
use crate::parts::{Settings, StateInfo, is_kinetic, parts};
use crate::states;
use std::collections::HashMap;

/// Every Create block state, by `state - first`: its block, its decoded properties, and whether it
/// holds kinetic parts. Built once when a world opens, never per tick.
pub struct StateTable {
    first: u32,
    block: Vec<Block>,
    info: Vec<StateInfo>,
    kinetic: Vec<bool>,
}

impl StateTable {
    /// From the registry's states `first..first + n`: each name (`create:shaft`) and properties.
    pub fn new<'a, P>(first: u32, states: impl Iterator<Item = (&'a str, P)>) -> StateTable
    where
        P: Iterator<Item = (&'a str, &'a str)>,
    {
        let mut t = StateTable {
            first,
            block: Vec::new(),
            info: Vec::new(),
            kinetic: Vec::new(),
        };
        for (name, props) in states {
            let block = name
                .strip_prefix("create:")
                .and_then(Block::from_name)
                // A Create id this build doesn't know (another version's): inert.
                .unwrap_or(Block::AndesiteCasing);
            t.kinetic.push(is_kinetic(block));
            t.block.push(block);
            t.info.push(StateInfo::read(props));
        }
        t
    }

    /// The Create block of a state id, if it is one.
    #[inline]
    pub fn get(&self, state: u32) -> Option<(Block, &StateInfo)> {
        let i = states::index(state)? as usize;
        Some((*self.block.get(i)?, self.info.get(i)?))
    }

    /// Whether a state is a kinetic Create block: the bit test of the hot hooks.
    #[inline]
    pub fn is_kinetic(&self, state: u32) -> bool {
        states::index(state).is_some_and(|i| self.kinetic.get(i as usize).copied().unwrap_or(false))
    }

    pub fn first(&self) -> u32 {
        self.first
    }

    pub fn len(&self) -> usize {
        self.block.len()
    }

    pub fn is_empty(&self) -> bool {
        self.block.is_empty()
    }
}

/// One dimension's Create state.
#[derive(Default)]
pub struct Dimension {
    pub kinetics: Kinetics,
    /// Kinetic block positions per loaded column, to forget them when it unloads.
    columns: HashMap<(i32, i32), Vec<[i32; 3]>>,
    /// Block entity settings that change a part (a motor's speed), by position.
    settings: HashMap<[i32; 3], Settings>,
}

/// Create's state for the open world: three vanilla dimensions, by index.
pub struct CreateWorld {
    pub table: StateTable,
    pub dims: [Dimension; 3],
}

/// `minecraft:overworld` 0, `minecraft:the_nether` 1, `minecraft:the_end` 2.
pub fn dimension_index(id: &str) -> Option<usize> {
    match id {
        "minecraft:overworld" => Some(0),
        "minecraft:the_nether" => Some(1),
        "minecraft:the_end" => Some(2),
        _ => None,
    }
}

impl CreateWorld {
    pub fn new(table: StateTable) -> CreateWorld {
        CreateWorld {
            table,
            dims: Default::default(),
        }
    }

    fn place(&mut self, dim: usize, pos: [i32; 3], state: u32) {
        let Some((block, info)) = self.table.get(state) else {
            return;
        };
        let d = &mut self.dims[dim];
        let settings = d.settings.get(&pos).copied().unwrap_or_default();
        let [a, b] = parts(block, info, &settings);
        d.kinetics.remove(pos);
        if let Some(a) = a {
            d.kinetics.set(pos, 0, a);
            d.columns
                .entry((pos[0] >> 4, pos[2] >> 4))
                .or_default()
                .push(pos);
        }
        if let Some(b) = b {
            d.kinetics.set(pos, 1, b);
        }
    }

    /// A committed block change (`native_block_changed`). The first lines are the hot path: two
    /// range tests that miss for every non-Create block.
    #[inline]
    pub fn block_changed(&mut self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        let (was, is) = (self.table.is_kinetic(old), self.table.is_kinetic(new));
        if !was && !is {
            return;
        }
        if is {
            self.place(dim, pos, new);
        } else {
            let d = &mut self.dims[dim];
            d.kinetics.remove(pos);
            d.settings.remove(&pos);
            if let Some(list) = d.columns.get_mut(&(pos[0] >> 4, pos[2] >> 4)) {
                list.retain(|p| *p != pos);
            }
        }
    }

    /// One section of a column that joined the level: `base` is its lowest corner, `states` its
    /// 4,096 states in vanilla's index order (`y << 8 | z << 4 | x`).
    pub fn section_loaded(&mut self, dim: usize, base: [i32; 3], states: &[u32; 4096]) {
        for (i, &s) in states.iter().enumerate() {
            if self.table.is_kinetic(s) {
                let pos = [
                    base[0] + (i & 15) as i32,
                    base[1] + (i >> 8) as i32,
                    base[2] + ((i >> 4) & 15) as i32,
                ];
                self.place(dim, pos, s);
            }
        }
    }

    /// A column left the level: its parts leave the network (their neighbours re-solve).
    pub fn column_unloaded(&mut self, dim: usize, cx: i32, cz: i32) {
        let d = &mut self.dims[dim];
        if let Some(list) = d.columns.remove(&(cx, cz)) {
            for pos in list {
                d.kinetics.remove(pos);
            }
        }
    }

    /// A block entity setting that changes a part (a motor's speed), then re-places the block.
    pub fn set_settings(&mut self, dim: usize, pos: [i32; 3], state: u32, settings: Settings) {
        self.dims[dim].settings.insert(pos, settings);
        if self.table.is_kinetic(state) {
            self.place(dim, pos, state);
        }
    }

    /// Re-solves what changed in a dimension: nothing at all when nothing did.
    #[inline]
    pub fn tick(&mut self, dim: usize, events: &mut Vec<Event>) -> SolveStats {
        let k = &mut self.dims[dim].kinetics;
        if !k.has_dirty() {
            return SolveStats::default();
        }
        k.solve(events)
    }

    pub fn speed(&self, dim: usize, pos: [i32; 3]) -> Option<f32> {
        self.dims[dim].kinetics.speed(pos, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny registry: two vanilla states, then Create's shaft (x, y, z) and creative motor
    /// facing east and west.
    fn world() -> CreateWorld {
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:shaft", vec![("axis", "x")]),
            ("create:shaft", vec![("axis", "y")]),
            ("create:shaft", vec![("axis", "z")]),
            ("create:creative_motor", vec![("facing", "east")]),
            ("create:creative_motor", vec![("facing", "west")]),
            ("create:andesite_casing", vec![]),
        ];
        states::set_range(2, names.len() as u32);
        let table = StateTable::new(2, names.iter().map(|(n, p)| (*n, p.iter().copied())));
        CreateWorld::new(table)
    }

    #[test]
    fn worlds_solve_on_change_and_pay_nothing_when_steady() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = world();
        let (shaft_x, motor_east, casing) = (2, 5, 7);
        assert!(!w.table.is_kinetic(0) && !w.table.is_kinetic(casing));
        // Vanilla changes miss at once.
        w.block_changed(0, [5, 5, 5], 0, 1);
        assert_eq!(w.tick(0, &mut Vec::new()), SolveStats::default());
        w.block_changed(0, [0, 64, 0], 0, motor_east);
        w.block_changed(0, [1, 64, 0], 0, shaft_x);
        let mut e = Vec::new();
        assert_eq!(w.tick(0, &mut e).components, 1);
        assert_eq!(w.speed(0, [1, 64, 0]), Some(16.0));
        assert_eq!(w.tick(0, &mut e), SolveStats::default(), "steady: no work");
        // The motor's speed setting re-solves it.
        w.set_settings(0, [0, 64, 0], motor_east, Settings { motor_rpm: 64.0 });
        w.tick(0, &mut e);
        assert_eq!(w.speed(0, [1, 64, 0]), Some(64.0));
        // Breaking the motor stops the shaft.
        w.block_changed(0, [0, 64, 0], motor_east, 0);
        w.tick(0, &mut e);
        assert_eq!(w.speed(0, [1, 64, 0]), Some(0.0));
        states::clear_range();
    }

    #[test]
    fn loaded_sections_join_and_unloaded_columns_leave() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = world();
        let mut states = [0u32; 4096];
        // x 0..3 at y 0, z 0 of a section at (16, 64, 32): a motor then two shafts.
        states[0] = 5;
        states[1] = 2;
        states[2] = 2;
        w.section_loaded(0, [16, 64, 32], &states);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [18, 64, 32]), Some(16.0));
        w.column_unloaded(0, 1, 2);
        assert_eq!(w.speed(0, [18, 64, 32]), None);
        assert!(w.dims[0].kinetics.is_empty());
        states::clear_range();
    }
}
