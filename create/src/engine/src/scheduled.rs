//! Vanilla's server block ticks, as Create's blocks schedule them: a block that cares about a
//! neighbour changes (`neighborChanged`) calls `level.scheduleTick(pos, this, 1)`, and its
//! `tick` runs a tick later, in the level's block-tick phase before block entities tick.
//!
//! `Level.getBlockTicks().willTickThisTick(pos, block)` is what a block checks first: while a
//! tick is already due this tick, `neighborChanged` schedules nothing, so a block whose own
//! state change notifies itself and its neighbour in the tick the neighbour is looked at is not
//! looked at a second time on the next.

/// The block ticks one block type is due in one level (`LevelChunkTicks` for that type).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScheduledTicks {
    /// Ticks scheduled for the next tick (`scheduleTick(pos, this, 1)` at or before this tick).
    due_next: Vec<[i32; 3]>,
    /// The ticks running this tick (`LevelTicks.toRunThisTick`).
    running: Vec<[i32; 3]>,
}

impl ScheduledTicks {
    /// `neighborChanged`'s `scheduleTick(pos, this, 1)`: the tick runs in the next tick's
    /// block-tick phase, unless it is already due this tick (`willTickThisTick`) or already
    /// scheduled for the next (`LevelChunkTicks` holds one tick per position and time).
    pub fn schedule(&mut self, pos: [i32; 3]) {
        if self.running.contains(&pos) || self.due_next.contains(&pos) {
            return;
        }
        self.due_next.push(pos);
    }

    /// The block-tick phase, before block entities tick: the ticks scheduled last tick run now,
    /// in the block-tick queue's order (positions, ties of which Create's rules do not depend
    /// on).
    pub fn due(&mut self) -> &[[i32; 3]] {
        self.running.clear();
        self.running.append(&mut self.due_next);
        self.running.sort_unstable();
        self.running.dedup();
        &self.running
    }

    /// The ticks running this tick, which `willTickThisTick` counts.
    pub fn running(&self) -> &[[i32; 3]] {
        &self.running
    }

    /// The block-tick phase is over: `LevelTicks.tick` clears the ticks it ran and the set
    /// `willTickThisTick` reads, so a neighbour change later in the tick (a block entity's own
    /// tick runs after the phase) schedules a tick for the next one.
    pub fn phase_ended(&mut self) {
        self.running.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_neighbour_change_runs_the_next_tick() {
        let mut ticks = ScheduledTicks::default();
        ticks.schedule([0, 8, 0]);
        // Nothing runs in the tick it was scheduled in.
        assert!(ticks.running().is_empty());
        assert_eq!(ticks.due(), [[0, 8, 0]]);
        // The following tick has nothing left.
        assert!(ticks.due().is_empty());
    }

    #[test]
    fn a_tick_due_this_tick_is_not_scheduled_again() {
        let mut ticks = ScheduledTicks::default();
        ticks.schedule([0, 8, 0]);
        assert_eq!(ticks.due(), [[0, 8, 0]]);
        // A neighbour changed while the block-tick phase runs (`willTickThisTick`): the tick
        // being looked at this tick serves it, so the next tick has none.
        ticks.schedule([0, 8, 0]);
        ticks.phase_ended();
        assert!(ticks.due().is_empty());
    }

    #[test]
    fn a_change_after_the_phase_schedules_the_next_tick() {
        let mut ticks = ScheduledTicks::default();
        ticks.schedule([0, 8, 0]);
        assert_eq!(ticks.due(), [[0, 8, 0]]);
        ticks.phase_ended();
        // Block entities tick after the phase: their changes are looked at in the next tick.
        ticks.schedule([0, 8, 0]);
        assert_eq!(ticks.due(), [[0, 8, 0]]);
    }

    #[test]
    fn schedules_dedupe_and_keep_the_queue_order() {
        let mut ticks = ScheduledTicks::default();
        ticks.schedule([1, 8, 0]);
        ticks.schedule([0, 8, 0]);
        ticks.schedule([1, 8, 0]);
        assert_eq!(ticks.due(), [[0, 8, 0], [1, 8, 0]]);
        // A position cleared from the level is still due; the block decides what it is then.
        ticks.phase_ended();
        ticks.schedule([2, 8, 0]);
        assert_eq!(ticks.due(), [[2, 8, 0]]);
        assert!(ticks.due().is_empty());
    }
}
