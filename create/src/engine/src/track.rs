//! Create's track placement (`TrackPlacement.tryConnect`, `PlacementInfo`, `ConnectingFrom`,
//! `TrackBlockItem.select`) and track shapes (`TrackShape`, `TrackBlock.getStateForPlacement`,
//! `getCurveStart`, `ITrackBlock.getNearestTrackAxis`): which ways a track block runs, the end a
//! player picks with the first click, and whether a second click can join the two ends, with
//! straight track, an S-bend, a turn or a ramp, and why not when it can't.
//!
//! Angles use `f64::atan2` where Create calls Minecraft's table-driven `Mth.atan2`; track axes
//! are multiples of 45°, so the limits (60°, 300°, within a degree of 90°) never sit near the
//! difference.

use crate::bezier::{self, Curve, Vec3};
use crate::kinetics::Dir;

pub type BlockPos = [i32; 3];

const ZERO: Vec3 = [0.0; 3];
const UP: Vec3 = [0.0, 1.0, 0.0];

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
fn distance_sqr(a: Vec3, b: Vec3) -> f64 {
    let d = sub(a, b);
    dot(d, d)
}
/// `Vec3.normalize`: zero under `1.0E-5F`, else each part divided by the length.
fn normalize(a: Vec3) -> Vec3 {
    let l = length(a);
    if l < f64::from(1.0e-5f32) {
        ZERO
    } else {
        [a[0] / l, a[1] / l, a[2] / l]
    }
}
/// `Mth.equal`.
fn equal(a: f64, b: f64) -> bool {
    (b - a).abs() < 1.0e-5
}
/// Java's `Math.round(double)` for the values here.
fn round(a: f64) -> i32 {
    (a + 0.5).floor() as i32
}
/// `VecHelper.getCenterOf`.
fn center(pos: BlockPos) -> Vec3 {
    [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + 0.5,
        f64::from(pos[2]) + 0.5,
    ]
}
/// `pos.offset(BlockPos.containing(offset))`.
fn offset(pos: BlockPos, by: Vec3) -> BlockPos {
    [
        pos[0] + by[0].floor() as i32,
        pos[1] + by[1].floor() as i32,
        pos[2] + by[2].floor() as i32,
    ]
}

/// The plane `VecHelper.intersect` works in, named by the axis it drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plane {
    X,
    Y,
    Z,
}

/// `VecHelper.intersect(p1, p2, r, s, plane)`: the X plane keeps (y, z), the Z plane (x, y).
fn intersect(p1: Vec3, p2: Vec3, r: Vec3, s: Vec3, plane: Plane) -> Option<(f64, f64)> {
    let flat = |v: Vec3| match plane {
        Plane::X => [v[1], 0.0, v[2]],
        Plane::Y => v,
        Plane::Z => [v[0], 0.0, v[1]],
    };
    bezier::intersect(flat(p1), flat(p2), flat(r), flat(s))
}

/// Minecraft's `Mirror`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mirror {
    None,
    LeftRight,
    FrontBack,
}

/// Minecraft's `Rotation`, clockwise quarter turns in its ordinal order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotation {
    None,
    Clockwise90,
    Clockwise180,
    Counterclockwise90,
}

/// `TrackShape`: a track block's `shape` property. Orthogonal (`Zo`, `Xo`), diagonal (`Pd`,
/// `Nd`), ascending toward a side (`An`..`Aw`), portal ends (`Tn`..`Tw`) and crossings (`Cr*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackShape {
    None,
    Zo,
    Xo,
    Pd,
    Nd,
    An,
    As,
    Ae,
    Aw,
    Tn,
    Ts,
    Te,
    Tw,
    CrO,
    CrD,
    CrPdx,
    CrPdz,
    CrNdx,
    CrNdz,
}

impl TrackShape {
    /// In declaration order (`values()`).
    pub const ALL: [TrackShape; 19] = [
        TrackShape::None,
        TrackShape::Zo,
        TrackShape::Xo,
        TrackShape::Pd,
        TrackShape::Nd,
        TrackShape::An,
        TrackShape::As,
        TrackShape::Ae,
        TrackShape::Aw,
        TrackShape::Tn,
        TrackShape::Ts,
        TrackShape::Te,
        TrackShape::Tw,
        TrackShape::CrO,
        TrackShape::CrD,
        TrackShape::CrPdx,
        TrackShape::CrPdz,
        TrackShape::CrNdx,
        TrackShape::CrNdz,
    ];

    /// `getSerializedName`: the state property's value.
    pub fn name(self) -> &'static str {
        match self {
            TrackShape::None => "none",
            TrackShape::Zo => "zo",
            TrackShape::Xo => "xo",
            TrackShape::Pd => "pd",
            TrackShape::Nd => "nd",
            TrackShape::An => "an",
            TrackShape::As => "as",
            TrackShape::Ae => "ae",
            TrackShape::Aw => "aw",
            TrackShape::Tn => "tn",
            TrackShape::Ts => "ts",
            TrackShape::Te => "te",
            TrackShape::Tw => "tw",
            TrackShape::CrO => "cr_o",
            TrackShape::CrD => "cr_d",
            TrackShape::CrPdx => "cr_pdx",
            TrackShape::CrPdz => "cr_pdz",
            TrackShape::CrNdx => "cr_ndx",
            TrackShape::CrNdz => "cr_ndz",
        }
    }

    pub fn read(name: &str) -> Option<TrackShape> {
        TrackShape::ALL.into_iter().find(|s| s.name() == name)
    }

    /// `getAxes`: the ways the track runs (unnormalized: a diagonal is (1, 0, 1), a ramp rises
    /// one block per block); two for a crossing.
    pub fn axes(self) -> &'static [Vec3] {
        match self {
            TrackShape::None => &[ZERO],
            TrackShape::Zo => &[[0.0, 0.0, 1.0]],
            TrackShape::Xo => &[[1.0, 0.0, 0.0]],
            TrackShape::Pd => &[[1.0, 0.0, 1.0]],
            TrackShape::Nd => &[[-1.0, 0.0, 1.0]],
            TrackShape::An => &[[0.0, 1.0, -1.0]],
            TrackShape::As => &[[0.0, 1.0, 1.0]],
            TrackShape::Ae => &[[1.0, 1.0, 0.0]],
            TrackShape::Aw => &[[-1.0, 1.0, 0.0]],
            TrackShape::Tn => &[[0.0, 0.0, -1.0]],
            TrackShape::Ts => &[[0.0, 0.0, 1.0]],
            TrackShape::Te => &[[1.0, 0.0, 0.0]],
            TrackShape::Tw => &[[-1.0, 0.0, 0.0]],
            TrackShape::CrO => &[[0.0, 0.0, 1.0], [1.0, 0.0, 0.0]],
            TrackShape::CrD => &[[1.0, 0.0, 1.0], [-1.0, 0.0, 1.0]],
            TrackShape::CrPdx => &[[1.0, 0.0, 0.0], [1.0, 0.0, 1.0]],
            TrackShape::CrPdz => &[[0.0, 0.0, 1.0], [1.0, 0.0, 1.0]],
            TrackShape::CrNdx => &[[1.0, 0.0, 0.0], [-1.0, 0.0, 1.0]],
            TrackShape::CrNdz => &[[0.0, 0.0, 1.0], [-1.0, 0.0, 1.0]],
        }
    }

    /// `getNormal`: the track's up, normalized; a ramp's leans back from its rise.
    pub fn normal(self) -> Vec3 {
        normalize(match self {
            TrackShape::An => [0.0, 1.0, 1.0],
            TrackShape::As => [0.0, 1.0, -1.0],
            TrackShape::Ae => [-1.0, 1.0, 0.0],
            TrackShape::Aw => [1.0, 1.0, 0.0],
            _ => UP,
        })
    }

    /// `getModel`.
    pub fn model(self) -> &'static str {
        match self {
            TrackShape::None => "",
            TrackShape::Zo => "z_ortho",
            TrackShape::Xo => "x_ortho",
            TrackShape::Pd => "diag",
            TrackShape::Nd => "diag_2",
            TrackShape::An | TrackShape::As | TrackShape::Ae | TrackShape::Aw => "ascending",
            TrackShape::Tn | TrackShape::Ts | TrackShape::Te | TrackShape::Tw => "teleport",
            TrackShape::CrO => "cross_ortho",
            TrackShape::CrD => "cross_diag",
            TrackShape::CrPdx => "cross_d1_xo",
            TrackShape::CrPdz => "cross_d1_zo",
            TrackShape::CrNdx => "cross_d2_xo",
            TrackShape::CrNdz => "cross_d2_zo",
        }
    }

    /// `getModelRotation`, degrees.
    pub fn model_rotation(self) -> i32 {
        match self {
            TrackShape::An | TrackShape::Tn => 180,
            TrackShape::Ae | TrackShape::Te => 270,
            TrackShape::Aw | TrackShape::Tw => 90,
            _ => 0,
        }
    }

    /// `isJunction`: more than one axis.
    pub fn is_junction(self) -> bool {
        self.axes().len() > 1
    }

    /// `isPortal`.
    pub fn is_portal(self) -> bool {
        matches!(
            self,
            TrackShape::Te | TrackShape::Tn | TrackShape::Ts | TrackShape::Tw
        )
    }

    /// `asPortal(horizontalFacing)`: west for anything not east, north or south.
    pub fn as_portal(facing: Dir) -> TrackShape {
        match facing {
            Dir::East => TrackShape::Te,
            Dir::North => TrackShape::Tn,
            Dir::South => TrackShape::Ts,
            _ => TrackShape::Tw,
        }
    }

    /// The shape `placeTracks` lays along an end: a portal end becomes the straight track it
    /// leads into.
    pub fn without_portal(self) -> TrackShape {
        match self {
            TrackShape::Te | TrackShape::Tw => TrackShape::Xo,
            TrackShape::Tn | TrackShape::Ts => TrackShape::Zo,
            s => s,
        }
    }

    /// `zMirror`.
    fn z_mirror(self) -> TrackShape {
        match self {
            TrackShape::Pd => TrackShape::Nd,
            TrackShape::Nd => TrackShape::Pd,
            TrackShape::An => TrackShape::As,
            TrackShape::As => TrackShape::An,
            TrackShape::CrPdx => TrackShape::CrNdx,
            TrackShape::CrNdx => TrackShape::CrPdx,
            TrackShape::CrPdz => TrackShape::CrNdz,
            TrackShape::CrNdz => TrackShape::CrPdz,
            s => s,
        }
    }

    /// `xMirror`.
    fn x_mirror(self) -> TrackShape {
        match self {
            TrackShape::Pd => TrackShape::Nd,
            TrackShape::Nd => TrackShape::Pd,
            TrackShape::Ae => TrackShape::Aw,
            TrackShape::Aw => TrackShape::Ae,
            TrackShape::CrPdx => TrackShape::CrNdx,
            TrackShape::CrNdx => TrackShape::CrPdx,
            TrackShape::CrPdz => TrackShape::CrNdz,
            TrackShape::CrNdz => TrackShape::CrPdz,
            s => s,
        }
    }

    /// `clockwise`.
    fn clockwise(self) -> TrackShape {
        match self {
            TrackShape::Pd => TrackShape::Nd,
            TrackShape::Nd => TrackShape::Pd,
            TrackShape::Xo => TrackShape::Zo,
            TrackShape::Zo => TrackShape::Xo,
            TrackShape::Ae => TrackShape::As,
            TrackShape::As => TrackShape::Aw,
            TrackShape::Aw => TrackShape::An,
            TrackShape::An => TrackShape::Ae,
            TrackShape::CrPdx => TrackShape::CrNdz,
            TrackShape::CrNdx => TrackShape::CrPdz,
            TrackShape::CrPdz => TrackShape::CrNdx,
            TrackShape::CrNdz => TrackShape::CrPdx,
            s => s,
        }
    }

    /// `mirror`: `FRONT_BACK` by `xMirror`, `LEFT_RIGHT` by `zMirror`.
    pub fn mirror(self, mirror: Mirror) -> TrackShape {
        match mirror {
            Mirror::None => self,
            Mirror::FrontBack => self.x_mirror(),
            Mirror::LeftRight => self.z_mirror(),
        }
    }

    /// `rotate`: `clockwise` once per quarter turn. Portal ends keep their shape, as in Create.
    pub fn rotate(self, rotation: Rotation) -> TrackShape {
        let turns = match rotation {
            Rotation::None => 0,
            Rotation::Clockwise90 => 1,
            Rotation::Clockwise180 => 2,
            Rotation::Counterclockwise90 => 3,
        };
        (0..turns).fold(self, |s, _| s.clockwise())
    }

    /// `TrackBlock.getStateForPlacement`: the straight, diagonal or ascending shape nearest the
    /// player's horizontal look (`look`, normalized by the caller; Create turns (0, 0, 1) by
    /// the yaw when looking straight up or down), a later shape winning a tie; then an
    /// orthogonal track becomes a ramp up toward a side where `ramp_toward(negative)` holds
    /// (the block that way has a sturdy top and the one above it no sturdy bottom), the
    /// positive side checked last.
    pub fn for_placement(look: Vec3, ramp_toward: impl Fn(bool) -> bool) -> TrackShape {
        let mut best = TrackShape::Zo;
        let mut best_value = f64::from(f32::MAX);
        for shape in TrackShape::ALL {
            if shape.is_junction() || shape.is_portal() {
                continue;
            }
            let axis = shape.axes()[0];
            let distance =
                distance_sqr(axis, look).min(distance_sqr(scale(normalize(axis), -1.0), look));
            if distance > best_value {
                continue;
            }
            best_value = distance;
            best = shape;
        }
        if dot(best.axes()[0], best.axes()[0]) == 1.0 {
            for negative in [true, false] {
                if ramp_toward(negative) {
                    if best == TrackShape::Xo {
                        best = if negative {
                            TrackShape::Aw
                        } else {
                            TrackShape::Ae
                        };
                    }
                    if best == TrackShape::Zo {
                        best = if negative {
                            TrackShape::An
                        } else {
                            TrackShape::As
                        };
                    }
                }
            }
        }
        best
    }

    /// `ITrackBlock.getNearestTrackAxis` signed as its callers sign it: the axis nearest the
    /// look (either way, a later one winning a tie), pointing along the look, away from the player
    /// (negated when the look runs against its horizontal part).
    pub fn nearest_axis(self, look: Vec3) -> Vec3 {
        let mut best = ZERO;
        let mut best_diff = f64::MAX;
        for &axis in self.axes() {
            for opposite in [1.0, -1.0] {
                let d = distance_sqr(normalize(axis), scale(look, opposite)).sqrt();
                if d > best_diff {
                    continue;
                }
                best_diff = d;
                best = axis;
            }
        }
        let flat = normalize([best[0], 0.0, best[2]]);
        if dot(look, flat) < 0.0 {
            scale(best, -1.0)
        } else {
            best
        }
    }
}

/// `TrackBlock.getCurveStart`: where a curve leaves a track block along `axis`, half a block
/// out from its centre, at its floor unless the axis climbs.
pub fn curve_start(pos: BlockPos, axis: Vec3) -> Vec3 {
    let vertical = axis[1] != 0.0;
    let c = center(pos);
    add(
        [c[0], c[1] + if vertical { 0.0 } else { -0.5 }, c[2]],
        scale(axis, 0.5),
    )
}

/// `TrackPlacement.ConnectingFrom`: one end of a connection, the track at `pos` along `axis`
/// with its up `normal` and the curve's start `end`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct End {
    pub pos: BlockPos,
    pub axis: Vec3,
    pub normal: Vec3,
    pub end: Vec3,
}

impl End {
    /// `TrackBlockItem.select` (and the second end in `tryConnect`): the track at `pos` of
    /// `shape`, along the axis nearest the player's `look`.
    pub fn select(pos: BlockPos, shape: TrackShape, look: Vec3) -> End {
        let axis = shape.nearest_axis(look);
        End {
            pos,
            axis,
            normal: normalize(shape.normal()),
            end: curve_start(pos, axis),
        }
    }
}

/// Why a connection can't be made: the `track.<key>` message Create shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Both clicks on the same block.
    SecondPoint,
    /// Further apart than `maxTrackPlacementLength`.
    TooFar,
    /// The first track is gone.
    OriginalMissing,
    /// The second track already has a turn that tilts it.
    TurnStart,
    /// Side by side, facing past each other.
    Perpendicular,
    /// An S-bend that climbs, or two ramps facing each other.
    AscendingSCurve,
    TooSharp,
    /// A turn onto or off a ramp.
    SlopeTurn,
    /// Two ramps leaning opposite ways.
    OpposingSlopes,
    LeaveSlopeAscending,
    LeaveSlopeDescending,
    TooSteep,
    /// Less than 60° off straight ahead.
    Turn90,
    NotEnoughTracks,
    NotEnoughPavement,
}

impl Failure {
    /// The lang key's last part (`track.` + this).
    pub fn key(self) -> &'static str {
        match self {
            Failure::SecondPoint => "second_point",
            Failure::TooFar => "too_far",
            Failure::OriginalMissing => "original_missing",
            Failure::TurnStart => "turn_start",
            Failure::Perpendicular => "perpendicular",
            Failure::AscendingSCurve => "ascending_s_curve",
            Failure::TooSharp => "too_sharp",
            Failure::SlopeTurn => "slope_turn",
            Failure::OpposingSlopes => "opposing_slopes",
            Failure::LeaveSlopeAscending => "leave_slope_ascending",
            Failure::LeaveSlopeDescending => "leave_slope_descending",
            Failure::TooSteep => "too_steep",
            Failure::Turn90 => "turn_90",
            Failure::NotEnoughTracks => "not_enough_tracks",
            Failure::NotEnoughPavement => "not_enough_pavement",
        }
    }
}

/// `CTrains.maxTrackPlacementLength` default (16 to 128).
pub const MAX_PLACEMENT_LENGTH: i32 = 32;

/// What `tryConnect` reads besides the two ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Request {
    /// The first track still has a `has_be` property (`original_missing` otherwise).
    pub original_present: bool,
    /// The second track's block entity is tilted (`turn_start`).
    pub second_tilted: bool,
    pub max_length: i32,
    /// Sneaking: the turn as large as the ends allow, with no straight lead-in.
    pub maximise_turn: bool,
    /// On the client Create builds the curve before the checks, to draw a failed one red;
    /// the server doesn't.
    pub preview: bool,
}

/// `PlacementInfo`: the ends as `tryConnect` leaves them (axes pointing at each other), how
/// many straight tracks each end lays before the curve (`end*Extent`), and the curve between
/// the blocks those reach (`targets`), none for straight track.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub valid: bool,
    pub message: Option<Failure>,
    pub curve: Option<Curve>,
    pub extents: [i32; 2],
    pub positions: [BlockPos; 2],
    pub targets: [BlockPos; 2],
    pub ends: [Vec3; 2],
    pub axes: [Vec3; 2],
    pub normals: [Vec3; 2],
}

impl Placement {
    fn fail(mut self, message: Failure) -> Placement {
        self.message = Some(message);
        self
    }

    /// `withMessage(..).tooJumbly()`: no curve to draw.
    fn jumbly(mut self, message: Failure) -> Placement {
        self.curve = None;
        self.fail(message)
    }

    /// The blocks `placeTracks` lays straight track on from end `i`: its extent (one more
    /// with a curve, the curve's own end block) along its axis from its track.
    pub fn straight(&self, i: usize) -> impl Iterator<Item = BlockPos> + '_ {
        let count = self.extents[i] + i32::from(self.curve.is_some());
        (0..count.max(0)).map(move |n| offset(self.positions[i], scale(self.axes[i], f64::from(n))))
    }

    /// `placeTracks`: the tracks a curve costs on top of its straight ones (one per two
    /// sleeper segments), when the ends weren't joined already.
    pub fn turn_tracks(&self) -> i32 {
        self.curve
            .as_ref()
            .map_or(0, |c| (c.segments as i32 + 1) / 2)
    }

    fn curve_now(&self) -> Curve {
        let offsets = [
            scale(self.axes[0], f64::from(self.extents[0])),
            scale(self.axes[1], f64::from(self.extents[1])),
        ];
        Curve::new(
            [add(self.ends[0], offsets[0]), add(self.ends[1], offsets[1])],
            [normalize(self.axes[0]), normalize(self.axes[1])],
            self.normals,
        )
    }
}

/// `TrackPlacement.tryConnect` up to the item checks: whether `from` (the first click) and
/// `to` (the track under the second) can be joined, and how.
pub fn try_connect(from: End, to: End, request: Request) -> Placement {
    let (pos1, pos2) = (from.pos, to.pos);
    let (mut axis1, mut axis2) = (from.axis, to.axis);
    let (mut end1, mut end2) = (from.end, to.end);
    let (normal1, normal2) = (from.normal, to.normal);
    let mut info = Placement {
        valid: false,
        message: None,
        curve: None,
        extents: [0, 0],
        positions: [pos1, pos2],
        targets: [pos1, pos2],
        ends: [end1, end2],
        axes: [axis1, axis2],
        normals: [normal1, normal2],
    };

    if pos1 == pos2 {
        return info.fail(Failure::SecondPoint);
    }
    let d = [
        f64::from(pos1[0] - pos2[0]),
        f64::from(pos1[1] - pos2[1]),
        f64::from(pos1[2] - pos2[2]),
    ];
    if dot(d, d) > f64::from(request.max_length * request.max_length) {
        return info.jumbly(Failure::TooFar);
    }
    if !request.original_present {
        return info.fail(Failure::OriginalMissing);
    }
    if request.second_tilted {
        return info.fail(Failure::TurnStart);
    }

    let mut normed1 = normalize(axis1);
    let mut normed2 = normalize(axis2);
    if dot(axis1, sub(end2, end1)) < 0.0 {
        axis1 = scale(axis1, -1.0);
        normed1 = scale(normed1, -1.0);
        end1 = curve_start(pos1, axis1);
    }

    let crossing = intersect(end1, end2, normed1, normed2, Plane::Y);
    let parallel = crossing.is_none();
    let mut skip_curve = false;
    let away = match crossing {
        None => dot(normed1, normed2) > 0.0,
        Some((t, u)) => t < 0.0 || u < 0.0,
    };
    if away {
        axis2 = scale(axis2, -1.0);
        normed2 = scale(normed2, -1.0);
        end2 = curve_start(pos2, axis2);
    }
    info.ends = [end1, end2];
    info.axes = [axis1, axis2];

    let cross2 = cross(normed2, UP);
    let angle = normed2[2].atan2(normed2[0]) - normed1[2].atan2(normed1[0]);
    let ascend = sub(end2, end1)[1];
    let abs_ascend = ascend.abs();
    let slope = normal1 != normal2;

    if request.preview {
        info.curve = Some(info.curve_now());
    }

    // S curve or straight
    let mut dist = 0.0;
    if parallel && let Some((s0, s1)) = intersect(end1, end2, normed1, cross2, Plane::Y) {
        let (t, u) = (s0.abs(), s1.abs());
        skip_curve = equal(u, 0.0);
        if !skip_curve && s0 < 0.0 {
            return info.jumbly(Failure::Perpendicular);
        }
        if skip_curve {
            dist = length(sub(center(pos1), center(pos2)));
            info.extents[0] = round((dist + 1.0) / length(axis1));
        } else {
            if !equal(ascend, 0.0) || normed1[1] != 0.0 {
                return info.fail(Failure::AscendingSCurve);
            }
            let target_t = if u <= 1.0 { 3.0 } else { u * 2.0 };
            if t < target_t {
                return info.fail(Failure::TooSharp);
            }
            // Standard S-bend sizes: the rest goes into straight lead-ins.
            if t > target_t {
                let correction = ((t - target_t) / length(axis1)) as i32;
                if !request.maximise_turn {
                    info.extents = [correction / 2 + correction % 2, correction / 2];
                }
            }
        }
    }

    // Slope
    if slope {
        if !skip_curve {
            return info.fail(Failure::SlopeTurn);
        }
        if equal(dot(normal1, normal2), 0.0) {
            return info.fail(Failure::OpposingSlopes);
        }
        if (axis1[1] < 0.0 || axis2[1] > 0.0) && ascend > 0.0 {
            return info.fail(Failure::LeaveSlopeAscending);
        }
        if (axis1[1] > 0.0 || axis2[1] < 0.0) && ascend < 0.0 {
            return info.fail(Failure::LeaveSlopeDescending);
        }
        skip_curve = false;
        info.extents = [0, 0];
        let plane = if equal(axis1[0], 0.0) {
            Plane::X
        } else {
            Plane::Z
        };
        // A flat end and a ramp in line always cross in their upright plane (Create would
        // throw here otherwise).
        let Some((i0, i1)) = intersect(end1, end2, normed1, normed2, plane) else {
            return info.jumbly(Failure::TooSharp);
        };
        let dist1 = (i0 / length(axis1)).abs();
        let dist2 = (i1 / length(axis2)).abs();
        if dist1 > dist2 {
            info.extents[0] = round(dist1 - dist2);
        }
        if dist2 > dist1 {
            info.extents[1] = round(dist2 - dist1);
        }
        let turn_size = dist1.min(dist2);
        if i0 < 0.0 || i1 < 0.0 {
            return info.jumbly(Failure::TooSharp);
        }
        if turn_size < 2.0 {
            return info.fail(Failure::TooSharp);
        }
        // Standard curve sizes (`end1Extent += turnSize - 2`, truncated).
        if turn_size > 2.0 && !request.maximise_turn {
            info.extents[0] = (f64::from(info.extents[0]) + turn_size - 2.0) as i32;
            info.extents[1] = (f64::from(info.extents[1]) + turn_size - 2.0) as i32;
        }
    }

    // Straight ascend
    if skip_curve && !equal(ascend, 0.0) {
        let h_distance = info.extents[0];
        if axis1[1] == 0.0 || !equal(abs_ascend + 1.0, dist / length(axis1)) {
            if axis1[1] != 0.0 && axis1[1] == -axis2[1] {
                return info.fail(Failure::AscendingSCurve);
            }
            info.extents[0] = 0;
            let rise = if abs_ascend < 4.0 {
                abs_ascend * 4.0
            } else {
                abs_ascend * 3.0
            };
            let min_h_distance = rise.max(6.0) / length(axis1);
            if f64::from(h_distance) < min_h_distance {
                return info.fail(Failure::TooSteep);
            }
            if f64::from(h_distance) > min_h_distance {
                let correction = (f64::from(h_distance) - min_h_distance) as i32;
                info.extents = if request.maximise_turn {
                    [0, 0]
                } else {
                    [correction / 2 + correction % 2, correction / 2]
                };
            }
            skip_curve = false;
        }
    }

    // Turn
    if !parallel {
        let abs_angle = degrees(angle).abs();
        if !(60.0..=300.0).contains(&abs_angle) {
            return info.jumbly(Failure::Turn90);
        }
        let Some((i0, i1)) = intersect(end1, end2, normed1, normed2, Plane::Y) else {
            return info.jumbly(Failure::TooSharp);
        };
        let (dist1, dist2) = (i0.abs(), i1.abs());
        let mut ex1 = 0.0f32;
        let mut ex2 = 0.0f32;
        if dist1 > dist2 {
            ex1 = ((dist1 - dist2) / length(axis1)) as f32;
        }
        if dist2 > dist1 {
            ex2 = ((dist2 - dist1) / length(axis2)) as f32;
        }
        let turn_size = dist1.min(dist2) - 0.1;
        let ninety = (abs_angle + 0.25) % 90.0 < 1.0;
        if i0 < 0.0 || i1 < 0.0 {
            return info.jumbly(Failure::TooSharp);
        }
        let min_turn_size = if ninety { 7.0 } else { 3.25 };
        let to_fit_ascend = min_turn_size
            + if ninety {
                (abs_ascend - 3.0).max(0.0) * 2.0
            } else {
                (abs_ascend - 1.5).max(0.0) * 1.5
            };
        if turn_size < min_turn_size {
            return info.fail(Failure::TooSharp);
        }
        if turn_size < to_fit_ascend {
            return info.fail(Failure::TooSteep);
        }
        // Standard curve sizes.
        if !request.maximise_turn {
            ex1 = (f64::from(ex1) + (turn_size - to_fit_ascend) / length(axis1)) as f32;
            ex2 = (f64::from(ex2) + (turn_size - to_fit_ascend) / length(axis2)) as f32;
        }
        info.extents = [ex1.floor() as i32, ex2.floor() as i32];
    }

    info.targets = [
        offset(pos1, scale(axis1, f64::from(info.extents[0]))),
        offset(pos2, scale(axis2, f64::from(info.extents[1]))),
    ];
    info.curve = if skip_curve {
        None
    } else {
        Some(info.curve_now())
    };
    info.valid = true;
    info
}

/// `AngleHelper.deg`: radians to degrees as a float.
fn degrees(angle: f64) -> f32 {
    if angle == 0.0 {
        0.0
    } else {
        (angle * 180.0 / std::f64::consts::PI) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASK: Request = Request {
        original_present: true,
        second_tilted: false,
        max_length: MAX_PLACEMENT_LENGTH,
        maximise_turn: false,
        preview: false,
    };
    const NORTH: Vec3 = [0.0, 0.0, -1.0];

    fn connect(
        a: (BlockPos, TrackShape),
        b: (BlockPos, TrackShape),
        request: Request,
    ) -> Placement {
        try_connect(
            End::select(a.0, a.1, NORTH),
            End::select(b.0, b.1, NORTH),
            request,
        )
    }
    fn close(a: Vec3, b: Vec3) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
    }

    #[test]
    fn shapes_run_their_ways() {
        assert_eq!(TrackShape::read("cr_pdx"), Some(TrackShape::CrPdx));
        assert!(
            TrackShape::ALL
                .iter()
                .all(|s| TrackShape::read(s.name()) == Some(*s))
        );
        assert!(TrackShape::CrD.is_junction() && !TrackShape::Pd.is_junction());
        assert!(TrackShape::Te.is_portal() && TrackShape::Te.without_portal() == TrackShape::Xo);
        assert_eq!(TrackShape::as_portal(Dir::Up), TrackShape::Tw);
        let h = std::f64::consts::FRAC_1_SQRT_2;
        assert!(close(TrackShape::As.normal(), [0.0, h, -h]));
        assert_eq!(TrackShape::Zo.normal(), UP);
        // A ramp rising east turns to one rising south; four turns come back round.
        assert_eq!(TrackShape::Ae.rotate(Rotation::Clockwise90), TrackShape::As);
        assert_eq!(
            TrackShape::An.rotate(Rotation::Counterclockwise90),
            TrackShape::Aw
        );
        for s in TrackShape::ALL {
            let r = (0..4).fold(s, |s, _| s.rotate(Rotation::Clockwise90));
            assert_eq!(r, s);
        }
        assert_eq!(TrackShape::Ae.mirror(Mirror::FrontBack), TrackShape::Aw);
        assert_eq!(TrackShape::Ae.mirror(Mirror::LeftRight), TrackShape::Ae);
        assert_eq!(TrackShape::An.mirror(Mirror::LeftRight), TrackShape::As);
    }

    #[test]
    fn placement_follows_the_look() {
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let flat = |_| false;
        assert_eq!(TrackShape::for_placement(NORTH, flat), TrackShape::Zo);
        assert_eq!(
            TrackShape::for_placement([1.0, 0.0, 0.0], flat),
            TrackShape::Xo
        );
        assert_eq!(TrackShape::for_placement([h, 0.0, h], flat), TrackShape::Pd);
        assert_eq!(
            TrackShape::for_placement([h, 0.0, -h], flat),
            TrackShape::Nd
        );
        // A step up to the north makes a ramp rising north; diagonals never ramp.
        assert_eq!(TrackShape::for_placement(NORTH, |neg| neg), TrackShape::An);
        assert_eq!(TrackShape::for_placement(NORTH, |neg| !neg), TrackShape::As);
        assert_eq!(
            TrackShape::for_placement([h, 0.0, h], |_| true),
            TrackShape::Pd
        );
    }

    #[test]
    fn the_first_click_picks_the_end_toward_the_player() {
        // A z track looked at northward: its axis turned to point north, and the curve starts
        // at its north edge, on its floor.
        let e = End::select([0, 64, 0], TrackShape::Zo, NORTH);
        assert_eq!(e.axis, [0.0, 0.0, -1.0]);
        assert_eq!(e.end, [0.5, 64.0, 0.0]);
        // A ramp's curve starts mid-height on its edge.
        let r = End::select([0, 64, 0], TrackShape::As, [0.0, 0.0, 1.0]);
        assert_eq!(r.axis, [0.0, 1.0, 1.0]);
        assert_eq!(r.end, [0.5, 65.0, 1.0]);
    }

    #[test]
    fn straight_runs_and_their_limits() {
        let z = TrackShape::Zo;
        assert_eq!(
            connect(([0, 0, 0], z), ([0, 0, 0], z), ASK).message,
            Some(Failure::SecondPoint)
        );
        // Ten blocks on: eleven tracks from the first, no curve.
        let p = connect(([0, 0, 0], z), ([0, 0, 10], z), ASK);
        assert!(p.valid && p.curve.is_none(), "{p:?}");
        assert_eq!(p.extents, [11, 0]);
        let laid: Vec<_> = p.straight(0).collect();
        assert_eq!(laid.first(), Some(&[0, 0, 0]));
        assert_eq!(laid.last(), Some(&[0, 0, 10]));
        // 32 is the default limit; 33 is too far and draws nothing.
        assert!(connect(([0, 0, 0], z), ([0, 0, 32], z), ASK).valid);
        let far = connect(
            ([0, 0, 0], z),
            ([0, 0, 33], z),
            Request {
                preview: true,
                ..ASK
            },
        );
        assert_eq!(far.message, Some(Failure::TooFar));
        assert!(far.curve.is_none());
        let gone = Request {
            original_present: false,
            ..ASK
        };
        assert_eq!(
            connect(([0, 0, 0], z), ([0, 0, 5], z), gone).message,
            Some(Failure::OriginalMissing)
        );
        let tilted = Request {
            second_tilted: true,
            ..ASK
        };
        assert_eq!(
            connect(([0, 0, 0], z), ([0, 0, 5], z), tilted).message,
            Some(Failure::TurnStart)
        );
        // Side by side: perpendicular, and nothing drawn.
        let side = connect(
            ([0, 0, 0], z),
            ([3, 0, 0], z),
            Request {
                preview: true,
                ..ASK
            },
        );
        assert_eq!(side.message, Some(Failure::Perpendicular));
        assert!(side.curve.is_none());
    }

    #[test]
    fn s_bends_are_twice_their_offset_long() {
        let z = TrackShape::Zo;
        // Offset 2 needs 4 along; 9 leaves 5 for lead-ins, 3 at the first end, 2 at the second.
        let p = connect(([0, 0, 0], z), ([2, 0, 10], z), ASK);
        assert!(p.valid, "{p:?}");
        assert_eq!(p.extents, [3, 2]);
        assert_eq!(p.targets, [[0, 0, 3], [2, 0, 8]]);
        let c = p.curve.as_ref().unwrap();
        assert_eq!(c.starts, [[0.5, 0.0, 4.0], [2.5, 0.0, 8.0]]);
        assert_eq!(c.radius, 0.0);
        assert_eq!(p.straight(0).count(), 4);
        // Sneaking keeps the whole run as the bend.
        let max = connect(
            ([0, 0, 0], z),
            ([2, 0, 10], z),
            Request {
                maximise_turn: true,
                ..ASK
            },
        );
        assert_eq!(max.extents, [0, 0]);
        // Offset 2 in 3 is too sharp (and still drawn); offset 1 needs 3.
        let sharp = connect(
            ([0, 0, 0], z),
            ([2, 0, 4], z),
            Request {
                preview: true,
                ..ASK
            },
        );
        assert_eq!(sharp.message, Some(Failure::TooSharp));
        assert!(sharp.curve.is_some());
        assert_eq!(
            connect(([0, 0, 0], z), ([1, 0, 3], z), ASK).message,
            Some(Failure::TooSharp)
        );
        assert!(connect(([0, 0, 0], z), ([1, 0, 4], z), ASK).valid);
        // An S-bend can't climb.
        assert_eq!(
            connect(([0, 0, 0], z), ([2, 1, 10], z), ASK).message,
            Some(Failure::AscendingSCurve)
        );
    }

    #[test]
    fn ninety_degree_turns_need_seven_blocks() {
        let (z, x) = (TrackShape::Zo, TrackShape::Xo);
        // The ends' lines cross 9.5 out: a turn of 9.4, standardised to 7 with 2 straight each.
        let p = connect(([0, 0, 0], z), ([10, 0, 10], x), ASK);
        assert!(p.valid, "{p:?}");
        assert_eq!(p.axes, [[0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]]);
        assert_eq!(p.extents, [2, 2]);
        let c = p.curve.as_ref().unwrap();
        assert_eq!(c.starts, [[0.5, 0.0, 3.0], [8.0, 0.0, 10.5]]);
        assert!((c.radius - 7.5).abs() < 1e-9, "{}", c.radius);
        let max = connect(
            ([0, 0, 0], z),
            ([10, 0, 10], x),
            Request {
                maximise_turn: true,
                ..ASK
            },
        );
        assert_eq!(max.extents, [0, 0]);
        assert!((max.curve.unwrap().radius - 9.5).abs() < 1e-9);
        // 7.5 out leaves 7.4: enough; 6.5 out is too sharp.
        assert!(connect(([0, 0, 0], z), ([8, 0, 8], x), ASK).valid);
        assert_eq!(
            connect(([0, 0, 0], z), ([7, 0, 7], x), ASK).message,
            Some(Failure::TooSharp)
        );
        // Climbing 3 fits a 7 turn; 5 needs 7 + 2×2 = 11.
        assert!(connect(([0, 0, 0], z), ([10, 3, 10], x), ASK).valid);
        assert_eq!(
            connect(([0, 0, 0], z), ([10, 5, 10], x), ASK).message,
            Some(Failure::TooSteep)
        );
    }

    #[test]
    fn forty_five_degree_turns_need_three_and_a_quarter() {
        let (z, d) = (TrackShape::Zo, TrackShape::Pd);
        // From (0.5, 1) north-south to (5, 10) on a diagonal: the lines cross 4.5 and 4.5√2 out.
        let p = connect(([0, 0, 0], z), ([5, 0, 10], d), ASK);
        assert!(p.valid, "{p:?}");
        assert_eq!(p.axes[1], [-1.0, 0.0, -1.0]);
        // 4.4 − 3.25 = 1.15 more each: ⌊1.15⌋ = 1, ⌊(4.5√2 − 4.5 + 1.15) / √2⌋ = 2.
        assert_eq!(p.extents, [1, 2]);
        assert_eq!(p.targets, [[0, 0, 1], [3, 0, 8]]);
        // Straight on into a diagonal is under 60° off: no turn, nothing drawn.
        let on = connect(
            ([0, 0, 0], z),
            ([5, 0, 10], TrackShape::Nd),
            Request {
                preview: true,
                ..ASK
            },
        );
        assert_eq!(on.message, Some(Failure::Turn90));
        assert!(on.curve.is_none());
    }

    #[test]
    fn ramps_rise_one_in_four_up_to_four_then_one_in_three() {
        let z = TrackShape::Zo;
        // Up 2 over 12: 13 laid would do, 8 needed, 5 to spare as lead-ins; a curve climbs.
        let p = connect(([0, 0, 0], z), ([0, 2, 12], z), ASK);
        assert!(p.valid, "{p:?}");
        assert_eq!(p.extents, [3, 2]);
        assert!(p.curve.is_some());
        // Up 2 over 6: 7 is under 8.
        assert_eq!(
            connect(([0, 0, 0], z), ([0, 2, 6], z), ASK).message,
            Some(Failure::TooSteep)
        );
        // Up 1 still needs 6; up 5 needs 15.
        assert!(connect(([0, 0, 0], z), ([0, 1, 5], z), ASK).valid);
        assert_eq!(
            connect(([0, 0, 0], z), ([0, 1, 4], z), ASK).message,
            Some(Failure::TooSteep)
        );
        assert!(connect(([0, 0, 0], z), ([0, 5, 15], z), ASK).valid);
        assert_eq!(
            connect(([0, 0, 0], z), ([0, 5, 12], z), ASK).message,
            Some(Failure::TooSteep)
        );
    }

    #[test]
    fn ramp_tracks_join_along_their_slope() {
        let s = TrackShape::As;
        // Three up and three on: the slope itself, four ramp blocks and no curve.
        let p = connect(([0, 0, 0], s), ([0, 3, 3], s), ASK);
        assert!(p.valid && p.curve.is_none(), "{p:?}");
        assert_eq!(p.extents, [4, 0]);
        let laid: Vec<_> = p.straight(0).collect();
        assert_eq!(laid, [[0, 0, 0], [0, 1, 1], [0, 2, 2], [0, 3, 3]]);
        // Two ramps off the same line climb into each other.
        assert_eq!(
            connect(([0, 0, 0], s), ([0, 3, 6], s), ASK).message,
            Some(Failure::AscendingSCurve)
        );
    }

    #[test]
    fn leaving_a_ramp() {
        let (z, s) = (TrackShape::Zo, TrackShape::As);
        // Flat at 0 to a ramp topping out at 3, 10 on: the lines meet 6 and 3 out, a turn of
        // 3 standardised to 2 with a block more at each end.
        let p = connect(([0, 0, 0], z), ([0, 3, 10], s), ASK);
        assert!(p.valid, "{p:?}");
        assert_eq!(p.extents, [4, 1]);
        let c = p.curve.as_ref().unwrap();
        assert_eq!(c.starts, [[0.5, 0.0, 5.0], [0.5, 2.0, 9.0]]);
        assert_eq!(
            connect(([0, 0, 0], z), ([0, 1, 4], s), ASK).message,
            Some(Failure::TooSharp)
        );
        // A ramp below the flat end, falling toward it, leaves the slope going down.
        assert_eq!(
            connect(([0, 0, 0], z), ([0, -3, 10], s), ASK).message,
            Some(Failure::LeaveSlopeDescending)
        );
        // A turn onto a ramp, and two ramps leaning apart.
        assert_eq!(
            connect(([0, 0, 0], z), ([10, 2, 10], TrackShape::Ae), ASK).message,
            Some(Failure::SlopeTurn)
        );
        assert_eq!(
            connect(([0, 0, 0], s), ([0, 0, 6], TrackShape::An), ASK).message,
            Some(Failure::OpposingSlopes)
        );
    }

    #[test]
    fn failure_keys_are_create_s() {
        assert_eq!(Failure::Turn90.key(), "turn_90");
        assert_eq!(Failure::AscendingSCurve.key(), "ascending_s_curve");
        assert_eq!(
            Failure::LeaveSlopeDescending.key(),
            "leave_slope_descending"
        );
    }
}
