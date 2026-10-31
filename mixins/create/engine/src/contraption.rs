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
    if sails < MIN_SAILS {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sail() -> Found {
        Found {
            state: 7,
            sail_facing: Some(Dir::North),
            sticky: None,
            movable: true,
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
                ..sail()
            })
        };
        assert_eq!(
            gather_windmill([0, 0, 0], Dir::North, stuck),
            Err(Refused::Unmovable([0, 0, -1]))
        );
    }
}
