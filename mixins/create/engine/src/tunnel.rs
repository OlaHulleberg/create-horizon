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

/// `BrassTunnelBlockEntity.SelectionMode`, picked by the wheel or the value board.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Split,
    ForcedSplit,
    RoundRobin,
    ForcedRoundRobin,
    PreferNearest,
}

impl Mode {
    /// The modes in Create's order (`RANDOMIZE` and `SYNCHRONIZE` are not ported).
    pub const ALL: [Mode; 5] = [
        Mode::Split,
        Mode::ForcedSplit,
        Mode::RoundRobin,
        Mode::ForcedRoundRobin,
        Mode::PreferNearest,
    ];

    pub fn index(self) -> usize {
        Mode::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }

    /// Its name (`create.tunnel.selection_mode.*`).
    pub fn label(self) -> &'static str {
        match self {
            Mode::Split => "Split",
            Mode::ForcedSplit => "Forced Split",
            Mode::RoundRobin => "Round Robin",
            Mode::ForcedRoundRobin => "Forced Round Robin",
            Mode::PreferNearest => "Prefer Nearest",
        }
    }

    /// Scrolled `steps` through the options, held at the ends (`ScrollOptionBehaviour`).
    pub fn scrolled(self, steps: i32) -> Mode {
        let i = (self.index() as i32 + steps).clamp(0, Mode::ALL.len() as i32 - 1);
        Mode::ALL[i as usize]
    }

    /// The mode at `SelectionMode` ordinal `i`; None for `RANDOMIZE`, `SYNCHRONIZE` (not ported)
    /// or past the end.
    pub fn from_option(i: i32) -> Option<Mode> {
        Mode::ALL.get(usize::try_from(i).ok()?).copied()
    }
}

/// The names of all of Create's `SelectionMode`s in order (`create.tunnel.selection_mode.*`):
/// the value board shows all seven, as Create's does.
pub const OPTIONS: [&str; 7] = [
    "Split",
    "Forced Split",
    "Round Robin",
    "Forced Round Robin",
    "Prefer Nearest",
    "Randomize",
    "Synchronize Inputs",
];

/// `BrassTunnelBlockEntity.hasDistributionBehaviour` (its mode's value box is active only then)
/// for a tunnel that is not connected to others (`connectedLeft`/`Right` are not ported): a flap
/// on a side across its axis (`updateTunnelConnections`), which a tunnel beside it (`tunnel(d)`)
/// occludes. The extended belt funnel that also occludes one is not modelled.
pub fn distributes(axis: Axis, shape: Shape, tunnel: impl Fn(Dir) -> bool) -> bool {
    sides(axis, shape)
        .into_iter()
        .any(|d| d.axis() != axis && !tunnel(d))
}

/// A brass tunnel's typed state: the stack it holds to hand out, the side it came in through,
/// `distributionProgress` (-1 to look for outputs), whether it just arrived, and
/// `previousOutputIndex`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Brass {
    pub stack: Option<(String, u8)>,
    pub entered_from: Option<Dir>,
    pub progress: i32,
    pub new_item: bool,
    pub previous_output: usize,
    /// `selectionMode`.
    pub mode: Mode,
}

impl Default for Brass {
    fn default() -> Brass {
        Brass {
            stack: None,
            entered_from: None,
            progress: -1,
            new_item: false,
            previous_output: 0,
            mode: Mode::Split,
        }
    }
}

/// `brassTunnelTimer`: ticks between distributions.
pub const BRASS_TIMER: i32 = 10;

/// `BrassTunnelBlockEntity.distribute`: hands `count` items out to `targets` outputs from
/// `previous_output`, as the mode says; `insert(target, count, simulate)` is
/// `insertIntoTunnel` (None: that output takes nothing of it, else what it leaves). The items
/// left over.
pub fn distribute(
    count: u8,
    targets: usize,
    previous_output: usize,
    mode: Mode,
    mut insert: impl FnMut(usize, u8, bool) -> Option<u8>,
) -> u8 {
    if targets == 0 {
        return count;
    }
    let start = if mode == Mode::PreferNearest {
        0
    } else {
        previous_output % targets
    };
    let force = matches!(mode, Mode::ForcedRoundRobin | Mode::ForcedSplit);
    let split = matches!(mode, Mode::ForcedSplit | Mode::Split);
    let robin = matches!(mode, Mode::ForcedRoundRobin | Mode::RoundRobin);
    let mut distributed: Vec<(usize, u8)> = Vec::new();
    let mut full: Vec<usize> = Vec::new();
    let mut to_distribute = count;
    for again in [true, false] {
        let mut this_cycle = to_distribute;
        let mut remaining = targets;
        let mut leftovers: u8 = 0;
        for simulate in [true, false] {
            if remaining == 0 {
                break;
            }
            leftovers = 0;
            let mut index = start;
            let size = to_distribute;
            let split_size = size as usize / remaining;
            let mut split_rem = (size as usize % remaining) as i32;
            this_cycle = to_distribute;
            if !(force || split) && simulate {
                continue;
            }
            let mut visited = 0;
            while visited < targets {
                let t = index;
                index = (index + 1) % targets;
                visited += 1;
                if full.contains(&t) {
                    if split && simulate {
                        remaining -= 1;
                    }
                    continue;
                }
                let n = if split {
                    (split_size + usize::from(split_rem > 0)) as u8
                } else {
                    size
                };
                // Grown by what it was already given, to tell whether it is full after it.
                let before = distributed.iter().find(|d| d.0 == t).map(|d| d.1);
                let output = n.saturating_add(before.unwrap_or(0));
                let left = insert(t, output, true);
                let whole = if before.is_some() { n + 1 } else { n };
                if left.is_none() || left == Some(whole) {
                    if force {
                        return count;
                    }
                    if split && simulate {
                        remaining -= 1;
                    }
                    if !simulate {
                        full.push(t);
                    }
                    if robin {
                        break;
                    }
                    continue;
                }
                let left = left.unwrap_or(0);
                if left > 0 && !simulate {
                    full.push(t);
                }
                if !simulate {
                    distributed.retain(|d| d.0 != t);
                    distributed.push((t, output - left));
                }
                leftovers += left;
                this_cycle = this_cycle.saturating_sub(n);
                if this_cycle == 0 {
                    break;
                }
                split_rem -= 1;
                if !split {
                    break;
                }
            }
        }
        to_distribute = this_cycle + leftovers;
        if leftovers == 0 && again {
            break;
        }
        if !split {
            break;
        }
    }
    let failed: u8 = distributed
        .iter()
        .map(|&(t, n)| insert(t, n, false).unwrap_or(n))
        .sum();
    to_distribute + failed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_brass_tunnel_splits_a_stack_evenly() {
        // Three outputs taking anything: 10 items as 4, 3, 3.
        let mut got = [0u8; 3];
        let left = distribute(10, 3, 0, Mode::Split, |t, n, sim| {
            if !sim {
                got[t] += n;
            }
            Some(0)
        });
        assert_eq!((left, got), (0, [4, 3, 3]));
        // One output refusing: the other two share it.
        let mut got = [0u8; 3];
        let left = distribute(9, 3, 0, Mode::Split, |t, n, sim| {
            if t == 1 {
                return None;
            }
            if !sim {
                got[t] += n;
            }
            Some(0)
        });
        assert_eq!((left, got), (0, [5, 0, 4]));
        // Outputs taking at most 2: what is left stays in the tunnel.
        let left = distribute(9, 3, 0, Mode::Split, |_, n, _| Some(n.saturating_sub(2)));
        assert_eq!(left, 3);
    }

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
        // Brass tunnels distribute with an open side across their axis not against a tunnel.
        assert!(distributes(Axis::X, Shape::TLeft, |_| false));
        assert!(!distributes(Axis::X, Shape::TLeft, |d| d == Dir::South));
        assert!(!distributes(Axis::X, Shape::Window, |_| false));
        assert_eq!(Mode::from_option(4), Some(Mode::PreferNearest));
        assert_eq!((Mode::from_option(5), Mode::from_option(-1)), (None, None));
        assert_eq!(OPTIONS[Mode::RoundRobin.index()], Mode::RoundRobin.label());
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
