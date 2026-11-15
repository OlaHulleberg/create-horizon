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
pub fn mth_sin(x: f32) -> f32 {
    table((f64::from(x) * SCALE) as i64)
}
pub(crate) fn mth_cos(x: f32) -> f32 {
    table((f64::from(x) * SCALE + 16384.0) as i64)
}
fn table(i: i64) -> f32 {
    ((i & 65535) as f64 / SCALE).sin() as f32
}

/// `BlockPlaceContext.getNearestLookingDirections`: `ordered_by_nearest`, but for a placement
/// against a block (`replace_clicked` false) the clicked face's opposite comes first.
pub fn nearest_looking_directions(ctx: Context, replace_clicked: bool) -> [Dir; 6] {
    let mut d = ordered_by_nearest(ctx.yaw, ctx.pitch);
    if replace_clicked {
        return d;
    }
    let toward = ctx.clicked_face.opposite();
    if let Some(i) = d.iter().position(|x| *x == toward) {
        d[..=i].rotate_right(1);
    }
    d
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
        Block::WindmillBearing
        | Block::WaterWheel
        | Block::MechanicalBearing
        | Block::ClockworkBearing
        | Block::MechanicalDrill => directional(preferred(false, shaft)),
        // DirectionalAxisKineticBlock.getFacingForPlacement (deployer, saw): away from the
        // player's view; sneaking, toward it.
        Block::Deployer | Block::MechanicalSaw if ctx.sneaking => nearest,
        Block::Deployer | Block::MechanicalSaw => nearest.opposite(),
        // HorizontalKineticBlock (the mechanical crafter, when not placed against another):
        // facing the player.
        Block::MechanicalCrafter => horizontal_direction(ctx.yaw).opposite(),
        // BasinBlock has no placement of its own: its default state, outputting down.
        Block::Basin => Dir::Down,
        // HorizontalKineticBlock (the elevator pulley): facing the player.
        Block::ElevatorPulley => horizontal_direction(ctx.yaw).opposite(),
        // WrenchableDirectionalBlock (the elevator contact): toward the player's view.
        Block::ElevatorContact => nearest,
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
        // HandCrankBlock (and ValveHandleBlock): on the clicked face, or away from the shaft it
        // found.
        b if b == Block::HandCrank || crate::dye::is_valve_handle(b) => {
            match preferred(false, shaft) {
                Some(p) if !ctx.sneaking => p.opposite(),
                _ => ctx.clicked_face,
            }
        }
        // ToolboxBlock and PostboxBlock: facing the player
        // (`getHorizontalDirection().getOpposite()`).
        b if crate::dye::is_toolbox(b) || crate::dye::is_postbox(b) => {
            horizontal_direction(ctx.yaw).opposite()
        }
        // MechanicalPressBlock: along a shaft beside it, else facing the player.
        Block::MechanicalPress => {
            preferred(true, shaft).unwrap_or_else(|| horizontal_direction(ctx.yaw).opposite())
        }
        // CuckooClockBlock.getStateForPlacement: its back to a shaft beside it, else facing
        // the player.
        Block::CuckooClock | Block::MysteriousCuckooClock => preferred(true, shaft)
            .map_or_else(|| horizontal_direction(ctx.yaw).opposite(), Dir::opposite),
        // StockTickerBlock: facing the player; sneaking, away.
        Block::StockTicker if ctx.sneaking => horizontal_direction(ctx.yaw),
        Block::StockTicker => horizontal_direction(ctx.yaw).opposite(),
        // BacktankBlock (HorizontalKineticBlock): facing the player.
        Block::CopperBacktank | Block::NetheriteBacktank => {
            horizontal_direction(ctx.yaw).opposite()
        }
        // RedstoneLinkBlock: on the clicked face.
        Block::RedstoneLink => ctx.clicked_face,
        // SailBlock (WrenchableDirectionalBlock): away from the player's view.
        b if is_sail(b) => nearest.opposite(),
        // StickerBlock: away from the player's view; sneaking, toward it.
        Block::Sticker if ctx.sneaking => nearest,
        Block::Sticker => nearest.opposite(),
        // RollerBlock.getStateForPlacement: facing the player.
        Block::MechanicalRoller => horizontal_direction(ctx.yaw).opposite(),
        // ControlsBlock (contraption controls): facing the player; sneaking, away.
        Block::ContraptionControls if ctx.sneaking => horizontal_direction(ctx.yaw),
        Block::ContraptionControls => horizontal_direction(ctx.yaw).opposite(),
        // PortableStorageInterfaceBlock: against the player's view; sneaking, along it.
        Block::PortableStorageInterface | Block::PortableFluidInterface if ctx.sneaking => nearest,
        Block::PortableStorageInterface | Block::PortableFluidInterface => nearest.opposite(),
        // AttachedActorBlock (harvester, plough): on a wall's clicked face, else facing the
        // player.
        Block::MechanicalHarvester | Block::MechanicalPlough => {
            if ctx.clicked_face.axis() == crate::kinetics::Axis::Y {
                horizontal_direction(ctx.yaw).opposite()
            } else {
                ctx.clicked_face
            }
        }
        _ => return None,
    })
}

/// What a placement sees around the block besides shafts: the axis of a large cog right above
/// it, of a small cog it was placed against (behind the clicked face), and of a rotation speed
/// controller right below it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Nearby {
    pub large_cog_above: Option<Axis>,
    pub small_cog_clicked: Option<Axis>,
    pub controller_below: Option<Axis>,
}

/// `DirectionalAxisKineticBlock.getStateForPlacement`'s `AXIS_ALONG_FIRST_COORDINATE` for a block
/// facing `facing`: sideways, its shaft across the facing on the ground (along x when facing
/// along z), turned up when a shaft is above or below; facing up or down, across the player's
/// facing, or along the shafts beside it on one axis.
pub fn along_first(facing: Dir, ctx: Context, shaft: impl Fn(Dir) -> bool) -> bool {
    let axis = facing.axis();
    if axis != Axis::Y {
        if shaft(Dir::Up) || shaft(Dir::Down) {
            return axis == Axis::X;
        }
        return axis == Axis::Z;
    }
    match preferred(true, shaft) {
        Some(side) => side.axis() == Axis::X,
        None => horizontal_direction(ctx.yaw).axis() == Axis::X,
    }
}

/// `SawBlock.getStateForPlacement`'s `FLIPPED`: facing up or down, placed by a player facing a
/// positive direction (east or south).
pub fn saw_flipped(facing: Dir, ctx: Context) -> bool {
    facing.axis() == Axis::Y && horizontal_direction(ctx.yaw).positive()
}

/// `MechanicalCrafterBlock.getStateForPlacement`: against another crafter (not sneaking) its
/// facing, else facing the player; and its pointing toward the face clicked
/// (`pointingFromFacing`), or up when the clicked face is its own facing. `against`: the facing
/// of the crafter clicked on.
pub fn crafter(ctx: Context, against: Option<Dir>) -> (Dir, &'static str) {
    let face = ctx.clicked_face;
    let pointing = |facing: Dir| {
        let positive = facing.positive();
        match face {
            Dir::Down => "up",
            Dir::East | Dir::North if positive => "left",
            Dir::East | Dir::North => "right",
            Dir::West | Dir::South if positive => "right",
            Dir::West | Dir::South => "left",
            Dir::Up => "down",
        }
    };
    match against {
        Some(facing) if !ctx.sneaking => (facing, pointing(facing)),
        _ => {
            let facing = horizontal_direction(ctx.yaw).opposite();
            (
                facing,
                if facing == face {
                    "up"
                } else {
                    pointing(facing)
                },
            )
        }
    }
}

/// The axis Create gives `block` placed in `ctx`, and whether it stands `vertical` (a block with
/// a horizontal axis), or None when its placement isn't Create's own. `shaft` as for `facing`.
pub fn axis(
    block: Block,
    ctx: Context,
    shaft: impl Fn(Dir) -> bool,
    near: Nearby,
) -> Option<(Axis, bool)> {
    let across = |a: Axis| if a == Axis::X { Axis::Z } else { Axis::X };
    let facing = horizontal_direction(ctx.yaw).axis();
    let cog_above = near.large_cog_above;
    let nearest = ordered_by_nearest(ctx.yaw, ctx.pitch)[0].axis();
    // RotatedPillarKineticBlock.getStateForPlacement: the axis of the shafts around it unless
    // sneaking; sneaking beside one, the clicked face's; else the view's.
    let pillar = || match preferred(false, &shaft) {
        Some(p) if !ctx.sneaking => p.axis(),
        Some(_) => ctx.clicked_face.axis(),
        None => nearest,
    };
    match block {
        // ShaftBlock, FlywheelBlock, CrushingWheelBlock, LargeWaterWheelBlock (whose room the
        // glue checks after).
        Block::Shaft | Block::Flywheel | Block::CrushingWheel | Block::LargeWaterWheel => {
            Some((pillar(), false))
        }
        // AbstractEncasedShaftBlock (encased shafts, clutch, gearshift): sneaking, the pillar's
        // rule; else the shafts' axis or the view's.
        Block::AndesiteEncasedShaft
        | Block::BrassEncasedShaft
        | Block::Clutch
        | Block::Gearshift => Some((
            if ctx.sneaking {
                pillar()
            } else {
                preferred(false, &shaft).map_or(nearest, |d| d.axis())
            },
            false,
        )),
        // GearboxBlock: always upright; the vertical gearbox is its own item.
        Block::Gearbox => Some((Axis::Y, false)),
        // EncasedCogwheelBlock: the pillar's rule, or the axis of a small cog it was placed on.
        Block::AndesiteEncasedCogwheel
        | Block::BrassEncasedCogwheel
        | Block::AndesiteEncasedLargeCogwheel
        | Block::BrassEncasedLargeCogwheel => {
            Some((near.small_cog_clicked.unwrap_or_else(pillar), false))
        }
        // CogWheelBlock.getAxisForPlacement: sneaking, the clicked face's; a large cog on a
        // speed controller, across it; on a small cog, its axis; else the shafts' or the clicked
        // face's.
        Block::Cogwheel | Block::LargeCogwheel => {
            let axis = if ctx.sneaking {
                ctx.clicked_face.axis()
            } else if let (Some(a), true) = (near.controller_below, block == Block::LargeCogwheel) {
                across(a)
            } else if let Some(a) = near.small_cog_clicked {
                a
            } else {
                preferred(false, &shaft).map_or(ctx.clicked_face.axis(), |d| d.axis())
            };
            Some((axis, false))
        }
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
        // HorizontalAxisKineticBlock (the rope pulley): the axis of the shafts beside it, else
        // across the player's facing.
        Block::RopePulley => Some((
            preferred(true, &shaft).map_or(across(facing), |d| d.axis()),
            false,
        )),
        // ChainDriveBlock: the view's axis when sneaking, else the axis of the shafts around it
        // (`getPreferredAxis`), else the view's.
        Block::EncasedChainDrive | Block::AdjustableChainGearshift => {
            let axis = match preferred(false, &shaft) {
                Some(p) if !ctx.sneaking => p.axis(),
                _ => nearest,
            };
            Some((axis, false))
        }
        // SequencedGearshiftBlock: the axis of the shafts around it unless sneaking
        // (`getPreferredAxis`), else the view's; standing on y, its horizontal axis is the
        // player's facing (`withAxis`).
        Block::SequencedGearshift => {
            let axis = match preferred(false, &shaft) {
                Some(p) if !ctx.sneaking => p.axis(),
                _ => nearest,
            };
            Some(if axis == Axis::Y {
                (facing, true)
            } else {
                (axis, false)
            })
        }
        // RedstoneRequesterBlock: the axis of the player's horizontal facing.
        Block::RedstoneRequester => Some((facing, false)),
        _ => None,
    }
}

/// `RedstoneContactBlock.getStateForPlacement`: away from the player's view, or onto the
/// clicked face when sneaking or when a contact there faces back; powered when the contact it
/// faces faces back (`valid`: `hasValidContact` toward a side).
pub fn redstone_contact(ctx: Context, valid: impl Fn(Dir) -> bool) -> (Dir, bool) {
    let mut facing = ordered_by_nearest(ctx.yaw, ctx.pitch)[0].opposite();
    let place = ctx.clicked_face.opposite();
    if ctx.sneaking || valid(place) {
        facing = place;
    }
    (facing, valid(facing))
}

/// `DoubleFaceAttachedBlock.DoubleAttachFace`: where a nixie tube hangs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoubleFace {
    Floor,
    Wall,
    WallReversed,
    Ceiling,
}

impl DoubleFace {
    pub fn name(self) -> &'static str {
        match self {
            DoubleFace::Floor => "floor",
            DoubleFace::Wall => "wall",
            DoubleFace::WallReversed => "wall_reversed",
            DoubleFace::Ceiling => "ceiling",
        }
    }
}

/// `Direction.getClockWise` of a horizontal direction.
fn clockwise(d: Dir) -> Dir {
    match d {
        Dir::North => Dir::East,
        Dir::East => Dir::South,
        Dir::South => Dir::West,
        Dir::West => Dir::North,
        other => other,
    }
}

/// `Entity.calculateViewVector`: the player's look as a vector (`getLookAngle`).
pub fn look_angle(yaw: f32, pitch: f32) -> [f64; 3] {
    let f = pitch * (std::f32::consts::PI / 180.0);
    let g = -yaw * (std::f32::consts::PI / 180.0);
    let (h, i, j, k) = (mth_cos(g), mth_sin(g), mth_cos(f), mth_sin(f));
    [f64::from(i * j), f64::from(-k), f64::from(h * j)]
}

/// `NixieTubeBlock.getStateForPlacement` over `DoubleFaceAttachedBlock`'s: the first of the
/// nearest looking directions (`BlockPlaceContext.getNearestLookingDirections` puts the clicked
/// face's opposite first; a nixie tube survives anywhere) gives its face. Up or down: on the
/// ceiling or the floor, facing the player's horizontal direction turned clockwise; else on the
/// wall it was clicked on, reversed when the look points against the wall's clockwise side.
pub fn nixie(ctx: Context) -> (DoubleFace, Dir) {
    let direction = ctx.clicked_face.opposite();
    match direction {
        Dir::Up | Dir::Down => {
            let face = if direction == Dir::Up {
                DoubleFace::Ceiling
            } else {
                DoubleFace::Floor
            };
            (face, clockwise(horizontal_direction(ctx.yaw)))
        }
        d => {
            let n = clockwise(d).offset();
            let look = look_angle(ctx.yaw, ctx.pitch);
            let dot =
                look[0] * f64::from(n[0]) + look[1] * f64::from(n[1]) + look[2] * f64::from(n[2]);
            let face = if dot < 0.0 {
                DoubleFace::WallReversed
            } else {
                DoubleFace::Wall
            };
            (face, d.opposite())
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Placement helpers: a shaft or cog item clicked on a shaft or cog places next to it

/// `IPlacementHelper.orderedByDistance`: the directions `keep` lets through, the one whose
/// face lies nearest the hit first (`hit` relative to the block's centre); ties keep
/// `Direction` order.
pub fn ordered_by_distance(hit: [f64; 3], keep: impl Fn(Dir) -> bool) -> Vec<Dir> {
    let mut dirs: Vec<(Dir, f64)> = Dir::ALL
        .into_iter()
        .filter(|d| keep(*d))
        .map(|d| {
            let o = d.offset();
            let dist = (0..3)
                .map(|i| (f64::from(o[i]) - hit[i]).powi(2))
                .sum::<f64>()
                .sqrt();
            (d, dist)
        })
        .collect();
    dirs.sort_by(|a, b| a.1.total_cmp(&b.1));
    dirs.into_iter().map(|d| d.0).collect()
}

fn add(p: [i32; 3], d: Dir) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

/// `AllConfigs.server().equipment.placementAssistRange`: how far a shaft line is extended.
pub const PLACEMENT_ASSIST_RANGE: usize = 12;

/// `ShaftBlock.PlacementHelper` (a `PoleHelper`): a shaft or cog item on a shaft along `axis`
/// at `pos` goes at the free end of the shafts in line with it, the end nearest the hit first,
/// with the same axis. `pole(p)` says whether `p` holds a shaft along `axis`, `free(p)` whether
/// it can be replaced.
pub fn pole_helper(
    pos: [i32; 3],
    axis: Axis,
    hit: [f64; 3],
    pole: impl Fn([i32; 3]) -> bool,
    free: impl Fn([i32; 3]) -> bool,
) -> Option<[i32; 3]> {
    for dir in ordered_by_distance(hit, |d| d.axis() == axis) {
        let mut at = add(pos, dir);
        let mut poles = 0;
        while pole(at) {
            poles += 1;
            at = add(at, dir);
        }
        if poles >= PLACEMENT_ASSIST_RANGE {
            continue;
        }
        if free(at) {
            return Some(at);
        }
    }
    None
}

/// `DiagonalCogHelper.hitOnShaft`: the hit (relative to the block's corner) lies in the
/// six-voxel pole along `axis`.
fn hit_on_shaft(local: [f64; 3], axis: Axis) -> bool {
    (0..3).all(|i| {
        let along = [Axis::X, Axis::Y, Axis::Z][i] == axis;
        let (lo, hi) = if along {
            (0.0, 1.0)
        } else {
            (5.0 / 16.0, 11.0 / 16.0)
        };
        local[i] >= lo - 0.001 && local[i] <= hi + 0.001
    })
}

/// The cog a cog helper clicked: whether it is large, and its axis.
#[derive(Clone, Copy, Debug)]
pub struct ClickedCog {
    pub large: bool,
    pub axis: Axis,
}

/// `CogwheelBlockItem.onItemUseFirst` with `SmallCogHelper` / `LargeCogHelper`: a cog item
/// (`large` or small) clicked, not sneaking, on the cog `cog` at `pos`, the hit `local` to its
/// corner. Where the new cog goes and its axis, or None to place as usual. `valid(p, large,
/// axis)` is `isValidCogwheelPosition`, `free(p)` whether `p` can be replaced.
pub fn cog_helper(
    large: bool,
    pos: [i32; 3],
    cog: ClickedCog,
    local: [f64; 3],
    valid: impl Fn([i32; 3], bool, Axis) -> bool,
    free: impl Fn([i32; 3]) -> bool,
) -> Option<([i32; 3], Axis)> {
    if hit_on_shaft(local, cog.axis) {
        return None;
    }
    let hit = local.map(|v| v - 0.5);
    let axis = cog.axis;
    let except = |d: Dir| d.axis() != axis;
    match (large, cog.large) {
        // SmallCogHelper on a small cog: beside it in its plane, the side nearest the hit.
        (false, false) => ordered_by_distance(hit, except).into_iter().find_map(|d| {
            let at = add(pos, d);
            (valid(at, false, axis) && free(at)).then_some((at, axis))
        }),
        // LargeCogHelper on a large cog: diagonally off its side nearest the hit, at right
        // angles to it.
        (true, true) => {
            let side = ordered_by_distance(hit, |d| d.axis() == axis)[0];
            ordered_by_distance(hit, except).into_iter().find_map(|d| {
                let at = add(add(pos, d), side);
                (valid(at, true, d.axis()) && free(at)).then_some((at, d.axis()))
            })
        }
        // DiagonalCogHelper: a cog of the other size diagonally in the same plane.
        _ => {
            let closest = ordered_by_distance(hit, except)[0];
            ordered_by_distance(hit, |d| except(d) && d.axis() != closest.axis())
                .into_iter()
                .find_map(|d| {
                    let at = add(add(pos, d), closest);
                    (free(at) && valid(at, cog.large, axis)).then_some((at, axis))
                })
        }
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
    fn deployers_saws_and_crafters_turn_as_create_places_them() {
        // Looking north and a little down: a deployer faces back south, its shaft along x.
        let c = ctx(180.0, 30.0, false);
        assert_eq!(facing(Block::Deployer, c, |_| false), Some(Dir::South));
        assert!(along_first(Dir::South, c, |_| false));
        // A shaft below turns it up.
        assert!(!along_first(Dir::South, c, |d| d == Dir::Down));
        // Facing up, across the player's facing; a saw placed facing south is flipped.
        assert!(!along_first(Dir::Up, c, |_| false));
        assert!(saw_flipped(Dir::Up, ctx(0.0, 90.0, false)));
        // A crafter on the floor faces the player and points into the floor; one placed on the
        // east face of another takes its facing and points left.
        assert_eq!(crafter(c, None), (Dir::South, "down"));
        let east = Context {
            clicked_face: Dir::East,
            ..c
        };
        assert_eq!(crafter(east, Some(Dir::South)), (Dir::South, "left"));
        assert_eq!(facing(Block::Basin, c, |_| false), Some(Dir::Down));
    }

    #[test]
    fn a_clicked_face_comes_first_when_looking() {
        // Looking steeply down and east at a block's west face: east (into the clicked block)
        // comes first, the rest keep their order.
        let c = Context {
            yaw: -90.0,
            pitch: 60.0,
            sneaking: false,
            clicked_face: Dir::West,
        };
        assert_eq!(
            nearest_looking_directions(c, false),
            [
                Dir::East,
                Dir::Down,
                Dir::South,
                Dir::North,
                Dir::West,
                Dir::Up
            ]
        );
        assert_eq!(nearest_looking_directions(c, true)[0], Dir::Down);
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
        // A valve handle as a crank; toolboxes and postboxes face the player.
        assert_eq!(
            facing(Block::CopperValveHandle, ctx(0.0, 0.0, false), none),
            Some(Dir::Up)
        );
        assert_eq!(
            facing(Block::RedValveHandle, ctx(0.0, 0.0, false), west),
            Some(Dir::East)
        );
        assert_eq!(
            facing(Block::BlackToolbox, ctx(90.0, 0.0, false), none),
            Some(Dir::East)
        );
        assert_eq!(
            facing(Block::WhitePostbox, ctx(0.0, 30.0, false), none),
            Some(Dir::North)
        );
    }

    #[test]
    fn nixie_tubes_hang_as_create_places_them() {
        let on = |face: Dir, yaw: f32, pitch: f32| {
            nixie(Context {
                yaw,
                pitch,
                sneaking: false,
                clicked_face: face,
            })
        };
        // On a floor, looking south: south turned clockwise, west.
        assert_eq!(on(Dir::Up, 0.0, 45.0), (DoubleFace::Floor, Dir::West));
        // Under a ceiling, looking east (yaw -90): east turned clockwise, south.
        assert_eq!(
            on(Dir::Down, -90.0, -45.0),
            (DoubleFace::Ceiling, Dir::South)
        );
        // On the south face of a block, looking north: facing south. The wall's clockwise
        // side is east (north's clockwise): a look a little east of north (yaw -170) points
        // along it, a little west (yaw 170) against it.
        assert_eq!(on(Dir::South, -170.0, 0.0), (DoubleFace::Wall, Dir::South));
        assert_eq!(
            on(Dir::South, 170.0, 0.0),
            (DoubleFace::WallReversed, Dir::South)
        );
        // The look itself: yaw 0, pitch 0 is south.
        let l = look_angle(0.0, 0.0);
        assert!(l[0].abs() < 1e-6 && l[1].abs() < 1e-6 && (l[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn axis_blocks_lie_as_create_places_them() {
        let none = |_: Dir| false;
        let b = Block::RotationSpeedController;
        // Looking south: across the view, on x; a large cog above on x: on z.
        assert_eq!(
            axis(b, ctx(0.0, 0.0, false), none, Nearby::default()),
            Some((Axis::X, false))
        );
        assert_eq!(
            axis(
                b,
                ctx(0.0, 0.0, false),
                none,
                Nearby {
                    large_cog_above: Some(Axis::X),
                    ..Nearby::default()
                }
            ),
            Some((Axis::Z, false))
        );
        // A shaft to the north: along it.
        assert_eq!(
            axis(
                b,
                ctx(0.0, 0.0, false),
                |d| d == Dir::North,
                Nearby::default()
            ),
            Some((Axis::Z, false))
        );
        let s = Block::SequencedGearshift;
        // Looking down while facing west: vertical, its horizontal axis x.
        assert_eq!(
            axis(s, ctx(90.0, 80.0, false), none, Nearby::default()),
            Some((Axis::X, true))
        );
        // A shaft below: vertical; sneaking, the view's axis.
        let below = |d: Dir| d == Dir::Down;
        assert_eq!(
            axis(s, ctx(0.0, 0.0, false), below, Nearby::default()),
            Some((Axis::Z, true))
        );
        assert_eq!(
            axis(s, ctx(0.0, 0.0, true), below, Nearby::default()),
            Some((Axis::Z, false))
        );
        assert_eq!(
            axis(Block::Belt, ctx(0.0, 0.0, false), none, Nearby::default()),
            None
        );
        // Requesters along the player's facing; tickers facing the player.
        assert_eq!(
            axis(
                Block::RedstoneRequester,
                ctx(90.0, 0.0, false),
                none,
                Nearby::default()
            ),
            Some((Axis::X, false))
        );
        assert_eq!(
            facing(Block::StockTicker, ctx(0.0, 0.0, false), none),
            Some(Dir::North)
        );
        assert_eq!(
            facing(Block::StockTicker, ctx(0.0, 0.0, true), none),
            Some(Dir::South)
        );
    }

    #[test]
    fn helpers_extend_shafts_and_mesh_cogs() {
        // Shafts at x 0..=2 on x; clicked at x 0 near its west end: the line grows west first.
        let pole = |p: [i32; 3]| p[1] == 0 && p[2] == 0 && (0..=2).contains(&p[0]);
        let at = pole_helper([0, 0, 0], Axis::X, [-0.4, 0.0, 0.0], pole, |_| true);
        assert_eq!(at, Some([-1, 0, 0]));
        // Near its east end: past the line's end.
        let at = pole_helper([0, 0, 0], Axis::X, [0.4, 0.0, 0.0], pole, |_| true);
        assert_eq!(at, Some([3, 0, 0]));
        let valid = |_: [i32; 3], _: bool, _: Axis| true;
        let small = ClickedCog {
            large: false,
            axis: Axis::Y,
        };
        // A small cog on a small cog's east rim: beside it to the east, same axis.
        let at = cog_helper(false, [0, 0, 0], small, [0.95, 0.5, 0.5], valid, |_| true);
        assert_eq!(at, Some(([1, 0, 0], Axis::Y)));
        // Clicked on its shaft: as usual.
        let at = cog_helper(false, [0, 0, 0], small, [0.5, 1.0, 0.5], valid, |_| true);
        assert_eq!(at, None);
        // A large cog on a small cog's east rim, a little north: diagonal, same axis.
        let at = cog_helper(true, [0, 0, 0], small, [0.95, 0.5, 0.3], valid, |_| true);
        assert_eq!(at, Some(([1, 0, -1], Axis::Y)));
        // A large cog on a large cog's top, near its east rim: off the top, turned to x.
        let large = ClickedCog {
            large: true,
            axis: Axis::Y,
        };
        let at = cog_helper(true, [0, 0, 0], large, [0.95, 0.6, 0.5], valid, |_| true);
        assert_eq!(at, Some(([1, 1, 0], Axis::X)));
    }

    #[test]
    fn shafts_and_cogs_lie_as_create_places_them() {
        let none = |_: Dir| false;
        let near = Nearby::default();
        let on = |face: Dir, sneaking: bool| Context {
            yaw: 0.0,
            pitch: 0.0,
            sneaking,
            clicked_face: face,
        };
        // A shaft on the floor, looking south: along the view, not the clicked face.
        assert_eq!(
            axis(Block::Shaft, on(Dir::Up, false), none, near),
            Some((Axis::Z, false))
        );
        // Beside a shaft to the west: along it; sneaking, the clicked face's.
        let west = |d: Dir| d == Dir::West;
        assert_eq!(
            axis(Block::Shaft, on(Dir::Up, false), west, near),
            Some((Axis::X, false))
        );
        assert_eq!(
            axis(Block::Shaft, on(Dir::Up, true), west, near),
            Some((Axis::Y, false))
        );
        // A cog on the floor: the clicked face's; on a small cog, its axis.
        assert_eq!(
            axis(Block::Cogwheel, on(Dir::Up, false), none, near),
            Some((Axis::Y, false))
        );
        let on_cog = Nearby {
            small_cog_clicked: Some(Axis::X),
            ..near
        };
        assert_eq!(
            axis(Block::LargeCogwheel, on(Dir::Up, false), none, on_cog),
            Some((Axis::X, false))
        );
        // A large cog on a speed controller along x: across it.
        let on_controller = Nearby {
            controller_below: Some(Axis::X),
            ..near
        };
        assert_eq!(
            axis(
                Block::LargeCogwheel,
                on(Dir::Up, false),
                none,
                on_controller
            ),
            Some((Axis::Z, false))
        );
        // Gearboxes stand upright; encased shafts take the view's axis.
        assert_eq!(
            axis(Block::Gearbox, on(Dir::North, false), none, near),
            Some((Axis::Y, false))
        );
        assert_eq!(
            axis(Block::Clutch, on(Dir::Up, false), none, near),
            Some((Axis::Z, false))
        );
    }
}
