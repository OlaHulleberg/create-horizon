//! How Create draws a curved track (`BezierConnection.SegmentAngles`, its `Bezierator`,
//! `TrackRenderer.getModelAngles`, Catnip's `VecHelper.rotate`): for each segment of the curve,
//! a tie and two rail pieces, each a partial model (`track/tie`, `track/segment_left`,
//! `track/segment_right`) under a transform relative to the curve's first track block.

use crate::bezier::{Curve, Vec3, bezier, bezier_derivative};

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
fn normalize(a: Vec3) -> Vec3 {
    let l = length(a);
    if l < 1.0e-4 {
        [0.0; 3]
    } else {
        scale(a, 1.0 / l)
    }
}

/// Catnip's `VecHelper.rotate(vec, deg, axis)`: about the x, y or z axis (0, 1, 2).
fn rotate(v: Vec3, deg: f64, axis: usize) -> Vec3 {
    if deg == 0.0 {
        return v;
    }
    let angle = (deg / 180.0 * std::f64::consts::PI) as f32;
    let (sin, cos) = (f64::from(angle.sin()), f64::from(angle.cos()));
    let [x, y, z] = v;
    match axis {
        0 => [x, y * cos - z * sin, z * cos + y * sin],
        1 => [x * cos + z * sin, y, z * cos - x * sin],
        _ => [x * cos - y * sin, y * cos + x * sin, z],
    }
}

/// `TrackRenderer.getModelAngles(normal, diff)`: (pitch, yaw, roll) in radians that turn a model
/// lying along +z with its up along +y to run along `diff` with its up toward `normal`.
pub fn model_angles(normal: Vec3, diff: Vec3) -> Vec3 {
    let [dx, dy, dz] = diff;
    let len = f64::from(((dx * dx + dz * dz) as f32).sqrt());
    let yaw = dx.atan2(dz);
    let pitch = len.atan2(dy) - std::f64::consts::PI * 0.5;
    let deg = |r: f64| {
        if r == 0.0 {
            0.0
        } else {
            (r * 180.0 / std::f64::consts::PI) as f32 as f64
        }
    };
    let ypn = rotate(rotate([0.0, 1.0, 0.0], deg(pitch), 0), deg(yaw), 1);
    let mut signum = dot(ypn, normal).signum();
    if dot(ypn, normal) == 0.0 {
        signum = 0.0;
    }
    if signum.abs() < 0.5 {
        let d = sub(ypn, normal);
        signum = if dot(d, d) < 0.5 { -1.0 } else { 1.0 };
    }
    let d = dot(normalize(cross(diff, normal)), ypn);
    let roll = d.clamp(-1.0, 1.0).acos() * signum;
    [pitch, yaw, roll]
}

/// A column-major 4×4 transform (as `glam::Mat4::from_cols_array_2d` takes it).
pub type Mat4 = [[f32; 4]; 4];

/// The three parts of a segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
    Tie,
    RailLeft,
    RailRight,
}

/// A matrix product `a · b` in f64.
fn mul(a: &[[f64; 4]; 4], b: &[[f64; 4]; 4]) -> [[f64; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, cell) in col.iter_mut().enumerate() {
            *cell = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

fn identity() -> [[f64; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// A `PoseStack` with the operations Create's `TransformStack` applies, each multiplied on the
/// right as `PoseStack` does; rotations take radians, as Create passes `getModelAngles`' result.
struct Pose([[f64; 4]; 4]);

impl Pose {
    fn translate(self, v: Vec3) -> Pose {
        let mut t = identity();
        t[3] = [v[0], v[1], v[2], 1.0];
        Pose(mul(&self.0, &t))
    }
    fn rotate(self, axis: usize, radians: f64) -> Pose {
        let (s, c) = (
            f64::from((radians as f32).sin()),
            f64::from((radians as f32).cos()),
        );
        let mut r = identity();
        match axis {
            0 => {
                r[1] = [0.0, c, s, 0.0];
                r[2] = [0.0, -s, c, 0.0];
            }
            1 => {
                r[0] = [c, 0.0, -s, 0.0];
                r[2] = [s, 0.0, c, 0.0];
            }
            _ => {
                r[0] = [c, s, 0.0, 0.0];
                r[1] = [-s, c, 0.0, 0.0];
            }
        }
        Pose(mul(&self.0, &r))
    }
    fn scale_z(self, z: f64) -> Pose {
        let mut s = identity();
        s[2][2] = z;
        Pose(mul(&self.0, &s))
    }
    fn angles(self, a: Vec3) -> Pose {
        self.rotate(1, a[1]).rotate(0, a[0]).rotate(2, a[2])
    }
    fn into_mat(self) -> Mat4 {
        self.0.map(|col| col.map(|v| v as f32))
    }
}

/// `SegmentAngles` for `curve`, relative to the block `origin` (the curve's first track, its
/// `bePositions.getFirst()`; the `Bezierator` offsets by `-origin + (0, 3/16, 0)`): for segments
/// 1..=segments, a tie and both rails, each piece's transform.
pub fn segment_parts(curve: &Curve, origin: [i32; 3]) -> Vec<(Piece, Mat4)> {
    let offset = [
        -f64::from(origin[0]),
        -f64::from(origin[1]) + f64::from(3.0f32 / 16.0),
        -f64::from(origin[2]),
    ];
    let end1 = add(curve.starts[0], offset);
    let end2 = add(curve.starts[1], offset);
    let finish1 = add(scale(normalize(curve.axes[0]), curve.handle_length), end1);
    let finish2 = add(scale(normalize(curve.axes[1]), curve.handle_length), end2);
    let [fn1, fn2] = curve.normals;
    let segments = curve.segments;
    let mut out = Vec::with_capacity(segments * 3);
    let mut previous: Option<[Vec3; 2]> = None;
    let below = -2.0 / 16.0 - 1.0 / 256.0;
    for i in 0..=segments {
        let t = f64::from(curve.segment_t(i));
        let position = bezier(end1, end2, finish1, finish2, t);
        let derivative = normalize(bezier_derivative(end1, end2, finish1, finish2, t));
        let face = if fn1 == fn2 {
            fn1
        } else {
            // `VecHelper.slerp` as Catnip writes it.
            let theta = dot(fn1, fn2).clamp(-1.0, 1.0).acos();
            let a = scale(fn1, f64::from(((1.0 - t) as f32).sin()) * theta);
            let b = scale(fn2, f64::from(((theta * t) as f32).sin()));
            scale(add(a, b), 1.0 / f64::from((theta as f32).sin()))
        };
        let normal = normalize(cross(face, derivative));
        let rails = [
            add(position, scale(normal, f64::from(0.965f32))),
            sub(position, scale(normal, f64::from(0.965f32))),
        ];
        let middle = scale(add(rails[0], rails[1]), 0.5);
        let Some(prev) = previous.replace(rails) else {
            continue;
        };
        let end = i == 0 || i == segments;
        let prev_middle = scale(add(prev[0], prev[1]), 0.5);
        let tie = Pose(identity())
            .translate(prev_middle)
            .angles(model_angles(normal, sub(middle, prev_middle)))
            .translate([-0.5, below, 0.0]);
        out.push((Piece::Tie, tie.into_mat()));
        let stretch = if end { 2.2 } else { 2.1 };
        for (k, piece) in [Piece::RailLeft, Piece::RailRight].into_iter().enumerate() {
            let diff = sub(rails[k], prev[k]);
            let rail = Pose(identity())
                .translate(prev[k])
                .angles(model_angles(normal, diff))
                .translate([0.0, below, -1.0 / 32.0])
                .scale_z(f64::from((length(diff) * stretch) as f32));
            out.push((piece, rail.into_mat()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const UP: Vec3 = [0.0, 1.0, 0.0];

    fn apply(m: &Mat4, p: Vec3) -> Vec3 {
        let v = [p[0] as f32, p[1] as f32, p[2] as f32, 1.0];
        let r = |row: usize| (0..4).map(|c| m[c][row] * v[c]).sum::<f32>() as f64;
        [r(0), r(1), r(2)]
    }

    #[test]
    fn a_model_turns_to_run_along_its_track() {
        // Along +z, upright: no turn at all.
        let a = model_angles([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        assert!(a.iter().all(|v| v.abs() < 1e-6), "{a:?}");
        // Along +x: a quarter turn about y.
        let a = model_angles([0.0, 0.0, -1.0], [1.0, 0.0, 0.0]);
        assert!((a[1] - std::f64::consts::FRAC_PI_2).abs() < 1e-6 && a[0].abs() < 1e-6);
    }

    #[test]
    fn a_quarter_turn_lays_its_ties_and_rails_along_the_curve() {
        // The engine's quarter turn of radius 8: east from (0.5, 0, 0.5) to south at (8.5, 0, 8.5).
        let c = Curve::new(
            [[0.5, 0.0, 0.5], [8.5, 0.0, 8.5]],
            [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
            [UP, UP],
        );
        let parts = segment_parts(&c, [0, 0, 0]);
        assert_eq!(parts.len(), c.segments * 3);
        let ties: Vec<&Mat4> = parts
            .iter()
            .filter(|p| p.0 == Piece::Tie)
            .map(|p| &p.1)
            .collect();
        // A tie model's middle (its 0.5, 0, 0 after the -1/2 shift) sits on the curve, 3/16 up
        // and then 2/16 + 1/256 down: within the circle's band.
        for m in &ties {
            let p = apply(m, [0.5, 0.0, 0.0]);
            let r = ((p[0] - 0.5).powi(2) + (p[2] - 8.5).powi(2)).sqrt();
            assert!((r - 8.0).abs() < 0.08, "{p:?} r {r}");
            assert!(
                (p[1] - (3.0 / 16.0 - 2.0 / 16.0 - 1.0 / 256.0)).abs() < 1e-4,
                "{p:?}"
            );
        }
        // The first tie points east, the last south (its +z mapped).
        let dir = |m: &Mat4| {
            let a = apply(m, [0.0, 0.0, 0.0]);
            let b = apply(m, [0.0, 0.0, 1.0]);
            sub(b, a)
        };
        assert!(dir(ties[0])[0] > 0.9, "{:?}", dir(ties[0]));
        assert!(
            dir(ties[ties.len() - 1])[2] > 0.9,
            "{:?}",
            dir(ties[ties.len() - 1])
        );
        // Rails lie 0.965 to either side of the middle.
        let rail = parts.iter().find(|p| p.0 == Piece::RailLeft).unwrap();
        let tie = ties[0];
        let gap = length(sub(apply(&rail.1, [0.0; 3]), apply(tie, [0.5, 0.0, 0.0])));
        assert!((gap - 0.965).abs() < 0.05, "{gap}");
    }
}
