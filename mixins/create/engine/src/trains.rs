//! Create's trains, first slice (`Train`, `Navigation`, `StationBlockEntity`): a train
//! assembled at a station on a straight track runs to another station on the same track, its
//! speed controlled as Create's navigation does (top speed, acceleration, braking distance, the
//! slow approach over the last ten blocks). Curves, junctions, signals and schedules are later
//! slices.

use crate::kinetics::Axis;

/// `trainTopSpeed` (28 m/s) and `trainAcceleration` (3 m/s²) in blocks a tick.
pub const TOP_SPEED: f64 = 28.0 / 20.0;
pub const ACCELERATION: f64 = 3.0 / 400.0;

/// A straight run of track along an axis: its fixed coordinates and the first and last
/// coordinate along it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub axis: Axis,
    pub fixed: [i32; 3],
    pub from: i32,
    pub to: i32,
}

impl Line {
    pub fn along(axis: Axis, p: [i32; 3]) -> i32 {
        match axis {
            Axis::X => p[0],
            Axis::Y => p[1],
            Axis::Z => p[2],
        }
    }

    /// The straight track through `at` on `axis`: as far both ways as `is_track` holds.
    pub fn trace(at: [i32; 3], axis: Axis, is_track: impl Fn([i32; 3]) -> bool) -> Line {
        let step = |p: [i32; 3], d: i32| match axis {
            Axis::X => [p[0] + d, p[1], p[2]],
            Axis::Y => [p[0], p[1] + d, p[2]],
            Axis::Z => [p[0], p[1], p[2] + d],
        };
        let (mut lo, mut hi) = (at, at);
        while is_track(step(lo, -1)) {
            lo = step(lo, -1);
        }
        while is_track(step(hi, 1)) {
            hi = step(hi, 1);
        }
        Line {
            axis,
            // Its low end, so lines traced from anywhere on the same track are equal.
            fixed: lo,
            from: Line::along(axis, lo),
            to: Line::along(axis, hi),
        }
    }

    pub fn contains(&self, p: [i32; 3]) -> bool {
        let same = |a: usize| a == self.axis_index() || p[a] == self.fixed[a];
        (0..3).all(same) && (self.from..=self.to).contains(&Line::along(self.axis, p))
    }

    fn axis_index(&self) -> usize {
        match self.axis {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        }
    }
}

/// A train on a line: where its first bogey is along the line, its speed (blocks a tick, signed
/// along the axis) and where it is going.
#[derive(Clone, Debug, PartialEq)]
pub struct Train {
    pub line: Line,
    pub position: f64,
    pub speed: f64,
    pub destination: Option<f64>,
}

/// What a tick did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
    Still,
    Moving,
    Arrived,
}

impl Train {
    /// `Navigation.tick` and `Train.tick` on a straight line, no signals: the speed toward the
    /// destination, then the move. Arrived within half a block (`distanceToDestination < .5`).
    pub fn tick(&mut self) -> Run {
        let Some(dest) = self.destination else {
            self.speed = 0.0;
            return Run::Still;
        };
        let to_go = dest - self.position;
        let dir = if to_go < 0.0 { -1.0 } else { 1.0 };
        let distance = to_go.abs();
        if distance < 0.5 {
            self.destination = None;
            self.speed = 0.0;
            return Run::Arrived;
        }
        // Always overshoot a little so the point crosses the target.
        let target = distance + 0.25;
        let speed = self.speed.abs();
        if target - speed < 1.0 / 32.0 {
            self.speed = target.max(1.0 / 32.0) * dir;
        } else {
            let braking = speed * speed / (2.0 * ACCELERATION);
            let max_approach = TOP_SPEED * (target / 10.0);
            if target < 10.0 && speed > max_approach {
                self.speed = (speed + (max_approach - speed) * 0.5) * dir;
            } else {
                let target_speed = if target > braking { TOP_SPEED } else { 0.0 };
                // `approachTargetSpeed`.
                let s = if speed < target_speed {
                    (speed + ACCELERATION).min(target_speed)
                } else {
                    (speed - ACCELERATION).max(target_speed)
                };
                self.speed = s * dir;
            }
        }
        self.position += self.speed;
        Run::Moving
    }
}

/// `Navigation`'s signal check on a straight line: the nearest signal ahead of the train's
/// `front` (moving `dir`, ±1) and short of where its front stops (`dest_front`) whose section
/// beyond (to the next signal, or the line's end) another train's span (`others`, along the
/// line) overlaps: the train must wait before it (`waitingForSignal`).
pub fn blocking_signal(
    front: f64,
    dir: f64,
    dest_front: f64,
    signals: &[f64],
    others: &[(f64, f64)],
) -> Option<f64> {
    let mut ahead: Vec<f64> = signals
        .iter()
        .copied()
        .filter(|s| (s - front) * dir > 0.0 && (dest_front - s) * dir > 0.0)
        .collect();
    ahead.sort_by(|a, b| ((a - front) * dir).total_cmp(&((b - front) * dir)));
    let s = *ahead.first()?;
    // The section beyond it: up to the next signal that way.
    let next = signals
        .iter()
        .copied()
        .filter(|t| (t - s) * dir > 0.0)
        .min_by(|a, b| ((a - s) * dir).total_cmp(&((b - s) * dir)));
    let (lo, hi) = match next {
        Some(t) => (s.min(t), s.max(t)),
        None if dir > 0.0 => (s, f64::INFINITY),
        None => (f64::NEG_INFINITY, s),
    };
    others.iter().any(|&(a, b)| a < hi && b > lo).then_some(s)
}

impl Train {
    /// A tick that stops at `stop` (a position short of the destination, before a red signal)
    /// without forgetting where it is going: true when it waits there.
    pub fn tick_until(&mut self, stop: Option<f64>) -> Run {
        let Some(stop) = stop else {
            return self.tick();
        };
        let dest = self.destination;
        self.destination = Some(stop);
        let run = self.tick();
        self.destination = dest;
        if run == Run::Arrived {
            self.speed = 0.0;
            return Run::Still;
        }
        run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_runs_as_far_as_the_track() {
        let l = Line::trace([5, 64, 0], Axis::X, |p| {
            (0..=40).contains(&p[0]) && p[1] == 64 && p[2] == 0
        });
        assert_eq!((l.from, l.to), (0, 40));
        assert!(l.contains([12, 64, 0]) && !l.contains([12, 64, 1]));
        let from_elsewhere = Line::trace([30, 64, 0], Axis::X, |p| {
            (0..=40).contains(&p[0]) && p[1] == 64 && p[2] == 0
        });
        assert_eq!(l, from_elsewhere);
    }

    #[test]
    fn a_train_waits_at_a_red_signal_and_goes_on_when_it_clears() {
        // Signals at 100 and 200; another train stands between 150 and 160.
        assert_eq!(
            blocking_signal(10.0, 1.0, 300.0, &[100.0, 200.0], &[(150.0, 160.0)]),
            Some(100.0)
        );
        assert_eq!(
            blocking_signal(10.0, 1.0, 300.0, &[100.0, 200.0], &[(250.0, 260.0)]),
            None,
            "a green signal"
        );
        assert_eq!(
            blocking_signal(10.0, 1.0, 90.0, &[100.0, 200.0], &[(150.0, 160.0)]),
            None,
            "stops short of it anyway"
        );
        assert_eq!(
            blocking_signal(300.0, -1.0, 0.0, &[100.0, 200.0], &[(150.0, 160.0)]),
            Some(200.0),
            "going the other way"
        );
        let line = Line {
            axis: Axis::X,
            fixed: [0, 64, 0],
            from: 0,
            to: 500,
        };
        let mut t = Train {
            line,
            position: 0.0,
            speed: 0.0,
            destination: Some(400.0),
        };
        for _ in 0..2000 {
            t.tick_until(Some(98.0));
        }
        assert!(
            (t.position - 98.0).abs() < 0.5,
            "waits at the signal: {}",
            t.position
        );
        assert_eq!(t.destination, Some(400.0), "still going there");
        while t.tick() != Run::Arrived {}
        assert!((t.position - 400.0).abs() < 0.5);
    }

    #[test]
    fn a_train_speeds_up_cruises_and_stops_at_the_station() {
        let line = Line {
            axis: Axis::X,
            fixed: [0, 64, 0],
            from: 0,
            to: 500,
        };
        let mut t = Train {
            line,
            position: 0.0,
            speed: 0.0,
            destination: Some(400.0),
        };
        let mut ticks = 0;
        let mut top = 0.0f64;
        while t.tick() != Run::Arrived {
            ticks += 1;
            top = top.max(t.speed);
            assert!(ticks < 2000, "never arrived");
        }
        assert!((t.position - 400.0).abs() < 0.5, "{}", t.position);
        assert!((top - TOP_SPEED).abs() < 1e-9, "reaches 28 m/s: {top}");
        // 187 ticks (131 blocks) to reach top speed, as long to brake, the rest at 1.4 a tick.
        assert!((440..520).contains(&ticks), "{ticks}");
    }
}
