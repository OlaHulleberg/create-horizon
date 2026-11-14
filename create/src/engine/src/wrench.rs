//! Create's wrench (`WrenchItem`, `IWrenchable`): a use turns a block about the axis of the
//! face clicked (`getRotatedBlockState`), a sneaking use picks it up.

use crate::kinetics::{Axis, Dir};
use std::collections::BTreeMap;

/// `Direction.getClockWise(axis)`: a quarter turn about `axis`, clockwise looking at it from
/// its positive end; a direction along the axis stays.
pub fn clockwise(d: Dir, axis: Axis) -> Dir {
    if d.axis() == axis {
        return d;
    }
    match axis {
        // getClockWiseX: down, south, up, north.
        Axis::X => match d {
            Dir::Down => Dir::South,
            Dir::South => Dir::Up,
            Dir::Up => Dir::North,
            _ => Dir::Down,
        },
        // getClockWise: north, east, south, west.
        Axis::Y => match d {
            Dir::North => Dir::East,
            Dir::East => Dir::South,
            Dir::South => Dir::West,
            _ => Dir::North,
        },
        // getClockWiseZ: down, west, up, east.
        Axis::Z => match d {
            Dir::Down => Dir::West,
            Dir::West => Dir::Up,
            Dir::Up => Dir::East,
            _ => Dir::Down,
        },
    }
}

fn dir_of(name: &str) -> Option<Dir> {
    Dir::ALL.into_iter().find(|d| dir_name(*d) == name)
}

fn dir_name(d: Dir) -> &'static str {
    match d {
        Dir::Down => "down",
        Dir::Up => "up",
        Dir::North => "north",
        Dir::South => "south",
        Dir::West => "west",
        Dir::East => "east",
    }
}

fn axis_of(name: &str) -> Option<Axis> {
    match name {
        "x" => Some(Axis::X),
        "y" => Some(Axis::Y),
        "z" => Some(Axis::Z),
        _ => None,
    }
}

fn axis_name(a: Axis) -> &'static str {
    match a {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

/// `VoxelShaper.axisAsFace(axis).getClockWise(about).getAxis()`.
fn turn_axis(a: Axis, about: Axis) -> Axis {
    clockwise(Dir::from_axis(a, true), about).axis()
}

/// `IWrenchable.getRotatedBlockState` on a state's properties: `horizontal` says its `facing`
/// is a horizontal one (`HORIZONTAL_FACING`) rather than any direction (`FACING`).
pub fn rotated(
    props: &BTreeMap<String, String>,
    horizontal: bool,
    face: Dir,
) -> BTreeMap<String, String> {
    let mut out = props.clone();
    let about = face.axis();
    if about == Axis::Y {
        if let Some(a) = props.get("horizontal_axis").and_then(|v| axis_of(v)) {
            out.insert(
                "horizontal_axis".into(),
                axis_name(turn_axis(a, about)).into(),
            );
            return out;
        }
        if horizontal && let Some(f) = props.get("facing").and_then(|v| dir_of(v)) {
            out.insert("facing".into(), dir_name(clockwise(f, about)).into());
            return out;
        }
    }
    // No Create block has both an axis and a facing: an `axis` beside a `facing` is the
    // template's (a gauge on stripped wood), not Create's `AXIS`.
    if !props.contains_key("facing")
        && let Some(a) = props.get("axis").and_then(|v| axis_of(v))
    {
        out.insert("axis".into(), axis_name(turn_axis(a, about)).into());
        return out;
    }
    if horizontal {
        return out;
    }
    let Some(mut f) = props.get("facing").and_then(|v| dir_of(v)) else {
        return out;
    };
    // DirectionalAxisKineticBlock.AXIS_ALONG_FIRST_COORDINATE: cycled when clicked on the
    // facing's own axis, and with each turn about y.
    let along = props.get("axis_along_first").map(|v| v == "true");
    let cycle = |out: &mut BTreeMap<String, String>| {
        if let Some(a) = out.get("axis_along_first").map(|v| v == "true") {
            out.insert("axis_along_first".into(), (!a).to_string());
        }
    };
    if f.axis() == about {
        if along.is_some() {
            cycle(&mut out);
        }
        return out;
    }
    loop {
        f = clockwise(f, about);
        if about == Axis::Y {
            cycle(&mut out);
        }
        if f.axis() != about {
            break;
        }
    }
    out.insert("facing".into(), dir_name(f).into());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn quarter_turns_are_one_rule_on_three_axes() {
        // Each turn is -90 degrees about the positive axis: four make a full turn, and the
        // turn about y is vanilla's clockwise seen from above.
        for axis in [Axis::X, Axis::Y, Axis::Z] {
            for d in Dir::ALL.into_iter().filter(|d| d.axis() != axis) {
                let mut t = d;
                for _ in 0..4 {
                    t = clockwise(t, axis);
                    assert_ne!(t.axis(), axis);
                }
                assert_eq!(t, d);
                // The cross product of the axis' positive direction and d is the turn's
                // opposite: x' = d x axis.
                let (o, a) = (d.offset(), Dir::from_axis(axis, true).offset());
                let cross = [
                    o[1] * a[2] - o[2] * a[1],
                    o[2] * a[0] - o[0] * a[2],
                    o[0] * a[1] - o[1] * a[0],
                ];
                assert_eq!(clockwise(d, axis).offset(), cross, "{d:?} about {axis:?}");
            }
        }
        assert_eq!(clockwise(Dir::North, Axis::Y), Dir::East);
    }

    #[test]
    fn wrenching_turns_axes_and_facings_as_create_does() {
        // A shaft along x clicked on top: about y, x becomes z.
        let shaft = rotated(&props(&[("axis", "x")]), false, Dir::Up);
        assert_eq!(shaft["axis"], "z");
        // Clicked on its end (x): it stays.
        assert_eq!(
            rotated(&props(&[("axis", "x")]), false, Dir::East)["axis"],
            "x"
        );
        // A press (horizontal facing) turns only when clicked on top or bottom.
        assert_eq!(
            rotated(&props(&[("facing", "north")]), true, Dir::Up)["facing"],
            "east"
        );
        assert_eq!(
            rotated(&props(&[("facing", "north")]), true, Dir::West)["facing"],
            "north"
        );
        // A motor facing up clicked on its north side turns about z: up becomes east.
        assert_eq!(
            rotated(&props(&[("facing", "up")]), false, Dir::North)["facing"],
            "east"
        );
        // Clicked on its own axis: it stays.
        assert_eq!(
            rotated(&props(&[("facing", "up")]), false, Dir::Down)["facing"],
            "up"
        );
    }

    #[test]
    fn gauges_turn_their_facing_and_cycle_their_shaft() {
        // A gauge on stripped wood carries the template's `axis`: its facing turns instead.
        let gauge = props(&[
            ("axis", "y"),
            ("facing", "north"),
            ("axis_along_first", "false"),
        ]);
        let up = rotated(&gauge, false, Dir::Up);
        assert_eq!((up["facing"].as_str(), up["axis"].as_str()), ("east", "y"));
        // About y the shaft's flag flips with the facing, keeping it along the same world axis.
        assert_eq!(up["axis_along_first"], "true");
        // Clicked on its facing's own axis: only the shaft turns.
        let front = rotated(&gauge, false, Dir::South);
        assert_eq!(
            (front["facing"].as_str(), front["axis_along_first"].as_str()),
            ("north", "true")
        );
        // About x, the facing turns and the flag stays.
        let side = rotated(&gauge, false, Dir::East);
        assert_eq!(
            (side["facing"].as_str(), side["axis_along_first"].as_str()),
            ("down", "false")
        );
    }
}
