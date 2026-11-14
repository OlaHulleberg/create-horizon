//! The few vector and matrix operations the renderer-facing rules use (part poses, item copies,
//! arm joints), in `f32` and with glam's conventions: column vectors, column-major matrices,
//! right-handed rotations. Kept here so the engine, and with it the mod, has no registry
//! dependency.

use std::fmt;
use std::ops::{Add, AddAssign, Mul, MulAssign, Neg, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3::splat(0.0);
    pub const ONE: Vec3 = Vec3::splat(1.0);
    pub const X: Vec3 = Vec3::new(1.0, 0.0, 0.0);
    pub const Y: Vec3 = Vec3::new(0.0, 1.0, 0.0);
    pub const Z: Vec3 = Vec3::new(0.0, 0.0, 1.0);

    pub const fn new(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }

    pub const fn splat(v: f32) -> Vec3 {
        Vec3::new(v, v, v)
    }

    pub const fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    /// `self / |self|`; as glam, not finite for a zero vector.
    pub fn normalize(self) -> Vec3 {
        self * (1.0 / self.length())
    }

    pub fn lerp(self, to: Vec3, t: f32) -> Vec3 {
        self + (to - self) * t
    }
}

impl From<[f32; 3]> for Vec3 {
    fn from(a: [f32; 3]) -> Vec3 {
        Vec3::new(a[0], a[1], a[2])
    }
}

impl From<Vec3> for [f32; 3] {
    fn from(v: Vec3) -> [f32; 3] {
        v.to_array()
    }
}

impl fmt::Display for Vec3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}, {}, {}]", self.x, self.y, self.z)
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl AddAssign for Vec3 {
    fn add_assign(&mut self, o: Vec3) {
        *self = *self + o;
    }
}

impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl SubAssign for Vec3 {
    fn sub_assign(&mut self, o: Vec3) {
        *self = *self - o;
    }
}

impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Mul<Vec3> for f32 {
    type Output = Vec3;
    fn mul(self, v: Vec3) -> Vec3 {
        v * self
    }
}

impl Mul for Vec3 {
    type Output = Vec3;
    fn mul(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x * o.x, self.y * o.y, self.z * o.z)
    }
}

impl Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

/// A rotation, as a unit quaternion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Quat = Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    /// `angle` radians about the unit vector `axis`.
    pub fn from_axis_angle(axis: Vec3, angle: f32) -> Quat {
        let (s, c) = (angle * 0.5).sin_cos();
        let v = axis * s;
        Quat {
            x: v.x,
            y: v.y,
            z: v.z,
            w: c,
        }
    }
}

impl Mul<Vec3> for Quat {
    type Output = Vec3;
    fn mul(self, v: Vec3) -> Vec3 {
        let q = Vec3::new(self.x, self.y, self.z);
        let t = q.cross(v) * 2.0;
        v + t * self.w + q.cross(t)
    }
}

/// A 4x4 matrix, column-major: `cols[c][r]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4 {
    cols: [[f32; 4]; 4],
}

impl Default for Mat4 {
    fn default() -> Mat4 {
        Mat4::IDENTITY
    }
}

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4 {
        cols: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
    };

    pub const fn from_cols_array_2d(cols: &[[f32; 4]; 4]) -> Mat4 {
        Mat4 { cols: *cols }
    }

    pub const fn to_cols_array_2d(&self) -> [[f32; 4]; 4] {
        self.cols
    }

    pub fn to_cols_array(&self) -> [f32; 16] {
        let mut out = [0.0; 16];
        for (c, col) in self.cols.iter().enumerate() {
            out[c * 4..c * 4 + 4].copy_from_slice(col);
        }
        out
    }

    pub fn from_translation(t: Vec3) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        m.cols[3] = [t.x, t.y, t.z, 1.0];
        m
    }

    pub fn from_scale(s: Vec3) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        m.cols[0][0] = s.x;
        m.cols[1][1] = s.y;
        m.cols[2][2] = s.z;
        m
    }

    pub fn from_rotation_x(a: f32) -> Mat4 {
        let (s, c) = a.sin_cos();
        Mat4::from_cols_array_2d(&[
            [1.0, 0.0, 0.0, 0.0],
            [0.0, c, s, 0.0],
            [0.0, -s, c, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    pub fn from_rotation_y(a: f32) -> Mat4 {
        let (s, c) = a.sin_cos();
        Mat4::from_cols_array_2d(&[
            [c, 0.0, -s, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [s, 0.0, c, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    pub fn from_rotation_z(a: f32) -> Mat4 {
        let (s, c) = a.sin_cos();
        Mat4::from_cols_array_2d(&[
            [c, s, 0.0, 0.0],
            [-s, c, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    /// Whether every element is within `max` of `o`'s.
    pub fn abs_diff_eq(&self, o: Mat4, max: f32) -> bool {
        self.to_cols_array()
            .iter()
            .zip(o.to_cols_array())
            .all(|(a, b)| (a - b).abs() <= max)
    }

    /// The point `p` moved by this matrix (w = 1).
    pub fn transform_point3(&self, p: Vec3) -> Vec3 {
        let c = &self.cols;
        Vec3::new(
            c[0][0] * p.x + c[1][0] * p.y + c[2][0] * p.z + c[3][0],
            c[0][1] * p.x + c[1][1] * p.y + c[2][1] * p.z + c[3][1],
            c[0][2] * p.x + c[1][2] * p.y + c[2][2] * p.z + c[3][2],
        )
    }

    /// The direction `v` turned and scaled by this matrix (w = 0).
    pub fn transform_vector3(&self, v: Vec3) -> Vec3 {
        let c = &self.cols;
        Vec3::new(
            c[0][0] * v.x + c[1][0] * v.y + c[2][0] * v.z,
            c[0][1] * v.x + c[1][1] * v.y + c[2][1] * v.z,
            c[0][2] * v.x + c[1][2] * v.y + c[2][2] * v.z,
        )
    }
}

impl Mul for Mat4 {
    type Output = Mat4;
    fn mul(self, o: Mat4) -> Mat4 {
        let mut out = [[0.0f32; 4]; 4];
        for (c, col) in out.iter_mut().enumerate() {
            for (r, v) in col.iter_mut().enumerate() {
                *v = (0..4).map(|k| self.cols[k][r] * o.cols[c][k]).sum();
            }
        }
        Mat4 { cols: out }
    }
}

impl MulAssign for Mat4 {
    fn mul_assign(&mut self, o: Mat4) {
        *self = *self * o;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn rotations_are_right_handed() {
        let q = std::f32::consts::FRAC_PI_2;
        assert!(close(
            Mat4::from_rotation_y(q).transform_vector3(Vec3::X),
            -Vec3::Z
        ));
        assert!(close(
            Mat4::from_rotation_x(q).transform_vector3(Vec3::Y),
            Vec3::Z
        ));
        assert!(close(
            Mat4::from_rotation_z(q).transform_vector3(Vec3::X),
            Vec3::Y
        ));
        assert!(close(Quat::from_axis_angle(Vec3::Z, q) * Vec3::X, Vec3::Y));
    }

    #[test]
    fn products_apply_right_to_left() {
        let m =
            Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0)) * Mat4::from_scale(Vec3::splat(2.0));
        assert!(close(
            m.transform_point3(Vec3::ONE),
            Vec3::new(3.0, 2.0, 2.0)
        ));
        assert!(close(m.transform_vector3(Vec3::ONE), Vec3::splat(2.0)));
    }
}
