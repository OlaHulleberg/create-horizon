//! Create's mechanical crafter (`MechanicalCrafterBlock`, `MechanicalCrafterBlockEntity`,
//! `RecipeGridHandler`, `MechanicalCraftingInput`, `MechanicalCraftingRecipe`): crafters facing
//! the same way, each pointing at the next, pass their items along their chain as one grid; the
//! last one crafts what the grid makes (a crafting recipe, else a mechanical crafting one) and
//! puts the result into what it points at, or throws everything out.

use crate::kinetics::{Axis, Dir};

/// Catnip's `Pointing`, with its `xRotation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointing {
    Up,
    Left,
    Down,
    Right,
}

impl Pointing {
    pub fn read(name: &str) -> Option<Pointing> {
        Some(match name {
            "up" => Pointing::Up,
            "left" => Pointing::Left,
            "down" => Pointing::Down,
            "right" => Pointing::Right,
            _ => return None,
        })
    }

    /// `GroupedItems.mergeOnto`: where a crafter's grid lands in its target's.
    pub fn merge_offset(self) -> (i32, i32) {
        match self {
            Pointing::Left => (1, 0),
            Pointing::Right => (-1, 0),
            Pointing::Down => (0, 1),
            Pointing::Up => (0, -1),
        }
    }
}

/// `MechanicalCrafterBlock.getTargetDirection`: up turned by `-xRotation` about z (left 270,
/// down 180, right 90), then by `horizontalAngle(facing)` about y.
pub fn target(facing: Dir, pointing: Pointing) -> Dir {
    // (x, y) after the turn about z.
    let (x, y) = match pointing {
        Pointing::Up => (0, 1),
        Pointing::Down => (0, -1),
        Pointing::Left => (-1, 0),
        Pointing::Right => (1, 0),
    };
    if y != 0 {
        return if y > 0 { Dir::Up } else { Dir::Down };
    }
    // `VecHelper.rotate` about y by south 0, west -90, north 180, east -270 (x' = x cos + z sin,
    // z' = -x sin).
    let (s, c): (i32, i32) = match facing {
        Dir::South => (0, 1),
        Dir::West => (-1, 0),
        Dir::North => (0, -1),
        _ => (1, 0),
    };
    let (nx, nz) = (x * c, -x * s);
    match (nx, nz) {
        (1, _) => Dir::East,
        (-1, _) => Dir::West,
        (_, 1) => Dir::South,
        _ => Dir::North,
    }
}

/// `MechanicalCrafterBlockEntity.Phase`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Accepting,
    Assembling,
    Exporting,
    Waiting,
    Crafting,
    Inserting,
}

/// A crafter's typed state: its phase, `countDown`, and the grid it holds (`groupedItems`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Crafter {
    pub phase: Phase,
    pub count_down: i32,
    pub grid: Vec<((i32, i32), String)>,
}

/// `getCountDownSpeed`: the speed held between 4 and 250 (0 when still).
pub fn count_down_speed(rpm: f32) -> i32 {
    if rpm == 0.0 {
        0
    } else {
        (rpm.abs() as i32).clamp(4, 250)
    }
}

/// `MechanicalCraftingInput.of`: the grid cut to its items, row by row from the top.
pub fn input(grid: &[((i32, i32), String)]) -> (usize, usize, Vec<Option<String>>) {
    if grid.is_empty() {
        return (0, 0, Vec::new());
    }
    let min_x = grid.iter().map(|g| g.0.0).min().unwrap_or(0);
    let max_x = grid.iter().map(|g| g.0.0).max().unwrap_or(0);
    let min_y = grid.iter().map(|g| g.0.1).min().unwrap_or(0);
    let max_y = grid.iter().map(|g| g.0.1).max().unwrap_or(0);
    let (w, h) = ((max_x - min_x + 1) as usize, (max_y - min_y + 1) as usize);
    let mut cells = vec![None; w * h];
    for ((x, y), item) in grid {
        let col = (x - min_x) as usize;
        let row = (max_y - y) as usize;
        cells[row * w + col] = Some(item.clone());
    }
    (w, h, cells)
}

/// A `create:mechanical_crafting` recipe: its pattern's cells (each the items it takes), whether
/// it also fits mirrored (`acceptMirrored`), and its result.
#[derive(Clone, Debug, PartialEq)]
pub struct Shaped {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<Option<Vec<String>>>,
    pub mirrored: bool,
    pub result: (String, u8),
}

impl Shaped {
    /// `ShapedRecipe.matches` on the cut grid: the same size, each cell's item taken by the
    /// pattern (mirrored too, when allowed).
    pub fn matches(&self, w: usize, h: usize, cells: &[Option<String>]) -> bool {
        if (w, h) != (self.width, self.height) {
            return false;
        }
        let fits = |mirror: bool| {
            (0..h).all(|r| {
                (0..w).all(|c| {
                    let p = &self.cells[r * w + if mirror { w - 1 - c } else { c }];
                    match (p, &cells[r * w + c]) {
                        (None, None) => true,
                        (Some(items), Some(i)) => items.iter().any(|x| x == i),
                        _ => false,
                    }
                })
            })
        };
        fits(false) || self.mirrored && fits(true)
    }
}

/// The rotation axis of a crafter facing `facing` (its cog's).
pub fn axis(facing: Dir) -> Axis {
    facing.axis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crafters_point_by_their_facing_and_grids_read_from_the_top() {
        // Facing south: left is west, right east; facing north the other way round.
        assert_eq!(target(Dir::South, Pointing::Left), Dir::West);
        assert_eq!(target(Dir::South, Pointing::Right), Dir::East);
        assert_eq!(target(Dir::North, Pointing::Left), Dir::East);
        assert_eq!(target(Dir::East, Pointing::Left), Dir::South);
        assert_eq!(target(Dir::West, Pointing::Down), Dir::Down);
        // A coal above a stick: the top row first.
        let grid = vec![((0, 0), "stick".to_owned()), ((0, 1), "coal".to_owned())];
        let (w, h, cells) = input(&grid);
        assert_eq!((w, h), (1, 2));
        assert_eq!(cells, vec![Some("coal".into()), Some("stick".into())]);
        let torch = Shaped {
            width: 1,
            height: 2,
            cells: vec![Some(vec!["coal".into()]), Some(vec!["stick".into()])],
            mirrored: true,
            result: ("torch".into(), 4),
        };
        assert!(torch.matches(w, h, &cells));
        assert!(!torch.matches(1, 2, &[Some("stick".into()), Some("coal".into())]));
        assert_eq!(
            (
                count_down_speed(16.0),
                count_down_speed(1.0),
                count_down_speed(-999.0)
            ),
            (16, 4, 250)
        );
    }
}
