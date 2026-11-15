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

/// `TranslatingContraption.createColliders`: the blocks of a structure (offsets) with no block
/// of it next to them going `dir`, the ones that meet the world first.
pub fn colliders(blocks: &[([i32; 3], u32)], dir: Dir) -> Vec<[i32; 3]> {
    let set: HashSet<[i32; 3]> = blocks.iter().map(|(p, _)| *p).collect();
    blocks
        .iter()
        .map(|(p, _)| *p)
        .filter(|p| !set.contains(&add(*p, dir, 1)))
        .collect()
}

/// `ContraptionCollider.collideBlocks` for a piston's structure (offsets from `anchor` at
/// offset 0, along `facing`) moved to offset `to` going `dir`: the world cells its leading
/// blocks enter (`BlockPos.containing(position)`, a block further on going a positive way),
/// each with its block's offset.
pub fn entered_cells(
    anchor: [i32; 3],
    facing: Dir,
    to: f32,
    dir: Dir,
    colliders: &[[i32; 3]],
) -> Vec<([i32; 3], [i32; 3])> {
    let f = facing.offset();
    let mut grid = [0, 1, 2].map(|i| (anchor[i] as f32 + f[i] as f32 * to).floor() as i32);
    if dir.positive() {
        grid = add(grid, dir, 1);
    }
    colliders
        .iter()
        .map(|l| ([grid[0] + l[0], grid[1] + l[1], grid[2] + l[2]], *l))
        .collect()
}

/// `LinearActuatorBlockEntity.getGridOffset`.
pub fn grid_offset(offset: f32, range: i32) -> f32 {
    ((offset + 0.5) as i32).clamp(0, range) as f32
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
    search(start, VecDeque::from([start]), Some(forced), exclude, at)
}

/// `kinetics.maxChassisRange`: how far ahead an extending piston pushes a line of blocks.
pub const MAX_CHASSIS_RANGE: i32 = 16;

/// `PistonContraption.assemble` (`searchMovedStructure` from the anchor in front of the head,
/// forced along `facing` while it extends, none while it retracts) with its
/// `addToInitialFrontier`: extending, the line of blocks ahead (up to `maxChassisDistance`)
/// until air, a brittle block or a face that pushes nothing; retracting, a sticky piston's
/// block in front of its head alone, and a plain piston's nothing. Offsets from `start`.
pub fn gather_piston(
    start: [i32; 3],
    facing: Dir,
    retracting: bool,
    sticky: bool,
    exclude: &[[i32; 3]],
    at: impl Fn([i32; 3]) -> Option<Found>,
) -> Result<Vec<([i32; 3], u32)>, Refused> {
    let mut frontier = VecDeque::new();
    if retracting && !sticky {
        return Ok(Vec::new());
    }
    for offset in 0..=MAX_CHASSIS_RANGE {
        if offset == 1 && retracting {
            break;
        }
        let p = add(start, facing, offset);
        let Some(f) = at(p) else {
            break;
        };
        // `isBrittle` (but carpet): every face pushes nothing.
        if f.unsupportive == 0x3F {
            break;
        }
        if !f.movable {
            if retracting {
                break;
            }
            return Err(Refused::Unmovable(p));
        }
        frontier.push_back(p);
        if crate::contraption::not_supportive(&f, facing) {
            break;
        }
    }
    search(
        start,
        frontier,
        (!retracting).then_some(facing),
        exclude,
        at,
    )
}

/// `Contraption.searchMovedStructure` from `frontier`, pushing ahead in `forced`.
fn search(
    start: [i32; 3],
    mut frontier: VecDeque<[i32; 3]>,
    forced: Option<Dir>,
    exclude: &[[i32; 3]],
    at: impl Fn([i32; 3]) -> Option<Found>,
) -> Result<Vec<([i32; 3], u32)>, Refused> {
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
            for p in crate::contraption::chassis_reach(pos, c, forced, &at) {
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
        for p in crate::contraption::belt_neighbours(pos, &found) {
            if !visited.contains(&p) && !exclude.contains(&p) {
                frontier.push_back(p);
            }
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
                if Some(d) == forced {
                    return Err(Refused::Unmovable(next));
                }
                continue;
            }
            // `Contraption.moveBlock`: what sticks, what hangs on to this block, glue, and the
            // block pushed in front unless this face pushes nothing.
            use crate::contraption::{attached_towards, glued_toward, not_supportive, sticks};
            if sticks(&found, &other)
                || attached_towards(&other, d.opposite())
                || glued_toward(&found, d)
                || (Some(d) == forced && !not_supportive(&found, d))
            {
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
    fn a_moving_structure_meets_the_cells_its_leading_blocks_enter() {
        // Two blocks in a row along x and one above the first; moving east the second and
        // the one above lead, moving west the first and the one above.
        let blocks = vec![([0, 0, 0], 1), ([1, 0, 0], 1), ([0, 1, 0], 1)];
        let mut east = colliders(&blocks, Dir::East);
        east.sort();
        assert_eq!(east, vec![[0, 1, 0], [1, 0, 0]]);
        let mut west = colliders(&blocks, Dir::West);
        west.sort();
        assert_eq!(west, vec![[0, 0, 0], [0, 1, 0]]);
        // Anchor at x 10, 0.3 out east: going east it enters the cells a block further on.
        let cells = entered_cells([10, 0, 0], Dir::East, 0.3, Dir::East, &[[1, 0, 0]]);
        assert_eq!(cells, vec![([12, 0, 0], [1, 0, 0])]);
        // Going back west from 0.3 it is entering the cells at its floor.
        let cells = entered_cells([10, 0, 0], Dir::East, 0.3, Dir::West, &[[0, 0, 0]]);
        assert_eq!(cells, vec![([10, 0, 0], [0, 0, 0])]);
        assert_eq!(grid_offset(2.4, 4), 2.0);
        assert_eq!(grid_offset(3.6, 3), 3.0);
    }

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

    #[test]
    fn a_plain_piston_pulls_nothing_back_and_a_sticky_one_its_front_block() {
        // Stone in a row at x 1..3 in front of a head facing east; a torch on the first.
        let at = |p: [i32; 3]| match p {
            [1..=3, 0, 0] => Some(Found {
                state: 1,
                movable: true,
                ..Default::default()
            }),
            [1, 1, 0] => Some(Found {
                state: 2,
                movable: true,
                attached: Dir::Down.bit(),
                unsupportive: 0x3F,
                ..Default::default()
            }),
            _ => None,
        };
        let mut out = gather_piston([1, 0, 0], Dir::East, false, false, &[], at).unwrap();
        out.sort();
        assert_eq!(
            out,
            vec![
                ([0, 0, 0], 1),
                ([0, 1, 0], 2),
                ([1, 0, 0], 1),
                ([2, 0, 0], 1)
            ],
            "extending pushes the row and what hangs on it"
        );
        assert!(
            gather_piston([1, 0, 0], Dir::East, true, false, &[], at)
                .unwrap()
                .is_empty()
        );
        let mut back = gather_piston([1, 0, 0], Dir::East, true, true, &[], at).unwrap();
        back.sort();
        assert_eq!(back, vec![([0, 0, 0], 1), ([0, 1, 0], 2)]);
    }
}
