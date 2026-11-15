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
    let along = props.get(ALONG_FIRST).map(|v| v == "true");
    let cycle = |out: &mut BTreeMap<String, String>| {
        if let Some(v) = out.get(ALONG_FIRST).map(|v| v == "true") {
            out.insert(ALONG_FIRST.into(), (!v).to_string());
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

/// `PumpBlock.getRotatedBlockState`: the state's `facing` turned around.
pub fn reversed(props: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut out = props.clone();
    if let Some(f) = props.get("facing").and_then(|v| dir_of(v)) {
        out.insert("facing".into(), dir_name(f.opposite()).into());
    }
    out
}

const ALONG_FIRST: &str = "axis_along_first";

/// The palette's stone types (`AllPaletteStoneTypes`), whose cut, polished, brick, layered and
/// pillar variants are plain blocks.
const STONE_TYPES: [&str; 14] = [
    "granite",
    "diorite",
    "andesite",
    "calcite",
    "dripstone",
    "deepslate",
    "tuff",
    "asurine",
    "crimsite",
    "limestone",
    "ochrum",
    "scorchia",
    "scoria",
    "veridium",
];

/// Whether Create's block `name` (without its namespace) is an `IWrenchable`: the wrench turns
/// and picks up only those (others only when in `create:wrench_pickup`, sneaking). The rest are
/// Create's plain building blocks and the few machines that are not: the schematic table and
/// cannon, analog lever, bells, lectern controller, minecart anchor, seats and toolboxes.
pub fn wrenchable(name: &str) -> bool {
    const PLAIN: [&str; 26] = [
        "schematicannon",
        "schematic_table",
        "crushing_wheel_controller",
        "minecart_anchor",
        "fake_track",
        "analog_lever",
        "lectern_controller",
        "peculiar_bell",
        "haunted_bell",
        "copycat_base",
        "zinc_ore",
        "deepslate_zinc_ore",
        "raw_zinc_block",
        "zinc_block",
        "andesite_alloy_block",
        "industrial_iron_block",
        "weathered_iron_block",
        "brass_block",
        "cardboard_block",
        "bound_cardboard_block",
        "experience_block",
        "rose_quartz_block",
        "rose_quartz_tiles",
        "small_rose_quartz_tiles",
        "tiled_glass",
        "framed_glass",
    ];
    if PLAIN.contains(&name)
        || name.ends_with("framed_glass")
        || name.ends_with("_window")
        || name.ends_with("_window_pane")
        || name.ends_with("_glass_pane")
        || name.ends_with("_bars")
        || name.ends_with("_toolbox")
        || name.ends_with("_seat")
        || name.ends_with("_pillar")
        || name.contains("copper_shingle")
        || name.contains("copper_tile")
    {
        return false;
    }
    // The stone types (Create's own; vanilla's are vanilla's) and their cut, polished, brick and
    // layered variants (`cut_granite_brick_stairs`, `layered_tuff`...).
    if STONE_TYPES[7..].contains(&name) {
        return false;
    }
    let Some(stem) = ["polished_cut_", "cut_", "small_", "layered_"]
        .iter()
        .find_map(|p| name.strip_prefix(p))
    else {
        return true;
    };
    !STONE_TYPES
        .iter()
        .any(|t| stem == *t || stem.strip_prefix(t).is_some_and(|r| r.starts_with('_')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pump_turns_around() {
        let p = props(&[("facing", "east"), ("waterlogged", "false")]);
        assert_eq!(
            reversed(&p),
            props(&[("facing", "west"), ("waterlogged", "false")])
        );
    }

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
        // A deployer facing up clicked on its top: its axis along the first coordinate cycles.
        let deployer = props(&[("facing", "up"), ("axis_along_first", "false")]);
        assert_eq!(
            rotated(&deployer, false, Dir::Up)["axis_along_first"],
            "true"
        );
        // Clicked on its north side, it turns about z without cycling.
        let turned = rotated(&deployer, false, Dir::North);
        assert_eq!(
            (
                turned["facing"].as_str(),
                turned["axis_along_first"].as_str()
            ),
            ("east", "false")
        );
        // A deployer facing north clicked on top turns about y once, cycling once.
        let flat = props(&[("facing", "north"), ("axis_along_first", "false")]);
        let turned = rotated(&flat, false, Dir::Up);
        assert_eq!(
            (
                turned["facing"].as_str(),
                turned["axis_along_first"].as_str()
            ),
            ("east", "true")
        );
    }

    #[test]
    fn only_wrenchables_turn() {
        for name in [
            "shaft",
            "chute",
            "andesite_casing",
            "andesite_door",
            "metal_girder",
            "placard",
            "small_bogey",
            "andesite_tunnel",
            "andesite_ladder",
            "andesite_scaffolding",
            "copycat_panel",
        ] {
            assert!(wrenchable(name), "{name}");
        }
        for name in [
            "cut_granite_brick_stairs",
            "granite_pillar",
            "layered_scoria",
            "scoria",
            "small_tuff_brick_wall",
            "polished_cut_andesite_slab",
            "oak_window_pane",
            "andesite_bars",
            "brown_toolbox",
            "red_seat",
            "schematic_table",
            "exposed_copper_shingle_stairs",
            "horizontal_framed_glass",
            "framed_glass",
        ] {
            assert!(!wrenchable(name), "{name}");
        }
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
