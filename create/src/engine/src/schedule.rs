//! Create's train schedules (`Schedule`, `ScheduleEntry`, `ScheduleRuntime`, `DestinationInstruction`,
//! `ScheduledDelay`, `IdleCargoCondition`): a list of entries, each sending the train to a
//! station whose name matches a filter, then waiting there until one of its condition groups
//! is met (every condition of the group, in order); at the end the schedule starts over, or
//! stops when it is not cyclic.

/// `TimedWaitCondition.TimeUnit`: ticks a unit.
pub const TICKS: i32 = 1;
pub const SECONDS: i32 = 20;
pub const MINUTES: i32 = 20 * 60;

/// `ScheduleRuntime.INTERVAL`: how long a train waits after an instruction could not start.
pub const INTERVAL: i32 = 40;

/// A wait condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Condition {
    /// `ScheduledDelay`: this many ticks at the station.
    Delay(i32),
    /// `IdleCargoCondition`: no cargo moved in or out for more than this many ticks.
    IdleCargo(i32),
}

/// An entry: where to go (`DestinationInstruction`'s station name filter) and the condition
/// groups (any group, all its conditions in turn).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub station: String,
    pub conditions: Vec<Vec<Condition>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Schedule {
    pub entries: Vec<Entry>,
    pub cyclic: bool,
}

/// `ScheduleRuntime.State`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    PreTransit,
    InTransit,
    PostTransit,
}

/// What the train must do after a tick of its schedule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Order {
    Nothing,
    /// Find the nearest free station matching this filter and go there
    /// (`instruction.start` → `findPathTo`); answer with [`Runtime::started`],
    /// [`Runtime::already_there`] or [`Runtime::no_path`].
    Navigate(String),
}

/// `ScheduleRuntime`: where the train is in its schedule.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Runtime {
    pub current: usize,
    pub state: State,
    pub paused: bool,
    pub completed: bool,
    pub cooldown: i32,
    /// Per condition group: how many of its conditions are met, and the current one's ticks.
    pub progress: Vec<(usize, i32)>,
    pub ticks_in_transit: i32,
}

impl Runtime {
    /// `setSchedule`: from its first entry, running.
    pub fn new() -> Runtime {
        Runtime::default()
    }

    /// `tick`: `navigating` while the train has a destination, `idle_cargo` the ticks since its
    /// carriages last exchanged cargo.
    pub fn tick(&mut self, schedule: &Schedule, navigating: bool, idle_cargo: i32) -> Order {
        if self.paused {
            return Order::Nothing;
        }
        if navigating {
            self.ticks_in_transit += 1;
            return Order::Nothing;
        }
        if self.end_reached(schedule) {
            return Order::Nothing;
        }
        let cooling = self.cooldown > 0;
        self.cooldown -= 1;
        if cooling {
            return Order::Nothing;
        }
        match self.state {
            State::InTransit => Order::Nothing,
            State::PostTransit => {
                self.tick_conditions(schedule, idle_cargo);
                Order::Nothing
            }
            State::PreTransit => {
                if self.end_reached(schedule) {
                    return Order::Nothing;
                }
                Order::Navigate(schedule.entries[self.current].station.clone())
            }
        }
    }

    /// `checkEndOfScheduleReached`.
    fn end_reached(&mut self, schedule: &Schedule) -> bool {
        if self.current < schedule.entries.len() {
            return false;
        }
        self.current = 0;
        if !schedule.cyclic {
            self.paused = true;
            self.completed = true;
        }
        true
    }

    /// The train set off for the station it was told.
    pub fn started(&mut self) {
        self.state = State::InTransit;
        self.ticks_in_transit = 0;
    }

    /// The matching station is the one it stands at: arrived at once.
    pub fn already_there(&mut self, schedule: &Schedule) {
        self.state = State::InTransit;
        self.arrived(schedule);
    }

    /// No matching station could be reached: try again after [`INTERVAL`] (`startCooldown`).
    pub fn no_path(&mut self) {
        self.cooldown = INTERVAL;
    }

    /// `destinationReached`: wait on the entry's conditions.
    pub fn arrived(&mut self, schedule: &Schedule) {
        if self.state != State::InTransit {
            return;
        }
        self.state = State::PostTransit;
        self.progress.clear();
        if let Some(entry) = schedule.entries.get(self.current) {
            self.progress = vec![(0, 0); entry.conditions.len()];
        }
    }

    /// `transitInterrupted`.
    pub fn interrupted(&mut self) {
        if self.state == State::InTransit {
            self.state = State::PreTransit;
            self.cooldown = 0;
        }
    }

    /// `tickConditions`: the first group whose conditions are all met moves on to the next
    /// entry; each group advances its current condition by one tick.
    fn tick_conditions(&mut self, schedule: &Schedule, idle_cargo: i32) {
        let entry = &schedule.entries[self.current];
        for (group, (done, time)) in entry.conditions.iter().zip(self.progress.iter_mut()) {
            if *done >= group.len() {
                self.state = State::PreTransit;
                self.current += 1;
                return;
            }
            let met = match group[*done] {
                Condition::Delay(ticks) => {
                    if *time >= ticks {
                        true
                    } else {
                        *time += 1;
                        false
                    }
                }
                Condition::IdleCargo(ticks) => {
                    *time = idle_cargo;
                    idle_cargo > ticks
                }
            };
            if met {
                *done += 1;
                *time = 0;
            }
        }
    }
}

/// A station name filter as Create's schedule offers it ("Use * as a text wildcard"): `*`
/// stands for any text, everything else matches itself.
pub fn station_matches(filter: &str, name: &str) -> bool {
    let parts: Vec<&str> = filter.split('*').collect();
    if parts.len() == 1 {
        return filter == name;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !name.starts_with(first) || name.len() < first.len() + last.len() || !name.ends_with(last) {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shuttle(cyclic: bool) -> Schedule {
        Schedule {
            entries: vec![
                Entry {
                    station: "Depot".into(),
                    conditions: vec![vec![Condition::Delay(5 * SECONDS)]],
                },
                Entry {
                    station: "Mine, Platform *".into(),
                    conditions: vec![
                        vec![Condition::IdleCargo(3 * SECONDS)],
                        vec![Condition::Delay(MINUTES)],
                    ],
                },
            ],
            cyclic,
        }
    }

    #[test]
    fn a_shuttle_goes_waits_and_starts_over() {
        let s = shuttle(true);
        let mut r = Runtime::new();
        assert_eq!(r.tick(&s, false, 0), Order::Navigate("Depot".into()));
        // No path: it looks again 40 ticks later.
        r.no_path();
        let asks = (0..41)
            .filter(|_| r.tick(&s, false, 0) != Order::Nothing)
            .count();
        assert_eq!(asks, 1);
        r.started();
        for _ in 0..30 {
            assert_eq!(r.tick(&s, true, 0), Order::Nothing);
        }
        assert_eq!(r.ticks_in_transit, 30);
        r.arrived(&s);
        // 5 s: 100 ticks counting, the 101st sees it done, the next moves on.
        let mut ticks = 0;
        while r.current == 0 {
            r.tick(&s, false, 0);
            ticks += 1;
        }
        assert_eq!(ticks, 102);
        assert_eq!(
            r.tick(&s, false, 0),
            Order::Navigate("Mine, Platform *".into())
        );
        r.started();
        r.arrived(&s);
        // Cargo idle for more than 3 s ends the wait before the minute.
        let mut ticks = 0;
        while r.current == 1 {
            r.tick(&s, false, ticks);
            ticks += 1;
        }
        assert!(ticks < 70, "{ticks}");
        // Cyclic: back to the first entry.
        assert_eq!(r.tick(&s, false, 0), Order::Nothing);
        assert_eq!(r.current, 0);
        assert!(!r.paused);
        assert_eq!(r.tick(&s, false, 0), Order::Navigate("Depot".into()));
    }

    #[test]
    fn a_one_way_schedule_completes() {
        let s = shuttle(false);
        let mut r = Runtime {
            current: 2,
            ..Runtime::new()
        };
        assert_eq!(r.tick(&s, false, 0), Order::Nothing);
        assert!(r.paused && r.completed);
    }

    #[test]
    fn station_filters_take_star_as_any_text() {
        assert!(station_matches("Mine, Platform *", "Mine, Platform 2"));
        assert!(station_matches("*Depot*", "North Depot East"));
        assert!(station_matches("Depot", "Depot"));
        assert!(!station_matches("Depot", "Depot 2"));
        assert!(!station_matches("Mine*2", "Mine 3"));
        assert!(station_matches("a*b*c", "a-b-b-c"));
    }
}
