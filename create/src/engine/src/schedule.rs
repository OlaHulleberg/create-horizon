//! Create's train schedules (`Schedule`, `ScheduleEntry`, `ScheduleRuntime`, the instructions
//! `DestinationInstruction`, `DeliverPackagesInstruction`, `FetchPackagesInstruction`,
//! `ChangeTitleInstruction`, `ChangeThrottleInstruction` and the wait conditions
//! `ScheduledDelay`, `TimeOfDayCondition`, `FluidThresholdCondition`, `ItemThresholdCondition`,
//! `RedstoneLinkCondition`, `PlayerPassengerCondition`, `IdleCargoCondition`,
//! `StationUnloadedCondition`, `StationPoweredCondition`): a list of entries, each an
//! instruction; a station instruction sends the train to a station whose name matches a filter
//! (a package instruction to the stations the glue found for it), then it waits there until one
//! of its condition groups is met (every condition of the group, in order); a title or throttle
//! instruction takes effect at once. At the end the schedule starts over, or stops when it is
//! not cyclic. What a condition reads of the world beyond the train (its cargo, a link's
//! network, whether the station is loaded) the glue measures before the tick ([`Probes`]).

/// `TimedWaitCondition.TimeUnit`: ticks a unit.
pub const TICKS: i32 = 1;
pub const SECONDS: i32 = 20;
pub const MINUTES: i32 = 20 * 60;

/// `ScheduleRuntime.INTERVAL`: how long a train waits after an instruction could not start.
pub const INTERVAL: i32 = 40;

/// `CargoThresholdCondition`'s `LazyTickedScheduleCondition` rate.
pub const LAZY: i32 = 20;

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

/// An item stack a condition holds (a ghost slot's), opaque to the engine: the glue's bytes
/// for it (its network NBT); empty for none.
pub type Stack = Vec<u8>;

/// What an entry does (`Schedule.INSTRUCTION_TYPES`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Instruction {
    /// `DestinationInstruction`: the station name filter (`*` any text).
    Destination(String),
    /// `DeliverPackagesInstruction`: to a station whose postboxes take the packages aboard.
    DeliverPackages,
    /// `FetchPackagesInstruction`: to a station whose postboxes hold packages for addresses
    /// matching the filter (`*` any text; empty: any).
    FetchPackages(String),
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
    pub const IDS: [&'static str; 5] = [
        "destination",
        "package_delivery",
        "package_retrieval",
        "rename",
        "throttle",
    ];

    pub fn id(&self) -> &'static str {
        match self {
            Instruction::Destination(_) => "destination",
            Instruction::DeliverPackages => "package_delivery",
            Instruction::FetchPackages(_) => "package_retrieval",
            Instruction::Rename(_) => "rename",
            Instruction::Throttle(_) => "throttle",
        }
    }

    /// A new instruction of type `id` with Create's defaults.
    pub fn of(id: &str) -> Option<Instruction> {
        Some(match id {
            "destination" => Instruction::Destination(String::new()),
            "package_delivery" => Instruction::DeliverPackages,
            "package_retrieval" => Instruction::FetchPackages(String::new()),
            "rename" => Instruction::Rename(String::new()),
            "throttle" => Instruction::Throttle(100),
            _ => return None,
        })
    }

    /// `supportsConditions`: a station (by name or for packages) has conditions to wait on.
    pub fn supports_conditions(&self) -> bool {
        matches!(
            self,
            Instruction::Destination(_)
                | Instruction::DeliverPackages
                | Instruction::FetchPackages(_)
        )
    }

    /// Whether it goes to the stations the glue finds for packages.
    pub fn packages(&self) -> bool {
        matches!(
            self,
            Instruction::DeliverPackages | Instruction::FetchPackages(_)
        )
    }
}

/// `CargoThresholdCondition.Ops`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Op {
    #[default]
    Greater,
    Less,
    Equal,
}

impl Op {
    pub const ALL: [Op; 3] = [Op::Greater, Op::Less, Op::Equal];

    pub fn test(self, current: i32, target: i32) -> bool {
        match self {
            Op::Greater => current > target,
            Op::Less => current < target,
            Op::Equal => current == target,
        }
    }

    /// `formatted`.
    pub fn symbol(self) -> &'static str {
        match self {
            Op::Greater => ">",
            Op::Less => "<",
            Op::Equal => "=",
        }
    }

    /// Its lang key's last part (`schedule.condition.threshold.greater`).
    pub fn key(self) -> &'static str {
        match self {
            Op::Greater => "greater",
            Op::Less => "less",
            Op::Equal => "equal",
        }
    }

    pub fn ordinal(self) -> i32 {
        Op::ALL.iter().position(|o| *o == self).unwrap_or(0) as i32
    }

    pub fn from_ordinal(i: i32) -> Op {
        Op::ALL.get(i.max(0) as usize).copied().unwrap_or_default()
    }
}

/// A wait condition (`Schedule.CONDITION_TYPES`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Condition {
    /// `ScheduledDelay`: this long at the station.
    Delay { value: i32, unit: TimeUnit },
    /// `TimeOfDayCondition`: at this hour and minute, every `rotation` (an index of
    /// `getRotation`: 0 every day ... 9 every quarter hour).
    TimeOfDay {
        hour: i32,
        minute: i32,
        rotation: i32,
    },
    /// `FluidThresholdCondition`: the train holds more, less or exactly this many buckets of
    /// the fluid of `item` (a bucket or a filter; none: any).
    FluidThreshold { op: Op, threshold: i32, item: Stack },
    /// `ItemThresholdCondition`: the train holds more, less or exactly this many items (or
    /// full stacks) matching `item` (none: any).
    ItemThreshold {
        op: Op,
        threshold: i32,
        stacks: bool,
        item: Stack,
    },
    /// `RedstoneLinkCondition`: the network of these two frequency items is powered (or not,
    /// `inverted`).
    RedstoneLink { freq: [Stack; 2], inverted: bool },
    /// `PlayerPassengerCondition`: this many players seated, or more unless `exact`.
    PlayerCount { count: i32, exact: bool },
    /// `IdleCargoCondition`: no cargo moved in or out for longer than this.
    Idle { value: i32, unit: TimeUnit },
    /// `StationUnloadedCondition`: the station's block is not in a ticking chunk.
    Unloaded,
    /// `StationPoweredCondition`: the station block has a redstone signal.
    Powered,
}

impl Condition {
    /// The types in the editor's order, as their ids.
    pub const IDS: [&'static str; 9] = [
        "delay",
        "time_of_day",
        "fluid_threshold",
        "item_threshold",
        "redstone_link",
        "player_count",
        "idle",
        "unloaded",
        "powered",
    ];

    pub fn id(&self) -> &'static str {
        match self {
            Condition::Delay { .. } => "delay",
            Condition::TimeOfDay { .. } => "time_of_day",
            Condition::FluidThreshold { .. } => "fluid_threshold",
            Condition::ItemThreshold { .. } => "item_threshold",
            Condition::RedstoneLink { .. } => "redstone_link",
            Condition::PlayerCount { .. } => "player_count",
            Condition::Idle { .. } => "idle",
            Condition::Unloaded => "unloaded",
            Condition::Powered => "powered",
        }
    }

    /// `slotsTargeted`: the ghost item slots its editor offers.
    pub fn slots(&self) -> usize {
        match self {
            Condition::FluidThreshold { .. } | Condition::ItemThreshold { .. } => 1,
            Condition::RedstoneLink { .. } => 2,
            _ => 0,
        }
    }

    /// Ghost slot `i`'s stack (`getItem`).
    pub fn item(&self, i: usize) -> Option<&Stack> {
        match self {
            Condition::FluidThreshold { item, .. } | Condition::ItemThreshold { item, .. } => {
                (i == 0).then_some(item)
            }
            Condition::RedstoneLink { freq, .. } => freq.get(i),
            _ => None,
        }
    }

    /// `setItem`.
    pub fn set_item(&mut self, i: usize, stack: Stack) {
        match self {
            Condition::FluidThreshold { item, .. } | Condition::ItemThreshold { item, .. } => {
                if i == 0 {
                    *item = stack;
                }
            }
            Condition::RedstoneLink { freq, .. } => {
                if let Some(f) = freq.get_mut(i) {
                    *f = stack;
                }
            }
            _ => {}
        }
    }

    /// Whether it reads the world through [`Probes`].
    pub fn probed(&self) -> bool {
        matches!(
            self,
            Condition::FluidThreshold { .. }
                | Condition::ItemThreshold { .. }
                | Condition::RedstoneLink { .. }
                | Condition::Unloaded
        )
    }

    /// `LazyTickedScheduleCondition`'s rate: the cargo thresholds look every 20 ticks.
    pub fn lazy(&self) -> bool {
        matches!(
            self,
            Condition::FluidThreshold { .. } | Condition::ItemThreshold { .. }
        )
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
            "fluid_threshold" => Condition::FluidThreshold {
                op: Op::Greater,
                threshold: 10,
                item: Stack::new(),
            },
            "item_threshold" => Condition::ItemThreshold {
                op: Op::Greater,
                threshold: 10,
                stacks: false,
                item: Stack::new(),
            },
            "redstone_link" => Condition::RedstoneLink {
                freq: [Stack::new(), Stack::new()],
                inverted: false,
            },
            "player_count" => Condition::PlayerCount {
                count: 1,
                exact: false,
            },
            "idle" => Condition::Idle {
                value: 5,
                unit: TimeUnit::Seconds,
            },
            "unloaded" => Condition::Unloaded,
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

#[derive(Clone, Debug, PartialEq)]
pub struct Schedule {
    pub entries: Vec<Entry>,
    pub cyclic: bool,
    /// `savedProgress`: the entry a train given this schedule starts at.
    pub saved_progress: usize,
}

/// `new Schedule()`: empty and cyclic.
impl Default for Schedule {
    fn default() -> Self {
        Schedule {
            entries: Vec::new(),
            cyclic: true,
            saved_progress: 0,
        }
    }
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
    /// A package instruction: go to the nearest of these stations, answered the same way.
    NavigateTo(Vec<[i32; 3]>),
    /// `ChangeThrottleInstruction`: the train's throttle is now this many percent.
    Throttle(i32),
}

/// What the glue measured for one train before the tick, for its current entry: each probed
/// condition's measure by (group, row) (items or full stacks, fluid in millibuckets, 1 for a
/// powered link network or an unloaded station, else 0), and for a package instruction the
/// stations it may go to and whether the train carries any package.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Probes {
    pub measures: Vec<((usize, usize), i32)>,
    pub packages: Option<(Vec<[i32; 3]>, bool)>,
}

impl Probes {
    fn measure(&self, at: (usize, usize)) -> i32 {
        self.measures
            .iter()
            .find(|(k, _)| *k == at)
            .map_or(0, |(_, v)| *v)
    }
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
    /// `predictionTicks`: each entry's last transit time in ticks ([`TBD`] until known).
    pub predictions: Vec<i32>,
    /// `displayLinkUpdateRequested`: the station's display links should refresh.
    pub display_update: bool,
}

/// `ScheduleRuntime.TBD` and `INVALID`: a transit time not known yet, and a time that cannot
/// be told.
pub const TBD: i32 = -1;
pub const INVALID: i32 = -2;

/// `TrainDeparturePrediction` (without its train): ticks until the train is at `destination`
/// (a station's name, or the filter of an entry to come), and the schedule's title then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prediction {
    pub ticks: i32,
    pub title: String,
    pub destination: String,
}

/// What `submitPredictions` reads of the train: the station it stands at, and its destination
/// with the distance left and the distance it set off with (`distanceStartedAt`), and its speed
/// (`min(throttle * maxSpeed, (maxSpeed + maxTurnSpeed) / 2)`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Whereabouts {
    pub station: Option<String>,
    pub destination: Option<(String, f64, f64)>,
    pub speed: f64,
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
            predictions: vec![TBD; schedule.entries.len()],
            display_update: true,
            ..Runtime::default()
        }
    }

    /// `submitPredictions`: when the train is next at each station of its schedule, from the
    /// transit times it measured and the delays it waits.
    pub fn predict(&self, schedule: &Schedule, at: &Whereabouts) -> Vec<Prediction> {
        let mut out: Vec<Option<Prediction>> = Vec::new();
        let count = schedule.entries.len();
        let mut time: i32 = 0;
        let current = self.current;
        if self.state == State::PostTransit || current >= count {
            if let Some(station) = &at.station {
                out.push(self.prediction(schedule, current, station, &self.title, 0));
            }
            let stay = stay_duration(schedule, current);
            time = if stay == INVALID {
                INVALID
            } else {
                time.wrapping_add(stay)
            };
        } else if let Some((name, distance, started)) = &at.destination {
            // `(int) (distance / speed) * 2`, in Java's arithmetic.
            let mut remaining = java_int(distance / at.speed).wrapping_mul(2);
            if self.predictions.len() > current && *started != 0.0 {
                let mut predicted = self.predictions[current] as f32;
                if predicted > 0.0 {
                    predicted *= (distance / started).clamp(0.0, 1.0) as f32;
                    remaining = remaining.wrapping_add(predicted as i32) / 2;
                }
            }
            time = time.wrapping_add(remaining);
            out.push(self.prediction(schedule, current, name, &self.title, time));
            let stay = stay_duration(schedule, current);
            time = if stay != INVALID {
                time.wrapping_add(stay)
            } else {
                INVALID
            };
        } else {
            time = self.predict_entry(schedule, current, &self.title, time, &mut out);
        }
        let mut title = self.title.clone();
        for i in 1..count {
            let index = (i + current) % count;
            if index == 0 && !schedule.cyclic {
                break;
            }
            if let Instruction::Rename(t) = &schedule.entries[index].instruction {
                title.clone_from(t);
                continue;
            }
            time = self.predict_entry(schedule, index, &title, time, &mut out);
        }
        out.into_iter().flatten().collect()
    }

    /// `predictForEntry`.
    fn predict_entry(
        &self,
        schedule: &Schedule,
        index: usize,
        title: &str,
        mut time: i32,
        out: &mut Vec<Option<Prediction>>,
    ) -> i32 {
        let Some(Instruction::Destination(filter)) =
            schedule.entries.get(index).map(|e| &e.instruction)
        else {
            return time;
        };
        if self.predictions.len() <= self.current {
            return time;
        }
        let stay = stay_duration(schedule, index);
        if time < 0 {
            out.push(self.prediction(schedule, index, filter, title, time));
            return time.min(stay);
        }
        let predicted = self.predictions.get(index).copied().unwrap_or(TBD);
        time = time.wrapping_add(predicted);
        if predicted == TBD {
            time = TBD;
        }
        out.push(self.prediction(schedule, index, filter, title, time));
        if time != TBD {
            time = time.wrapping_add(stay);
        }
        if stay == INVALID {
            time = INVALID;
        }
        time
    }

    /// `createPrediction`: none for an `INVALID` time; the title is the next station
    /// instruction's filter without its `*` when the schedule has none.
    fn prediction(
        &self,
        schedule: &Schedule,
        index: usize,
        destination: &str,
        title: &str,
        ticks: i32,
    ) -> Option<Prediction> {
        if ticks == INVALID {
            return None;
        }
        let size = schedule.entries.len();
        let mut index = index;
        if index >= size {
            if !schedule.cyclic {
                return Some(Prediction {
                    ticks,
                    title: " ".to_owned(),
                    destination: destination.to_owned(),
                });
            }
            index %= size.max(1);
        }
        let mut text = title.to_owned();
        if text.trim().is_empty() {
            for i in 1..size {
                let j = (index + i) % size;
                if let Instruction::Destination(f) = &schedule.entries[j].instruction {
                    text = f.replace('*', "").trim().to_owned();
                    break;
                }
            }
        }
        Some(Prediction {
            ticks,
            title: text,
            destination: destination.to_owned(),
        })
    }

    /// `tick`: `navigating` while the train has a destination.
    pub fn tick(
        &mut self,
        schedule: &Schedule,
        navigating: bool,
        inputs: Inputs,
        probes: &Probes,
    ) -> Order {
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
                self.tick_conditions(schedule, inputs, probes);
                Order::Nothing
            }
            State::PreTransit => self.start_current(schedule, probes),
        }
    }

    /// The conditions the glue must measure before this tick: (group, row) of each group's
    /// current condition that reads the world, when it will be looked at (a cargo threshold
    /// every 20 ticks, `LazyTickedScheduleCondition`).
    pub fn probes_wanted(&self, schedule: &Schedule) -> Vec<(usize, usize)> {
        if self.paused || self.state != State::PostTransit || self.cooldown > 0 {
            return Vec::new();
        }
        let Some(entry) = schedule.entries.get(self.current) else {
            return Vec::new();
        };
        entry
            .conditions
            .iter()
            .enumerate()
            .filter_map(|(g, group)| {
                let (done, time) = self.progress.get(g).copied().unwrap_or((0, 0));
                let c = group.get(done)?;
                (c.probed() && (!c.lazy() || time % LAZY == 0)).then_some((g, done))
            })
            .collect()
    }

    /// Whether the glue must find stations for a package instruction before this tick.
    pub fn packages_wanted(&self, schedule: &Schedule) -> bool {
        !self.paused
            && self.state == State::PreTransit
            && self.cooldown <= 0
            && schedule
                .entries
                .get(self.current)
                .is_some_and(|e| e.instruction.packages())
    }

    /// `startCurrentInstruction`: a station is looked for; a title or throttle takes effect
    /// and the next entry follows.
    fn start_current(&mut self, schedule: &Schedule, probes: &Probes) -> Order {
        if self.end_reached(schedule) {
            return Order::Nothing;
        }
        match &schedule.entries[self.current].instruction {
            Instruction::Destination(filter) => Order::Navigate(filter.clone()),
            // `DeliverPackagesInstruction.start`: no station takes the packages aboard: wait
            // and look again, or with none aboard, on to the next entry.
            Instruction::DeliverPackages => match &probes.packages {
                Some((stations, _)) if !stations.is_empty() => Order::NavigateTo(stations.clone()),
                Some((_, true)) | None => {
                    self.no_path();
                    Order::Nothing
                }
                Some((_, false)) => {
                    self.current += 1;
                    Order::Nothing
                }
            },
            // `FetchPackagesInstruction.start`: no station has packages for it: on to the
            // next entry after a wait.
            Instruction::FetchPackages(_) => match &probes.packages {
                Some((stations, _)) if !stations.is_empty() => Order::NavigateTo(stations.clone()),
                _ => {
                    self.no_path();
                    self.current += 1;
                    Order::Nothing
                }
            },
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
        self.display_update = true;
        // The transit time measured, averaged with the last one.
        if self.ticks_in_transit > 0
            && let Some(p) = self.predictions.get_mut(self.current)
        {
            if *p > 0 {
                self.ticks_in_transit = (*p + self.ticks_in_transit) / 2;
            }
            *p = self.ticks_in_transit;
        }
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
    fn tick_conditions(&mut self, schedule: &Schedule, inputs: Inputs, probes: &Probes) {
        let entry = &schedule.entries[self.current];
        if !entry.instruction.supports_conditions() {
            self.state = State::PreTransit;
            self.current += 1;
            return;
        }
        if self.progress.len() != entry.conditions.len() {
            self.progress = vec![(0, 0); entry.conditions.len()];
        }
        for (g, (group, (done, time))) in entry
            .conditions
            .iter()
            .zip(self.progress.iter_mut())
            .enumerate()
        {
            if *done >= group.len() {
                self.state = State::PreTransit;
                self.current += 1;
                return;
            }
            let measure = probes.measure((g, *done));
            // `requestDisplayIfNecessary`: the first group's status shows a new time.
            let timed = |time: i32, total: i32| {
                let left = total - time;
                (left < 20 * 60 && left % 100 == 0) || (left >= 20 * 60 && left % (20 * 60) == 0)
            };
            let first = g == 0;
            let met = match &group[*done] {
                Condition::Delay { value, unit } => {
                    let total = Condition::ticks(*value, *unit);
                    if *time >= total {
                        true
                    } else {
                        self.display_update |= first && timed(*time, total);
                        *time += 1;
                        false
                    }
                }
                Condition::Idle { value, unit } => {
                    *time = inputs.idle_cargo;
                    let total = Condition::ticks(*value, *unit);
                    self.display_update |= first && timed(*time, total);
                    inputs.idle_cargo > total
                }
                Condition::TimeOfDay {
                    hour,
                    minute,
                    rotation,
                } => {
                    let r = rotation_ticks(*rotation);
                    let day = inputs.day_time.rem_euclid(r);
                    let target = ((i64::from((hour + 18) % 24)) * 1000
                        + (f64::from(*minute) / 60.0 * 1000.0).ceil() as i64)
                        % r;
                    let diff = day - target;
                    (0..=40).contains(&diff)
                }
                Condition::PlayerCount { count, exact } => {
                    // `PrevPlayerCount` (kept in the condition's time): a change shows.
                    self.display_update |= first && *time != inputs.players;
                    *time = inputs.players;
                    if *exact {
                        inputs.players == *count
                    } else {
                        inputs.players >= *count
                    }
                }
                Condition::Powered => inputs.powered,
                // `LazyTickedScheduleCondition`: looked at every 20 ticks.
                c @ (Condition::ItemThreshold { op, threshold, .. }
                | Condition::FluidThreshold { op, threshold, .. }) => {
                    let unit = if matches!(c, Condition::FluidThreshold { .. }) {
                        1000
                    } else {
                        1
                    };
                    let looked = *time % LAZY == 0;
                    let met = looked && op.test(measure, threshold * unit);
                    if !met {
                        *time = if looked { 1 } else { *time + 1 };
                    }
                    met
                }
                Condition::RedstoneLink { inverted, .. } => (measure != 0) != *inverted,
                Condition::Unloaded => measure != 0,
            };
            if met {
                *done += 1;
                *time = 0;
                self.display_update |= first;
            }
        }
    }
}

/// `estimateStayDuration`: the ticks the train waits at entry `index`: the first condition
/// group made only of delays (their sum), else [`INVALID`] (past the end: the first entry's of a
/// cyclic schedule).
pub fn stay_duration(schedule: &Schedule, index: usize) -> i32 {
    let mut index = index;
    if index >= schedule.entries.len() {
        if !schedule.cyclic {
            return INVALID;
        }
        index = 0;
    }
    let Some(entry) = schedule.entries.get(index) else {
        return INVALID;
    };
    'groups: for group in &entry.conditions {
        let mut total: i32 = 0;
        for c in group {
            let Condition::Delay { value, unit } = c else {
                continue 'groups;
            };
            total = total.wrapping_add(Condition::ticks(*value, *unit));
        }
        return total;
    }
    INVALID
}

/// Java's `(int) d`: NaN is 0, out of range saturates.
fn java_int(d: f64) -> i32 {
    if d.is_nan() { 0 } else { d as i32 }
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

    fn none() -> Probes {
        Probes::default()
    }

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
        assert_eq!(
            r.tick(&s, false, idle(0), &none()),
            Order::Navigate("Depot".into())
        );
        // No path: it looks again 40 ticks later.
        r.no_path();
        let asks = (0..41)
            .filter(|_| r.tick(&s, false, idle(0), &none()) != Order::Nothing)
            .count();
        assert_eq!(asks, 1);
        r.started();
        for _ in 0..30 {
            assert_eq!(r.tick(&s, true, idle(0), &none()), Order::Nothing);
        }
        assert_eq!(r.ticks_in_transit, 30);
        r.arrived(&s);
        // 5 s: 100 ticks counting, the 101st sees it done, the next moves on.
        let mut ticks = 0;
        while r.current == 0 {
            r.tick(&s, false, idle(0), &none());
            ticks += 1;
        }
        assert_eq!(ticks, 102);
        assert_eq!(
            r.tick(&s, false, idle(0), &none()),
            Order::Navigate("Mine, Platform *".into())
        );
        r.started();
        r.arrived(&s);
        // Cargo idle for more than 3 s ends the wait before the minute.
        let mut ticks = 0;
        while r.current == 1 {
            r.tick(&s, false, idle(ticks), &none());
            ticks += 1;
        }
        assert!(ticks < 70, "{ticks}");
        // Cyclic: back to the first entry.
        assert_eq!(r.tick(&s, false, idle(0), &none()), Order::Nothing);
        assert_eq!(r.current, 0);
        assert!(!r.paused);
        assert_eq!(
            r.tick(&s, false, idle(0), &none()),
            Order::Navigate("Depot".into())
        );
    }

    #[test]
    fn a_one_way_schedule_completes() {
        let s = shuttle(false);
        let mut r = Runtime {
            current: 2,
            ..Runtime::new()
        };
        assert_eq!(r.tick(&s, false, idle(0), &none()), Order::Nothing);
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
        assert_eq!(r.tick(&s, false, idle(0), &none()), Order::Nothing);
        assert_eq!(r.title, "Express");
        assert_eq!(r.tick(&s, false, idle(0), &none()), Order::Throttle(50));
        assert_eq!(
            r.tick(&s, false, idle(0), &none()),
            Order::Navigate("Depot".into())
        );
        r.already_there(&s);
        // Unpowered it waits; powered it is done.
        for _ in 0..10 {
            r.tick(&s, false, idle(0), &none());
        }
        assert_eq!(r.current, 2);
        let on = Inputs {
            powered: true,
            ..Inputs::default()
        };
        r.tick(&s, false, on, &none());
        r.tick(&s, false, on, &none());
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
        r.tick(&s, false, at(0, 0), &none());
        r.already_there(&s);
        r.tick(&s, false, at(1999, 3), &none());
        r.tick(&s, false, at(1999, 3), &none());
        assert_eq!(r.current, 0);
        r.tick(&s, false, at(24_000 + 2010, 0), &none());
        r.tick(&s, false, at(24_000 + 2011, 0), &none());
        assert_eq!(r.current, 1);
        let mut r = Runtime::new();
        r.tick(&s, false, at(0, 0), &none());
        r.already_there(&s);
        r.tick(&s, false, at(0, 2), &none());
        r.tick(&s, false, at(0, 2), &none());
        assert_eq!(r.current, 1);
    }

    #[test]
    fn cargo_thresholds_look_every_20_ticks() {
        let s = Schedule {
            entries: vec![Entry::station(
                "A",
                vec![
                    vec![Condition::ItemThreshold {
                        op: Op::Greater,
                        threshold: 10,
                        stacks: false,
                        item: Stack::new(),
                    }],
                    vec![Condition::FluidThreshold {
                        op: Op::Less,
                        threshold: 2,
                        item: Stack::new(),
                    }],
                ],
            )],
            cyclic: true,
            saved_progress: 0,
        };
        let mut r = Runtime::new();
        r.tick(&s, false, idle(0), &none());
        r.already_there(&s);
        // Looked at once at arrival: 5 items, 3000 mB: neither met.
        let at = |items, fluid| Probes {
            measures: vec![((0, 0), items), ((1, 0), fluid)],
            packages: None,
        };
        assert_eq!(r.probes_wanted(&s), vec![(0, 0), (1, 0)]);
        r.tick(&s, false, idle(0), &at(5, 3000));
        assert_eq!(r.current, 0);
        // Not looked at again for 19 ticks, whatever the cargo.
        for _ in 0..19 {
            assert!(r.probes_wanted(&s).is_empty());
            r.tick(&s, false, idle(0), &at(11, 0));
        }
        assert_eq!(r.current, 0);
        assert_eq!(r.probes_wanted(&s), vec![(0, 0), (1, 0)]);
        r.tick(&s, false, idle(0), &at(11, 3000));
        r.tick(&s, false, idle(0), &none());
        assert_eq!(r.current, 1);
    }

    #[test]
    fn links_and_unloaded_stations_are_read_each_tick() {
        let s = Schedule {
            entries: vec![Entry::station(
                "A",
                vec![vec![
                    Condition::RedstoneLink {
                        freq: [Stack::new(), Stack::new()],
                        inverted: true,
                    },
                    Condition::Unloaded,
                ]],
            )],
            cyclic: true,
            saved_progress: 0,
        };
        let mut r = Runtime::new();
        r.tick(&s, false, idle(0), &none());
        r.already_there(&s);
        let m = |v| Probes {
            measures: vec![((0, 0), v), ((0, 1), v)],
            packages: None,
        };
        // A powered network does not meet an inverted link.
        r.tick(&s, false, idle(0), &m(1));
        assert_eq!(r.progress[0].0, 0);
        r.tick(&s, false, idle(0), &m(0));
        assert_eq!(r.progress[0].0, 1);
        assert_eq!(r.probes_wanted(&s), vec![(0, 1)]);
        r.tick(&s, false, idle(0), &m(0));
        assert_eq!(r.progress[0].0, 1);
        r.tick(&s, false, idle(0), &m(1));
        r.tick(&s, false, idle(0), &none());
        assert_eq!(r.current, 1);
    }

    #[test]
    fn package_instructions_go_where_the_glue_found() {
        let s = Schedule {
            entries: vec![
                Entry {
                    instruction: Instruction::DeliverPackages,
                    conditions: vec![vec![Condition::Powered]],
                },
                Entry {
                    instruction: Instruction::FetchPackages("Home*".into()),
                    conditions: vec![vec![Condition::Powered]],
                },
            ],
            cyclic: true,
            saved_progress: 0,
        };
        let found = |stations: Vec<[i32; 3]>, any| Probes {
            measures: Vec::new(),
            packages: Some((stations, any)),
        };
        let mut r = Runtime::new();
        assert!(r.packages_wanted(&s));
        // Packages aboard but no postbox for them: it waits and looks again.
        assert_eq!(
            r.tick(&s, false, idle(0), &found(vec![], true)),
            Order::Nothing
        );
        assert_eq!((r.current, r.cooldown), (0, INTERVAL));
        r.cooldown = 0;
        assert_eq!(
            r.tick(&s, false, idle(0), &found(vec![[1, 2, 3]], true)),
            Order::NavigateTo(vec![[1, 2, 3]])
        );
        // Nothing aboard: on to fetching; nothing to fetch: on again after a wait.
        let mut r = Runtime::new();
        r.tick(&s, false, idle(0), &found(vec![], false));
        assert_eq!(r.current, 1);
        r.tick(&s, false, idle(0), &found(vec![], false));
        assert_eq!((r.current, r.cooldown), (2, INTERVAL));
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

    fn delay(seconds: i32) -> Vec<Vec<Condition>> {
        vec![vec![Condition::Delay {
            value: seconds,
            unit: TimeUnit::Seconds,
        }]]
    }

    #[test]
    fn transit_times_are_measured_and_averaged() {
        let s = Schedule {
            entries: vec![Entry::station("A", delay(1)), Entry::station("B", delay(1))],
            ..Schedule::default()
        };
        let mut r = Runtime::for_schedule(&s, false);
        assert_eq!(r.predictions, vec![TBD, TBD]);
        r.started();
        r.ticks_in_transit = 100;
        r.arrived(&s);
        assert_eq!(r.predictions, vec![100, TBD]);
        assert!(r.display_update);
        r.state = State::InTransit;
        r.ticks_in_transit = 140;
        r.arrived(&s);
        assert_eq!(r.predictions[0], 120, "averaged with the last");
    }

    #[test]
    fn predictions_follow_create() {
        let s = Schedule {
            entries: vec![
                Entry::station("South", delay(10)),
                Entry::station("North", delay(10)),
            ],
            ..Schedule::default()
        };
        // Waiting at North (entry 1), both transits measured at 123 ticks.
        let mut r = Runtime::for_schedule(&s, false);
        r.current = 1;
        r.state = State::PostTransit;
        r.predictions = vec![123, 123];
        let at = Whereabouts {
            station: Some("North".into()),
            ..Whereabouts::default()
        };
        let p = r.predict(&s, &at);
        assert_eq!(
            p,
            vec![
                Prediction {
                    ticks: 0,
                    title: "South".into(),
                    destination: "North".into()
                },
                Prediction {
                    ticks: 200 + 123,
                    title: "North".into(),
                    destination: "South".into()
                },
            ]
        );
        // On its way to South, half way, at 0.7 blocks a tick: the estimate is averaged with the
        // measured time scaled by the distance left.
        r.state = State::InTransit;
        r.current = 0;
        let at = Whereabouts {
            station: None,
            destination: Some(("South".into(), 17.0, 34.0)),
            speed: 0.7,
        };
        let p = r.predict(&s, &at);
        let remaining = ((17.0f64 / 0.7) as i32) * 2;
        let first = (remaining + (123.0f32 * 0.5) as i32) / 2;
        assert_eq!(p[0].ticks, first);
        assert_eq!(p[1].ticks, first + 200 + 123);
        // Unknown transit times: TBD on, and a condition group that is no delay ends it.
        let mut r = Runtime::for_schedule(&s, false);
        r.current = 1;
        r.state = State::PostTransit;
        let p = r.predict(&s, &Whereabouts::default());
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].ticks, TBD);
        let s2 = Schedule {
            entries: vec![
                Entry::station("A", vec![vec![Condition::Powered]]),
                Entry::station("B", delay(1)),
            ],
            ..Schedule::default()
        };
        let mut r = Runtime::for_schedule(&s2, false);
        r.state = State::PostTransit;
        r.predictions = vec![50, 50];
        let at = Whereabouts {
            station: Some("A".into()),
            ..Whereabouts::default()
        };
        assert_eq!(r.predict(&s2, &at).len(), 1, "no stay is known after A");
    }
}
