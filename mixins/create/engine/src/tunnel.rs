//! Create's andesite tunnel (`BeltTunnelBlock`, `BeltTunnelBlockEntity`, `BeltTunnelShapes`,
//! `BeltTunnelInteractionHandler`): a cover on a cased belt segment whose shape follows the belts
//! and inputs beside the segment; as a junction it hands one item of each stack that enters its
//! segment to each side's belt or depot, keeping one on the belt.

use crate::kinetics::{Axis, Dir};

/// `BeltTunnelBlock.Shape`, by its state value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Straight,
    Window,
    Closed,
    TLeft,
    TRight,
    Cross,
}

impl Shape {
    pub const ALL: [Shape; 6] = [
        Shape::Straight,
        Shape::Window,
        Shape::Closed,
        Shape::TLeft,
        Shape::TRight,
        Shape::Cross,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Shape::Straight => "straight",
            Shape::Window => "window",
            Shape::Closed => "closed",
            Shape::TLeft => "t_left",
            Shape::TRight => "t_right",
            Shape::Cross => "cross",
        }
    }

    pub fn read(name: &str) -> Option<Shape> {
        Shape::ALL.into_iter().find(|s| s.name() == name)
    }

    /// `hasWindow`.
    pub fn has_window(self) -> bool {
        matches!(self, Shape::Window | Shape::Closed)
    }

    /// `isStraight`.
    pub fn straight(self) -> bool {
        self.has_window() || self == Shape::Straight
    }

    /// `isJunction`.
    pub fn junction(self) -> bool {
        matches!(self, Shape::Cross | Shape::TLeft | Shape::TRight)
    }
}

/// `Direction.get(POSITIVE, axis).getClockWise()`: the tunnel's left side.
pub fn left(axis: Axis) -> Dir {
    if axis == Axis::X {
        Dir::South
    } else {
        Dir::West
    }
}

/// `getTunnelState`: the shape from the outputs beside the belt segment (`hasValidOutput` to the
/// left and right) and, when straight, whether a window fits (`canHaveWindow`).
pub fn shape_for(on_left: bool, on_right: bool, can_window: bool) -> Shape {
    match (on_left, on_right) {
        (true, true) => Shape::Cross,
        (true, false) => Shape::TLeft,
        (false, true) => Shape::TRight,
        _ if can_window => Shape::Window,
        _ => Shape::Straight,
    }
}

/// `BeltTunnelBlock.updateShape`: the state kept when its axis and windowedness still fit (a
/// closed window stays closed), else the new one.
pub fn updated(now: (Axis, Shape), fresh: (Axis, Shape)) -> (Axis, Shape) {
    if now.0 == fresh.0 && now.1.has_window() == fresh.1.has_window() {
        now
    } else {
        fresh
    }
}

/// `updateTunnelConnections`' `sides`: the ends along its axis and the open sides across it, in
/// `Iterate.horizontalDirections` order (south, west, north, east).
pub fn sides(axis: Axis, shape: Shape) -> Vec<Dir> {
    [Dir::South, Dir::West, Dir::North, Dir::East]
        .into_iter()
        .filter(|d| {
            if d.axis() == axis {
                return true;
            }
            let positive = d.positive() ^ (d.axis() == Axis::Z);
            !(shape.straight()
                || positive && shape == Shape::TLeft
                || !positive && shape == Shape::TRight)
        })
        .collect()
}

/// `BeltTunnelShapes.getShape` in pixels: the block from 5 px below (over the belt) to the top,
/// less a 12 px wide, 15 px high opening on each open side.
pub fn boxes(axis: Axis, shape: Shape) -> Vec<[f64; 6]> {
    let open = sides(axis, shape);
    let mut b = vec![[2., -5., 2., 14., 16., 14.]];
    for (x, z) in [(0., 0.), (14., 0.), (0., 14.), (14., 14.)] {
        b.push([x, -5., z, x + 2., 16., z + 2.]);
    }
    for d in [Dir::North, Dir::South, Dir::West, Dir::East] {
        let bottom = if open.contains(&d) { 10. } else { -5. };
        b.push(match d {
            Dir::North => [2., bottom, 0., 14., 16., 2.],
            Dir::South => [2., bottom, 14., 14., 16., 16.],
            Dir::West => [0., bottom, 2., 2., 16., 14.],
            _ => [14., bottom, 2., 16., 16., 14.],
        });
    }
    b
}

/// Whether an andesite tunnel splits a stack of `count` moving along `movement` (`count > 1`, a
/// junction on the belt's axis).
pub fn splits(axis: Axis, shape: Shape, movement: Dir, count: u8) -> bool {
    count > 1 && shape.junction() && movement.axis() == axis
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_follow_the_outputs_and_open_their_sides() {
        // A tunnel on a belt along x: south is its left.
        assert_eq!(shape_for(true, false, false), Shape::TLeft);
        assert_eq!(
            sides(Axis::X, Shape::TLeft),
            vec![Dir::South, Dir::West, Dir::East]
        );
        assert_eq!(
            sides(Axis::X, Shape::TRight),
            vec![Dir::West, Dir::North, Dir::East]
        );
        assert_eq!(
            sides(Axis::Z, Shape::TLeft),
            vec![Dir::South, Dir::West, Dir::North]
        );
        assert_eq!(sides(Axis::Z, Shape::Cross).len(), 4);
        assert_eq!(sides(Axis::Z, Shape::Window), vec![Dir::South, Dir::North]);
        // A closed window stays closed while it still is a straight tunnel with a window.
        let closed = (Axis::Z, Shape::Closed);
        assert_eq!(updated(closed, (Axis::Z, Shape::Window)), closed);
        assert_eq!(
            updated(closed, (Axis::Z, Shape::Cross)),
            (Axis::Z, Shape::Cross)
        );
        // Junctions split stacks moving along them.
        assert!(splits(Axis::X, Shape::Cross, Dir::East, 2));
        assert!(!splits(Axis::X, Shape::Cross, Dir::East, 1));
        assert!(!splits(Axis::X, Shape::Window, Dir::East, 5));
        // The open sides' walls start 10 px up; closed ones reach down over the belt.
        let b = boxes(Axis::X, Shape::TLeft);
        assert!(
            b.contains(&[2., 10., 14., 14., 16., 16.]) && b.contains(&[2., -5., 0., 14., 16., 2.])
        );
    }
}
