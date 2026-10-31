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
            t.kinetic.push(is_kinetic(block) || has_state(block));
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

    /// Whether a state is a Create block this keeps (kinetic, or with state of its own like a
    /// depot): the bit test of the hot hooks.
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
    /// The state of each kinetic block, by position.
    states: HashMap<[i32; 3], u32>,
    /// Columns whose saved data changed, to mark unsaved when the dimension is in place.
    unsaved: Vec<(i32, i32)>,
    /// Hand cranks being turned: ticks left (`HandCrankBlockEntity.inUse`). Empty at rest, so
    /// cranks cost nothing per tick unless turned.
    cranks: HashMap<[i32; 3], u8>,
    /// Millstones and their state (`MillstoneBlockEntity.timer`, last recipe).
    pub millstones: HashMap<[i32; 3], crate::processing::Millstone>,
    /// Water wheels, to look at their flow every 60 ticks (`lazyTick`), staggered by position.
    wheels: Vec<[i32; 3]>,
    /// Water wheels to look at now: placed, or a neighbour changed.
    wheel_checks: Vec<[i32; 3]>,
    /// Blocks whose redstone power may have changed (a clutch or gearshift that was placed or
    /// whose neighbour changed), checked after the level's tick.
    redstone: Vec<[i32; 3]>,
    /// Depots and their held stack's motion; the stacks are in their block entities' slots.
    pub depots: HashMap<[i32; 3], Option<crate::depot::Held>>,
    /// Depots whose held stack still slides: the only ones that tick.
    pub sliding: Vec<[i32; 3]>,
    /// Depots whose contents clients must see again (put down, taken, filled by a hopper).
    pub depot_changes: Vec<[i32; 3]>,
}

/// Blocks that keep state of their own here but are not kinetic: depots.
fn has_state(block: Block) -> bool {
    matches!(block, Block::Depot)
}

/// Blocks that follow redstone power (`powered`): Create's split shafts.
fn follows_power(block: Block) -> bool {
    matches!(block, Block::Clutch | Block::Gearshift)
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
        if block == Block::Millstone {
            d.millstones.entry(pos).or_default();
        }
        if block == Block::Depot && !d.depots.contains_key(&pos) {
            // Placed or loaded: clients draw what its block entity holds.
            d.depots.insert(pos, None);
            d.depot_changes.push(pos);
        }
        let settings = d.settings.get(&pos).copied().unwrap_or_default();
        let [a, b] = parts(block, info, &settings);
        d.kinetics.remove(pos);
        d.states.insert(pos, state);
        d.columns
            .entry((pos[0] >> 4, pos[2] >> 4))
            .or_default()
            .push(pos);
        if let Some(a) = a {
            d.kinetics.set(pos, 0, a);
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
            if self.table.get(new).is_some_and(|(b, _)| follows_power(b))
                && self.table.get(old).map(|(b, _)| b) != self.table.get(new).map(|(b, _)| b)
            {
                // ClutchBlock.getStateForPlacement: powered as placed.
                self.dims[dim].redstone.push(pos);
            }
            if self.table.get(new).map(|(b, _)| b) == Some(Block::Millstone) {
                self.dims[dim].millstones.entry(pos).or_default();
            }
            if self.table.get(new).map(|(b, _)| b) == Some(Block::WaterWheel)
                && self.table.get(old).map(|(b, _)| b) != Some(Block::WaterWheel)
            {
                self.dims[dim].wheels.push(pos);
                self.dims[dim].wheel_checks.push(pos);
            }
            self.place(dim, pos, new);
        } else {
            let d = &mut self.dims[dim];
            d.kinetics.remove(pos);
            d.states.remove(&pos);
            d.cranks.remove(&pos);
            d.wheels.retain(|p| *p != pos);
            d.millstones.remove(&pos);
            if d.depots.remove(&pos).is_some() {
                d.sliding.retain(|p| *p != pos);
                d.depot_changes.push(pos);
            }
            if d.settings.remove(&pos).is_some() {
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
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
                d.states.remove(&pos);
                d.cranks.remove(&pos);
            }
            d.wheels.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.millstones
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.depots.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.sliding.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
        }
        d.settings.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
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

    /// A player scrolls on the creative motor at `pos`: its speed moves by one per step, between
    /// -256 and 256, never 0 (Create 5's scroll value; Create 6 sets it on a value board).
    /// Returns the new speed, or None when there is no motor there.
    pub fn scroll_motor(&mut self, dim: usize, pos: [i32; 3], steps: i32) -> Option<f32> {
        let state = *self.dims[dim].states.get(&pos)?;
        let (block, _) = self.table.get(state)?;
        if block != Block::CreativeMotor || steps == 0 {
            return None;
        }
        let d = &mut self.dims[dim];
        let old = d.settings.get(&pos).copied().unwrap_or_default().motor_rpm as i32;
        let mut new = old;
        for _ in 0..steps.unsigned_abs().min(512) {
            new += steps.signum();
            if new == 0 {
                new += steps.signum();
            }
        }
        let new = new.clamp(-256, 256) as f32;
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        self.set_settings(
            dim,
            pos,
            state,
            Settings {
                motor_rpm: new,
                ..old
            },
        );
        Some(new)
    }

    /// The settings of a column's blocks, to save with it: `(position, motor speed)`.
    pub fn column_settings(&self, dim: usize, cx: i32, cz: i32) -> Vec<([i32; 3], i32)> {
        let mut out: Vec<([i32; 3], i32)> = self.dims[dim]
            .settings
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
            .filter(|(p, _)| {
                self.dims[dim]
                    .states
                    .get(*p)
                    .and_then(|s| self.table.get(*s))
                    .map(|(b, _)| b)
                    == Some(Block::CreativeMotor)
            })
            .map(|(p, s)| (*p, s.motor_rpm as i32))
            .collect();
        out.sort();
        out
    }

    /// A player turns the hand crank at `pos` (`HandCrankBlockEntity.turn`): it generates for
    /// the next 10 ticks, backwards when `back` (sneaking). False when there is no crank there.
    pub fn turn_crank(&mut self, dim: usize, pos: [i32; 3], back: bool) -> bool {
        let Some(&state) = self.dims[dim].states.get(&pos) else {
            return false;
        };
        if self.table.get(state).map(|(b, _)| b) != Some(Block::HandCrank) {
            return false;
        }
        let d = &mut self.dims[dim];
        d.cranks.insert(pos, 10);
        let crank = if back { -1 } else { 1 };
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        if old.crank != crank {
            self.set_settings(dim, pos, state, Settings { crank, ..old });
        }
        true
    }

    /// One tick of the cranks being turned: those whose time ran out stop.
    pub fn tick_cranks(&mut self, dim: usize) {
        if self.dims[dim].cranks.is_empty() {
            return;
        }
        let mut stopped = Vec::new();
        for (pos, left) in self.dims[dim].cranks.iter_mut() {
            *left -= 1;
            if *left == 0 {
                stopped.push(*pos);
            }
        }
        for pos in stopped {
            let d = &mut self.dims[dim];
            d.cranks.remove(&pos);
            if let (Some(&state), Some(old)) = (d.states.get(&pos), d.settings.get(&pos).copied()) {
                self.set_settings(dim, pos, state, Settings { crank: 0, ..old });
            }
        }
    }

    /// Settings saved with a column, before its blocks are scanned.
    pub fn load_settings(&mut self, dim: usize, saved: impl Iterator<Item = ([i32; 3], i32)>) {
        for (pos, rpm) in saved {
            self.dims[dim].settings.insert(
                pos,
                Settings {
                    motor_rpm: rpm as f32,
                    ..Settings::default()
                },
            );
        }
    }

    /// A neighbour of the block `state` at `pos` changed: a block that follows redstone is
    /// checked after the tick. The first line is the hot path for every other block.
    #[inline]
    pub fn neighbor_changed(&mut self, dim: usize, pos: [i32; 3], state: u32) {
        if !self.table.is_kinetic(state) {
            return;
        }
        match self.table.get(state).map(|(b, _)| b) {
            Some(b) if follows_power(b) => self.dims[dim].redstone.push(pos),
            Some(Block::WaterWheel) => self.dims[dim].wheel_checks.push(pos),
            _ => {}
        }
    }

    /// The water wheels to look at in this tick (`tick` counts ticks): those whose neighbour
    /// changed, and one in 60 of the others (Create's `lazyTick` every 60 ticks), with their axis.
    pub fn take_wheel_checks(
        &mut self,
        dim: usize,
        tick: u64,
    ) -> Vec<([i32; 3], crate::kinetics::Axis)> {
        let d = &mut self.dims[dim];
        let mut list = std::mem::take(&mut d.wheel_checks);
        list.extend(d.wheels.iter().filter(|p| {
            (p[0].wrapping_mul(31) ^ p[2].wrapping_mul(17) ^ p[1]).rem_euclid(60) as u64
                == tick % 60
        }));
        list.sort();
        list.dedup();
        list.into_iter()
            .filter_map(|p| {
                let state = *self.dims[dim].states.get(&p)?;
                let (block, info) = self.table.get(state)?;
                (block == Block::WaterWheel).then_some((p, info.facing?.axis()))
            })
            .collect()
    }

    /// Millstones that turn: the only ones that do anything this tick (Create's tick returns at
    /// once without speed), with their speed.
    pub fn turning_millstones(&self, dim: usize) -> Vec<([i32; 3], f32)> {
        let d = &self.dims[dim];
        d.millstones
            .keys()
            .filter_map(|p| Some((*p, d.kinetics.speed(*p, 0).filter(|s| *s != 0.0)?)))
            .collect()
    }

    /// A water wheel's new flow score: it turns again when the score changed.
    pub fn set_wheel_flow(&mut self, dim: usize, pos: [i32; 3], score: i32) {
        let flow = score.clamp(-1, 1) as i8;
        let d = &mut self.dims[dim];
        let Some(&state) = d.states.get(&pos) else {
            return;
        };
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        if old.flow != flow {
            self.set_settings(dim, pos, state, Settings { flow, ..old });
        }
    }

    /// Blocks to check for redstone power, with whether each is powered now.
    pub fn take_redstone(&mut self, dim: usize) -> Vec<[i32; 3]> {
        let mut v = std::mem::take(&mut self.dims[dim].redstone);
        v.sort();
        v.dedup();
        v
    }

    /// Whether the block `state` is powered (`powered`), for a block that follows redstone.
    pub fn powered(&self, state: u32) -> Option<bool> {
        let (block, info) = self.table.get(state)?;
        follows_power(block).then_some(info.powered)
    }

    /// Columns whose saved data changed since the last call.
    /// A stack was put on the depot at `pos` with this motion (it slides from now on), or its
    /// held stack left (None).
    pub fn depot_put(&mut self, dim: usize, pos: [i32; 3], held: Option<crate::depot::Held>) {
        let d = &mut self.dims[dim];
        if let Some(h) = d.depots.get_mut(&pos) {
            *h = held;
            if held.is_some_and(|h| h.sliding()) && !d.sliding.contains(&pos) {
                d.sliding.push(pos);
            }
            d.depot_changes.push(pos);
        }
    }

    /// The depot at `pos` changed in a way clients must see (`notifyUpdate`).
    pub fn depot_changed(&mut self, dim: usize, pos: [i32; 3]) {
        if self.dims[dim].depots.contains_key(&pos) {
            self.dims[dim].depot_changes.push(pos);
        }
    }

    /// `DepotBehaviour.tick` for the depots whose stack slides; `upright(pos)` says whether the
    /// stack there stands (`BeltHelper.isItemUpright`). Depots at rest cost nothing.
    pub fn tick_depots(&mut self, dim: usize, upright: impl Fn([i32; 3]) -> bool) {
        let d = &mut self.dims[dim];
        if d.sliding.is_empty() {
            return;
        }
        let depots = &mut d.depots;
        d.sliding.retain(|p| {
            let Some(Some(h)) = depots.get_mut(p) else {
                return false;
            };
            h.slide(upright(*p));
            h.sliding()
        });
    }

    /// Depots whose contents changed since the last call, once each.
    pub fn take_depot_changes(&mut self, dim: usize) -> Vec<[i32; 3]> {
        let mut v = std::mem::take(&mut self.dims[dim].depot_changes);
        v.sort();
        v.dedup();
        v
    }

    pub fn take_unsaved(&mut self, dim: usize) -> Vec<(i32, i32)> {
        let mut v = std::mem::take(&mut self.dims[dim].unsaved);
        v.sort();
        v.dedup();
        v
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
        w.set_settings(
            0,
            [0, 64, 0],
            motor_east,
            Settings {
                motor_rpm: 64.0,
                ..Settings::default()
            },
        );
        w.tick(0, &mut e);
        assert_eq!(w.speed(0, [1, 64, 0]), Some(64.0));
        // Breaking the motor stops the shaft.
        w.block_changed(0, [0, 64, 0], motor_east, 0);
        w.tick(0, &mut e);
        assert_eq!(w.speed(0, [1, 64, 0]), Some(0.0));
        states::clear_range();
    }

    #[test]
    fn scrolling_a_motor_skips_zero_and_saves_with_its_column() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = world();
        let motor_east = 5;
        w.block_changed(0, [1, 64, 1], 0, motor_east);
        assert_eq!(
            w.scroll_motor(0, [1, 64, 1], -16),
            Some(-1.0),
            "16 down 16 skips 0"
        );
        assert_eq!(w.scroll_motor(0, [1, 64, 1], 1), Some(1.0));
        assert_eq!(w.scroll_motor(0, [1, 64, 1], 1000), Some(256.0));
        assert_eq!(w.scroll_motor(0, [2, 64, 1], 1), None, "no motor there");
        assert_eq!(w.take_unsaved(0), [(0, 0)]);
        assert_eq!(w.column_settings(0, 0, 0), [([1, 64, 1], 256)]);
        // A reload brings the setting back before the motor is found.
        w.column_unloaded(0, 0, 0);
        assert!(w.column_settings(0, 0, 0).is_empty());
        w.load_settings(0, [([1, 64, 1], 256)].into_iter());
        w.block_changed(0, [1, 64, 1], 0, motor_east);
        w.block_changed(0, [2, 64, 1], 0, 2);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [2, 64, 1]), Some(256.0));
        states::clear_range();
    }

    #[test]
    fn split_shafts_are_checked_for_power_when_placed_or_their_neighbour_changes() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:clutch", vec![("axis", "x"), ("powered", "false")]),
            ("create:clutch", vec![("axis", "x"), ("powered", "true")]),
            ("create:shaft", vec![("axis", "x")]),
        ];
        states::set_range(10, 3);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        w.block_changed(0, [0, 0, 0], 0, 10);
        assert_eq!(w.take_redstone(0), [[0, 0, 0]], "placed: checked once");
        w.block_changed(0, [0, 0, 0], 10, 11);
        assert!(
            w.take_redstone(0).is_empty(),
            "its own power change is no new check"
        );
        w.neighbor_changed(0, [0, 0, 0], 11);
        w.neighbor_changed(0, [1, 0, 0], 12);
        w.neighbor_changed(0, [2, 0, 0], 3);
        assert_eq!(
            w.take_redstone(0),
            [[0, 0, 0]],
            "shafts and vanilla blocks are not"
        );
        assert_eq!(w.powered(11), Some(true));
        assert_eq!(w.powered(12), None);
        states::clear_range();
    }

    #[test]
    fn a_depot_ticks_only_while_its_stack_slides() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![("create:depot", vec![])];
        states::set_range(10, 1);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        w.block_changed(0, [0, 0, 0], 0, 10);
        assert_eq!(
            w.take_depot_changes(0),
            [[0, 0, 0]],
            "placed: clients see it"
        );
        assert!(w.dims[0].sliding.is_empty(), "an empty depot does nothing");
        let held = crate::depot::Held::put_by_player(0, false, crate::depot::Side::East);
        w.depot_put(0, [0, 0, 0], Some(held));
        assert_eq!(w.take_depot_changes(0), [[0, 0, 0]]);
        let mut ticks = 0;
        while !w.dims[0].sliding.is_empty() {
            w.tick_depots(0, |_| false);
            ticks += 1;
        }
        assert!((5..30).contains(&ticks), "{ticks}");
        let at = w.dims[0].depots[&[0, 0, 0]].unwrap();
        assert!(0.5 - at.position <= 1.0 / 512.0);
        assert!(
            w.take_depot_changes(0).is_empty(),
            "arriving is no change to send"
        );
        w.column_unloaded(0, 0, 0);
        assert!(w.dims[0].depots.is_empty());
        states::clear_range();
    }

    #[test]
    fn a_turned_crank_drives_for_ten_ticks() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            (
                "create:hand_crank",
                vec![("facing", "west"), ("waterlogged", "false")],
            ),
            ("create:shaft", vec![("axis", "x")]),
        ];
        states::set_range(10, 2);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        // Facing west, hanging from the block to its east: the shaft at x + 1.
        w.block_changed(0, [0, 0, 0], 0, 10);
        w.block_changed(0, [1, 0, 0], 0, 11);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [1, 0, 0]), Some(0.0));
        assert!(w.turn_crank(0, [0, 0, 0], false));
        w.tick(0, &mut Vec::new());
        // convertToDirection(32, west): -32 along x.
        assert_eq!(w.speed(0, [1, 0, 0]), Some(-32.0));
        for _ in 0..9 {
            w.tick_cranks(0);
        }
        w.tick(0, &mut Vec::new());
        assert_eq!(
            w.speed(0, [1, 0, 0]),
            Some(-32.0),
            "still turning at tick 9"
        );
        w.tick_cranks(0);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [1, 0, 0]), Some(0.0), "stopped after 10 ticks");
        assert!(!w.turn_crank(0, [1, 0, 0], false), "a shaft is no crank");
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
