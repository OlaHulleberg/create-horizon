//! Create's block shapes as colliders (`Level.clip` with `ClipContext.Block.COLLIDER`): the
//! boxes the ejector's obstacle scan flies through, ported from `AllShapes` and the blocks'
//! own `getShape` (no block of Create's overrides `getCollisionShape` but the belt, whose
//! collision shape for an empty `CollisionContext` is its shape). Boxes are in block units;
//! they may reach past the block they belong to, as Create's shapes do.

use crate::Block;
use crate::kinetics::{Axis, Dir};
use crate::parts::StateInfo;

/// A box in block units.
type Box = [f64; 6];

const fn px(x0: f64, y0: f64, z0: f64, x1: f64, y1: f64, z1: f64) -> Box {
    [
        x0 / 16.0,
        y0 / 16.0,
        z0 / 16.0,
        x1 / 16.0,
        y1 / 16.0,
        z1 / 16.0,
    ]
}

/// `VecHelper.rotate(vec, deg, Axis.Y)` about the block's centre, by quarter turns
/// clockwise seen from above (one turn takes +z, south, to +x, east). Ranges stay sorted.
fn turned_y(b: Box, quarter: u8) -> Box {
    match quarter % 4 {
        0 => b,
        // (x, z) → (z, −x).
        1 => [b[2], b[1], 1.0 - b[3], b[5], b[4], 1.0 - b[0]],
        // (x, z) → (−x, −z).
        2 => [1.0 - b[3], b[1], 1.0 - b[5], 1.0 - b[0], b[4], 1.0 - b[2]],
        // (x, z) → (−z, x).
        _ => [1.0 - b[5], b[1], b[0], 1.0 - b[2], b[4], b[3]],
    }
}

/// `VecHelper.rotate(vec, ±90, Axis.X)` about the block's centre, as `DefaultRotationValues`
/// turns a shape from up to south (+z) or north. Ranges stay sorted.
fn turned_x(b: Box, up_to_south: bool) -> Box {
    if up_to_south {
        // (y, z) → (−z, y).
        [b[0], 1.0 - b[5], b[1], b[3], 1.0 - b[2], b[4]]
    } else {
        // (y, z) → (z, −y).
        [b[0], b[2], 1.0 - b[4], b[3], b[5], 1.0 - b[1]]
    }
}

fn min_max(b: &mut Box) {
    for a in 0..3 {
        if b[a] > b[a + 3] {
            (b[a], b[a + 3]) = (b[a + 3], b[a]);
        }
    }
}

/// The collider boxes of a Create block state, or None when this does not know them (the
/// caller falls back to what its own world knows about the block).
pub fn colliders(block: Block, info: &StateInfo) -> Option<Vec<Box>> {
    match block {
        // `DepotBlock.getShape` and `EjectorBlock.getShape`: `AllShapes.CASING_13PX` up.
        Block::Depot | Block::WeightedEjector => Some(vec![px(0.0, 0.0, 0.0, 16.0, 13.0, 16.0)]),
        // `ShaftBlock.getShape`: `AllShapes.SIX_VOXEL_POLE` on its axis.
        Block::Shaft => Some(vec![match info.axis? {
            Axis::X => px(0.0, 5.0, 5.0, 16.0, 11.0, 11.0),
            Axis::Y => px(5.0, 0.0, 5.0, 11.0, 16.0, 11.0),
            Axis::Z => px(5.0, 5.0, 0.0, 11.0, 11.0, 16.0),
        }]),
        // `CogWheelBlock.getShape`: `AllShapes.SMALL_GEAR` / `LARGE_GEAR` — the gear plate
        // (`SMALL_GEAR_SHAPE` cuboid(2, 6, 2, 14, 10, 14), `LARGE_GEAR_SHAPE` cuboid(0, 6, 0,
        // 16, 10, 16)) added to `SIX_VOXEL_POLE` on the axis (`VoxelShaper.forAxis`).
        Block::Cogwheel | Block::LargeCogwheel => {
            let axis = info.axis?;
            let (x0, x1) = if block == Block::LargeCogwheel {
                (0.0, 16.0)
            } else {
                (2.0, 14.0)
            };
            // On the y axis: the plate across x/z, the pole up the middle.
            let plate = px(x0, 6.0, x0, x1, 10.0, x1);
            let pole = px(5.0, 0.0, 5.0, 11.0, 16.0, 11.0);
            let turn = |mut b: Box| {
                b = match axis {
                    Axis::Y => b,
                    // Up to south (DefaultRotationValues), then south to east for the x axis.
                    Axis::Z => turned_x(b, true),
                    Axis::X => turned_y(turned_x(b, true), 1),
                };
                min_max(&mut b);
                b
            };
            Some(vec![turn(pole), turn(plate)])
        }
        // `CreativeMotorBlock.getShape`: `AllShapes.MOTOR_BLOCK` on its facing.
        Block::CreativeMotor => {
            let facing = info.facing?;
            let b = px(3.0, 0.0, 3.0, 13.0, 14.0, 13.0);
            let mut b = match facing {
                Dir::Up => b,
                Dir::Down => turned_x(turned_x(b, true), true),
                // forDirectional turns up to south (+90 about x), then about y.
                d => turned_y(turned_x(b, true), quarter_to(d)),
            };
            min_max(&mut b);
            Some(vec![b])
        }
        // `HandCrankBlock.getShape`: `AllShapes.CRANK` on its facing.
        Block::HandCrank => {
            let facing = info.facing?;
            let b = [
                px(5.0, 0.0, 5.0, 11.0, 6.0, 11.0),
                px(1.0, 3.0, 1.0, 15.0, 8.0, 15.0),
            ];
            let b: Vec<Box> = b
                .into_iter()
                .map(|mut b| {
                    b = match facing {
                        Dir::Up => b,
                        Dir::Down => turned_x(turned_x(b, true), true),
                        d => turned_y(turned_x(b, true), quarter_to(d)),
                    };
                    min_max(&mut b);
                    b
                })
                .collect();
            Some(b)
        }
        // `BeltBlock.getCollisionShape` with an empty `CollisionContext` is `BeltShapes.getShape`.
        Block::Belt => belt(info),
        _ => None,
    }
}

/// Quarter turns about y that take south (the canonical facing of `VoxelShaper.forHorizontal`)
/// to `d`.
fn quarter_to(d: Dir) -> u8 {
    match d {
        Dir::South => 0,
        Dir::East => 1,
        Dir::North => 2,
        _ => 3,
    }
}

/// `BeltShapes.getBeltShape` (the casing shape of an uncased belt is empty; the driver's
/// worlds hold none other).
fn belt(info: &StateInfo) -> Option<Vec<Box>> {
    let seg = info.belt?;
    // The building parts, as `makeFlatFull` and friends define them (south-facing).
    let flat_full = || px(1.0, 3.0, 0.0, 15.0, 13.0, 16.0);
    let flat_end = || {
        vec![
            px(1.0, 4.0, 0.0, 15.0, 12.0, 16.0),
            px(1.0, 3.0, 1.0, 15.0, 13.0, 15.0),
        ]
    };
    let side_full = || px(3.0, 1.0, 0.0, 13.0, 15.0, 16.0);
    let side_end = || {
        vec![
            px(4.0, 1.0, 0.0, 12.0, 15.0, 16.0),
            px(3.0, 1.0, 1.0, 13.0, 15.0, 15.0),
        ]
    };
    // `makeSlopePart`: sixteen one-pixel-deep slices, each a pixel lower (descending).
    let slope = |ascending: bool| {
        (0..16)
            .map(|i| {
                let y = if ascending {
                    10.0 - i as f64
                } else {
                    i as f64 - 5.0
                };
                px(1.0, y, 15.0 - i as f64, 15.0, y + 11.0, 16.0 - i as f64)
            })
            .collect::<Vec<Box>>()
    };
    // `compose(south, north)`: each part clipped to its half of the block (the masks span it).
    let composed = |south: Vec<Box>, north: Vec<Box>| -> Vec<Box> {
        south
            .into_iter()
            .map(|mut b| {
                b[2] = b[2].max(8.0 / 16.0);
                b
            })
            .chain(north.into_iter().map(|mut b| {
                b[5] = b[5].min(8.0 / 16.0);
                b
            }))
            .filter(|b| b[2] < b[5])
            .collect()
    };
    use crate::belt::{Part, Slope};
    let (slope_v, part_v) = (seg.slope, seg.part);
    let full = part_v == Part::Middle || part_v == Part::Pulley;
    let boxes: Vec<Box> = match (slope_v, full) {
        (Slope::Horizontal, true) => vec![flat_full()],
        (Slope::Horizontal, false) => {
            // START keeps the north (pulley) end, END the south one.
            if part_v == Part::Start {
                composed(vec![flat_full()], flat_end())
            } else {
                composed(flat_end(), vec![flat_full()])
            }
        }
        (Slope::Sideways, true) => vec![side_full()],
        (Slope::Sideways, false) => {
            if part_v == Part::Start {
                composed(vec![side_full()], side_end())
            } else {
                composed(side_end(), vec![side_full()])
            }
        }
        (Slope::Downward, true) => slope(false),
        (Slope::Downward, false) => {
            if part_v == Part::Start {
                composed(slope(false), flat_end())
            } else {
                composed(flat_end(), slope(false))
            }
        }
        (Slope::Upward, true) => slope(true),
        (Slope::Upward, false) => {
            if part_v == Part::Start {
                composed(slope(true), flat_end())
            } else {
                composed(flat_end(), slope(true))
            }
        }
        // Vertical belts: the flat part turned on its side (`VerticalBeltShaper`).
        (Slope::Vertical, _) => {
            let mut b = turned_x(flat_full(), true);
            min_max(&mut b);
            vec![b]
        }
    };
    Some(
        boxes
            .into_iter()
            .map(|b| turned_y(b, quarter_to(seg.facing)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info_of(props: &[(&str, &str)]) -> StateInfo {
        StateInfo::read(props.iter().copied())
    }

    #[test]
    fn a_depot_collides_thirteen_pixels_up() {
        // e4-ejector-clip-whole-blocks: `AllShapes.CASING_13PX` — the throw meets the depot's
        // top at 13/16 (y 8.8125 for a depot at y 8), not the block face at y 9.
        let b = colliders(Block::Depot, &info_of(&[])).unwrap();
        assert_eq!(b, vec![[0.0, 0.0, 0.0, 1.0, 13.0 / 16.0, 1.0]]);
        assert_eq!(
            colliders(Block::WeightedEjector, &info_of(&[("facing", "east")])).unwrap(),
            b
        );
    }

    #[test]
    fn shafts_and_cogs_collide_as_poles_with_gear_plates() {
        let shaft = colliders(Block::Shaft, &info_of(&[("axis", "x")])).unwrap();
        assert_eq!(
            shaft,
            vec![[0.0, 5.0 / 16.0, 5.0 / 16.0, 1.0, 11.0 / 16.0, 11.0 / 16.0]]
        );
        // SMALL_GEAR_SHAPE cuboid(2, 6, 2, 14, 10, 14) on the y pole; the large one spans the
        // block (LARGE_GEAR_SHAPE cuboid(0, 6, 0, 16, 10, 16)).
        let cog = colliders(Block::Cogwheel, &info_of(&[("axis", "y")])).unwrap();
        assert!(cog.contains(&[
            2.0 / 16.0,
            6.0 / 16.0,
            2.0 / 16.0,
            14.0 / 16.0,
            10.0 / 16.0,
            14.0 / 16.0
        ]));
        let large = colliders(Block::LargeCogwheel, &info_of(&[("axis", "y")])).unwrap();
        assert!(large.contains(&[0.0, 6.0 / 16.0, 0.0, 1.0, 10.0 / 16.0, 1.0]));
        // An x-axis cog: the plate turned, its four pixels of thickness along x.
        let cog = colliders(Block::Cogwheel, &info_of(&[("axis", "x")])).unwrap();
        assert!(cog.contains(&[
            6.0 / 16.0,
            2.0 / 16.0,
            2.0 / 16.0,
            10.0 / 16.0,
            14.0 / 16.0,
            14.0 / 16.0
        ]));
    }

    #[test]
    fn a_south_facing_motor_is_open_on_its_facing_side() {
        // `AllShapes.MOTOR_BLOCK` cuboid(3, 0, 3, 13, 14, 13), turned from up to south: the
        // closed back sits on the north side.
        let b = colliders(Block::CreativeMotor, &info_of(&[("facing", "south")])).unwrap();
        assert_eq!(
            b,
            vec![[
                3.0 / 16.0,
                3.0 / 16.0,
                0.0,
                13.0 / 16.0,
                13.0 / 16.0,
                14.0 / 16.0
            ]]
        );
        let up = colliders(Block::CreativeMotor, &info_of(&[("facing", "up")])).unwrap();
        assert_eq!(
            up,
            vec![[
                3.0 / 16.0,
                0.0,
                3.0 / 16.0,
                13.0 / 16.0,
                14.0 / 16.0,
                13.0 / 16.0
            ]]
        );
    }

    #[test]
    fn a_flat_belt_carries_its_plate_between_one_and_fifteen() {
        // `makeFlatFull`: cuboid(1, 3, 0, 15, 13, 16), along z as placed; `forHorizontalAxis`
        // turns it for x.
        let along_z = colliders(
            Block::Belt,
            &info_of(&[
                ("facing", "south"),
                ("slope", "horizontal"),
                ("part", "middle"),
            ]),
        )
        .unwrap();
        assert_eq!(
            along_z,
            vec![[1.0 / 16.0, 3.0 / 16.0, 0.0, 15.0 / 16.0, 13.0 / 16.0, 1.0]]
        );
        let along_x = colliders(
            Block::Belt,
            &info_of(&[
                ("facing", "east"),
                ("slope", "horizontal"),
                ("part", "middle"),
            ]),
        )
        .unwrap();
        assert_eq!(
            along_x,
            vec![[0.0, 3.0 / 16.0, 1.0 / 16.0, 1.0, 13.0 / 16.0, 15.0 / 16.0]]
        );
        // A pulley end (`makeFlatEnding`) keeps its rim on one half: facing south, the START's
        // pulley is the north half, thinned to 4..12 with the rim at 3..13 around it.
        let start = colliders(
            Block::Belt,
            &info_of(&[
                ("facing", "south"),
                ("slope", "horizontal"),
                ("part", "start"),
            ]),
        )
        .unwrap();
        assert_eq!(
            start,
            vec![
                // The full south half.
                [
                    1.0 / 16.0,
                    3.0 / 16.0,
                    8.0 / 16.0,
                    15.0 / 16.0,
                    13.0 / 16.0,
                    1.0
                ],
                // The pulley's thin north half with its rim.
                [
                    1.0 / 16.0,
                    4.0 / 16.0,
                    0.0,
                    15.0 / 16.0,
                    12.0 / 16.0,
                    8.0 / 16.0
                ],
                [
                    1.0 / 16.0,
                    3.0 / 16.0,
                    1.0 / 16.0,
                    15.0 / 16.0,
                    13.0 / 16.0,
                    8.0 / 16.0
                ],
            ]
        );
    }
}
