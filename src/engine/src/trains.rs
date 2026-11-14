//! Create's trains on the track graph (`Train`, `Carriage`, `CarriageBogey`, `Navigation`,
//! `TravellingPoint`, `ScheduleRuntime`'s driving, `StationBlockEntity.assemble`): a train is
//! carriages on one or two bogeys each, every bogey two wheel points on the graph; carriages keep
//! their spacing by Create's coupling stress, bogeys their wheel spacing by theirs. It finds its
//! way to a station with Create's search (`findPathTo`, its penalties for stations, trains, red
//! and redstone-red signals, never the wrong way past a one-sided signal), drives there with
//! Create's speed control (top speed, acceleration, braking distance, the slow approach, slowing
//! for curves, waiting before an entry signal whose section is held or a cross signal whose chain
//! is, reserving the sections it is about to enter) and stops with its front just past the
//! station. A schedule sends it from station to station; a player's controls drive it by hand.

use crate::bezier::Vec3;
use crate::graph::{Graph, Point};
use crate::network::{Key, Mark, Network, SignalType, key_position, side_of};
use crate::schedule::{Order, Runtime, Schedule, station_matches};
use crate::track::TrackShape;
use std::collections::{BinaryHeap, HashMap, HashSet};

/// `trainTopSpeed` (28 m/s), `trainTurningTopSpeed` (14 m/s) and `trainAcceleration` (3 m/s²)
/// in blocks a tick.
pub const TOP_SPEED: f64 = 28.0 / 20.0;
pub const TURN_TOP_SPEED: f64 = 14.0 / 20.0;
pub const ACCELERATION: f64 = 3.0 / 400.0;
/// `manualTrainSpeedModifier`: a train driven by hand runs at 75% of a scheduled one's speed.
pub const MANUAL_SPEED: f64 = 0.75;
/// `StandardBogeyBlock.getWheelPointSpacing` (both sizes).
pub const WHEEL_SPACING: f64 = 2.0;
/// `maxAssemblyLength` and `maxBogeyCount`.
pub const MAX_ASSEMBLY_LENGTH: i32 = 128;
pub const MAX_BOGEY_COUNT: usize = 20;
/// `Train.Penalties`.
pub const STATION: i32 = 50;
pub const STATION_WITH_TRAIN: i32 = 300;
pub const IDLE_TRAIN: i32 = 700;
pub const ARRIVING_TRAIN: i32 = 50;
pub const WAITING_TRAIN: i32 = 50;
pub const ANY_TRAIN: i32 = 25;
pub const RED_SIGNAL: i32 = 25;
pub const REDSTONE_RED_SIGNAL: i32 = 400;
/// GlobalStation's name until one is given.
pub const DEFAULT_STATION_NAME: &str = "Track Station";

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mid(a: Vec3, b: Vec3) -> Vec3 {
    [
        (a[0] + b[0]) / 2.0,
        (a[1] + b[1]) / 2.0,
        (a[2] + b[2]) / 2.0,
    ]
}
fn length(a: Vec3) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn distance(a: Vec3, b: Vec3) -> f64 {
    length(sub(a, b))
}

/// `BogeySizes`: a standard bogey's size, by its block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BogeySize {
    #[default]
    Small,
    Large,
}

impl BogeySize {
    /// `StandardBogeyBlock.getWheelRadius`.
    pub fn wheel_radius(self) -> f64 {
        match self {
            BogeySize::Small => 6.5 / 16.0,
            BogeySize::Large => 12.5 / 16.0,
        }
    }
}

/// A carriage (`Carriage`): its first bogey (by index into the train's bogeys), whether it
/// stands on a second one right behind it, and how far apart the two are (`bogeySpacing`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CarriageShape {
    pub first: usize,
    pub two: bool,
    pub bogey_spacing: i32,
}

/// Where a train is going (`Navigation`): the station, how far its leading point still has to
/// go, the edges to take at junctions on the way (`currentPath`), whether it backs there
/// (`destinationBehindTrain`), and the signal it waits before, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct Nav {
    pub station: [i32; 3],
    pub distance: f64,
    pub path: Vec<usize>,
    pub behind: bool,
    pub signal: Option<f64>,
    pub ticks_waiting: i32,
}

/// A player's hold on a train's controls (`CarriageContraptionEntity.control`): the throttle
/// (the forward or back key, scaled by the train's throttle; 0 lets go), the steering at
/// junctions (1 left, -1 right), and whether the controls face the train's back (`inverted`:
/// driven toward its back at full speed, toward its front at a quarter).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Manual {
    pub throttle: f64,
    pub steer: i8,
    pub inverted: bool,
}

/// A point as saved and found again when the graph is built anew: its edge by its nodes, how
/// far along, and where it was in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub from: Key,
    pub to: Key,
    pub position: f64,
    pub at: Vec3,
}

/// A train's motion: its wheel points (two per bogey, the front bogey's first, all facing its
/// front), its bogeys and carriages and the spacing between carriages (`carriageSpacing`), its
/// speed (blocks a tick, positive toward its front), where it is going and the station it
/// stands at.
#[derive(Clone, Debug, PartialEq)]
pub struct Train {
    pub points: Vec<Point>,
    pub bogeys: Vec<BogeySize>,
    pub carriages: Vec<CarriageShape>,
    pub spacing: Vec<i32>,
    /// Controls face both ways (`doubleEnded`): it may drive backwards to a station.
    pub double_ended: bool,
    pub speed: f64,
    pub nav: Option<Nav>,
    pub at_station: Option<[i32; 3]>,
    pub manual: Option<Manual>,
    /// The share of its top speed a driver may take (`throttle`, set by the HUD's wheel in
    /// 1/18 steps; 1 at first).
    pub throttle: f64,
    /// Sections it holds ahead of a cross signal (`reservedSignalBlocks`).
    pub reserved: Vec<usize>,
    /// The points by their nodes, to find them on a graph built anew; `bound` while `points`
    /// are on the current graph.
    pub placed: Vec<Placed>,
    pub bound: bool,
}

/// What a tick did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
    Still,
    Moving,
    Arrived,
    /// Came to the end of the track (`status.endOfTrack`).
    Blocked,
}

/// A path found (`DiscoveredPath`).
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub distance: f64,
    pub cost: f64,
    pub path: Vec<usize>,
    pub station: usize,
    pub behind: bool,
}

/// The signal sections in a tick (`SignalEdgeGroup`): which trains stand in each (two distinct
/// ones are enough to tell whether another train than the one asking does), the boundary each is
/// reserved for this tick (`reserved`, cleared every tick as Create's `GlobalRailwayManager.tick`
/// does), and each boundary side held red by redstone (`isForcedRed`).
#[derive(Clone, Debug, Default)]
pub struct Occupancy {
    slots: Vec<[u32; 2]>,
    reserved: Vec<Option<usize>>,
    pub forced: Vec<[bool; 2]>,
}

impl Occupancy {
    pub fn fill(&mut self, net: &Network, trains: &[TrainOnTrack]) {
        self.slots.clear();
        self.slots.resize(net.section_slots(), [0, 0]);
        self.reserved.clear();
        self.reserved.resize(net.section_slots(), None);
        if net.signals.is_empty() {
            return;
        }
        for t in trains.iter().filter(|t| t.train.bound) {
            let sections = t
                .train
                .points
                .iter()
                .map(|p| net.section(p.edge, p.position))
                .chain(t.train.reserved.iter().copied());
            for k in sections {
                let Some(s) = self.slots.get_mut(k) else {
                    continue;
                };
                if s[0] == 0 || s[0] == t.id {
                    s[0] = t.id;
                } else if s[1] == 0 {
                    s[1] = t.id;
                }
            }
        }
    }

    /// `SignalEdgeGroup.isOccupiedUnless(train)`: reserved, or held by another train.
    pub fn occupied_unless(&self, section: usize, me: u32) -> bool {
        self.reserved.get(section).is_some_and(Option::is_some)
            || self
                .slots
                .get(section)
                .is_some_and(|s| (s[0] != 0 && s[0] != me) || (s[1] != 0 && s[1] != me))
    }

    /// `SignalEdgeGroup.isOccupiedUnless(boundary)`: any train in it, or reserved through
    /// another boundary.
    pub fn occupied_unless_boundary(&self, section: usize, boundary: usize) -> bool {
        self.slots.get(section).is_some_and(|s| s[0] != 0)
            || self
                .reserved
                .get(section)
                .is_some_and(|r| r.is_some_and(|r| r != boundary))
    }

    /// `signalEdgeGroup.reserved = signal`.
    pub fn reserve(&mut self, section: usize, boundary: usize) {
        if let Some(r) = self.reserved.get_mut(section) {
            *r = Some(boundary);
        }
    }

    /// `isForcedRed(side)`: a signal block of that side is powered.
    pub fn forced_red(&self, boundary: usize, side: usize) -> bool {
        self.forced.get(boundary).is_some_and(|f| f[side])
    }
}

/// A curve the train must slow for: not a straight, gentle slope (`controlSignalScout`'s turn
/// listener).
fn slows(g: &Graph, e: usize) -> bool {
    let Some(c) = &g.edges[e].turn else {
        return false;
    };
    let v = (c.starts[0][1] - c.starts[1][1]).abs();
    if v > 1.0 / 16.0 {
        let (a, b) = (c.axes[0], c.axes[1]);
        let flat = length([a[0] + b[0], 0.0, a[2] + b[2]]);
        if flat < 1.0 / 64.0 && v / c.length < 0.225 {
            return false;
        }
    }
    true
}

/// `TravellingPoint.follow(other)`: at a junction, the way toward the edge the point ahead is
/// on (searching up to 20 edges down each way), else the first.
fn follow(g: &Graph, target: usize, candidates: &[usize]) -> usize {
    if let Some(i) = candidates.iter().position(|&c| c == target) {
        return i;
    }
    for (i, &c) in candidates.iter().enumerate() {
        let mut seen = vec![c];
        let mut frontier = vec![c];
        for _ in 0..20 {
            let mut next = Vec::new();
            for &e in &frontier {
                for n in g.onward_iter(e) {
                    if n == target {
                        return i;
                    }
                    if !seen.contains(&n) {
                        seen.push(n);
                        next.push(n);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
    }
    0
}

/// `TravellingPoint.steer(direction, up)`: at a junction, the way whose start lies closest to
/// the side asked for (`targetDot`: -1 left, 0 straight on, 1 right) of the edge it leaves.
fn steer(g: &Graph, from: usize, steer: i8, candidates: &[usize]) -> usize {
    let target = -f64::from(steer.signum());
    let t = g.direction(from, false);
    // `trajectory.cross(up)` with up = (0, 1, 0).
    let normal = [-t[2], 0.0, t[0]];
    let mut best = 0;
    let mut closest = f64::MAX;
    for (i, &c) in candidates.iter().enumerate() {
        let d = g.direction(c, true);
        let dot = normal[0] * d[0] + normal[2] * d[2];
        let diff = (target - dot).abs();
        if diff > closest {
            continue;
        }
        closest = diff;
        best = i;
    }
    best
}

/// How the train's front point chooses at junctions: along a path (`navigateOptions`), or by the
/// controls' steering (`steer`).
pub enum Control<'a> {
    Path(&'a mut Vec<usize>),
    Steer(i8),
}

impl Control<'_> {
    fn select(&mut self, g: &Graph, from: usize, c: &[usize]) -> usize {
        match self {
            Control::Path(path) => {
                if let Some(&want) = path.first()
                    && let Some(i) = c.iter().position(|&e| e == want)
                {
                    path.remove(0);
                    return i;
                }
                0
            }
            Control::Steer(s) => steer(g, from, *s, c),
        }
    }
}

/// The search frontier (`Navigation.FrontierEntry`), cheapest first.
struct Frontier {
    key: f64,
    distance: f64,
    penalty: i32,
    has_destination: bool,
    edge: usize,
}
impl PartialEq for Frontier {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl Eq for Frontier {}
impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Frontier {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.key.total_cmp(&self.key)
    }
}

/// What a search knows of the world besides the graph: the train asking, the penalties other
/// trains put on edges, the stations a train stands at, sections held and how long it waited.
pub struct SearchContext<'a> {
    pub me: u32,
    pub penalties: &'a HashMap<usize, i32>,
    pub present: &'a HashMap<[i32; 3], u32>,
    pub occupancy: &'a Occupancy,
    pub ticks_waiting: i32,
}

/// The penalty a signal boundary passed along `edge` adds to a search (`Navigation.search`), or
/// None when no train may pass it that way (`canNavigateVia`: no signal block on that side).
fn signal_penalty(
    net: &Network,
    cx: &SearchContext,
    b: usize,
    edge: usize,
    weight: &mut i32,
) -> Option<i32> {
    let side = side_of(edge);
    if net.signals[b].blocks[side].is_empty() {
        return None;
    }
    if cx.occupancy.forced_red(b, side) {
        return Some(REDSTONE_RED_SIGNAL);
    }
    if cx
        .occupancy
        .occupied_unless_boundary(net.beyond(b, side), b)
    {
        let p = *weight;
        *weight /= 2;
        return Some(p);
    }
    Some(0)
}

/// `Navigation.search` from `start` (facing the way it goes) for the first of `targets`
/// (station indices) it reaches, by distance plus penalties plus the octile distance left.
pub fn search(net: &Network, start: Point, targets: &[usize], cx: &SearchContext) -> Option<Found> {
    let g = &net.graph;
    let initial = start.edge;
    let to_node2 = g.length(initial) - start.position;
    let mut signal_weight = (cx.ticks_waiting * 2).clamp(RED_SIGNAL, 200);
    let mut penalty = cx.penalties.get(&initial).copied().unwrap_or(0);
    for &(pos, mark) in &net.marks[initial] {
        if pos < start.position {
            continue;
        }
        match mark {
            Mark::Signal(b) => {
                penalty += signal_penalty(net, cx, b, initial, &mut signal_weight)?;
            }
            Mark::Observer(_) => {}
            Mark::Station(i) => {
                let present = cx.present.get(&net.stations[i].0).copied();
                let own = present == Some(cx.me);
                if present.is_some() && !own {
                    penalty += STATION_WITH_TRAIN;
                }
                if targets.contains(&i) {
                    return Some(Found {
                        distance: pos - start.position,
                        cost: to_node2 + f64::from(penalty),
                        path: Vec::new(),
                        station: i,
                        behind: false,
                    });
                }
                if !own {
                    penalty += STATION;
                }
            }
        }
    }
    let mut frontier = BinaryHeap::new();
    frontier.push(Frontier {
        key: to_node2 + f64::from(penalty),
        distance: to_node2,
        penalty,
        has_destination: false,
        edge: initial,
    });
    let mut visited: HashSet<usize> = HashSet::new();
    let mut reached: HashMap<usize, (bool, usize)> = HashMap::new();
    while let Some(entry) = frontier.pop() {
        if !visited.insert(entry.edge) {
            continue;
        }
        let e = entry.edge;
        if entry.has_destination {
            for &(pos, mark) in &net.marks[e] {
                let Mark::Station(i) = mark else { continue };
                if !targets.contains(&i) {
                    continue;
                }
                let mut path = Vec::new();
                let mut at = e;
                while at != initial {
                    let Some(&(junction, previous)) = reached.get(&at) else {
                        break;
                    };
                    if junction {
                        path.push(at);
                    }
                    at = previous;
                }
                path.reverse();
                return Some(Found {
                    distance: entry.distance - (g.length(e) - pos),
                    cost: f64::from(entry.penalty),
                    path,
                    station: i,
                    behind: false,
                });
            }
        }
        let next: Vec<usize> = g.onward(e);
        'next: for &n in &next {
            let mut penalty = entry.penalty + cx.penalties.get(&n).copied().unwrap_or(0);
            let distance = entry.distance + g.length(n);
            let mut has_destination = false;
            for &(_, mark) in &net.marks[n] {
                match mark {
                    Mark::Signal(b) => {
                        match signal_penalty(net, cx, b, n, &mut signal_weight) {
                            Some(p) => penalty += p,
                            // `continue Search`: not this way.
                            None => continue 'next,
                        }
                    }
                    Mark::Observer(_) => {}
                    Mark::Station(i) => {
                        let present = cx.present.get(&net.stations[i].0).copied();
                        let own = present == Some(cx.me);
                        if present.is_some() && !own {
                            penalty += STATION_WITH_TRAIN;
                        }
                        if targets.contains(&i) {
                            has_destination = true;
                            break;
                        }
                        if !own {
                            penalty += STATION;
                        }
                    }
                }
            }
            // The octile distance from the new node to each target's first node.
            let node = g.nodes[g.edges[n].to];
            let mut remaining = 0.0f64;
            if !targets.is_empty() {
                remaining = f64::MAX;
                for &t in targets {
                    let loc = net.stations[t].1;
                    let first = g.edges[loc.edge].from;
                    let d = g.nodes[first];
                    let mut v = [
                        (node[0] - d[0]).abs(),
                        (node[1] - d[1]).abs(),
                        (node[2] - d[2]).abs(),
                    ];
                    v.sort_by(f64::total_cmp);
                    let mut r = 0.317_837_245_195_782 * v[0]
                        + 0.414_213_562_373_095 * v[1]
                        + v[2]
                        + loc.position;
                    if g.edges[e].to == first {
                        r -= g.length(n) * 2.0;
                    }
                    remaining = remaining.min(r);
                }
            }
            reached.entry(n).or_insert((next.len() > 1, e));
            frontier.push(Frontier {
                key: distance + f64::from(penalty) + remaining,
                distance,
                penalty,
                has_destination,
                edge: n,
            });
        }
    }
    None
}

/// What lies ahead of a navigating train (`Navigation.tick`'s signal scout): the signal it must
/// wait at (its distance), and the nearest curve.
struct Scouted {
    signal: Option<f64>,
    curve: Option<f64>,
}

/// Why a station can't assemble a train (`AssemblyException`), by Create's message key
/// (`create.train_assembly.<key>`) and its arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssemblyError {
    FrontmostBogeyAtStation,
    BogeysTooClose(usize, usize),
    NothingAttached(usize),
    TooManyBogeys(usize),
    SidewaysControls,
    NotConnectedInOrder,
    NoControls,
    NoBogeys,
}

impl AssemblyError {
    pub fn key(self) -> &'static str {
        match self {
            AssemblyError::FrontmostBogeyAtStation => "frontmost_bogey_at_station",
            AssemblyError::BogeysTooClose(..) => "bogeys_too_close",
            AssemblyError::NothingAttached(_) => "nothing_attached",
            AssemblyError::TooManyBogeys(_) => "too_many_bogeys",
            AssemblyError::SidewaysControls => "sideways_controls",
            AssemblyError::NotConnectedInOrder => "not_connected_in_order",
            AssemblyError::NoControls => "no_controls",
            AssemblyError::NoBogeys => "no_bogeys",
        }
    }

    /// The message's arguments.
    pub fn args(self) -> Vec<String> {
        match self {
            AssemblyError::BogeysTooClose(a, b) => vec![a.to_string(), b.to_string()],
            AssemblyError::NothingAttached(n) | AssemblyError::TooManyBogeys(n) => {
                vec![n.to_string()]
            }
            _ => Vec::new(),
        }
    }
}

/// What a carriage's structure holds (`CarriageContraption.assemble` from one bogey): how many
/// blocks, the bogeys in it (their places along the station's track), and its train controls
/// facing the assembly direction (forward), against it (backward) or across (sideways).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Structure {
    pub blocks: usize,
    pub bogeys: Vec<i32>,
    pub forward: bool,
    pub backward: bool,
    pub sideways: bool,
}

/// A train's layout as a station assembles it (`StationBlockEntity.assemble`): its carriages,
/// the spacing between them, where each wheel point goes (blocks from the end of the station's
/// track toward the assembly direction), and whether it can be driven both ways.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub carriages: Vec<CarriageShape>,
    pub spacing: Vec<i32>,
    pub offsets: Vec<f64>,
    pub double_ended: bool,
}

/// `StationBlockEntity.assemble`'s rules over the bogeys found along the station's track
/// (`refreshAssemblyInfo`: their places, 0 the block next to the station's, nearest first),
/// with `structure(i)` gathering what bogey `i` carries: the frontmost bogey stands at the
/// station, bogeys are at least 3 apart, a carriage holds more than its bogey, one or two bogeys
/// (the second the next one along), no controls sideways, and some controls face forward.
pub fn layout(
    bogeys: &[i32],
    mut structure: impl FnMut(usize) -> Structure,
) -> Result<Layout, AssemblyError> {
    if bogeys.first() != Some(&0) {
        return Err(AssemblyError::FrontmostBogeyAtStation);
    }
    let mut offsets = Vec::with_capacity(bogeys.len() * 2);
    let mut previous = -100;
    for (i, &loc) in bogeys.iter().enumerate() {
        if loc - previous < 3 {
            return Err(AssemblyError::BogeysTooClose(i, i + 1));
        }
        offsets.push(f64::from(loc) + 0.5 - WHEEL_SPACING / 2.0);
        offsets.push(f64::from(loc) + 0.5 + WHEEL_SPACING / 2.0);
        previous = loc;
    }
    if offsets.is_empty() {
        return Err(AssemblyError::NoBogeys);
    }
    let mut carriages = Vec::new();
    let mut spacing = Vec::new();
    let (mut forward, mut backward) = (false, false);
    let mut i = 0;
    while i < bogeys.len() {
        if i > 0 {
            spacing.push(bogeys[i] - bogeys[i - 1]);
        }
        let s = structure(i);
        let count = s.bogeys.len().max(1);
        if s.blocks <= 1 {
            return Err(AssemblyError::NothingAttached(i + 1));
        }
        if count > 2 {
            return Err(AssemblyError::TooManyBogeys(count));
        }
        if s.sideways {
            return Err(AssemblyError::SidewaysControls);
        }
        forward |= s.forward;
        backward |= s.backward;
        let second = s.bogeys.iter().copied().find(|&b| b != bogeys[i]);
        let shape = match second {
            Some(b) => {
                if i == bogeys.len() - 1 || b != bogeys[i + 1] {
                    return Err(AssemblyError::NotConnectedInOrder);
                }
                let shape = CarriageShape {
                    first: i,
                    two: true,
                    bogey_spacing: bogeys[i + 1] - bogeys[i],
                };
                i += 1;
                shape
            }
            None => CarriageShape {
                first: i,
                two: false,
                bogey_spacing: 0,
            },
        };
        carriages.push(shape);
        i += 1;
    }
    if !forward {
        return Err(AssemblyError::NoControls);
    }
    Ok(Layout {
        carriages,
        spacing,
        offsets,
        double_ended: backward,
    })
}

impl Train {
    /// A train of `layout` assembled at a station whose track ends at `origin` (the end of the
    /// station's track block toward `direction`, the assembly direction, a unit along x or z):
    /// each wheel point its offset along `direction` from there, facing back toward the station
    /// (its front). None when a point would be off the track.
    pub fn assembled(
        net: &Network,
        origin: Vec3,
        direction: Vec3,
        layout: &Layout,
        sizes: &[BogeySize],
    ) -> Option<Train> {
        let back = [-direction[0], -direction[1], -direction[2]];
        let mut points = Vec::with_capacity(layout.offsets.len());
        for &o in &layout.offsets {
            let at = [
                origin[0] + direction[0] * o,
                origin[1] + direction[1] * o,
                origin[2] + direction[2] * o,
            ];
            points.push(net.locate(at, back)?);
        }
        let mut t = Train {
            points,
            bogeys: sizes.to_vec(),
            carriages: layout.carriages.clone(),
            spacing: layout.spacing.clone(),
            double_ended: layout.double_ended,
            speed: 0.0,
            nav: None,
            at_station: None,
            manual: None,
            throttle: 1.0,
            reserved: Vec::new(),
            placed: Vec::new(),
            bound: true,
        };
        t.remember(net);
        Some(t)
    }

    /// A train of single-bogey carriages on the track blocks `under` (front first), facing
    /// `front` (a unit along the track), driven both ways: each bogey's wheels a wheel spacing
    /// apart about its block's centre. None when a wheel would be off the track.
    pub fn on_bogeys(net: &Network, under: &[[i32; 3]], front: Vec3) -> Option<Train> {
        let g = &net.graph;
        let mut points = Vec::with_capacity(under.len() * 2);
        let mut spacing = Vec::new();
        for (i, b) in under.iter().enumerate() {
            let loc = net.blocks.get(b)?;
            let centre = Point {
                edge: loc.edge,
                position: loc.position,
                blocked: false,
            };
            let at = centre.at(g);
            let p = net.locate(at, front)?;
            let mut lead = p;
            let mut trail = p;
            lead.travel_signed(g, WHEEL_SPACING / 2.0, |_, _| 0);
            trail.travel_signed(g, -WHEEL_SPACING / 2.0, |_, _| 0);
            if lead.blocked || trail.blocked {
                return None;
            }
            points.push(lead);
            points.push(trail);
            if i > 0 {
                let a = under[i - 1];
                spacing.push((a[0] - b[0]).abs() + (a[2] - b[2]).abs());
            }
        }
        let mut t = Train {
            points,
            bogeys: vec![BogeySize::Small; under.len()],
            carriages: (0..under.len())
                .map(|first| CarriageShape {
                    first,
                    two: false,
                    bogey_spacing: 0,
                })
                .collect(),
            spacing,
            double_ended: true,
            speed: 0.0,
            nav: None,
            at_station: None,
            manual: None,
            throttle: 1.0,
            reserved: Vec::new(),
            placed: Vec::new(),
            bound: true,
        };
        t.remember(net);
        Some(t)
    }

    /// Keeps where the points are by their nodes (before the graph is built anew, and to save).
    pub fn remember(&mut self, net: &Network) {
        if !self.bound {
            return;
        }
        let g = &net.graph;
        self.placed = self
            .points
            .iter()
            .map(|p| {
                let e = &g.edges[p.edge];
                Placed {
                    from: net.keys[e.from],
                    to: net.keys[e.to],
                    position: p.position,
                    at: p.at(g),
                }
            })
            .collect();
    }

    /// Finds the points on `net` again: on the edge between the same nodes, else at the same
    /// place in the world. Its path is found again by the caller.
    pub fn bind(&mut self, net: &Network) -> bool {
        let g = &net.graph;
        let mut points = Vec::with_capacity(self.placed.len());
        for p in &self.placed {
            let same = net.nodes.get(&p.from).and_then(|&a| {
                let b = *net.nodes.get(&p.to)?;
                g.out[a]
                    .iter()
                    .copied()
                    .find(|&e| g.edges[e].to == b && p.position <= g.length(e) + 1.0e-6)
            });
            let point = match same {
                Some(edge) => Some(Point {
                    edge,
                    position: p.position,
                    blocked: false,
                }),
                None => net.locate(p.at, sub(key_position(p.to), key_position(p.from))),
            };
            match point {
                Some(q) => points.push(q),
                None => {
                    self.bound = false;
                    return false;
                }
            }
        }
        self.points = points;
        self.bound = !self.points.is_empty() && self.points.len() == self.bogeys.len() * 2;
        self.bound
    }

    /// The leading point the way it moves (`forward`: toward its front), facing that way.
    pub fn leading(&self, g: &Graph, forward: bool) -> Point {
        if forward {
            self.points[0]
        } else {
            self.points[self.points.len() - 1].flipped(g)
        }
    }

    /// Bogey `i`'s anchor (`getAnchorPosition`: between its wheels).
    pub fn anchor(&self, g: &Graph, i: usize) -> Vec3 {
        mid(self.points[2 * i].at(g), self.points[2 * i + 1].at(g))
    }

    /// Carriage `c`'s last bogey (`trailingBogey`).
    pub fn last_bogey(&self, c: usize) -> usize {
        let s = self.carriages[c];
        s.first + usize::from(s.two)
    }

    /// Carriage `c`'s leading and trailing points (`getLeadingPoint`, `getTrailingPoint`).
    fn ends(&self, c: usize) -> (usize, usize) {
        (2 * self.carriages[c].first, 2 * self.last_bogey(c) + 1)
    }

    /// The pose of carriage `c` (`alignEntity`): its leading bogey's anchor (`positionAnchor`),
    /// and its front's direction from its rotation anchors (its two bogeys' anchors, or its one
    /// bogey's wheels).
    pub fn carriage_pose(&self, g: &Graph, c: usize) -> (Vec3, Vec3) {
        let s = self.carriages[c];
        let (a, b) = if s.two {
            (self.anchor(g, s.first), self.anchor(g, s.first + 1))
        } else {
            (
                self.points[2 * s.first].at(g),
                self.points[2 * s.first + 1].at(g),
            )
        };
        (self.anchor(g, s.first), unit(sub(a, b)))
    }

    /// Bogey `b`'s anchor and the way its wheels line up (front first).
    pub fn bogey_pose(&self, g: &Graph, b: usize) -> (Vec3, Vec3) {
        let (a, c) = (self.points[2 * b].at(g), self.points[2 * b + 1].at(g));
        (mid(a, c), unit(sub(a, c)))
    }

    /// Carriage `c`'s travel (`Carriage.travel`): each wheel of each bogey (from the back when
    /// running backwards) moves what the one before it moved, corrected by its bogey's stress
    /// (`getStress`: wheel spacing less how far apart its wheels are) and the carriage's (bogey
    /// spacing less how far apart its anchors are), so the wheels and bogeys keep their
    /// distance; the front wheel the way of the carriage ahead (or the train's control), the
    /// back wheel the way of the carriage behind. The distance its last wheel moved.
    fn carriage_travel(
        &mut self,
        g: &Graph,
        c: usize,
        distance: f64,
        ahead: Option<usize>,
        behind: Option<usize>,
        control: &mut Control,
    ) -> (f64, bool) {
        let shape = self.carriages[c];
        let mut blocked = false;
        let two = shape.two;
        let stress = if two {
            f64::from(shape.bogey_spacing)
                - distance_between(self.anchor(g, shape.first), self.anchor(g, shape.first + 1))
        } else {
            0.0
        };
        let mut moved = distance;
        let from_back = distance < 0.0;
        for first_bogey in [true, false] {
            if !first_bogey && !two {
                continue;
            }
            let actually_first = !two || (first_bogey ^ from_back);
            let b = if actually_first {
                shape.first
            } else {
                shape.first + 1
            };
            let bogey_correction = stress * if actually_first { 0.5 } else { -0.5 };
            let bogey_stress = WHEEL_SPACING
                - distance_between(self.points[2 * b].at(g), self.points[2 * b + 1].at(g));
            for first_wheel in [true, false] {
                let actually_first_wheel = first_wheel ^ from_back;
                let p = if actually_first_wheel {
                    2 * b
                } else {
                    2 * b + 1
                };
                let prev = if !actually_first_wheel {
                    Some(2 * b)
                } else if !actually_first && two {
                    Some(2 * shape.first + 1)
                } else {
                    None
                };
                let next = if actually_first_wheel {
                    Some(2 * b + 1)
                } else if actually_first && two {
                    Some(2 * (shape.first + 1))
                } else {
                    None
                };
                let correction = bogey_stress * if actually_first_wheel { 0.5 } else { -0.5 };
                let to_move = moved + correction + bogey_correction;
                let lead = if to_move > 0.0 {
                    prev.or(ahead)
                } else {
                    next.or(behind)
                };
                let target = lead.map(|k| {
                    if to_move > 0.0 {
                        self.points[k].edge
                    } else {
                        Graph::reverse(self.points[k].edge)
                    }
                });
                let point = &mut self.points[p];
                moved = point.travel_signed(g, to_move, |from, cands| match target {
                    Some(t) => follow(g, t, cands),
                    None => control.select(g, from, cands),
                });
                blocked |= point.blocked;
            }
        }
        (moved, blocked)
    }

    /// Moves the train `distance` along (`Train.tick`'s carriage loop): each carriage (from the
    /// back when running backwards) moves the distance the first moved, plus the coupling
    /// stress to the carriages beside it (spacing less how far apart their facing anchors
    /// are), halved toward the front and an eighth toward the back (three quarters and none when
    /// `approaching` a station). The distance moved and whether the track ended.
    pub fn travel(
        &mut self,
        g: &Graph,
        distance: f64,
        control: &mut Control,
        approaching: bool,
    ) -> (f64, bool) {
        let n = self.carriages.len();
        if self.points.is_empty() || n == 0 || distance == 0.0 {
            return (0.0, false);
        }
        let stress: Vec<f64> = (1..n)
            .map(|i| {
                let lead = self.anchor(g, self.carriages[i].first);
                let trail = self.anchor(g, self.last_bogey(i - 1));
                f64::from(self.spacing.get(i - 1).copied().unwrap_or(0))
                    - distance_between(lead, trail)
            })
            .collect();
        let (leading_mod, trailing_mod) = if approaching {
            (0.75, 0.0)
        } else {
            (0.5, 0.125)
        };
        let from_back = distance < 0.0;
        let mut distance = distance;
        let mut blocked = false;
        let mut first_moved = 0.0;
        for index in 0..n {
            let i = if from_back { n - 1 - index } else { index };
            let leading_stress = if i == 0 {
                0.0
            } else {
                stress[i - 1] * -(if from_back { trailing_mod } else { leading_mod })
            };
            let trailing_stress = if i == n - 1 {
                0.0
            } else {
                stress[i] * if from_back { leading_mod } else { trailing_mod }
            };
            let ahead = (i > 0).then(|| self.ends(i - 1).1);
            let behind = (i + 1 < n).then(|| self.ends(i + 1).0);
            let (moved, stopped) = self.carriage_travel(
                g,
                i,
                distance + leading_stress + trailing_stress,
                ahead,
                behind,
                control,
            );
            blocked |= stopped;
            if index == 0 {
                distance = moved;
                first_moved = moved;
            }
        }
        (first_moved, blocked)
    }

    /// `Navigation.tick`'s scout: from the leading point along the path, the signals ahead
    /// within the braking distance (`scan`). An entry signal whose section another train holds
    /// (or redstone holds red) stops it; a free one within reach is reserved for it. A cross
    /// signal starts a chain of sections up to the next entry signal: the train waits at the
    /// cross signal while any of them is held, and reserves them all when none is.
    fn scout(&mut self, net: &Network, occ: &mut Occupancy, me: u32, nav: &Nav) -> Scouted {
        let g = &net.graph;
        let lead = self.leading(g, !nav.behind);
        let braking = self.speed * self.speed / (2.0 * ACCELERATION);
        let no_flicker = braking + 3.0 - braking % 3.0;
        let look_ahead = if self.at_station.is_some() { 4.5 } else { 0.0 };
        // `Mth.clamp(value, min, max)`.
        let scan = if no_flicker < look_ahead {
            look_ahead
        } else {
            no_flicker.min(nav.distance)
        };
        let manual = self.manual.is_some_and(|m| m.throttle != 0.0);
        // On a curve already: it counts from here (`edgeTraversedFrom` on the first edge).
        let mut curve = slows(g, lead.edge).then_some(0.0);
        let mut waiting: Option<f64> = None;
        let mut tracking: Option<f64> = None;
        let mut chain: Vec<(usize, usize)> = Vec::new();
        self.reserved.clear();
        let (mut edge, mut base, mut cursor) = (lead.edge, -lead.position, 0);
        let limit = nav.distance + 50.0;
        let mut first = true;
        'walk: loop {
            for &(pos, mark) in &net.marks[edge] {
                if first && pos <= lead.position {
                    continue;
                }
                let d = base + pos;
                match mark {
                    Mark::Signal(b) => {
                        if tracking.is_none() && d > scan {
                            break 'walk;
                        }
                        let side = side_of(edge);
                        let entering = net.beyond(b, side);
                        let cross = net.signals[b].types[side] == SignalType::Cross;
                        let occupied = !manual
                            && (occ.forced_red(b, side) || occ.occupied_unless(entering, me));
                        match tracking {
                            None => {
                                if cross {
                                    tracking = Some(d);
                                    chain.push((entering, b));
                                }
                                if occupied {
                                    waiting = Some(d);
                                    if !cross {
                                        break 'walk;
                                    }
                                }
                                if !occupied
                                    && !cross
                                    && d < waiting.unwrap_or(f64::MAX) + 0.25
                                    && d < no_flicker
                                {
                                    occ.reserve(entering, b);
                                }
                            }
                            Some(start) => {
                                chain.push((entering, b));
                                if occupied {
                                    waiting = Some(start);
                                    if !cross {
                                        break 'walk;
                                    }
                                }
                                if !cross {
                                    if d < waiting.unwrap_or(f64::MAX) + 0.25 {
                                        tracking = None;
                                        self.reserve_chain(occ, &mut chain);
                                    } else {
                                        break 'walk;
                                    }
                                }
                            }
                        }
                    }
                    Mark::Observer(_) => {}
                    Mark::Station(i) => {
                        if net.stations[i].0 == nav.station {
                            break 'walk;
                        }
                    }
                }
            }
            base += g.length(edge);
            if base > limit {
                break;
            }
            let mut next = None;
            let mut count = 0;
            for c in g.onward_iter(edge) {
                count += 1;
                if next.is_none() {
                    next = Some(c);
                }
                if nav.path.get(cursor) == Some(&c) {
                    next = Some(c);
                }
            }
            let Some(n) = next else {
                break;
            };
            if count > 1 && nav.path.get(cursor) == Some(&n) {
                cursor += 1;
            }
            if curve.is_none() && slows(g, n) {
                curve = Some(base.max(0.0));
            }
            edge = n;
            first = false;
        }
        if tracking.is_some() && waiting.is_none() {
            self.reserve_chain(occ, &mut chain);
        }
        Scouted {
            signal: waiting,
            curve,
        }
    }

    /// `reserveChain`: the train holds the chain's sections, each reserved through its signal.
    fn reserve_chain(&mut self, occ: &mut Occupancy, chain: &mut Vec<(usize, usize)>) {
        for (section, b) in chain.drain(..) {
            self.reserved.push(section);
            occ.reserve(section, b);
        }
    }

    /// `frontSignalListener`: a signal the front passed reserves the section beyond for it.
    fn passed(net: &Network, occ: &mut Occupancy, before: Point, after: Point) {
        let mut on = |edge: usize, from: f64, to: f64| {
            for &(pos, mark) in &net.marks[edge] {
                if let Mark::Signal(b) = mark
                    && pos > from
                    && pos <= to
                {
                    occ.reserve(net.beyond(b, side_of(edge)), b);
                }
            }
        };
        if before.edge == after.edge {
            on(after.edge, before.position, after.position);
        } else {
            on(before.edge, before.position, f64::MAX);
            on(after.edge, f64::MIN, after.position);
        }
    }

    /// The controls held (`CarriageContraptionEntity.control`): toward the throttle's way at
    /// 75% of top speed scaled by the throttle and the train's (`cappedTopSpeed`), the turning
    /// speed on a curve, a quarter of it backwards, braking twice as hard against the way it
    /// runs; it leaves the station.
    fn drive(&mut self, g: &Graph, m: Manual) {
        let way = m.throttle.signum();
        let mut top = TOP_SPEED * MANUAL_SPEED;
        let capped = top * m.throttle.abs().min(1.0) * self.throttle;
        let (lead, trail) = (self.points[0], self.points[self.points.len() - 1]);
        if g.edges[lead.edge].turn.is_some() || g.edges[trail.edge].turn.is_some() {
            top = TURN_TOP_SPEED;
        }
        // `slow`: driven against the way the controls face.
        if m.inverted ^ (way < 0.0) {
            top /= 4.0;
        }
        let target = top.min(capped) * way;
        let countering = (way - self.speed.signum()).abs() > 1.5;
        self.at_station = None;
        let rate = if countering { 2.0 } else { 1.0 } * ACCELERATION;
        if (target - self.speed).abs() >= 1.0e-5 {
            self.speed = if self.speed < target {
                (self.speed + rate).min(target)
            } else {
                (self.speed - rate).max(target)
            };
        }
    }

    /// `Train.tick` with `Navigation.tick`: by hand while its controls are held, else the speed
    /// toward the destination, else coasting to a stop; then the move. Arrived when its leading
    /// point passes the station.
    pub fn tick(&mut self, net: &Network, occ: &mut Occupancy, me: u32) -> Run {
        if !self.bound {
            return Run::Still;
        }
        let g = &net.graph;
        let steer = self.manual.map_or(0, |m| m.steer);
        if let Some(m) = self.manual.filter(|m| m.throttle != 0.0) {
            // The controls cancel its navigation.
            self.nav = None;
            self.reserved.clear();
            self.drive(g, m);
            let before = self.leading(g, self.speed >= 0.0);
            let (_, blocked) = self.travel(g, self.speed, &mut Control::Steer(steer), false);
            Self::passed(net, occ, before, self.leading(g, self.speed >= 0.0));
            if blocked {
                self.speed = 0.0;
                return Run::Blocked;
            }
            return if self.speed == 0.0 {
                Run::Still
            } else {
                Run::Moving
            };
        }
        let Some(mut nav) = self.nav.take() else {
            // `tickPassiveSlowdown`.
            self.reserved.clear();
            if self.speed == 0.0 {
                return Run::Still;
            }
            self.speed = if self.speed > 0.0 {
                (self.speed - ACCELERATION).max(0.0)
            } else {
                (self.speed + ACCELERATION).min(0.0)
            };
            let (_, blocked) = self.travel(g, self.speed, &mut Control::Steer(steer), false);
            if blocked {
                self.speed = 0.0;
                return Run::Blocked;
            }
            return Run::Moving;
        };
        let dir = if nav.behind { -1.0 } else { 1.0 };
        let braking = self.speed * self.speed / (2.0 * ACCELERATION);
        let look_ahead = if self.at_station.is_some() { 4.5 } else { 0.0 };
        let seen = self.scout(net, occ, me, &nav);
        nav.signal = seen.signal;
        // Stops half a block short of a signal it waits at.
        let stop = seen.signal.map(|d| d - 0.5);
        if let Some(s) = stop
            && s <= 0.0
        {
            nav.ticks_waiting += 1;
            self.speed = 0.0;
            self.nav = Some(nav);
            return Run::Still;
        }
        if seen.signal.is_none() {
            nav.ticks_waiting = 0;
        }
        // Always overshoots a little, so the point crosses the target.
        let target = stop.unwrap_or(nav.distance) + 0.25;
        if target > 1.0 / 32.0 && self.at_station.is_some() {
            // Doesn't leave until the signal ahead is green.
            if seen.signal.is_some_and(|d| d < look_ahead) {
                nav.ticks_waiting += 1;
                self.nav = Some(nav);
                return Run::Still;
            }
            self.at_station = None;
        }
        if target - self.speed.abs() < 1.0 / 32.0 {
            self.speed = target.max(1.0 / 32.0) * dir;
        } else {
            let max_approach = TOP_SPEED * (target / 10.0);
            if target < 10.0 && self.speed * dir > max_approach {
                self.speed += (max_approach - self.speed.abs()) * 0.5 * dir;
            } else {
                let turn_top = TOP_SPEED.min(TURN_TOP_SPEED);
                let mut target_speed = if target > braking {
                    TOP_SPEED * dir
                } else {
                    0.0
                };
                if let Some(curve) = seen.curve {
                    let slowing = braking - turn_top * turn_top / (2.0 * ACCELERATION);
                    let turn = if curve > slowing { TOP_SPEED } else { turn_top } * dir;
                    if turn.abs() < target_speed.abs() {
                        target_speed = turn;
                    }
                }
                // `approachTargetSpeed`.
                if (target_speed - self.speed).abs() >= 1.0e-5 {
                    self.speed = if self.speed < target_speed {
                        (self.speed + ACCELERATION).min(target_speed)
                    } else {
                        (self.speed - ACCELERATION).max(target_speed)
                    };
                }
            }
        }
        let approaching = nav.distance < 5.0;
        let before = self.leading(g, !nav.behind);
        let (moved, blocked) = self.travel(
            g,
            self.speed,
            &mut Control::Path(&mut nav.path),
            approaching,
        );
        Self::passed(net, occ, before, self.leading(g, !nav.behind));
        nav.distance -= moved * dir;
        if nav.distance <= 0.0 {
            self.speed = 0.0;
            self.at_station = Some(nav.station);
            self.reserved.clear();
            return Run::Arrived;
        }
        if blocked {
            self.speed = 0.0;
            self.reserved.clear();
            return Run::Blocked;
        }
        self.nav = Some(nav);
        Run::Moving
    }

    /// `Navigation.findPathTo`: searched ahead of its front and behind its back (not back
    /// out of a path it is already running, and behind only when it has controls facing
    /// backwards, `doubleEnded`), the cheaper one.
    pub fn find_path(&self, net: &Network, targets: &[usize], cx: &SearchContext) -> Option<Found> {
        let g = &net.graph;
        let mut found: [Option<Found>; 2] = [None, None];
        for (k, forward) in [true, false].into_iter().enumerate() {
            if !forward && !self.double_ended {
                continue;
            }
            if let Some(nav) = &self.nav
                && nav.behind == forward
            {
                continue;
            }
            if !forward && self.nav.is_none() && self.speed > 0.0 {
                continue;
            }
            found[k] = search(net, self.leading(g, forward), targets, cx).map(|mut f| {
                f.behind = !forward;
                f
            });
        }
        let [front, back] = found;
        match (front, back) {
            (None, b) => b,
            (f, None) => f,
            (Some(f), Some(b)) => Some(if b.cost > f.cost { f } else { b }),
        }
    }

    /// `startNavigation`.
    pub fn start(&mut self, net: &Network, found: Found) {
        self.manual = None;
        self.nav = Some(Nav {
            station: net.stations[found.station].0,
            distance: found.distance.abs(),
            path: found.path,
            behind: found.behind,
            signal: None,
            ticks_waiting: 0,
        });
    }
}

fn distance_between(a: Vec3, b: Vec3) -> f64 {
    distance(a, b)
}

fn unit(d: Vec3) -> Vec3 {
    let l = length(d);
    if l < 1.0e-6 {
        [1.0, 0.0, 0.0]
    } else {
        [d[0] / l, d[1] / l, d[2] / l]
    }
}

/// A carriage's blocks: offsets from its leading bogey's block, and states.
pub type CarBlocks = Vec<([i32; 3], u32)>;

/// An assembled train: its motion, each carriage's blocks (offset from its leading bogey's
/// block, state), the way its front faced when assembled (a unit along x or z), its schedule,
/// and the column it is saved with.
#[derive(Clone, Debug, PartialEq)]
pub struct TrainOnTrack {
    pub id: u32,
    pub train: Train,
    pub cars: Vec<CarBlocks>,
    pub heading: [i32; 2],
    pub schedule: Option<Schedule>,
    pub runtime: Runtime,
    /// Ticks since it last arrived (no cargo moves: `IdleCargoCondition` counts them all).
    pub idle: i32,
    pub column: (i32, i32),
    /// For the glue: when clients were last told of it, the leading edge then, and whether
    /// they must get its blocks.
    pub sent_tick: u64,
    pub sent_edge: usize,
    pub fresh: bool,
}

impl TrainOnTrack {
    /// `Train.getNavigationPenalty`.
    pub fn navigation_penalty(&self) -> i32 {
        if self.schedule.is_none() || self.runtime.paused {
            return IDLE_TRAIN;
        }
        if let Some(nav) = &self.train.nav {
            if nav.signal.is_some() && nav.ticks_waiting > 0 {
                return WAITING_TRAIN + (nav.ticks_waiting / 20).min(1000);
            }
            if nav.distance < 50.0 {
                return ARRIVING_TRAIN;
            }
        }
        ANY_TRAIN
    }

    /// Where its first carriage is (its leading bogey's anchor).
    pub fn position(&self, net: &Network) -> Vec3 {
        if self.train.bound {
            self.train.carriage_pose(&net.graph, 0).0
        } else {
            let p = &self.train.placed;
            match (p.first(), p.get(1)) {
                (Some(a), Some(b)) => mid(a.at, b.at),
                (Some(a), None) => a.at,
                _ => [0.0; 3],
            }
        }
    }

    /// The column its first carriage stands in.
    pub fn column_on(&self, net: &Network) -> (i32, i32) {
        let at = self.position(net);
        ((at[0].floor() as i32) >> 4, (at[2].floor() as i32) >> 4)
    }
}

/// What happened to a train in a tick, for the glue: how it ran, whether it set off or its
/// speed changed, and the column it was saved with before and after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub id: u32,
    pub run: Run,
    pub started: bool,
    pub accelerating: bool,
    pub was: (i32, i32),
    pub now: (i32, i32),
}

/// Ticks between motion updates while a train is on a curve (clients draw it gliding along the
/// chord, turning at a steady rate, in between).
pub const CURVE_UPDATE_TICKS: u64 = 2;

/// The mod message channels: trains and signal lights, server to client.
pub const CHANNEL: &str = "create:train";
pub const SIGNAL_CHANNEL: &str = "create:signal";

/// `CarriageContraptionEntity`'s yaw and pitch, degrees: the turn about up from the way the
/// train faced when assembled (`heading`, a unit along x or z) to `front`, counterclockwise
/// looking down (as the parts turn), and how far `front` climbs.
pub fn yaw_pitch(front: Vec3, heading: [i32; 2]) -> (f64, f64) {
    let angle = |x: f64, z: f64| (-z).atan2(x).to_degrees();
    let yaw = angle(front[0], front[2]) - angle(f64::from(heading[0]), f64::from(heading[1]));
    let yaw = (yaw + 540.0).rem_euclid(360.0) - 180.0;
    let flat = (front[0] * front[0] + front[2] * front[2]).sqrt();
    (yaw, front[1].atan2(flat).to_degrees())
}

/// A carriage's or a bogey's motion as clients get it: where it is (a carriage's pivot, its
/// leading bogey's anchor; a bogey's anchor), its velocity (blocks a second) and yaw rate
/// (degrees a second) until the next update, and its yaw and pitch from the train's heading.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Pose {
    pub at: Vec3,
    pub velocity: [f32; 3],
    pub yaw: f32,
    pub yaw_rate: f32,
    pub pitch: f32,
}

/// A train's motion as clients get it: each carriage's pose, each bogey's size and pose, the
/// way it faced when assembled, and each carriage's blocks (only when clients must learn them).
#[derive(Clone, Debug, PartialEq)]
pub struct Motion {
    pub id: u32,
    pub heading: [i32; 2],
    pub cars: Vec<Pose>,
    pub bogeys: Vec<(BogeySize, Pose)>,
    pub blocks: Option<Vec<CarBlocks>>,
}

const HEADINGS: [[i32; 2]; 4] = [[1, 0], [0, 1], [-1, 0], [0, -1]];
/// Blocks a message carries at most (a message stays under 32 KiB).
const MAX_SENT_BLOCKS: usize = 1800;

fn put_pose(out: &mut Vec<u8>, p: &Pose) {
    for v in p.at {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in p.velocity.into_iter().chain([p.yaw, p.yaw_rate, p.pitch]) {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn get_pose(d: &[u8], k: usize) -> Option<Pose> {
    let r = d.get(k..k + 48)?;
    let float = |i: usize| f64::from_le_bytes(r[i..i + 8].try_into().unwrap_or([0; 8]));
    let single = |i: usize| f32::from_le_bytes(r[i..i + 4].try_into().unwrap_or([0; 4]));
    Some(Pose {
        at: [float(0), float(8), float(16)],
        velocity: [single(24), single(28), single(32)],
        yaw: single(36),
        yaw_rate: single(40),
        pitch: single(44),
    })
}

impl Motion {
    /// The message: id, heading, carriage and bogey counts, whether blocks follow; each
    /// carriage's pose; each bogey's size and pose; then each carriage's block count and
    /// blocks. A message of the id alone clears the train.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + 48 * self.cars.len() + 49 * self.bogeys.len());
        out.extend_from_slice(&self.id.to_le_bytes());
        out.push(
            HEADINGS
                .iter()
                .position(|h| *h == self.heading)
                .unwrap_or(0) as u8,
        );
        out.push(self.cars.len().min(255) as u8);
        out.push(self.bogeys.len().min(255) as u8);
        out.push(u8::from(self.blocks.is_some()));
        for c in self.cars.iter().take(255) {
            put_pose(&mut out, c);
        }
        for (size, p) in self.bogeys.iter().take(255) {
            out.push(u8::from(*size == BogeySize::Large));
            put_pose(&mut out, p);
        }
        if let Some(cars) = &self.blocks {
            let mut left = MAX_SENT_BLOCKS;
            for car in cars.iter().take(self.cars.len().min(255)) {
                let n = car.len().min(left);
                left -= n;
                out.extend_from_slice(&(n as u32).to_le_bytes());
                for (o, s) in &car[..n] {
                    for v in o {
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                    out.extend_from_slice(&s.to_le_bytes());
                }
            }
        }
        out
    }

    /// The motion in a message, or the id of a train to clear.
    pub fn decode(d: &[u8]) -> Result<Motion, Option<u32>> {
        let word = |k: usize| -> Option<[u8; 4]> { d.get(k..k + 4)?.try_into().ok() };
        let Some(id) = word(0).map(u32::from_le_bytes) else {
            return Err(None);
        };
        if d.len() < 8 {
            return Err(Some(id));
        }
        let heading = HEADINGS[usize::from(d[4]).min(3)];
        let (n_cars, n_bogeys, with_blocks) = (usize::from(d[5]), usize::from(d[6]), d[7] != 0);
        let mut k = 8;
        let mut cars = Vec::with_capacity(n_cars);
        for _ in 0..n_cars {
            cars.push(get_pose(d, k).ok_or(Some(id))?);
            k += 48;
        }
        let mut bogeys = Vec::with_capacity(n_bogeys);
        for _ in 0..n_bogeys {
            let size = if *d.get(k).ok_or(Some(id))? != 0 {
                BogeySize::Large
            } else {
                BogeySize::Small
            };
            bogeys.push((size, get_pose(d, k + 1).ok_or(Some(id))?));
            k += 49;
        }
        let blocks = if with_blocks {
            let mut all = Vec::with_capacity(n_cars);
            for _ in 0..n_cars {
                let n = word(k).map_or(0, u32::from_le_bytes) as usize;
                k += 4;
                let mut car = Vec::with_capacity(n.min(MAX_SENT_BLOCKS));
                for _ in 0..n {
                    let Some(r) = d.get(k..k + 16) else { break };
                    let i = |j: usize| i32::from_le_bytes([r[j], r[j + 1], r[j + 2], r[j + 3]]);
                    car.push(([i(0), i(4), i(8)], i(12) as u32));
                    k += 16;
                }
                all.push(car);
            }
            Some(all)
        } else {
            None
        };
        Ok(Motion {
            id,
            heading,
            cars,
            bogeys,
            blocks,
        })
    }
}

impl TrainOnTrack {
    /// Its motion now and as it will be `ticks` later at this speed along its path, for
    /// clients to draw in between: each carriage turning about its pivot, each bogey about
    /// its anchor.
    pub fn motion(&self, net: &Network, ticks: u64, full: bool) -> Motion {
        let g = &net.graph;
        let mut ahead = self.train.clone();
        let mut path = ahead
            .nav
            .as_ref()
            .map(|n| n.path.clone())
            .unwrap_or_default();
        let distance = self.train.speed * ticks as f64;
        let steer = self.train.manual.map_or(0, |m| m.steer);
        let mut control = if self.train.nav.is_some() {
            Control::Path(&mut path)
        } else {
            Control::Steer(steer)
        };
        ahead.travel(g, distance, &mut control, false);
        let seconds = ticks as f64 / 20.0;
        let pose = |(now, front): (Vec3, Vec3), (then, front_then): (Vec3, Vec3)| {
            let (yaw, pitch) = yaw_pitch(front, self.heading);
            let (yaw_then, _) = yaw_pitch(front_then, self.heading);
            let turn = (yaw_then - yaw + 540.0).rem_euclid(360.0) - 180.0;
            Pose {
                at: now,
                velocity: std::array::from_fn(|k| ((then[k] - now[k]) / seconds) as f32),
                yaw: yaw as f32,
                yaw_rate: (turn / seconds) as f32,
                pitch: pitch as f32,
            }
        };
        let cars = (0..self.train.carriages.len())
            .map(|c| pose(self.train.carriage_pose(g, c), ahead.carriage_pose(g, c)))
            .collect();
        let bogeys = (0..self.train.bogeys.len())
            .map(|b| {
                (
                    self.train.bogeys[b],
                    pose(self.train.bogey_pose(g, b), ahead.bogey_pose(g, b)),
                )
            })
            .collect();
        Motion {
            id: self.id,
            heading: self.heading,
            cars,
            bogeys,
            blocks: full.then(|| self.cars.clone()),
        }
    }

    /// Whether clients must be told of it after a tick (`event`): when it sets off, arrives or
    /// stops, every few ticks on a curve, when its leading point passes onto another edge, and
    /// every ten ticks while its speed changes.
    pub fn due(&self, net: &Network, tick: u64, event: &Event) -> bool {
        let g = &net.graph;
        let lead = self.train.points.first().map_or(0, |p| p.edge);
        let on_turn = self
            .train
            .points
            .iter()
            .any(|p| g.edges.get(p.edge).is_some_and(|e| e.turn.is_some()));
        event.started
            || matches!(event.run, Run::Arrived | Run::Blocked)
            || self.train.speed == 0.0
            || (on_turn && tick >= self.sent_tick + CURVE_UPDATE_TICKS)
            || lead != self.sent_edge
            || (event.accelerating && tick.is_multiple_of(10))
    }
}

/// A signal side's state (`SignalBlockEntity.SignalState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SignalState {
    Red,
    Yellow,
    Green,
    #[default]
    Invalid,
}

impl SignalState {
    /// `isRedLight` (an invalid signal's short blink is left out: it is drawn dark).
    pub fn red_light(self) -> bool {
        self == SignalState::Red
    }
}

/// A signal block as the world has it: its type and whether redstone powers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SignalBlock {
    pub kind: SignalType,
    pub powered: bool,
}

/// `TrackObserver.keepAlive`'s count: decremented before and after the trains move each tick
/// (`tickPoints` both times), so an observer stays lit 3 ticks after the last one leaves.
pub const OBSERVER_KEEP_ALIVE: i32 = 8;

/// A track observer (`TrackObserver` with its block): whether its filter holds anything (trains
/// carry no cargo in this port, so a filtered one sees none: `tickOccupiedObservers` finds no
/// item or fluid passing), its `activated` count, the train it last saw (`currentTrain`), and
/// whether its block is lit (`powered`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObserverBlock {
    pub filtered: bool,
    pub activated: i32,
    pub train: Option<u32>,
    pub lit: bool,
}

impl ObserverBlock {
    /// `TrackObserver.tick`.
    fn tick(&mut self) {
        if self.activated > 0 {
            self.activated -= 1;
        }
        if self.activated <= 0 {
            self.train = None;
        }
    }
}

/// A dimension's railway: its track, stations (with their names) and signals as the world has
/// them, the graph built from them (again only when they or the curves change), its trains and
/// its signals' states (worked out after the trains move, and when signals change).
#[derive(Debug, Default)]
pub struct Railway {
    pub tracks: HashMap<[i32; 3], TrackShape>,
    pub stations: HashMap<[i32; 3], String>,
    pub signals: HashMap<[i32; 3], SignalBlock>,
    /// Names saved with a column, waiting for their station block.
    pub saved_names: HashMap<[i32; 3], String>,
    pub trains: Vec<TrainOnTrack>,
    pub net: Network,
    pub occupancy: Occupancy,
    /// Each boundary side's state (`cachedStates`), and each signal block's light as clients
    /// were last told (red or not); `lights_changed` holds the blocks to tell them of (None: the
    /// signal is gone).
    pub states: Vec<[SignalState; 2]>,
    pub lights: HashMap<[i32; 3], bool>,
    pub lights_changed: Vec<([i32; 3], Option<bool>)>,
    /// Track observers as the world has them, the observers each train stands over as of its
    /// last move (`occupiedObservers`), and the observers whose `powered` must change.
    pub observers: HashMap<[i32; 3], ObserverBlock>,
    occupied: HashMap<u32, Vec<[i32; 3]>>,
    pub observer_changes: Vec<([i32; 3], bool)>,
    observers_active: bool,
    dirty: bool,
    signals_dirty: bool,
    curves_seen: u64,
}

impl Railway {
    pub fn set_track(&mut self, pos: [i32; 3], shape: TrackShape) {
        if self.tracks.insert(pos, shape) != Some(shape) {
            self.dirty = true;
        }
    }

    pub fn add_station(&mut self, pos: [i32; 3]) {
        if !self.stations.contains_key(&pos) {
            let name = self
                .saved_names
                .remove(&pos)
                .unwrap_or_else(|| DEFAULT_STATION_NAME.to_owned());
            self.stations.insert(pos, name);
            self.dirty = true;
        }
    }

    /// A signal block placed or changed: a new one or a new type builds the graph again; power
    /// only turns its side red or back (`updateBlockEntityPower`).
    pub fn add_signal(&mut self, pos: [i32; 3], kind: SignalType, powered: bool) {
        let block = SignalBlock { kind, powered };
        match self.signals.insert(pos, block) {
            Some(old) if old == block => {}
            Some(old) if old.kind == kind => {
                self.refresh_forced();
                self.signals_dirty = true;
            }
            _ => self.dirty = true,
        }
    }

    /// A track observer placed (or loaded) beside a track: on the graph when it is next built.
    pub fn add_observer(&mut self, pos: [i32; 3], lit: bool) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.observers.entry(pos) {
            e.insert(ObserverBlock {
                lit,
                ..Default::default()
            });
            self.dirty = true;
            // A block loaded lit with no train over it goes dark (`tick`).
            self.observers_active |= lit;
        }
    }

    /// `setFilterAndNotify`: whether the observer's filter holds anything.
    pub fn set_observer_filtered(&mut self, pos: [i32; 3], filtered: bool) {
        if let Some(o) = self.observers.get_mut(&pos) {
            o.filtered = filtered;
        }
    }

    /// The observers each train stands over now (`collectInitiallyOccupiedSignalBlocks` for
    /// them, and the listeners as it runs), for the trains `which` gives.
    fn occupy_observers(&mut self, which: impl Fn(&TrainOnTrack) -> bool) {
        if self.net.observers.is_empty() {
            self.occupied.clear();
            return;
        }
        for t in &self.trains {
            if !t.train.bound || !which(t) {
                continue;
            }
            let over: Vec<[i32; 3]> = self
                .net
                .observers
                .iter()
                .filter(|(_, at)| self.net.covers(&t.train.points, *at))
                .map(|(p, _)| *p)
                .collect();
            self.occupied.insert(t.id, over);
        }
    }

    /// `tickPoints` for the observers: each counts down; a block whose light changes is told.
    fn tick_observers(&mut self) -> bool {
        let mut active = false;
        for o in self.observers.values_mut() {
            o.tick();
            active |= o.activated > 0;
        }
        active
    }

    /// Which boundary sides redstone holds red, from their blocks' power.
    fn refresh_forced(&mut self) {
        let signals = &self.signals;
        self.occupancy.forced = self
            .net
            .signals
            .iter()
            .map(|b| {
                std::array::from_fn(|side| {
                    b.blocks[side]
                        .iter()
                        .any(|p| signals.get(p).is_some_and(|s| s.powered))
                })
            })
            .collect();
    }

    /// A track, station or signal at `pos` is gone.
    pub fn remove(&mut self, pos: [i32; 3]) {
        let signal = self.signals.remove(&pos).is_some();
        if signal && self.lights.remove(&pos).is_some() {
            self.lights_changed.push((pos, None));
        }
        let observer = self.observers.remove(&pos).is_some();
        if self.tracks.remove(&pos).is_some()
            || self.stations.remove(&pos).is_some()
            || signal
            || observer
        {
            self.dirty = true;
        }
    }

    /// A column left: its track, stations and signals leave the graph, its trains memory
    /// (saved with it).
    pub fn column_unloaded(&mut self, cx: i32, cz: i32) {
        let out = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) != (cx, cz);
        let (t, s, g) = (self.tracks.len(), self.stations.len(), self.signals.len());
        let o = self.observers.len();
        self.tracks.retain(|p, _| out(p));
        self.stations.retain(|p, _| out(p));
        self.signals.retain(|p, _| out(p));
        self.observers.retain(|p, _| out(p));
        if o != self.observers.len() {
            self.dirty = true;
        }
        let gone = &mut self.lights_changed;
        self.lights.retain(|p, _| {
            let keep = out(p);
            if !keep {
                gone.push((*p, None));
            }
            keep
        });
        self.saved_names.retain(|p, _| out(p));
        self.trains.retain(|t| t.column != (cx, cz));
        if (t, s, g) != (self.tracks.len(), self.stations.len(), self.signals.len()) {
            self.dirty = true;
        }
    }

    /// The station names in a column, to save with it.
    pub fn column_names(&self, cx: i32, cz: i32) -> Vec<([i32; 3], String)> {
        let mut out: Vec<([i32; 3], String)> = self
            .stations
            .iter()
            .filter(|(p, n)| (p[0] >> 4, p[2] >> 4) == (cx, cz) && *n != DEFAULT_STATION_NAME)
            .map(|(p, n)| (*p, n.clone()))
            .collect();
        out.sort();
        out
    }

    /// Station names saved with a column that joined, before its blocks are found.
    pub fn load_names(&mut self, saved: impl Iterator<Item = ([i32; 3], String)>) {
        for (p, n) in saved {
            match self.stations.get_mut(&p) {
                Some(name) => *name = n,
                None => {
                    self.saved_names.insert(p, n);
                }
            }
        }
    }

    /// Trains saved with a column that joined: each gets a fresh id, and is found on the
    /// graph when it is next built.
    pub fn load_trains(&mut self, saved: Vec<TrainOnTrack>) -> Vec<u32> {
        let mut ids = Vec::new();
        for mut t in saved {
            t.id = self.next_id();
            t.train.bound = false;
            t.fresh = true;
            ids.push(t.id);
            self.trains.push(t);
        }
        self.dirty = true;
        ids
    }

    pub fn next_id(&self) -> u32 {
        self.trains.iter().map(|t| t.id).max().unwrap_or(0) + 1
    }

    /// Builds the graph again if track, stations, signals or `curves` changed (`revision`):
    /// trains are found on it again, and those navigating look for their path again.
    pub fn ensure(&mut self, curves: &crate::track_curves::Curves) -> bool {
        if !self.dirty && curves.revision == self.curves_seen {
            return false;
        }
        for t in &mut self.trains {
            t.train.remember(&self.net);
        }
        let mut stations: Vec<[i32; 3]> = self.stations.keys().copied().collect();
        stations.sort_unstable();
        let signals: Vec<([i32; 3], SignalType)> =
            self.signals.iter().map(|(p, s)| (*p, s.kind)).collect();
        self.net = Network::build(&self.tracks, curves, &stations, &signals);
        let mut observers: Vec<[i32; 3]> = self.observers.keys().copied().collect();
        observers.sort_unstable();
        self.net.bind_observers(&self.tracks, &observers);
        self.dirty = false;
        self.signals_dirty = true;
        self.curves_seen = curves.revision;
        self.states = vec![[SignalState::Invalid; 2]; self.net.signals.len()];
        self.refresh_forced();
        self.occupancy.fill(&self.net, &[]);
        for i in 0..self.trains.len() {
            let was = self.trains[i].train.bound;
            let bound = self.trains[i].train.bind(&self.net);
            if bound && !was {
                self.trains[i].fresh = true;
            }
            if !bound {
                continue;
            }
            if let Some(station) = self.trains[i].train.nav.as_ref().map(|n| n.station) {
                let target: Vec<usize> = self.station_index(station).into_iter().collect();
                if !self.navigate(i, &target) {
                    self.trains[i].train.nav = None;
                    self.trains[i].runtime.interrupted();
                }
            }
        }
        // `updateSignalBlocks`: what each train stands over on the new graph.
        self.occupy_observers(|_| true);
        true
    }

    /// The index of a station on the graph.
    pub fn station_index(&self, pos: [i32; 3]) -> Option<usize> {
        self.net.stations.iter().position(|(p, _)| *p == pos)
    }

    /// The stations whose names match a schedule's filter.
    pub fn matching(&self, filter: &str) -> Vec<usize> {
        self.net
            .stations
            .iter()
            .enumerate()
            .filter(|(_, (p, _))| {
                self.stations
                    .get(p)
                    .is_some_and(|name| station_matches(filter, name))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Train `i` finds its way to the nearest of `targets` and sets off (`findPathTo`,
    /// `startNavigation`); false when there is none.
    pub fn navigate(&mut self, i: usize, targets: &[usize]) -> bool {
        if targets.is_empty() || !self.trains[i].train.bound {
            return false;
        }
        let me = self.trains[i].id;
        // Other trains weigh on the edges their ends stand on.
        let mut penalties: HashMap<usize, i32> = HashMap::new();
        let mut present: HashMap<[i32; 3], u32> = HashMap::new();
        for t in &self.trains {
            if let Some(s) = t.train.at_station {
                present.insert(s, t.id);
            }
            if t.id == me || !t.train.bound {
                continue;
            }
            let p = t.navigation_penalty();
            if let (Some(a), Some(b)) = (t.train.points.first(), t.train.points.last()) {
                for e in [a.edge, b.edge] {
                    for e in [e, Graph::reverse(e)] {
                        *penalties.entry(e).or_default() += p / 2;
                    }
                }
            }
        }
        let cx = SearchContext {
            me,
            penalties: &penalties,
            present: &present,
            occupancy: &self.occupancy,
            ticks_waiting: self.trains[i]
                .train
                .nav
                .as_ref()
                .map_or(0, |n| n.ticks_waiting),
        };
        match self.trains[i].train.find_path(&self.net, targets, &cx) {
            Some(found) => {
                self.trains[i].train.start(&self.net, found);
                true
            }
            None => false,
        }
    }

    /// `SignalBoundary.tickState`: each side with signal blocks is red while redstone holds it
    /// or the section beyond is held (by a train, or reserved through another signal), invalid
    /// when both sides lead into one section; else an entry signal is green and a cross signal
    /// takes its chain's states (`resolveSignalChain`: all free green, all red red, else
    /// yellow). The blocks whose light changed are kept for clients.
    fn tick_signal_states(&mut self) {
        let net = &self.net;
        for b in 0..net.signals.len() {
            for side in 0..2 {
                let boundary = &net.signals[b];
                if boundary.blocks[side].is_empty() {
                    continue;
                }
                let group = net.beyond(b, side);
                let state = if group == net.beyond(b, 1 - side) {
                    SignalState::Invalid
                } else if self.occupancy.forced_red(b, side)
                    || self.occupancy.occupied_unless_boundary(group, b)
                {
                    SignalState::Red
                } else if boundary.types[side] != SignalType::Cross {
                    SignalState::Green
                } else {
                    let (mut all_free, mut none_free) = (true, true);
                    for &(o, k) in &boundary.chain[side] {
                        if net.signals[o].blocks[k].is_empty() {
                            continue;
                        }
                        let s = self.states[o][k];
                        all_free &= matches!(s, SignalState::Green | SignalState::Invalid);
                        none_free &= s == SignalState::Red;
                    }
                    if all_free {
                        SignalState::Green
                    } else if none_free {
                        SignalState::Red
                    } else {
                        SignalState::Yellow
                    }
                };
                self.states[b][side] = state;
                for &p in &boundary.blocks[side] {
                    let red = state.red_light();
                    if self.lights.insert(p, red) != Some(red) {
                        self.lights_changed.push((p, Some(red)));
                    }
                }
            }
        }
    }

    /// Whether a tick has nothing to do: no trains, no signal changed, and nothing for clients;
    /// with signals, track or curves changed builds the graph (once) for their states.
    pub fn idle(&self, curves: &crate::track_curves::Curves) -> bool {
        self.trains.is_empty()
            && !self.signals_dirty
            && !self.observers_active
            && self.lights_changed.is_empty()
            && self.observer_changes.is_empty()
            && (self.signals.is_empty() || (!self.dirty && curves.revision == self.curves_seen))
    }

    /// The controls held on train `id` (`/create_debug drive`, standing in for a player riding
    /// it): None when there is no such train.
    pub fn drive(&mut self, id: u32, manual: Manual) -> Option<()> {
        let t = self.trains.iter_mut().find(|t| t.id == id)?;
        t.train.manual = Some(manual);
        if manual.throttle != 0.0 && t.train.nav.is_some() {
            t.train.nav = None;
            t.runtime.interrupted();
        }
        Some(())
    }

    /// `CarriageContraptionEntity.startControlling`: a player takes train `id`'s controls; its
    /// schedule pauses (`manualControls`) and its navigation is cancelled. False when there is
    /// no such train on the graph (Create's `carriage == null`; trains don't derail here).
    pub fn start_controlling(&mut self, id: u32) -> bool {
        let Some(t) = self.trains.iter_mut().find(|t| t.id == id && t.train.bound) else {
            return false;
        };
        t.train.nav = None;
        t.train.reserved.clear();
        t.runtime.paused = true;
        true
    }

    /// The controls of train `id` let go (`ControlsServerHandler` dropped its keys): it runs
    /// on by itself again, coasting or navigating.
    pub fn release(&mut self, id: u32) {
        if let Some(t) = self.trains.iter_mut().find(|t| t.id == id) {
            t.train.manual = None;
        }
    }

    /// `CarriageContraptionEntity.control` for train `id`, after the player's checks: the keys
    /// held (`train_controls` bits) on controls facing its back or not. Space at a station
    /// tells its name; driving off it, the station departed; without a station, space follows
    /// the navigation to the station ahead (`findNearestApproachable`, started by space and
    /// kept only while space is held) with its progress, else the station ahead is offered;
    /// then the throttle and steering by hand. None when there is no such train (the controls
    /// are let go).
    pub fn control(
        &mut self,
        id: u32,
        held: u8,
        inverted: bool,
        driving: &mut crate::train_controls::Driving,
    ) -> Option<crate::train_controls::Prompt> {
        use crate::train_controls::{BACK, FORWARD, JUMP, LEFT, Prompt, RIGHT};
        let i = self
            .trains
            .iter()
            .position(|t| t.id == id && t.train.bound)?;
        let has = |k: u8| held & (1 << k) != 0;
        let mut speed = i32::from(has(FORWARD)) - i32::from(has(BACK));
        let mut steer = i8::from(has(LEFT)) - i8::from(has(RIGHT));
        if inverted {
            speed = -speed;
            steer = -steer;
        }
        let space = has(JUMP);
        let name = |r: &Railway, p: [i32; 3]| {
            r.stations
                .get(&p)
                .cloned()
                .unwrap_or_else(|| DEFAULT_STATION_NAME.to_owned())
        };
        let station = self.trains[i].train.at_station;
        if let Some(p) = station
            && space
        {
            return Some(Prompt::Arrived(name(self, p)));
        }
        let mut prompt = Prompt::None;
        if let Some(p) = station
            && speed != 0
        {
            driving.station_message = false;
            prompt = Prompt::Departing(name(self, p));
        }
        if station.is_none() {
            if let Some(nav) = &self.trains[i].train.nav {
                if space {
                    let f = nav.distance / driving.nav_total.max(1.0e-9);
                    let progress = ((1.0 - (1.0 - f) * (1.0 - f)).clamp(0.0, 1.0) * 30.0) as u8;
                    self.trains[i].train.manual = None;
                    return Some(Prompt::Progress(progress));
                }
                self.trains[i].train.nav = None;
            }
            let t = &self.trains[i].train;
            let directed = if speed != 0 {
                f64::from(speed)
            } else {
                t.speed
            };
            let forward = !t.double_ended
                || if directed != 0.0 {
                    directed > 0.0
                } else {
                    !inverted
                };
            match self.nearest_approachable(i, forward) {
                Some(s) if space => {
                    self.trains[i].train.manual = None;
                    if self.navigate(i, &[s]) {
                        driving.nav_total = self.trains[i]
                            .train
                            .nav
                            .as_ref()
                            .map_or(0.0, |n| n.distance);
                    }
                    return Some(Prompt::None);
                }
                Some(s) => {
                    driving.station_message = true;
                    prompt = Prompt::Approach(name(self, self.net.stations[s].0));
                }
                None if driving.station_message => {
                    driving.station_message = false;
                    prompt = Prompt::Clear;
                }
                None => {}
            }
        }
        let t = &mut self.trains[i];
        t.train.manual = Some(Manual {
            throttle: f64::from(speed),
            steer,
            inverted,
        });
        if speed != 0 {
            t.train.nav = None;
            t.train.reserved.clear();
        }
        Some(prompt)
    }

    /// `Navigation.findNearestApproachable`: the nearest station ahead of train `i` (from its
    /// front, or its back when not `forward`) further than three quarters of its braking
    /// distance and within 32 blocks or one and a half braking distances, no other train
    /// standing at it; never past a signal from a side without blocks (`canNavigateVia`).
    pub fn nearest_approachable(&self, i: usize, forward: bool) -> Option<usize> {
        let t = &self.trains[i];
        let g = &self.net.graph;
        let braking = t.train.speed * t.train.speed / (2.0 * ACCELERATION);
        let (min, max) = (0.75 * braking, (1.5 * braking).max(32.0));
        let start = t.train.leading(g, forward);
        let other_at = |s: usize| {
            let p = self.net.stations[s].0;
            self.trains
                .iter()
                .any(|o| o.id != t.id && o.train.at_station == Some(p))
        };
        let initial = start.edge;
        let to_end = g.length(initial) - start.position;
        for &(pos, mark) in &self.net.marks[initial] {
            if pos < start.position {
                continue;
            }
            match mark {
                Mark::Signal(b) if self.net.signals[b].blocks[side_of(initial)].is_empty() => {
                    return None;
                }
                Mark::Station(s)
                    if to_end >= min && pos - start.position >= min && !other_at(s) =>
                {
                    return Some(s);
                }
                _ => {}
            }
        }
        let mut frontier = BinaryHeap::new();
        frontier.push(Frontier {
            key: to_end,
            distance: to_end,
            penalty: 0,
            has_destination: false,
            edge: initial,
        });
        let mut visited: HashSet<usize> = HashSet::new();
        while let Some(entry) = frontier.pop() {
            if !visited.insert(entry.edge) || entry.distance > max {
                continue;
            }
            let e = entry.edge;
            if entry.has_destination {
                let len = g.length(e);
                for &(pos, mark) in &self.net.marks[e] {
                    let Mark::Station(s) = mark else { continue };
                    if entry.distance >= min && entry.distance - (len - pos) >= min && !other_at(s)
                    {
                        return Some(s);
                    }
                }
            }
            'next: for n in g.onward_iter(e) {
                let mut has_destination = false;
                for &(_, mark) in &self.net.marks[n] {
                    match mark {
                        Mark::Signal(b) if self.net.signals[b].blocks[side_of(n)].is_empty() => {
                            continue 'next;
                        }
                        Mark::Station(_) => has_destination = true,
                        _ => {}
                    }
                }
                let distance = entry.distance + g.length(n);
                frontier.push(Frontier {
                    key: distance,
                    distance,
                    penalty: 0,
                    has_destination,
                    edge: n,
                });
            }
        }
        None
    }

    /// A tick of every train: schedules give orders, navigating trains drive, those whose
    /// controls are held drive by hand, the others coast to a stop; then the signals' states.
    /// Nothing is done without trains unless a signal changed.
    pub fn tick(&mut self, events: &mut Vec<Event>) {
        events.clear();
        if self.trains.is_empty() && !self.signals_dirty && !self.observers_active {
            return;
        }
        self.signals_dirty = false;
        // Observers: `tickPoints(true)`, then each train keeps those it stands over alive
        // (`tickOccupiedObservers` in `earlyTick`, before it moves).
        if !self.observers.is_empty() {
            self.tick_observers();
            let trains = &self.trains;
            self.occupied
                .retain(|id, _| trains.iter().any(|t| t.id == *id));
            for (id, over) in &self.occupied {
                for p in over {
                    if let Some(o) = self.observers.get_mut(p)
                        && !o.filtered
                    {
                        o.activated = OBSERVER_KEEP_ALIVE;
                        o.train = Some(*id);
                    }
                }
            }
        }
        self.occupancy.fill(&self.net, &self.trains);
        for i in 0..self.trains.len() {
            if !self.trains[i].train.bound {
                continue;
            }
            self.trains[i].idle = self.trains[i].idle.saturating_add(1);
            let Some(schedule) = self.trains[i].schedule.take() else {
                continue;
            };
            let t = &mut self.trains[i];
            let order = t.runtime.tick(&schedule, t.train.nav.is_some(), t.idle);
            if let Order::Navigate(filter) = order {
                let targets = self.matching(&filter);
                let here = self.trains[i].train.at_station;
                if here.is_some_and(|h| {
                    self.stations
                        .get(&h)
                        .is_some_and(|n| station_matches(&filter, n))
                }) {
                    self.trains[i].runtime.already_there(&schedule);
                    self.trains[i].idle = 0;
                } else if self.navigate(i, &targets) {
                    self.trains[i].runtime.started();
                } else {
                    self.trains[i].runtime.no_path();
                }
            }
            self.trains[i].schedule = Some(schedule);
        }
        for i in 0..self.trains.len() {
            let t = &mut self.trains[i];
            let was = t.train.speed;
            let run = t.train.tick(&self.net, &mut self.occupancy, t.id);
            match run {
                Run::Arrived => {
                    t.idle = 0;
                    if let Some(s) = &t.schedule {
                        t.runtime.arrived(s);
                    }
                }
                Run::Blocked => t.runtime.interrupted(),
                _ => {}
            }
            if run != Run::Still || was != 0.0 {
                let before = t.column;
                t.column = t.column_on(&self.net);
                events.push(Event {
                    id: t.id,
                    run,
                    started: was == 0.0 && t.train.speed != 0.0,
                    accelerating: was != t.train.speed,
                    was: before,
                    now: t.column,
                });
            }
        }
        if !self.observers.is_empty() {
            // The listeners as each moving train ran, then `tickPoints(false)`; the blocks
            // whose light changed are told (`TrackObserverBlockEntity.tick`).
            let moved: Vec<u32> = events.iter().map(|e| e.id).collect();
            self.occupy_observers(|t| moved.contains(&t.id));
            self.observers_active = self.tick_observers();
            for (p, o) in self.observers.iter_mut() {
                let lit = o.activated > 0;
                if lit != o.lit {
                    o.lit = lit;
                    self.observer_changes.push((*p, lit));
                }
            }
        } else {
            self.observers_active = false;
        }
        self.tick_signal_states();
    }
}

/// A saved train's motion (`Train.write`): its speed, whether its controls face both ways, its
/// heading, the station it stands at and the station it is bound for, with whether it reached
/// it backwards (`behind`), and its throttle (`Throttle`). Saved as 14 ints: speed (f32 bits),
/// double-ended, heading (2), at a station (flag, then 3), bound for one (flag, then 3), behind,
/// throttle (f32 bits; a save of 13 without it reads as full throttle).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedMotion {
    pub speed: f64,
    pub double_ended: bool,
    pub heading: [i32; 2],
    pub at_station: Option<[i32; 3]>,
    pub nav: Option<[i32; 3]>,
    pub behind: bool,
    pub throttle: f64,
}

impl SavedMotion {
    pub fn encode(&self) -> Vec<i32> {
        let pos = |p: Option<[i32; 3]>| p.unwrap_or_default();
        let mut v = vec![
            (self.speed as f32).to_bits() as i32,
            i32::from(self.double_ended),
            self.heading[0],
            self.heading[1],
            i32::from(self.at_station.is_some()),
        ];
        v.extend(pos(self.at_station));
        v.push(i32::from(self.nav.is_some()));
        v.extend(pos(self.nav));
        v.push(i32::from(self.behind));
        v.push((self.throttle as f32).to_bits() as i32);
        v
    }

    /// None for fewer than 13 ints (`encode` writes 14; the 14th came with the throttle).
    pub fn decode(m: &[i32]) -> Option<Self> {
        if m.len() < 13 {
            return None;
        }
        let at = |k: usize| [m[k], m[k + 1], m[k + 2]];
        Some(Self {
            speed: f64::from(f32::from_bits(m[0] as u32)),
            double_ended: m[1] != 0,
            heading: [m[2], m[3]],
            at_station: (m[4] != 0).then(|| at(5)),
            nav: (m[8] != 0).then(|| at(9)),
            behind: m[12] != 0,
            throttle: m
                .get(13)
                .map_or(1.0, |&t| f64::from(f32::from_bits(t as u32))),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::network::SignalType;
    use crate::schedule::{Condition, Entry, SECONDS};
    use crate::track_curves::{Curves, Saved};

    #[test]
    fn a_saved_trains_motion_reads_back_as_written() {
        for m in [
            SavedMotion {
                speed: 0.0,
                double_ended: false,
                heading: [1, 0],
                at_station: Some([3, 64, -2]),
                nav: None,
                behind: false,
                throttle: 1.0,
            },
            SavedMotion {
                speed: 1.25,
                double_ended: true,
                heading: [0, -1],
                at_station: None,
                nav: Some([-40, 70, 12]),
                behind: true,
                throttle: 0.5,
            },
        ] {
            let ints = m.encode();
            assert_eq!(ints.len(), 14);
            assert_eq!(SavedMotion::decode(&ints), Some(m));
            // Saved before the throttle: full throttle.
            let old = SavedMotion::decode(&ints[..13]).unwrap();
            assert_eq!(old.throttle, 1.0);
        }
        assert_eq!(SavedMotion::decode(&[0; 12]), None);
    }

    const UP: Vec3 = [0.0, 1.0, 0.0];

    fn line(
        tracks: &mut HashMap<[i32; 3], TrackShape>,
        from: [i32; 3],
        step: [i32; 3],
        n: i32,
        shape: TrackShape,
    ) {
        for i in 0..n {
            tracks.insert(
                [
                    from[0] + step[0] * i,
                    from[1] + step[1] * i,
                    from[2] + step[2] * i,
                ],
                shape,
            );
        }
    }

    /// An L: x from -40 to 2 at z = 0, a quarter turn to the south, x = 10 from z = 8 to 50.
    fn l_shape() -> Railway {
        let mut r = Railway::default();
        let mut t = HashMap::new();
        line(&mut t, [-40, 64, 0], [1, 0, 0], 43, TrackShape::Xo);
        line(&mut t, [10, 64, 8], [0, 0, 1], 43, TrackShape::Zo);
        for (p, s) in t {
            r.set_track(p, s);
        }
        r
    }

    fn l_curves() -> Curves {
        let mut c = Curves::default();
        c.add(Saved {
            from: [2, 64, 0],
            to: [10, 64, 8],
            starts: [[3.0, 64.0, 0.5], [10.5, 64.0, 8.0]],
            axes: [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
            normals: [UP, UP],
        });
        c
    }

    fn assemble(r: &mut Railway, under: &[[i32; 3]], front: Vec3) -> usize {
        let train = Train::on_bogeys(&r.net, under, front).expect("on the track");
        let id = r.next_id();
        r.trains.push(TrainOnTrack {
            id,
            train,
            cars: vec![Vec::new(); under.len()],
            heading: [1, 0],
            schedule: None,
            runtime: Runtime::new(),
            idle: 0,
            column: (0, 0),
            sent_tick: 0,
            sent_edge: 0,
            fresh: true,
        });
        r.trains.len() - 1
    }

    #[test]
    fn a_train_runs_the_curve_to_a_station_and_stops_there() {
        let mut r = l_shape();
        // Stations beside x = -30 on the x straight and z = 40 on the z straight.
        r.add_station([-30, 64, 1]);
        r.add_station([11, 64, 40]);
        let curves = l_curves();
        assert!(r.ensure(&curves));
        assert_eq!(r.net.stations.len(), 2);
        let i = assemble(&mut r, &[[-30, 64, 0]], [1.0, 0.0, 0.0]);
        let south = r.station_index([11, 64, 40]).unwrap();
        assert!(r.navigate(i, &[south]));
        let nav = r.trains[i].train.nav.clone().unwrap();
        assert!(!nav.behind);
        // Its front wheel is a block east of x = -29.5; the station's track centre is at
        // z = 40.5: 31.5 + 11.78 + 32.5 along the track.
        assert!((nav.distance - 75.75).abs() < 0.1, "{}", nav.distance);
        let mut events = Vec::new();
        let mut ticks = 0;
        let mut top = 0.0f64;
        let mut in_turn = 0.0f64;
        loop {
            r.tick(&mut events);
            ticks += 1;
            let t = &r.trains[i].train;
            top = top.max(t.speed);
            if r.net.graph.edges[t.points[0].edge].turn.is_some() {
                in_turn = in_turn.max(t.speed);
            }
            if events.iter().any(|e| e.run == Run::Arrived) {
                break;
            }
            assert!(ticks < 2000, "never arrived");
        }
        let t = &r.trains[i].train;
        assert_eq!(t.at_station, Some([11, 64, 40]));
        assert_eq!(t.speed, 0.0);
        let front = t.points[0].at(&r.net.graph);
        assert!(
            (front[0] - 10.5).abs() < 1e-6 && (front[2] - 40.5).abs() < 0.5,
            "stops within half a block: {front:?}"
        );
        // Facing south now: the pose turned a quarter.
        let (_, heading) = t.carriage_pose(&r.net.graph, 0);
        assert!(heading[2] > 0.99, "{heading:?}");
        assert!(top <= TOP_SPEED + 1e-9);
        assert!(
            in_turn <= TURN_TOP_SPEED + ACCELERATION + 1e-9,
            "slowed for the turn: {in_turn}"
        );
        // And back the other way: it backs out (its front still faces south).
        let west = r.station_index([-30, 64, 1]).unwrap();
        assert!(r.navigate(i, &[west]));
        assert!(r.trains[i].train.nav.as_ref().unwrap().behind);
        loop {
            r.tick(&mut events);
            if events.iter().any(|e| e.run == Run::Arrived) {
                break;
            }
            ticks += 1;
            assert!(ticks < 4000, "never came back");
        }
        let back = r.trains[i].train.points[1].at(&r.net.graph);
        assert!((back[0] + 29.5).abs() < 0.5, "{back:?}");
    }

    #[test]
    fn a_long_straight_run_reaches_top_speed() {
        let mut r = Railway::default();
        let mut t = HashMap::new();
        line(&mut t, [0, 64, 0], [1, 0, 0], 500, TrackShape::Xo);
        for (p, s) in t {
            r.set_track(p, s);
        }
        r.add_station([410, 64, 1]);
        r.ensure(&Curves::default());
        let i = assemble(&mut r, &[[9, 64, 0]], [1.0, 0.0, 0.0]);
        assert!(r.navigate(i, &[0]));
        let (mut events, mut ticks, mut top) = (Vec::new(), 0, 0.0f64);
        loop {
            r.tick(&mut events);
            ticks += 1;
            top = top.max(r.trains[i].train.speed);
            if events.iter().any(|e| e.run == Run::Arrived) {
                break;
            }
            assert!(ticks < 2000, "never arrived");
        }
        assert!((top - TOP_SPEED).abs() < 1e-9, "reaches 28 m/s: {top}");
        // 187 ticks (131 blocks) to reach top speed, as long to brake, the rest at 1.4 a tick.
        assert!((440..520).contains(&ticks), "{ticks}");
        let front = r.trains[i].train.points[0].at(&r.net.graph);
        assert!((front[0] - 410.5).abs() < 0.5, "{front:?}");
    }

    #[test]
    fn pathfinding_takes_the_branch_that_leads_to_the_station() {
        let mut r = l_shape();
        // The x straight goes on east past the turn to x = 40, with a station by x = 30.
        let mut t = HashMap::new();
        line(&mut t, [3, 64, 0], [1, 0, 0], 38, TrackShape::Xo);
        for (p, s) in t {
            r.set_track(p, s);
        }
        r.add_station([30, 64, 1]);
        r.add_station([11, 64, 40]);
        r.ensure(&l_curves());
        let i = assemble(&mut r, &[[-30, 64, 0]], [1.0, 0.0, 0.0]);
        let turn = (0..r.net.graph.edges.len())
            .find(|&e| r.net.graph.edges[e].turn.is_some())
            .unwrap();
        let east = r.station_index([30, 64, 1]).unwrap();
        let south = r.station_index([11, 64, 40]).unwrap();
        assert!(r.navigate(i, &[south]));
        let nav = r.trains[i].train.nav.clone().unwrap();
        assert_eq!(nav.path, vec![turn], "into the turn at the junction");
        assert!(r.navigate(i, &[east]));
        let nav = r.trains[i].train.nav.clone().unwrap();
        assert_eq!(nav.path.len(), 1);
        assert!(r.net.graph.edges[nav.path[0]].turn.is_none(), "straight on");
        // It runs there, past the junction.
        let mut events = Vec::new();
        for _ in 0..1000 {
            r.tick(&mut events);
            if events.iter().any(|e| e.run == Run::Arrived) {
                break;
            }
        }
        let front = r.trains[i].train.points[0].at(&r.net.graph);
        assert!(
            (front[0] - 30.5).abs() < 0.5 && front[2] == 0.5,
            "{front:?}"
        );
    }

    #[test]
    fn a_train_waits_before_a_signal_whose_section_is_held() {
        let mut r = Railway::default();
        let mut t = HashMap::new();
        line(&mut t, [0, 64, 0], [1, 0, 0], 200, TrackShape::Xo);
        for (p, s) in t {
            r.set_track(p, s);
        }
        r.add_signal([100, 64, 1], SignalType::Entry, false);
        r.add_station([180, 64, 1]);
        r.ensure(&Curves::default());
        let a = assemble(&mut r, &[[10, 64, 0]], [1.0, 0.0, 0.0]);
        let b = assemble(&mut r, &[[150, 64, 0]], [1.0, 0.0, 0.0]);
        let station = r.station_index([180, 64, 1]).unwrap();
        assert!(r.navigate(a, &[station]));
        let mut events = Vec::new();
        for _ in 0..1500 {
            r.tick(&mut events);
        }
        let front = r.trains[a].train.points[0].at(&r.net.graph);
        assert!(
            front[0] < 100.5 && front[0] > 99.0,
            "waits before the signal: {front:?}"
        );
        assert!(r.trains[a].train.nav.is_some(), "still going there");
        // The other train leaves the section: it goes on.
        r.trains.remove(b);
        for _ in 0..1500 {
            r.tick(&mut events);
            if events.iter().any(|e| e.run == Run::Arrived) {
                break;
            }
        }
        let front = r.trains[a].train.points[0].at(&r.net.graph);
        assert!((front[0] - 180.5).abs() < 0.5, "{front:?}");
    }

    #[test]
    fn a_scheduled_train_shuttles_between_stations() {
        let mut r = l_shape();
        r.add_station([-30, 64, 1]);
        r.add_station([11, 64, 40]);
        r.stations.insert([-30, 64, 1], "West".into());
        r.stations.insert([11, 64, 40], "South".into());
        r.ensure(&l_curves());
        let i = assemble(&mut r, &[[-30, 64, 0]], [1.0, 0.0, 0.0]);
        r.trains[i].train.at_station = Some([-30, 64, 1]);
        r.trains[i].schedule = Some(Schedule {
            entries: vec![
                Entry {
                    station: "South".into(),
                    conditions: vec![vec![Condition::Delay(2 * SECONDS)]],
                },
                Entry {
                    station: "W*".into(),
                    conditions: vec![vec![Condition::Delay(2 * SECONDS)]],
                },
            ],
            cyclic: true,
        });
        let mut events = Vec::new();
        let mut visits = Vec::new();
        for _ in 0..3000 {
            r.tick(&mut events);
            if events.iter().any(|e| e.run == Run::Arrived) {
                visits.push(r.trains[i].train.at_station.unwrap());
            }
        }
        assert!(visits.len() >= 4, "{visits:?}");
        assert_eq!(visits[0], [11, 64, 40]);
        assert_eq!(visits[1], [-30, 64, 1]);
        assert_eq!(visits[2], [11, 64, 40]);
    }

    #[test]
    fn trains_are_found_again_when_the_graph_is_built_anew() {
        let mut r = l_shape();
        r.add_station([11, 64, 40]);
        r.ensure(&l_curves());
        let i = assemble(&mut r, &[[-30, 64, 0]], [1.0, 0.0, 0.0]);
        let south = r.station_index([11, 64, 40]).unwrap();
        assert!(r.navigate(i, &[south]));
        let mut events = Vec::new();
        // Into the turn.
        while r.net.graph.edges[r.trains[i].train.points[0].edge]
            .turn
            .is_none()
        {
            r.tick(&mut events);
        }
        let before = r.trains[i].train.points[0].at(&r.net.graph);
        // Track laid elsewhere: the graph is built again, the train found on it.
        r.set_track([-60, 64, 30], TrackShape::Xo);
        assert!(r.ensure(&l_curves()));
        let t = &r.trains[i].train;
        assert!(t.bound);
        let after = t.points[0].at(&r.net.graph);
        assert!(length(sub(before, after)) < 1e-6, "{before:?} {after:?}");
        assert!(r.trains[i].train.nav.is_some());
        // Saved and loaded: unbound until the graph is built, then on the curve again.
        r.trains[i].train.remember(&r.net);
        let saved = r.trains[i].clone();
        r.trains.clear();
        r.load_trains(vec![saved]);
        r.ensure(&l_curves());
        let t = &r.trains[0].train;
        assert!(t.bound);
        let again = t.points[0].at(&r.net.graph);
        assert!(length(sub(before, again)) < 1e-6);
        while !r.trains[0].train.points.is_empty() {
            r.tick(&mut events);
            if events.iter().any(|e| e.run == Run::Arrived) {
                break;
            }
        }
        assert_eq!(r.trains[0].train.at_station, Some([11, 64, 40]));
    }

    #[test]
    fn clients_get_the_pose_and_its_turn_on_a_curve() {
        let mut r = l_shape();
        r.add_station([11, 64, 40]);
        r.ensure(&l_curves());
        let i = assemble(&mut r, &[[-30, 64, 0]], [1.0, 0.0, 0.0]);
        r.trains[i].cars = vec![vec![([0, 0, 0], 7), ([0, 1, 0], 9)]];
        let m = r.trains[i].motion(&r.net, 2, true);
        let c = m.cars[0];
        assert_eq!((c.yaw, c.yaw_rate, c.pitch), (0.0, 0.0, 0.0));
        assert!((c.at[0] + 29.5).abs() < 1e-6 && c.at[2] == 0.5);
        assert_eq!(m.bogeys.len(), 1);
        assert_eq!(Motion::decode(&m.encode()), Ok(m.clone()));
        let moving = Motion { blocks: None, ..m };
        assert_eq!(Motion::decode(&moving.encode()), Ok(moving));
        assert_eq!(Motion::decode(&7u32.to_le_bytes()), Err(Some(7)));
        // Mid-turn: turning clockwise looking down (east to south), so its yaw falls.
        let south = r.station_index([11, 64, 40]).unwrap();
        assert!(r.navigate(i, &[south]));
        let mut events = Vec::new();
        while r.net.graph.edges[r.trains[i].train.points[1].edge]
            .turn
            .is_none()
        {
            r.tick(&mut events);
        }
        r.tick(&mut events);
        let m = r.trains[i].motion(&r.net, 2, false);
        let c = m.cars[0];
        assert!(c.yaw < 0.0 && c.yaw > -90.0, "{}", c.yaw);
        assert!(c.yaw_rate < 0.0, "{}", c.yaw_rate);
        assert!(r.trains[i].due(&r.net, 1000, &events[0]));
        // East along x is yaw 0; facing south is a quarter turn clockwise.
        assert_eq!(yaw_pitch([0.0, 0.0, 1.0], [1, 0]).0, -90.0);
        let (_, pitch) = yaw_pitch([0.0, 1.0, 1.0], [0, 1]);
        assert!((pitch - 45.0).abs() < 1e-9);
    }

    /// A straight run of track along x from x = 0 to `n - 1` at z = 0.
    fn straight(n: i32) -> Railway {
        let mut r = Railway::default();
        let mut t = HashMap::new();
        line(&mut t, [0, 64, 0], [1, 0, 0], n, TrackShape::Xo);
        for (p, s) in t {
            r.set_track(p, s);
        }
        r
    }

    fn structure(blocks: usize, bogeys: &[i32], forward: bool) -> Structure {
        Structure {
            blocks,
            bogeys: bogeys.to_vec(),
            forward,
            ..Structure::default()
        }
    }

    /// Bogeys at 0, 3, 8 and 13 along a station's track: the first two carry one carriage (two
    /// bogeys), the others one each; controls in the first face forward.
    fn three_carriage_layout() -> Layout {
        layout(&[0, 3, 8, 13], |i| match i {
            0 => structure(6, &[0, 3], true),
            2 => structure(3, &[8], false),
            _ => structure(2, &[13], false),
        })
        .expect("assembles")
    }

    fn push(r: &mut Railway, train: Train) -> usize {
        let id = r.next_id();
        let cars = vec![Vec::new(); train.carriages.len()];
        r.trains.push(TrainOnTrack {
            id,
            train,
            cars,
            heading: [1, 0],
            schedule: None,
            runtime: Runtime::new(),
            idle: 0,
            column: (0, 0),
            sent_tick: 0,
            sent_edge: 0,
            fresh: true,
        });
        r.trains.len() - 1
    }

    #[test]
    fn a_station_assembles_carriages_by_create_s_rules() {
        // The frontmost bogey stands right beside the station's track block; bogeys at least
        // three apart.
        let any = |_: usize| structure(5, &[], true);
        assert_eq!(
            layout(&[1, 5], any),
            Err(AssemblyError::FrontmostBogeyAtStation)
        );
        assert_eq!(
            layout(&[], any),
            Err(AssemblyError::FrontmostBogeyAtStation)
        );
        assert_eq!(
            layout(&[0, 2], any),
            Err(AssemblyError::BogeysTooClose(1, 2))
        );
        // A bogey alone is nothing attached; three bogeys in one structure are too many; the
        // second must be the next bogey; controls across the track are refused; some must face
        // the way it was assembled.
        assert_eq!(
            layout(&[0], |_| structure(1, &[0], true)),
            Err(AssemblyError::NothingAttached(1))
        );
        assert_eq!(
            layout(&[0, 3, 6], |_| structure(9, &[0, 3, 6], true)),
            Err(AssemblyError::TooManyBogeys(3))
        );
        assert_eq!(
            layout(&[0, 3, 6], |_| structure(9, &[0, 6], true)),
            Err(AssemblyError::NotConnectedInOrder)
        );
        assert_eq!(
            layout(&[0], |_| Structure {
                sideways: true,
                ..structure(4, &[0], true)
            }),
            Err(AssemblyError::SidewaysControls)
        );
        assert_eq!(
            layout(&[0, 4], |_| structure(4, &[], false)),
            Err(AssemblyError::NoControls)
        );
        let both = layout(&[0], |_| Structure {
            backward: true,
            ..structure(4, &[0], true)
        })
        .unwrap();
        assert!(both.double_ended, "controls facing back drive it backwards");
        assert_eq!(AssemblyError::BogeysTooClose(1, 2).args(), ["1", "2"]);

        let l = three_carriage_layout();
        assert_eq!(
            l.carriages,
            vec![
                CarriageShape {
                    first: 0,
                    two: true,
                    bogey_spacing: 3
                },
                CarriageShape {
                    first: 2,
                    two: false,
                    bogey_spacing: 0
                },
                CarriageShape {
                    first: 3,
                    two: false,
                    bogey_spacing: 0
                },
            ]
        );
        // `carriageSpacing`: from each carriage's last bogey to the next one's first.
        assert_eq!(l.spacing, vec![5, 5]);
        assert_eq!(l.offsets[..4], [-0.5, 1.5, 2.5, 4.5]);
        assert!(!l.double_ended);

        // On the track, the station's track block at x = 30, assembled toward -x: the wheels
        // where Create puts them (front toward the station), the anchors as far apart as the
        // bogeys were.
        let mut r = straight(40);
        r.ensure(&Curves::default());
        let sizes = [
            BogeySize::Small,
            BogeySize::Large,
            BogeySize::Small,
            BogeySize::Small,
        ];
        let t = Train::assembled(&r.net, [30.0, 64.0, 0.5], [-1.0, 0.0, 0.0], &l, &sizes)
            .expect("on the track");
        let g = &r.net.graph;
        let xs: Vec<f64> = t.points.iter().map(|p| p.at(g)[0]).collect();
        assert_eq!(xs, [30.5, 28.5, 27.5, 25.5, 22.5, 20.5, 17.5, 15.5]);
        assert!(t.points.iter().all(|p| p.at(g)[2] == 0.5));
        let (pivot, front) = t.carriage_pose(g, 0);
        assert!(front[0] > 0.99 && pivot[0] == 29.5);
        let d = |a: usize, b: usize| distance(t.anchor(g, a), t.anchor(g, b));
        assert!((d(0, 1) - 3.0).abs() < 1e-9);
        assert!((d(1, 2) - 5.0).abs() < 1e-9);
        assert!((d(2, 3) - 5.0).abs() < 1e-9);
        assert_eq!(BogeySize::Large.wheel_radius(), 12.5 / 16.0);
        // Off the end of the track: no train.
        assert!(Train::assembled(&r.net, [8.0, 64.0, 0.5], [-1.0, 0.0, 0.0], &l, &sizes).is_none());
    }

    #[test]
    fn carriages_follow_a_curve_without_drifting_apart() {
        let mut r = l_shape();
        r.add_station([11, 64, 40]);
        r.ensure(&l_curves());
        // Three carriages on the x straight, assembled toward -x from x = -10: front east.
        let train = Train::assembled(
            &r.net,
            [-10.0, 64.0, 0.5],
            [-1.0, 0.0, 0.0],
            &three_carriage_layout(),
            &[BogeySize::Small; 4],
        )
        .unwrap();
        let i = push(&mut r, train);
        let south = r.station_index([11, 64, 40]).unwrap();
        assert!(r.navigate(i, &[south]));
        let mut events = Vec::new();
        let (mut worst_coupling, mut worst_bogey, mut worst_wheels) = (0.0f64, 0.0f64, 0.0f64);
        let mut most_on_curve = 0;
        for _ in 0..3000 {
            r.tick(&mut events);
            let t = &r.trains[i].train;
            let g = &r.net.graph;
            let d = |a: usize, b: usize| distance(t.anchor(g, a), t.anchor(g, b));
            // Straight-line distances, as Create measures them.
            worst_bogey = worst_bogey.max((d(0, 1) - 3.0).abs());
            worst_coupling = worst_coupling
                .max((d(1, 2) - 5.0).abs())
                .max((d(2, 3) - 5.0).abs());
            for b in 0..4 {
                let w = distance(t.points[2 * b].at(g), t.points[2 * b + 1].at(g));
                worst_wheels = worst_wheels.max((w - WHEEL_SPACING).abs());
            }
            most_on_curve = most_on_curve.max(
                t.points
                    .iter()
                    .filter(|p| g.edges[p.edge].turn.is_some())
                    .count(),
            );
            if events.iter().any(|e| e.run == Run::Arrived) {
                break;
            }
        }
        let t = &r.trains[i].train;
        assert_eq!(t.at_station, Some([11, 64, 40]), "arrived");
        assert!(
            most_on_curve >= 4,
            "it ran through the turn: {most_on_curve}"
        );
        assert!(
            worst_bogey < 0.15,
            "bogeys kept their spacing: {worst_bogey}"
        );
        assert!(
            worst_coupling < 0.25,
            "carriages kept theirs: {worst_coupling}"
        );
        assert!(worst_wheels < 0.05, "wheels kept theirs: {worst_wheels}");
        // All of it on the south straight now, every carriage facing south.
        let g = &r.net.graph;
        for c in 0..3 {
            let (at, front) = t.carriage_pose(g, c);
            assert!((at[0] - 10.5).abs() < 1e-6, "{at:?}");
            assert!(front[2] > 0.999, "{front:?}");
        }
        let d = |a: usize, b: usize| distance(t.anchor(g, a), t.anchor(g, b));
        assert!((d(0, 1) - 3.0).abs() < 0.05 && (d(2, 3) - 5.0).abs() < 0.05);
        // Each carriage and bogey is its own part for clients.
        let m = r.trains[i].motion(&r.net, 2, true);
        assert_eq!((m.cars.len(), m.bogeys.len()), (3, 4));
        assert_eq!(Motion::decode(&m.encode()), Ok(m));
    }

    /// A long line with an entry signal at x = 100 (south side: eastbound), a station at
    /// x = 280, a train at x = 10.
    fn signalled() -> (Railway, usize) {
        let mut r = straight(300);
        r.add_signal([100, 64, 1], SignalType::Entry, false);
        r.add_station([280, 64, 1]);
        r.ensure(&Curves::default());
        let a = assemble(&mut r, &[[10, 64, 0]], [1.0, 0.0, 0.0]);
        (r, a)
    }

    fn search_from(r: &Railway, i: usize, station: usize) -> Option<Found> {
        let none = HashMap::new();
        let present = HashMap::new();
        let cx = SearchContext {
            me: r.trains[i].id,
            penalties: &none,
            present: &present,
            occupancy: &r.occupancy,
            ticks_waiting: 0,
        };
        search(
            &r.net,
            r.trains[i].train.leading(&r.net.graph, true),
            &[station],
            &cx,
        )
    }

    #[test]
    fn a_signal_only_lets_trains_through_from_its_side() {
        // The station lies west of a signal standing for eastbound trains only: no way there
        // from the east.
        let mut r = straight(300);
        r.add_signal([100, 64, 1], SignalType::Entry, false);
        r.add_station([20, 64, 1]);
        r.ensure(&Curves::default());
        let a = assemble(&mut r, &[[200, 64, 0]], [-1.0, 0.0, 0.0]);
        r.trains[a].train.double_ended = false;
        let west = r.station_index([20, 64, 1]).unwrap();
        assert!(
            !r.navigate(a, &[west]),
            "the wrong way past a one-sided signal"
        );
        // A signal block on the other side too: one boundary, both ways.
        r.add_signal([100, 64, -1], SignalType::Entry, false);
        r.ensure(&Curves::default());
        assert_eq!(r.net.signals.len(), 1);
        assert!(r.navigate(a, &[west]));
        // An eastbound train passes the first one.
        let (mut r, a) = signalled();
        let station = r.station_index([280, 64, 1]).unwrap();
        assert!(r.navigate(a, &[station]));
    }

    #[test]
    fn redstone_holds_a_signal_red() {
        let (mut r, a) = signalled();
        let station = r.station_index([280, 64, 1]).unwrap();
        let mut events = Vec::new();
        r.tick(&mut events);
        assert_eq!(r.lights.get(&[100, 64, 1]), Some(&false), "green: dark");
        let free = search_from(&r, a, station).unwrap();
        // Powered: red, and a search weighs it as Create does (`REDSTONE_RED_SIGNAL`).
        r.add_signal([100, 64, 1], SignalType::Entry, true);
        let red = search_from(&r, a, station).unwrap();
        assert_eq!(red.cost - free.cost, f64::from(REDSTONE_RED_SIGNAL));
        r.lights_changed.clear();
        r.tick(&mut events);
        assert_eq!(r.lights_changed, [([100, 64, 1], Some(true))]);
        assert!(r.navigate(a, &[station]));
        for _ in 0..1500 {
            r.tick(&mut events);
        }
        let front = r.trains[a].train.points[0].at(&r.net.graph);
        assert!(
            front[0] < 100.5 && front[0] > 99.0,
            "waits at the red signal: {front:?}"
        );
        // Power off: it goes on, the light goes dark.
        r.lights_changed.clear();
        r.add_signal([100, 64, 1], SignalType::Entry, false);
        r.tick(&mut events);
        assert_eq!(r.lights_changed, [([100, 64, 1], Some(false))]);
        for _ in 0..1500 {
            r.tick(&mut events);
            if events.iter().any(|e| e.run == Run::Arrived) {
                break;
            }
        }
        assert_eq!(r.trains[a].train.at_station, Some([280, 64, 1]));
    }

    #[test]
    fn signals_without_trains_work_once_per_change() {
        let mut r = straight(40);
        let curves = Curves::default();
        assert!(r.idle(&curves), "no track work without signals or trains");
        r.add_signal([10, 64, 1], SignalType::Entry, false);
        assert!(!r.idle(&curves), "a new signal: the graph and its state");
        r.ensure(&curves);
        let mut events = Vec::new();
        r.tick(&mut events);
        assert_eq!(r.lights_changed, [([10, 64, 1], Some(false))]);
        r.lights_changed.clear();
        assert!(r.idle(&curves), "then nothing per tick");
        r.add_signal([10, 64, 1], SignalType::Entry, true);
        assert!(!r.idle(&curves));
        r.ensure(&curves);
        r.tick(&mut events);
        assert_eq!(r.lights_changed, [([10, 64, 1], Some(true))]);
        r.lights_changed.clear();
        r.remove([10, 64, 1]);
        assert_eq!(
            r.lights_changed,
            [([10, 64, 1], None)],
            "clients drop its light"
        );
    }

    #[test]
    fn a_cross_signal_waits_for_its_whole_chain() {
        // Eastbound: a signal at x = 100, entry signals at x = 150 and 200; another train stands
        // between 150 and 200. Behind an entry signal at 100 the train runs on to wait at 150;
        // behind a cross signal it waits at 100 for the chain up to 200 to be free.
        for (kind, waits_at) in [(SignalType::Entry, 150.5), (SignalType::Cross, 100.5)] {
            let mut r = straight(300);
            r.add_signal([100, 64, 1], kind, false);
            r.add_signal([150, 64, 1], SignalType::Entry, false);
            r.add_signal([200, 64, 1], SignalType::Entry, false);
            r.add_station([280, 64, 1]);
            r.ensure(&Curves::default());
            let a = assemble(&mut r, &[[10, 64, 0]], [1.0, 0.0, 0.0]);
            let b = assemble(&mut r, &[[175, 64, 0]], [1.0, 0.0, 0.0]);
            let station = r.station_index([280, 64, 1]).unwrap();
            assert!(r.navigate(a, &[station]));
            let mut events = Vec::new();
            for _ in 0..1500 {
                r.tick(&mut events);
            }
            let front = r.trains[a].train.points[0].at(&r.net.graph);
            assert!(
                front[0] < waits_at && front[0] > waits_at - 1.5,
                "{kind:?}: waits before x = {waits_at}: {front:?}"
            );
            assert!(
                r.lights[&[150, 64, 1]],
                "the section beyond 150 is held: red"
            );
            if kind == SignalType::Cross {
                assert!(r.lights[&[100, 64, 1]], "its chain is held: red");
            }
            // The other train leaves: it goes on.
            r.trains.remove(b);
            for _ in 0..2000 {
                r.tick(&mut events);
                if events.iter().any(|e| e.run == Run::Arrived) {
                    break;
                }
            }
            assert_eq!(r.trains[0].train.at_station, Some([280, 64, 1]));
        }
    }

    #[test]
    fn a_train_reserves_the_section_it_is_about_to_enter() {
        // Two branches join at x = 3 into the west straight: one along x from the east, the
        // L's turn from the south. Each has a signal before the junction for the trains running
        // toward it; the shared section beyond holds a station.
        let mut r = l_shape();
        let mut t = HashMap::new();
        line(&mut t, [3, 64, 0], [1, 0, 0], 38, TrackShape::Xo);
        for (p, s) in t {
            r.set_track(p, s);
        }
        r.add_signal([10, 64, -1], SignalType::Entry, false);
        r.add_signal([11, 64, 20], SignalType::Entry, false);
        r.add_station([-30, 64, 1]);
        r.ensure(&l_curves());
        let a = assemble(&mut r, &[[20, 64, 0]], [-1.0, 0.0, 0.0]);
        let b = assemble(&mut r, &[[10, 64, 45]], [0.0, 0.0, -1.0]);
        let (ida, idb) = (r.trains[a].id, r.trains[b].id);
        let station = r.station_index([-30, 64, 1]).unwrap();
        assert!(r.navigate(a, &[station]));
        assert!(r.navigate(b, &[station]));
        let shared = r.net.section(r.net.blocks[&[0, 64, 0]].edge, 1.0);
        let mut events = Vec::new();
        let mut reserved_seen = false;
        for _ in 0..2000 {
            r.tick(&mut events);
            let inside = |i: usize| {
                r.trains[i]
                    .train
                    .points
                    .iter()
                    .any(|p| r.net.section(p.edge, p.position) == shared)
            };
            assert!(
                !(inside(a) && inside(b)),
                "never both in the shared section"
            );
            if !inside(a) && !inside(b) && r.occupancy.occupied_unless(shared, idb) {
                reserved_seen = true;
                assert!(
                    r.occupancy.occupied_unless(shared, ida),
                    "reserved: held for everyone until the tick ends"
                );
            }
        }
        assert!(reserved_seen, "the first train reserved the shared section");
        assert_eq!(r.trains[a].train.at_station, Some([-30, 64, 1]));
        let front = r.trains[b].train.points[0].at(&r.net.graph);
        assert!(
            front[2] > 20.5 && front[2] < 22.0,
            "the second waits at its signal: {front:?}"
        );
    }

    #[test]
    fn controls_drive_and_steer_a_train_by_hand() {
        // The junction: the x straight on east past the turn, the turn to the south (right).
        for (steer, south) in [(0, false), (-1, true)] {
            let mut r = l_shape();
            let mut t = HashMap::new();
            line(&mut t, [3, 64, 0], [1, 0, 0], 38, TrackShape::Xo);
            for (p, s) in t {
                r.set_track(p, s);
            }
            r.ensure(&l_curves());
            let a = assemble(&mut r, &[[-20, 64, 0]], [1.0, 0.0, 0.0]);
            let id = r.trains[a].id;
            r.drive(
                id,
                Manual {
                    throttle: 1.0,
                    steer,
                    inverted: false,
                },
            )
            .unwrap();
            let mut events = Vec::new();
            let mut top = 0.0f64;
            for _ in 0..70 {
                r.tick(&mut events);
                top = top.max(r.trains[a].train.speed);
            }
            assert!(top <= TOP_SPEED * MANUAL_SPEED + 1e-9, "{top}");
            assert!(top > 0.4, "{top}");
            // Let go: it coasts to a stop.
            r.drive(
                id,
                Manual {
                    throttle: 0.0,
                    steer,
                    inverted: false,
                },
            )
            .unwrap();
            for _ in 0..400 {
                r.tick(&mut events);
            }
            let t = &r.trains[a].train;
            assert_eq!(t.speed, 0.0);
            let front = t.points[0].at(&r.net.graph);
            if south {
                assert!(
                    front[2] > 8.0 && (front[0] - 10.5).abs() < 1e-6,
                    "{front:?}"
                );
            } else {
                assert!(front[0] > 12.0 && front[2] == 0.5, "{front:?}");
            }
        }
        assert!(Railway::default().drive(9, Manual::default()).is_none());
    }

    #[test]
    fn the_hud_s_throttle_caps_the_speed_by_hand() {
        use crate::train_controls::{Driving, FORWARD};
        let mut r = straight(600);
        r.ensure(&Curves::default());
        let a = assemble(&mut r, &[[8, 64, 0]], [1.0, 0.0, 0.0]);
        let id = r.trains[a].id;
        assert!(r.start_controlling(id));
        // 9/18 (`TrainHUD.onScroll` steps): half the speed by hand (`cappedTopSpeed`).
        r.trains[a].train.throttle = crate::train_hud::throttle(9);
        let (mut d, mut events) = (Driving::default(), Vec::new());
        for _ in 0..200 {
            r.control(id, 1 << FORWARD, false, &mut d);
            r.tick(&mut events);
        }
        let top = TOP_SPEED * MANUAL_SPEED;
        assert!((r.trains[a].train.speed - top / 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_player_s_keys_drive_the_train_as_create_s_controls() {
        use crate::train_controls::{BACK, Driving, FORWARD, JUMP, Prompt};
        let w = 1 << FORWARD;
        // A straight line, a station at x 10 (where the train stands) and one at x 500.
        let mut r = straight(600);
        r.add_station([10, 64, 1]);
        r.add_station([500, 64, 1]);
        r.stations.insert([10, 64, 1], "Home".into());
        r.stations.insert([500, 64, 1], "Far".into());
        r.ensure(&Curves::default());
        let a = assemble(&mut r, &[[8, 64, 0]], [1.0, 0.0, 0.0]);
        let id = r.trains[a].id;
        r.trains[a].train.at_station = Some([10, 64, 1]);
        r.trains[a].schedule = Some(Schedule {
            entries: Vec::new(),
            cyclic: false,
        });
        assert!(r.start_controlling(id));
        assert!(r.trains[a].runtime.paused, "the schedule pauses");
        let mut d = Driving::default();
        // Space at the station names it; W departs from it.
        assert_eq!(
            r.control(id, 1 << JUMP, false, &mut d),
            Some(Prompt::Arrived("Home".into()))
        );
        assert_eq!(
            r.control(id, w, false, &mut d),
            Some(Prompt::Departing("Home".into()))
        );
        // Held: it speeds up at 3 m/s² to 75% of top speed.
        let mut events = Vec::new();
        let mut speeds = Vec::new();
        for _ in 0..200 {
            r.control(id, w, false, &mut d);
            r.tick(&mut events);
            speeds.push(r.trains[a].train.speed);
        }
        assert!(
            (speeds[9] - 10.0 * ACCELERATION).abs() < 1e-9,
            "{}",
            speeds[9]
        );
        assert!((speeds[199] - TOP_SPEED * MANUAL_SPEED).abs() < 1e-9);
        // Controls facing its back: W drives it backwards, braking twice as hard against its
        // motion first; S forward at a quarter of the speed.
        let top = TOP_SPEED * MANUAL_SPEED;
        r.control(id, w, true, &mut d);
        r.tick(&mut events);
        assert!((r.trains[a].train.speed - (top - 2.0 * ACCELERATION)).abs() < 1e-9);
        for _ in 0..200 {
            r.control(id, 1 << BACK, true, &mut d);
            r.tick(&mut events);
        }
        assert!((r.trains[a].train.speed - top / 4.0).abs() < 1e-9);
        // Let go: it coasts to a stop.
        r.release(id);
        for _ in 0..200 {
            r.tick(&mut events);
        }
        assert_eq!(r.trains[a].train.speed, 0.0);
        // Within one and a half braking distances of the far station (to the end of its edge),
        // it is offered; space then drives there by itself while space is held, telling the
        // progress, and stops at it.
        while r.trains[a].train.points[0].at(&r.net.graph)[0] < 410.0 {
            r.control(id, w, false, &mut d);
            r.tick(&mut events);
        }
        assert_eq!(
            r.control(id, 0, false, &mut d),
            Some(Prompt::Approach("Far".into()))
        );
        r.tick(&mut events);
        assert_eq!(r.control(id, 1 << JUMP, false, &mut d), Some(Prompt::None));
        assert!(r.trains[a].train.nav.is_some());
        let mut progress = Vec::new();
        for _ in 0..1000 {
            if let Some(Prompt::Progress(p)) = r.control(id, 1 << JUMP, false, &mut d) {
                progress.push(p);
            }
            r.tick(&mut events);
            if r.trains[a].train.at_station.is_some() {
                break;
            }
        }
        assert_eq!(r.trains[a].train.at_station, Some([500, 64, 1]));
        assert!(progress.windows(2).all(|p| p[1] <= p[0]), "{progress:?}");
        // A train that is gone lets the controls go.
        assert!(r.control(9, 0, false, &mut d).is_none(), "no such train");
    }

    #[test]
    fn an_observer_is_lit_while_a_train_passes_over_it() {
        // An observer beside the track at x 60, a filtered one at x 61, a station at x 120 and a
        // two-bogey train from x 10.
        let mut r = straight(200);
        r.add_observer([60, 64, 1], false);
        r.add_observer([61, 64, -1], false);
        r.set_observer_filtered([61, 64, -1], true);
        r.add_station([120, 64, 1]);
        r.ensure(&Curves::default());
        assert_eq!(r.net.observers.len(), 2);
        let a = assemble(&mut r, &[[14, 64, 0], [10, 64, 0]], [1.0, 0.0, 0.0]);
        // The graph ran before the train: what it stands over is looked at as it moves.
        let station = r.station_index([120, 64, 1]).unwrap();
        assert!(r.navigate(a, &[station]));
        let mut events = Vec::new();
        let mut lit_from = None;
        let mut dark_at = None;
        for tick in 0..2000 {
            r.tick(&mut events);
            for (p, lit) in r.observer_changes.drain(..) {
                assert_eq!(p, [60, 64, 1], "the filtered one sees no cargo");
                if lit {
                    lit_from = Some(tick);
                } else {
                    dark_at = Some(tick);
                }
            }
            if events.iter().any(|e| e.run == Run::Arrived) {
                break;
            }
        }
        let (lit, dark) = (lit_from.unwrap(), dark_at.unwrap());
        assert!(dark > lit);
        // While lit, the train's points were over x 60.5 (the observer's track point); it went
        // dark 3 ticks after the last of them passed (8, counted down twice a tick).
        assert!(r.observers[&[60, 64, 1]].train.is_none());
        assert!(!r.observers[&[60, 64, 1]].lit);
        assert!(r.idle(&Curves::default()) || !r.trains.is_empty());
        // Without trains and with every observer dark, a tick does nothing.
        r.trains.clear();
        r.tick(&mut events);
        assert!(r.idle(&Curves::default()));
    }

    #[test]
    fn a_train_assembled_over_an_observer_lights_it() {
        let mut r = straight(100);
        r.add_observer([20, 64, 1], false);
        r.ensure(&Curves::default());
        assemble(&mut r, &[[22, 64, 0], [18, 64, 0]], [1.0, 0.0, 0.0]);
        // A rebuilt graph looks at what each train stands over.
        r.set_track([150, 64, 0], TrackShape::Xo);
        r.ensure(&Curves::default());
        let mut events = Vec::new();
        r.tick(&mut events);
        assert_eq!(r.observer_changes, vec![([20, 64, 1], true)]);
        assert_eq!(r.observers[&[20, 64, 1]].activated, OBSERVER_KEEP_ALIVE - 1);
        // Across a node (the chunk line at x 16): both sides of it are under the train, past
        // it nothing is.
        let mut r = straight(100);
        for x in [15, 16, 25] {
            r.add_observer([x, 64, 1], false);
        }
        r.ensure(&Curves::default());
        let i = assemble(&mut r, &[[19, 64, 0], [13, 64, 0]], [1.0, 0.0, 0.0]);
        let points = r.trains[i].train.points.clone();
        let covered: Vec<[i32; 3]> = r
            .net
            .observers
            .iter()
            .filter(|(_, at)| r.net.covers(&points, *at))
            .map(|(p, _)| *p)
            .collect();
        assert_eq!(covered, vec![[15, 64, 1], [16, 64, 1]]);
    }
}
