//! Create's mechanical saw facing up (`SawBlockEntity` processing): an item dropped on it is
//! cut by its cutting recipes (taking turns when several fit), then thrown off the way it turns.

use crate::processing::{Recipe, Rng, roll};

/// The saw's inventory (`ProcessingInventory`): the input, then up to 9 results.
#[derive(Clone, Debug, PartialEq)]
pub struct Saw {
    pub slots: Vec<(String, u8)>,
    pub remaining: f32,
    pub applied: bool,
    pub index: usize,
}

impl Default for Saw {
    fn default() -> Saw {
        Saw {
            slots: Vec::new(),
            remaining: -1.0,
            applied: false,
            index: 0,
        }
    }
}

/// What a tick asks of the glue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
    /// Start on the input with this many recipes fitting it (`start`).
    Start,
    /// Apply the current recipe now (`applyRecipe`).
    Apply,
    /// Let the results out (eject).
    Out,
}

impl Saw {
    /// `start`: with recipes, the next of them in turn, its time (`processing_time`) per 5 items;
    /// without, 10 ticks before the input goes on unchanged.
    pub fn start(&mut self, recipes: &[&Recipe]) {
        let count = self.slots.first().map_or(1, |s| i32::from(s.1));
        self.applied = false;
        if recipes.is_empty() {
            self.remaining = 10.0;
            return;
        }
        self.index += 1;
        if self.index >= recipes.len() {
            self.index = 0;
        }
        let time = recipes[self.index].time;
        self.remaining = (time * (count / 5).max(1)) as f32;
    }

    /// `tick` at `speed` rpm.
    pub fn tick(&mut self, speed: f32) -> Step {
        if speed == 0.0 {
            return Step::Wait;
        }
        if self.remaining == -1.0 {
            return if !self.slots.is_empty() && !self.applied {
                Step::Start
            } else {
                Step::Wait
            };
        }
        self.remaining -= (speed.abs() / 24.0).clamp(1.0, 128.0);
        if self.remaining < 5.0 && !self.applied {
            self.applied = true;
            self.remaining = 20.0;
            return Step::Apply;
        }
        if self.remaining > 0.0 {
            return Step::Wait;
        }
        self.remaining = 0.0;
        Step::Out
    }

    /// `applyRecipe` with the recipe in turn: each item rolls its results, merged into stacks of
    /// at most `max`. No recipe: the input stays as it is.
    pub fn apply(&mut self, recipe: Option<&Recipe>, max: impl Fn(&str) -> u8, rng: &mut Rng) {
        let (Some(recipe), Some((_, count))) = (recipe, self.slots.first().cloned()) else {
            return;
        };
        let mut list: Vec<(String, u8)> = Vec::new();
        for _ in 0..count {
            for (item, mut n) in roll(recipe, rng) {
                for (have, m) in list.iter_mut() {
                    if n > 0 && *have == item {
                        let moved = n.min(max(have).saturating_sub(*m));
                        *m += moved;
                        n -= moved;
                    }
                }
                if n > 0 {
                    list.push((item, n));
                }
            }
        }
        list.truncate(9);
        self.slots = list;
    }

    /// After ejecting: empty, waiting (`remainingTime = -1`).
    pub fn clear(&mut self) {
        self.slots.clear();
        self.remaining = -1.0;
    }
}

/// `getItemMovementVec`: along x unless `axis_along_first`, the sign of the speed.
pub fn movement(along_first: bool, speed: f32) -> [i32; 3] {
    let s = if speed < 0.0 { -1 } else { 1 };
    if !along_first { [s, 0, 0] } else { [0, 0, -s] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::Output;

    #[test]
    fn a_log_is_cut_into_planks_and_thrown_off() {
        let log = Recipe {
            id: "create:cutting/oak_log".into(),
            ingredients: vec![vec!["minecraft:oak_log".into()]],
            results: vec![Output {
                item: "minecraft:stripped_oak_log".into(),
                count: 1,
                chance: 1.0,
            }],
            time: 50,
        };
        let mut s = Saw {
            slots: vec![("minecraft:oak_log".into(), 2)],
            ..Saw::default()
        };
        assert_eq!(s.tick(32.0), Step::Start);
        s.start(&[&log]);
        assert_eq!(s.remaining, 50.0);
        let mut ticks = 0;
        while s.tick(32.0) != Step::Apply {
            ticks += 1;
        }
        // 32 rpm: 1.33 a tick, 50 → under 5 after 34 ticks.
        assert_eq!(ticks, 33);
        s.apply(Some(&log), |_| 64, &mut Rng::new(1));
        assert_eq!(s.slots, vec![("minecraft:stripped_oak_log".to_owned(), 2)]);
        let mut out = 0;
        while s.tick(32.0) != Step::Out {
            out += 1;
        }
        assert_eq!(out, 15);
        assert_eq!(movement(false, -8.0), [-1, 0, 0]);
    }
}
