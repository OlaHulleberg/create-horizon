//! Water wheels, after Create's `WaterWheelBlockEntity`: the flow around a wheel turns it one way
//! or the other at 8 RPM (4 for a large one).

use crate::kinetics::{Axis, Dir};

/// `WaterWheelBlockEntity.SMALL_OFFSETS`: the four blocks beside a wheel, across its axis.
pub fn small_offsets(axis: Axis) -> [[i32; 3]; 4] {
    let mut out = [[0; 3]; 4];
    let mut i = 0;
    for d in Dir::ALL {
        if d.axis() != axis {
            out[i] = d.offset();
            i += 1;
        }
    }
    out
}

/// `WaterWheelBlockEntity.LARGE_OFFSETS`: two blocks out from a large wheel across its axis, and
/// the blocks beside those in the wheel's plane (12 in all).
pub fn large_offsets(axis: Axis) -> Vec<[i32; 3]> {
    let mut out = Vec::with_capacity(12);
    for d in Dir::ALL {
        if d.axis() == axis {
            continue;
        }
        let o = d.offset();
        let central = [o[0] * 2, o[1] * 2, o[2] * 2];
        out.push(central);
        for d2 in Dir::ALL {
            if d2.axis() == axis || d2.axis() == d.axis() {
                continue;
            }
            let p = d2.offset();
            out.push([central[0] + p[0], central[1] + p[1], central[2] + p[2]]);
        }
    }
    out
}

/// `Direction.getClockWise(axis)`: a quarter turn about `axis` (a direction along it stays).
pub fn clockwise(d: Dir, axis: Axis) -> Dir {
    use Dir::*;
    match axis {
        Axis::X => match d {
            Down => South,
            South => Up,
            Up => North,
            North => Down,
            other => other,
        },
        Axis::Y => match d {
            North => East,
            East => South,
            South => West,
            West => North,
            other => other,
        },
        Axis::Z => match d {
            Down => West,
            West => Up,
            Up => East,
            East => Down,
            other => other,
        },
    }
}

/// `LargeWaterWheelBlock.tick`: the eight structure blocks around a large wheel on `axis`, as
/// offsets from it, each with the facing it is placed with (toward the wheel, or toward the
/// structure block beside the wheel for a corner).
pub fn structure(axis: Axis) -> Vec<([i32; 3], Dir)> {
    let mut out = Vec::with_capacity(8);
    for side in Dir::ALL {
        if side.axis() == axis {
            continue;
        }
        for secondary in [false, true] {
            let target = if secondary {
                clockwise(side, axis)
            } else {
                side
            };
            let base = if secondary { side.offset() } else { [0; 3] };
            let t = target.offset();
            out.push((
                [base[0] + t[0], base[1] + t[1], base[2] + t[2]],
                target.opposite(),
            ));
        }
    }
    out
}

/// `LargeWaterWheelBlock.getStateForPlacement`: the eight blocks around it in its plane must be
/// replaceable for it to be placed.
pub fn plane(axis: Axis) -> Vec<[i32; 3]> {
    let mut out = Vec::with_capacity(8);
    for x in -1..=1 {
        for y in -1..=1 {
            for z in -1..=1 {
                let o = [x, y, z];
                if Axis::of(o, axis) != 0 || o == [0, 0, 0] {
                    continue;
                }
                out.push(o);
            }
        }
    }
    out
}

/// Catnip's `VecHelper.rotate(v, 90, axis)`.
fn turn_90(v: [f64; 3], axis: Axis) -> [f64; 3] {
    let [x, y, z] = v;
    match axis {
        Axis::X => [x, -z, y],
        Axis::Y => [z, y, -x],
        Axis::Z => [-y, x, z],
    }
}

/// `WaterWheelBlockEntity.determineAndApplyFlowScore`: each neighbour whose flow (in the wheel's
/// plane) runs along the wheel's turning direction there counts +1, against it -1.
pub fn flow_score(axis: Axis, samples: &[([i32; 3], [f64; 3])]) -> i32 {
    let plane = match axis {
        Axis::X => [0.0, 1.0, 1.0],
        Axis::Y => [1.0, 0.0, 1.0],
        Axis::Z => [1.0, 1.0, 0.0],
    };
    let mut score = 0;
    for (offset, flow) in samples {
        let f: [f64; 3] = std::array::from_fn(|i| flow[i] * plane[i]);
        let len = f.iter().map(|c| c * c).sum::<f64>().sqrt();
        if len == 0.0 {
            continue;
        }
        let f = f.map(|c| c / len);
        let n = offset.map(f64::from);
        let nl = n.iter().map(|c| c * c).sum::<f64>().sqrt();
        let positive = turn_90(n.map(|c| c / nl), axis);
        let dot: f64 = (0..3).map(|i| f[i] * positive[i]).sum();
        if dot.abs() > 0.5 {
            score += dot.signum() as i32;
        }
    }
    score
}

/// `WaterWheelBlockEntity.getGeneratedSpeed`: `clamp(score, -1, 1) * 8 / size`.
pub fn generated_speed(score: i32, size: i32) -> f32 {
    (score.clamp(-1, 1) * 8) as f32 / size as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn water_falling_past_one_side_turns_the_wheel() {
        // A wheel on the x axis; water falling (-y) on its north side (z - 1).
        let north = [0, 0, -1];
        let down = [0.0, -1.0, 0.0];
        // The positive turning direction at the north neighbour: rotate (0,0,-1) by 90 about x
        // gives (0, 1, 0): falling water runs against it.
        assert_eq!(flow_score(Axis::X, &[(north, down)]), -1);
        assert_eq!(
            flow_score(Axis::X, &[([0, 0, 1], down)]),
            1,
            "the south side turns it the other way"
        );
        assert_eq!(
            flow_score(Axis::X, &[(north, down), ([0, 0, 1], down)]),
            0,
            "both sides cancel"
        );
        // Flow along the axis does not count.
        assert_eq!(flow_score(Axis::X, &[(north, [1.0, 0.0, 0.0])]), 0);
        assert_eq!(generated_speed(-3, 1), -8.0);
        assert_eq!(generated_speed(1, 2), 4.0);
        assert_eq!(small_offsets(Axis::Y).len(), 4);
    }

    #[test]
    fn a_large_wheel_reads_twelve_blocks_and_builds_its_frame() {
        let offsets = large_offsets(Axis::Z);
        assert_eq!(offsets.len(), 12);
        assert!(offsets.contains(&[0, 2, 0]) && offsets.contains(&[1, -2, 0]));
        assert!(offsets.iter().all(|o| o[2] == 0));
        // Water falling past the north side of an x wheel two out.
        let down = [0.0, -1.0, 0.0];
        let samples: Vec<_> = large_offsets(Axis::X)
            .into_iter()
            .filter(|o| o[2] == -2)
            .map(|o| (o, down))
            .collect();
        assert_eq!(samples.len(), 3);
        assert_eq!(
            flow_score(Axis::X, &samples),
            -3,
            "the score sums; the speed clamps it"
        );
        assert_eq!(generated_speed(-1, 2), -4.0);
        // The frame: the eight blocks of the plane, the sides facing the wheel, the corners
        // facing the side block they hang on.
        let s = structure(Axis::Y);
        assert_eq!(s.len(), 8);
        let mut at: Vec<[i32; 3]> = s.iter().map(|x| x.0).collect();
        at.sort();
        let mut p = plane(Axis::Y);
        p.sort();
        assert_eq!(at, p);
        assert!(s.contains(&([0, 0, -1], Dir::South)));
        // North's corner: north then clockwise (east) gives (1, 0, -1), facing west.
        assert!(s.contains(&([1, 0, -1], Dir::West)));
        assert_eq!(clockwise(Dir::Down, Axis::X), Dir::South);
        assert_eq!(clockwise(Dir::Up, Axis::Z), Dir::East);
    }
}
