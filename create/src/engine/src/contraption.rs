//! Create's contraptions (`Contraption.searchMovedStructure`, `BlockMovementChecks`), for the
//! windmill bearing first (`BearingContraption`, `WindmillBearingBlockEntity`): the structure in
//! front of the bearing, as Create gathers it, and the speed its sails give.

use crate::kinetics::Dir;
use std::collections::{HashSet, VecDeque};

/// A block as the search reads it: its id, whether it is in `create:windmill_sails` (and so a
/// `SailBlock` and its `facing`), whether it is slime or honey, and whether it can move at all.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub state: u32,
    pub sail_facing: Option<Dir>,
    pub sticky: Option<Sticky>,
    pub movable: bool,
    /// A linear chassis (`LinearChassisBlock`) and what it holds on to.
    pub chassis: Option<Chassis>,
    /// Its faces super glue holds to the block beside them (`SuperGlueEntity.isGlued`, a
    /// `Dir::bit` each).
    pub glued: u8,
    /// An extended sticker's facing (`StickerBlock.EXTENDED`): it takes the block it faces.
    pub sticker: Option<Dir>,
    /// The sides it hangs on to the block beside (`BlockMovementChecks.isBlockAttachedTowards`
    /// for all but sails: a torch down, a ladder or button the way it faces), a `Dir::bit` each.
    pub attached: u8,
    /// The faces that push nothing in front of them (`BlockMovementChecks.isNotSupportive`: a
    /// drill's head, a sail's flat sides, anything brittle), a `Dir::bit` each.
    pub unsupportive: u8,
    /// A belt segment (`BeltBlock`): the whole belt moves with it (`moveBelt`).
    pub belt: Option<crate::belt::Segment>,
}

/// `Contraption.moveBlock`'s extended sticker: the block it faces joins the frontier (an
/// unmovable one there then refuses the structure).
pub(crate) fn sticker_target(pos: [i32; 3], f: &Found) -> Option<[i32; 3]> {
    f.sticker.map(|d| add(pos, d.offset()))
}

/// `Contraption.moveBelt`: a belt segment takes the segments before and after it.
pub(crate) fn belt_neighbours(pos: [i32; 3], f: &Found) -> impl Iterator<Item = [i32; 3]> + '_ {
    [true, false]
        .into_iter()
        .filter_map(move |forward| f.belt.as_ref()?.next(pos, forward))
}

/// `faceHasGlue`: glue on this face of the block takes its neighbour.
pub(crate) fn glued_toward(f: &Found, d: Dir) -> bool {
    f.glued & d.bit() != 0
}

/// A linear chassis as the search reads it: its kind (`sameKind`: linear or secondary), axis,
/// glued ends (`sticky_top` toward the positive end, `sticky_bottom`) and range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chassis {
    pub secondary: bool,
    pub axis: crate::kinetics::Axis,
    pub sticky_top: bool,
    pub sticky_bottom: bool,
    pub range: i32,
    /// A radial chassis (`RadialChassisBlock`): its glued sides (a `Dir::bit` each, across its
    /// axis) instead of glued ends.
    pub radial: Option<u8>,
}

/// `RadialChassisBlock.getGlueableSide`: the `sticky_*` property a face of a radial chassis on
/// `axis` glues (none along its axis).
pub fn radial_side(axis: crate::kinetics::Axis, face: Dir) -> Option<&'static str> {
    use crate::kinetics::Axis;
    Some(match (axis, face) {
        (Axis::X, Dir::North) => "sticky_west",
        (Axis::X, Dir::South) => "sticky_east",
        (Axis::X, Dir::Up) => "sticky_north",
        (Axis::X, Dir::Down) => "sticky_south",
        (Axis::Y, Dir::North) => "sticky_north",
        (Axis::Y, Dir::South) => "sticky_south",
        (Axis::Y, Dir::East) => "sticky_east",
        (Axis::Y, Dir::West) => "sticky_west",
        (Axis::Z, Dir::Up) => "sticky_north",
        (Axis::Z, Dir::Down) => "sticky_south",
        (Axis::Z, Dir::East) => "sticky_east",
        (Axis::Z, Dir::West) => "sticky_west",
        _ => return None,
    })
}

/// `ChassisBlockEntity.getIncludedBlockPositionsRadial`: from each glued side, the blocks in the
/// chassis' plane reached through blocks that need moving, closer than its range and a half.
fn radial_reach(
    pos: [i32; 3],
    c: Chassis,
    sides: u8,
    at: &impl Fn([i32; 3]) -> Option<Found>,
) -> Vec<[i32; 3]> {
    let mut out = Vec::new();
    let limit = (f64::from(c.range) + 0.5).powi(2);
    for facing in Dir::ALL {
        if facing.axis() == c.axis || sides & facing.bit() == 0 {
            continue;
        }
        let mut frontier = VecDeque::from([add(pos, facing.offset())]);
        let mut visited = HashSet::new();
        while let Some(p) = frontier.pop_front() {
            if visited.contains(&p) {
                continue;
            }
            let d: [i32; 3] = std::array::from_fn(|i| p[i] - pos[i]);
            let dist = f64::from(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
            if dist >= limit {
                continue;
            }
            // `isMovementNecessary`, and nothing brittle.
            let Some(f) = at(p).filter(|f| f.unsupportive != 0x3F) else {
                continue;
            };
            visited.insert(p);
            if p != pos && !out.contains(&p) {
                out.push(p);
            }
            for offset in Dir::ALL {
                // Back through the chassis only the way of this glued side.
                if offset.axis() == c.axis
                    || (p == pos && offset != facing)
                    || not_supportive(&f, offset)
                {
                    continue;
                }
                frontier.push_back(add(p, offset.offset()));
            }
        }
    }
    out
}

/// `ChassisBlockEntity`'s range when not set (`maxChassisRange / 2`).
pub const CHASSIS_RANGE: i32 = 8;

/// `Contraption.moveChassis` for the chassis at `pos`: the chassis beside it of its kind on its
/// axis (`addAttachedChasses`), then `getIncludedBlockPositionsLinear`: from each end, along its
/// axis, up to its range of blocks that need moving, while that end is glued or is the way the
/// structure moves.
pub fn chassis_reach(
    pos: [i32; 3],
    c: Chassis,
    forced: Option<Dir>,
    at: &impl Fn([i32; 3]) -> Option<Found>,
) -> Vec<[i32; 3]> {
    // `addAttachedChasses` for a radial chassis: the radial chassis on its axis either side.
    if let Some(sides) = c.radial {
        let mut out = radial_reach(pos, c, sides, at);
        for positive in [true, false] {
            let d = Dir::ALL
                .into_iter()
                .find(|d| d.axis() == c.axis && d.positive() == positive)
                .unwrap_or(Dir::Up);
            let next = add(pos, d.offset());
            if let Some(Found {
                chassis: Some(o), ..
            }) = at(next)
                && o.radial.is_some()
                && o.axis == c.axis
                && !out.contains(&next)
            {
                out.push(next);
            }
        }
        let _ = forced;
        return out;
    }
    let mut out = Vec::new();
    for d in Dir::ALL {
        let next = add(pos, d.offset());
        if let Some(Found {
            chassis: Some(o), ..
        }) = at(next)
            && o.radial.is_none()
            && o.secondary == c.secondary
            && o.axis == c.axis
        {
            out.push(next);
        }
    }
    for positive in [true, false] {
        let facing = Dir::ALL
            .into_iter()
            .find(|d| d.axis() == c.axis && d.positive() == positive)
            .unwrap_or(Dir::Up);
        let sticky = if positive {
            c.sticky_top
        } else {
            c.sticky_bottom
        };
        if forced != Some(facing) && !sticky {
            continue;
        }
        let o = facing.offset();
        for i in 1..=c.range {
            let current = [pos[0] + o[0] * i, pos[1] + o[1] * i, pos[2] + o[2] * i];
            // Up to air, a brittle block, or a face that pushes nothing on.
            let Some(f) = at(current).filter(|f| f.unsupportive != 0x3F) else {
                break;
            };
            out.push(current);
            if not_supportive(&f, facing) {
                break;
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sticky {
    Slime,
    Honey,
}

/// Why a structure could not be gathered (`AssemblyException`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    Unmovable([i32; 3]),
    TooLarge,
    NotEnoughSails(u32),
    Empty,
}

/// A gathered structure: each block's offset from the bearing and state, and its sail count.
pub type Gathered = (Vec<([i32; 3], u32)>, u32);

/// `kinetics.maxBlocksMoved`.
pub const MAX_BLOCKS: usize = 2048;
/// `kinetics.minimumWindmillSails` and `windmillSailsPerRPM`.
pub const MIN_SAILS: u32 = 8;
pub const SAILS_PER_RPM: u32 = 8;

fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `BlockMovementChecks.isBlockAttachedTowards`: a sail holds on to the blocks beside it in its
/// own plane; other blocks to the sides they hang on (`Found::attached`).
pub(crate) fn attached_towards(f: &Found, toward: Dir) -> bool {
    f.attached & toward.bit() != 0 || f.sail_facing.is_some_and(|d| toward.axis() != d.axis())
}

/// `BlockMovementChecks.isNotSupportive`: the face `facing` of the block pushes nothing.
pub(crate) fn not_supportive(f: &Found, facing: Dir) -> bool {
    f.unsupportive & facing.bit() != 0 || f.sail_facing.is_some_and(|d| facing.axis() == d.axis())
}

/// `canStickTo` both ways, as vanilla pistons treat sticky blocks: slime and honey blocks stick
/// to any block but each other.
pub(crate) fn sticks(a: &Found, b: &Found) -> bool {
    let either = a.sticky.is_some() || b.sticky.is_some();
    let clash = matches!(
        (a.sticky, b.sticky),
        (Some(Sticky::Slime), Some(Sticky::Honey)) | (Some(Sticky::Honey), Some(Sticky::Slime))
    );
    either && !clash
}

/// `BearingContraption.assemble` for a windmill bearing at `bearing` facing `facing`: the blocks
/// (offset from the bearing, state) and how many of them are sails. `at` reads the world: None
/// for a block that needn't move (air, a fluid).
pub fn gather_windmill(
    bearing: [i32; 3],
    facing: Dir,
    at: impl Fn([i32; 3]) -> Option<Found>,
) -> Result<Gathered, Refused> {
    gather_bearing(bearing, facing, MIN_SAILS, at)
}

/// `BearingContraption.assemble` for any bearing: a windmill needs `min_sails` (a mechanical
/// bearing none).
pub fn gather_bearing(
    bearing: [i32; 3],
    facing: Dir,
    min_sails: u32,
    at: impl Fn([i32; 3]) -> Option<Found>,
) -> Result<Gathered, Refused> {
    let start = add(bearing, facing.offset());
    let (blocks, sails) = gather_from(start, bearing, |p| p == bearing, at)?;
    if sails < min_sails {
        return Err(Refused::NotEnoughSails(sails));
    }
    if blocks.is_empty() {
        return Err(Refused::Empty);
    }
    Ok((blocks, sails))
}

/// `Contraption.searchMovedStructure` from `start` with no forced direction: the block there
/// and what holds on to it (slime, honey, sails in their plane, chassis), never a block `skip`
/// names (`isAnchoringBlockAt`, a clock's other hand). Offsets from `origin`, and the sails.
pub fn gather_from(
    start: [i32; 3],
    origin: [i32; 3],
    skip: impl Fn([i32; 3]) -> bool,
    at: impl Fn([i32; 3]) -> Option<Found>,
) -> Result<Gathered, Refused> {
    let mut frontier = VecDeque::from([start]);
    let mut visited: HashSet<[i32; 3]> = HashSet::new();
    let mut blocks = Vec::new();
    let mut sails = 0;
    while let Some(pos) = frontier.pop_front() {
        if !visited.insert(pos) {
            continue;
        }
        if skip(pos) {
            continue;
        }
        let Some(found) = at(pos) else {
            continue;
        };
        if !found.movable {
            return Err(Refused::Unmovable(pos));
        }
        if let Some(c) = found.chassis {
            for p in chassis_reach(pos, c, None, &at) {
                if !visited.contains(&p) && !skip(p) {
                    frontier.push_back(p);
                }
            }
        }
        if let Some(p) = sticker_target(pos, &found).filter(|p| !visited.contains(p) && !skip(*p)) {
            frontier.push_back(p);
        }
        for p in belt_neighbours(pos, &found) {
            if !visited.contains(&p) && !skip(p) {
                frontier.push_back(p);
            }
        }
        for d in Dir::ALL {
            let next = add(pos, d.offset());
            if visited.contains(&next) || skip(next) {
                continue;
            }
            let Some(other) = at(next) else {
                continue;
            };
            if !other.movable {
                continue;
            }
            if sticks(&found, &other)
                || attached_towards(&other, d.opposite())
                || glued_toward(&found, d)
            {
                frontier.push_back(next);
            }
        }
        if found.sail_facing.is_some() {
            sails += 1;
        }
        blocks.push((
            [pos[0] - origin[0], pos[1] - origin[1], pos[2] - origin[2]],
            found.state,
        ));
        if blocks.len() > MAX_BLOCKS {
            return Err(Refused::TooLarge);
        }
    }
    Ok((blocks, sails))
}

/// `GantryContraption.assemble` (`searchMovedStructure` from the carriage, `moveGantryPinion`):
/// the carriage, the block in front of it and what holds on to them, never the shaft it rides
/// on (`isAnchoringBlockAt`); a chassis reaches along the way it moves. Offsets from the carriage.
pub fn gather_gantry(
    carriage: [i32; 3],
    facing: Dir,
    moving: Dir,
    at: impl Fn([i32; 3]) -> Option<Found>,
) -> Result<Vec<([i32; 3], u32)>, Refused> {
    let shaft = add(carriage, facing.opposite().offset());
    let mut frontier = VecDeque::from([carriage, add(carriage, facing.offset())]);
    let mut visited: HashSet<[i32; 3]> = HashSet::new();
    let mut blocks = Vec::new();
    while let Some(pos) = frontier.pop_front() {
        if pos == shaft || !visited.insert(pos) {
            continue;
        }
        let Some(found) = at(pos) else {
            continue;
        };
        if !found.movable {
            return Err(Refused::Unmovable(pos));
        }
        if let Some(c) = found.chassis {
            for p in chassis_reach(pos, c, Some(moving), &at) {
                if !visited.contains(&p) && p != shaft {
                    frontier.push_back(p);
                }
            }
        }
        if let Some(p) = sticker_target(pos, &found).filter(|p| !visited.contains(p) && *p != shaft)
        {
            frontier.push_back(p);
        }
        for p in belt_neighbours(pos, &found) {
            if !visited.contains(&p) && p != shaft {
                frontier.push_back(p);
            }
        }
        for d in Dir::ALL {
            let next = add(pos, d.offset());
            if visited.contains(&next) || next == shaft {
                continue;
            }
            let Some(other) = at(next) else {
                continue;
            };
            if other.movable
                && (sticks(&found, &other)
                    || attached_towards(&other, d.opposite())
                    || glued_toward(&found, d))
            {
                frontier.push_back(next);
            }
        }
        blocks.push((
            [
                pos[0] - carriage[0],
                pos[1] - carriage[1],
                pos[2] - carriage[2],
            ],
            found.state,
        ));
        if blocks.len() > MAX_BLOCKS {
            return Err(Refused::TooLarge);
        }
    }
    Ok(blocks)
}

/// `WindmillBearingBlockEntity.getGeneratedSpeed`: a sail per 8 gives an rpm, 1 to 16,
/// clockwise positive.
pub fn windmill_speed(sails: u32, clockwise: bool) -> f32 {
    let rpm = (sails / SAILS_PER_RPM).clamp(1, 16) as f32;
    if clockwise { rpm } else { -rpm }
}

/// A mechanical bearing's turning (`MechanicalBearingBlockEntity.angle`): the angle at game time
/// `since` and the speed since then; the angle at any later tick follows without ticking.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Turn {
    pub angle: f32,
    pub since: i64,
    pub rpm: f32,
    /// `sequencedAngleLimit`: the degrees it may still turn from `since` (a sequenced
    /// gearshift's or valve handle's turn), none for no limit.
    pub limit: Option<f32>,
}

impl Turn {
    /// `angle += convertToAngular(speed)` (`speed × 0.3`° a tick) each tick, `% 360`.
    /// Each tick's turn is held to the limit left, which it uses up.
    pub fn angle_at(&self, now: i64) -> f32 {
        (self.angle + self.turned(now)) % 360.0
    }

    /// The degrees turned from `since` to `now`.
    fn turned(&self, now: i64) -> f32 {
        let d = self.rpm * 0.3 * (now - self.since) as f32;
        match self.limit {
            Some(l) => d.clamp(-l, l),
            None => d,
        }
    }

    /// The speed changed at `now`; what is left of the limit goes on.
    pub fn rebase(&mut self, now: i64, rpm: f32) {
        let turned = self.turned(now);
        self.angle = (self.angle + turned) % 360.0;
        self.limit = self.limit.map(|l| (l - turned.abs()).max(0.0));
        self.since = now;
        self.rpm = rpm;
    }
}

/// `StructureTransform`'s angle: the nearest quarter turn (`Math.round(angle / 90) × 90`, in
/// -90..270).
pub fn quarter(angle: f32) -> i32 {
    let mut a = ((angle / 90.0 + 0.5).floor() as i32 * 90) % 360;
    if a < -90 {
        a += 360;
    }
    a
}

/// `StructureTransform.rotation.ordinal()`: how many `getClockWise(axis)` turns a facing takes.
pub fn clockwise_turns(quarter: i32) -> u8 {
    match quarter {
        -90 | 270 => 1,
        180 => 2,
        90 => 3,
        _ => 0,
    }
}

/// `VecHelper.rotate` by a quarter-turn angle about `axis`, exact for whole blocks.
pub fn rotate(v: [i32; 3], quarter: i32, axis: crate::kinetics::Axis) -> [i32; 3] {
    use crate::kinetics::Axis;
    let (s, c) = match quarter.rem_euclid(360) {
        90 => (1, 0),
        180 => (0, -1),
        270 => (-1, 0),
        _ => (0, 1),
    };
    let [x, y, z] = v;
    match axis {
        Axis::X => [x, y * c - z * s, z * c + y * s],
        Axis::Y => [x * c + z * s, y, z * c - x * s],
        Axis::Z => [x * c - y * s, y * c + x * s, z],
    }
}

/// `StructureTransform` as a moved block sees it: the quarter turns taking its offsets and
/// directions from where it was to where it went back (none for a structure that only slid).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transform {
    /// Column `i` is where unit vector `i` goes.
    m: [[i32; 3]; 3],
}

impl Transform {
    pub const NONE: Transform = Transform {
        m: [[1, 0, 0], [0, 1, 0], [0, 0, 1]],
    };

    /// The turn of `place` (offset in the structure to where it went back), from where it puts
    /// the three unit offsets; none when that is no quarter-turn rotation.
    pub fn from_place(place: impl Fn([i32; 3]) -> [i32; 3]) -> Transform {
        let o = place([0; 3]);
        let col = |e: [i32; 3]| {
            let p = place(e);
            [p[0] - o[0], p[1] - o[1], p[2] - o[2]]
        };
        let cols = [col([1, 0, 0]), col([0, 1, 0]), col([0, 0, 1])];
        let m = [0, 1, 2].map(|r| [0, 1, 2].map(|c| cols[c][r]));
        let unit = |v: [i32; 3]| v.iter().map(|x| x.abs()).sum::<i32>() == 1;
        let rows_unit = m.iter().all(|r| unit(*r));
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        if cols.iter().all(|c| unit(*c)) && rows_unit && det == 1 {
            Transform { m }
        } else {
            Transform::NONE
        }
    }

    /// Offset `v` turned.
    pub fn vec(&self, v: [i32; 3]) -> [i32; 3] {
        let m = &self.m;
        [0, 1, 2].map(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
    }

    /// Direction `d` turned.
    pub fn dir(&self, d: crate::kinetics::Dir) -> crate::kinetics::Dir {
        let v = self.vec(d.offset());
        crate::kinetics::Dir::ALL
            .into_iter()
            .find(|x| x.offset() == v)
            .unwrap_or(d)
    }

    /// Whether it turns anything.
    pub fn turns(&self) -> bool {
        *self != Transform::NONE
    }

    /// The axis it turns about and by how much ([`rotate`]'s quarter, in degrees); none when it
    /// turns nothing (or is no single quarter turn about an axis).
    pub fn quarter_axis(&self) -> Option<(i32, crate::kinetics::Axis)> {
        use crate::kinetics::Axis;
        for axis in [Axis::X, Axis::Y, Axis::Z] {
            for quarter in [90, 180, 270] {
                let same = [[1, 0, 0], [0, 1, 0], [0, 0, 1]]
                    .into_iter()
                    .all(|e| self.vec(e) == rotate(e, quarter, axis));
                if same {
                    return Some((quarter, axis));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transform_is_the_turn_its_placement_makes() {
        use crate::kinetics::{Axis, Dir};
        let slid = Transform::from_place(|o| [o[0] + 3, o[1] - 2, o[2] + 7]);
        assert_eq!(slid, Transform::NONE);
        assert!(!slid.turns());
        let turned = Transform::from_place(|o| {
            let r = rotate(o, 90, Axis::Y);
            [r[0] + 10, r[1], r[2] - 4]
        });
        assert!(turned.turns());
        for d in Dir::ALL {
            let v = rotate(d.offset(), 90, Axis::Y);
            assert_eq!(turned.dir(d).offset(), v, "{d:?}");
        }
        assert_eq!(turned.dir(Dir::Up), Dir::Up);
        assert_eq!(turned.quarter_axis(), Some((90, Axis::Y)));
        assert_eq!(slid.quarter_axis(), None);
        // Not a rotation (a mirror): none.
        assert_eq!(
            Transform::from_place(|o| [-o[0], o[1], o[2]]),
            Transform::NONE
        );
    }

    #[test]
    fn a_sequenced_turn_stops_at_its_limit() {
        // 32 rpm (9.6° a tick) held to 45°: stops after 4.7 ticks, at 45.
        let mut t = Turn {
            angle: 10.0,
            since: 0,
            rpm: -32.0,
            limit: Some(45.0),
        };
        assert!((t.angle_at(2) - (10.0 - 19.2)).abs() < 1e-3);
        assert!((t.angle_at(7) + 35.0).abs() < 1e-3);
        // A stall part way keeps what is left.
        t.rebase(2, 0.0);
        assert!((t.limit.unwrap() - 25.8).abs() < 1e-3);
        t.rebase(5, -32.0);
        assert!((t.angle_at(9) + 35.0).abs() < 1e-3);
    }

    #[test]
    fn a_stopped_bearing_snaps_to_the_nearest_quarter_turn() {
        use crate::kinetics::Axis;
        let mut t = Turn {
            angle: 0.0,
            since: 0,
            rpm: 16.0,
            limit: None,
        };
        // 16 rpm: 4.8° a tick; after 20 ticks 96°.
        assert!((t.angle_at(20) - 96.0).abs() < 1e-3);
        t.rebase(20, -16.0);
        assert!((t.angle_at(30) - 48.0).abs() < 1e-3);
        assert_eq!(
            (
                quarter(96.0),
                quarter(44.0),
                quarter(-100.0),
                quarter(200.0)
            ),
            (90, 0, -90, 180)
        );
        assert_eq!(
            (
                clockwise_turns(90),
                clockwise_turns(-90),
                clockwise_turns(180)
            ),
            (3, 1, 2)
        );
        // East of an up-facing bearing's anchor, a quarter turn about y goes north
        // (counterclockwise from above).
        assert_eq!(rotate([1, 0, 0], 90, Axis::Y), [0, 0, -1]);
        assert_eq!(rotate([0, 1, 0], 90, Axis::X), [0, 0, 1]);
        assert_eq!(rotate([2, 1, 0], 180, Axis::Z), [-2, -1, 0]);
    }

    #[test]
    fn a_chassis_carries_its_glued_end_and_its_linked_chassis() {
        use crate::kinetics::Axis;
        // A vertical chassis at the origin glued on top, a second one above it, stone above
        // both up to y 4, air at 5; another stone below (its bottom isn't glued).
        let chassis = Chassis {
            secondary: false,
            axis: Axis::Y,
            sticky_top: true,
            sticky_bottom: false,
            range: CHASSIS_RANGE,
            radial: None,
        };
        let at = |p: [i32; 3]| -> Option<Found> {
            if p[0] != 0 || p[2] != 0 || !(-1..=4).contains(&p[1]) {
                return None;
            }
            Some(Found {
                state: 1,
                sail_facing: None,
                sticky: None,
                movable: true,
                chassis: (p[1] == 0 || p[1] == 1).then_some(chassis),
                ..Default::default()
            })
        };
        let reach = chassis_reach([0, 0, 0], chassis, None, &at);
        assert_eq!(
            reach,
            vec![[0, 1, 0], [0, 1, 0], [0, 2, 0], [0, 3, 0], [0, 4, 0]]
        );
        // Moved down, its bottom end goes too.
        assert!(chassis_reach([0, 0, 0], chassis, Some(Dir::Down), &at).contains(&[0, -1, 0]));
        let range = Chassis {
            range: 2,
            ..chassis
        };
        assert!(!chassis_reach([0, 0, 0], range, None, &at).contains(&[0, 3, 0]));
    }

    #[test]
    fn a_radial_chassis_carries_its_glued_sides_within_its_radius() {
        use crate::kinetics::Axis;
        // A radial chassis on y glued on its east side; a 5x5 floor of stone around it at its
        // height and one stone above it; range 2.
        let chassis = Chassis {
            secondary: false,
            axis: Axis::Y,
            sticky_top: false,
            sticky_bottom: false,
            range: 2,
            radial: Some(Dir::East.bit()),
        };
        let at = |p: [i32; 3]| {
            let floor = p[1] == 0 && p[0].abs() <= 3 && p[2].abs() <= 3;
            (floor || p == [0, 1, 0]).then(|| Found {
                state: 1,
                movable: true,
                chassis: (p == [0, 0, 0]).then_some(chassis),
                ..Found::default()
            })
        };
        let reach = chassis_reach([0, 0, 0], chassis, None, &at);
        assert!(reach.contains(&[1, 0, 0]) && reach.contains(&[2, 0, 0]));
        // Through the plane round the chassis, within 2.5 of it.
        assert!(reach.contains(&[-2, 0, 0]) && reach.contains(&[1, 0, 2]));
        assert!(!reach.contains(&[2, 0, 2]), "2.83 away");
        assert!(!reach.contains(&[3, 0, 0]));
        assert!(!reach.contains(&[0, 1, 0]), "not along its axis");
        assert!(!reach.contains(&[0, 0, 0]));
        assert_eq!(radial_side(Axis::X, Dir::Up), Some("sticky_north"));
        assert_eq!(radial_side(Axis::Z, Dir::North), None);
    }

    fn sail() -> Found {
        Found {
            state: 7,
            sail_facing: Some(Dir::North),
            sticky: None,
            movable: true,
            chassis: None,
            ..Default::default()
        }
    }

    #[test]
    fn sails_in_front_of_the_bearing_gather_in_their_plane() {
        // A bearing at the origin facing north; a cross of sails at z = -1 in the xy plane
        // (facing north), and a stone behind the plane that nothing holds.
        let at = |p: [i32; 3]| match p {
            [x, y, -1]
                if (x == 0 && (-4..=4).contains(&y)) || (y == 0 && (-4..=4).contains(&x)) =>
            {
                Some(sail())
            }
            [0, 0, -2] => Some(Found {
                state: 1,
                sail_facing: None,
                sticky: None,
                movable: true,
                chassis: None,
                ..Default::default()
            }),
            _ => None,
        };
        let (blocks, sails) = gather_windmill([0, 0, 0], Dir::North, at).unwrap();
        assert_eq!(sails, 17);
        assert_eq!(blocks.len(), 17, "the stone behind is not attached");
        assert!(blocks.iter().all(|(o, _)| o[2] == -1));
        assert_eq!(windmill_speed(sails, true), 2.0);
        assert_eq!(windmill_speed(200, false), -16.0);
        assert_eq!(windmill_speed(3, true), 1.0);
    }

    #[test]
    fn too_few_sails_or_an_unmovable_block_refuse() {
        let few = |p: [i32; 3]| (p[2] == -1 && p[0] == 0 && (0..3).contains(&p[1])).then(sail);
        assert_eq!(
            gather_windmill([0, 0, 0], Dir::North, few),
            Err(Refused::NotEnoughSails(3))
        );
        let stuck = |p: [i32; 3]| {
            (p == [0, 0, -1]).then(|| Found {
                movable: false,
                chassis: None,
                ..sail()
            })
        };
        assert_eq!(
            gather_windmill([0, 0, 0], Dir::North, stuck),
            Err(Refused::Unmovable([0, 0, -1]))
        );
    }

    #[test]
    fn a_radial_chassis_takes_only_what_its_glued_side_reaches() {
        // A radial chassis on y at the origin glued north, stone north and south of it.
        let chassis = Chassis {
            secondary: false,
            axis: crate::kinetics::Axis::Y,
            sticky_top: false,
            sticky_bottom: false,
            range: 2,
            radial: Some(Dir::North.bit()),
        };
        let at = |p: [i32; 3]| match p {
            [0, 0, 0] => Some(Found {
                movable: true,
                chassis: Some(chassis),
                ..Default::default()
            }),
            [0, 0, -1] | [0, 0, 1] => Some(Found {
                movable: true,
                ..Default::default()
            }),
            _ => None,
        };
        assert_eq!(
            chassis_reach([0, 0, 0], chassis, None, &at),
            vec![[0, 0, -1]]
        );
    }
}
