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
//!   and two large cogs on crossed axes at ±1;
//! - the generator whose speed reaches the network fastest drives it; one turning the other way, or a
//!   loop whose ratios disagree, breaks the newest part (Create destroys the block that caused it), and
//!   so does any part past the maximum speed (256 RPM);
//! - stress is the sum of each part's impact times its speed, capacity the sum of each generator's
//!   capacity times its own generated speed; a network whose stress exceeds its capacity stops.

use std::collections::HashMap;

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
    if let Some(d) = Dir::from_offset(diff)
        && a.shafts & d.bit() != 0
        && b.shafts & d.opposite().bit() != 0
    {
        return a.face_sign[d as usize] as f32 / b.face_sign[d.opposite() as usize] as f32;
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
    overstressed: bool,
    live: bool,
}

/// The kinetic parts of one dimension.
#[derive(Default)]
pub struct Kinetics {
    nodes: Vec<Node>,
    free: Vec<u32>,
    /// Nodes at a position: index 0, and a split shaft's second half at 1.
    at: HashMap<[i32; 3], [u32; 2]>,
    dirty: Vec<u32>,
    stamp: u64,
    /// Scratch for solving, kept to avoid allocating per solve.
    factor: HashMap<u32, f32>,
    queue: Vec<u32>,
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
            self.dirty.push(existing);
            return;
        }
        let node = Node {
            pos,
            sub,
            part,
            stamp: self.stamp,
            speed: 0.0,
            overstressed: false,
            live: true,
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
    }

    pub fn speed(&self, pos: [i32; 3], sub: u8) -> Option<f32> {
        let id = *self.at.get(&pos)?.get(sub as usize)?;
        (id != NONE).then(|| self.nodes[id as usize].speed)
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

    /// Re-solves every component holding a changed part, and reports what changed.
    pub fn solve(&mut self, events: &mut Vec<Event>) -> SolveStats {
        let mut stats = SolveStats::default();
        let mut done: HashMap<u32, ()> = HashMap::new();
        let mut edges = Vec::new();
        while let Some(start) = self.dirty.pop() {
            if !self.nodes[start as usize].live || done.contains_key(&start) {
                continue;
            }
            // Factors relative to `start`, by breadth-first search; a loop that disagrees
            // breaks the newest part on it.
            self.factor.clear();
            self.queue.clear();
            self.factor.insert(start, 1.0);
            self.queue.push(start);
            let mut head = 0;
            let mut conflict: Option<u32> = None;
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
                            self.queue.push(m);
                        }
                        Some(&have) if (have - want).abs() > have.abs().max(want.abs()) * 1e-4 => {
                            let newer =
                                if self.nodes[m as usize].stamp > self.nodes[id as usize].stamp {
                                    m
                                } else {
                                    id
                                };
                            conflict = Some(conflict.map_or(newer, |c| {
                                if self.nodes[c as usize].stamp >= self.nodes[newer as usize].stamp
                                {
                                    c
                                } else {
                                    newer
                                }
                            }));
                        }
                        _ => {}
                    }
                }
            }
            stats.components += 1;
            stats.nodes += self.queue.len() as u32;
            for &id in &self.queue {
                done.insert(id, ());
            }
            // Generators: the one reaching the network fastest drives it (Create's overpowering);
            // one turning the other way breaks the newest of the two.
            let mut drive: Option<(u32, f32)> = None;
            let mut capacity = 0.0f32;
            for &id in &self.queue {
                let g = self.nodes[id as usize].part.generated;
                if g == 0.0 {
                    continue;
                }
                capacity += self.nodes[id as usize].part.capacity * g.abs();
                let root = g / self.factor[&id];
                match drive {
                    None => drive = Some((id, root)),
                    Some((other, r)) => {
                        if r.signum() != root.signum() {
                            let newer = if self.nodes[id as usize].stamp
                                > self.nodes[other as usize].stamp
                            {
                                id
                            } else {
                                other
                            };
                            conflict = conflict.or(Some(newer));
                        } else if root.abs() > r.abs() {
                            drive = Some((id, root));
                        }
                    }
                }
            }
            let root = drive.map_or(0.0, |(_, r)| r);
            if conflict.is_none() && root != 0.0 {
                // Too fast anywhere breaks the newest part that turns too fast.
                let mut fastest: Option<u32> = None;
                for &id in &self.queue {
                    if (root * self.factor[&id]).abs() > MAX_SPEED {
                        fastest = Some(match fastest {
                            Some(f)
                                if self.nodes[f as usize].stamp > self.nodes[id as usize].stamp =>
                            {
                                f
                            }
                            _ => id,
                        });
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
            let stress: f32 = self
                .queue
                .iter()
                .map(|&id| self.nodes[id as usize].part.impact * (root * self.factor[&id]).abs())
                .sum();
            let overstressed = stress > capacity;
            for i in 0..self.queue.len() {
                let id = self.queue[i];
                let theoretical = root * self.factor[&id];
                let shown = if overstressed { 0.0 } else { theoretical };
                let n = &mut self.nodes[id as usize];
                if n.speed != shown || n.overstressed != overstressed {
                    n.speed = shown;
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
}
