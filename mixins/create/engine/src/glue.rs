//! Create's super glue (`SuperGlueEntity`, `SuperGlueSelectionHelper`, `SuperGlueSelectionPacket`)
//! as plain data: a glue area is a box of whole blocks; two blocks are glued along a face when
//! one area holds both, and a contraption's structure search then takes the other block too.
//! Also the sticker (`StickerBlock`): extended, it holds on to the block it faces.

use crate::kinetics::Dir;
use std::collections::HashSet;

/// A glue area (`SuperGlueEntity`'s bounding box): the blocks `min..=max` on every axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Glue {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

impl Glue {
    /// `SuperGlueEntity.span`: the box from one block to another, both included.
    pub fn span(a: [i32; 3], b: [i32; 3]) -> Glue {
        Glue {
            min: [0, 1, 2].map(|i| a[i].min(b[i])),
            max: [0, 1, 2].map(|i| a[i].max(b[i])),
        }
    }

    /// `contains`: the block's centre is inside the box.
    pub fn contains(&self, p: [i32; 3]) -> bool {
        (0..3).all(|i| self.min[i] <= p[i] && p[i] <= self.max[i])
    }

    /// `AABB.getSize()` (the mean edge) above 1.01: more than one block.
    pub fn more_than_one(&self) -> bool {
        let edges: i32 = (0..3).map(|i| self.max[i] - self.min[i] + 1).sum();
        f64::from(edges) / 3.0 > 1.01
    }

    /// The column whose chunk holds the entity (`resetPositionToBB`: the box's centre in x and
    /// z, its floor in y), which saves and loads it.
    pub fn column(&self) -> (i32, i32) {
        let centre = |i: usize| (f64::from(self.min[i]) + f64::from(self.max[i]) + 1.0) / 2.0;
        (
            (centre(0).floor() as i32) >> 4,
            (centre(2).floor() as i32) >> 4,
        )
    }

    /// The box moved by whole blocks.
    pub fn moved(&self, by: [i32; 3]) -> Glue {
        Glue {
            min: add(self.min, by),
            max: add(self.max, by),
        }
    }
}

/// `SuperGlueEntity.isGlued`: an area holds both the block and its neighbour toward `d`.
pub fn is_glued(areas: &[Glue], pos: [i32; 3], d: Dir) -> bool {
    let next = add(pos, d.offset());
    areas.iter().any(|g| g.contains(pos) && g.contains(next))
}

/// The faces of `pos` some area glues (`Dir::bit` each), for the structure search.
pub fn glued_faces(areas: &[Glue], pos: [i32; 3]) -> u8 {
    let mut faces = 0;
    for g in areas.iter().filter(|g| g.contains(pos)) {
        for d in Dir::ALL {
            if g.contains(add(pos, d.offset())) {
                faces |= d.bit();
            }
        }
    }
    faces
}

/// Why glue could not be put down (`SuperGlueSelectionPacket.handle` returns early).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// `to.closerThan(from, 25)` fails.
    TooFar,
    /// `searchGlueGroup` found nothing that holds together, or not the second corner.
    NotConnected,
}

/// `SuperGlueSelectionHelper.searchGlueGroup(level, from, to, false)`: the blocks glue spanning
/// `from` to `to` holds together, spreading past the box only to neighbours whose side toward
/// it is sticky already (`isSideSticky`). `sticky(p)` reads a block: None when it needs no moving
/// (air, a fluid), else its sticky sides (`Dir::bit` each).
pub fn search_group(
    from: [i32; 3],
    to: [i32; 3],
    sticky: impl Fn([i32; 3]) -> Option<u8>,
) -> Option<HashSet<[i32; 3]>> {
    let area = Glue::span(from, to);
    let mut frontier = vec![from];
    let mut visited = HashSet::from([from]);
    let mut attached = HashSet::new();
    while !frontier.is_empty() {
        let current = frontier.remove(0);
        attached.insert(current);
        // `isValidFace` at the current block: it must need moving (the start may not).
        if sticky(current).is_none() {
            continue;
        }
        for d in Dir::ALL {
            let next = add(current, d.offset());
            let Some(next_sticky) = sticky(next) else {
                continue;
            };
            let already = next_sticky & d.opposite().bit() != 0;
            if !already && !area.contains(next) {
                continue;
            }
            if visited.insert(next) {
                frontier.push(next);
            }
        }
    }
    if attached.len() < 2 && attached.contains(&to) {
        return None;
    }
    Some(attached)
}

/// `SuperGlueSelectionPacket.handle` (the player's reach and the item's charges aside): the area
/// from `from` to `to`, if they are under 25 blocks apart and the group holds the second corner.
pub fn place(
    from: [i32; 3],
    to: [i32; 3],
    sticky: impl Fn([i32; 3]) -> Option<u8>,
) -> Result<Glue, Refused> {
    // `Vec3i.closerThan`: squared distance below 25².
    let d2: i64 = (0..3).map(|i| i64::from(to[i] - from[i]).pow(2)).sum();
    if d2 >= 625 {
        return Err(Refused::TooFar);
    }
    match search_group(from, to, sticky) {
        Some(group) if group.contains(&to) => Ok(Glue::span(from, to)),
        _ => Err(Refused::NotConnected),
    }
}

/// What a contraption takes of the glue (`glueToRemove`, `removeBlocksFromWorld`): the areas
/// that glued one of its blocks to a neighbour the search could take (`skip` names the
/// anchoring blocks), and each shrunk to the blocks it holds of the contraption, kept when it
/// still holds more than one (offsets as `blocks`). The others stay in the world.
pub fn carried(
    areas: &[Glue],
    blocks: &[[i32; 3]],
    skip: impl Fn([i32; 3]) -> bool,
) -> (Vec<usize>, Vec<Glue>) {
    let mut used = Vec::new();
    let mut shrunk = Vec::new();
    for (i, g) in areas.iter().enumerate() {
        let glues = blocks.iter().any(|&p| {
            g.contains(p)
                && Dir::ALL.into_iter().any(|d| {
                    let n = add(p, d.offset());
                    !skip(n) && g.contains(n)
                })
        });
        if !glues {
            continue;
        }
        used.push(i);
        let mut bounds: Option<Glue> = None;
        for &p in blocks.iter().filter(|&&p| g.contains(p)) {
            bounds = Some(match bounds {
                None => Glue { min: p, max: p },
                Some(b) => Glue {
                    min: [0, 1, 2].map(|a| b.min[a].min(p[a])),
                    max: [0, 1, 2].map(|a| b.max[a].max(p[a])),
                },
            });
        }
        if let Some(b) = bounds.filter(Glue::more_than_one) {
            shrunk.push(b);
        }
    }
    (used, shrunk)
}

/// `StickerBlock.neighborChanged`: a change of power flips `powered`; turned on, it also flips
/// `extended`. The new (powered, extended), or None when nothing changes.
pub fn sticker_neighbor(powered: bool, extended: bool, signal: bool) -> Option<(bool, bool)> {
    if powered == signal {
        return None;
    }
    Some((signal, if signal { !extended } else { extended }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glue_holds_faces_inside_its_area() {
        let g = Glue::span([2, 0, 0], [0, 1, 0]);
        assert_eq!((g.min, g.max), ([0, 0, 0], [2, 1, 0]));
        assert!(g.contains([1, 1, 0]) && !g.contains([3, 0, 0]));
        assert!(is_glued(&[g], [0, 0, 0], Dir::East));
        assert!(!is_glued(&[g], [2, 0, 0], Dir::East));
        assert_eq!(
            glued_faces(&[g], [0, 0, 0]),
            Dir::East.bit() | Dir::Up.bit()
        );
        assert!(!Glue::span([0, 0, 0], [0, 0, 0]).more_than_one());
        assert!(Glue::span([0, 0, 0], [1, 0, 0]).more_than_one());
        assert_eq!(Glue::span([-3, 0, 0], [-1, 0, 0]).column(), (-1, 0));
    }

    #[test]
    fn the_glue_group_needs_both_corners_held_together() {
        // Stone at x 0..=2 except a gap at 1: from 0 to 2 the group is just the first block.
        let gap = |p: [i32; 3]| (p[1] == 0 && p[2] == 0 && (p[0] == 0 || p[0] == 2)).then_some(0);
        assert_eq!(place([0, 0, 0], [2, 0, 0], gap), Err(Refused::NotConnected));
        let row = |p: [i32; 3]| (p[1] == 0 && p[2] == 0 && (0..=2).contains(&p[0])).then_some(0);
        assert_eq!(
            place([0, 0, 0], [2, 0, 0], row),
            Ok(Glue::span([0, 0, 0], [2, 0, 0]))
        );
        assert_eq!(place([0, 0, 0], [30, 0, 0], row), Err(Refused::TooFar));
        // A slime block beside the box joins the group by its sticky side.
        let slime = |p: [i32; 3]| match p {
            [0..=2, 0, 0] => Some(0),
            [0, 1, 0] => Some(0x3f),
            _ => None,
        };
        let group = search_group([0, 0, 0], [2, 0, 0], slime).unwrap();
        assert!(group.contains(&[0, 1, 0]) && group.len() == 4);
    }

    #[test]
    fn a_contraption_takes_the_glue_it_used_shrunk_to_its_blocks() {
        let areas = [
            Glue::span([0, 0, 0], [5, 0, 0]),
            Glue::span([10, 0, 0], [11, 0, 0]),
        ];
        let (used, shrunk) = carried(&areas, &[[0, 0, 0], [1, 0, 0]], |_| false);
        assert_eq!(used, vec![0]);
        assert_eq!(shrunk, vec![Glue::span([0, 0, 0], [1, 0, 0])]);
        // One block of it only: used, but nothing left to carry.
        let (used, shrunk) = carried(&areas, &[[5, 0, 0]], |_| false);
        assert_eq!((used, shrunk), (vec![0], vec![]));
    }

    #[test]
    fn a_sticker_extends_on_each_rising_signal() {
        assert_eq!(sticker_neighbor(false, false, true), Some((true, true)));
        assert_eq!(sticker_neighbor(true, true, false), Some((false, true)));
        assert_eq!(sticker_neighbor(false, true, true), Some((true, false)));
        assert_eq!(sticker_neighbor(true, true, true), None);
    }
}
