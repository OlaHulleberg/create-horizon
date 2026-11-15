//! Create's `TreeCutter`: what falls with the block a saw cut. A tree (its logs and roots, the
//! leaves that hang on them, the attachments on those), or the stalk of a vertical plant above
//! the cut (bamboo, cactus, sugar cane, kelp), or the chorus plant above it.

use std::collections::{HashSet, VecDeque};

/// What the cutter sees of a block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    /// `isLog`: the `minecraft:logs` and `create:slimy_logs` tags, the mushroom stem.
    pub log: bool,
    /// `isRoot`: `create:roots`.
    pub root: bool,
    /// `isLeaf`: its `distance` property (not scaffolding's stability distance).
    pub leaf: Option<i32>,
    /// `isVerticalPlant`: bamboo, cactus, sugar cane, kelp.
    pub vertical: bool,
    /// `isChorus`: a chorus plant or flower.
    pub chorus: bool,
    /// `create:tree_attachments`.
    pub attachment: bool,
    /// `nonDecayingLeafDistance`: mushroom blocks, wart blocks and weeping vines (-1 for others).
    pub non_decaying: i32,
}

/// `TreeCutter.Tree`: the blocks to break, in the order they break (attachments, logs, leaves).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub logs: Vec<[i32; 3]>,
    pub leaves: Vec<[i32; 3]>,
    pub attachments: Vec<[i32; 3]>,
}

impl Tree {
    /// Every block with the chance its break shows particles (`destroyBlocks`'s effect chances).
    pub fn blocks(&self) -> impl Iterator<Item = ([i32; 3], f32)> + '_ {
        let a = self.attachments.iter().map(|p| (*p, 1.0 / 32.0));
        let l = self.logs.iter().map(|p| (*p, 0.5));
        let f = self.leaves.iter().map(|p| (*p, 1.0 / 8.0));
        a.chain(l).chain(f)
    }
}

fn add(p: [i32; 3], d: [i32; 3]) -> [i32; 3] {
    [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
}

/// `BlockPos.betweenClosed` from `pos + (-1, min_y, -1)` to `pos + (1, max_y, 1)`, x fastest,
/// then y, then z, as Java walks it.
fn around(pos: [i32; 3], min_y: i32, max_y: i32) -> impl Iterator<Item = [i32; 3]> {
    (-1..=1).flat_map(move |z| {
        (min_y..=max_y).flat_map(move |y| (-1..=1).map(move |x| add(pos, [x, y, z])))
    })
}

const UP: (i32, i32) = (0, 1);
const DOWN: (i32, i32) = (-1, 0);
const BOTH: (i32, i32) = (-1, 1);

/// `TreeCutter.findTree`: what falls when the saw cut `pos` (now air), the block it broke
/// being `broken`. `at` reads a block; `height` is the level's height (the stalk's limit).
pub fn find(pos: [i32; 3], broken: Cell, height: i32, at: impl Fn([i32; 3]) -> Cell) -> Tree {
    let mut tree = Tree::default();
    let above = add(pos, [0, 1, 0]);
    let state_above = at(above);
    // Bamboo, sugar cane, cactus: the stalk above, top first.
    if broken.vertical {
        if !state_above.vertical {
            return tree;
        }
        tree.logs.push(above);
        // Java's loop starts at the block above again; it adds nothing twice here.
        for i in 2..height {
            let current = add(pos, [0, i, 0]);
            if !at(current).vertical {
                break;
            }
            tree.logs.push(current);
        }
        tree.logs.reverse();
        return tree;
    }
    // Chorus: every chorus block joined to the one above, the last found first.
    if broken.chorus {
        if !state_above.chorus {
            return tree;
        }
        let mut visited = HashSet::new();
        let mut frontier = VecDeque::from([above]);
        while let Some(current) = frontier.pop_front() {
            visited.insert(current);
            tree.logs.push(current);
            for d in [
                [0, -1, 0],
                [0, 1, 0],
                [0, 0, -1],
                [0, 0, 1],
                [-1, 0, 0],
                [1, 0, 0],
            ] {
                let offset = add(current, d);
                if visited.contains(&offset) || !at(offset).chorus {
                    continue;
                }
                frontier.push_back(offset);
            }
        }
        tree.logs.reverse();
        return tree;
    }
    if !validate_cut(pos, &at) {
        return tree;
    }
    let mut visited: HashSet<[i32; 3]> = HashSet::from([pos]);
    let mut frontier: VecDeque<[i32; 3]> = around(pos, 0, 1).collect();
    // The logs and roots, upward.
    let mut has_roots = false;
    while let Some(current) = frontier.pop_front() {
        if !visited.insert(current) {
            continue;
        }
        let c = at(current);
        if c.root {
            has_roots = true;
        } else if !c.log {
            continue;
        }
        tree.logs.push(current);
        frontier.extend(around(current, UP.0, UP.1).filter(|p| !visited.contains(p)));
    }
    visited = tree.logs.iter().copied().collect();
    frontier = tree.logs.iter().copied().collect();
    if has_roots {
        let old: HashSet<[i32; 3]> = tree.logs.iter().copied().collect();
        while let Some(current) = frontier.pop_front() {
            if !at(current).root {
                continue;
            }
            if !old.contains(&current) {
                tree.logs.push(current);
            }
            for p in around(current, DOWN.0, DOWN.1) {
                if visited.insert(p) {
                    frontier.push_back(p);
                }
            }
        }
        visited = tree.logs.iter().copied().collect();
        frontier = tree.logs.iter().copied().collect();
    }
    // The leaves: those nearer the trunk than their kind decays from, and leaves farther from a
    // log than the one before them.
    while let Some(prev) = frontier.pop_front() {
        let p = at(prev);
        let prev_distance = p.leaf.unwrap_or(0);
        let next: Vec<[i32; 3]> = around(prev, BOTH.0, BOTH.1)
            .filter(|n| !visited.contains(n))
            .collect();
        for current in next {
            let c = at(current);
            if c.attachment {
                tree.attachments.push(current);
                visited.insert(current);
                continue;
            }
            let horizontal = (current[0] - pos[0]).abs().max((current[2] - pos[2]).abs());
            if horizontal <= c.non_decaying && visited.insert(current) {
                tree.leaves.push(current);
                frontier.push_back(current);
                continue;
            }
            if c.leaf.is_some_and(|d| d > prev_distance) && visited.insert(current) {
                tree.leaves.push(current);
                frontier.push_back(current);
            }
        }
    }
    tree
}

/// `TreeCutter.validateCut`: the cut went through the whole trunk, nothing above the cut layer
/// stands on another log or root.
fn validate_cut(pos: [i32; 3], at: &impl Fn([i32; 3]) -> Cell) -> bool {
    let mut visited = HashSet::new();
    let mut frontier = VecDeque::from([pos, add(pos, [0, 1, 0])]);
    while let Some(current) = frontier.pop_front() {
        let below = add(current, [0, -1, 0]);
        visited.insert(current);
        let lower = current[1] == pos[1];
        let c = at(current);
        if !c.log && !c.root {
            continue;
        }
        let b = at(below);
        if !lower && below != pos && (b.log || b.root) {
            return false;
        }
        // Every direction but down; up only from the cut layer (Iterate.directions order).
        for d in [[0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]] {
            if d == [0, 1, 0] && !lower {
                continue;
            }
            let offset = add(current, d);
            if visited.contains(&offset) {
                continue;
            }
            frontier.push_back(offset);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn log() -> Cell {
        Cell {
            log: true,
            non_decaying: -1,
            ..Cell::default()
        }
    }

    fn leaf(d: i32) -> Cell {
        Cell {
            leaf: Some(d),
            non_decaying: -1,
            ..Cell::default()
        }
    }

    fn world(cells: &[([i32; 3], Cell)]) -> impl Fn([i32; 3]) -> Cell {
        let map: HashMap<[i32; 3], Cell> = cells.iter().copied().collect();
        move |p| {
            map.get(&p).copied().unwrap_or(Cell {
                non_decaying: -1,
                ..Cell::default()
            })
        }
    }

    #[test]
    fn a_cut_trunk_takes_its_logs_and_leaves() {
        // Logs at y 1..=4 over the cut at y 0; leaves around the top, one detached far away.
        let mut cells = vec![];
        for y in 1..=4 {
            cells.push(([0, y, 0], log()));
        }
        cells.push(([1, 4, 0], leaf(1)));
        cells.push(([2, 4, 0], leaf(2)));
        cells.push(([3, 4, 0], leaf(1)));
        cells.push(([0, 5, 0], leaf(1)));
        let tree = find([0, 0, 0], log(), 384, world(&cells));
        assert_eq!(tree.logs.len(), 4);
        let mut leaves = tree.leaves.clone();
        leaves.sort();
        assert_eq!(
            leaves,
            [[0, 5, 0], [1, 4, 0], [2, 4, 0]],
            "a leaf nearer another log stays"
        );
    }

    #[test]
    fn a_trunk_still_standing_on_another_log_does_not_fall() {
        // Two trunks joined above the cut: the other one holds it.
        let mut cells = vec![([1, 0, 0], log())];
        for y in 1..=3 {
            cells.push(([0, y, 0], log()));
            cells.push(([1, y, 0], log()));
        }
        let tree = find([0, 0, 0], log(), 384, world(&cells));
        assert_eq!(tree, Tree::default());
    }

    #[test]
    fn a_cut_cane_drops_its_stalk_top_first() {
        let cane = Cell {
            vertical: true,
            non_decaying: -1,
            ..Cell::default()
        };
        let cells = [([0, 1, 0], cane), ([0, 2, 0], cane)];
        let tree = find([0, 0, 0], cane, 384, world(&cells));
        assert_eq!(tree.logs, [[0, 2, 0], [0, 1, 0]]);
    }
}
