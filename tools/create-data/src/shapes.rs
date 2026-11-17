//! Create's voxel shapes (`AllShapes.java`), as the content mod's `shapes` lists: boxes in block
//! units, rotated from the orientation Create writes them in, as its `VoxelShaper` does.

use serde_json::{Value, json};

/// A box in Create's pixels: `[x0, y0, z0, x1, y1, z1]`.
pub type Px = [f64; 6];

/// `AllShapes.SIX_VOXEL_POLE`, for the Y axis.
pub const SIX_VOXEL_POLE: Px = [5., 0., 5., 11., 16., 11.];
/// `AllShapes.SMALL_GEAR_SHAPE` (with the pole).
pub const SMALL_GEAR: Px = [2., 6., 2., 14., 10., 14.];
/// `AllShapes.LARGE_GEAR_SHAPE` (with the pole).
pub const LARGE_GEAR: Px = [0., 6., 0., 16., 10., 16.];
/// `AllShapes.MOTOR_BLOCK`, facing up.
pub const MOTOR_BLOCK: Px = [3., 0., 3., 13., 14., 13.];
/// `AllShapes.GAUGE_SHAPE_UP`: the plate and the dial box on it.
pub const GAUGE: [Px; 2] = [[1., 0., 0., 15., 2., 16.], [2., 2., 1., 14., 14., 15.]];
/// `AllShapes.CRANK`, facing up.
pub const CRANK: [Px; 2] = [[5., 0., 5., 11., 6., 11.], [1., 3., 1., 15., 8., 15.]];

/// A Y-axis box turned to `axis` (`VoxelShaper.forAxis`).
fn to_axis(b: Px, axis: &str) -> Px {
    match axis {
        "x" => [b[1], b[0], b[2], b[4], b[3], b[5]],
        "z" => [b[0], b[2], b[1], b[3], b[5], b[4]],
        _ => b,
    }
}

/// An up-facing box turned to `facing` (`VoxelShaper.forDirectional`), about the block's centre.
fn to_facing(b: Px, facing: &str) -> Px {
    let turn = |p: [f64; 3]| -> [f64; 3] {
        let [x, y, z] = [p[0] - 8., p[1] - 8., p[2] - 8.];
        let r = match facing {
            "down" => [x, -y, -z],
            "north" => [x, z, -y],
            "south" => [x, -z, y],
            "east" => [y, -x, z],
            "west" => [-y, x, z],
            _ => [x, y, z],
        };
        [r[0] + 8., r[1] + 8., r[2] + 8.]
    };
    let (a, c) = (turn([b[0], b[1], b[2]]), turn([b[3], b[4], b[5]]));
    [
        a[0].min(c[0]),
        a[1].min(c[1]),
        a[2].min(c[2]),
        a[0].max(c[0]),
        a[1].max(c[1]),
        a[2].max(c[2]),
    ]
}

fn units(b: Px) -> Value {
    json!(b.map(|v| v / 16.0))
}

/// A constant shape for a block whose geometry has no state selector.
pub fn fixed(boxes: &[Px]) -> Value {
    json!({ "default": boxes.iter().map(|b| units(*b)).collect::<Vec<_>>() })
}

/// A `shapes` object selecting by `axis`.
pub fn by_axis(boxes: &[Px]) -> Value {
    let values: serde_json::Map<String, Value> = ["x", "y", "z"]
        .iter()
        .map(|a| {
            (
                a.to_string(),
                json!(
                    boxes
                        .iter()
                        .map(|b| units(to_axis(*b, a)))
                        .collect::<Vec<_>>()
                ),
            )
        })
        .collect();
    json!({ "property": "axis", "values": values })
}

/// A `shapes` object giving every value of `property` the same boxes.
pub fn by_property(property: &str, values: &[&str], boxes: &[Px]) -> Value {
    let list: Vec<Value> = boxes.iter().map(|b| units(*b)).collect();
    let values: serde_json::Map<String, Value> = values
        .iter()
        .map(|v| (v.to_string(), json!(list)))
        .collect();
    json!({ "property": property, "values": values })
}

/// A `shapes` object giving each value of `property` its own boxes.
pub fn by_property_values(property: &str, values: &[(&str, &[Px])]) -> Value {
    let values: serde_json::Map<String, Value> = values
        .iter()
        .map(|(v, boxes)| {
            let list: Vec<Value> = boxes.iter().map(|b| units(*b)).collect();
            (v.to_string(), json!(list))
        })
        .collect();
    json!({ "property": property, "values": values })
}

/// A `shapes` object selecting by `facing`, from the up-facing boxes.
pub fn by_facing(boxes: &[Px]) -> Value {
    let values: serde_json::Map<String, Value> = ["down", "up", "north", "south", "west", "east"]
        .iter()
        .map(|f| {
            (
                f.to_string(),
                json!(
                    boxes
                        .iter()
                        .map(|b| units(to_facing(*b, f)))
                        .collect::<Vec<_>>()
                ),
            )
        })
        .collect();
    json!({ "property": "facing", "values": values })
}

/// A `shapes` object selecting by a horizontal `facing`, from boxes written facing north
/// (`VoxelShaper.forHorizontal(NORTH)`), turned about the block's centre.
pub fn by_horizontal(boxes: &[Px]) -> Value {
    let turn = |b: &Px, f: &str| -> Px {
        let (x0, z0, x1, z1) = (b[0], b[2], b[3], b[5]);
        let (a, c) = match f {
            "east" => ([16. - z0, x0], [16. - z1, x1]),
            "south" => ([16. - x0, 16. - z0], [16. - x1, 16. - z1]),
            "west" => ([z0, 16. - x0], [z1, 16. - x1]),
            _ => ([x0, z0], [x1, z1]),
        };
        [
            a[0].min(c[0]),
            b[1],
            a[1].min(c[1]),
            a[0].max(c[0]),
            b[4],
            a[1].max(c[1]),
        ]
    };
    let values: serde_json::Map<String, Value> = ["north", "south", "west", "east"]
        .iter()
        .map(|f| {
            (
                f.to_string(),
                json!(boxes.iter().map(|b| units(turn(b, f))).collect::<Vec<_>>()),
            )
        })
        .collect();
    json!({ "property": "facing", "values": values })
}

/// Create's `BeltShapes`: a belt's outline (`getShape`: the belt's part and its casing) and its
/// collision (`getCollisionShape`: the outline cut at 19 px, `AllShapes.BELT_COLLISION_MASK`), as
/// `shapes` tables over `facing`, `slope`, `part` and `casing`. Built as Create builds them: parts
/// for a belt facing south, composed of a south and a north half, turned by `VoxelShaper`'s
/// rotations (`rotatedCopy`, `VecHelper.rotate`).
pub fn belt_shapes() -> (Value, Value) {
    let mut outline = serde_json::Map::new();
    let mut collision = serde_json::Map::new();
    for facing in ["north", "south", "west", "east"] {
        for slope in ["horizontal", "upward", "downward", "vertical", "sideways"] {
            for part in ["start", "middle", "end", "pulley"] {
                for casing in [false, true] {
                    let mut boxes = belt::shape(facing, slope, part);
                    boxes.extend(belt::casing(facing, slope, part, casing));
                    let boxes = belt::simplify(boxes);
                    let cut = belt::simplify(
                        boxes
                            .iter()
                            .filter_map(|b| belt::and(*b, [0., 0., 0., 16., 19., 16.]))
                            .collect(),
                    );
                    assert!(
                        boxes.len() <= 16,
                        "belt {facing},{slope},{part},{casing}: {} boxes",
                        boxes.len()
                    );
                    let key = format!("{facing},{slope},{part},{casing}");
                    let list = |b: &[Px]| json!(b.iter().map(|b| units(*b)).collect::<Vec<_>>());
                    outline.insert(key.clone(), list(&boxes));
                    collision.insert(key, list(&cut));
                }
            }
        }
    }
    let table =
        |values| json!({ "properties": ["facing", "slope", "part", "casing"], "values": values });
    (table(outline), table(collision))
}

mod belt {
    use super::Px;

    const SOUTH_MASK: Px = [0., -5., 8., 16., 21., 16.];
    const NORTH_MASK: Px = [0., -5., 0., 16., 21., 8.];
    const FLAT_FULL: Px = [1., 3., 0., 15., 13., 16.];
    const FLAT_END: [Px; 2] = [[1., 4., 0., 15., 12., 16.], [1., 3., 1., 15., 13., 15.]];
    const SIDE_FULL: Px = [3., 1., 0., 13., 15., 16.];
    const SIDE_END: [Px; 2] = [[4., 1., 0., 12., 15., 16.], [3., 1., 1., 13., 15., 15.]];
    const CASING_11PX: Px = [0., 0., 0., 16., 11., 16.];
    const PARTIAL_CASING: Px = [0., 0., 5., 16., 11., 16.];

    /// The intersection of two boxes, if they overlap in a volume.
    pub fn and(a: Px, b: Px) -> Option<Px> {
        let r = [
            a[0].max(b[0]),
            a[1].max(b[1]),
            a[2].max(b[2]),
            a[3].min(b[3]),
            a[4].min(b[4]),
            a[5].min(b[5]),
        ];
        (r[0] < r[3] && r[1] < r[4] && r[2] < r[5]).then_some(r)
    }

    /// `makeSlopePart`: sixteen slices one pixel apart along z, each a pixel up (descending: up
    /// toward the north) or down.
    fn slope_boxes(ascending: bool) -> Vec<Px> {
        (0..16)
            .map(|i| {
                let y = if ascending { 10 - i } else { i - 5 } as f64;
                let z = -(i as f64);
                [1., y, 15. + z, 15., 11. + y, 16. + z]
            })
            .collect()
    }

    /// `compose`: the south half of one part and the north half of another.
    fn compose(south: &[Px], north: &[Px]) -> Vec<Px> {
        let mut out: Vec<Px> = south.iter().filter_map(|b| and(*b, SOUTH_MASK)).collect();
        out.extend(north.iter().filter_map(|b| and(*b, NORTH_MASK)));
        out
    }

    /// `VecHelper.rotate` of a point about the block's centre by X, then Y, then Z degrees, as
    /// `VoxelShaper.rotatedCopy` turns a box's corners (in floats, as `Mth.sin` gives).
    fn rotate(p: [f64; 3], rot: [f64; 3]) -> [f64; 3] {
        let [mut x, mut y, mut z] = [p[0] - 8., p[1] - 8., p[2] - 8.];
        for (axis, deg) in rot.iter().enumerate() {
            if *deg == 0. {
                continue;
            }
            let a = (*deg as f32 / 180.0 * std::f32::consts::PI) as f64;
            let (s, c) = (a.sin(), a.cos());
            (x, y, z) = match axis {
                0 => (x, y * c - z * s, z * c + y * s),
                1 => (x * c + z * s, y, z * c - x * s),
                _ => (x * c - y * s, y * c + x * s, z),
            };
        }
        [x + 8., y + 8., z + 8.].map(|v| (v * 1.0e4).round() / 1.0e4)
    }

    fn rotated(boxes: &[Px], rot: [f64; 3]) -> Vec<Px> {
        if rot == [0.; 3] {
            return boxes.to_vec();
        }
        boxes
            .iter()
            .map(|b| {
                let (a, c) = (
                    rotate([b[0], b[1], b[2]], rot),
                    rotate([b[3], b[4], b[5]], rot),
                );
                [
                    a[0].min(c[0]),
                    a[1].min(c[1]),
                    a[2].min(c[2]),
                    a[0].max(c[0]),
                    a[1].max(c[1]),
                    a[2].max(c[2]),
                ]
            })
            .collect()
    }

    /// `Direction.get2DDataValue` times 90 (`horizontalAngleFromDirection`) and `toYRot`.
    fn angle(facing: &str) -> f64 {
        match facing {
            "west" => 90.,
            "north" => 180.,
            "east" => 270.,
            _ => 0.,
        }
    }

    /// `VoxelShaper.forHorizontal(shape, SOUTH).get(facing)`.
    fn horizontal(boxes: &[Px], facing: &str) -> Vec<Px> {
        rotated(boxes, [0., -angle(facing), 0.])
    }

    /// `forHorizontalAxis(shape, Z).get(axis)`: the shape for south, or for east.
    fn horizontal_axis(boxes: &[Px], facing: &str) -> Vec<Px> {
        let along_x = matches!(facing, "east" | "west");
        horizontal(boxes, if along_x { "east" } else { "south" })
    }

    /// `VerticalBeltShaper.make(shape).get(facing)`: the south shape stood up (X by -90), then
    /// turned to `facing` (X by 180 for a negative direction, Y by `-toYRot`).
    fn vertical(boxes: &[Px], facing: &str) -> Vec<Px> {
        let up = rotated(boxes, [-90., 0., 0.]);
        let negative = matches!(facing, "north" | "west");
        rotated(&up, [if negative { 180. } else { 0. }, -angle(facing), 0.])
    }

    /// `getBeltShape`.
    pub fn shape(facing: &str, slope: &str, part: &str) -> Vec<Px> {
        let full = matches!(part, "middle" | "pulley");
        let start = part == "start";
        let ending = |end: &[Px], other: &[Px]| -> Vec<Px> {
            if start {
                compose(other, end)
            } else {
                compose(end, other)
            }
        };
        match slope {
            "vertical" if full => {
                let positive = if matches!(facing, "east" | "west") {
                    "east"
                } else {
                    "south"
                };
                vertical(&[FLAT_FULL], positive)
            }
            "vertical" => vertical(&ending(&FLAT_END, &[FLAT_FULL]), facing),
            "horizontal" if full => horizontal_axis(&[FLAT_FULL], facing),
            "horizontal" => horizontal(&ending(&FLAT_END, &[FLAT_FULL]), facing),
            "sideways" if full => horizontal_axis(&[SIDE_FULL], facing),
            "sideways" => horizontal(&ending(&SIDE_END, &[SIDE_FULL]), facing),
            _ => {
                let part = slope_part(slope);
                if full {
                    horizontal(&part, facing)
                } else {
                    horizontal(&ending(&FLAT_END, &part), facing)
                }
            }
        }
    }

    fn slope_part(slope: &str) -> Vec<Px> {
        slope_boxes(slope == "upward")
    }

    /// `getCasingShape`.
    pub fn casing(facing: &str, slope: &str, part: &str, casing: bool) -> Vec<Px> {
        if !casing || matches!(slope, "vertical" | "sideways") {
            return Vec::new();
        }
        let opposite = match facing {
            "north" => "south",
            "south" => "north",
            "west" => "east",
            _ => "west",
        };
        let upward = slope == "upward";
        let partial = |f: &str| horizontal(&[PARTIAL_CASING], f);
        match (slope, part) {
            ("horizontal", _) => vec![CASING_11PX],
            (_, "middle" | "pulley") => partial(if upward { facing } else { opposite }),
            (_, "start") if upward => vec![CASING_11PX],
            (_, "start") => partial(opposite),
            (_, _) if !upward => vec![CASING_11PX],
            _ => partial(facing),
        }
    }

    /// The same union in fewer boxes: boxes inside another dropped, and pairs that make one
    /// box together joined.
    pub fn simplify(mut boxes: Vec<Px>) -> Vec<Px> {
        let inside = |a: &Px, b: &Px| (0..3).all(|i| a[i] >= b[i] && a[i + 3] <= b[i + 3]);
        loop {
            let mut changed = false;
            'outer: for i in 0..boxes.len() {
                for j in 0..boxes.len() {
                    if i == j {
                        continue;
                    }
                    let (a, b) = (boxes[i], boxes[j]);
                    if inside(&a, &b) {
                        boxes.remove(i);
                        changed = true;
                        break 'outer;
                    }
                    // Equal on two axes and touching or overlapping on the third.
                    for axis in 0..3 {
                        let same = (0..3)
                            .filter(|k| *k != axis)
                            .all(|k| a[k] == b[k] && a[k + 3] == b[k + 3]);
                        if same && a[axis] <= b[axis + 3] && b[axis] <= a[axis + 3] {
                            let mut m = a;
                            m[axis] = a[axis].min(b[axis]);
                            m[axis + 3] = a[axis + 3].max(b[axis + 3]);
                            boxes[i] = m;
                            boxes.remove(j);
                            changed = true;
                            break 'outer;
                        }
                    }
                }
            }
            if !changed {
                return boxes;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poles_turn_to_their_axis_and_motors_to_their_facing() {
        assert_eq!(to_axis(SIX_VOXEL_POLE, "x"), [0., 5., 5., 16., 11., 11.]);
        assert_eq!(to_axis(SIX_VOXEL_POLE, "z"), [5., 5., 0., 11., 11., 16.]);
        // A motor facing north has its body at the south end, open to the north.
        assert_eq!(to_facing(MOTOR_BLOCK, "north"), [3., 3., 2., 13., 13., 16.]);
        assert_eq!(to_facing(MOTOR_BLOCK, "east"), [0., 3., 3., 14., 13., 13.]);
        assert_eq!(to_facing(MOTOR_BLOCK, "down"), [3., 2., 3., 13., 16., 13.]);
        // A belt funnel facing north has its body to the south; facing east, to the west.
        let v = by_horizontal(&[[0., -5., 8., 16., 16., 14.]]);
        let east: Vec<f64> = serde_json::from_value(v["values"]["east"][0].clone()).unwrap();
        assert_eq!(east, [2. / 16., -5. / 16., 0., 8. / 16., 1., 1.]);
    }

    #[test]
    fn belts_take_create_shapes_by_facing_slope_part_and_casing() {
        let (outline, collision) = belt_shapes();
        let px = |t: &Value, key: &str| -> Vec<Px> {
            serde_json::from_value::<Vec<Px>>(t["values"][key].clone())
                .unwrap()
                .into_iter()
                .map(|b| b.map(|v| v * 16.0))
                .collect()
        };
        assert_eq!(
            px(&outline, "north,horizontal,middle,false"),
            [[1., 3., 0., 15., 13., 16.]]
        );
        assert_eq!(
            px(&outline, "east,horizontal,pulley,false"),
            [[0., 3., 1., 16., 13., 15.]]
        );
        // A flat belt in its casing is the casing's 11 px and the belt over it.
        let cased = px(&outline, "south,horizontal,middle,true");
        assert!(cased.contains(&[0., 0., 0., 16., 11., 16.]));
        // Descending south: low at the south end, high at the north, past the block both ways.
        let slope = px(&outline, "south,downward,middle,false");
        assert_eq!(slope.len(), 16);
        assert!(slope.contains(&[1., -5., 15., 15., 6., 16.]));
        assert!(slope.contains(&[1., 10., 0., 15., 21., 1.]));
        // Ascending north rises toward the north, as descending south does.
        let mut up = px(&outline, "north,upward,middle,false");
        up.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut down = slope.clone();
        down.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(up, down);
        // Collision is cut at 19 px.
        assert!(
            px(&collision, "south,downward,middle,false")
                .iter()
                .all(|b| b[4] <= 19.)
        );
        // Vertical: a panel across the facing's axis.
        assert_eq!(
            px(&outline, "south,vertical,middle,false"),
            [[1., 0., 3., 15., 16., 13.]]
        );
        assert_eq!(
            px(&outline, "west,vertical,pulley,false"),
            [[3., 0., 1., 13., 16., 15.]]
        );
        // Every state fits the declaration's 16 boxes.
        for v in outline["values"].as_object().unwrap().values() {
            assert!(!v.as_array().unwrap().is_empty() && v.as_array().unwrap().len() <= 16);
        }
    }
}
