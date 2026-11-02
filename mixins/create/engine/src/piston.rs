//! Create's mechanical piston (`MechanicalPistonBlockEntity`, `LinearActuatorBlockEntity`,
//! `PistonContraption`): its poles behind it set how far it reaches; turning, it takes its head,
//! poles and the structure in front off the world and slides them `speed / 512` blocks a tick,
//! then puts them back where they stop.

use crate::contraption::{Found, MAX_BLOCKS, Refused};
use crate::kinetics::{Axis, Dir};
use std::collections::{HashSet, VecDeque};

/// `maxAllowedPistonPoles` (`kinetics.maxPistonPoles`).
pub const MAX_POLES: i32 = 64;

/// A block as the piston's pole search reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoleCell {
    /// A piston extension pole along an axis.
    Pole(Axis),
    /// A piston head facing a direction.
    Head(Dir),
    Other,
}

/// What a piston takes when it starts: its head and poles (`poles`, offsets from the anchor),
/// how far it can go and where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Collected {
    /// `anchor`: the block in front of the head, from which the structure is searched.
    pub anchor: [i32; 3],
    /// The poles and head, offset from the anchor, and whether each is the head.
    pub poles: Vec<([i32; 3], bool)>,
    /// `extensionLength`, `initialExtensionProgress`.
    pub range: i32,
    pub progress: i32,
}

fn add(a: [i32; 3], d: Dir, n: i32) -> [i32; 3] {
    let o = d.offset();
    [a[0] + o[0] * n, a[1] + o[1] * n, a[2] + o[2] * n]
}

/// `PistonContraption.collectExtensions` for the piston at `pos` facing `facing`, `extended` when
/// its state is `EXTENDED`. None without poles (`noPistonPoles`) or with too many.
pub fn collect(
    pos: [i32; 3],
    facing: Dir,
    extended: bool,
    cell: impl Fn([i32; 3]) -> PoleCell,
) -> Option<Collected> {
    let mut poles: Vec<([i32; 3], bool)> = Vec::new();
    let mut front = 0;
    if extended {
        let mut at = pos;
        loop {
            let next = add(at, facing, 1);
            match cell(next) {
                PoleCell::Pole(a) if a == facing.axis() => {
                    at = next;
                    poles.push((at, false));
                    front += 1;
                }
                PoleCell::Head(f) if f == facing => {
                    poles.push((next, true));
                    front += 1;
                    break;
                }
                _ => break,
            }
            if front > MAX_POLES {
                return None;
            }
        }
    }
    // The head sits in the piston while retracted; a pole there when extended.
    poles.push((pos, front == 0));
    let mut back = 0;
    let mut end = pos;
    while let PoleCell::Pole(a) = cell(add(end, facing.opposite(), 1)) {
        if a != facing.axis() {
            break;
        }
        end = add(end, facing.opposite(), 1);
        poles.push((end, false));
        back += 1;
        if front + back > MAX_POLES {
            return None;
        }
    }
    let range = front + back;
    if range == 0 {
        return None;
    }
    let anchor = add(pos, facing, front + 1);
    let poles = poles
        .into_iter()
        .map(|(p, head)| {
            let rel = add(p, facing, -front);
            (
                [rel[0] - anchor[0], rel[1] - anchor[1], rel[2] - anchor[2]],
                head,
            )
        })
        .collect();
    Some(Collected {
        anchor,
        poles,
        range,
        progress: front,
    })
}

/// `Contraption.searchMovedStructure` from `start` with a forced direction: blocks stuck to each
/// other (slime, honey) and the block ahead in `forced` move together; an unmovable block ahead
/// refuses the move. Offsets from `start`, and states.
pub fn gather(
    start: [i32; 3],
    forced: Dir,
    exclude: &[[i32; 3]],
    at: impl Fn([i32; 3]) -> Option<Found>,
) -> Result<Vec<([i32; 3], u32)>, Refused> {
    let mut frontier = VecDeque::from([start]);
    let mut visited: HashSet<[i32; 3]> = HashSet::new();
    let mut blocks = Vec::new();
    while let Some(pos) = frontier.pop_front() {
        if !visited.insert(pos) || exclude.contains(&pos) {
            continue;
        }
        let Some(found) = at(pos) else {
            continue;
        };
        if !found.movable {
            return Err(Refused::Unmovable(pos));
        }
        if let Some(c) = found.chassis {
            for p in crate::contraption::chassis_reach(pos, c, Some(forced), &at) {
                if !visited.contains(&p) && !exclude.contains(&p) {
                    frontier.push_back(p);
                }
            }
        }
        if let Some(p) = crate::contraption::sticker_target(pos, &found)
            .filter(|p| !visited.contains(p) && !exclude.contains(p))
        {
            frontier.push_back(p);
        }
        for d in Dir::ALL {
            let next = add(pos, d, 1);
            if visited.contains(&next) || exclude.contains(&next) {
                continue;
            }
            let Some(other) = at(next) else {
                continue;
            };
            if !other.movable {
                if d == forced {
                    return Err(Refused::Unmovable(next));
                }
                continue;
            }
            let stick = found.sticky.is_some() || other.sticky.is_some();
            if stick || d == forced || crate::contraption::glued_toward(&found, d) {
                frontier.push_back(next);
            }
        }
        blocks.push((
            [pos[0] - start[0], pos[1] - start[1], pos[2] - start[2]],
            found.state,
        ));
        if blocks.len() > MAX_BLOCKS {
            return Err(Refused::TooLarge);
        }
    }
    Ok(blocks)
}

/// `MechanicalPistonBlockEntity.getMovementSpeed` on the server: blocks a tick, positive out,
/// held between the ends.
pub fn movement(speed: f32, facing: Dir, offset: f32, range: i32) -> f32 {
    let m = (speed / 512.0).clamp(-0.49, 0.49);
    let step = if facing.positive() { 1.0 } else { -1.0 };
    let modifier = step * if facing.axis() == Axis::Z { -1.0 } else { 1.0 };
    (m * -modifier).clamp(-offset, range as f32 - offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_retracted_piston_takes_its_poles_behind_it() {
        // A piston at x 0 facing east, three poles behind it (x -1..-3).
        let cell = |p: [i32; 3]| {
            if (-3..=-1).contains(&p[0]) && p[1] == 0 && p[2] == 0 {
                PoleCell::Pole(Axis::X)
            } else {
                PoleCell::Other
            }
        };
        let c = collect([0, 0, 0], Dir::East, false, cell).unwrap();
        assert_eq!((c.anchor, c.range, c.progress), ([1, 0, 0], 3, 0));
        assert_eq!(c.poles[0], ([-1, 0, 0], true), "the head, in the piston");
        assert_eq!(c.poles.last(), Some(&([-4, 0, 0], false)));
        assert!(collect([0, 0, 0], Dir::East, false, |_| PoleCell::Other).is_none());
    }

    #[test]
    fn glued_faces_and_an_extended_sticker_take_their_neighbours() {
        use crate::glue::{Glue, glued_faces};
        // Pushed east from x 1: stone at x 1, a stone glued on top of it (y 1), a loose stone
        // north of it, and an extended sticker at x 2 facing up holding a block at (2, 1, 0).
        let glue = [Glue::span([1, 0, 0], [1, 1, 0])];
        let at = |p: [i32; 3]| -> Option<Found> {
            let stone = Found {
                state: 1,
                movable: true,
                glued: glued_faces(&glue, p),
                ..Default::default()
            };
            match p {
                [1, 0, 0] | [1, 1, 0] | [1, 0, -1] | [2, 1, 0] => Some(stone),
                [2, 0, 0] => Some(Found {
                    state: 2,
                    sticker: Some(Dir::Up),
                    ..stone
                }),
                _ => None,
            }
        };
        let mut got = gather([1, 0, 0], Dir::East, &[], at).unwrap();
        got.sort();
        assert_eq!(
            got,
            vec![
                ([0, 0, 0], 1),
                ([0, 1, 0], 1),
                ([1, 0, 0], 2),
                ([1, 1, 0], 1)
            ],
            "the glued and stuck blocks come, the loose one stays"
        );
        // A sticker facing bedrock refuses the move.
        let stuck = |p: [i32; 3]| match p {
            [1, 0, 0] => Some(Found {
                movable: true,
                sticker: Some(Dir::Up),
                ..Default::default()
            }),
            [1, 1, 0] => Some(Found::default()),
            _ => None,
        };
        assert_eq!(
            gather([1, 0, 0], Dir::East, &[], stuck),
            Err(Refused::Unmovable([1, 1, 0]))
        );
    }

    #[test]
    fn it_pushes_what_is_ahead_and_moves_at_its_speed() {
        let stone = |p: [i32; 3]| {
            (p[0] >= 1 && p[0] <= 2 && p[1] == 0 && p[2] == 0).then_some(Found {
                state: 1,
                sail_facing: None,
                sticky: None,
                movable: true,
                chassis: None,
                ..Default::default()
            })
        };
        let got = gather([1, 0, 0], Dir::East, &[], stone).unwrap();
        assert_eq!(got, vec![([0, 0, 0], 1), ([1, 0, 0], 1)]);
        // Facing east: -64 rpm extends 0.125 blocks a tick, 64 retracts (none left at 0); the
        // far end stops it.
        assert_eq!(movement(64.0, Dir::East, 0.0, 3), 0.0);
        assert_eq!(movement(-64.0, Dir::East, 0.0, 3), 0.125);
        assert_eq!(movement(-64.0, Dir::East, 2.95, 3), 0.049999952);
        assert_eq!(movement(64.0, Dir::North, 1.0, 3), -0.125);
    }
}
