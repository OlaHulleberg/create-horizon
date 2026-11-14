//! Create's track graph built from the world (`TrackPropagator.onRailAdded`,
//! `ITrackBlock.getConnected`, `TrackBlock.getConnected`, `TrackNodeLocation`,
//! `isValidGraphNodeLocation`): every track block joins the two ends it runs between (a crossing
//! joins its centre to each of its four ends), every curve its two starts; a place where track
//! ends, branches, bends out of line or a curve starts is a node (and a straight run gets one at
//! each chunk line, as Create's); the straight runs between nodes are straight edges, curves are
//! curved ones. Built once when track changes, never per tick.
//!
//! Stations and signals become points on the edges of the track beside them (`EdgePointStorage`,
//! `TrackTargetingBehaviour`), and signals split the graph into sections (`SignalEdgeGroup`,
//! `SignalPropagator`): the track between signals, joined through nodes. Signal blocks beside the
//! same track block are one boundary (`SignalBoundary`, `canMerge`) with two sides, one for each
//! way a train runs past it; a block stands for the trains that have it on their right (the side
//! Create takes from the click that targets the track, as this port has no targeting item yet).

use crate::bezier::{Curve, Vec3};
use crate::graph::{Graph, Point};
use crate::track::TrackShape;
use crate::track_curves::Curves;
use std::collections::HashMap;

/// A node's place: `TrackNodeLocation`'s half-block x and z, and twice its floored y.
pub type Key = [i32; 3];

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: Vec3, s: f64) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn length(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}
fn normalize(a: Vec3) -> Vec3 {
    let l = length(a);
    if l < 1.0e-4 {
        [0.0; 3]
    } else {
        scale(a, 1.0 / l)
    }
}
/// Java's `Math.round`.
fn round(a: f64) -> i64 {
    (a + 0.5).floor() as i64
}

/// `new TrackNodeLocation(vec)`.
pub fn key(v: Vec3) -> Key {
    [
        round(v[0] * 2.0) as i32,
        v[1].floor() as i32 * 2,
        round(v[2] * 2.0) as i32,
    ]
}

/// `TrackNodeLocation.getLocation`.
pub fn key_position(k: Key) -> Vec3 {
    [
        f64::from(k[0]) / 2.0,
        f64::from(k[1]) / 2.0,
        f64::from(k[2]) / 2.0,
    ]
}

/// A place on the graph: an edge and how far along it.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Location {
    pub edge: usize,
    pub position: f64,
}

/// What stands on an edge (`TrackEdgePoint`): a station or a signal, by its index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Station(usize),
    Signal(usize),
    /// A track observer (`TrackObserver`), by its index in `Network::observers`.
    Observer(usize),
}

/// What a signal block is (`SignalBlock.TYPE`): an entry signal holds trains while the section
/// beyond is occupied; a cross signal while any section of the chain beyond it up to the next
/// entry signal is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SignalType {
    #[default]
    Entry,
    Cross,
}

/// A signal boundary (`SignalBoundary`): where it stands (on the piece's even edge), and for each
/// side, the signal blocks standing for the trains running that way (side 0: along the even
/// edge, side 1: along its reverse) and the side's type (`types`: its first block's), and the
/// signals ending the chain a cross signal looks down (`collectChainedSignals`: boundary and
/// side, leaving the section beyond).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Boundary {
    pub at: Location,
    pub blocks: [Vec<[i32; 3]>; 2],
    pub types: [SignalType; 2],
    pub chain: [Vec<(usize, usize)>; 2],
}

/// The side of a boundary a train running along `edge` passes it from.
#[inline]
pub fn side_of(edge: usize) -> usize {
    edge & 1
}

/// A piece of track between two places: a track block's run (its block), a crossing's half, or
/// a curve.
struct Segment {
    ends: [Key; 2],
    turn: Option<Curve>,
    block: Option<[i32; 3]>,
    direction: Vec3,
}

/// A dimension's track graph and what stands on it.
#[derive(Clone, Debug, Default)]
pub struct Network {
    pub graph: Graph,
    pub nodes: HashMap<Key, usize>,
    pub keys: Vec<Key>,
    /// Straight track blocks: the edge they lie on and how far along it their centre is.
    pub blocks: HashMap<[i32; 3], Location>,
    /// Stations bound to the graph: their block and where on it.
    pub stations: Vec<([i32; 3], Location)>,
    /// Signal boundaries (`SignalBoundary`), each where its track block's centre is.
    pub signals: Vec<Boundary>,
    /// Track observers bound to the graph: their block and where on the piece's even edge.
    pub observers: Vec<([i32; 3], Location)>,
    /// Per edge, what stands on it, by how far along it.
    pub marks: Vec<Vec<(f64, Mark)>>,
    /// Signal sections: each piece of track's first slot (its stretches between signals are
    /// consecutive slots), and each slot's section.
    slot_base: Vec<usize>,
    section_of: Vec<usize>,
}

/// The straight track a station or signal looks at (`TrackTargetingBehaviour`, as this port
/// places it): the track block beside it running straight along x or z.
pub fn target_track(tracks: &HashMap<[i32; 3], TrackShape>, pos: [i32; 3]) -> Option<[i32; 3]> {
    for o in [[0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]] {
        let t = [pos[0] + o[0], pos[1], pos[2] + o[2]];
        if matches!(tracks.get(&t), Some(TrackShape::Xo | TrackShape::Zo)) {
            return Some(t);
        }
    }
    None
}

/// `TrackPropagator.isValidGraphNodeLocation` for a place and the pieces meeting there.
fn is_node(at: Key, here: &[usize], segments: &[Segment]) -> bool {
    if here.len() != 2 {
        return true;
    }
    // `viaTurn` forces a node at both ends of a curve.
    if here.iter().any(|&s| segments[s].turn.is_some()) {
        return true;
    }
    // `notInLineWith`: either way within about 29°.
    let (a, b) = (segments[here[0]].direction, segments[here[1]].direction);
    if dot(a, b).abs() < 7.0 / 8.0 {
        return true;
    }
    let v = key_position(at);
    let centred_x = (v[0] - round(v[0]) as f64).abs() >= 1.0e-5;
    let centred_z = (v[2] - round(v[2]) as f64).abs() >= 1.0e-5;
    if centred_x && !centred_z {
        return round(v[2]) % 16 == 0;
    }
    round(v[0]) % 16 == 0
}

impl Network {
    /// The graph of `tracks` (block → shape) and the `curves` between them (only those whose
    /// both blocks are track), with `stations` and `signals` bound to the track beside them.
    pub fn build(
        tracks: &HashMap<[i32; 3], TrackShape>,
        curves: &Curves,
        stations: &[[i32; 3]],
        signals: &[([i32; 3], SignalType)],
    ) -> Network {
        let mut segments: Vec<Segment> = Vec::new();
        let mut positions: Vec<[i32; 3]> = tracks.keys().copied().collect();
        positions.sort_unstable();
        for p in &positions {
            let shape = tracks[p];
            if shape == TrackShape::None {
                continue;
            }
            let axes = shape.axes();
            // `getElevationAtCenter`: half a block up on a slope.
            let elevation = if axes[0][1] != 0.0 { 0.5 } else { 0.0 };
            let centre = [
                f64::from(p[0]) + 0.5,
                f64::from(p[1]) + elevation,
                f64::from(p[2]) + 0.5,
            ];
            if let [axis] = axes {
                let half = scale(*axis, 0.5);
                segments.push(Segment {
                    ends: [key(sub(centre, half)), key(add(centre, half))],
                    turn: None,
                    block: Some(*p),
                    direction: normalize(*axis),
                });
            } else {
                for axis in axes {
                    for side in [-0.5, 0.5] {
                        segments.push(Segment {
                            ends: [key(centre), key(add(centre, scale(*axis, side)))],
                            turn: None,
                            block: None,
                            direction: normalize(*axis),
                        });
                    }
                }
            }
        }
        let mut turns: Vec<(&[i32; 3], &crate::track_curves::Connection)> = curves
            .at
            .iter()
            .flat_map(|(p, list)| list.iter().filter(|c| c.primary).map(move |c| (p, c)))
            .filter(|(p, c)| tracks.contains_key(*p) && tracks.contains_key(&c.to))
            .collect();
        turns.sort_unstable_by_key(|(p, c)| (**p, c.to));
        for (_, c) in turns {
            segments.push(Segment {
                ends: [key(c.curve.starts[0]), key(c.curve.starts[1])],
                turn: Some(c.curve.clone()),
                block: None,
                direction: [0.0; 3],
            });
        }
        let mut at: HashMap<Key, Vec<usize>> = HashMap::new();
        for (i, s) in segments.iter().enumerate() {
            at.entry(s.ends[0]).or_default().push(i);
            if s.ends[1] != s.ends[0] {
                at.entry(s.ends[1]).or_default().push(i);
            }
        }
        let mut places: Vec<Key> = at.keys().copied().collect();
        places.sort_unstable();
        let mut net = Network::default();
        for &p in &places {
            if is_node(p, &at[&p], &segments) {
                net.add_node(p);
            }
        }
        // Walk each piece from its node to the next node: one edge.
        let mut used = vec![false; segments.len()];
        let mut on: Vec<([i32; 3], f64)> = Vec::new();
        let mut n = 0;
        while n < net.keys.len() {
            let start = net.keys[n];
            n += 1;
            for &first in &at[&start] {
                if used[first] {
                    continue;
                }
                on.clear();
                let (mut here, mut s, mut run) = (start, first, 0.0);
                let end = loop {
                    used[s] = true;
                    let seg = &segments[s];
                    let next = if seg.ends[0] == here {
                        seg.ends[1]
                    } else {
                        seg.ends[0]
                    };
                    let step = length(sub(key_position(next), key_position(here)));
                    if let Some(b) = seg.block {
                        on.push((b, run + step / 2.0));
                    }
                    run += step;
                    if net.nodes.contains_key(&next) {
                        break next;
                    }
                    match at[&next].iter().find(|&&o| o != s && !used[o]) {
                        Some(&o) => {
                            here = next;
                            s = o;
                        }
                        // A loop with no node on it: one here.
                        None => break next,
                    }
                };
                let a = net.nodes[&start];
                let b = net.add_node(end);
                let turn = segments[first].turn.as_ref().map(|c| {
                    if segments[first].ends[0] == start {
                        c.clone()
                    } else {
                        Curve::new(
                            [c.starts[1], c.starts[0]],
                            [c.axes[1], c.axes[0]],
                            [c.normals[1], c.normals[0]],
                        )
                    }
                });
                let e = net.graph.edges.len();
                net.graph.connect(a, b, turn);
                for &(block, along) in &on {
                    net.blocks.insert(
                        block,
                        Location {
                            edge: e,
                            position: along,
                        },
                    );
                }
            }
        }
        net.marks = vec![Vec::new(); net.graph.edges.len()];
        for &pos in stations {
            if let Some(loc) = target_track(tracks, pos).and_then(|t| net.blocks.get(&t)) {
                let mark = Mark::Station(net.stations.len());
                net.stations.push((pos, *loc));
                net.mark(*loc, mark);
            }
        }
        let mut signals: Vec<([i32; 3], SignalType)> = signals.to_vec();
        signals.sort_unstable_by_key(|s| s.0);
        for &(pos, kind) in &signals {
            let Some(track) = target_track(tracks, pos) else {
                continue;
            };
            let Some(&loc) = net.blocks.get(&track) else {
                continue;
            };
            // On the piece's even edge, facing its way.
            let at = if loc.edge.is_multiple_of(2) {
                loc
            } else {
                Location {
                    edge: loc.edge ^ 1,
                    position: net.graph.length(loc.edge) - loc.position,
                }
            };
            let d = net.graph.direction(at.edge, true);
            let right = [-d[2], 0.0, d[0]];
            let beside = [
                f64::from(pos[0] - track[0]),
                0.0,
                f64::from(pos[2] - track[2]),
            ];
            let side = usize::from(dot(right, beside) < 0.0);
            let b =
                match net.signals.iter().position(|b| {
                    b.at.edge == at.edge && (b.at.position - at.position).abs() < 1.0e-6
                }) {
                    Some(b) => b,
                    None => {
                        net.signals.push(Boundary {
                            at,
                            ..Boundary::default()
                        });
                        net.mark(at, Mark::Signal(net.signals.len() - 1));
                        net.signals.len() - 1
                    }
                };
            let boundary = &mut net.signals[b];
            // `blockEntityAdded`: the side takes its first block's type.
            if boundary.blocks[side].is_empty() {
                boundary.types[side] = kind;
            }
            boundary.blocks[side].push(pos);
        }
        for m in &mut net.marks {
            m.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        net.sections();
        net.chains();
        net
    }

    /// Track observers on the track beside them (`TrackTargetingBehaviour`, as stations are
    /// placed), each on its piece's even edge; they split no section.
    pub fn bind_observers(
        &mut self,
        tracks: &HashMap<[i32; 3], TrackShape>,
        observers: &[[i32; 3]],
    ) {
        for &pos in observers {
            let Some(&loc) = target_track(tracks, pos).and_then(|t| self.blocks.get(&t)) else {
                continue;
            };
            let at = if loc.edge.is_multiple_of(2) {
                loc
            } else {
                Location {
                    edge: loc.edge ^ 1,
                    position: self.graph.length(loc.edge) - loc.position,
                }
            };
            let mark = Mark::Observer(self.observers.len());
            self.observers.push((pos, at));
            self.mark(at, mark);
        }
        for m in &mut self.marks {
            m.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
    }

    /// Whether the train whose wheel points are `points` (front first, all facing its front)
    /// stands over the place `at` (on an even edge): between its first and last point
    /// (`frontSignalListener` added it, `backSignalListener` not yet taken it off). Each stretch
    /// between neighbouring points is followed from the back one up to 8 edges on.
    pub fn covers(&self, points: &[Point], at: Location) -> bool {
        let g = &self.graph;
        // `at` along edge `e`, or None when it is on another piece.
        let along = |e: usize| -> Option<f64> {
            if e == at.edge {
                Some(at.position)
            } else if e == at.edge ^ 1 {
                Some(g.length(e) - at.position)
            } else {
                None
            }
        };
        for w in points.windows(2) {
            let (front, back) = (w[0], w[1]);
            if front.edge == back.edge {
                if along(back.edge).is_some_and(|p| p >= back.position && p <= front.position) {
                    return true;
                }
                continue;
            }
            if along(back.edge).is_some_and(|p| p >= back.position) {
                return true;
            }
            if along(front.edge).is_some_and(|p| p <= front.position) {
                return true;
            }
            // The edges between: the shortest way (in edges) from the back's edge to the front's.
            let mut parent: Vec<(usize, usize)> = vec![(back.edge, usize::MAX)];
            let mut frontier = vec![back.edge];
            let mut found = None;
            'search: for _ in 0..8 {
                let mut next = Vec::new();
                for &e in &frontier {
                    for n in g.onward_iter(e) {
                        if parent.iter().any(|(x, _)| *x == n) {
                            continue;
                        }
                        parent.push((n, e));
                        if n == front.edge {
                            found = Some(e);
                            break 'search;
                        }
                        next.push(n);
                    }
                }
                frontier = next;
            }
            let mut e = found.unwrap_or(back.edge);
            while e != back.edge {
                if along(e).is_some() {
                    return true;
                }
                e = parent
                    .iter()
                    .find(|(x, _)| *x == e)
                    .map_or(back.edge, |p| p.1);
            }
        }
        false
    }

    fn add_node(&mut self, k: Key) -> usize {
        if let Some(&n) = self.nodes.get(&k) {
            return n;
        }
        let n = self.graph.node(key_position(k));
        self.nodes.insert(k, n);
        self.keys.push(k);
        n
    }

    /// A station or signal on both ways of its edge.
    fn mark(&mut self, loc: Location, mark: Mark) {
        let length = self.graph.length(loc.edge);
        self.marks[loc.edge].push((loc.position, mark));
        self.marks[Graph::reverse(loc.edge)].push((length - loc.position, mark));
    }

    /// `SignalPropagator`: the stretches between signals, joined through nodes.
    fn sections(&mut self) {
        let pieces = self.graph.edges.len() / 2;
        let mut base = Vec::with_capacity(pieces + 1);
        let mut slots = 0;
        for u in 0..pieces {
            base.push(slots);
            slots += 1 + self.signal_count(u);
        }
        base.push(slots);
        let mut parent: Vec<usize> = (0..slots).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        for n in 0..self.graph.nodes.len() {
            let mut first = None;
            for &e in &self.graph.out[n] {
                let u = e / 2;
                // Leaving the node along a piece's own way: its first stretch; else its last.
                let slot = if e.is_multiple_of(2) {
                    base[u]
                } else {
                    base[u + 1] - 1
                };
                let root = find(&mut parent, slot);
                match first {
                    None => first = Some(root),
                    Some(f) if f != root => parent[root] = f,
                    _ => {}
                }
            }
        }
        self.section_of = (0..slots).map(|i| find(&mut parent, i)).collect();
        self.slot_base = base;
    }

    fn signal_count(&self, piece: usize) -> usize {
        self.marks[piece * 2]
            .iter()
            .filter(|(_, m)| matches!(m, Mark::Signal(_)))
            .count()
    }

    /// How many sections there can be (the size of an occupancy table).
    pub fn section_slots(&self) -> usize {
        self.section_of.len()
    }

    /// The section a place on an edge is in; one exactly at a signal is in the section beyond
    /// it, the way the edge runs.
    pub fn section(&self, edge: usize, position: f64) -> usize {
        let u = edge / 2;
        let length = self.graph.length(edge);
        let (along, beyond) = if edge.is_multiple_of(2) {
            (position, true)
        } else {
            (length - position, false)
        };
        let k = self.marks[u * 2]
            .iter()
            .filter(|(p, m)| {
                matches!(m, Mark::Signal(_)) && if beyond { *p <= along } else { *p < along }
            })
            .count();
        self.section_of[self.slot_base[u] + k]
    }

    /// The section a train enters passing boundary `b` from `side` (`getGroup`).
    pub fn beyond(&self, b: usize, side: usize) -> usize {
        let at = self.signals[b].at;
        if side == 0 {
            self.section(at.edge, at.position)
        } else {
            self.section(at.edge ^ 1, self.graph.length(at.edge) - at.position)
        }
    }

    /// `SignalPropagator.collectChainedSignals` for every cross signal side: the boundaries a
    /// train leaves the section beyond it by (and the side it passes them from), once per build.
    fn chains(&mut self) {
        for b in 0..self.signals.len() {
            for side in 0..2 {
                if self.signals[b].types[side] != SignalType::Cross
                    || self.signals[b].blocks[side].is_empty()
                {
                    continue;
                }
                let inside = self.beyond(b, side);
                let mut chain = Vec::new();
                for o in 0..self.signals.len() {
                    for k in 0..2 {
                        if (o, k) == (b, 1 - side) {
                            continue;
                        }
                        if self.beyond(o, 1 - k) == inside && self.beyond(o, k) != inside {
                            chain.push((o, k));
                        }
                    }
                }
                self.signals[b].chain[side] = chain;
            }
        }
    }

    /// The point on the track nearest `at` (within half a block) facing along `direction`, on
    /// either way of its edge.
    pub fn locate(&self, at: Vec3, direction: Vec3) -> Option<Point> {
        let g = &self.graph;
        let mut best: Option<(f64, usize, f64)> = None;
        for e in (0..g.edges.len()).step_by(2) {
            let span = g.length(e);
            let (t, d) = match &g.edges[e].turn {
                None => {
                    let (a, b) = (g.nodes[g.edges[e].from], g.nodes[g.edges[e].to]);
                    let ab = sub(b, a);
                    let l2 = dot(ab, ab);
                    let t = if l2 == 0.0 {
                        0.0
                    } else {
                        (dot(sub(at, a), ab) / l2).clamp(0.0, 1.0)
                    };
                    (t, length(sub(at, add(a, scale(ab, t)))))
                }
                Some(c) => {
                    let steps = (c.length * 8.0).ceil().max(8.0) as usize;
                    let mut found = (0.0, f64::MAX);
                    for i in 0..=steps {
                        let t = i as f64 / steps as f64;
                        let d = length(sub(at, c.position(t)));
                        if d < found.1 {
                            found = (t, d);
                        }
                    }
                    found
                }
            };
            if d < 0.5 && best.is_none_or(|b| d < b.0) {
                best = Some((d, e, t * span));
            }
        }
        let (_, e, position) = best?;
        let p = Point {
            edge: e,
            position,
            blocked: false,
        };
        let span = g.length(e);
        let t = if span == 0.0 { 0.0 } else { position / span };
        let ahead = g.position(e, (t + 0.01).min(1.0));
        let behind = g.position(e, (t - 0.01).max(0.0));
        Some(if dot(sub(ahead, behind), direction) < 0.0 {
            p.flipped(g)
        } else {
            p
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track_curves::Saved;

    const UP: Vec3 = [0.0, 1.0, 0.0];

    pub fn line(
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

    #[test]
    fn a_straight_line_is_one_edge_per_chunk() {
        let mut t = HashMap::new();
        line(&mut t, [1, 64, 3], [1, 0, 0], 10, TrackShape::Xo);
        let n = Network::build(&t, &Curves::default(), &[], &[]);
        // Two nodes, one piece of track both ways.
        assert_eq!(n.graph.nodes.len(), 2);
        assert_eq!(n.graph.edges.len(), 2);
        assert!((n.graph.length(0) - 10.0).abs() < 1e-9);
        assert_eq!(n.graph.nodes[0], [1.0, 64.0, 3.5]);
        // Each block's centre on it.
        let loc = n.blocks[&[4, 64, 3]];
        assert!((loc.position - 3.5).abs() < 1e-9, "{loc:?}");
        // Over a chunk line: a node there.
        let mut t = HashMap::new();
        line(&mut t, [10, 64, 3], [1, 0, 0], 10, TrackShape::Xo);
        let n = Network::build(&t, &Curves::default(), &[], &[]);
        assert_eq!(n.graph.nodes.len(), 3);
        assert!(n.nodes.contains_key(&key([16.0, 64.0, 3.5])));
        // A slope up and on: nodes where it bends.
        let mut t = HashMap::new();
        line(&mut t, [0, 64, 1], [0, 0, 1], 3, TrackShape::Zo);
        line(&mut t, [0, 64, 4], [0, 1, 1], 3, TrackShape::As);
        line(&mut t, [0, 67, 7], [0, 0, 1], 3, TrackShape::Zo);
        let n = Network::build(&t, &Curves::default(), &[], &[]);
        assert_eq!(n.graph.edges.len(), 6, "{:?}", n.keys);
        let slope = n.blocks[&[0, 65, 5]].edge;
        assert!((n.graph.length(slope) - 3.0 * 2f64.sqrt()).abs() < 1e-9);
        // A 45° bend is too sharp to run over (`canTravelTo`): Create's ramps onto a slope are
        // curves laid by the track item.
        let onward = n.graph.onward(n.blocks[&[0, 64, 2]].edge);
        assert!(onward.is_empty());
        assert_eq!(n.graph.onward(slope).len(), 0);
    }

    /// Straight along x from x = -20 to 2 at z = 0, a quarter turn of radius 7.5 to the south,
    /// and straight on south at x = 10 from z = 8 to 30.
    pub fn l_shape() -> (HashMap<[i32; 3], TrackShape>, Curves) {
        let mut t = HashMap::new();
        line(&mut t, [-20, 64, 0], [1, 0, 0], 23, TrackShape::Xo);
        line(&mut t, [10, 64, 8], [0, 0, 1], 23, TrackShape::Zo);
        let mut c = Curves::default();
        c.add(Saved {
            from: [2, 64, 0],
            to: [10, 64, 8],
            starts: [[3.0, 64.0, 0.5], [10.5, 64.0, 8.0]],
            axes: [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
            normals: [UP, UP],
        });
        (t, c)
    }

    #[test]
    fn a_quarter_turn_joins_two_straights() {
        let (t, c) = l_shape();
        let n = Network::build(&t, &c, &[], &[]);
        let turn = (0..n.graph.edges.len())
            .find(|&e| n.graph.edges[e].turn.is_some())
            .unwrap();
        assert!((n.graph.length(turn) - 11.78).abs() < 0.05);
        // From the x straight, through the turn and onto the z straight.
        let west = n.blocks[&[0, 64, 0]].edge;
        let next = n.graph.onward(west);
        assert_eq!(next.len(), 1);
        let after = n.graph.onward(next[0]);
        assert_eq!(after.len(), 1);
        let south = n.blocks[&[10, 64, 9]].edge;
        assert!(after[0] == south || n.graph.edges[after[0]].to == n.graph.edges[south].from);
        // Nothing runs off the end of the curve sideways.
        assert!(n.graph.edges.iter().all(|e| e.from != e.to));
    }

    #[test]
    fn a_junction_is_a_node_with_three_ways() {
        let (mut t, c) = l_shape();
        // The x straight goes on past where the turn leaves.
        line(&mut t, [3, 64, 0], [1, 0, 0], 10, TrackShape::Xo);
        let n = Network::build(&t, &c, &[], &[]);
        let at = n.nodes[&key([3.0, 64.0, 0.5])];
        assert_eq!(n.graph.out[at].len(), 3);
        let west = n.blocks[&[1, 64, 0]].edge;
        let mut onward = n.graph.onward(west);
        onward.sort();
        assert_eq!(onward.len(), 2, "on east, or into the turn");
        assert_eq!(
            onward
                .iter()
                .filter(|&&e| n.graph.edges[e].turn.is_some())
                .count(),
            1
        );
        // A crossing: its centre is a node of four, and no train turns there.
        let mut t = HashMap::new();
        line(&mut t, [0, 64, 5], [1, 0, 0], 5, TrackShape::Xo);
        line(&mut t, [6, 64, 5], [1, 0, 0], 5, TrackShape::Xo);
        line(&mut t, [5, 64, 0], [0, 0, 1], 5, TrackShape::Zo);
        line(&mut t, [5, 64, 6], [0, 0, 1], 5, TrackShape::Zo);
        t.insert([5, 64, 5], TrackShape::CrO);
        let n = Network::build(&t, &Curves::default(), &[], &[]);
        let centre = n.nodes[&key([5.5, 64.0, 5.5])];
        assert_eq!(n.graph.out[centre].len(), 4);
        let into = n.graph.out[centre]
            .iter()
            .map(|&e| Graph::reverse(e))
            .next()
            .unwrap();
        assert_eq!(n.graph.onward(into).len(), 1, "straight across only");
    }

    #[test]
    fn signals_split_the_track_into_sections() {
        let mut t = HashMap::new();
        line(&mut t, [0, 64, 0], [1, 0, 0], 15, TrackShape::Xo);
        // Signals beside x = 5 and x = 10, a station beside x = 12.
        let n = Network::build(
            &t,
            &Curves::default(),
            &[[12, 64, 1]],
            &[
                ([5, 64, 1], SignalType::Entry),
                ([10, 64, -1], SignalType::Entry),
            ],
        );
        assert_eq!(n.signals.len(), 2);
        assert_eq!(n.stations.len(), 1);
        let e = n.blocks[&[0, 64, 0]].edge;
        let (a, b, c) = (n.section(e, 2.0), n.section(e, 7.0), n.section(e, 13.0));
        assert!(a != b && b != c && a != c);
        // At the signal: the section beyond it, either way.
        assert_eq!(n.section(e, 5.5), b);
        assert_eq!(n.section(Graph::reverse(e), 15.0 - 5.5), a);
        // A point found again by where it is.
        let p = n.locate([7.25, 64.0, 0.5], [-1.0, 0.0, 0.0]).unwrap();
        assert_eq!(p.edge, Graph::reverse(e));
        assert!((p.position - 7.75).abs() < 1e-6);
    }

    #[test]
    fn a_signal_stands_for_the_trains_that_have_it_on_their_right() {
        let mut t = HashMap::new();
        line(&mut t, [0, 64, 0], [1, 0, 0], 20, TrackShape::Xo);
        // South of the track at x = 5: for trains running east; north at x = 10: west; both
        // sides at x = 15 (one boundary, a cross signal for the trains running west).
        let n = Network::build(
            &t,
            &Curves::default(),
            &[],
            &[
                ([5, 64, 1], SignalType::Entry),
                ([10, 64, -1], SignalType::Entry),
                ([15, 64, 1], SignalType::Entry),
                ([15, 64, -1], SignalType::Cross),
            ],
        );
        assert_eq!(n.signals.len(), 3, "the two at x = 15 are one boundary");
        let east = n.blocks[&[0, 64, 0]].edge;
        let east_side = side_of(east);
        let at = |x: f64| {
            n.signals
                .iter()
                .find(|b| {
                    let p = if b.at.edge == east {
                        b.at.position
                    } else {
                        20.0 - b.at.position
                    };
                    (p - x).abs() < 1e-6
                })
                .unwrap()
        };
        assert_eq!(at(5.5).blocks[east_side], [[5, 64, 1]]);
        assert!(at(5.5).blocks[1 - east_side].is_empty());
        assert_eq!(at(10.5).blocks[1 - east_side], [[10, 64, -1]]);
        assert!(at(10.5).blocks[east_side].is_empty());
        let both = at(15.5);
        assert_eq!(both.types[east_side], SignalType::Entry);
        assert_eq!(both.types[1 - east_side], SignalType::Cross);
        // The cross signal (westbound at x = 15) looks down its section to the boundary at
        // x = 10, passed westward.
        let b15 = n
            .signals
            .iter()
            .position(|b| std::ptr::eq(b, both))
            .unwrap();
        let b10 = n
            .signals
            .iter()
            .position(|b| std::ptr::eq(b, at(10.5)))
            .unwrap();
        assert_eq!(both.chain[1 - east_side], [(b10, 1 - east_side)]);
        assert!(
            both.chain[east_side].is_empty(),
            "an entry side has no chain"
        );
        assert_ne!(n.beyond(b15, east_side), n.beyond(b15, 1 - east_side));
    }
}
