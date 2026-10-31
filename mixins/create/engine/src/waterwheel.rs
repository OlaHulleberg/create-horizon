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
}
