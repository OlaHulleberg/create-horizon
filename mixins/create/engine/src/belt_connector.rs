//! Create's belt connector (`BeltConnectorItem`): a first click on a shaft remembers it on the
//! stack (`BELT_FIRST_SHAFT`), a second click on another shaft lays a belt between them when
//! they connect (`canConnect`), turning the shafts at the ends into the belt's pulleys.

use crate::belt::{Part, Segment, Slope};
use crate::kinetics::{Axis, Dir};

/// The item.
pub const ITEM: &str = "create:belt_connector";
/// `kinetics.maxBeltLength`.
pub const MAX_LENGTH: i32 = 20;
/// The stack's component (`AllDataComponents.BELT_FIRST_SHAFT`).
pub const FIRST_SHAFT_KEY: &str = "create:belt_first_shaft";

/// What `canConnect` and `createBelts` ask of the world.
pub trait Ground {
    /// `Level.isLoaded`.
    fn loaded(&self, pos: [i32; 3]) -> bool;
    /// The axis of the shaft at `pos` (`ShaftBlock.isShaft`: a plain shaft), None for anything
    /// else.
    fn shaft(&self, pos: [i32; 3]) -> Option<Axis>;
    /// `KineticBlockEntity.getTheoreticalSpeed`.
    fn speed(&self, pos: [i32; 3]) -> f32;
    /// `BlockState.canBeReplaced`.
    fn replaceable(&self, pos: [i32; 3]) -> bool;
}

/// `Vec3i.closerThan`: squared block distance under `d²`.
pub fn closer_than(a: [i32; 3], b: [i32; 3], d: i32) -> bool {
    let s: i64 = (0..3).map(|i| i64::from(a[i] - b[i]).pow(2)).sum();
    s < i64::from(d) * i64::from(d)
}

fn choose(axis: Axis, v: [i32; 3]) -> i32 {
    match axis {
        Axis::X => v[0],
        Axis::Y => v[1],
        Axis::Z => v[2],
    }
}

/// `validateAxis`: a loaded shaft.
pub fn valid_axis(g: &impl Ground, pos: [i32; 3]) -> bool {
    g.loaded(pos) && g.shaft(pos).is_some()
}

/// `BeltConnectorItem.canConnect`.
pub fn can_connect(g: &impl Ground, first: [i32; 3], second: [i32; 3]) -> bool {
    if !g.loaded(first) || !g.loaded(second) {
        return false;
    }
    if !closer_than(second, first, MAX_LENGTH) {
        return false;
    }
    let diff = [0, 1, 2].map(|i| second[i] - first[i]);
    // `getValue(AXIS)` of a block that is no longer a shaft would throw: no belt.
    let Some(axis) = g.shaft(first) else {
        return false;
    };
    let [x, y, z] = diff;
    let sames = i32::from(x.abs() == y.abs())
        + i32::from(y.abs() == z.abs())
        + i32::from(z.abs() == x.abs());
    if choose(axis, diff) != 0 {
        return false;
    }
    if sames != 1 {
        return false;
    }
    if g.shaft(second) != Some(axis) {
        return false;
    }
    if axis == Axis::Y && x != 0 && z != 0 {
        return false;
    }
    let (s1, s2) = (g.speed(first), g.speed(second));
    if s1.signum() != s2.signum() && s1 != 0.0 && s2 != 0.0 {
        return false;
    }
    let step = diff.map(i32::signum);
    let mut p = [0, 1, 2].map(|i| first[i] + step[i]);
    let mut limit = 1000;
    while p != second && limit > 0 {
        limit -= 1;
        if g.shaft(p) != Some(axis) && !g.replaceable(p) {
            return false;
        }
        p = [0, 1, 2].map(|i| p[i] + step[i]);
    }
    true
}

/// `getSlopeBetween`.
pub fn slope_between(start: [i32; 3], end: [i32; 3]) -> Slope {
    let d = [0, 1, 2].map(|i| end[i] - start[i]);
    if d[1] != 0 {
        if d[0] != 0 || d[2] != 0 {
            return if d[1] > 0 {
                Slope::Upward
            } else {
                Slope::Downward
            };
        }
        return Slope::Vertical;
    }
    Slope::Horizontal
}

/// `getFacingFromTo`.
pub fn facing_from_to(start: [i32; 3], end: [i32; 3]) -> Dir {
    let axis = if start[0] == end[0] { Axis::Z } else { Axis::X };
    let d = [0, 1, 2].map(|i| end[i] - start[i]);
    let positive = if d[0] == 0 && d[2] == 0 {
        d[1] > 0
    } else {
        choose(axis, [d[0], 0, d[2]]) > 0
    };
    Dir::from_axis(axis, positive)
}

/// `getBeltChainBetween`: the positions from `start` to `end`.
pub fn chain_between(start: [i32; 3], end: [i32; 3], slope: Slope, facing: Dir) -> Vec<[i32; 3]> {
    let mut out = Vec::new();
    let mut limit = 1000;
    let mut current = start;
    let o = facing.offset();
    loop {
        out.push(current);
        if slope == Slope::Vertical {
            current[1] += if facing.positive() { 1 } else { -1 };
        } else {
            current = [current[0] + o[0], current[1], current[2] + o[2]];
            if slope != Slope::Horizontal {
                current[1] += if slope == Slope::Upward { 1 } else { -1 };
            }
        }
        let go = current != end && limit > 0;
        limit -= 1;
        if !go {
            break;
        }
    }
    out.push(end);
    out
}

/// `createBelts`' layout: each position and the belt segment it becomes (`part`, `slope`,
/// `facing`), with `shaft` the axis of a shaft standing there now (pulleys in the middle; a
/// vertical shaft makes the belt sideways from there on).
pub fn layout(
    start: [i32; 3],
    end: [i32; 3],
    start_axis: Axis,
    shaft: impl Fn([i32; 3]) -> Option<Axis>,
) -> Vec<([i32; 3], Segment)> {
    let mut slope = slope_between(start, end);
    let mut facing = facing_from_to(start, end);
    if end[0] - start[0] == end[2] - start[2] {
        let axis = if start_axis == Axis::X {
            Axis::Z
        } else {
            Axis::X
        };
        facing = Dir::from_axis(axis, facing.positive());
    }
    let mut out = Vec::new();
    for pos in chain_between(start, end, slope, facing) {
        let mut part = if pos == start {
            Part::Start
        } else if pos == end {
            Part::End
        } else {
            Part::Middle
        };
        let s = shaft(pos);
        if part == Part::Middle && s.is_some() {
            part = Part::Pulley;
        }
        if s == Some(Axis::Y) {
            slope = Slope::Sideways;
        }
        out.push((
            pos,
            Segment {
                facing,
                slope,
                part,
            },
        ));
    }
    out
}

/// `BeltSlope`'s state value.
pub fn slope_name(s: Slope) -> &'static str {
    match s {
        Slope::Horizontal => "horizontal",
        Slope::Upward => "upward",
        Slope::Downward => "downward",
        Slope::Vertical => "vertical",
        Slope::Sideways => "sideways",
    }
}

/// `BeltPart`'s state value.
pub fn part_name(p: Part) -> &'static str {
    match p {
        Part::Start => "start",
        Part::Middle => "middle",
        Part::End => "end",
        Part::Pulley => "pulley",
    }
}

/// What a click with the belt connector does (`useOn` on the server).
#[derive(Clone, Debug, PartialEq)]
pub enum Click {
    /// Sneaking: the remembered shaft is forgotten.
    Forget,
    /// Not a shaft (or no player): nothing; `forget` the remembered shaft first when it went.
    Fail { forget: bool },
    /// The first shaft: remembered on the stack.
    Remember,
    /// The second shaft, not connecting: nothing, the first stays remembered.
    Refused,
    /// The second shaft: a belt from the first to it (and one belt item used); the first is
    /// forgotten while some of the stack is left.
    Connect([i32; 3]),
}

/// `BeltConnectorItem.useOn` on the server: the stack remembers `first`, the click is on `pos`.
pub fn click(
    g: &impl Ground,
    sneaking: bool,
    first: Option<[i32; 3]>,
    pos: [i32; 3],
    player: bool,
) -> Click {
    if sneaking {
        return Click::Forget;
    }
    let valid = valid_axis(g, pos);
    // "Remove first if no longer existant or valid".
    let forget = first.is_some_and(|f| !valid_axis(g, f) || !closer_than(f, pos, MAX_LENGTH * 2));
    if !valid || !player {
        return Click::Fail { forget };
    }
    match first.filter(|_| !forget) {
        None => Click::Remember,
        // `canConnect` never joins a shaft to itself.
        Some(f) if !can_connect(g, f, pos) => Click::Refused,
        Some(f) => Click::Connect(f),
    }
}

/// The block `createBelts` plays its sound at: `containing(getCenterOf(start + end) / 2)`.
pub fn sound_at(start: [i32; 3], end: [i32; 3]) -> [i32; 3] {
    [0, 1, 2].map(|i| ((f64::from(start[i] + end[i]) + 0.5) * 0.5).floor() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct World {
        shafts: HashMap<[i32; 3], (Axis, f32)>,
        solid: Vec<[i32; 3]>,
    }

    impl Ground for World {
        fn loaded(&self, _: [i32; 3]) -> bool {
            true
        }
        fn shaft(&self, pos: [i32; 3]) -> Option<Axis> {
            self.shafts.get(&pos).map(|s| s.0)
        }
        fn speed(&self, pos: [i32; 3]) -> f32 {
            self.shafts.get(&pos).map_or(0.0, |s| s.1)
        }
        fn replaceable(&self, pos: [i32; 3]) -> bool {
            !self.solid.contains(&pos) && !self.shafts.contains_key(&pos)
        }
    }

    fn world(shafts: &[([i32; 3], Axis)]) -> World {
        World {
            shafts: shafts.iter().map(|&(p, a)| (p, (a, 0.0))).collect(),
            solid: Vec::new(),
        }
    }

    #[test]
    fn shafts_connect_by_create_s_rules() {
        let x = Axis::X;
        // Along z, square to the x shafts: yes; along their own axis: no.
        let w = world(&[([0, 0, 0], x), ([0, 0, 5], x), ([5, 0, 0], x)]);
        assert!(can_connect(&w, [0, 0, 0], [0, 0, 5]));
        assert!(!can_connect(&w, [0, 0, 0], [5, 0, 0]));
        // A 45° slope connects, a crooked one doesn't; nor does a shaft on another axis.
        let w = world(&[([0, 0, 0], x), ([0, 3, 3], x), ([0, 2, 3], x)]);
        assert!(can_connect(&w, [0, 0, 0], [0, 3, 3]));
        assert!(!can_connect(&w, [0, 0, 0], [0, 2, 3]));
        let w = world(&[([0, 0, 0], x), ([0, 0, 4], Axis::Z)]);
        assert!(!can_connect(&w, [0, 0, 0], [0, 0, 4]));
        // Straight up from a horizontal shaft: a vertical belt.
        let w = world(&[([0, 0, 0], x), ([0, 6, 0], x)]);
        assert!(can_connect(&w, [0, 0, 0], [0, 6, 0]));
        // Vertical shafts: flat along x or z only, never diagonally, never up.
        let y = Axis::Y;
        let w = world(&[
            ([0, 0, 0], y),
            ([4, 0, 0], y),
            ([3, 0, 3], y),
            ([0, 3, 0], y),
        ]);
        assert!(can_connect(&w, [0, 0, 0], [4, 0, 0]));
        assert!(!can_connect(&w, [0, 0, 0], [3, 0, 3]));
        assert!(!can_connect(&w, [0, 0, 0], [0, 3, 0]));
        // At most 20 blocks apart (distance under 20).
        let w = world(&[([0, 0, 0], x), ([0, 0, 19], x), ([0, 0, 20], x)]);
        assert!(can_connect(&w, [0, 0, 0], [0, 0, 19]));
        assert!(!can_connect(&w, [0, 0, 0], [0, 0, 20]));
        // Shafts turning against each other don't; one standing still does.
        let mut w = world(&[([0, 0, 0], x), ([0, 0, 5], x)]);
        w.shafts.insert([0, 0, 0], (x, 16.0));
        w.shafts.insert([0, 0, 5], (x, -16.0));
        assert!(!can_connect(&w, [0, 0, 0], [0, 0, 5]));
        w.shafts.insert([0, 0, 5], (x, 0.0));
        assert!(can_connect(&w, [0, 0, 0], [0, 0, 5]));
        // A solid block between refuses; a shaft on the same axis between is a pulley.
        let mut w = world(&[([0, 0, 0], x), ([0, 0, 5], x), ([0, 0, 2], x)]);
        assert!(can_connect(&w, [0, 0, 0], [0, 0, 5]));
        w.solid.push([0, 0, 3]);
        assert!(!can_connect(&w, [0, 0, 0], [0, 0, 5]));
        let w = world(&[([0, 0, 0], x), ([0, 0, 5], x), ([0, 0, 2], Axis::Z)]);
        assert!(!can_connect(&w, [0, 0, 0], [0, 0, 5]));
        // The same shaft twice: never.
        let w = world(&[([0, 0, 0], x)]);
        assert!(!can_connect(&w, [0, 0, 0], [0, 0, 0]));
    }

    #[test]
    fn a_flat_belt_runs_from_start_to_end_with_pulleys_at_shafts() {
        let shafts = [[0, 0, 0], [0, 0, 2], [0, 0, -4]];
        let at = |p: [i32; 3]| shafts.contains(&p).then_some(Axis::X);
        let l = layout([0, 0, 0], [0, 0, -4], Axis::X, at);
        let parts: Vec<_> = l.iter().map(|(p, s)| (*p, s.part)).collect();
        assert_eq!(
            parts,
            vec![
                ([0, 0, 0], Part::Start),
                ([0, 0, -1], Part::Middle),
                ([0, 0, -2], Part::Middle),
                ([0, 0, -3], Part::Middle),
                ([0, 0, -4], Part::End),
            ]
        );
        assert!(
            l.iter()
                .all(|(_, s)| s.facing == Dir::North && s.slope == Slope::Horizontal)
        );
        // A shaft in the middle becomes a pulley.
        let l = layout([0, 0, 0], [0, 0, 4], Axis::X, at);
        assert_eq!(
            l[2],
            (
                [0, 0, 2],
                Segment {
                    facing: Dir::South,
                    slope: Slope::Horizontal,
                    part: Part::Pulley
                }
            )
        );
        assert_eq!(l.len(), 5);
    }

    #[test]
    fn slopes_vertical_and_sideways_belts_take_create_s_states() {
        let x = |_| Some(Axis::X);
        // Up a slope to the east from z shafts.
        let l = layout([0, 0, 0], [3, 3, 0], Axis::Z, |_| None);
        assert_eq!(
            l.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
            vec![[0, 0, 0], [1, 1, 0], [2, 2, 0], [3, 3, 0]]
        );
        assert!(
            l.iter()
                .all(|(_, s)| s.facing == Dir::East && s.slope == Slope::Upward)
        );
        // Down a slope to the north.
        let l = layout([0, 2, 0], [0, 0, -2], Axis::X, |_| None);
        assert!(
            l.iter()
                .all(|(_, s)| s.facing == Dir::North && s.slope == Slope::Downward)
        );
        // Vertical, between x shafts: facing along z, the way the belt goes up.
        let l = layout([0, 0, 0], [0, 3, 0], Axis::X, x);
        assert_eq!(l.len(), 4);
        assert!(
            l.iter()
                .all(|(_, s)| s.facing == Dir::South && s.slope == Slope::Vertical)
        );
        assert_eq!(l[1].1.part, Part::Pulley);
        // Going down between z shafts: facing west (negative x).
        let l = layout([0, 3, 0], [0, 0, 0], Axis::Z, |_| None);
        assert!(l.iter().all(|(_, s)| s.facing == Dir::West));
        assert_eq!(l.last().unwrap().0, [0, 0, 0]);
        // Between vertical shafts: sideways.
        let l = layout([0, 0, 0], [3, 0, 0], Axis::Y, |p| {
            (p == [0, 0, 0] || p == [3, 0, 0]).then_some(Axis::Y)
        });
        assert!(
            l.iter()
                .all(|(_, s)| s.facing == Dir::East && s.slope == Slope::Sideways)
        );
    }

    #[test]
    fn clicks_remember_connect_and_forget() {
        let x = Axis::X;
        let w = world(&[([0, 0, 0], x), ([0, 0, 5], x), ([0, 0, 50], x)]);
        assert_eq!(
            click(&w, true, Some([0, 0, 0]), [0, 0, 5], true),
            Click::Forget
        );
        assert_eq!(click(&w, false, None, [0, 0, 0], true), Click::Remember);
        assert_eq!(
            click(&w, false, Some([0, 0, 0]), [0, 0, 5], true),
            Click::Connect([0, 0, 0])
        );
        // Not a shaft.
        assert_eq!(
            click(&w, false, Some([0, 0, 0]), [1, 0, 0], true),
            Click::Fail { forget: false }
        );
        // Too far from the first (40 or more): it is forgotten and this one remembered.
        assert_eq!(
            click(&w, false, Some([0, 0, 0]), [0, 0, 50], true),
            Click::Remember
        );
        // The first is gone.
        assert_eq!(
            click(&w, false, Some([0, 0, 1]), [7, 0, 7], true),
            Click::Fail { forget: true }
        );
        // In reach of the first but not connecting: refused, the first kept.
        let w = world(&[([0, 0, 0], x), ([0, 0, 30], x)]);
        assert_eq!(
            click(&w, false, Some([0, 0, 0]), [0, 0, 30], true),
            Click::Refused
        );
        assert_eq!(sound_at([0, 0, 0], [0, 0, 5]), [0, 0, 2]);
        // The names read back as the belt's states.
        for slope in [
            Slope::Horizontal,
            Slope::Upward,
            Slope::Downward,
            Slope::Vertical,
            Slope::Sideways,
        ] {
            for part in [Part::Start, Part::Middle, Part::End, Part::Pulley] {
                let props = [
                    ("facing", "east"),
                    ("slope", slope_name(slope)),
                    ("part", part_name(part)),
                ];
                assert_eq!(
                    Segment::read(props.into_iter()),
                    Some(Segment {
                        facing: Dir::East,
                        slope,
                        part
                    })
                );
            }
        }
    }
}
