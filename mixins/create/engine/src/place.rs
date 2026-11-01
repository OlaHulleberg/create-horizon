//! Create's placement rules for its directional blocks (`getStateForPlacement` of
//! `DirectionalKineticBlock`, `HorizontalKineticBlock` and the blocks that change them), and
//! vanilla's `Direction.orderedByNearest` they read the player's look with.

use crate::Block;
use crate::kinetics::{Axis, Dir};

/// What a placement knows of the player: their view and whether they sneak, and the face of the
/// block they clicked (`BlockPlaceContext`).
#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub yaw: f32,
    pub pitch: f32,
    pub sneaking: bool,
    pub clicked_face: Dir,
}

/// `Direction.orderedByNearest`: the six directions, nearest to the player's view first.
pub fn ordered_by_nearest(yaw: f32, pitch: f32) -> [Dir; 6] {
    let f = pitch * (std::f32::consts::PI / 180.0);
    let g = -yaw * (std::f32::consts::PI / 180.0);
    let (h, i, j, k) = (mth_sin(f), mth_cos(f), mth_sin(g), mth_cos(g));
    let (east, up, south) = (j > 0.0, h < 0.0, k > 0.0);
    let l = if east { j } else { -j };
    let m = if up { -h } else { h };
    let n = if south { k } else { -k };
    let (o, p) = (l * i, n * i);
    let a = if east { Dir::East } else { Dir::West };
    let b = if up { Dir::Up } else { Dir::Down };
    let c = if south { Dir::South } else { Dir::North };
    let make = |x: Dir, y: Dir, z: Dir| [x, y, z, z.opposite(), y.opposite(), x.opposite()];
    if l > n {
        if m > o {
            make(b, a, c)
        } else if p > m {
            make(a, c, b)
        } else {
            make(a, b, c)
        }
    } else if m > p {
        make(b, c, a)
    } else if o > m {
        make(c, a, b)
    } else {
        make(c, b, a)
    }
}

/// `Mth.sin` and `Mth.cos` of 26.3: a 65,536-step table indexed from a double, so exact angles
/// tie as in vanilla (cos 90° is a little above 0).
const SCALE: f64 = 10430.378350470453;
fn mth_sin(x: f32) -> f32 {
    table((f64::from(x) * SCALE) as i64)
}
fn mth_cos(x: f32) -> f32 {
    table((f64::from(x) * SCALE + 16384.0) as i64)
}
fn table(i: i64) -> f32 {
    ((i & 65535) as f64 / SCALE).sin() as f32
}

/// `Entity.getDirection` (`Direction.fromYRot`): the horizontal direction the player faces.
pub fn horizontal_direction(yaw: f32) -> Dir {
    match ((yaw / 90.0 + 0.5).floor() as i32) & 3 {
        0 => Dir::South,
        1 => Dir::West,
        2 => Dir::North,
        _ => Dir::East,
    }
}

/// `getPreferredFacing` (all six sides) and `getPreferredHorizontalFacing`: the side whose
/// neighbour has a shaft toward the block, none when neighbours on two axes have one.
/// `shaft(side)` says whether the neighbour at `side` has a shaft facing back.
pub fn preferred(horizontal: bool, shaft: impl Fn(Dir) -> bool) -> Option<Dir> {
    let mut preferred: Option<Dir> = None;
    for side in Dir::ALL {
        if horizontal && side.axis() == crate::kinetics::Axis::Y {
            continue;
        }
        if shaft(side) {
            if preferred.is_some_and(|p| p.axis() != side.axis()) {
                return None;
            }
            preferred = Some(side);
        }
    }
    preferred
}

/// The facing Create gives `block` placed in `ctx` next to shafts `shaft` would find, or None
/// when its placement isn't Create's own.
pub fn facing(block: Block, ctx: Context, shaft: impl Fn(Dir) -> bool) -> Option<Dir> {
    let nearest = ordered_by_nearest(ctx.yaw, ctx.pitch)[0];
    // DirectionalKineticBlock.getStateForPlacement.
    let directional = |preferred: Option<Dir>| match preferred {
        Some(p) if !ctx.sneaking => p.opposite(),
        _ if ctx.sneaking => nearest,
        _ => nearest.opposite(),
    };
    Some(match block {
        Block::WindmillBearing | Block::WaterWheel => directional(preferred(false, shaft)),
        // CreativeMotorBlock: toward the shaft it found, not away.
        Block::CreativeMotor => match preferred(false, shaft) {
            Some(p) if !ctx.sneaking => p,
            p => directional(p),
        },
        // EncasedFanBlock: away from the shaft or the player; sneaking, toward them.
        Block::EncasedFan => {
            let p = preferred(false, shaft).unwrap_or(nearest);
            if ctx.sneaking { p } else { p.opposite() }
        }
        // HandCrankBlock: on the clicked face, or away from the shaft it found.
        Block::HandCrank => match preferred(false, shaft) {
            Some(p) if !ctx.sneaking => p.opposite(),
            _ => ctx.clicked_face,
        },
        // MechanicalPressBlock: along a shaft beside it, else facing the player.
        Block::MechanicalPress => {
            preferred(true, shaft).unwrap_or_else(|| horizontal_direction(ctx.yaw).opposite())
        }
        // RedstoneLinkBlock: on the clicked face.
        Block::RedstoneLink => ctx.clicked_face,
        // SailBlock (WrenchableDirectionalBlock): away from the player's view.
        b if is_sail(b) => nearest.opposite(),
        _ => return None,
    })
}

/// The axis Create gives `block` placed in `ctx`, and whether it stands `vertical` (a block with
/// a horizontal axis), or None when its placement isn't Create's own. `shaft` as for `facing`;
/// `cog_above` is the axis of a large cog right above it.
pub fn axis(
    block: Block,
    ctx: Context,
    shaft: impl Fn(Dir) -> bool,
    cog_above: Option<Axis>,
) -> Option<(Axis, bool)> {
    let across = |a: Axis| if a == Axis::X { Axis::Z } else { Axis::X };
    let facing = horizontal_direction(ctx.yaw).axis();
    match block {
        // SpeedControllerBlock: across the large cog above it; else
        // HorizontalAxisKineticBlock's: the axis of the shafts beside it, else across the
        // player's facing (`getClockWise`).
        Block::RotationSpeedController => {
            let axis = match cog_above {
                Some(a) if a != Axis::Y => across(a),
                _ => preferred(true, &shaft).map_or(across(facing), |d| d.axis()),
            };
            Some((axis, false))
        }
        // ChainDriveBlock: the view's axis when sneaking, else the axis of the shafts around it
        // (`getPreferredAxis`), else the view's.
        Block::EncasedChainDrive | Block::AdjustableChainGearshift => {
            let view = ordered_by_nearest(ctx.yaw, ctx.pitch)[0].axis();
            let axis = match preferred(false, &shaft) {
                Some(p) if !ctx.sneaking => p.axis(),
                _ => view,
            };
            Some((axis, false))
        }
        // SequencedGearshiftBlock: the axis of the shafts around it unless sneaking
        // (`getPreferredAxis`), else the view's; standing on y, its horizontal axis is the
        // player's facing (`withAxis`).
        Block::SequencedGearshift => {
            let axis = match preferred(false, &shaft) {
                Some(p) if !ctx.sneaking => p.axis(),
                _ => ordered_by_nearest(ctx.yaw, ctx.pitch)[0].axis(),
            };
            Some(if axis == Axis::Y {
                (facing, true)
            } else {
                (axis, false)
            })
        }
        _ => None,
    }
}

fn is_sail(block: Block) -> bool {
    block == Block::SailFrame || block.name().ends_with("_sail")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(yaw: f32, pitch: f32, sneaking: bool) -> Context {
        Context {
            yaw,
            pitch,
            sneaking,
            clicked_face: Dir::Up,
        }
    }

    #[test]
    fn the_view_orders_directions() {
        // Yaw 0 looks south, 90 west, 180 north; pitch 90 looks down.
        assert_eq!(ordered_by_nearest(0.0, 0.0)[0], Dir::South);
        assert_eq!(ordered_by_nearest(90.0, 10.0)[0], Dir::West);
        assert_eq!(ordered_by_nearest(180.0, -80.0)[0], Dir::Up);
        assert_eq!(
            ordered_by_nearest(-90.0, 60.0),
            [
                Dir::Down,
                Dir::East,
                Dir::South,
                Dir::North,
                Dir::West,
                Dir::Up
            ]
        );
        assert_eq!(horizontal_direction(0.0), Dir::South);
        assert_eq!(horizontal_direction(-100.0), Dir::East);
    }

    #[test]
    fn blocks_face_as_create_places_them() {
        let none = |_: Dir| false;
        // A player looking north places a fan blowing at them (south); sneaking, away (north).
        assert_eq!(
            facing(Block::EncasedFan, ctx(180.0, 0.0, false), none),
            Some(Dir::South)
        );
        assert_eq!(
            facing(Block::EncasedFan, ctx(180.0, 0.0, true), none),
            Some(Dir::North)
        );
        // A shaft to the west: the fan's back joins it, the motor points at it.
        let west = |d: Dir| d == Dir::West;
        assert_eq!(
            facing(Block::EncasedFan, ctx(0.0, 0.0, false), west),
            Some(Dir::East)
        );
        assert_eq!(
            facing(Block::CreativeMotor, ctx(0.0, 0.0, false), west),
            Some(Dir::West)
        );
        assert_eq!(
            facing(Block::WindmillBearing, ctx(0.0, 0.0, false), west),
            Some(Dir::East)
        );
        // Shafts on two axes: no preference, the view decides.
        let two = |d: Dir| d == Dir::West || d == Dir::Up;
        assert_eq!(
            facing(Block::WindmillBearing, ctx(0.0, 0.0, false), two),
            Some(Dir::North)
        );
        // A press faces the player unless a shaft beside it sets its axis.
        assert_eq!(
            facing(Block::MechanicalPress, ctx(90.0, 0.0, false), none),
            Some(Dir::East)
        );
        assert_eq!(
            facing(Block::MechanicalPress, ctx(90.0, 0.0, false), |d| d
                == Dir::North),
            Some(Dir::North)
        );
        // A crank and a link go on the clicked face; a sail faces away from the view.
        assert_eq!(
            facing(Block::HandCrank, ctx(0.0, 0.0, false), none),
            Some(Dir::Up)
        );
        assert_eq!(
            facing(Block::RedstoneLink, ctx(0.0, 0.0, true), |_| true),
            Some(Dir::Up)
        );
        assert_eq!(
            facing(Block::WhiteSail, ctx(0.0, 0.0, false), none),
            Some(Dir::North)
        );
        assert_eq!(facing(Block::Shaft, ctx(0.0, 0.0, false), none), None);
    }

    #[test]
    fn axis_blocks_lie_as_create_places_them() {
        let none = |_: Dir| false;
        let b = Block::RotationSpeedController;
        // Looking south: across the view, on x; a large cog above on x: on z.
        assert_eq!(
            axis(b, ctx(0.0, 0.0, false), none, None),
            Some((Axis::X, false))
        );
        assert_eq!(
            axis(b, ctx(0.0, 0.0, false), none, Some(Axis::X)),
            Some((Axis::Z, false))
        );
        // A shaft to the north: along it.
        assert_eq!(
            axis(b, ctx(0.0, 0.0, false), |d| d == Dir::North, None),
            Some((Axis::Z, false))
        );
        let s = Block::SequencedGearshift;
        // Looking down while facing west: vertical, its horizontal axis x.
        assert_eq!(
            axis(s, ctx(90.0, 80.0, false), none, None),
            Some((Axis::X, true))
        );
        // A shaft below: vertical; sneaking, the view's axis.
        let below = |d: Dir| d == Dir::Down;
        assert_eq!(
            axis(s, ctx(0.0, 0.0, false), below, None),
            Some((Axis::Z, true))
        );
        assert_eq!(
            axis(s, ctx(0.0, 0.0, true), below, None),
            Some((Axis::Z, false))
        );
        assert_eq!(axis(Block::Shaft, ctx(0.0, 0.0, false), none, None), None);
    }
}
