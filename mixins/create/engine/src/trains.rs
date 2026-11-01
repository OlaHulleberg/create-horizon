//! Create's trains on the track graph (`Train`, `Carriage`, `CarriageBogey`, `Navigation`,
//! `TravellingPoint`, `ScheduleRuntime`'s driving): a train's bogeys stand on the graph as two
//! wheel points each; it finds its way to a station with Create's search (`findPathTo`, its
//! penalties for stations, trains and red signals passed), drives there with Create's speed
//! control (top speed, acceleration, braking distance, the slow approach, slowing for curves,
//! waiting before a signal whose section another train holds) and stops with its front just past
//! the station. A schedule sends it from station to station.

use crate::bezier::Vec3;
use crate::graph::{Graph, Point};
use crate::network::{Key, Mark, Network, key_position};
use crate::schedule::{Order, Runtime, Schedule, station_matches};
use crate::track::TrackShape;
use std::collections::{BinaryHeap, HashMap, HashSet};

/// `trainTopSpeed` (28 m/s), `trainTurningTopSpeed` (14 m/s) and `trainAcceleration` (3 m/s²)
/// in blocks a tick.
pub const TOP_SPEED: f64 = 28.0 / 20.0;
pub const TURN_TOP_SPEED: f64 = 14.0 / 20.0;
pub const ACCELERATION: f64 = 3.0 / 400.0;
/// `StandardBogeyBlock.getWheelPointSpacing`.
pub const WHEEL_SPACING: f64 = 2.0;
/// `Train.Penalties`.
pub const STATION: i32 = 50;
pub const STATION_WITH_TRAIN: i32 = 300;
pub const IDLE_TRAIN: i32 = 700;
pub const ARRIVING_TRAIN: i32 = 50;
pub const WAITING_TRAIN: i32 = 50;
pub const ANY_TRAIN: i32 = 25;
pub const RED_SIGNAL: i32 = 25;
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
/// front), its speed (blocks a tick, positive toward its front), where it is going and the
/// station it stands at.
#[derive(Clone, Debug, PartialEq)]
pub struct Train {
    pub points: Vec<Point>,
    pub speed: f64,
    pub nav: Option<Nav>,
    pub at_station: Option<[i32; 3]>,
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

/// Which trains stand in each signal section (two distinct ones are enough to tell whether
/// another train than the one asking does).
#[derive(Clone, Debug, Default)]
pub struct Occupancy {
    slots: Vec<[u32; 2]>,
}

impl Occupancy {
    pub fn fill(&mut self, net: &Network, trains: &[TrainOnTrack]) {
        self.slots.clear();
        self.slots.resize(net.section_slots(), [0, 0]);
        if net.signals.is_empty() {
            return;
        }
        for t in trains.iter().filter(|t| t.train.bound) {
            for p in &t.train.points {
                let s = &mut self.slots[net.section(p.edge, p.position)];
                if s[0] == 0 || s[0] == t.id {
                    s[0] = t.id;
                } else if s[1] == 0 {
                    s[1] = t.id;
                }
            }
        }
    }

    /// `SignalEdgeGroup.isOccupiedUnless(train)`.
    pub fn occupied_unless(&self, section: usize, me: u32) -> bool {
        self.slots
            .get(section)
            .is_some_and(|s| (s[0] != 0 && s[0] != me) || (s[1] != 0 && s[1] != me))
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

/// What lies ahead of a navigating train (`Navigation.tick`'s signal scout).
struct Scouted {
    signal: Option<f64>,
    curve: Option<f64>,
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
            Mark::Signal(_) => {
                if cx
                    .occupancy
                    .occupied_unless(net.section(initial, pos), cx.me)
                {
                    penalty += signal_weight;
                    signal_weight /= 2;
                }
            }
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
        for &n in &next {
            let mut penalty = entry.penalty + cx.penalties.get(&n).copied().unwrap_or(0);
            let distance = entry.distance + g.length(n);
            let mut has_destination = false;
            for &(pos, mark) in &net.marks[n] {
                match mark {
                    Mark::Signal(_) => {
                        if cx.occupancy.occupied_unless(net.section(n, pos), cx.me) {
                            penalty += signal_weight;
                            signal_weight /= 2;
                        }
                    }
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

impl Train {
    /// A train whose bogeys stand on the track blocks `under` (front first), facing `front`
    /// (a unit along the track): each bogey's wheels a wheel spacing apart about its block's
    /// centre (`CarriageBogey`'s points). None when a wheel would be off the track.
    pub fn on_bogeys(net: &Network, under: &[[i32; 3]], front: Vec3) -> Option<Train> {
        let g = &net.graph;
        let mut points = Vec::with_capacity(under.len() * 2);
        for b in under {
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
            lead.travel_signed(g, WHEEL_SPACING / 2.0, |_| 0);
            trail.travel_signed(g, -WHEEL_SPACING / 2.0, |_| 0);
            if lead.blocked || trail.blocked {
                return None;
            }
            points.push(lead);
            points.push(trail);
        }
        let mut t = Train {
            points,
            speed: 0.0,
            nav: None,
            at_station: None,
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
        self.bound = !self.points.is_empty();
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

    /// The pose of the carriage (`alignEntity`): the anchor of bogey `pivot`, and its front's
    /// direction from its rotation anchors (its two bogeys, or one bogey's wheels).
    pub fn pose(&self, g: &Graph, pivot: usize) -> (Vec3, Vec3) {
        let bogeys = self.points.len() / 2;
        let (a, b) = if bogeys >= 2 {
            (self.anchor(g, 0), self.anchor(g, bogeys - 1))
        } else {
            (self.points[0].at(g), self.points[1].at(g))
        };
        let d = sub(a, b);
        let l = length(d);
        let heading = if l < 1.0e-6 {
            [1.0, 0.0, 0.0]
        } else {
            [d[0] / l, d[1] / l, d[2] / l]
        };
        (self.anchor(g, pivot.min(bogeys.saturating_sub(1))), heading)
    }

    /// Moves every point `distance` along (`Carriage.travel`): the leading one follows `path`
    /// at junctions (`navigateOptions`), each other the point ahead of it. The distance moved
    /// and whether the track ended.
    pub fn travel(&mut self, g: &Graph, distance: f64, path: &mut Vec<usize>) -> (f64, bool) {
        let n = self.points.len();
        if n == 0 || distance == 0.0 {
            return (0.0, false);
        }
        let forward = distance > 0.0;
        let order = |k: usize| if forward { k } else { n - 1 - k };
        let lead = order(0);
        let moved = self.points[lead].travel_signed(g, distance, |c| {
            if let Some(&want) = path.first()
                && let Some(i) = c.iter().position(|&e| e == want)
            {
                path.remove(0);
                return i;
            }
            0
        });
        let blocked = self.points[lead].blocked;
        for k in 1..n {
            let (i, ahead) = (order(k), order(k - 1));
            let target = if forward {
                self.points[ahead].edge
            } else {
                Graph::reverse(self.points[ahead].edge)
            };
            self.points[i].travel_signed(g, moved, |c| follow(g, target, c));
        }
        (moved, blocked)
    }

    /// `Navigation.tick`'s scout: from the leading point along the path, the nearest signal
    /// within `scan` whose section beyond is held by another train, and the nearest curve.
    fn scout(&self, net: &Network, occ: &Occupancy, me: u32, nav: &Nav, scan: f64) -> Scouted {
        let g = &net.graph;
        let lead = self.leading(g, !nav.behind);
        // On a curve already: it counts from here (`edgeTraversedFrom` on the first edge).
        let mut out = Scouted {
            signal: None,
            curve: slows(g, lead.edge).then_some(0.0),
        };
        let (mut edge, mut base, mut cursor) = (lead.edge, -lead.position, 0);
        let limit = nav.distance + 50.0;
        let mut first = true;
        loop {
            for &(pos, mark) in &net.marks[edge] {
                if first && pos <= lead.position {
                    continue;
                }
                let d = base + pos;
                match mark {
                    Mark::Signal(_) => {
                        if d > scan {
                            return out;
                        }
                        if occ.occupied_unless(net.section(edge, pos), me) {
                            out.signal = Some(d);
                            return out;
                        }
                    }
                    Mark::Station(i) => {
                        if net.stations[i].0 == nav.station {
                            return out;
                        }
                    }
                }
            }
            base += g.length(edge);
            if base > limit {
                return out;
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
                return out;
            };
            if count > 1 && nav.path.get(cursor) == Some(&n) {
                cursor += 1;
            }
            if out.curve.is_none() && slows(g, n) {
                out.curve = Some(base.max(0.0));
            }
            edge = n;
            first = false;
        }
    }

    /// `Train.tick` with `Navigation.tick`: the speed toward the destination, then the move.
    /// Arrived when its leading point passes the station.
    pub fn tick(&mut self, net: &Network, occ: &Occupancy, me: u32) -> Run {
        if !self.bound {
            return Run::Still;
        }
        let g = &net.graph;
        let Some(mut nav) = self.nav.take() else {
            // `tickPassiveSlowdown`.
            if self.speed == 0.0 {
                return Run::Still;
            }
            self.speed = if self.speed > 0.0 {
                (self.speed - ACCELERATION).max(0.0)
            } else {
                (self.speed + ACCELERATION).min(0.0)
            };
            let (_, blocked) = self.travel(g, self.speed, &mut Vec::new());
            if blocked {
                self.speed = 0.0;
                return Run::Blocked;
            }
            return Run::Moving;
        };
        let dir = if nav.behind { -1.0 } else { 1.0 };
        let braking = self.speed * self.speed / (2.0 * ACCELERATION);
        let look_ahead = if self.at_station.is_some() { 4.5 } else { 0.0 };
        let no_flicker = braking + 3.0 - braking % 3.0;
        // `Mth.clamp(value, min, max)`.
        let scan = if no_flicker < look_ahead {
            look_ahead
        } else {
            no_flicker.min(nav.distance)
        };
        let seen = self.scout(net, occ, me, &nav, scan);
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
        let (moved, blocked) = self.travel(g, self.speed, &mut nav.path);
        nav.distance -= moved * dir;
        if nav.distance <= 0.0 {
            self.speed = 0.0;
            self.at_station = Some(nav.station);
            return Run::Arrived;
        }
        if blocked {
            self.speed = 0.0;
            return Run::Blocked;
        }
        self.nav = Some(nav);
        Run::Moving
    }

    /// `Navigation.findPathTo`: searched ahead of its front and behind its back (not back
    /// out of a path it is already running), the cheaper one.
    pub fn find_path(&self, net: &Network, targets: &[usize], cx: &SearchContext) -> Option<Found> {
        let g = &net.graph;
        let mut found: [Option<Found>; 2] = [None, None];
        for (k, forward) in [true, false].into_iter().enumerate() {
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

/// An assembled train: its motion, its blocks (offset from the block of bogey `pivot`, state),
/// the way its front faced when assembled (a unit along x or z), its schedule, and the column
/// it is saved with.
#[derive(Clone, Debug, PartialEq)]
pub struct TrainOnTrack {
    pub id: u32,
    pub train: Train,
    pub blocks: Vec<([i32; 3], u32)>,
    pub pivot: usize,
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

    /// The column its pivot bogey stands in.
    pub fn column_on(&self, net: &Network) -> (i32, i32) {
        let at = if self.train.bound {
            self.train.pose(&net.graph, self.pivot).0
        } else {
            let k = (2 * self.pivot).min(self.train.placed.len().saturating_sub(1));
            self.train.placed.get(k).map_or([0.0; 3], |p| p.at)
        };
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

/// The mod message channel for trains, server to client.
pub const CHANNEL: &str = "create:train";

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

/// A train's motion as clients get it: where its pivot is, its velocity (blocks a second) and
/// yaw rate (degrees a second) until the next update, its yaw and pitch, the way it faced when
/// assembled, and its blocks (only when clients must learn them).
#[derive(Clone, Debug, PartialEq)]
pub struct Motion {
    pub id: u32,
    pub pivot: Vec3,
    pub velocity: [f32; 3],
    pub yaw: f32,
    pub yaw_rate: f32,
    pub pitch: f32,
    pub heading: [i32; 2],
    pub blocks: Option<Vec<([i32; 3], u32)>>,
}

const HEADINGS: [[i32; 2]; 4] = [[1, 0], [0, 1], [-1, 0], [0, -1]];

impl Motion {
    /// The message: id, pivot, velocity, yaw, yaw rate, pitch (f32s), heading, then the block
    /// count and blocks if any. A message of the id alone clears the train.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(48 + self.blocks.as_ref().map_or(0, |b| 4 + b.len() * 16));
        out.extend_from_slice(&self.id.to_le_bytes());
        for v in self.pivot {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in self
            .velocity
            .into_iter()
            .chain([self.yaw, self.yaw_rate, self.pitch])
        {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(
            HEADINGS
                .iter()
                .position(|h| *h == self.heading)
                .unwrap_or(0) as u8,
        );
        if let Some(blocks) = &self.blocks {
            let n = blocks.len().min(30_000 / 16);
            out.extend_from_slice(&(n as u32).to_le_bytes());
            for (o, s) in &blocks[..n] {
                for v in o {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                out.extend_from_slice(&s.to_le_bytes());
            }
        }
        out
    }

    /// The motion in a message, or the id of a train to clear.
    pub fn decode(d: &[u8]) -> Result<Motion, Option<u32>> {
        let word = |k: usize| -> [u8; 4] { [d[k], d[k + 1], d[k + 2], d[k + 3]] };
        if d.len() < 4 {
            return Err(None);
        }
        let id = u32::from_le_bytes(word(0));
        if d.len() < 4 + 24 + 24 + 1 {
            return Err(Some(id));
        }
        let float = |k: usize| f64::from_le_bytes(d[k..k + 8].try_into().unwrap_or([0; 8]));
        let single = |k: usize| f32::from_le_bytes(word(k));
        let pivot = [float(4), float(12), float(20)];
        let f: [f32; 6] = std::array::from_fn(|i| single(28 + 4 * i));
        let heading = HEADINGS[usize::from(d[52]).min(3)];
        let blocks = (d.len() >= 57).then(|| {
            let n = u32::from_le_bytes(word(53)) as usize;
            d[57..]
                .chunks_exact(16)
                .take(n)
                .map(|r| {
                    let i = |k: usize| i32::from_le_bytes([r[k], r[k + 1], r[k + 2], r[k + 3]]);
                    ([i(0), i(4), i(8)], i(12) as u32)
                })
                .collect()
        });
        Ok(Motion {
            id,
            pivot,
            velocity: [f[0], f[1], f[2]],
            yaw: f[3],
            yaw_rate: f[4],
            pitch: f[5],
            heading,
            blocks,
        })
    }
}

impl TrainOnTrack {
    /// Its motion now and as it will be `ticks` later at this speed along its path, for
    /// clients to draw in between.
    pub fn motion(&self, net: &Network, ticks: u64, full: bool) -> Motion {
        let g = &net.graph;
        let (pivot, front) = self.train.pose(g, self.pivot);
        let (yaw, pitch) = yaw_pitch(front, self.heading);
        let mut ahead = self.train.clone();
        let mut path = ahead
            .nav
            .as_ref()
            .map(|n| n.path.clone())
            .unwrap_or_default();
        let distance = self.train.speed * ticks as f64;
        ahead.travel(g, distance, &mut path);
        let (then, front_then) = ahead.pose(g, self.pivot);
        let (yaw_then, _) = yaw_pitch(front_then, self.heading);
        let seconds = ticks as f64 / 20.0;
        let turn = (yaw_then - yaw + 540.0).rem_euclid(360.0) - 180.0;
        Motion {
            id: self.id,
            pivot,
            velocity: std::array::from_fn(|k| ((then[k] - pivot[k]) / seconds) as f32),
            yaw: yaw as f32,
            yaw_rate: (turn / seconds) as f32,
            pitch: pitch as f32,
            heading: self.heading,
            blocks: full.then(|| self.blocks.clone()),
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

/// A dimension's railway: its track, stations (with their names) and signals as the world has
/// them, the graph built from them (again only when they or the curves change), and its trains.
#[derive(Debug, Default)]
pub struct Railway {
    pub tracks: HashMap<[i32; 3], TrackShape>,
    pub stations: HashMap<[i32; 3], String>,
    pub signals: Vec<[i32; 3]>,
    /// Names saved with a column, waiting for their station block.
    pub saved_names: HashMap<[i32; 3], String>,
    pub trains: Vec<TrainOnTrack>,
    pub net: Network,
    pub occupancy: Occupancy,
    dirty: bool,
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

    pub fn add_signal(&mut self, pos: [i32; 3]) {
        if !self.signals.contains(&pos) {
            self.signals.push(pos);
            self.dirty = true;
        }
    }

    /// A track, station or signal at `pos` is gone.
    pub fn remove(&mut self, pos: [i32; 3]) {
        let before = self.signals.len();
        self.signals.retain(|p| *p != pos);
        if self.tracks.remove(&pos).is_some()
            || self.stations.remove(&pos).is_some()
            || self.signals.len() != before
        {
            self.dirty = true;
        }
    }

    /// A column left: its track, stations and signals leave the graph, its trains memory
    /// (saved with it).
    pub fn column_unloaded(&mut self, cx: i32, cz: i32) {
        let out = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) != (cx, cz);
        let (t, s, g) = (self.tracks.len(), self.stations.len(), self.signals.len());
        self.tracks.retain(|p, _| out(p));
        self.stations.retain(|p, _| out(p));
        self.signals.retain(out);
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
        self.net = Network::build(&self.tracks, curves, &stations, &self.signals);
        self.dirty = false;
        self.curves_seen = curves.revision;
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
        let g = &self.net.graph;
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
        let _ = g;
        match self.trains[i].train.find_path(&self.net, targets, &cx) {
            Some(found) => {
                self.trains[i].train.start(&self.net, found);
                true
            }
            None => false,
        }
    }

    /// A tick of every train: schedules give orders, navigating trains drive, the others coast
    /// to a stop. Nothing is done without trains.
    pub fn tick(&mut self, events: &mut Vec<Event>) {
        events.clear();
        if self.trains.is_empty() {
            return;
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
            let run = t.train.tick(&self.net, &self.occupancy, t.id);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::{Condition, Entry, SECONDS};
    use crate::track_curves::{Curves, Saved};

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
            blocks: Vec::new(),
            pivot: 0,
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
        let (_, heading) = t.pose(&r.net.graph, 0);
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
        r.add_signal([100, 64, 1]);
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
        r.trains[i].blocks = vec![([0, 0, 0], 7), ([0, 1, 0], 9)];
        let m = r.trains[i].motion(&r.net, 2, true);
        assert_eq!((m.yaw, m.yaw_rate, m.pitch), (0.0, 0.0, 0.0));
        assert!((m.pivot[0] + 29.5).abs() < 1e-6 && m.pivot[2] == 0.5);
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
        assert!(m.yaw < 0.0 && m.yaw > -90.0, "{}", m.yaw);
        assert!(m.yaw_rate < 0.0, "{}", m.yaw_rate);
        assert!(r.trains[i].due(&r.net, 1000, &events[0]));
        // East along x is yaw 0; facing south is a quarter turn clockwise.
        assert_eq!(yaw_pitch([0.0, 0.0, 1.0], [1, 0]).0, -90.0);
        let (_, pitch) = yaw_pitch([0.0, 1.0, 1.0], [0, 1]);
        assert!((pitch - 45.0).abs() < 1e-9);
    }
}
