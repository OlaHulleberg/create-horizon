//! Rotation networks, after Create's `RotationPropagator`, `KineticNetwork` and `KineticBlockEntity`.
//!
//! Create propagates speed block entity by block entity, from source to source, each tick a change
//! happens. Here a world's kinetic parts are nodes in a table, and a change marks its connected
//! component dirty. `solve` re-solves only dirty components, at most once per call, which the game
//! makes once per tick at most. A steady factory costs nothing per tick.
//!
//! Solving a component keeps Create's rules:
//! - two parts are connected with a speed ratio (`ratio`, Create's `getRotationSpeedModifier`):
//!   facing shafts carry the speed (through a gearbox's or split shaft's face signs), small cogs
//!   beside each other reverse it, a large cog turns a small one diagonally at -2 (and back at -0.5),
//!   and two large cogs on crossed axes at ±1, encased chain drives along their chain at the
//!   adjustable gearshift's modifier;
//! - a speed controller sets the large cog above it to its target, and a sequenced gearshift its
//!   far half to the running instruction's modifier, from whichever side turns (`Couple`);
//! - the generator whose speed reaches the network fastest drives it; one turning the other way, or a
//!   loop whose ratios disagree, breaks the newest part (Create destroys the block that caused it), and
//!   so does any part past the maximum speed (256 RPM);
//! - stress is the sum of each part's impact times its speed, capacity the sum of each generator's
//!   capacity times its own generated speed; a network whose stress exceeds its capacity stops.

use std::collections::{HashMap, HashSet};

/// Create's `maxRotationSpeed` (CKinetics, default 256).
pub const MAX_SPEED: f32 = 256.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub fn of(v: [i32; 3], axis: Axis) -> i32 {
        match axis {
            Axis::X => v[0],
            Axis::Y => v[1],
            Axis::Z => v[2],
        }
    }
}

/// Vanilla's `Direction`, in its data order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Dir {
    pub const ALL: [Dir; 6] = [
        Dir::Down,
        Dir::Up,
        Dir::North,
        Dir::South,
        Dir::West,
        Dir::East,
    ];

    pub fn offset(self) -> [i32; 3] {
        match self {
            Dir::Down => [0, -1, 0],
            Dir::Up => [0, 1, 0],
            Dir::North => [0, 0, -1],
            Dir::South => [0, 0, 1],
            Dir::West => [-1, 0, 0],
            Dir::East => [1, 0, 0],
        }
    }
    pub fn axis(self) -> Axis {
        match self {
            Dir::Down | Dir::Up => Axis::Y,
            Dir::North | Dir::South => Axis::Z,
            Dir::West | Dir::East => Axis::X,
        }
    }
    pub fn positive(self) -> bool {
        matches!(self, Dir::Up | Dir::South | Dir::East)
    }
    pub fn opposite(self) -> Dir {
        match self {
            Dir::Down => Dir::Up,
            Dir::Up => Dir::Down,
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::West => Dir::East,
            Dir::East => Dir::West,
        }
    }
    pub fn bit(self) -> u8 {
        1 << self as u8
    }
    pub fn from_offset(o: [i32; 3]) -> Option<Dir> {
        Dir::ALL.into_iter().find(|d| d.offset() == o)
    }
    pub fn from_axis(axis: Axis, positive: bool) -> Dir {
        match (axis, positive) {
            (Axis::X, true) => Dir::East,
            (Axis::X, false) => Dir::West,
            (Axis::Y, true) => Dir::Up,
            (Axis::Y, false) => Dir::Down,
            (Axis::Z, true) => Dir::South,
            (Axis::Z, false) => Dir::North,
        }
    }
}

/// Create's `KineticBlockEntity.convertToDirection`: a setting (a motor's RPM) as speed along the
/// axis of the face it turns, negated for a face on the axis' negative side.
pub fn convert_to_direction(speed: f32, facing: Dir) -> f32 {
    if facing.positive() { speed } else { -speed }
}

/// Create's `convertToAngular`: RPM as degrees per tick.
pub fn convert_to_angular(rpm: f32) -> f32 {
    rpm * 360.0 / 60.0 / 20.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cog {
    None,
    Small,
    Large,
}

/// One kinetic part as the solver sees it: what it connects to and what it adds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    /// The rotation axis.
    pub axis: Axis,
    /// Faces with a shaft end (`Dir::bit`), Create's `hasShaftTowards`.
    pub shafts: u8,
    /// The speed a face's shaft turns at, relative to the part's own: -1 on a gearbox's faces that
    /// turn the other way. 1 elsewhere.
    pub face_sign: [i8; 6],
    pub cog: Cog,
    /// RPM this part generates, signed along its axis (0 for none).
    pub generated: f32,
    /// Stress capacity per RPM of generated speed (SU).
    pub capacity: f32,
    /// Stress impact per RPM (SU).
    pub impact: f32,
    /// A split shaft's second half: `Some(link)` joins this node to the other node at its position
    /// (index 0) at ratio `link` (1, -1 for a gearshift that reverses, 0 for a clutch that cuts).
    pub link: Option<f32>,
    /// A belt segment's controller (its chain's start): segments of one belt turn together
    /// (`BeltBlockEntity.propagateRotationTo`).
    pub belt: Option<[i32; 3]>,
    /// An encased chain drive's chain to its neighbours (`ChainDriveBlock`).
    pub chain: Option<Chain>,
    /// A speed set across to another part instead of a ratio: a rotation speed controller's, or a
    /// sequenced gearshift's second half.
    pub couple: Option<Couple>,
}

/// The chain of an encased chain drive or adjustable chain gearshift: the axis it runs along
/// (`getConnectionAxis`), whether it goes on toward the positive (`START`, `MIDDLE`) and the
/// negative (`MIDDLE`, `END`) side, and the speed it gives the chain
/// (`ChainGearshiftBlockEntity.getModifier`, 1 for a plain drive).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chain {
    pub along: Axis,
    pub positive: bool,
    pub negative: bool,
    pub modifier: f32,
}

/// A part that sets another's speed, whichever turns first driving the other
/// (`RotationPropagator.getConveyedSpeed`, `SplitShaftBlockEntity.getRotationSpeedModifier`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Couple {
    /// A rotation speed controller's target (RPM): the large cog on a horizontal axis across it,
    /// above it, turns at it while the controller turns, and the controller at it while only
    /// the cog does (`SpeedControllerBlockEntity.getDesiredOutputSpeed`).
    Controller(f32),
    /// On a split shaft's second half: the half away from the one turned turns at this times
    /// its speed (a sequenced gearshift's instruction modifier; 0 cuts).
    Relay(f32),
}

impl Part {
    /// A part on `axis` with shafts at both ends of it.
    pub fn shaft(axis: Axis) -> Part {
        Part {
            axis,
            shafts: Dir::from_axis(axis, true).bit() | Dir::from_axis(axis, false).bit(),
            face_sign: [1; 6],
            cog: Cog::None,
            generated: 0.0,
            capacity: 0.0,
            impact: 0.0,
            link: None,
            belt: None,
            chain: None,
            couple: None,
        }
    }
    pub fn cog(axis: Axis, large: bool) -> Part {
        Part {
            cog: if large { Cog::Large } else { Cog::Small },
            ..Part::shaft(axis)
        }
    }
    /// Create's gearbox: shafts on the four faces around `axis`, two of them turning the other way
    /// (`RotationPropagator.getAxisModifier` for `GearboxBlockEntity`, which is the same for any
    /// source face up to the network's sign).
    pub fn gearbox(axis: Axis) -> Part {
        let first = match axis {
            Axis::X => Axis::Y,
            _ => Axis::X,
        };
        let mut face_sign = [1i8; 6];
        let mut shafts = 0;
        for d in Dir::ALL {
            if d.axis() == axis {
                continue;
            }
            shafts |= d.bit();
            let along = if d.positive() { 1 } else { -1 };
            face_sign[d as usize] = if d.axis() == first { along } else { -along };
        }
        Part {
            shafts,
            face_sign,
            ..Part::shaft(axis)
        }
    }
    /// A shaft end on one face only (a motor's output, one half of a split shaft).
    pub fn half(face: Dir) -> Part {
        Part {
            shafts: face.bit(),
            ..Part::shaft(face.axis())
        }
    }
}

/// Create's `getRotationSpeedModifier(from, to)` for two parts, `diff` = to - from.
pub fn ratio(a: &Part, b: &Part, diff: [i32; 3]) -> f32 {
    // BeltBlockEntity.propagateRotationTo: a belt's segments, side by side, unless their
    // shafts already meet.
    if let Some(d) = Dir::from_offset(diff)
        && a.belt.is_some()
        && a.belt == b.belt
        && !(a.shafts & d.bit() != 0 && b.shafts & d.opposite().bit() != 0)
    {
        return 1.0;
    }
    if let Some(d) = Dir::from_offset(diff)
        && a.shafts & d.bit() != 0
        && b.shafts & d.opposite().bit() != 0
    {
        return a.face_sign[d as usize] as f32 / b.face_sign[d.opposite() as usize] as f32;
    }
    // Attached encased chain drives: ChainDriveBlock.areBlocksConnected, at
    // `ChainDriveBlock.getRotationSpeedModifier` (from's modifier over to's).
    if let (Some(d), Some(x), Some(y)) = (Dir::from_offset(diff), a.chain, b.chain) {
        let connected = x.along == y.along
            && d.axis() == x.along
            && if d.positive() { x.positive } else { x.negative };
        return if connected {
            x.modifier / y.modifier
        } else {
            0.0
        };
    }
    let axes = [Axis::X, Axis::Y, Axis::Z];
    // Large <-> large on crossed axes, diagonally.
    if a.cog == Cog::Large && b.cog == Cog::Large && a.axis != b.axis {
        let fits = axes.iter().all(|&x| {
            let c = Axis::of(diff, x);
            if x == a.axis || x == b.axis {
                c != 0
            } else {
                c == 0
            }
        });
        if fits {
            let s = Axis::of(diff, a.axis) > 0;
            let t = Axis::of(diff, b.axis) > 0;
            return if s ^ t { -1.0 } else { 1.0 };
        }
    }
    let large_to_small = |l: &Part, s: &Part| {
        l.axis == s.axis
            && Axis::of(diff, l.axis) == 0
            && axes
                .iter()
                .filter(|&&x| x != l.axis)
                .all(|&x| Axis::of(diff, x).abs() == 1)
    };
    if a.cog == Cog::Large && b.cog == Cog::Small && large_to_small(a, b) {
        return -2.0;
    }
    if b.cog == Cog::Large && a.cog == Cog::Small && large_to_small(b, a) {
        return -0.5;
    }
    if a.cog == Cog::Small && b.cog == Cog::Small {
        let manhattan: i32 = diff.iter().map(|c| c.abs()).sum();
        if let Some(d) = Dir::from_offset(diff)
            && manhattan == 1
            && d.axis() != a.axis
            && a.axis == b.axis
        {
            return -1.0;
        }
    }
    0.0
}

/// Positions a part may connect to: the six faces, and for cogs the twelve edge diagonals
/// (Create's `getPotentialNeighbourLocations` and `addPropagationLocations`).
fn neighbour_offsets(p: &Part) -> impl Iterator<Item = [i32; 3]> {
    let faces = Dir::ALL.into_iter().map(Dir::offset);
    let diagonals = (p.cog != Cog::None)
        .then(|| {
            (-1..=1).flat_map(|x| {
                (-1..=1).flat_map(move |y| {
                    (-1..=1)
                        .map(move |z| [x, y, z])
                        .filter(|o| o.iter().map(|c| c * c).sum::<i32>() == 2)
                })
            })
        })
        .into_iter()
        .flatten();
    faces.chain(diagonals)
}

fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// What a solve changed, for the game to apply: speeds to show and act on, blocks to break.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A part's speed (0 when stopped or overstressed) and whether its network is overstressed.
    Speed {
        pos: [i32; 3],
        sub: u8,
        speed: f32,
        overstressed: bool,
    },
    /// Create destroys this block: a conflicting direction, a loop that disagrees, or too fast.
    Break { pos: [i32; 3] },
}

#[derive(Clone, Debug)]
struct Node {
    pos: [i32; 3],
    sub: u8,
    part: Part,
    /// When it was set, to find the newest part of a conflict.
    stamp: u64,
    /// The speed it shows now (0 when overstressed).
    speed: f32,
    /// The speed its network turns it at, overstressed or not (`getTheoreticalSpeed`).
    theoretical: f32,
    overstressed: bool,
    /// The network's stress and capacity at the last solve (`KineticBlockEntity.stress`,
    /// `capacity`).
    stress: f32,
    capacity: f32,
    live: bool,
    /// Set or replaced since the last solve: reported even when its speed is unchanged, so the
    /// game shows every part (a still one too).
    fresh: bool,
}

/// The kinetic parts of one dimension.
#[derive(Default)]
pub struct Kinetics {
    nodes: Vec<Node>,
    free: Vec<u32>,
    /// Nodes at a position: index 0, and a split shaft's second half at 1.
    at: HashMap<[i32; 3], [u32; 2]>,
    dirty: Vec<u32>,
    /// Positions whose parts left since the last `take_removed`.
    removed: Vec<[i32; 3]>,
    stamp: u64,
    /// Scratch for solving, kept to avoid allocating per solve.
    factor: HashMap<u32, f32>,
    /// Which side of a coupling each part solved is on.
    side: HashMap<u32, u32>,
    queue: Vec<u32>,
    /// Positions whose network's figures someone reads (gauges), and those of them a solve
    /// looked at since the last `take_touched`.
    watched: HashSet<[i32; 3]>,
    touched: Vec<[i32; 3]>,
}

const NONE: u32 = u32::MAX;

/// The networks a solve looked at, for tests and benchmarks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SolveStats {
    pub components: u32,
    pub nodes: u32,
}

impl Kinetics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len() - self.free.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn has_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Adds or replaces the part at `pos` (`sub` 1 is a split shaft's second half).
    pub fn set(&mut self, pos: [i32; 3], sub: u8, part: Part) {
        self.stamp += 1;
        let slot = self.at.entry(pos).or_insert([NONE, NONE]);
        let existing = slot[sub as usize];
        if existing != NONE {
            let n = &mut self.nodes[existing as usize];
            if n.part == part {
                return;
            }
            n.part = part;
            n.stamp = self.stamp;
            n.fresh = true;
            self.dirty.push(existing);
            return;
        }
        let node = Node {
            pos,
            sub,
            part,
            stamp: self.stamp,
            speed: 0.0,
            theoretical: 0.0,
            overstressed: false,
            stress: 0.0,
            capacity: 0.0,
            live: true,
            fresh: true,
        };
        let id = match self.free.pop() {
            Some(id) => {
                self.nodes[id as usize] = node;
                id
            }
            None => {
                self.nodes.push(node);
                (self.nodes.len() - 1) as u32
            }
        };
        slot[sub as usize] = id;
        self.dirty.push(id);
    }

    /// Removes every part at `pos`; its neighbours re-solve.
    pub fn remove(&mut self, pos: [i32; 3]) {
        let Some(ids) = self.at.remove(&pos) else {
            return;
        };
        for id in ids.into_iter().filter(|&i| i != NONE) {
            let part = self.nodes[id as usize].part;
            for o in neighbour_offsets(&part) {
                if let Some(n) = self.at.get(&add(pos, o)) {
                    self.dirty.extend(n.iter().copied().filter(|&i| i != NONE));
                }
            }
            self.nodes[id as usize].live = false;
            self.free.push(id);
        }
        self.removed.push(pos);
    }

    /// The positions whose parts left since the last call (for the game to hide them).
    pub fn take_removed(&mut self) -> Vec<[i32; 3]> {
        std::mem::take(&mut self.removed)
    }

    /// Watches (or stops watching) `pos`: each solve of its network lists it in `take_touched`.
    pub fn watch(&mut self, pos: [i32; 3], on: bool) {
        if on {
            self.watched.insert(pos);
        } else {
            self.watched.remove(&pos);
        }
    }

    /// The watched positions whose network solved since the last call, moved into `out`.
    pub fn take_touched(&mut self, out: &mut Vec<[i32; 3]>) {
        out.append(&mut self.touched);
    }

    /// Every part: position, half (`sub`), shown speed and whether its network is overstressed.
    pub fn parts(&self) -> impl Iterator<Item = ([i32; 3], u8, f32, bool)> + '_ {
        self.nodes
            .iter()
            .filter(|n| n.live)
            .map(|n| (n.pos, n.sub, n.speed, n.overstressed))
    }

    /// A part's theoretical speed, its network's stress and capacity, and whether it is
    /// overstressed, as of the last solve (what a stressometer reads).
    pub fn network(&self, pos: [i32; 3], sub: u8) -> Option<(f32, f32, f32, bool)> {
        let id = *self.at.get(&pos)?.get(sub as usize)?;
        (id != NONE).then(|| {
            let n = &self.nodes[id as usize];
            (n.theoretical, n.stress, n.capacity, n.overstressed)
        })
    }

    pub fn speed(&self, pos: [i32; 3], sub: u8) -> Option<f32> {
        let id = *self.at.get(&pos)?.get(sub as usize)?;
        (id != NONE).then(|| self.nodes[id as usize].speed)
    }

    /// The speed the part's network turns it at, shown or not (`getTheoreticalSpeed`), and
    /// whether that network is overstressed.
    pub fn theoretical(&self, pos: [i32; 3], sub: u8) -> Option<(f32, bool)> {
        let id = *self.at.get(&pos)?.get(sub as usize)?;
        (id != NONE).then(|| {
            let n = &self.nodes[id as usize];
            (n.theoretical, n.overstressed)
        })
    }

    /// The nodes connected to `id`, with the ratio from it to each.
    fn edges(&self, id: u32, out: &mut Vec<(u32, f32)>) {
        out.clear();
        let n = &self.nodes[id as usize];
        let (pos, part) = (n.pos, n.part);
        // A split shaft's halves.
        if let Some(pair) = self.at.get(&pos) {
            let other = pair[1 - n.sub as usize];
            if other != NONE {
                let link = if n.sub == 1 {
                    part.link
                } else {
                    self.nodes[other as usize].part.link
                }
                .unwrap_or(0.0);
                if link != 0.0 {
                    out.push((other, if n.sub == 1 { 1.0 / link } else { link }));
                }
            }
        }
        for o in neighbour_offsets(&part) {
            let Some(pair) = self.at.get(&add(pos, o)) else {
                continue;
            };
            for &m in pair.iter().filter(|&&m| m != NONE) {
                let other = &self.nodes[m as usize].part;
                let mut r = ratio(&part, other, o);
                if r == 0.0 {
                    // Create checks both ways (`isConnected(a, b) || isConnected(b, a)`).
                    let back = ratio(other, &part, sub([0, 0, 0], o));
                    if back != 0.0 {
                        r = 1.0 / back;
                    }
                }
                if r != 0.0 {
                    out.push((m, r));
                }
            }
        }
    }

    /// The parts `id` sets the speed of, or that set its speed (`Couple`), as couplings from the
    /// part that holds the setting; `pull` gets the parts on the other side, to solve with it.
    fn couplings(&self, id: u32, out: &mut Vec<Coupling>, pull: &mut Vec<u32>) {
        let n = &self.nodes[id as usize];
        let at =
            |p: [i32; 3], sub: usize| self.at.get(&p).map(|pair| pair[sub]).filter(|&m| m != NONE);
        match n.part.couple {
            Some(Couple::Controller(target)) => {
                if let Some(cog) = at(add(n.pos, [0, 1, 0]), 0)
                    && controls(&n.part, &self.nodes[cog as usize].part)
                {
                    out.push(Coupling {
                        from: id,
                        to: cog,
                        kind: Couple::Controller(target),
                        dir: 0,
                    });
                    pull.push(cog);
                }
            }
            Some(Couple::Relay(m)) if n.sub == 1 => {
                if let Some(first) = at(n.pos, 0) {
                    out.push(Coupling {
                        from: first,
                        to: id,
                        kind: Couple::Relay(m),
                        dir: 0,
                    });
                    pull.push(first);
                }
            }
            _ => {}
        }
        // The other side: the controller below a large cog, a split shaft's second half.
        if n.part.cog == Cog::Large
            && n.part.axis != Axis::Y
            && let Some(c) = at(add(n.pos, [0, -1, 0]), 0)
            && controls(&self.nodes[c as usize].part, &n.part)
        {
            pull.push(c);
        }
        if n.sub == 0
            && n.part.shafts.count_ones() == 1
            && let Some(second) = at(n.pos, 1)
            && matches!(
                self.nodes[second as usize].part.couple,
                Some(Couple::Relay(_))
            )
        {
            pull.push(second);
        }
    }

    /// Re-solves every component holding a changed part, and reports what changed.
    ///
    /// Parts set across by a `Couple` solve together, each side with its own factors: the side
    /// whose own generators turn it drives the other, then sides reached that way drive theirs.
    /// Stress and capacity are the whole group's, as Create's controller and cog share one
    /// network (`KineticBlockEntity.setSource` takes the source's network).
    pub fn solve(&mut self, events: &mut Vec<Event>) -> SolveStats {
        let mut stats = SolveStats::default();
        let mut done: HashMap<u32, ()> = HashMap::new();
        let mut edges = Vec::new();
        let mut couplings: Vec<Coupling> = Vec::new();
        let mut pull: Vec<u32> = Vec::new();
        let mut root: Vec<f32> = Vec::new();
        let mut driver: Vec<u32> = Vec::new();
        while let Some(start) = self.dirty.pop() {
            if !self.nodes[start as usize].live || done.contains_key(&start) {
                continue;
            }
            // Factors relative to each side's first part, by breadth-first search; a loop that
            // disagrees breaks the newest part on it.
            self.factor.clear();
            self.side.clear();
            self.queue.clear();
            couplings.clear();
            pull.clear();
            pull.push(start);
            let mut sides = 0u32;
            let mut head = 0;
            let mut conflict: Option<u32> = None;
            while let Some(first) = pull.pop() {
                if self.factor.contains_key(&first) {
                    continue;
                }
                self.factor.insert(first, 1.0);
                self.side.insert(first, sides);
                self.queue.push(first);
                while head < self.queue.len() {
                    let id = self.queue[head];
                    head += 1;
                    let f = self.factor[&id];
                    self.edges(id, &mut edges);
                    for &(m, r) in &edges {
                        let want = f * r;
                        match self.factor.get(&m) {
                            None => {
                                self.factor.insert(m, want);
                                self.side.insert(m, sides);
                                self.queue.push(m);
                            }
                            Some(&have)
                                if (have - want).abs() > have.abs().max(want.abs()) * 1e-4 =>
                            {
                                let newer = self.newer(m, id);
                                conflict = Some(conflict.map_or(newer, |c| self.newer(c, newer)));
                            }
                            _ => {}
                        }
                    }
                    self.couplings(id, &mut couplings, &mut pull);
                }
                sides += 1;
            }
            stats.components += 1;
            stats.nodes += self.queue.len() as u32;
            for &id in &self.queue {
                done.insert(id, ());
            }
            // Generators: the one reaching its side fastest drives it (Create's overpowering);
            // one turning the other way breaks the newest of the two.
            root.clear();
            root.resize(sides as usize, 0.0);
            driver.clear();
            driver.resize(sides as usize, NONE);
            let mut capacity = 0.0f32;
            for &id in &self.queue {
                let g = self.nodes[id as usize].part.generated;
                if g == 0.0 {
                    continue;
                }
                capacity += self.nodes[id as usize].part.capacity * g.abs();
                let s = self.side[&id] as usize;
                let r = g / self.factor[&id];
                if driver[s] == NONE {
                    (root[s], driver[s]) = (r, id);
                } else if r.signum() != root[s].signum() {
                    conflict = conflict.or(Some(self.newer(id, driver[s])));
                } else if r.abs() > root[s].abs() {
                    (root[s], driver[s]) = (r, id);
                }
            }
            // Couplings carry speed from a side that turns to the other, as far as it reaches;
            // a side turned the other way by its own generators breaks the newer part.
            for _ in 0..=couplings.len() {
                let mut changed = false;
                for c in couplings.iter_mut() {
                    let (a, b) = (self.side[&c.from] as usize, self.side[&c.to] as usize);
                    if a == b {
                        continue;
                    }
                    if c.dir == 0 {
                        c.dir = if root[a] != 0.0 {
                            1
                        } else if root[b] != 0.0 {
                            -1
                        } else {
                            continue;
                        };
                    }
                    let (src, dst) = if c.dir > 0 {
                        (c.from, c.to)
                    } else {
                        (c.to, c.from)
                    };
                    let (s, d) = (self.side[&src] as usize, self.side[&dst] as usize);
                    let want = match c.kind {
                        Couple::Controller(target) => target,
                        Couple::Relay(m) => root[s] * self.factor[&src] * m,
                    };
                    if root[s] == 0.0 || want == 0.0 {
                        continue;
                    }
                    let r = want / self.factor[&dst];
                    if root[d] == 0.0 {
                        root[d] = r;
                        changed = true;
                    } else if r.signum() != root[d].signum() {
                        let newer = self.newer(src, dst);
                        conflict = conflict.or(Some(newer));
                    } else if r.abs() > root[d].abs() * (1.0 + 1e-4) {
                        root[d] = r;
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            // A coupling whose ends a loop already joins: Create does not overpower its own
            // network (`RotationPropagator.propagateNewSource`), it breaks the block when the
            // speed conveyed is faster than, or opposed to, what the other end turns at.
            for c in &couplings {
                let s = self.side[&c.from] as usize;
                if s != self.side[&c.to] as usize || root[s] == 0.0 {
                    continue;
                }
                let (from, to) = (root[s] * self.factor[&c.from], root[s] * self.factor[&c.to]);
                let breaks = match c.kind {
                    // `SpeedControllerBlockEntity.getConveyedSpeed`: the larger of the target and
                    // the cog's speed, or the target when their signs differ; Create's epsilon is
                    // the cog's speed / 256 / 256.
                    Couple::Controller(target) => {
                        target != 0.0
                            && (target.signum() != to.signum()
                                || target.abs() > to.abs() + to.abs() / 256.0 / 256.0)
                    }
                    // The far half at the modifier times the near one's, as any loop's ratio
                    // (`getRotationSpeedModifier`; 0 cuts, conveying nothing either way).
                    Couple::Relay(m) => {
                        m != 0.0 && (to - from * m).abs() > to.abs().max((from * m).abs()) * 1e-4
                    }
                };
                if breaks {
                    let newer = self.newer(c.from, c.to);
                    conflict = Some(conflict.map_or(newer, |x| self.newer(x, newer)));
                }
            }
            if conflict.is_none() {
                // Too fast anywhere breaks the newest part that turns too fast.
                let mut fastest: Option<u32> = None;
                for &id in &self.queue {
                    if (root[self.side[&id] as usize] * self.factor[&id]).abs() > MAX_SPEED {
                        fastest = Some(fastest.map_or(id, |f| self.newer(f, id)));
                    }
                }
                conflict = fastest;
            }
            if let Some(victim) = conflict {
                let pos = self.nodes[victim as usize].pos;
                events.push(Event::Break { pos });
                // The rest re-solves without it.
                for &id in &self.queue {
                    done.remove(&id);
                }
                self.remove(pos);
                continue;
            }
            let theoretical = |k: &Self, id: u32| root[k.side[&id] as usize] * k.factor[&id];
            let stress: f32 = self
                .queue
                .iter()
                .map(|&id| self.nodes[id as usize].part.impact * theoretical(self, id).abs())
                .sum();
            let overstressed = stress > capacity;
            if !self.watched.is_empty() {
                for &id in &self.queue {
                    let pos = self.nodes[id as usize].pos;
                    if self.nodes[id as usize].sub == 0 && self.watched.contains(&pos) {
                        self.touched.push(pos);
                    }
                }
            }
            for i in 0..self.queue.len() {
                let id = self.queue[i];
                let turned = theoretical(self, id);
                let shown = if overstressed { 0.0 } else { turned };
                let n = &mut self.nodes[id as usize];
                n.stress = stress;
                n.capacity = capacity;
                // Create calls `onSpeedChanged` when the theoretical speed changes too, shown or
                // not (`RotationPropagator` sets it on overstressed networks as well).
                if n.fresh
                    || n.speed != shown
                    || n.theoretical != turned
                    || n.overstressed != overstressed
                {
                    n.fresh = false;
                    n.speed = shown;
                    n.theoretical = turned;
                    n.overstressed = overstressed;
                    events.push(Event::Speed {
                        pos: n.pos,
                        sub: n.sub,
                        speed: shown,
                        overstressed,
                    });
                }
            }
        }
        stats
    }

    /// The newer of two parts (set later): the one Create breaks in a conflict.
    fn newer(&self, a: u32, b: u32) -> u32 {
        if self.nodes[a as usize].stamp > self.nodes[b as usize].stamp {
            a
        } else {
            b
        }
    }
}

/// A coupling found while solving: `from` holds the setting (the controller, a split shaft's
/// first half), `dir` says which side drives (1 `from`'s, -1 `to`'s, 0 not known yet).
#[derive(Clone, Copy, Debug)]
struct Coupling {
    from: u32,
    to: u32,
    kind: Couple,
    dir: i8,
}

/// `RotationPropagator.isLargeCogToSpeedController`: a large cog on a horizontal axis right above
/// a speed controller, across the controller's axis.
fn controls(controller: &Part, cog: &Part) -> bool {
    matches!(controller.couple, Some(Couple::Controller(_)))
        && cog.cog == Cog::Large
        && cog.axis != Axis::Y
        && cog.axis != controller.axis
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A creative motor set to `rpm`, turning the shaft on `face` (CreativeMotorBlockEntity).
    fn motor(face: Dir, rpm: f32) -> Part {
        Part {
            generated: convert_to_direction(rpm, face),
            capacity: 16384.0 / 256.0,
            ..Part::half(face)
        }
    }

    fn solve(k: &mut Kinetics) -> Vec<Event> {
        let mut e = Vec::new();
        k.solve(&mut e);
        e
    }

    #[test]
    fn a_motor_drives_a_line_of_shafts() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 32.0));
        for x in 1..=5 {
            k.set([x, 0, 0], 0, Part::shaft(Axis::X));
        }
        solve(&mut k);
        for x in 1..=5 {
            assert_eq!(k.speed([x, 0, 0], 0), Some(32.0));
        }
        // A steady network does nothing.
        let mut e = Vec::new();
        assert_eq!(k.solve(&mut e), SolveStats::default());
        assert!(e.is_empty());
        // Removing a shaft stops what is past it.
        k.remove([3, 0, 0]);
        solve(&mut k);
        assert_eq!(k.speed([2, 0, 0], 0), Some(32.0));
        assert_eq!(k.speed([4, 0, 0], 0), Some(0.0));
    }

    #[test]
    fn new_parts_are_reported_even_when_still_and_removals_are_kept() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, Part::shaft(Axis::X));
        let e = solve(&mut k);
        assert_eq!(
            e,
            [Event::Speed {
                pos: [0, 0, 0],
                sub: 0,
                speed: 0.0,
                overstressed: false
            }]
        );
        assert!(solve(&mut k).is_empty());
        k.remove([0, 0, 0]);
        assert_eq!(k.take_removed(), [[0, 0, 0]]);
        assert!(k.take_removed().is_empty());
        assert_eq!(k.parts().count(), 0);
    }

    #[test]
    fn shafts_on_another_axis_do_not_connect() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, Part::shaft(Axis::Z));
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(0.0));
    }

    #[test]
    fn cogs_reverse_and_large_cogs_change_speed() {
        let mut k = Kinetics::new();
        k.set([0, 0, -1], 0, motor(Dir::South, 16.0));
        k.set([0, 0, 0], 0, Part::cog(Axis::Z, false));
        k.set([1, 0, 0], 0, Part::cog(Axis::Z, false));
        // A large cog diagonal to the second small one: small -> large is -0.5.
        k.set([2, 1, 0], 0, Part::cog(Axis::Z, true));
        solve(&mut k);
        assert_eq!(k.speed([0, 0, 0], 0), Some(16.0));
        assert_eq!(k.speed([1, 0, 0], 0), Some(-16.0));
        assert_eq!(k.speed([2, 1, 0], 0), Some(8.0));
    }

    #[test]
    fn large_cogs_on_crossed_axes_mesh() {
        let mut k = Kinetics::new();
        k.set([0, -1, 0], 0, motor(Dir::Up, 10.0));
        k.set([0, 0, 0], 0, Part::cog(Axis::Y, true));
        k.set([1, 1, 0], 0, Part::cog(Axis::X, true));
        solve(&mut k);
        // diff (1, 1, 0): along Y (from) +1, along X (to) +1: same sign, ratio 1.
        assert_eq!(k.speed([1, 1, 0], 0), Some(10.0));
    }

    #[test]
    fn a_gearbox_turns_the_corner() {
        let mut k = Kinetics::new();
        k.set([-1, 0, 0], 0, motor(Dir::East, 20.0));
        k.set([0, 0, 0], 0, Part::gearbox(Axis::Y));
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        k.set([0, 0, 1], 0, Part::shaft(Axis::Z));
        k.set([0, 0, -1], 0, Part::shaft(Axis::Z));
        solve(&mut k);
        // RotationPropagator.getAxisModifier with the source on the west face: east (same axis,
        // other side) -1, south (positive, unlike the west source) 1, north (negative like it) -1.
        assert_eq!(k.speed([1, 0, 0], 0), Some(-20.0));
        assert_eq!(k.speed([0, 0, 1], 0), Some(20.0));
        assert_eq!(k.speed([0, 0, -1], 0), Some(-20.0));
    }

    #[test]
    fn angular_speed_is_create_s() {
        assert_eq!(convert_to_angular(32.0), 9.6);
        assert_eq!(convert_to_direction(16.0, Dir::West), -16.0);
    }

    #[test]
    fn opposing_motors_break_the_newest() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        k.set([2, 0, 0], 0, motor(Dir::West, 16.0));
        let e = solve(&mut k);
        assert!(e.contains(&Event::Break { pos: [2, 0, 0] }), "{e:?}");
        assert_eq!(k.speed([1, 0, 0], 0), Some(16.0));
    }

    #[test]
    fn the_faster_motor_drives_and_too_fast_breaks() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        // Facing west, -64 is +64 along X: the same way as the first, and faster.
        k.set([2, 0, 0], 0, motor(Dir::West, -64.0));
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(64.0));
        // Two large-to-small steps from 128 RPM: 256 is allowed, 512 is not.
        let mut k = Kinetics::new();
        k.set([0, 0, -1], 0, motor(Dir::South, 128.0));
        k.set([0, 0, 0], 0, Part::cog(Axis::Z, true));
        k.set([1, 1, 0], 0, Part::cog(Axis::Z, false));
        let e = solve(&mut k);
        assert!(e.is_empty() || !e.iter().any(|x| matches!(x, Event::Break { .. })));
        assert_eq!(k.speed([1, 1, 0], 0), Some(-256.0));
        // A large cog on the small one's shaft turns at -256 and doubles again: 512 is too fast.
        k.set([1, 1, 1], 0, Part::cog(Axis::Z, true));
        k.set([2, 2, 1], 0, Part::cog(Axis::Z, false));
        let e = solve(&mut k);
        assert!(e.contains(&Event::Break { pos: [2, 2, 1] }), "{e:?}");
        assert_eq!(k.speed([1, 1, 1], 0), Some(-256.0));
    }

    #[test]
    fn a_loop_of_four_cogs_agrees() {
        // Four small cogs in a square each reverse the next: the loop agrees, nothing breaks.
        let mut k = Kinetics::new();
        k.set([0, 0, -1], 0, motor(Dir::South, 8.0));
        for p in [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0]] {
            k.set(p, 0, Part::cog(Axis::Z, false));
        }
        let e = solve(&mut k);
        assert!(!e.iter().any(|x| matches!(x, Event::Break { .. })), "{e:?}");
        assert_eq!(k.speed([1, 1, 0], 0), Some(8.0));
    }

    #[test]
    fn overstress_stops_the_network() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0)); // capacity 64 SU/RPM * 16 = 1024 SU
        let heavy = Part {
            impact: 32.0,
            ..Part::shaft(Axis::X)
        };
        k.set([1, 0, 0], 0, heavy); // 32 * 16 = 512 SU
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(16.0));
        k.set([2, 0, 0], 0, heavy);
        k.set([3, 0, 0], 0, heavy); // 1536 SU > 1024 SU
        let e = solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(0.0));
        assert!(e.iter().any(|x| matches!(
            x,
            Event::Speed {
                overstressed: true,
                ..
            }
        )));
    }

    #[test]
    fn split_shafts_cut_or_reverse() {
        let mut k = Kinetics::new();
        k.set([-1, 0, 0], 0, motor(Dir::East, 12.0));
        k.set([0, 0, 0], 0, Part::half(Dir::West));
        k.set(
            [0, 0, 0],
            1,
            Part {
                link: Some(-1.0),
                ..Part::half(Dir::East)
            },
        );
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        solve(&mut k);
        assert_eq!(
            k.speed([1, 0, 0], 0),
            Some(-12.0),
            "a powered gearshift reverses"
        );
        k.set(
            [0, 0, 0],
            1,
            Part {
                link: Some(0.0),
                ..Part::half(Dir::East)
            },
        );
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(0.0), "a powered clutch cuts");
        assert_eq!(k.speed([0, 0, 0], 0), Some(12.0));
    }

    #[test]
    fn only_the_changed_component_is_solved() {
        let mut k = Kinetics::new();
        for n in 0..3 {
            let z = n * 10;
            k.set([0, 0, z], 0, motor(Dir::East, 16.0));
            for x in 1..=100 {
                k.set([x, 0, z], 0, Part::shaft(Axis::X));
            }
        }
        k.solve(&mut Vec::new());
        k.set([101, 0, 10], 0, Part::shaft(Axis::X));
        let stats = k.solve(&mut Vec::new());
        assert_eq!(
            stats,
            SolveStats {
                components: 1,
                nodes: 102
            }
        );
    }

    /// A chain drive on the x axis whose chain runs along z.
    fn drive(positive: bool, negative: bool, modifier: f32) -> Part {
        Part {
            chain: Some(Chain {
                along: Axis::Z,
                positive,
                negative,
                modifier,
            }),
            ..Part::shaft(Axis::X)
        }
    }

    #[test]
    fn chain_drives_carry_their_speed_times_the_gearshift_s_modifier() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, drive(true, false, 1.0));
        k.set([1, 0, 1], 0, drive(true, true, 1.0));
        k.set([1, 0, 2], 0, drive(false, true, 1.0));
        k.set([2, 0, 2], 0, Part::shaft(Axis::X));
        // Off the chain's line: a drive beside it on x does not join.
        k.set([1, 1, 0], 0, drive(true, true, 1.0));
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 2], 0), Some(16.0));
        assert_eq!(k.speed([2, 0, 2], 0), Some(16.0));
        assert_eq!(k.speed([1, 1, 0], 0), Some(0.0));
        // An adjustable chain gearshift at 2 (signal 15) as the first: the chain turns twice as
        // fast (from 2 to 1), its own shaft as the motor.
        k.set([1, 0, 0], 0, drive(true, false, 2.0));
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(16.0));
        assert_eq!(k.speed([1, 0, 1], 0), Some(32.0));
        assert_eq!(k.speed([2, 0, 2], 0), Some(32.0));
        // Neither going on toward the other (an end, then a start): they do not connect. One
        // toward the other is enough (Create checks both ways).
        k.set([1, 0, 1], 0, drive(false, true, 1.0));
        k.set([1, 0, 2], 0, drive(true, false, 1.0));
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 2], 0), Some(0.0));
    }

    fn controller(target: f32) -> Part {
        Part {
            couple: Some(Couple::Controller(target)),
            ..Part::shaft(Axis::X)
        }
    }

    #[test]
    fn a_speed_controller_sets_the_large_cog_above_it() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, controller(64.0));
        k.set([1, 1, 0], 0, Part::cog(Axis::Z, true));
        k.set([2, 2, 0], 0, Part::cog(Axis::Z, false));
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(16.0));
        assert_eq!(k.speed([1, 1, 0], 0), Some(64.0));
        assert_eq!(k.speed([2, 2, 0], 0), Some(-128.0));
        // A cog on the controller's own axis is not set.
        let mut k2 = Kinetics::new();
        k2.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k2.set([1, 0, 0], 0, controller(64.0));
        k2.set([1, 1, 0], 0, Part::cog(Axis::X, true));
        solve(&mut k2);
        assert_eq!(k2.speed([1, 1, 0], 0), Some(0.0));
        // The controller not turning: nothing turns (getDesiredOutputSpeed: no source, 0).
        k.remove([0, 0, 0]);
        solve(&mut k);
        assert_eq!(k.speed([1, 1, 0], 0), Some(0.0));
        // Target 0: the cog stays still.
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.remove([1, 0, 0]);
        k.set([1, 0, 0], 0, controller(0.0));
        solve(&mut k);
        assert_eq!(k.speed([1, 1, 0], 0), Some(0.0));
    }

    #[test]
    fn a_turning_cog_drives_a_still_controller_at_its_target() {
        let mut k = Kinetics::new();
        k.set([1, 0, 0], 0, controller(-48.0));
        k.set([2, 0, 0], 0, Part::shaft(Axis::X));
        k.set([1, 1, 0], 0, Part::cog(Axis::Z, true));
        k.set([1, 1, -1], 0, motor(Dir::South, 16.0));
        solve(&mut k);
        assert_eq!(k.speed([1, 1, 0], 0), Some(16.0));
        assert_eq!(k.speed([2, 0, 0], 0), Some(-48.0));
    }

    #[test]
    fn a_controlled_cog_shares_its_controller_s_stress() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0)); // 1024 SU
        k.set([1, 0, 0], 0, controller(128.0));
        k.set(
            [1, 1, 0],
            0,
            Part {
                impact: 8.0, // 8 * 128 = 1024 SU: at capacity
                ..Part::cog(Axis::Z, true)
            },
        );
        solve(&mut k);
        assert_eq!(k.speed([1, 1, 0], 0), Some(128.0));
        k.set(
            [1, 1, 1],
            0,
            Part {
                impact: 1.0,
                ..Part::shaft(Axis::Z)
            },
        );
        solve(&mut k);
        assert_eq!(k.speed([1, 1, 0], 0), Some(0.0), "overstressed");
        assert_eq!(
            k.theoretical([1, 1, 0], 0),
            Some((128.0, true)),
            "still turned at its target in theory"
        );
        assert_eq!(
            k.speed([1, 0, 0], 0),
            Some(0.0),
            "the controller's side too"
        );
    }

    fn sequencer(k: &mut Kinetics, modifier: f32) {
        k.set([0, 0, 0], 0, Part::half(Dir::West));
        k.set(
            [0, 0, 0],
            1,
            Part {
                couple: Some(Couple::Relay(modifier)),
                ..Part::half(Dir::East)
            },
        );
    }

    #[test]
    fn a_relay_turns_the_far_half_from_whichever_side_turns() {
        for (m, expect) in [(2.0, 24.0), (-1.0, -12.0), (-2.0, -24.0), (0.0, 0.0)] {
            // Turned from the west.
            let mut k = Kinetics::new();
            k.set([-1, 0, 0], 0, motor(Dir::East, 12.0));
            sequencer(&mut k, m);
            k.set([1, 0, 0], 0, Part::shaft(Axis::X));
            solve(&mut k);
            assert_eq!(k.speed([1, 0, 0], 0), Some(expect), "west, {m}");
            assert_eq!(k.speed([0, 0, 0], 0), Some(12.0));
            // Turned from the east: the west half turns at the modifier times it too.
            let mut k = Kinetics::new();
            k.set([1, 0, 0], 0, motor(Dir::West, -12.0));
            sequencer(&mut k, m);
            k.set([-1, 0, 0], 0, Part::shaft(Axis::X));
            solve(&mut k);
            assert_eq!(k.speed([-1, 0, 0], 0), Some(expect), "east, {m}");
        }
        // Past 256 RPM breaks it.
        let mut k = Kinetics::new();
        k.set([-1, 0, 0], 0, motor(Dir::East, 200.0));
        sequencer(&mut k, 2.0);
        let e = solve(&mut k);
        assert!(e.iter().any(|x| matches!(x, Event::Break { .. })), "{e:?}");
    }

    #[test]
    fn a_controller_geared_back_into_its_own_network_breaks_unless_slower() {
        // Motor -> controller -> small cog, up to a large cog and across to the controlled one:
        // the controlled cog already turns at 8 from the controller's own network.
        let build = |target: f32| {
            let mut k = Kinetics::new();
            k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
            k.set([1, 0, 0], 0, controller(target));
            k.set([2, 0, 0], 0, Part::cog(Axis::X, false));
            k.set([2, 1, 1], 0, Part::cog(Axis::X, true));
            k.set([1, 1, 0], 0, Part::cog(Axis::Z, true));
            let e = solve(&mut k);
            (k, e)
        };
        let (k, _) = build(0.0);
        let wheel = k.speed([1, 1, 0], 0).unwrap();
        assert_eq!(wheel.abs(), 8.0);
        // Slower than the cog turns, the same way: the loop holds at the cog's speed
        // (`getConveyedSpeed` is the larger).
        let (k, e) = build(wheel / 2.0);
        assert!(!e.iter().any(|x| matches!(x, Event::Break { .. })), "{e:?}");
        assert_eq!(k.speed([1, 1, 0], 0), Some(wheel));
        assert!(
            !build(wheel)
                .1
                .iter()
                .any(|x| matches!(x, Event::Break { .. }))
        );
        // Faster, or the other way: Create breaks it (`propagateNewSource`).
        for target in [wheel * 8.0, -wheel / 2.0] {
            let (_, e) = build(target);
            assert!(
                e.contains(&Event::Break { pos: [1, 1, 0] }),
                "{target}: {e:?}"
            );
        }
    }

    #[test]
    fn a_relay_whose_halves_a_loop_joins_breaks_unless_it_agrees() {
        // The far half reached around the sequencer by cogs, at the near half's speed.
        for (m, breaks) in [(1.0, false), (2.0, true), (-1.0, true), (0.0, false)] {
            let mut k = Kinetics::new();
            k.set([-2, 0, 0], 0, motor(Dir::East, 12.0));
            for p in [[-1, 0, 0], [-1, 1, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]] {
                k.set(p, 0, Part::cog(Axis::X, false));
            }
            sequencer(&mut k, m);
            let e = solve(&mut k);
            assert_eq!(
                e.iter().any(|x| matches!(x, Event::Break { .. })),
                breaks,
                "{m}: {e:?}"
            );
        }
    }

    #[test]
    fn only_watched_parts_of_solved_networks_are_touched() {
        let mut k = Kinetics::new();
        for z in [0, 10] {
            k.set([0, 0, z], 0, motor(Dir::East, 16.0));
            k.set([1, 0, z], 0, Part::shaft(Axis::X));
        }
        k.watch([1, 0, 0], true);
        k.solve(&mut Vec::new());
        let mut out = Vec::new();
        k.take_touched(&mut out);
        assert_eq!(out, [[1, 0, 0]]);
        // The other network solves: nothing to read.
        k.set([2, 0, 10], 0, Part::shaft(Axis::X));
        k.solve(&mut Vec::new());
        out.clear();
        k.take_touched(&mut out);
        assert!(out.is_empty());
        // Its own does, until it is no longer watched.
        k.set([2, 0, 0], 0, Part::shaft(Axis::X));
        k.solve(&mut Vec::new());
        k.take_touched(&mut out);
        assert_eq!(out, [[1, 0, 0]]);
        k.watch([1, 0, 0], false);
        k.set([3, 0, 0], 0, Part::shaft(Axis::X));
        k.solve(&mut Vec::new());
        out.clear();
        k.take_touched(&mut out);
        assert!(out.is_empty());
    }
}
