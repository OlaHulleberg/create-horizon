//! Create's controller rail (`ControllerRailBlock`): a straight rail whose `power` (0-15) is
//! the strongest signal next to it, else the signal reached along the controller rails it is
//! lined up with (`calculatePower`), and which sets a minecart passing it to that share of the
//! cart's top speed in the way it points (`onMinecartPass`, `cart_motion`, which the glue runs
//! for each cart on one).

use crate::kinetics::Dir;

/// `BlockStateProperties.RAIL_SHAPE_STRAIGHT`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Shape {
    #[default]
    NorthSouth,
    EastWest,
    AscendingEast,
    AscendingWest,
    AscendingNorth,
    AscendingSouth,
}

impl Shape {
    pub fn read(v: &str) -> Option<Shape> {
        Some(match v {
            "north_south" => Shape::NorthSouth,
            "east_west" => Shape::EastWest,
            "ascending_east" => Shape::AscendingEast,
            "ascending_west" => Shape::AscendingWest,
            "ascending_north" => Shape::AscendingNorth,
            "ascending_south" => Shape::AscendingSouth,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Shape::NorthSouth => "north_south",
            Shape::EastWest => "east_west",
            Shape::AscendingEast => "ascending_east",
            Shape::AscendingWest => "ascending_west",
            Shape::AscendingNorth => "ascending_north",
            Shape::AscendingSouth => "ascending_south",
        }
    }

    pub fn ascending(self) -> bool {
        !matches!(self, Shape::NorthSouth | Shape::EastWest)
    }

    /// `getPointingTowards`.
    pub fn pointing(self) -> Dir {
        match self {
            Shape::AscendingWest | Shape::EastWest => Dir::West,
            Shape::AscendingEast => Dir::East,
            Shape::AscendingSouth => Dir::South,
            _ => Dir::North,
        }
    }

    /// `isReversedSlope`.
    fn reversed_slope(self) -> bool {
        matches!(self, Shape::AscendingSouth | Shape::AscendingEast)
    }
}

/// A controller rail's state as its rules read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rail {
    pub shape: Shape,
    pub backwards: bool,
    pub power: u8,
}

impl Rail {
    /// `isStateBackwards`.
    pub fn is_backwards(self) -> bool {
        self.backwards ^ self.shape.reversed_slope()
    }

    /// `getAccelerationVector`.
    pub fn acceleration(self) -> Dir {
        let p = self.shape.pointing();
        if self.is_backwards() { p.opposite() } else { p }
    }
}

/// `getStateForPlacement`: `BaseRailBlock`'s shape across the player's horizontal facing
/// (east-west when facing along x), `backwards` when the player faces a positive way.
pub fn placement(facing: Dir) -> (Shape, bool) {
    let shape = if matches!(facing, Dir::East | Dir::West) {
        Shape::EastWest
    } else {
        Shape::NorthSouth
    };
    let positive = matches!(facing, Dir::East | Dir::South | Dir::Up);
    (shape, positive)
}

/// What the power rule reads of the world.
pub trait Probe {
    /// `getBestNeighborSignal`.
    fn signal(&self, pos: [i32; 3]) -> i32;
    /// The controller rail at `pos`, if one is there.
    fn rail(&self, pos: [i32; 3]) -> Option<Rail>;
}

fn step(p: [i32; 3], d: Dir, sign: i32) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0] * sign, p[1] + o[1] * sign, p[2] + o[2] * sign]
}

/// `findNextRail`: the controller rail lined up with the one at `from`, one block on (back when
/// `reversed`), level, below or (from a slope) above it.
fn next_rail(probe: &impl Probe, from: [i32; 3], reversed: bool) -> Option<[i32; 3]> {
    let current = probe.rail(from)?;
    let accel = current.acceleration();
    let base = step(from, accel, if reversed { -1 } else { 1 });
    // `Iterate.hereBelowAndAbove`.
    for test in [
        base,
        [base[0], base[1] - 1, base[2]],
        [base[0], base[1] + 1, base[2]],
    ] {
        if test[1] > from[1] && !current.shape.ascending() {
            continue;
        }
        if probe.rail(test).is_some_and(|r| r.acceleration() == accel) {
            return Some(test);
        }
    }
    None
}

/// `calculatePower` for the controller rail at `pos`.
pub fn power(probe: &impl Probe, pos: [i32; 3]) -> u8 {
    let own = probe.signal(pos);
    if own != 0 {
        return own.clamp(0, 15) as u8;
    }
    let walk = |reversed: bool| {
        let (mut distance, mut last, mut power) = (0, pos, 0);
        for _ in 0..15 {
            let Some(next) = next_rail(probe, last, reversed) else {
                break;
            };
            distance += 1;
            last = next;
            power = probe.signal(next);
            if power != 0 {
                break;
            }
        }
        (distance, power)
    };
    let (forward_distance, forward_power) = walk(false);
    let (backwards_distance, backwards_power) = walk(true);
    let p = if forward_distance > 8 && backwards_distance > 8 {
        0
    } else if backwards_power == 0 && forward_distance <= 8 {
        forward_power
    } else if forward_power == 0 && backwards_distance <= 8 {
        backwards_power
    } else if backwards_power != 0 && forward_power != 0 {
        // `Mth.ceil` of the doubles.
        (f64::from(backwards_power * forward_distance + forward_power * backwards_distance)
            / f64::from(forward_distance + backwards_distance))
        .ceil() as i32
    } else {
        0
    };
    p.clamp(0, 15) as u8
}

/// `onMinecartPass` on the server: a cart moving the rail's way (or still) is set to
/// `power / 15` of its top speed with the rail (`getMaxSpeedWithRail`) along it when that is
/// above 0; otherwise it is pulled to the rail's centre at a sixteenth of the distance a tick,
/// stopping vertical motion (`decelerateCart`). The new motion, and a furnace cart's push.
pub fn cart_motion(
    rail: Rail,
    pos: [i32; 3],
    cart: [f64; 3],
    motion: [f64; 3],
    max_speed: f64,
) -> ([f64; 3], [f64; 2]) {
    let a = rail.acceleration().offset().map(f64::from);
    let target = max_speed * f64::from(rail.power) / 15.0;
    let dot = motion[0] * a[0] + motion[1] * a[1] + motion[2] * a[2];
    let length_sqr = motion[0] * motion[0] + motion[1] * motion[1] + motion[2] * motion[2];
    let push = [a[0], a[2]];
    if (dot >= 0.0 || length_sqr < 0.0001) && target > 0.0 {
        return (a.map(|v| v * target), push);
    }
    let centre = pos.map(|v| f64::from(v) + 0.5);
    // `diff.x / 16f`: a double over a float.
    (
        [
            (centre[0] - cart[0]) / 16.0,
            0.0,
            (centre[2] - cart[2]) / 16.0,
        ],
        push,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct World {
        rails: HashMap<[i32; 3], Rail>,
        signals: HashMap<[i32; 3], i32>,
    }

    impl Probe for World {
        fn signal(&self, pos: [i32; 3]) -> i32 {
            self.signals.get(&pos).copied().unwrap_or(0)
        }
        fn rail(&self, pos: [i32; 3]) -> Option<Rail> {
            self.rails.get(&pos).copied()
        }
    }

    fn line(n: i32) -> World {
        let rail = Rail {
            shape: Shape::NorthSouth,
            ..Rail::default()
        };
        World {
            rails: (0..n).map(|z| ([0, 64, z], rail)).collect(),
            signals: HashMap::new(),
        }
    }

    #[test]
    fn shapes_point_and_turn_back() {
        let r = |shape, backwards| Rail {
            shape,
            backwards,
            power: 0,
        };
        assert_eq!(r(Shape::NorthSouth, false).acceleration(), Dir::North);
        assert_eq!(r(Shape::NorthSouth, true).acceleration(), Dir::South);
        assert_eq!(r(Shape::EastWest, false).acceleration(), Dir::West);
        // A slope up to the south points south, reversed: backwards points it north.
        assert_eq!(r(Shape::AscendingSouth, false).acceleration(), Dir::North);
        assert_eq!(r(Shape::AscendingSouth, true).acceleration(), Dir::South);
        assert_eq!(placement(Dir::South), (Shape::NorthSouth, true));
        assert_eq!(placement(Dir::West), (Shape::EastWest, false));
    }

    #[test]
    fn power_comes_from_beside_or_along_the_line() {
        // A signal of 15 beside the rail at z 0 of a line of 12 running north (-z).
        let mut w = line(12);
        w.signals.insert([0, 64, 0], 15);
        assert_eq!(power(&w, [0, 64, 0]), 15);
        // z 5 finds it 5 back (south is back, toward +z? it points north: back is +z): the
        // forward walk (north, -z) reaches z 0 at distance 5.
        assert_eq!(power(&w, [0, 64, 5]), 15);
        // Further than 8 along: none.
        assert_eq!(power(&w, [0, 64, 9]), 0);
        // Signals both ways: the mean weighted by distance, rounded up.
        w.signals.insert([0, 64, 11], 6);
        // z 4: forward 4 to 15, backwards 7 to 6: ceil((6*4 + 15*7) / 11) = ceil(11.73) = 12.
        assert_eq!(power(&w, [0, 64, 4]), 12);
        // A rail turned the other way breaks the line.
        w.rails.get_mut(&[0, 64, 2]).unwrap().backwards = true;
        assert_eq!(power(&w, [0, 64, 4]), 6);
    }

    #[test]
    fn a_cart_is_set_to_its_share_of_top_speed_or_brought_to_rest() {
        let rail = Rail {
            shape: Shape::EastWest,
            backwards: true,
            power: 15,
        };
        // Backwards east-west points east: a still cart leaves east at top speed.
        let (m, push) = cart_motion(rail, [0, 64, 0], [0.5, 64.0, 0.5], [0.0; 3], 0.4);
        assert_eq!(m, [0.4, 0.0, 0.0]);
        assert_eq!(push, [1.0, 0.0]);
        let half = Rail { power: 5, ..rail };
        let (m, _) = cart_motion(half, [0, 64, 0], [0.5, 64.0, 0.5], [0.1, 0.0, 0.0], 0.4);
        assert!((m[0] - 0.4 / 3.0).abs() < 1e-12);
        // Against it, or unpowered: pulled to the centre.
        let (m, _) = cart_motion(rail, [0, 64, 0], [0.9, 64.0, 0.5], [-0.2, 0.0, 0.0], 0.4);
        assert_eq!(m, [-0.4 / 16.0, 0.0, 0.0]);
        let off = Rail { power: 0, ..rail };
        let (m, _) = cart_motion(off, [0, 64, 0], [0.5, 64.0, 0.1], [0.0; 3], 0.4);
        assert_eq!(m, [0.0, 0.0, 0.4 / 16.0]);
    }
}
