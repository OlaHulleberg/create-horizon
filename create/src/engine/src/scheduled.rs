//! Vanilla's server block ticks, as Create's blocks schedule them: a block that cares about a
//! neighbour changes (`neighborChanged`) calls `level.scheduleTick(pos, this, 1)`, and its
//! `tick` runs a tick later, in the level's block-tick phase before block entities tick.
//!
//! `Level.getBlockTicks().willTickThisTick(pos, block)` is what a block checks first: while a
//! tick is already due this tick, `neighborChanged` schedules nothing, so a block whose own
//! state change notifies itself and its neighbour in the tick the neighbour is looked at is not
//! looked at a second time on the next.
//!
//! A tick carries vanilla's priority: the level collects every block type's due ticks into one
//! queue and runs the lowest value first, so `GearshiftBlock`'s `EXTREMELY_HIGH` re-attach
//! runs before `SequencedGearshiftBlock`'s `NORMAL` power check when both are due in a tick.

/// Vanilla's `TickPriority`, lowest value first (`LevelTicks` orders its queue by it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum TickPriority {
    /// `TickPriority.EXTREMELY_HIGH` (-3): `GearshiftBlock.detachKinetics`' re-attach.
    ExtremelyHigh,
    /// `TickPriority.VERY_HIGH` (-2).
    VeryHigh,
    /// `TickPriority.HIGH` (-1).
    High,
    /// `TickPriority.NORMAL` (0): what `scheduleTick(pos, this, 1)` gets.
    #[default]
    Normal,
    /// `TickPriority.LOW` (1).
    Low,
    /// `TickPriority.VERY_LOW` (2).
    VeryLow,
    /// `TickPriority.EXTREMELY_LOW` (3).
    ExtremelyLow,
}

/// The block ticks one level owes (`LevelTicks` and the `LevelChunkTicks` it collects from).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScheduledTicks {
    /// Ticks scheduled for the next tick (`scheduleTick(pos, this, 1)` at or before this tick),
    /// with the priority they were scheduled at.
    due_next: Vec<(TickPriority, [i32; 3])>,
    /// The ticks running this tick (`LevelTicks.toRunThisTick`), in the queue's order.
    running: Vec<[i32; 3]>,
    /// Whether this tick's ticks were collected: `LevelTicks.tick` collects once, and its
    /// `toRunThisTick` stays readable (`willTickThisTick`) until the tick ends.
    collected: bool,
}

impl ScheduledTicks {
    /// `neighborChanged`'s `scheduleTick(pos, this, 1)`: the tick runs in the next tick's
    /// block-tick phase, unless it is already due this tick (`willTickThisTick`) or already
    /// scheduled for the next (`LevelChunkTicks` holds one tick per position and time, and a
    /// second schedule does not replace it).
    pub fn schedule(&mut self, pos: [i32; 3]) {
        self.schedule_at(pos, TickPriority::Normal);
    }

    /// `scheduleTick(pos, this, 1, priority)`: as `schedule`, at a chosen priority. A tick
    /// already owed keeps the priority it was scheduled with.
    pub fn schedule_at(&mut self, pos: [i32; 3], priority: TickPriority) {
        if self.running.contains(&pos) || self.due_next.iter().any(|&(_, p)| p == pos) {
            return;
        }
        self.due_next.push((priority, pos));
    }

    /// The block-tick phase, before block entities tick: the ticks scheduled earlier run now,
    /// higher priority first, then in position order (ties of which Create's rules do not
    /// depend on). `LevelTicks.tick` collects once per tick, so a second call returns the
    /// ticks running.
    pub fn due(&mut self) -> &[[i32; 3]] {
        if self.collected {
            return &self.running;
        }
        self.collected = true;
        self.due_next
            .sort_unstable_by_key(|&(priority, pos)| (priority, pos));
        self.running.clear();
        self.running
            .extend(self.due_next.drain(..).map(|(_, pos)| pos));
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
        self.collected = false;
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
        ticks.phase_ended();
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
        ticks.phase_ended();
        assert!(ticks.due().is_empty());
    }

    #[test]
    fn collecting_once_keeps_the_ticks_running_for_a_second_reader() {
        let mut ticks = ScheduledTicks::default();
        ticks.schedule([1, 8, 0]);
        // One level tick collects the block types' ticks once; every consumer of the phase
        // reads the same `toRunThisTick`.
        assert_eq!(ticks.due(), [[1, 8, 0]]);
        assert_eq!(ticks.due(), [[1, 8, 0]]);
        ticks.phase_ended();
        assert!(ticks.due().is_empty());
    }

    #[test]
    fn the_highest_priority_runs_first() {
        let mut ticks = ScheduledTicks::default();
        ticks.schedule_at([1, 8, 0], TickPriority::Normal);
        ticks.schedule_at([0, 8, 0], TickPriority::ExtremelyHigh);
        ticks.schedule([2, 8, 0]);
        assert_eq!(ticks.due(), [[0, 8, 0], [1, 8, 0], [2, 8, 0]]);
    }

    #[test]
    fn a_tick_already_owed_keeps_its_priority() {
        let mut ticks = ScheduledTicks::default();
        ticks.schedule([0, 8, 0]);
        // `LevelChunkTicks.schedule` keeps the first tick for a position.
        ticks.schedule_at([0, 8, 0], TickPriority::ExtremelyHigh);
        ticks.schedule([1, 8, 0]);
        ticks.schedule_at([1, 8, 0], TickPriority::ExtremelyHigh);
        assert_eq!(ticks.due(), [[0, 8, 0], [1, 8, 0]]);
    }

    #[test]
    fn a_change_after_the_collection_waits_for_the_next_tick() {
        let mut ticks = ScheduledTicks::default();
        ticks.schedule([0, 8, 0]);
        assert_eq!(ticks.due(), [[0, 8, 0]]);
        // Block entities tick after the collection: their neighbour changes wait.
        ticks.schedule([1, 8, 0]);
        assert_eq!(ticks.due(), [[0, 8, 0]]);
        ticks.phase_ended();
        assert_eq!(ticks.due(), [[1, 8, 0]]);
    }
}
