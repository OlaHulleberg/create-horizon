//! Create's curved track (`BezierConnection` and its `Runtime`, with Catnip's `VecHelper.bezier`,
//! `bezierDerivative`, `intersect` and `slerp`): a cubic Bézier between two track ends, its
//! handles sized so a turn follows a circle, its length, the step table that spaces sleepers
//! evenly, and how far along it a train moves for a distance.

pub type Vec3 = [f64; 3];

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: Vec3, s: f64) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}
/// `Vec3.normalize`: zero for a vector shorter than 1e-4.
fn normalize(a: Vec3) -> Vec3 {
    let l = length(a);
    if l < 1.0e-4 {
        [0.0; 3]
    } else {
        scale(a, 1.0 / l)
    }
}
fn lerp(t: f64, from: Vec3, to: Vec3) -> Vec3 {
    add(from, scale(sub(to, from), t))
}
/// `Mth.equal`: within 1e-5.
fn equal(a: f64, b: f64) -> bool {
    (b - a).abs() < 1.0e-5
}

/// `VecHelper.bezier(p1, p2, q1, q2, t)`: ends `p1`, `p2`, handles `q1` (after `p1`), `q2`
/// (before `p2`), by de Casteljau.
pub fn bezier(p1: Vec3, p2: Vec3, q1: Vec3, q2: Vec3, t: f64) -> Vec3 {
    let v1 = lerp(t, p1, q1);
    let v2 = lerp(t, q1, q2);
    let v3 = lerp(t, q2, p2);
    lerp(t, lerp(t, v1, v2), lerp(t, v2, v3))
}

/// `VecHelper.bezierDerivative`.
pub fn bezier_derivative(p1: Vec3, p2: Vec3, q1: Vec3, q2: Vec3, t: f64) -> Vec3 {
    let a = scale(p1, -3.0 * t * t + 6.0 * t - 3.0);
    let b = scale(q1, 9.0 * t * t - 12.0 * t + 3.0);
    let c = scale(q2, -9.0 * t * t + 6.0 * t);
    let d = scale(p2, 3.0 * t * t);
    add(add(a, b), add(c, d))
}

/// `VecHelper.intersect(p1, p2, r, s, Axis.Y)`: where the lines `p1 + t·r` and `p2 + u·s` cross
/// in the horizontal plane, as `(t, u)`; None when parallel.
pub fn intersect(p1: Vec3, p2: Vec3, r: Vec3, s: Vec3) -> Option<(f64, f64)> {
    let q = sub(p2, p1);
    let rcs = r[0] * s[2] - r[2] * s[0];
    if equal(rcs, 0.0) {
        return None;
    }
    let (rd, sd) = (scale(r, 1.0 / rcs), scale(s, 1.0 / rcs));
    Some((q[0] * sd[2] - q[2] * sd[0], q[0] * rd[2] - q[2] * rd[0]))
}

/// `VecHelper.slerp(p, from, to)` as Catnip writes it.
fn slerp(p: f64, from: Vec3, to: Vec3) -> Vec3 {
    let theta = dot(from, to).clamp(-1.0, 1.0).acos();
    let a = scale(from, ((1.0 - p) as f32).sin() as f64 * theta);
    let b = scale(to, ((theta * p) as f32).sin() as f64);
    scale(add(a, b), 1.0 / (theta as f32).sin() as f64)
}

/// A curve between two track ends: where each end is and the direction the track leaves it
/// toward the other (`starts`, `axes`), with what `Runtime` derives from them.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub starts: [Vec3; 2],
    pub axes: [Vec3; 2],
    pub normals: [Vec3; 2],
    /// The handles' far points (`finish1`, `finish2`), their length, and the circle's radius
    /// (0 for an S-bend).
    pub finishes: [Vec3; 2],
    pub handle_length: f64,
    pub radius: f64,
    /// Length over 16 straight pieces (`computeLength`), and `length × 2` segments with
    /// `stepLUT`.
    pub length: f64,
    pub segments: usize,
    pub step_lut: Vec<f32>,
}

impl Curve {
    /// `new Runtime(starts, axes)`.
    pub fn new(starts: [Vec3; 2], axes: [Vec3; 2], normals: [Vec3; 2]) -> Curve {
        let (end1, end2) = (starts[0], starts[1]);
        let (axis1, axis2) = (normalize(axes[0]), normalize(axes[1]));
        let (handle_length, radius) = handles(end1, end2, axis1, axis2);
        let finish1 = add(scale(axis1, handle_length), end1);
        let finish2 = add(scale(axis2, handle_length), end2);
        let at = |t: f64| bezier(end1, end2, finish1, finish2, t);
        let mut len = 0.0;
        let mut previous = end1;
        for i in 0..=16 {
            let p = at(f64::from(i as f32 / 16.0));
            len += length(sub(p, previous));
            previous = p;
        }
        let segments = (len * 2.0) as usize;
        let mut step_lut = vec![0.0f32; segments + 1];
        step_lut[0] = 1.0;
        let mut combined = 0.0f32;
        let mut previous = end1;
        for (i, step) in step_lut.iter_mut().enumerate() {
            let t = i as f32 / segments as f32;
            let p = at(f64::from(t));
            if i > 0 {
                combined += (length(sub(p, previous)) / len) as f32;
                *step = t / combined;
            }
            previous = p;
        }
        Curve {
            starts,
            axes,
            normals,
            finishes: [finish1, finish2],
            handle_length,
            radius,
            length: len,
            segments,
            step_lut,
        }
    }

    /// `getPosition(t)`.
    pub fn position(&self, t: f64) -> Vec3 {
        bezier(
            self.starts[0],
            self.starts[1],
            self.finishes[0],
            self.finishes[1],
            f64::from(t as f32),
        )
    }

    fn derivative(&self, t: f64) -> Vec3 {
        bezier_derivative(
            self.starts[0],
            self.starts[1],
            self.finishes[0],
            self.finishes[1],
            f64::from(t as f32),
        )
    }

    /// `getSegmentT(index)`: the `t` of a sleeper, spaced evenly along the curve.
    pub fn segment_t(&self, index: usize) -> f32 {
        if index == self.segments {
            1.0
        } else {
            index as f32 * self.step_lut[index] / self.segments as f32
        }
    }

    /// `incrementT(t, distance)`: the `t` further along by `distance`, a share of the curve's
    /// length (`distance / (|B'(t)| / length)`).
    pub fn increment_t(&self, t: f64, distance: f64) -> f64 {
        let dx = length(self.derivative(t)) / self.length;
        t + distance / dx
    }

    /// `getNormal(t)`: the track's up, between the ends' normals.
    pub fn normal(&self, t: f64) -> Vec3 {
        let derivative = normalize(self.derivative(t));
        let [fn1, fn2] = self.normals;
        let face = if fn1 == fn2 { fn1 } else { slerp(t, fn1, fn2) };
        let normal = normalize(cross(face, derivative));
        cross(derivative, normal)
    }
}

/// `Runtime.determineHandles`: a turn's handles are `radius × 4/3 tan(π / 2n)` long (a circle
/// split in `n` parts by the turn's angle); an S-bend (no turn) takes the distance its
/// side lines leave, else a third of the span.
fn handles(end1: Vec3, end2: Vec3, axis1: Vec3, axis2: Vec3) -> (f64, f64) {
    let up = [0.0, 1.0, 0.0];
    let (cross1, cross2) = (cross(axis1, up), cross(axis2, up));
    let a1 = (-axis2[2]).atan2(-axis2[0]);
    let a2 = axis1[2].atan2(axis1[0]);
    let circle = 2.0 * std::f64::consts::PI;
    let mut angle = ((a1 - a2) + circle) % circle;
    if (circle - angle).abs() < angle.abs() {
        angle = circle - angle;
    }
    let third = length(sub(end2, end1)) / 3.0;
    if equal(angle, 0.0) {
        if let Some((t, u)) = intersect(end1, end2, axis1, cross2) {
            let (t, u) = (t.abs(), u.abs());
            let (min, max) = (t.min(u), t.max(u));
            if min > 1.2 && max / min > 1.0 && max / min < 3.0 {
                return (max - min, 0.0);
            }
        }
        return (third, 0.0);
    }
    let n = circle / angle;
    let factor = 4.0 / 3.0 * (std::f64::consts::PI / (2.0 * n)).tan();
    let Some((_, u)) = intersect(end1, end2, cross1, cross2) else {
        return (third, 0.0);
    };
    let radius = u.abs();
    let handle = radius * factor;
    (if equal(handle, 0.0) { 1.0 } else { handle }, radius)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UP: Vec3 = [0.0, 1.0, 0.0];

    #[test]
    fn a_quarter_turn_follows_its_circle() {
        // From (0, 0, 0) heading +x, to (8, 0, 8) entered heading +z: the track at the far end
        // leaves toward the first, -z.
        let c = Curve::new(
            [[0.0; 3], [8.0, 0.0, 8.0]],
            [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
            [UP, UP],
        );
        assert!((c.radius - 8.0).abs() < 1e-9, "{}", c.radius);
        // 4/3 tan(π/8) × 8: the classic quarter-circle handle.
        assert!((c.handle_length - 8.0 * 0.552_284_75).abs() < 1e-6);
        // A quarter of a circle of 8 is 12.566; 16 straight pieces fall just short.
        assert!((c.length - 12.566).abs() < 0.02, "{}", c.length);
        assert_eq!(c.segments, 25);
        // The middle lies on the circle around (0, 0, 8).
        let mid = c.position(0.5);
        let r = ((mid[0]).powi(2) + (mid[2] - 8.0).powi(2)).sqrt();
        assert!((r - 8.0).abs() < 0.01, "{r}");
        // Sleepers are spaced evenly: neighbouring ones half a block apart.
        let gaps: Vec<f64> = (0..c.segments)
            .map(|i| {
                let a = c.position(f64::from(c.segment_t(i)));
                let b = c.position(f64::from(c.segment_t(i + 1)));
                length(sub(b, a))
            })
            .collect();
        assert!(gaps.iter().all(|g| (g - 0.5).abs() < 0.03), "{gaps:?}");
        // `incrementT` takes the distance as a share of the curve's length: one block's worth
        // from the start lands about a block along.
        let t = c.increment_t(0.0, 1.0 / c.length);
        assert!((length(c.position(t)) - 1.0).abs() < 0.05, "{t}");
        // Flat track stays upright.
        let n = c.normal(0.3);
        assert!((n[1] - 1.0).abs() < 1e-6, "{n:?}");
    }

    #[test]
    fn an_s_bend_and_a_straight_run() {
        // Parallel ends offset sideways: no circle, the handles from the side lines' crossing.
        let s = Curve::new(
            [[0.0; 3], [12.0, 0.0, 3.0]],
            [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]],
            [UP, UP],
        );
        assert_eq!(s.radius, 0.0);
        assert!(s.handle_length > 0.0);
        assert!(s.length > 12.0 && s.length < 13.5, "{}", s.length);
        let mid = s.position(0.5);
        assert!(
            (mid[0] - 6.0).abs() < 1e-9 && (mid[2] - 1.5).abs() < 1e-9,
            "{mid:?}"
        );
        // Straight ahead: a third of the span, its length the distance.
        let line = Curve::new(
            [[0.0; 3], [9.0, 0.0, 0.0]],
            [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]],
            [UP, UP],
        );
        assert!((line.handle_length - 3.0).abs() < 1e-9);
        assert!((line.length - 9.0).abs() < 1e-9);
    }
}
