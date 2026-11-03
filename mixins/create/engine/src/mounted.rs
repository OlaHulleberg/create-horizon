//! Create's minecart contraptions (`MountedContraption`, `OrientedContraptionEntity`,
//! `MinecartContraptionItem`, `CartAssemblerBlockEntity.CartMovementMode`): the structure a cart
//! carries, the way it turns to follow its cart, where it goes back into the world, and its
//! motion as clients draw it (one part model gliding and turning on the GPU, sent again only
//! when the cart's motion leaves the prediction).

use crate::kinetics::Dir;

/// `CartAssemblerBlockEntity.CartMovementMode`, the assembler's value box
/// (`contraptions.cart_movement_mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Rotate,
    RotatePaused,
    RotationLocked,
}

/// The modes' names in Create's lang (`create.contraptions.cart_movement_mode.*`), the board's
/// options in order.
pub const MODE_OPTIONS: [&str; 3] = [
    "Always face toward motion",
    "Pause actors while rotating",
    "Lock rotation",
];

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Rotate, Mode::RotatePaused, Mode::RotationLocked];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: i32) -> Option<Mode> {
        usize::try_from(i)
            .ok()
            .and_then(|i| Mode::ALL.get(i).copied())
    }

    /// `NBTHelper.writeEnum` (`RotationMode`).
    pub fn name(self) -> &'static str {
        match self {
            Mode::Rotate => "ROTATE",
            Mode::RotatePaused => "ROTATE_PAUSED",
            Mode::RotationLocked => "ROTATION_LOCKED",
        }
    }

    pub fn read(name: &str) -> Mode {
        Mode::ALL
            .into_iter()
            .find(|m| m.name() == name)
            .unwrap_or_default()
    }

    /// `ScrollOptionBehaviour`: a scroll moves one option per step, clamped.
    pub fn scrolled(self, steps: i32) -> Mode {
        Mode::from_index((self.index() as i32 + steps).clamp(0, 2)).unwrap_or(self)
    }
}

/// `RailShape`, as rails' `shape` property names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RailShape {
    NorthSouth,
    EastWest,
    AscendingEast,
    AscendingWest,
    AscendingNorth,
    AscendingSouth,
    SouthEast,
    SouthWest,
    NorthWest,
    NorthEast,
}

impl RailShape {
    pub fn read(v: &str) -> Option<RailShape> {
        Some(match v {
            "north_south" => RailShape::NorthSouth,
            "east_west" => RailShape::EastWest,
            "ascending_east" => RailShape::AscendingEast,
            "ascending_west" => RailShape::AscendingWest,
            "ascending_north" => RailShape::AscendingNorth,
            "ascending_south" => RailShape::AscendingSouth,
            "south_east" => RailShape::SouthEast,
            "south_west" => RailShape::SouthWest,
            "north_west" => RailShape::NorthWest,
            "north_east" => RailShape::NorthEast,
            _ => return None,
        })
    }

    pub fn ascending(self) -> bool {
        matches!(
            self,
            RailShape::AscendingEast
                | RailShape::AscendingWest
                | RailShape::AscendingNorth
                | RailShape::AscendingSouth
        )
    }

    /// `AbstractMinecart.EXITS` (`MinecartSim2020.MATRIX`): the two ends a rail joins, a block
    /// below for the low end of a slope.
    pub fn exits(self) -> ([i32; 3], [i32; 3]) {
        let (w, e, n, s) = ([-1, 0, 0], [1, 0, 0], [0, 0, -1], [0, 0, 1]);
        let below = |v: [i32; 3]| [v[0], -1, v[2]];
        match self {
            RailShape::NorthSouth => (n, s),
            RailShape::EastWest => (w, e),
            RailShape::AscendingEast => (below(w), e),
            RailShape::AscendingWest => (w, below(e)),
            RailShape::AscendingNorth => (n, below(s)),
            RailShape::AscendingSouth => (below(n), s),
            RailShape::SouthEast => (s, e),
            RailShape::SouthWest => (s, w),
            RailShape::NorthWest => (n, w),
            RailShape::NorthEast => (n, e),
        }
    }

    /// `MinecartSim2020.getRailVec`.
    pub fn vec(self) -> [f64; 3] {
        let d = std::f64::consts::FRAC_1_SQRT_2;
        match self {
            RailShape::AscendingNorth | RailShape::AscendingSouth | RailShape::NorthSouth => {
                [0.0, 0.0, 1.0]
            }
            RailShape::AscendingEast | RailShape::AscendingWest | RailShape::EastWest => {
                [1.0, 0.0, 0.0]
            }
            RailShape::NorthEast | RailShape::SouthWest => [d, 0.0, d],
            RailShape::NorthWest | RailShape::SouthEast => [d, 0.0, -d],
        }
    }
}

fn length(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// `Vec3.normalize`.
pub fn normalize(v: [f64; 3]) -> [f64; 3] {
    let d = length(v);
    if d < 1.0e-4 {
        [0.0; 3]
    } else {
        v.map(|c| c / d)
    }
}

/// `VecHelper.project`.
pub fn project(v: [f64; 3], onto: [f64; 3]) -> [f64; 3] {
    let l2 = onto[0] * onto[0] + onto[1] * onto[1] + onto[2] * onto[2];
    if l2 == 0.0 {
        return [0.0; 3];
    }
    let dot = v[0] * onto[0] + v[1] * onto[1] + v[2] * onto[2];
    onto.map(|c| c * dot / l2)
}

/// `AbstractContraptionEntity.yawFromVector`: degrees, the way `Direction.toYRot` counts them
/// (south 0, west 90, north 180, east 270).
pub fn yaw_from_vector(v: [f64; 3]) -> f32 {
    ((3.0 * std::f64::consts::PI / 2.0 + v[2].atan2(v[0])) / std::f64::consts::PI * 180.0) as f32
}

/// `Direction.toYRot`.
pub fn y_rot(d: Dir) -> f32 {
    match d {
        Dir::South => 0.0,
        Dir::West => 90.0,
        Dir::North => 180.0,
        Dir::East => 270.0,
        _ => 0.0,
    }
}

/// `AngleHelper.getShortestAngleDiff` (Java's `%`: the dividend's sign).
pub fn shortest_angle_diff(current: f64, target: f64) -> f32 {
    let current = current % 360.0;
    let target = target % 360.0;
    ((((target - current) % 360.0) + 540.0) % 360.0 - 180.0) as f32
}

/// `AngleHelper.getShortestAngleDiff(current, target, hint)`: a half turn goes the way `hint`
/// says.
pub fn shortest_angle_diff_hint(current: f64, target: f64, hint: f32) -> f32 {
    let diff = shortest_angle_diff(current, target);
    if (diff.abs() - 180.0).abs() < 1.0e-5 && diff.signum() != hint.signum() {
        return diff + 360.0 * hint.signum();
    }
    diff
}

/// `Contraption.getRadius(blocks, Y)` then `expandBoundsAroundAxis(Y)`: the contraption's
/// bounding box width along x (`getBoundingBox().getXsize()`).
pub fn x_size(blocks: &[([i32; 3], u32)]) -> f64 {
    let max = blocks
        .iter()
        .map(|(o, _)| o[0] * o[0] + o[2] * o[2])
        .max()
        .unwrap_or(0);
    let radius = f64::from(max).sqrt().ceil() as i32;
    f64::from(2 * radius + 3)
}

/// `OrientedContraptionEntity`'s `yaw`, `prevYaw`, `targetYaw` and `pitch` (only a contraption
/// on a coupling pitches, along the line between its carts).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Orientation {
    pub yaw: f32,
    pub prev_yaw: f32,
    pub target_yaw: f32,
    pub pitch: f32,
}

impl Orientation {
    /// `startAtYaw`.
    pub fn at(yaw: f32) -> Orientation {
        Orientation {
            yaw,
            prev_yaw: yaw,
            target_yaw: yaw,
            pitch: 0.0,
        }
    }
}

/// `OrientedContraptionEntity.updateOrientation` for a contraption on one cart (not on a
/// coupling, not stalled): unless its rotation is locked, it turns toward the cart's motion
/// (`deltaMovement`, along the rail it is on), at most `12 × |motion| / max(1, xSize / 6)`
/// degrees a tick, a half turn going the way the cart moved (`moved`). Whether it is still
/// turning.
pub fn orient(
    o: &mut Orientation,
    mode: Mode,
    motion: [f64; 3],
    moved: [f64; 3],
    rail: Option<RailShape>,
    x_size: f64,
) -> bool {
    o.prev_yaw = o.yaw;
    if mode == Mode::RotationLocked {
        return false;
    }
    let mut m = normalize(motion);
    if let Some(shape) = rail {
        m = project(m, shape.vec());
    }
    let len = length(m);
    if len > 0.0 {
        o.target_yaw = yaw_from_vector(m);
        if o.target_yaw < 0.0 {
            o.target_yaw += 360.0;
        }
        if o.yaw < 0.0 {
            o.yaw += 360.0;
        }
    }
    o.prev_yaw = o.yaw;
    let max = (len * 12.0 / (x_size / 6.0).max(1.0)) as f32;
    let hint = shortest_angle_diff(f64::from(o.yaw), f64::from(yaw_from_vector(moved)));
    let approach =
        shortest_angle_diff_hint(f64::from(o.yaw), f64::from(o.target_yaw), hint).clamp(-max, max);
    o.yaw += approach;
    if shortest_angle_diff(f64::from(o.yaw), f64::from(o.target_yaw)).abs() < 1.0 {
        o.yaw = o.target_yaw;
        false
    } else {
        true
    }
}

/// `makeStructureTransform` (`-yaw + initialYaw`, to the nearest quarter): where each block
/// goes back, offset from the block the cart is in (`BlockPos.containing(anchor + .5)`), and the
/// quarter turn its state takes. The anchor (offset 0, `MinecartAnchorBlock`) is not placed
/// (`customBlockPlacement`).
pub fn placed_back(
    blocks: &[([i32; 3], u32)],
    initial_yaw: f32,
    yaw: f32,
) -> (i32, Vec<([i32; 3], u32)>) {
    let angle = -yaw + initial_yaw;
    let quarter = if angle == 0.0 {
        0
    } else {
        crate::contraption::quarter(angle)
    };
    let turned = blocks
        .iter()
        .filter(|(o, _)| *o != [0, 0, 0])
        .map(|(o, s)| {
            (
                crate::contraption::rotate(*o, quarter, crate::kinetics::Axis::Y),
                *s,
            )
        })
        .collect();
    (quarter, turned)
}

/// `MinecartContraptionItem.useOn`: a cart placed on a rail stands 1/16 up, half a block more
/// on a slope.
pub fn placed_cart(pos: [i32; 3], shape: Option<RailShape>) -> [f64; 3] {
    let up = if shape.is_some_and(RailShape::ascending) {
        0.5
    } else {
        0.0
    };
    [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + 0.0625 + up,
        f64::from(pos[2]) + 0.5,
    ]
}

/// A contraption on a cart: its blocks (offset from the anchor, which the cart is in), its
/// assembler's mode, the way it faced when assembled (`InitialOrientation`), how it is turned
/// now and whether a player placed it (`Placed`).
#[derive(Clone, Debug, PartialEq)]
pub struct Mounted {
    pub blocks: Vec<([i32; 3], u32)>,
    pub mode: Mode,
    pub initial: Dir,
    pub turn: Orientation,
    pub placed: bool,
}

impl Mounted {
    /// `OrientedContraptionEntity.create`: it starts facing its initial orientation.
    pub fn new(blocks: Vec<([i32; 3], u32)>, mode: Mode, initial: Dir) -> Mounted {
        Mounted {
            blocks,
            mode,
            initial,
            turn: Orientation::at(y_rot(initial)),
            placed: false,
        }
    }

    /// `createAtYaw`: placed by a player, facing `yaw`.
    pub fn placed_at(blocks: Vec<([i32; 3], u32)>, mode: Mode, initial: Dir, yaw: f32) -> Mounted {
        Mounted {
            turn: Orientation::at(yaw),
            placed: true,
            ..Mounted::new(blocks, mode, initial)
        }
    }

    /// The turn about up the structure is drawn at, degrees counterclockwise looking down
    /// (`applyLocalTransforms`: `rotateY(-yaw)` then `rotateY(initialYaw)`).
    pub fn angle(&self) -> f32 {
        y_rot(self.initial) - self.turn.yaw
    }
}

/// `AllDataComponents.MINECRAFT_CONTRAPTION_DATA`, kept in `minecraft:custom_data`.
pub const DATA_KEY: &str = "create:minecart_contraption_data";

/// The mod message channel: contraptions on carts, server to client.
pub const CHANNEL: &str = "create:cart";

/// A cart's contraption as clients draw it: the pivot (the anchor block's centre, half a block
/// above the cart), its velocity (blocks a second) and turn (degrees, and degrees a second)
/// until the next update, its pitch and the turn of its initial orientation (`base`: it is drawn
/// `Y(angle - base) · Z(pitch) · Y(base)` about the pivot, `applyLocalTransforms`), and its blocks
/// when the client must learn them.
#[derive(Clone, Debug, PartialEq)]
pub struct Pose {
    pub id: u32,
    pub at: [f64; 3],
    pub velocity: [f32; 3],
    pub angle: f32,
    pub rate: f32,
    pub pitch: f32,
    pub base: f32,
    pub blocks: Option<Vec<([i32; 3], u32)>>,
}

/// Blocks a message carries at most (it stays under 30,000 bytes).
const MAX_SENT_BLOCKS: usize = 1800;

impl Pose {
    /// id, whether blocks follow, pivot, velocity, angle, rate, pitch, base, then the block
    /// count and each block. A message of the id alone clears the cart's contraption.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(61);
        out.extend_from_slice(&self.id.to_le_bytes());
        out.push(u8::from(self.blocks.is_some()));
        for v in self.at {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in self
            .velocity
            .into_iter()
            .chain([self.angle, self.rate, self.pitch, self.base])
        {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if let Some(blocks) = &self.blocks {
            let n = blocks.len().min(MAX_SENT_BLOCKS);
            out.extend_from_slice(&(n as u32).to_le_bytes());
            for (o, s) in &blocks[..n] {
                for v in o {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                out.extend_from_slice(&s.to_le_bytes());
            }
        }
        out
    }

    /// The pose in a message, or the id of a cart to clear.
    pub fn decode(d: &[u8]) -> Result<Pose, Option<u32>> {
        let word = |k: usize| -> Option<[u8; 4]> { d.get(k..k + 4)?.try_into().ok() };
        let id = word(0).map(u32::from_le_bytes).ok_or(None)?;
        if d.len() < 57 {
            return Err(Some(id));
        }
        let float = |k: usize| f64::from_le_bytes(d[k..k + 8].try_into().unwrap_or([0; 8]));
        let single = |k: usize| f32::from_le_bytes(d[k..k + 4].try_into().unwrap_or([0; 4]));
        let blocks = (d[4] != 0).then(|| {
            let n = word(57).map_or(0, u32::from_le_bytes) as usize;
            d.get(61..)
                .unwrap_or(&[])
                .chunks_exact(16)
                .take(n.min(MAX_SENT_BLOCKS))
                .map(|r| {
                    let i = |j: usize| i32::from_le_bytes([r[j], r[j + 1], r[j + 2], r[j + 3]]);
                    ([i(0), i(4), i(8)], i(12) as u32)
                })
                .collect()
        });
        Ok(Pose {
            id,
            at: [float(5), float(13), float(21)],
            velocity: [single(29), single(33), single(37)],
            angle: single(41),
            rate: single(45),
            pitch: single(49),
            base: single(53),
            blocks,
        })
    }
}

/// What clients last heard of a contraption's motion, and when (game ticks).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sent {
    pub at: [f64; 3],
    pub velocity: [f32; 3],
    pub angle: f32,
    pub rate: f32,
    pub pitch: f32,
    pub tick: i64,
}

/// How far a client's glide may be off before it is told again: a 32nd of a block, half a
/// degree.
pub const DRIFT: f64 = 1.0 / 32.0;
pub const TURN_DRIFT: f32 = 0.5;

impl Sent {
    /// Whether clients, gliding on from what they heard, are off the contraption's motion now
    /// (its pitch is drawn as heard, to half a degree).
    pub fn due(&self, at: [f64; 3], angle: f32, pitch: f32, tick: i64) -> bool {
        let t = (tick - self.tick) as f64 / 20.0;
        let off = (0..3)
            .map(|k| {
                let p = self.at[k] + f64::from(self.velocity[k]) * t;
                (p - at[k]).powi(2)
            })
            .sum::<f64>()
            .sqrt();
        let turned = self.angle + self.rate * t as f32;
        off > DRIFT
            || shortest_angle_diff(f64::from(turned), f64::from(angle)).abs() > TURN_DRIFT
            || (pitch - self.pitch).abs() > TURN_DRIFT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_counts_as_directions_do() {
        assert_eq!(yaw_from_vector([1.0, 0.0, 0.0]), 270.0);
        assert!((yaw_from_vector([0.0, 0.0, 1.0]) - 360.0).abs() < 1e-4);
        assert_eq!(yaw_from_vector([-1.0, 0.0, 0.0]), 450.0);
        assert_eq!(y_rot(Dir::East), 270.0);
        assert_eq!(shortest_angle_diff(350.0, 10.0), 20.0);
        assert_eq!(shortest_angle_diff(10.0, 350.0), -20.0);
        // A half turn goes the way the hint says.
        assert_eq!(shortest_angle_diff_hint(0.0, 180.0, 5.0), 180.0);
        assert_eq!(shortest_angle_diff_hint(0.0, 180.0, -5.0), -180.0);
        assert_eq!(Mode::Rotate.scrolled(5), Mode::RotationLocked);
        assert_eq!(Mode::read("ROTATE_PAUSED"), Mode::RotatePaused);
    }

    #[test]
    fn it_turns_toward_the_cart_s_motion_at_create_s_rate() {
        // Assembled facing west (90), the cart going east on an east-west rail: 12° a tick for a
        // small contraption, the half turn the way it moved.
        let mut o = Orientation::at(90.0);
        let blocks = vec![([0, 0, 0], 1), ([0, 1, 0], 2)];
        let size = x_size(&blocks);
        assert_eq!(size, 3.0);
        let rail = Some(RailShape::EastWest);
        assert!(orient(
            &mut o,
            Mode::Rotate,
            [0.4, 0.0, 0.0],
            [0.4, 0.0, 0.0],
            rail,
            size
        ));
        assert_eq!(o.target_yaw, 270.0);
        assert!(
            (o.yaw - 102.0).abs() < 1e-4 || (o.yaw - 78.0).abs() < 1e-4,
            "{o:?}"
        );
        let mut ticks = 1;
        while orient(
            &mut o,
            Mode::Rotate,
            [0.4, 0.0, 0.0],
            [0.4, 0.0, 0.0],
            rail,
            size,
        ) {
            ticks += 1;
        }
        assert_eq!(o.yaw, 270.0);
        assert_eq!(ticks + 1, 15, "180° at 12° a tick: the 15th ends it");
        // A wide contraption turns slower; a locked one not at all.
        assert_eq!(x_size(&[([5, 0, 0], 1)]), 13.0);
        let mut locked = Orientation::at(90.0);
        assert!(!orient(
            &mut locked,
            Mode::RotationLocked,
            [0.4, 0.0, 0.0],
            [0.4, 0.0, 0.0],
            rail,
            3.0
        ));
        assert_eq!(locked.yaw, 90.0);
        // A still cart keeps the way it faces.
        let mut still = Orientation::at(90.0);
        assert!(!orient(
            &mut still,
            Mode::Rotate,
            [0.0; 3],
            [0.0; 3],
            rail,
            3.0
        ));
        assert_eq!(still.yaw, 90.0);
    }

    #[test]
    fn blocks_go_back_turned_to_the_nearest_quarter() {
        let blocks = vec![([0, 0, 0], 9), ([1, 1, 0], 5)];
        // Facing as assembled: where they were, the anchor left out.
        assert_eq!(placed_back(&blocks, 90.0, 90.0), (0, vec![([1, 1, 0], 5)]));
        // Assembled facing west, now east: a half turn.
        assert_eq!(
            placed_back(&blocks, 90.0, 270.0),
            (180, vec![([-1, 1, 0], 5)])
        );
        // 80° off rounds to a quarter: `-yaw + initial` = 90 (counterclockwise looking down).
        let (q, b) = placed_back(&blocks, 90.0, 10.0);
        assert_eq!(q, 90);
        assert_eq!(b, vec![([0, 1, -1], 5)]);
        assert_eq!(
            placed_cart([1, 2, 3], Some(RailShape::AscendingEast)),
            [1.5, 2.5625, 3.5]
        );
    }

    #[test]
    fn poses_round_trip_and_glide_until_they_drift() {
        let p = Pose {
            id: 7,
            at: [1.5, 64.5625, -3.25],
            velocity: [8.0, 0.0, 0.0],
            angle: -180.0,
            rate: 240.0,
            pitch: -45.0,
            base: 90.0,
            blocks: Some(vec![([0, 1, 0], 42)]),
        };
        assert_eq!(Pose::decode(&p.encode()), Ok(p.clone()));
        let still = Pose { blocks: None, ..p };
        assert_eq!(Pose::decode(&still.encode()), Ok(still));
        assert_eq!(Pose::decode(&7u32.to_le_bytes()), Err(Some(7)));
        let s = Sent {
            at: [0.0; 3],
            velocity: [8.0, 0.0, 0.0],
            angle: 0.0,
            rate: 0.0,
            pitch: 0.0,
            tick: 100,
        };
        // Two ticks on at 0.4 a tick: on the glide.
        assert!(!s.due([0.8, 0.0, 0.0], 0.0, 0.0, 102));
        // Slowed down: off by more than a 32nd.
        assert!(s.due([0.7, 0.0, 0.0], 0.0, 0.0, 102));
        assert!(s.due([0.8, 0.0, 0.0], 2.0, 0.0, 102));
        // Onto a slope: the pitch changed.
        assert!(s.due([0.8, 0.0, 0.0], 0.0, 45.0, 102));
        let m = Mounted::new(vec![], Mode::Rotate, Dir::West);
        assert_eq!(m.angle(), 0.0);
        let placed = Mounted::placed_at(vec![], Mode::Rotate, Dir::West, 270.0);
        assert_eq!(placed.angle(), -180.0);
    }
}
