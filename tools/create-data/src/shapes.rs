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
    }
}
