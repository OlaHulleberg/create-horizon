//! The engine's poses (`Mat4`, built from translations, turns and scales as Create's renderers
//! build their pose stacks) drawn through a block entity's draw context, which takes a pose as
//! a translation, a turn and a scale.

use create_engine::math::Mat4;
use mod_api::BlockEntityDrawContext;

/// A pose taken apart: what it moves by, the turn (a unit axis and degrees, none for no turn)
/// and the scale along each of its own axes (one negative for a mirroring pose).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Parts {
    pub translation: [f32; 3],
    pub rotation: Option<([f32; 3], f32)>,
    pub scale: [f32; 3],
}

/// `m` as a translation, a turn and a scale (`m = T * R * S`). The engine's poses have no
/// shear, so their columns are a turn's scaled.
pub(crate) fn decompose(m: &Mat4) -> Parts {
    let c = m.to_cols_array_2d();
    let col = |i: usize| [c[i][0], c[i][1], c[i][2]];
    let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let (a, b, d) = (col(0), col(1), col(2));
    let det = a[0] * (b[1] * d[2] - b[2] * d[1]) - b[0] * (a[1] * d[2] - a[2] * d[1])
        + d[0] * (a[1] * b[2] - a[2] * b[1]);
    let mut scale = [len(a), len(b), len(d)];
    if det < 0.0 {
        scale[0] = -scale[0];
    }
    let unit = |v: [f32; 3], s: f32| {
        if s == 0.0 { [0.0; 3] } else { v.map(|x| x / s) }
    };
    let (r0, r1, r2) = (unit(a, scale[0]), unit(b, scale[1]), unit(d, scale[2]));
    // `r[row][col]`.
    let r = |row: usize, column: usize| [r0, r1, r2][column][row];
    let trace = r(0, 0) + r(1, 1) + r(2, 2);
    let (x, y, z, w) = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        (
            (r(2, 1) - r(1, 2)) / s,
            (r(0, 2) - r(2, 0)) / s,
            (r(1, 0) - r(0, 1)) / s,
            0.25 * s,
        )
    } else if r(0, 0) > r(1, 1) && r(0, 0) > r(2, 2) {
        let s = (1.0 + r(0, 0) - r(1, 1) - r(2, 2)).sqrt() * 2.0;
        (
            0.25 * s,
            (r(0, 1) + r(1, 0)) / s,
            (r(0, 2) + r(2, 0)) / s,
            (r(2, 1) - r(1, 2)) / s,
        )
    } else if r(1, 1) > r(2, 2) {
        let s = (1.0 + r(1, 1) - r(0, 0) - r(2, 2)).sqrt() * 2.0;
        (
            (r(0, 1) + r(1, 0)) / s,
            0.25 * s,
            (r(1, 2) + r(2, 1)) / s,
            (r(0, 2) - r(2, 0)) / s,
        )
    } else {
        let s = (1.0 + r(2, 2) - r(0, 0) - r(1, 1)).sqrt() * 2.0;
        (
            (r(0, 2) + r(2, 0)) / s,
            (r(1, 2) + r(2, 1)) / s,
            0.25 * s,
            (r(1, 0) - r(0, 1)) / s,
        )
    };
    let (x, y, z, w) = if w < 0.0 {
        (-x, -y, -z, -w)
    } else {
        (x, y, z, w)
    };
    let half = w.clamp(-1.0, 1.0).acos();
    let sin = half.sin();
    let rotation =
        (sin.abs() > 1e-6).then(|| ([x / sin, y / sin, z / sin], 2.0 * half.to_degrees()));
    Parts {
        translation: [c[3][0], c[3][1], c[3][2]],
        rotation,
        scale,
    }
}

/// Puts `m` on `draw`'s pose stack (`PoseStack.mulPose` of the engine's matrix).
pub(crate) fn apply(draw: &mut dyn BlockEntityDrawContext, m: &Mat4) {
    let p = decompose(m);
    if p.translation != [0.0; 3] {
        draw.translate(p.translation);
    }
    if let Some((axis, degrees)) = p.rotation {
        draw.rotate_about(axis, degrees, [0.0; 3]);
    }
    if p.scale != [1.0; 3] {
        draw.scale(p.scale);
    }
}

#[cfg(test)]
mod tests {
    use super::decompose;
    use create_engine::math::{Mat4, Quat, Vec3};

    /// `p` through the parts, as the pose stack applies them.
    fn through(parts: &super::Parts, p: Vec3) -> Vec3 {
        let s = Vec3::new(
            p.x * parts.scale[0],
            p.y * parts.scale[1],
            p.z * parts.scale[2],
        );
        let r = match parts.rotation {
            Some((axis, deg)) => Quat::from_axis_angle(Vec3::from(axis), deg.to_radians()) * s,
            None => s,
        };
        r + Vec3::from(parts.translation)
    }

    #[test]
    fn poses_come_apart_and_back_together() {
        let poses = [
            Mat4::IDENTITY,
            Mat4::from_translation(Vec3::new(0.5, 0.25, -1.0)),
            Mat4::from_translation(Vec3::splat(0.5))
                * Mat4::from_rotation_y(90f32.to_radians())
                * Mat4::from_rotation_z((-90f32).to_radians())
                * Mat4::from_scale(Vec3::new(0.05, -0.05, 0.05)),
            Mat4::from_rotation_x(180f32.to_radians())
                * Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)),
            Mat4::from_rotation_y(270f32.to_radians()) * Mat4::from_translation(Vec3::splat(-0.5)),
        ];
        for m in poses {
            let parts = decompose(&m);
            for p in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(0.3, -2.0, 7.0)] {
                let want = m.transform_point3(p);
                let got = through(&parts, p);
                assert!((want - got).length() < 1e-4, "{m:?}: {want:?} != {got:?}");
            }
        }
    }
}
