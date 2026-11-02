//! Create's track graph, as a train moves over it (`TrackGraph`, `TrackEdge`, `TravellingPoint`):
//! nodes where track pieces meet, edges between them (straight, or a curve), and a point that
//! travels a distance along them, passing from edge to edge where the track runs on smoothly
//! (`canTravelTo`) and letting a selector choose at a junction.

use crate::bezier::{Curve, Vec3};

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn normalize(a: Vec3) -> Vec3 {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if l < 1.0e-4 {
        [0.0; 3]
    } else {
        [a[0] / l, a[1] / l, a[2] / l]
    }
}
fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// An edge from one node to another: straight between them, or along `turn`.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub turn: Option<Curve>,
}

/// The nodes (their positions) and the edges leaving each (both ways of a piece of track are
/// two edges, as Create keeps them: an edge and its reverse are `e` and `e ^ 1`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub nodes: Vec<Vec3>,
    pub edges: Vec<Edge>,
    /// The edges leaving each node (`getConnectionsFrom`).
    pub out: Vec<Vec<usize>>,
}

impl Graph {
    pub fn node(&mut self, at: Vec3) -> usize {
        self.nodes.push(at);
        self.out.push(Vec::new());
        self.nodes.len() - 1
    }

    /// The same piece of track the other way.
    #[inline]
    pub fn reverse(e: usize) -> usize {
        e ^ 1
    }

    /// A piece of track between `a` and `b`, both ways; `turn` runs from `a` to `b` (its
    /// reverse is the same curve from the other end).
    pub fn connect(&mut self, a: usize, b: usize, turn: Option<Curve>) {
        let back = turn.as_ref().map(|c| {
            Curve::new(
                [c.starts[1], c.starts[0]],
                [c.axes[1], c.axes[0]],
                [c.normals[1], c.normals[0]],
            )
        });
        self.out[a].push(self.edges.len());
        self.edges.push(Edge {
            from: a,
            to: b,
            turn,
        });
        self.out[b].push(self.edges.len());
        self.edges.push(Edge {
            from: b,
            to: a,
            turn: back,
        });
    }

    /// `getLength`.
    pub fn length(&self, e: usize) -> f64 {
        let edge = &self.edges[e];
        match &edge.turn {
            Some(c) => c.length,
            None => {
                let d = sub(self.nodes[edge.to], self.nodes[edge.from]);
                dot(d, d).sqrt()
            }
        }
    }

    /// `getPosition(t)`: along the curve, or between the nodes.
    pub fn position(&self, e: usize, t: f64) -> Vec3 {
        let edge = &self.edges[e];
        match &edge.turn {
            Some(c) => c.position(t.clamp(0.0, 1.0)),
            None => {
                let (a, b) = (self.nodes[edge.from], self.nodes[edge.to]);
                let t = f64::from(t as f32);
                [
                    a[0] + (b[0] - a[0]) * t,
                    a[1] + (b[1] - a[1]) * t,
                    a[2] + (b[2] - a[2]) * t,
                ]
            }
        }
    }

    /// `incrementT`: a curve's own stepping for short moves, straight otherwise.
    fn increment_t(&self, e: usize, t: f64, distance: f64) -> f64 {
        let too_far = distance.abs() > 5.0;
        let length = self.length(e);
        let share = distance / if length == 0.0 { 1.0 } else { length };
        match &self.edges[e].turn {
            Some(c) if !too_far => c.increment_t(t, share),
            _ => t + share,
        }
    }

    /// `getDirection(fromFirst)`: the way the edge leaves its first node, or enters its last.
    pub fn direction(&self, e: usize, from_first: bool) -> Vec3 {
        let (a, b) = if from_first { (0.0, 0.25) } else { (0.75, 1.0) };
        normalize(sub(self.position(e, b), self.position(e, a)))
    }

    /// `canTravelTo`: the next edge goes on within about 29° (`dot > 7/8`).
    pub fn can_travel_to(&self, e: usize, next: usize) -> bool {
        dot(self.direction(e, false), self.direction(next, true)) > 7.0 / 8.0
    }

    /// The edges a train on `e` may go on to at its end (not straight back).
    pub fn onward(&self, e: usize) -> Vec<usize> {
        self.onward_iter(e).collect()
    }

    /// [`Graph::onward`] without collecting.
    pub fn onward_iter(&self, e: usize) -> impl Iterator<Item = usize> + '_ {
        let edge = &self.edges[e];
        self.out[edge.to].iter().copied().filter(move |&n| {
            let next = &self.edges[n];
            next.to != edge.from && self.can_travel_to(e, n)
        })
    }
}

/// A point on the graph: its edge and how far along it (blocks).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub edge: usize,
    pub position: f64,
    /// It came to the end of the track (`blocked`).
    pub blocked: bool,
}

impl Point {
    /// `TravellingPoint.travel`, forward: moves `distance` along, on to the next edge at an end
    /// (`select` chooses among several, given the edge it leaves and the candidates), stopping
    /// at a dead end. The distance it really travelled.
    pub fn travel(
        &mut self,
        g: &Graph,
        distance: f64,
        mut select: impl FnMut(usize, &[usize]) -> usize,
    ) -> f64 {
        self.blocked = false;
        let length = g.length(self.edge);
        if distance.abs() < 1.0e-5 {
            return 0.0;
        }
        let mut travelled = distance;
        let t = if length == 0.0 {
            0.0
        } else {
            self.position / length
        };
        self.position = g.increment_t(self.edge, t, distance) * length;
        let mut length = length;
        while self.position > length {
            let targets = g.onward(self.edge);
            if targets.is_empty() {
                travelled -= self.position - length;
                self.position = length;
                self.blocked = true;
                break;
            }
            let next = if targets.len() == 1 {
                targets[0]
            } else {
                targets[select(self.edge, &targets).min(targets.len() - 1)]
            };
            self.edge = next;
            self.position -= length;
            length = g.length(next);
        }
        travelled
    }

    /// The same place facing the other way (on the reverse edge).
    pub fn flipped(&self, g: &Graph) -> Point {
        Point {
            edge: Graph::reverse(self.edge),
            position: g.length(self.edge) - self.position,
            blocked: self.blocked,
        }
    }

    /// [`Point::travel`] either way: a negative distance runs backward along the track (the
    /// selector then chooses among the edges behind it, facing the way it moves). The signed
    /// distance it really travelled.
    pub fn travel_signed(
        &mut self,
        g: &Graph,
        distance: f64,
        select: impl FnMut(usize, &[usize]) -> usize,
    ) -> f64 {
        if distance >= 0.0 {
            return self.travel(g, distance, select);
        }
        let mut back = self.flipped(g);
        let travelled = back.travel(g, -distance, select);
        *self = back.flipped(g);
        -travelled
    }

    /// Where it is in the world.
    pub fn at(&self, g: &Graph) -> Vec3 {
        let length = g.length(self.edge);
        g.position(
            self.edge,
            if length == 0.0 {
                0.0
            } else {
                self.position / length
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UP: Vec3 = [0.0, 1.0, 0.0];

    /// A straight run east, a quarter turn to the south and on south; and a branch going on
    /// east from the same node.
    fn junction() -> (Graph, usize) {
        let mut g = Graph::default();
        let a = g.node([0.0, 0.0, 0.0]);
        let b = g.node([10.0, 0.0, 0.0]);
        let c = g.node([18.0, 0.0, 8.0]);
        let d = g.node([18.0, 0.0, 20.0]);
        let e = g.node([30.0, 0.0, 0.0]);
        g.connect(a, b, None);
        let turn = Curve::new(
            [[10.0, 0.0, 0.0], [18.0, 0.0, 8.0]],
            [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
            [UP, UP],
        );
        g.connect(b, c, Some(turn));
        g.connect(c, d, None);
        g.connect(b, e, None);
        (g, 0)
    }

    #[test]
    fn a_point_runs_through_the_turn_or_straight_on_at_the_junction() {
        let (g, first) = junction();
        // From the end of a→b two edges go on: the turn and the straight branch; not back.
        let onward = g.onward(first);
        assert_eq!(onward.len(), 2);
        // Into the turn: after it, heading south on c→d.
        let turn = *onward.iter().find(|&&e| g.edges[e].turn.is_some()).unwrap();
        let mut p = Point {
            edge: first,
            position: 0.0,
            blocked: false,
        };
        let mut moved = 0.0;
        for _ in 0..40 {
            moved += p.travel(&g, 0.8, |_, c| c.iter().position(|&e| e == turn).unwrap());
        }
        assert!((moved - 32.0).abs() < 1e-6, "{moved}");
        let at = p.at(&g);
        assert!((at[0] - 18.0).abs() < 1e-6 && at[2] > 8.0, "{at:?}");
        assert!(g.edges[p.edge].turn.is_none() && g.nodes[g.edges[p.edge].to][2] == 20.0);
        // Straight on instead: along the branch to x = 30, then the dead end blocks it.
        let mut q = Point {
            edge: first,
            position: 0.0,
            blocked: false,
        };
        let straight =
            |_: usize, c: &[usize]| c.iter().position(|&e| g.edges[e].turn.is_none()).unwrap();
        let mut travelled = 0.0;
        for _ in 0..40 {
            travelled += q.travel(&g, 1.0, straight);
        }
        assert!(q.blocked);
        assert!((travelled - 30.0).abs() < 1e-6, "{travelled}");
        assert_eq!(q.at(&g), [30.0, 0.0, 0.0]);
    }

    #[test]
    fn a_sharp_corner_is_not_track() {
        // Two straight edges meeting at a right angle: no smooth way on.
        let mut g = Graph::default();
        let a = g.node([0.0, 0.0, 0.0]);
        let b = g.node([5.0, 0.0, 0.0]);
        let c = g.node([5.0, 0.0, 5.0]);
        g.connect(a, b, None);
        g.connect(b, c, None);
        assert!(g.onward(0).is_empty());
    }
}
