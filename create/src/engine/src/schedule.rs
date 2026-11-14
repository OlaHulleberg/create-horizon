//! Create's train schedules (`Schedule`, `ScheduleEntry`, `ScheduleRuntime`, the instructions
//! `DestinationInstruction`, `ChangeTitleInstruction`, `ChangeThrottleInstruction` and the wait
//! conditions `ScheduledDelay`, `TimeOfDayCondition`, `PlayerPassengerCondition`,
//! `IdleCargoCondition`, `StationPoweredCondition`): a list of entries, each an instruction; a
//! station instruction sends the train to a station whose name matches a filter, then it waits
//! there until one of its condition groups is met (every condition of the group, in order); a
//! title or throttle instruction takes effect at once. At the end the schedule starts over, or
//! stops when it is not cyclic.

/// `TimedWaitCondition.TimeUnit`: ticks a unit.
pub const TICKS: i32 = 1;
pub const SECONDS: i32 = 20;
pub const MINUTES: i32 = 20 * 60;

/// `ScheduleRuntime.INTERVAL`: how long a train waits after an instruction could not start.
pub const INTERVAL: i32 = 40;

/// `TimedWaitCondition.TimeUnit`, by its ordinal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimeUnit {
    Ticks,
    #[default]
    Seconds,
    Minutes,
}

impl TimeUnit {
    pub const ALL: [TimeUnit; 3] = [TimeUnit::Ticks, TimeUnit::Seconds, TimeUnit::Minutes];

    pub fn ticks_per(self) -> i32 {
        match self {
            TimeUnit::Ticks => TICKS,
            TimeUnit::Seconds => SECONDS,
            TimeUnit::Minutes => MINUTES,
        }
    }

    /// The compact summary's suffix (`5s`).
    pub fn suffix(self) -> &'static str {
        match self {
            TimeUnit::Ticks => "t",
            TimeUnit::Seconds => "s",
            TimeUnit::Minutes => "min",
        }
    }

    /// Its lang key (`create.generic.unit.seconds`).
    pub fn key(self) -> &'static str {
        match self {
            TimeUnit::Ticks => "create.generic.unit.ticks",
            TimeUnit::Seconds => "create.generic.unit.seconds",
            TimeUnit::Minutes => "create.generic.unit.minutes",
        }
    }

    pub fn ordinal(self) -> i32 {
        TimeUnit::ALL.iter().position(|u| *u == self).unwrap_or(1) as i32
    }

    pub fn from_ordinal(i: i32) -> TimeUnit {
        TimeUnit::ALL
            .get(i.max(0) as usize)
            .copied()
            .unwrap_or_default()
    }
}

/// What an entry does (`Schedule.INSTRUCTION_TYPES` that the port runs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Instruction {
    /// `DestinationInstruction`: the station name filter (`*` any text).
    Destination(String),
    /// `ChangeTitleInstruction`: the schedule's title shown on displays.
    Rename(String),
    /// `ChangeThrottleInstruction`: the top speed in percent, 5 to 100.
    Throttle(i32),
}

impl Default for Instruction {
    fn default() -> Self {
        Instruction::Destination(String::new())
    }
}

impl Instruction {
    /// The types in the editor's order, as their ids.
    pub const IDS: [&'static str; 3] = ["destination", "rename", "throttle"];

    pub fn id(&self) -> &'static str {
        match self {
            Instruction::Destination(_) => "destination",
            Instruction::Rename(_) => "rename",
            Instruction::Throttle(_) => "throttle",
        }
    }

    /// A new instruction of type `id` with Create's defaults.
    pub fn of(id: &str) -> Option<Instruction> {
        Some(match id {
            "destination" => Instruction::Destination(String::new()),
            "rename" => Instruction::Rename(String::new()),
            "throttle" => Instruction::Throttle(100),
            _ => return None,
        })
    }

    /// `supportsConditions`: only a station has conditions to wait on.
    pub fn supports_conditions(&self) -> bool {
        matches!(self, Instruction::Destination(_))
    }
}

/// A wait condition (`Schedule.CONDITION_TYPES` that the port runs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Condition {
    /// `ScheduledDelay`: this long at the station.
    Delay { value: i32, unit: TimeUnit },
    /// `TimeOfDayCondition`: at this hour and minute, every `rotation` (an index of
    /// `getRotation`: 0 every day ... 9 every quarter hour).
    TimeOfDay { hour: i32, minute: i32, rotation: i32 },
    /// `PlayerPassengerCondition`: this many players seated, or more unless `exact`.
    PlayerCount { count: i32, exact: bool },
    /// `IdleCargoCondition`: no cargo moved in or out for longer than this.
    Idle { value: i32, unit: TimeUnit },
    /// `StationPoweredCondition`: the station block has a redstone signal.
    Powered,
}

impl Condition {
    /// The types in the editor's order, as their ids.
    pub const IDS: [&'static str; 5] = ["delay", "time_of_day", "player_count", "idle", "powered"];

    pub fn id(&self) -> &'static str {
        match self {
            Condition::Delay { .. } => "delay",
            Condition::TimeOfDay { .. } => "time_of_day",
            Condition::PlayerCount { .. } => "player_count",
            Condition::Idle { .. } => "idle",
            Condition::Powered => "powered",
        }
    }

    /// A new condition of type `id` with Create's defaults.
    pub fn of(id: &str) -> Option<Condition> {
        Some(match id {
            "delay" => Condition::Delay {
                value: 5,
                unit: TimeUnit::Seconds,
            },
            "time_of_day" => Condition::TimeOfDay {
                hour: 8,
                minute: 0,
                rotation: 5,
            },
            "player_count" => Condition::PlayerCount {
                count: 1,
                exact: false,
            },
            "idle" => Condition::Idle {
                value: 5,
                unit: TimeUnit::Seconds,
            },
            "powered" => Condition::Powered,
            _ => return None,
        })
    }

    /// `TimedWaitCondition.totalWaitTicks`.
    pub fn ticks(value: i32, unit: TimeUnit) -> i32 {
        value * unit.ticks_per()
    }
}

/// `TimeOfDayCondition.getRotation`: ticks between the times it is met.
pub fn rotation_ticks(index: i32) -> i64 {
    match index {
        9 => 250,
        8 => 500,
        7 => 750,
        6 => 1000,
        5 => 2000,
        4 => 3000,
        3 => 4000,
        2 => 6000,
        1 => 12000,
        _ => 24000,
    }
}

/// An entry: what to do and, for a station, the condition groups to wait on there (any
/// group, all its conditions in turn).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub instruction: Instruction,
    pub conditions: Vec<Vec<Condition>>,
}

impl Entry {
    /// A station entry (`DestinationInstruction`).
    pub fn station(filter: &str, conditions: Vec<Vec<Condition>>) -> Entry {
        Entry {
            instruction: Instruction::Destination(filter.to_owned()),
            conditions,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Schedule {
    pub entries: Vec<Entry>,
    pub cyclic: bool,
    /// `savedProgress`: the entry a train given this schedule starts at.
    pub saved_progress: usize,
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
    /// `ChangeThrottleInstruction`: the train's throttle is now this many percent.
    Throttle(i32),
}

/// What the conditions read of the world each tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    /// Ticks since the train's carriages last exchanged cargo.
    pub idle_cargo: i32,
    /// The level's day time (`getDayTime`).
    pub day_time: i64,
    /// Players seated on the train (`countPlayerPassengers`).
    pub players: i32,
    /// The station it stands at has a redstone signal.
    pub powered: bool,
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
    /// `currentTitle`: set by a title instruction.
    pub title: String,
    /// `isAutoSchedule`: given by a station, not by a player (it is not handed back).
    pub auto: bool,
}

impl Runtime {
    /// `setSchedule`: from its saved progress, running.
    pub fn new() -> Runtime {
        Runtime::default()
    }

    /// `setSchedule(schedule, auto)`.
    pub fn for_schedule(schedule: &Schedule, auto: bool) -> Runtime {
        Runtime {
            current: schedule
                .saved_progress
                .min(schedule.entries.len().saturating_sub(1)),
            auto,
            ..Runtime::default()
        }
    }

    /// `tick`: `navigating` while the train has a destination.
    pub fn tick(&mut self, schedule: &Schedule, navigating: bool, inputs: Inputs) -> Order {
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
                self.tick_conditions(schedule, inputs);
                Order::Nothing
            }
            State::PreTransit => self.start_current(schedule),
        }
    }

    /// `startCurrentInstruction`: a station is looked for; a title or throttle takes effect
    /// and the next entry follows.
    fn start_current(&mut self, schedule: &Schedule) -> Order {
        if self.end_reached(schedule) {
            return Order::Nothing;
        }
        match &schedule.entries[self.current].instruction {
            Instruction::Destination(filter) => Order::Navigate(filter.clone()),
            Instruction::Rename(title) => {
                self.title.clone_from(title);
                self.current += 1;
                Order::Nothing
            }
            Instruction::Throttle(p) => {
                self.current += 1;
                Order::Throttle(*p)
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
    fn tick_conditions(&mut self, schedule: &Schedule, inputs: Inputs) {
        let entry = &schedule.entries[self.current];
        if !entry.instruction.supports_conditions() {
            self.state = State::PreTransit;
            self.current += 1;
            return;
        }
        if self.progress.len() != entry.conditions.len() {
            self.progress = vec![(0, 0); entry.conditions.len()];
        }
        for (group, (done, time)) in entry.conditions.iter().zip(self.progress.iter_mut()) {
            if *done >= group.len() {
                self.state = State::PreTransit;
                self.current += 1;
                return;
            }
            let met = match group[*done] {
                Condition::Delay { value, unit } => {
                    if *time >= Condition::ticks(value, unit) {
                        true
                    } else {
                        *time += 1;
                        false
                    }
                }
                Condition::Idle { value, unit } => {
                    *time = inputs.idle_cargo;
                    inputs.idle_cargo > Condition::ticks(value, unit)
                }
                Condition::TimeOfDay {
                    hour,
                    minute,
                    rotation,
                } => {
                    let r = rotation_ticks(rotation);
                    let day = inputs.day_time.rem_euclid(r);
                    let target = ((i64::from((hour + 18) % 24)) * 1000
                        + (f64::from(minute) / 60.0 * 1000.0).ceil() as i64)
                        % r;
                    let diff = day - target;
                    (0..=40).contains(&diff)
                }
                Condition::PlayerCount { count, exact } => {
                    if exact {
                        inputs.players == count
                    } else {
                        inputs.players >= count
                    }
                }
                Condition::Powered => inputs.powered,
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

    fn idle(n: i32) -> Inputs {
        Inputs {
            idle_cargo: n,
            ..Inputs::default()
        }
    }

    fn shuttle(cyclic: bool) -> Schedule {
        Schedule {
            entries: vec![
                Entry::station(
                    "Depot",
                    vec![vec![Condition::Delay {
                        value: 5,
                        unit: TimeUnit::Seconds,
                    }]],
                ),
                Entry::station(
                    "Mine, Platform *",
                    vec![
                        vec![Condition::Idle {
                            value: 3,
                            unit: TimeUnit::Seconds,
                        }],
                        vec![Condition::Delay {
                            value: 1,
                            unit: TimeUnit::Minutes,
                        }],
                    ],
                ),
            ],
            cyclic,
            saved_progress: 0,
        }
    }

    #[test]
    fn a_shuttle_goes_waits_and_starts_over() {
        let s = shuttle(true);
        let mut r = Runtime::new();
        assert_eq!(r.tick(&s, false, idle(0)), Order::Navigate("Depot".into()));
        // No path: it looks again 40 ticks later.
        r.no_path();
        let asks = (0..41)
            .filter(|_| r.tick(&s, false, idle(0)) != Order::Nothing)
            .count();
        assert_eq!(asks, 1);
        r.started();
        for _ in 0..30 {
            assert_eq!(r.tick(&s, true, idle(0)), Order::Nothing);
        }
        assert_eq!(r.ticks_in_transit, 30);
        r.arrived(&s);
        // 5 s: 100 ticks counting, the 101st sees it done, the next moves on.
        let mut ticks = 0;
        while r.current == 0 {
            r.tick(&s, false, idle(0));
            ticks += 1;
        }
        assert_eq!(ticks, 102);
        assert_eq!(
            r.tick(&s, false, idle(0)),
            Order::Navigate("Mine, Platform *".into())
        );
        r.started();
        r.arrived(&s);
        // Cargo idle for more than 3 s ends the wait before the minute.
        let mut ticks = 0;
        while r.current == 1 {
            r.tick(&s, false, idle(ticks));
            ticks += 1;
        }
        assert!(ticks < 70, "{ticks}");
        // Cyclic: back to the first entry.
        assert_eq!(r.tick(&s, false, idle(0)), Order::Nothing);
        assert_eq!(r.current, 0);
        assert!(!r.paused);
        assert_eq!(r.tick(&s, false, idle(0)), Order::Navigate("Depot".into()));
    }

    #[test]
    fn a_one_way_schedule_completes() {
        let s = shuttle(false);
        let mut r = Runtime {
            current: 2,
            ..Runtime::new()
        };
        assert_eq!(r.tick(&s, false, idle(0)), Order::Nothing);
        assert!(r.paused && r.completed);
    }

    #[test]
    fn titles_and_throttles_take_effect_at_once() {
        let s = Schedule {
            entries: vec![
                Entry {
                    instruction: Instruction::Rename("Express".into()),
                    conditions: Vec::new(),
                },
                Entry {
                    instruction: Instruction::Throttle(50),
                    conditions: Vec::new(),
                },
                Entry::station("Depot", vec![vec![Condition::Powered]]),
            ],
            cyclic: false,
            saved_progress: 0,
        };
        let mut r = Runtime::new();
        assert_eq!(r.tick(&s, false, idle(0)), Order::Nothing);
        assert_eq!(r.title, "Express");
        assert_eq!(r.tick(&s, false, idle(0)), Order::Throttle(50));
        assert_eq!(r.tick(&s, false, idle(0)), Order::Navigate("Depot".into()));
        r.already_there(&s);
        // Unpowered it waits; powered it is done.
        for _ in 0..10 {
            r.tick(&s, false, idle(0));
        }
        assert_eq!(r.current, 2);
        let on = Inputs {
            powered: true,
            ..Inputs::default()
        };
        r.tick(&s, false, on);
        r.tick(&s, false, on);
        assert_eq!(r.current, 3);
    }

    #[test]
    fn time_of_day_and_passengers() {
        // 8:00 is day time 2000 (`(8 + 18) % 24 * 1000`), met for 40 ticks each day.
        let s = Schedule {
            entries: vec![Entry::station(
                "A",
                vec![
                    vec![Condition::TimeOfDay {
                        hour: 8,
                        minute: 0,
                        rotation: 0,
                    }],
                    vec![Condition::PlayerCount {
                        count: 2,
                        exact: true,
                    }],
                ],
            )],
            cyclic: true,
            saved_progress: 0,
        };
        let at = |day_time, players| Inputs {
            day_time,
            players,
            ..Inputs::default()
        };
        let mut r = Runtime::new();
        r.tick(&s, false, at(0, 0));
        r.already_there(&s);
        r.tick(&s, false, at(1999, 3));
        r.tick(&s, false, at(1999, 3));
        assert_eq!(r.current, 0);
        r.tick(&s, false, at(24_000 + 2010, 0));
        r.tick(&s, false, at(24_000 + 2011, 0));
        assert_eq!(r.current, 1);
        let mut r = Runtime::new();
        r.tick(&s, false, at(0, 0));
        r.already_there(&s);
        r.tick(&s, false, at(0, 2));
        r.tick(&s, false, at(0, 2));
        assert_eq!(r.current, 1);
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
