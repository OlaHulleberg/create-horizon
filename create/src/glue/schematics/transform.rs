//! The held schematic's place on the client (`SchematicTransformation`): where it is, turned
//! and mirrored, each chased smoothly by catnip's `LerpedFloat`s, and the anchor and
//! placement settings that come out of it; and `RaycastHelper.rayTraceUntil`, by which a tool
//! finds the face of the schematic looked at. Plain arithmetic, so it is tested here.

use create_engine::kinetics::Dir;
use create_engine::schematic::{Mirror, Rotation};

/// `Mth.equal`.
fn equal(a: f64, b: f64) -> bool {
    (b - a).abs() < 1.0e-5
}

/// `AngleHelper.getShortestAngleDiff`.
fn shortest_angle_diff(current: f64, target: f64) -> f32 {
    let (current, target) = (current % 360.0, target % 360.0);
    ((((target - current) % 360.0) + 540.0) % 360.0 - 180.0) as f32
}

/// catnip's `LerpedFloat` with the `Chaser.EXP` chase: linear, or angular (chasing the
/// shortest way round and lerped as angles).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Lerped {
    previous: f32,
    value: f32,
    target: f32,
    speed: f32,
    chasing: bool,
    angular: bool,
}

impl Lerped {
    pub(super) fn linear() -> Lerped {
        Lerped {
            previous: 0.0,
            value: 0.0,
            target: 0.0,
            speed: 0.0,
            chasing: false,
            angular: false,
        }
    }

    pub(super) fn angular() -> Lerped {
        Lerped {
            angular: true,
            ..Lerped::linear()
        }
    }

    /// `startWithValue`.
    fn start_with(&mut self, v: f32) {
        self.previous = v;
        self.target = v;
        self.value = v;
    }

    /// `chase(value, speed, Chaser.EXP)`.
    fn chase(&mut self, target: f32, speed: f32) {
        self.update_target(target);
        self.speed = speed;
        self.chasing = true;
    }

    /// `updateChaseTarget`.
    fn update_target(&mut self, target: f32) {
        self.target = if self.angular {
            self.value + shortest_angle_diff(f64::from(self.value), f64::from(target))
        } else {
            target
        };
    }

    /// `tickChaser`.
    fn tick(&mut self) {
        self.previous = self.value;
        if !self.chasing {
            return;
        }
        if equal(f64::from(self.value), f64::from(self.target)) {
            self.value = self.target;
            return;
        }
        let (c, t) = (f64::from(self.value), f64::from(self.target));
        self.value = (c + (t - c) * f64::from(self.speed)) as f32;
    }

    /// `getValue(partialTicks)`.
    pub(super) fn get(&self, pt: f32) -> f32 {
        let (p, c, t) = (
            f64::from(pt),
            f64::from(self.previous),
            f64::from(self.value),
        );
        if self.angular {
            (c + f64::from(shortest_angle_diff(c, t)) * p) as f32
        } else {
            (c + (t - c) * p) as f32
        }
    }

    pub(super) fn target(&self) -> f32 {
        self.target
    }
}

type Vec3 = [f64; 3];

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn lerp(p: f64, from: Vec3, to: Vec3) -> Vec3 {
    [0, 1, 2].map(|i| from[i] + (to[i] - from[i]) * p)
}

/// `Mth.SIN`'s entry `i`: the table vanilla's `Mth.sin` and `cos` read.
fn sin_table(i: i32) -> f64 {
    f64::from((f64::from(i & 65535) * std::f64::consts::PI * 2.0 / 65536.0).sin() as f32)
}

/// `Mth.sin`.
fn mth_sin(v: f32) -> f64 {
    sin_table((v * 10430.378) as i32)
}

/// `Mth.cos`.
fn mth_cos(v: f32) -> f64 {
    sin_table((v * 10430.378 + 16384.0) as i32)
}

/// `VecHelper.rotate(vec, deg, Axis.Y)` (`Mth.sin`/`cos` of the float angle: their table, whose
/// half turn is not quite zero the way `f32::sin`'s is not either, but on the other side).
pub(super) fn rotate_y(v: Vec3, deg: f64) -> Vec3 {
    if deg == 0.0 {
        return v;
    }
    let angle = (deg / 180.0 * std::f64::consts::PI) as f32;
    let (sin, cos) = (mth_sin(angle), mth_cos(angle));
    [v[0] * cos + v[2] * sin, v[1], v[2] * cos - v[0] * sin]
}

fn floor(v: Vec3) -> [i32; 3] {
    v.map(|c| c.floor() as i32)
}

/// `SchematicTransformation`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Placing {
    chasing: Vec3,
    prev_chasing: Vec3,
    target: [i32; 3],
    scale_fb: Lerped,
    scale_lr: Lerped,
    rotation: Lerped,
    x_origin: f64,
    z_origin: f64,
}

impl Default for Placing {
    fn default() -> Self {
        Placing {
            chasing: [0.0; 3],
            prev_chasing: [0.0; 3],
            target: [0; 3],
            scale_fb: Lerped::linear(),
            scale_lr: Lerped::linear(),
            rotation: Lerped::angular(),
            x_origin: 0.0,
            z_origin: 0.0,
        }
    }
}

impl Placing {
    /// `init(anchor, settings, bounds)`: settled at the anchor, turned and mirrored as the item
    /// says, for a schematic of `size`.
    pub(super) fn init(
        &mut self,
        anchor: [i32; 3],
        rotation: Rotation,
        mirror: Mirror,
        size: [i32; 3],
    ) {
        let lr = if mirror == Mirror::LeftRight {
            -1.0
        } else {
            1.0
        };
        let fb = if mirror == Mirror::FrontBack {
            -1.0
        } else {
            1.0
        };
        self.scale_fb.chase(0.0, 0.45);
        self.scale_fb.start_with(fb);
        self.scale_lr.chase(0.0, 0.45);
        self.scale_lr.start_with(lr);
        self.x_origin = f64::from(size[0] as f32 / 2.0);
        self.z_origin = f64::from(size[2] as f32 / 2.0);
        let r = -(rotation as i32 * 90);
        self.rotation.chase(0.0, 0.45);
        self.rotation.start_with(r as f32);
        self.target = self.target_of(anchor);
        self.chasing = self.target.map(f64::from);
        self.prev_chasing = self.chasing;
    }

    /// The chased position, between ticks.
    fn position(&self, pt: f32) -> Vec3 {
        lerp(f64::from(pt), self.prev_chasing, self.chasing)
    }

    /// `applyTransformations`: a point of the schematic's own box (`0..size`) where it is drawn.
    pub(super) fn apply(&self, p: Vec3, pt: f32) -> Vec3 {
        let ro = self.rotation_offset(true);
        let fb = self.scale_fb.get(pt);
        let lr = self.scale_lr.get(pt);
        let rot = self.rotation.get(pt) + if fb < 0.0 && lr < 0.0 { 180.0 } else { 0.0 };
        let o = [self.x_origin, 0.0, self.z_origin];
        let mut q = sub(p, o);
        q = [q[0] * f64::from(fb.abs()), q[1], q[2] * f64::from(lr.abs())];
        q = sub(q, ro);
        q = rotate_y(q, f64::from(rot));
        q = add(q, ro);
        q = add(q, o);
        add(q, self.position(pt))
    }

    /// `isFlipped`.
    #[cfg(test)]
    fn is_flipped(&self) -> bool {
        (self.mirror_modifier_x() < 0) != (self.mirror_modifier_z() < 0)
    }

    /// `getRotationOffset`.
    pub(super) fn rotation_offset(&self, ignore_mirrors: bool) -> Vec3 {
        if ((self.z_origin * 2.0) as i32) % 2 == ((self.x_origin * 2.0) as i32) % 2 {
            return [0.0; 3];
        }
        let x_greater = self.x_origin > self.z_origin;
        let mut x_in: f64 = if x_greater { 0.0 } else { 0.5 };
        let mut z_in: f64 = if !x_greater { 0.0 } else { 0.5 };
        if !ignore_mirrors {
            x_in *= f64::from(self.mirror_modifier_x());
            z_in *= f64::from(self.mirror_modifier_z());
        }
        [x_in, 0.0, z_in]
    }

    /// `toLocalSpace`: a point of the world in the schematic's own box.
    pub(super) fn to_local(&self, v: Vec3, pt: f32) -> Vec3 {
        let ro = self.rotation_offset(true);
        let mut v = sub(v, self.position(pt));
        v = sub(v, [self.x_origin + ro[0], 0.0, self.z_origin + ro[2]]);
        v = rotate_y(v, -f64::from(self.rotation.get(pt)));
        v = add(v, [ro[0], 0.0, ro[2]]);
        v = [
            v[0] * f64::from(self.scale_fb.get(pt)),
            v[1],
            v[2] * f64::from(self.scale_lr.get(pt)),
        ];
        add(v, [self.x_origin, 0.0, self.z_origin])
    }

    /// `toSettings`: the rotation and mirror the chase heads for.
    pub(super) fn to_settings(&self) -> (Rotation, Mirror) {
        let mut i = self.rotation.target() as i32;
        let mut lr = self.scale_lr.target() < 0.0;
        let mut fb = self.scale_fb.target() < 0.0;
        if lr && fb {
            lr = false;
            fb = false;
            i += 180;
        }
        i %= 360;
        if i < 0 {
            i += 360;
        }
        let rotation = match i {
            90 => Rotation::Counterclockwise90,
            180 => Rotation::Clockwise180,
            270 => Rotation::Clockwise90,
            _ => Rotation::None,
        };
        // `setMirror(FRONT_BACK)` then `setMirror(LEFT_RIGHT)`: the last one set stands.
        let mirror = if lr {
            Mirror::LeftRight
        } else if fb {
            Mirror::FrontBack
        } else {
            Mirror::None
        };
        (rotation, mirror)
    }

    /// The anchor's offset from the target (`getAnchor`'s and `fromAnchor`'s shared vector).
    fn anchor_offset(&self) -> Vec3 {
        let ro = self.rotation_offset(false);
        let mut v = [0.5, 0.0, 0.5];
        v = sub(v, [self.x_origin, 0.0, self.z_origin]);
        v = sub(v, [ro[0], 0.0, ro[2]]);
        v = [
            v[0] * f64::from(self.scale_fb.target()),
            v[1],
            v[2] * f64::from(self.scale_lr.target()),
        ];
        v = rotate_y(v, f64::from(self.rotation.target()));
        add(v, [self.x_origin, 0.0, self.z_origin])
    }

    /// `getAnchor`.
    pub(super) fn anchor(&self) -> [i32; 3] {
        floor(add(self.anchor_offset(), self.target.map(f64::from)))
    }

    /// `fromAnchor`: the target an anchor puts it at.
    fn target_of(&self, pos: [i32; 3]) -> [i32; 3] {
        let o = floor(self.anchor_offset());
        [0, 1, 2].map(|i| pos[i] - o[i])
    }

    /// `getRotationTarget`.
    pub(super) fn rotation_target(&self) -> i32 {
        self.rotation.target() as i32
    }

    /// `getMirrorModifier(Axis.X)`.
    pub(super) fn mirror_modifier_x(&self) -> i32 {
        self.scale_fb.target() as i32
    }

    /// `getMirrorModifier(Axis.Z)`.
    pub(super) fn mirror_modifier_z(&self) -> i32 {
        self.scale_lr.target() as i32
    }

    /// `getCurrentRotation`.
    pub(super) fn current_rotation(&self, pt: f32) -> f32 {
        self.rotation.get(pt)
    }

    /// The mirrors between ticks (`getScaleFB`, `getScaleLR`).
    #[cfg(test)]
    fn scales(&self, pt: f32) -> (f32, f32) {
        (self.scale_fb.get(pt), self.scale_lr.get(pt))
    }

    /// `tick`.
    pub(super) fn tick(&mut self) {
        self.prev_chasing = self.chasing;
        self.chasing = lerp(0.45, self.chasing, self.target.map(f64::from));
        self.scale_lr.tick();
        self.scale_fb.tick();
        self.rotation.tick();
    }

    /// `flip(axis)`: across x turns the left-right mirror, across z the front-back one.
    pub(super) fn flip_x(&mut self) {
        let t = self.scale_lr.target();
        self.scale_lr.update_target(-t);
    }

    pub(super) fn flip_z(&mut self) {
        let t = self.scale_fb.target();
        self.scale_fb.update_target(-t);
    }

    /// `rotate90`.
    pub(super) fn rotate90(&mut self, clockwise: bool) {
        let t = self.rotation.target() + if clockwise { -90.0 } else { 90.0 };
        self.rotation.update_target(t);
    }

    /// `move`.
    pub(super) fn shift(&mut self, by: [i32; 3]) {
        self.target = [0, 1, 2].map(|i| self.target[i] + by[i]);
    }

    /// `startAt`: there at once.
    pub(super) fn start_at(&mut self, pos: [i32; 3]) {
        self.chasing = pos.map(f64::from);
        self.prev_chasing = self.chasing;
        self.target = pos;
    }

    /// `moveTo`.
    pub(super) fn move_to(&mut self, pos: [i32; 3]) {
        self.target = pos;
    }
}

/// `Direction.getNearest(x, y, z)`.
fn nearest(v: [i32; 3]) -> Dir {
    let mut best = (Dir::North, f32::MIN_POSITIVE);
    for d in Dir::ALL {
        let o = d.offset();
        let g = (v[0] * o[0] + v[1] * o[1] + v[2] * o[2]) as f32;
        if g > best.1 {
            best = (d, g);
        }
    }
    best.0
}

/// `RaycastHelper.rayTraceUntil(start, end, predicate)`: the first block from `start` to `end`
/// the predicate takes, and the face the line entered it by.
pub(super) fn trace_until(
    mut start: Vec3,
    end: Vec3,
    test: impl Fn([i32; 3]) -> bool,
) -> Option<([i32; 3], Dir)> {
    if start.iter().chain(end.iter()).any(|v| v.is_nan()) {
        return None;
    }
    let d = end.map(|v| v.floor() as i32);
    let mut p = start.map(|v| v.floor() as i32);
    if test(p) {
        return Some((p, nearest([d[0] - p[0], d[1] - p[1], d[2] - p[2]])));
    }
    for _ in 0..=200 {
        if p == d {
            return None;
        }
        let mut bound = [999.0; 3];
        let mut moves = [true; 3];
        for i in 0..3 {
            if d[i] > p[i] {
                bound[i] = f64::from(p[i]) + 1.0;
            } else if d[i] < p[i] {
                bound[i] = f64::from(p[i]);
            } else {
                moves[i] = false;
            }
        }
        let delta = sub(end, start);
        let mut t = [999.0; 3];
        for i in 0..3 {
            if moves[i] {
                t[i] = (bound[i] - start[i]) / delta[i];
            }
            if t[i] == 0.0 && t[i].is_sign_negative() {
                t[i] = -1.0e-4;
            }
        }
        let face = if t[0] < t[1] && t[0] < t[2] {
            start = [
                bound[0],
                start[1] + delta[1] * t[0],
                start[2] + delta[2] * t[0],
            ];
            if d[0] > p[0] { Dir::West } else { Dir::East }
        } else if t[1] < t[2] {
            start = [
                start[0] + delta[0] * t[1],
                bound[1],
                start[2] + delta[2] * t[1],
            ];
            if d[1] > p[1] { Dir::Down } else { Dir::Up }
        } else {
            start = [
                start[0] + delta[0] * t[2],
                start[1] + delta[1] * t[2],
                bound[2],
            ];
            if d[2] > p[2] { Dir::North } else { Dir::South }
        };
        p = [
            start[0].floor() as i32 - i32::from(face == Dir::East),
            start[1].floor() as i32 - i32::from(face == Dir::Up),
            start[2].floor() as i32 - i32::from(face == Dir::South),
        ];
        if test(p) {
            return Some((p, face));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use create_engine::schematic::transform;

    /// The world box `SchematicHandler` would outline once settled: the corners of the
    /// schematic's own box carried by the transformation.
    fn drawn(t: &Placing, size: [i32; 3]) -> ([i32; 3], [i32; 3]) {
        let mut min = [i32::MAX; 3];
        let mut max = [i32::MIN; 3];
        for c in 0..8 {
            let p = [
                if c & 1 == 0 { 0.0 } else { f64::from(size[0]) },
                if c & 2 == 0 { 0.0 } else { f64::from(size[1]) },
                if c & 4 == 0 { 0.0 } else { f64::from(size[2]) },
            ];
            let w = t.apply(p, 1.0);
            for i in 0..3 {
                let v = w[i].round() as i32;
                min[i] = min[i].min(v);
                max[i] = max[i].max(v);
            }
        }
        (min, max)
    }

    /// The box the printer fills (`StructureTemplate.transform` round the anchor).
    fn printed(anchor: [i32; 3], size: [i32; 3], r: Rotation, m: Mirror) -> ([i32; 3], [i32; 3]) {
        let a = transform([0; 3], m, r, [0; 3]);
        let b = transform([size[0] - 1, size[1] - 1, size[2] - 1], m, r, [0; 3]);
        (
            [0, 1, 2].map(|i| anchor[i] + a[i].min(b[i])),
            [0, 1, 2].map(|i| anchor[i] + a[i].max(b[i]) + 1),
        )
    }

    fn settle(t: &mut Placing) {
        for _ in 0..200 {
            t.tick();
        }
    }

    /// Sizes whose halves are both whole or both half (no `getRotationOffset`): there the
    /// outline and the print agree and the anchor comes back as it went in.
    const EVEN: [[i32; 3]; 3] = [[3, 2, 1], [4, 1, 2], [5, 3, 5]];

    #[test]
    fn the_outline_is_where_the_printer_prints() {
        for size in EVEN {
            for r in Rotation::ALL {
                for m in Mirror::ALL {
                    let mut t = Placing::default();
                    t.init([10, 64, -20], r, m, size);
                    settle(&mut t);
                    assert_eq!(t.anchor(), [10, 64, -20], "{size:?} {r:?} {m:?}");
                    assert_eq!(t.to_settings(), (r, m), "{size:?} {r:?} {m:?}");
                    assert_eq!(
                        drawn(&t, size),
                        printed([10, 64, -20], size, r, m),
                        "{size:?} {r:?} {m:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn turning_and_flipping_keep_the_drawn_box_on_the_printed_one() {
        for size in EVEN {
            let mut t = Placing::default();
            t.init([0, 64, 0], Rotation::None, Mirror::None, size);
            for step in 0..12 {
                match step % 3 {
                    0 => t.rotate90(step % 2 == 0),
                    1 => t.flip_x(),
                    _ => t.flip_z(),
                }
                settle(&mut t);
                let (r, m) = t.to_settings();
                assert_eq!(
                    drawn(&t, size),
                    printed(t.anchor(), size, r, m),
                    "{size:?} step {step}"
                );
            }
        }
    }

    /// Create's own arithmetic, kept: an even by odd schematic turned half round comes back a
    /// block east of the anchor it was loaded at (`fromAnchor` floors 1.9999999999999996 to 1,
    /// and `getAnchor` adds the target before flooring), as the same sums give in Java.
    #[test]
    fn an_even_by_odd_half_turn_creeps_as_in_create() {
        let mut t = Placing::default();
        t.init(
            [10, 64, -20],
            Rotation::Clockwise180,
            Mirror::None,
            [2, 2, 7],
        );
        assert_eq!(t.anchor(), [11, 64, -20]);
    }

    #[test]
    fn a_flip_both_ways_is_a_half_turn() {
        let mut t = Placing::default();
        t.init([0, 0, 0], Rotation::None, Mirror::None, [3, 1, 3]);
        t.flip_x();
        t.flip_z();
        assert_eq!(t.to_settings(), (Rotation::Clockwise180, Mirror::None));
        assert!(!t.is_flipped());
        t.flip_x();
        assert!(t.is_flipped());
    }

    #[test]
    fn the_chase_eases_in() {
        let mut t = Placing::default();
        t.init([0, 0, 0], Rotation::None, Mirror::None, [3, 1, 3]);
        t.flip_z();
        t.tick();
        let (fb, _) = t.scales(1.0);
        assert!((fb - 0.1).abs() < 1e-6, "{fb}");
        t.rotate90(true);
        assert_eq!(t.rotation_target(), -90);
    }

    #[test]
    fn a_world_point_comes_back_to_its_place_in_the_schematic() {
        let mut t = Placing::default();
        t.init([5, 70, 5], Rotation::Clockwise90, Mirror::None, [3, 4, 5]);
        let local = [1.25, 2.0, 3.5];
        let back = t.to_local(t.apply(local, 1.0), 1.0);
        for i in 0..3 {
            assert!((back[i] - local[i]).abs() < 1e-4, "{back:?}");
        }
    }

    #[test]
    fn a_trace_finds_the_face_it_enters_by() {
        let inside = |p: [i32; 3]| (0..3).all(|i| (0..3).contains(&p[i]));
        let hit = trace_until([1.5, 1.5, -5.0], [1.5, 1.5, 10.0], inside);
        assert_eq!(hit, Some(([1, 1, 0], Dir::North)));
        let hit = trace_until([1.5, 10.0, 1.5], [1.6, -10.0, 1.5], inside);
        assert_eq!(hit, Some(([1, 2, 1], Dir::Up)));
        assert_eq!(
            trace_until([1.5, 10.0, 1.5], [1.5, 20.0, 1.5], inside),
            None
        );
    }
}
