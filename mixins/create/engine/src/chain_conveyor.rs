//! Create's chain conveyor (`ChainConveyorBlock`, `ChainConveyorBlockEntity`,
//! `ChainConveyorPackage`, `ChainConveyorRoutingTable`): a kinetic wheel on the y axis whose
//! chains join other conveyors (set by `/create_debug chain`, standing in for the chain item).
//! Packages loop round a wheel and leave along the chain the routing table names for their
//! address, or into a frogport whose filter takes them. Packages are plain arrays on each
//! conveyor, moved only on conveyors that carry some and turn: a conveyor with nothing on it
//! costs nothing beyond its kinetic part. Routes are worked out again only when a port,
//! connection or conveyor changes (Create's tables gossip every tick to reach the same
//! shortest routes), and addresses are matched only where a package reaches a junction or port.

use crate::ejector::rotate_y;
use crate::package::match_address;
use std::collections::{HashMap, VecDeque};

/// `chainConveyorCapacity`.
pub const CAPACITY: usize = 20;
/// `calculateConnectionStats`' `offBranchDistance`.
const OFF_BRANCH: f32 = 35.0;
/// The wheel's radius in `tick` (`radius`).
const RADIUS: f32 = 1.5;
/// `RoutingTableEntry` distance of a `*` port.
const STAR_DISTANCE: u32 = 1000;

/// A package and its address (`PackageItem.getAddress`), read once when it went on a chain.
#[derive(Clone, Debug, PartialEq)]
pub struct Parcel<P> {
    pub stack: P,
    pub address: String,
}

/// `ChainConveyorPackage`: where it is (degrees round the wheel, or blocks along a chain) and a
/// net id for clients.
#[derive(Clone, Debug, PartialEq)]
pub struct Package<P> {
    pub position: f32,
    pub parcel: Parcel<P>,
    pub id: u32,
    just_flipped: bool,
}

/// `ConnectionStats`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub tangent_angle: f32,
    pub length: f32,
    pub start: [f64; 3],
    pub end: [f64; 3],
}

/// `ConnectedPort`: a frogport's place on this conveyor (round the wheel, or along the chain to
/// `connection`) and its filter.
#[derive(Clone, Debug, PartialEq)]
pub struct PortLink {
    pub chain_pos: f32,
    pub connection: Option<[i32; 3]>,
    pub filter: String,
}

/// One conveyor.
#[derive(Clone, Debug, Default)]
pub struct Conveyor<P> {
    /// To other conveyors, relative.
    pub connections: Vec<[i32; 3]>,
    pub speed: f32,
    pub reversed: bool,
    stats: Vec<([i32; 3], Stats)>,
    pub looping: Vec<Package<P>>,
    /// By connection; an entry stays once made (Create's map), and counts toward the capacity.
    pub travelling: Vec<([i32; 3], Vec<Package<P>>)>,
    /// By the frogport's position relative to this conveyor.
    pub loop_ports: Vec<([i32; 3], PortLink)>,
    pub travel_ports: Vec<([i32; 3], PortLink)>,
    /// The routing table: (filter, distance, next connection; zero stays on the wheel), by
    /// distance.
    routes: Vec<(String, u32, [i32; 3])>,
}

/// What a tick asks of the frogports and the world.
#[derive(Clone, Debug, PartialEq)]
pub enum Event<P> {
    /// A package reached the frogport at this position (`exportToPort` succeeded):
    /// `startAnimation(box, false)`.
    Catch([i32; 3], Parcel<P>),
    /// One is about to (`notifyPortToAnticipate`).
    Anticipate([i32; 3]),
    /// A package fell off where it was (`drop`: a connection or conveyor went).
    Drop([f64; 3], Parcel<P>),
}

/// Catnip's `getShortestAngleDiff`.
pub fn shortest_angle_diff(current: f64, target: f64) -> f32 {
    let (c, t) = (current % 360.0, target % 360.0);
    ((((t - c) % 360.0) + 540.0) % 360.0 - 180.0) as f32
}

/// `wrapAngle`.
pub fn wrap_angle(a: f32) -> f32 {
    let mut a = a % 360.0;
    if a < 0.0 {
        a += 360.0;
    }
    a
}

fn sign(v: f32) -> i32 {
    if v > 0.0 {
        1
    } else if v < 0.0 {
        -1
    } else {
        0
    }
}

/// `Mth.atan2`: vanilla's table-driven arctangent.
pub fn mth_atan2(mut y: f64, mut x: f64) -> f64 {
    let d0 = x * x + y * y;
    if d0.is_nan() {
        return f64::NAN;
    }
    let neg_y = y < 0.0;
    if neg_y {
        y = -y;
    }
    let neg_x = x < 0.0;
    if neg_x {
        x = -x;
    }
    let steep = y > x;
    if steep {
        std::mem::swap(&mut x, &mut y);
    }
    // `fastInvSqrt`.
    let half = 0.5 * d0;
    let mut r =
        f64::from_bits((6_910_469_410_427_058_090i64 - ((d0.to_bits() as i64) >> 1)) as u64);
    r *= 1.5 - half * r * r;
    x *= r;
    y *= r;
    let bias = f64::from_bits(4_805_340_802_404_319_232);
    let d2 = bias + y;
    let i = d2.to_bits() as i32 as usize;
    let asin = (i as f64 / 256.0).asin();
    let cos = asin.cos();
    let d5 = d2 - bias;
    let d6 = y * cos - x * d5;
    let mut theta = asin + (6.0 + d6 * d6) * d6 * 0.166_666_666_666_666_66;
    if steep {
        theta = std::f64::consts::FRAC_PI_2 - theta;
    }
    if neg_x {
        theta = std::f64::consts::PI - theta;
    }
    if neg_y {
        theta = -theta;
    }
    theta
}

/// `Mth.RAD_TO_DEG`.
const RAD_TO_DEG: f32 = 57.295_78;

fn bottom_center(p: [i32; 3]) -> [f64; 3] {
    [
        f64::from(p[0]) + 0.5,
        f64::from(p[1]),
        f64::from(p[2]) + 0.5,
    ]
}

fn add3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `calculateConnectionStats` of the conveyor at `pos` toward `connection`.
pub fn connection_stats(pos: [i32; 3], connection: [i32; 3], reversed: bool) -> Stats {
    let dir = if reversed { -1.0 } else { 1.0 };
    let direction =
        RAD_TO_DEG * mth_atan2(f64::from(connection[0]), f64::from(connection[2])) as f32;
    let angle = wrap_angle(direction - OFF_BRANCH * dir);
    let opposite = wrap_angle(angle + 180.0 + 2.0 * OFF_BRANCH * dir);
    let lift = [0.0, 6.0 / 16.0, 0.0];
    let start = add3(
        add3(
            bottom_center(pos),
            rotate_y([0.0, 0.0, 1.25], f64::from(angle)),
        ),
        lift,
    );
    let other = [
        pos[0] + connection[0],
        pos[1] + connection[1],
        pos[2] + connection[2],
    ];
    let end = add3(
        add3(
            bottom_center(other),
            rotate_y([0.0, 0.0, 1.25], f64::from(opposite)),
        ),
        lift,
    );
    let d = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    Stats {
        tangent_angle: angle,
        length: (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32,
        start,
        end,
    }
}

/// `getChainCost`: chains a connection takes.
pub fn chain_cost(connection: [i32; 3]) -> i32 {
    let l = connection
        .iter()
        .map(|&c| f64::from(c) * f64::from(c))
        .sum::<f64>()
        .sqrt();
    ((l / 2.5).round() as i32).max(1)
}

impl<P> Conveyor<P> {
    fn stats(&self, connection: [i32; 3]) -> Option<&Stats> {
        self.stats
            .iter()
            .find(|(c, _)| *c == connection)
            .map(|(_, s)| s)
    }

    /// `canAcceptMorePackages`: looping packages and travelling entries.
    pub fn can_accept_more(&self) -> bool {
        self.looping.len() + self.travelling.len() < CAPACITY
    }

    /// `canAcceptMorePackagesFromOtherConveyor`.
    pub fn can_accept_from_other(&self) -> bool {
        self.looping.len() < CAPACITY
    }

    pub fn is_empty(&self) -> bool {
        self.looping.is_empty() && self.travelling.iter().all(|(_, l)| l.is_empty())
    }

    /// `loopThresholdCrossed`.
    pub fn crossed(&self, position: f32, previous: f32, angle: f32) -> bool {
        let s1 = sign(shortest_angle_diff(f64::from(angle), f64::from(previous)));
        let s2 = sign(shortest_angle_diff(f64::from(angle), f64::from(position)));
        let not_crossed = s1 >= s2 && !self.reversed || s1 <= s2 && self.reversed;
        !not_crossed
    }

    /// `getExitFor`: the connection of the nearest route whose port matches, zero for none (or
    /// a port on this wheel).
    pub fn exit_for(&self, address: &str) -> [i32; 3] {
        self.routes
            .iter()
            .find(|(f, _, _)| match_address(address, f))
            .map_or([0; 3], |r| r.2)
    }

    /// The routing table, nearest first: (filter, distance, next connection).
    pub fn routes(&self) -> &[(String, u32, [i32; 3])] {
        &self.routes
    }
}

/// Every conveyor of a dimension.
#[derive(Clone, Debug)]
pub struct Chains<P> {
    pub conveyors: HashMap<[i32; 3], Conveyor<P>>,
    /// Conveyors carrying packages (the ones that tick).
    moving: Vec<[i32; 3]>,
    routes_dirty: bool,
    next_id: u32,
    /// Conveyors whose packages or connections changed since `take_changed` (clients redraw them).
    changed: Vec<[i32; 3]>,
}

impl<P> Default for Chains<P> {
    fn default() -> Self {
        Chains {
            conveyors: HashMap::new(),
            moving: Vec::new(),
            routes_dirty: false,
            next_id: 0,
            changed: Vec::new(),
        }
    }
}

fn neg(v: [i32; 3]) -> [i32; 3] {
    [-v[0], -v[1], -v[2]]
}

fn offset(p: [i32; 3], o: [i32; 3]) -> [i32; 3] {
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

impl<P: Clone> Chains<P> {
    /// A conveyor placed or loaded (`connections` relative, as saved).
    pub fn add(&mut self, pos: [i32; 3], connections: Vec<[i32; 3]>) {
        let c = self.conveyors.entry(pos).or_insert_with(|| Conveyor {
            connections: Vec::new(),
            speed: 0.0,
            reversed: false,
            stats: Vec::new(),
            looping: Vec::new(),
            travelling: Vec::new(),
            loop_ports: Vec::new(),
            travel_ports: Vec::new(),
            routes: Vec::new(),
        });
        for k in connections {
            if !c.connections.contains(&k) {
                c.connections.push(k);
            }
        }
        Self::prepare(pos, c);
        self.routes_dirty = true;
        self.changed.push(pos);
    }

    /// Packages back on a loaded conveyor (`read`).
    pub fn load_packages(
        &mut self,
        pos: [i32; 3],
        looping: Vec<(f32, Parcel<P>)>,
        travelling: Vec<([i32; 3], Vec<(f32, Parcel<P>)>)>,
    ) {
        let mut ids = || {
            self.next_id = self.next_id.wrapping_add(1);
            self.next_id
        };
        let mut make = |(position, parcel): (f32, Parcel<P>)| Package {
            position,
            parcel,
            id: ids(),
            just_flipped: false,
        };
        let looping: Vec<_> = looping.into_iter().map(&mut make).collect();
        let travelling: Vec<_> = travelling
            .into_iter()
            .map(|(k, l)| (k, l.into_iter().map(&mut make).collect::<Vec<_>>()))
            .collect();
        let Some(c) = self.conveyors.get_mut(&pos) else {
            return;
        };
        c.looping.extend(looping);
        for (k, l) in travelling {
            match c.travelling.iter_mut().find(|(t, _)| *t == k) {
                Some((_, v)) => v.extend(l),
                None => c.travelling.push((k, l)),
            }
        }
        self.track(pos);
        self.changed.push(pos);
    }

    /// The conveyor is gone (`destroy`): its packages drop, the conveyors it joined forget it
    /// (`removeConnectionTo`, dropping what travelled toward it).
    pub fn remove(&mut self, pos: [i32; 3], drops: &mut Vec<Event<P>>) {
        let Some(c) = self.conveyors.remove(&pos) else {
            return;
        };
        for p in &c.looping {
            drops.push(Event::Drop(
                package_position(pos, &c, p.position, None),
                p.parcel.clone(),
            ));
        }
        for (k, l) in &c.travelling {
            for p in l {
                drops.push(Event::Drop(
                    package_position(pos, &c, p.position, Some(*k)),
                    p.parcel.clone(),
                ));
            }
        }
        for k in &c.connections {
            self.disconnect_one(offset(pos, *k), pos, drops);
        }
        self.moving.retain(|p| *p != pos);
        self.routes_dirty = true;
    }

    /// The conveyor is unloaded with its column: forgotten, its neighbours untouched.
    pub fn unload(&mut self, pos: [i32; 3]) {
        if self.conveyors.remove(&pos).is_some() {
            self.moving.retain(|p| *p != pos);
            self.routes_dirty = true;
        }
    }

    /// `ChainConveyorConnectionPacket` / `addConnectionTo` on both: a chain between `a` and `b`.
    /// False when either is not a conveyor or they are the same.
    pub fn connect(&mut self, a: [i32; 3], b: [i32; 3]) -> bool {
        if a == b || !self.conveyors.contains_key(&a) || !self.conveyors.contains_key(&b) {
            return false;
        }
        for (from, to) in [(a, b), (b, a)] {
            let k = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
            let c = self.conveyors.get_mut(&from).unwrap();
            if !c.connections.contains(&k) {
                c.connections.push(k);
                Self::prepare(from, c);
            }
            self.changed.push(from);
        }
        self.routes_dirty = true;
        true
    }

    /// The chain between `a` and `b` taken away (`removeConnectionTo` on both).
    pub fn disconnect(&mut self, a: [i32; 3], b: [i32; 3], drops: &mut Vec<Event<P>>) {
        self.disconnect_one(a, b, drops);
        self.disconnect_one(b, a, drops);
    }

    fn disconnect_one(&mut self, at: [i32; 3], other: [i32; 3], drops: &mut Vec<Event<P>>) {
        let Some(c) = self.conveyors.get_mut(&at) else {
            return;
        };
        let k = [other[0] - at[0], other[1] - at[1], other[2] - at[2]];
        if !c.connections.contains(&k) {
            return;
        }
        if let Some(i) = c.travelling.iter().position(|(t, _)| *t == k) {
            let (_, list) = c.travelling.remove(i);
            for p in list {
                drops.push(Event::Drop(
                    package_position(at, c, p.position, Some(k)),
                    p.parcel,
                ));
            }
        }
        c.connections.retain(|x| *x != k);
        c.stats.retain(|(x, _)| *x != k);
        self.routes_dirty = true;
        self.changed.push(at);
    }

    /// `prepareStats` with the stats always fresh.
    fn prepare(pos: [i32; 3], c: &mut Conveyor<P>) {
        c.stats = c
            .connections
            .iter()
            .map(|&k| (k, connection_stats(pos, k, c.reversed)))
            .collect();
    }

    /// The conveyor's speed from the kinetic network. A change of direction flips what travels
    /// to the other conveyor's side (`reversedPreviously != reversed`).
    pub fn set_speed(&mut self, pos: [i32; 3], speed: f32) {
        let Some(c) = self.conveyors.get_mut(&pos) else {
            return;
        };
        c.speed = speed;
        let reversed = speed < 0.0;
        if speed != 0.0 && reversed != c.reversed {
            c.reversed = reversed;
            Self::prepare(pos, c);
            // Packages travelling toward another conveyor now travel from it.
            let mut moved = Vec::new();
            for (k, list) in c.travelling.iter_mut() {
                let length = (k
                    .iter()
                    .map(|&v| f64::from(v) * f64::from(v))
                    .sum::<f64>()
                    .sqrt()) as f32
                    - 22.0 / 16.0;
                list.retain_mut(|b| {
                    if b.just_flipped {
                        return true;
                    }
                    b.just_flipped = true;
                    b.position = length - b.position;
                    moved.push((*k, b.clone()));
                    false
                });
            }
            for (k, b) in moved {
                let other = offset(pos, k);
                if let Some(o) = self.conveyors.get_mut(&other)
                    && o.connections.contains(&neg(k))
                {
                    push_travelling(o, neg(k), b);
                    self.changed.push(other);
                    if !self.moving.contains(&other) {
                        self.moving.push(other);
                    }
                }
            }
        }
        self.changed.push(pos);
        self.track(pos);
    }

    fn track(&mut self, pos: [i32; 3]) {
        let has = self
            .conveyors
            .get(&pos)
            .is_some_and(|c| !c.is_empty() && c.speed != 0.0);
        if has && !self.moving.contains(&pos) {
            self.moving.push(pos);
        }
    }

    /// `ChainConveyorFrogportTarget.register`: the frogport at `port` with `filter` on the
    /// conveyor at `conveyor`, round its wheel (`connection` None) or along a chain.
    pub fn register_port(
        &mut self,
        port: [i32; 3],
        conveyor: [i32; 3],
        chain_pos: f32,
        connection: Option<[i32; 3]>,
        filter: String,
    ) {
        let Some(c) = self.conveyors.get_mut(&conveyor) else {
            return;
        };
        if connection.is_some_and(|k| !c.connections.contains(&k)) {
            return;
        }
        let rel = [
            port[0] - conveyor[0],
            port[1] - conveyor[1],
            port[2] - conveyor[2],
        ];
        let link = PortLink {
            chain_pos,
            connection,
            filter,
        };
        let list = if connection.is_none() {
            &mut c.loop_ports
        } else {
            &mut c.travel_ports
        };
        match list.iter_mut().find(|(p, _)| *p == rel) {
            Some((_, l)) if *l == link => return,
            Some((_, l)) => *l = link,
            None => list.push((rel, link)),
        }
        self.routes_dirty = true;
    }

    /// `deregister`.
    pub fn deregister_port(&mut self, port: [i32; 3], conveyor: [i32; 3]) {
        let Some(c) = self.conveyors.get_mut(&conveyor) else {
            return;
        };
        let rel = [
            port[0] - conveyor[0],
            port[1] - conveyor[1],
            port[2] - conveyor[2],
        ];
        let n = c.loop_ports.len() + c.travel_ports.len();
        c.loop_ports.retain(|(p, _)| *p != rel);
        c.travel_ports.retain(|(p, _)| *p != rel);
        if c.loop_ports.len() + c.travel_ports.len() != n {
            self.routes_dirty = true;
        }
    }

    /// `export(simulate)` of a frogport's target: the conveyor turns and has room.
    pub fn can_accept(&self, conveyor: [i32; 3], connection: Option<[i32; 3]>) -> bool {
        let Some(c) = self.conveyors.get(&conveyor) else {
            return false;
        };
        if connection.is_some_and(|k| !c.connections.contains(&k)) {
            return false;
        }
        if c.speed == 0.0 {
            return false;
        }
        match connection {
            None => c.can_accept_more(),
            Some(k) => self
                .conveyors
                .get(&offset(conveyor, k))
                .is_some_and(Conveyor::can_accept_more),
        }
    }

    /// `export`: the package goes onto the conveyor where the frogport is (`addLoopingPackage`,
    /// `addTravellingPackage`). Gives it back when the target is not there.
    pub fn insert(
        &mut self,
        conveyor: [i32; 3],
        chain_pos: f32,
        connection: Option<[i32; 3]>,
        parcel: Parcel<P>,
    ) -> Result<(), Parcel<P>> {
        let id = self.next_id.wrapping_add(1);
        let Some(c) = self.conveyors.get_mut(&conveyor) else {
            return Err(parcel);
        };
        let package = Package {
            position: chain_pos,
            parcel,
            id,
            just_flipped: false,
        };
        match connection {
            None => c.looping.push(package),
            Some(k) if c.connections.contains(&k) => push_travelling(c, k, package),
            Some(_) => return Err(package.parcel),
        }
        self.next_id = id;
        self.changed.push(conveyor);
        self.track(conveyor);
        Ok(())
    }

    /// The conveyors whose packages or chains changed since the last call.
    pub fn take_changed(&mut self) -> Vec<[i32; 3]> {
        let mut v = std::mem::take(&mut self.changed);
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Whether anything moves (a tick costs nothing otherwise).
    pub fn is_idle(&self) -> bool {
        self.moving.is_empty() && !self.routes_dirty
    }

    /// The routing tables again (`ChainConveyorRoutingTable`'s steady state): every port's filter
    /// spread from its conveyor to the others along the chains, a hop at a time, each conveyor
    /// keeping the nearest route for each filter (the first found on a tie); a `*` port starts
    /// at 1000.
    fn route(&mut self) {
        self.routes_dirty = false;
        let mut sources: Vec<([i32; 3], String, u32, [i32; 3])> = Vec::new();
        let mut keys: Vec<&[i32; 3]> = self.conveyors.keys().collect();
        keys.sort_unstable();
        for &pos in &keys {
            let c = &self.conveyors[pos];
            for (_, l) in c.loop_ports.iter().chain(&c.travel_ports) {
                let d = if l.filter == "*" { STAR_DISTANCE } else { 0 };
                sources.push((*pos, l.filter.clone(), d, l.connection.unwrap_or([0; 3])));
            }
        }
        let positions: Vec<[i32; 3]> = keys.into_iter().copied().collect();
        let mut tables: HashMap<[i32; 3], Vec<(String, u32, [i32; 3])>> =
            positions.iter().map(|p| (*p, Vec::new())).collect();
        let mut filters: Vec<String> = Vec::new();
        for s in &sources {
            if !filters.contains(&s.1) {
                filters.push(s.1.clone());
            }
        }
        for filter in &filters {
            // Breadth first from every conveyor holding a port with this filter.
            let mut best: HashMap<[i32; 3], (u32, [i32; 3])> = HashMap::new();
            let mut queue: VecDeque<[i32; 3]> = VecDeque::new();
            let mut starts: Vec<&([i32; 3], String, u32, [i32; 3])> =
                sources.iter().filter(|s| &s.1 == filter).collect();
            starts.sort_by_key(|s| s.2);
            for s in starts {
                if !best.contains_key(&s.0) {
                    best.insert(s.0, (s.2, s.3));
                    queue.push_back(s.0);
                }
            }
            while let Some(at) = queue.pop_front() {
                let (d, _) = best[&at];
                for k in self.conveyors[&at].connections.clone() {
                    let n = offset(at, k);
                    let Some(other) = self.conveyors.get(&n) else {
                        continue;
                    };
                    if !other.connections.contains(&neg(k)) || best.contains_key(&n) {
                        continue;
                    }
                    best.insert(n, (d + 1, neg(k)));
                    queue.push_back(n);
                }
            }
            for (p, (d, next)) in best {
                tables.get_mut(&p).unwrap().push((filter.clone(), d, next));
            }
        }
        for (p, mut t) in tables {
            // Nearest first; filters in the order their ports were found on a tie.
            t.sort_by_key(|e| (e.1, filters.iter().position(|f| *f == e.0)));
            if let Some(c) = self.conveyors.get_mut(&p) {
                c.routes = t;
            }
        }
    }

    /// One tick of every conveyor carrying packages (`ChainConveyorBlockEntity.tick`, the
    /// server's part). `accepts(port)` is `exportToPort`'s test: a frogport there, not
    /// animating, not backed up. Caught packages leave the chain.
    pub fn tick(&mut self, mut accepts: impl FnMut([i32; 3]) -> bool, events: &mut Vec<Event<P>>) {
        if self.routes_dirty {
            self.route();
        }
        if self.moving.is_empty() {
            return;
        }
        let moving = self.moving.clone();
        for pos in moving {
            self.tick_one(pos, &mut accepts, events);
        }
        let conveyors = &self.conveyors;
        self.moving.retain(|p| {
            conveyors
                .get(p)
                .is_some_and(|c| !c.is_empty() && c.speed != 0.0)
        });
    }

    fn tick_one(
        &mut self,
        pos: [i32; 3],
        accepts: &mut impl FnMut([i32; 3]) -> bool,
        events: &mut Vec<Event<P>>,
    ) {
        let Some(c) = self.conveyors.get(&pos) else {
            return;
        };
        let speed = c.speed / 360.0;
        if speed == 0.0 {
            return;
        }
        let distance = speed.abs();
        let degrees = (speed / (std::f32::consts::PI * RADIUS)) * 360.0;
        // Whether each neighbour takes packages from another conveyor, as it stands now.
        let room: Vec<([i32; 3], bool)> = c
            .connections
            .iter()
            .map(|&k| {
                (
                    k,
                    self.conveyors
                        .get(&offset(pos, k))
                        .is_none_or(Conveyor::can_accept_from_other),
                )
            })
            .collect();
        let mut handoff: Vec<([i32; 3], Package<P>)> = Vec::new();
        let mut changed = false;
        let c = self.conveyors.get_mut(&pos).unwrap();
        let reversed = c.reversed;
        let mut travelling = std::mem::take(&mut c.travelling);
        for (target, list) in travelling.iter_mut() {
            let Some(stats) = c.stats(*target).copied() else {
                continue;
            };
            let mut i = 0;
            'travelling: while i < list.len() {
                let b = &mut list[i];
                b.just_flipped = false;
                let prev = b.position;
                b.position = (b.position + distance).min(stats.length);
                let anticipate = (b.position + distance * 4.0).min(stats.length);
                for (port, link) in &c.travel_ports {
                    if prev > link.chain_pos || link.connection != Some(*target) {
                        continue;
                    }
                    let not_yet = b.position < link.chain_pos;
                    if not_yet && anticipate < link.chain_pos {
                        continue;
                    }
                    if !match_address(&b.parcel.address, &link.filter) {
                        continue;
                    }
                    let at = offset(pos, *port);
                    if not_yet {
                        events.push(Event::Anticipate(at));
                        continue;
                    }
                    if !accepts(at) {
                        continue;
                    }
                    let b = list.remove(i);
                    events.push(Event::Catch(at, b.parcel));
                    changed = true;
                    continue 'travelling;
                }
                if list[i].position < stats.length {
                    i += 1;
                    continue;
                }
                // On to the other conveyor's wheel.
                let mut b = list.remove(i);
                b.position = wrap_angle(
                    stats.tangent_angle
                        + 180.0
                        + 2.0 * OFF_BRANCH * if reversed { -1.0 } else { 1.0 },
                );
                handoff.push((*target, b));
                changed = true;
            }
        }
        c.travelling = travelling;
        let mut looping = std::mem::take(&mut c.looping);
        let mut i = 0;
        'looping: while i < looping.len() {
            let b = &mut looping[i];
            b.just_flipped = false;
            let prev = b.position;
            b.position = wrap_angle(b.position + degrees);
            let anticipate = wrap_angle(b.position + degrees * 4.0);
            for (port, link) in &c.loop_ports {
                let not_yet = !c.crossed(b.position, prev, link.chain_pos);
                if not_yet && !c.crossed(anticipate, prev, link.chain_pos) {
                    continue;
                }
                if !match_address(&b.parcel.address, &link.filter) {
                    continue;
                }
                let at = offset(pos, *port);
                if not_yet {
                    events.push(Event::Anticipate(at));
                    continue;
                }
                if !accepts(at) {
                    continue;
                }
                let b = looping.remove(i);
                events.push(Event::Catch(at, b.parcel));
                changed = true;
                continue 'looping;
            }
            for &(k, takes) in &room {
                if !takes {
                    continue;
                }
                let Some(stats) = c.stats(k) else { continue };
                if !c.crossed(looping[i].position, prev, stats.tangent_angle) {
                    continue;
                }
                if c.exit_for(&looping[i].parcel.address) != k {
                    continue;
                }
                let mut b = looping.remove(i);
                b.position = 0.0;
                push_travelling(c, k, b);
                changed = true;
                continue 'looping;
            }
            i += 1;
        }
        c.looping = looping;
        if changed {
            self.changed.push(pos);
        }
        for (k, b) in handoff {
            let other = offset(pos, k);
            if let Some(o) = self.conveyors.get_mut(&other) {
                o.looping.push(b);
                self.changed.push(other);
                if o.speed != 0.0 && !self.moving.contains(&other) {
                    self.moving.push(other);
                }
            }
        }
    }

    /// Where a package is in the world (`getPackagePosition`).
    pub fn position_of(
        &self,
        pos: [i32; 3],
        position: f32,
        connection: Option<[i32; 3]>,
    ) -> [f64; 3] {
        match self.conveyors.get(&pos) {
            Some(c) => package_position(pos, c, position, connection),
            None => bottom_center(pos),
        }
    }
}

fn push_travelling<P>(c: &mut Conveyor<P>, k: [i32; 3], b: Package<P>) {
    match c.travelling.iter_mut().find(|(t, _)| *t == k) {
        Some((_, l)) => l.push(b),
        None => c.travelling.push((k, vec![b])),
    }
}

/// `getPackagePosition`: round the wheel 6 px up and 14 out, or along the chain.
pub fn package_position<P>(
    pos: [i32; 3],
    c: &Conveyor<P>,
    position: f32,
    connection: Option<[i32; 3]>,
) -> [f64; 3] {
    match connection {
        None => add3(
            bottom_center(pos),
            rotate_y([0.0, 6.0 / 16.0, 0.875], f64::from(position)),
        ),
        Some(k) => match c.stats(k) {
            Some(s) => {
                let d = [
                    s.end[0] - s.start[0],
                    s.end[1] - s.start[1],
                    s.end[2] - s.start[2],
                ];
                let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let t = f64::from(s.length.min(position)) / l;
                [
                    s.start[0] + d[0] * t,
                    s.start[1] + d[1] * t,
                    s.start[2] + d[2] * t,
                ]
            }
            None => [0.0; 3],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parcel(address: &str) -> Parcel<&'static str> {
        Parcel {
            stack: "box",
            address: address.into(),
        }
    }

    #[test]
    fn atan2_follows_the_table() {
        for (y, x) in [
            (1.0, 0.0),
            (0.0, 1.0),
            (3.0, 4.0),
            (-2.0, 5.0),
            (-1.0, -1.0),
        ] {
            assert!((mth_atan2(y, x) - f64::atan2(y, x)).abs() < 1e-2, "{y} {x}");
        }
    }

    #[test]
    fn stats_of_a_chain_east() {
        // Toward +x: atan2(x=4, z=0) = 90°, the chain leaves 35° before it.
        let s = connection_stats([0, 0, 0], [4, 0, 0], false);
        assert!((s.tangent_angle - 55.0).abs() < 1e-3);
        assert!((s.start[1] - 0.375).abs() < 1e-9);
        // Both ends 1.25 out from their wheels on the same side, 35° short of straight across:
        // 4 - 2.5 sin 55° along.
        let expect = 4.0 - 2.5 * 55f32.to_radians().sin();
        assert!((s.length - expect).abs() < 0.01, "{}", s.length);
        assert_eq!(chain_cost([4, 0, 0]), 2);
        assert_eq!(chain_cost([1, 0, 0]), 1);
    }

    #[test]
    fn crossing_a_threshold_depends_on_the_direction() {
        let mut c = Conveyor::<()>::default();
        assert!(c.crossed(10.0, 350.0, 0.0));
        assert!(!c.crossed(350.0, 340.0, 0.0));
        c.reversed = true;
        assert!(c.crossed(350.0, 10.0, 0.0));
        assert!(!c.crossed(10.0, 350.0, 0.0));
    }

    /// Three conveyors in a row east, 4 apart: a, b, c.
    fn line() -> Chains<&'static str> {
        let mut ch = Chains::default();
        for x in [0, 4, 8] {
            ch.add([x, 0, 0], vec![]);
            ch.set_speed([x, 0, 0], 32.0);
        }
        assert!(ch.connect([0, 0, 0], [4, 0, 0]));
        assert!(ch.connect([4, 0, 0], [8, 0, 0]));
        ch
    }

    #[test]
    fn routes_lead_toward_the_nearest_matching_port() {
        let mut ch = line();
        ch.register_port([9, 0, 0], [8, 0, 0], 90.0, None, "Mine".into());
        ch.register_port([1, 0, 0], [0, 0, 0], 90.0, None, "*".into());
        let mut ev = Vec::new();
        ch.tick(|_| true, &mut ev);
        let a = &ch.conveyors[&[0, 0, 0]];
        assert_eq!(a.exit_for("Mine"), [4, 0, 0]);
        assert_eq!(
            a.exit_for("Farm"),
            [0, 0, 0],
            "the `*` port is on this wheel"
        );
        let b = &ch.conveyors[&[4, 0, 0]];
        assert_eq!(b.exit_for("Mine"), [4, 0, 0]);
        assert_eq!(b.exit_for("Farm"), [-4, 0, 0]);
        assert_eq!(b.routes()[0], ("Mine".to_string(), 1, [4, 0, 0]));
        assert_eq!(b.routes()[1], ("*".to_string(), 1001, [-4, 0, 0]));
        // A glob port matches too.
        ch.register_port([9, 0, 0], [8, 0, 0], 90.0, None, "M*".into());
        ch.tick(|_| true, &mut ev);
        assert_eq!(ch.conveyors[&[0, 0, 0]].exit_for("Mill"), [4, 0, 0]);
        // Gone: no route.
        ch.deregister_port([9, 0, 0], [8, 0, 0]);
        ch.tick(|_| true, &mut ev);
        assert_eq!(ch.conveyors[&[4, 0, 0]].exit_for("Mill"), [-4, 0, 0]);
    }

    #[test]
    fn a_package_rides_to_the_frogport_of_its_address() {
        let mut ch = line();
        // The frogport at the far end takes "Mine", round its wheel at 180°.
        ch.register_port([8, 0, -2], [8, 0, 0], 180.0, None, "Mine".into());
        assert!(ch.can_accept([0, 0, 0], None));
        ch.insert([0, 0, 0], 0.0, None, parcel("Mine")).unwrap();
        let mut caught = None;
        let mut anticipated = false;
        for t in 0..2000 {
            let mut ev = Vec::new();
            ch.tick(|_| true, &mut ev);
            for e in ev {
                match e {
                    Event::Catch(at, p) => caught = Some((t, at, p)),
                    Event::Anticipate(_) => anticipated = true,
                    Event::Drop(..) => panic!("dropped"),
                }
            }
            if caught.is_some() {
                break;
            }
        }
        let (t, at, p) = caught.expect("caught");
        assert_eq!(at, [8, 0, -2]);
        assert_eq!(p.address, "Mine");
        assert!(anticipated, "the frogport heard it coming");
        assert!(t > 20, "{t}");
        assert!(ch.conveyors.values().all(Conveyor::is_empty));
        assert!(ch.is_idle(), "nothing ticks once it is caught");
    }

    #[test]
    fn a_package_with_no_route_keeps_looping_and_a_busy_port_lets_it_pass() {
        let mut ch = line();
        ch.register_port([8, 0, -2], [8, 0, 0], 180.0, None, "Mine".into());
        ch.insert([0, 0, 0], 0.0, None, parcel("Farm")).unwrap();
        let mut ev = Vec::new();
        for _ in 0..500 {
            ch.tick(|_| true, &mut ev);
        }
        assert!(ev.is_empty(), "nothing on the way matches");
        assert_eq!(
            ch.conveyors[&[0, 0, 0]].looping.len(),
            1,
            "still on its wheel"
        );
        // The matching package passes a frogport that is busy, then loops back to it.
        ch.insert([8, 0, 0], 170.0, None, parcel("Mine")).unwrap();
        let mut caught = Vec::new();
        for t in 0..500 {
            let mut ev = Vec::new();
            ch.tick(|_| t >= 3, &mut ev);
            if ev.iter().any(|e| matches!(e, Event::Catch(..))) {
                caught.push(t);
            }
        }
        assert_eq!(caught.len(), 1);
        assert!(caught[0] > 40, "a lap later: {caught:?}");
    }

    #[test]
    fn capacity_and_stopped_conveyors() {
        let mut ch = line();
        for _ in 0..CAPACITY {
            assert!(ch.can_accept([0, 0, 0], None));
            ch.insert([0, 0, 0], 0.0, None, parcel("x")).unwrap();
        }
        assert!(!ch.can_accept([0, 0, 0], None));
        ch.set_speed([4, 0, 0], 0.0);
        assert!(
            !ch.can_accept([4, 0, 0], None),
            "a still conveyor takes nothing"
        );
        // A removed conveyor drops what it carried.
        let mut drops = Vec::new();
        ch.remove([0, 0, 0], &mut drops);
        assert_eq!(drops.len(), CAPACITY);
        assert!(ch.conveyors[&[4, 0, 0]].connections == vec![[4, 0, 0]]);
    }

    #[test]
    fn reversing_flips_travelling_packages_to_the_other_side() {
        let mut ch = line();
        ch.insert([0, 0, 0], 1.0, Some([4, 0, 0]), parcel("x"))
            .unwrap();
        ch.set_speed([0, 0, 0], -32.0);
        assert!(ch.conveyors[&[0, 0, 0]].is_empty());
        let b = &ch.conveyors[&[4, 0, 0]].travelling[0];
        assert_eq!(b.0, [-4, 0, 0]);
        assert!((b.1[0].position - (4.0 - 22.0 / 16.0 - 1.0)).abs() < 1e-5);
    }
}
