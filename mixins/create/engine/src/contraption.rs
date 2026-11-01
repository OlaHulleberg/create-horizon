//! Create's contraptions (`Contraption.searchMovedStructure`, `BlockMovementChecks`), for the
//! windmill bearing first (`BearingContraption`, `WindmillBearingBlockEntity`): the structure in
//! front of the bearing, as Create gathers it, and the speed its sails give.

use crate::kinetics::Dir;
use std::collections::{HashSet, VecDeque};

/// A block as the search reads it: its id, whether it is in `create:windmill_sails` (and so a
/// `SailBlock` and its `facing`), whether it is slime or honey, and whether it can move at all.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub state: u32,
    pub sail_facing: Option<Dir>,
    pub sticky: Option<Sticky>,
    pub movable: bool,
    /// A linear chassis (`LinearChassisBlock`) and what it holds on to.
    pub chassis: Option<Chassis>,
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
    let mut out = Vec::new();
    for d in Dir::ALL {
        let next = add(pos, d.offset());
        if let Some(Found {
            chassis: Some(o), ..
        }) = at(next)
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
            if at(current).is_none() {
                break;
            }
            out.push(current);
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

/// `BlockMovementChecks.isBlockAttachedTowards` for what is ported: a sail holds on to the
/// blocks beside it in its own plane.
fn attached_towards(f: &Found, toward: Dir) -> bool {
    f.sail_facing.is_some_and(|d| toward.axis() != d.axis())
}

/// `canStickTo` both ways (NeoForge: slime and honey stick to anything but each other).
fn sticks(a: &Found, b: &Found) -> bool {
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
    let mut frontier = VecDeque::from([start]);
    let mut visited: HashSet<[i32; 3]> = HashSet::new();
    let mut blocks = Vec::new();
    let mut sails = 0;
    while let Some(pos) = frontier.pop_front() {
        if !visited.insert(pos) {
            continue;
        }
        // isAnchoringBlockAt: the bearing itself stays.
        if pos == bearing {
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
                if !visited.contains(&p) && p != bearing {
                    frontier.push_back(p);
                }
            }
        }
        for d in Dir::ALL {
            let next = add(pos, d.offset());
            if visited.contains(&next) || next == bearing {
                continue;
            }
            let Some(other) = at(next) else {
                continue;
            };
            if !other.movable {
                continue;
            }
            if sticks(&found, &other) || attached_towards(&other, d.opposite()) {
                frontier.push_back(next);
            }
        }
        if found.sail_facing.is_some() {
            sails += 1;
        }
        blocks.push((
            [
                pos[0] - bearing[0],
                pos[1] - bearing[1],
                pos[2] - bearing[2],
            ],
            found.state,
        ));
        if blocks.len() > MAX_BLOCKS {
            return Err(Refused::TooLarge);
        }
    }
    if sails < min_sails {
        return Err(Refused::NotEnoughSails(sails));
    }
    if blocks.is_empty() {
        return Err(Refused::Empty);
    }
    Ok((blocks, sails))
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
}

impl Turn {
    /// `angle += convertToAngular(speed)` (`speed × 0.3`° a tick) each tick, `% 360`.
    pub fn angle_at(&self, now: i64) -> f32 {
        (self.angle + self.rpm * 0.3 * (now - self.since) as f32) % 360.0
    }

    /// The speed changed at `now`.
    pub fn rebase(&mut self, now: i64, rpm: f32) {
        self.angle = self.angle_at(now);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stopped_bearing_snaps_to_the_nearest_quarter_turn() {
        use crate::kinetics::Axis;
        let mut t = Turn {
            angle: 0.0,
            since: 0,
            rpm: 16.0,
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

    fn sail() -> Found {
        Found {
            state: 7,
            sail_facing: Some(Dir::North),
            sticky: None,
            movable: true,
            chassis: None,
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
}
