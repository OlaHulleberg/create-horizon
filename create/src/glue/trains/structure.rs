//! A structure's blocks as a train's or a cart's assembly reads them (`Contraption.moveBlock`'s
//! tests), and blocks turned as they go back (`StructureTransform`).

use super::super::kinetics;
use super::super::server::ServerLevel;
use super::super::world::with_world;
use create_engine::contraption::{CHASSIS_RANGE, Chassis, Found, Sticky, radial_side};
use create_engine::kinetics::{Axis, Dir};
use mod_api::{BlockState, Host};

fn dir_named(name: &str) -> Option<Dir> {
    Dir::ALL
        .into_iter()
        .find(|d| super::super::kinetics::dir_name(*d) == name)
}

fn axis_named(name: Option<&str>) -> Axis {
    match name {
        Some("x") => Axis::X,
        Some("z") => Axis::Z,
        _ => Axis::Y,
    }
}

/// The block at `p` as a structure search reads it: nothing for air and fluids; unmovable for
/// unbreakable blocks and for a block whose block entity is not Create's own (moving a block
/// entity's contents isn't ported: refused rather than emptied); its stickiness, glue (the
/// glue the world keeps), an extended sticker and a chassis.
/// For a train's carriages: chests, barrels, vaults, toolboxes (`cargo`) and sliding doors move
/// too.
pub fn found_carrying(lv: &ServerLevel<'_>, p: [i32; 3]) -> Option<Found> {
    found_with(lv, p, true)
}

fn found_with(lv: &ServerLevel<'_>, p: [i32; 3], storage: bool) -> Option<Found> {
    let state = lv.block_state(p)?;
    let (name, props) = super::state_parts(&*lv.host, state)?;
    let prop = |k: &str| props.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    if matches!(
        name.as_str(),
        "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:water"
            | "minecraft:lava"
    ) {
        return None;
    }
    let sail =
        name == "create:sail_frame" || (name.starts_with("create:") && name.ends_with("_sail"));
    let sail_facing = sail.then(|| prop("facing").and_then(dir_named)).flatten();
    // A storage block's items ride with it (`MountedStorageManager`, `cargo`).
    let foreign_entity = lv
        .host
        .block_entity(lv.level, p)
        .is_some_and(|e| e.kind(&*lv.host) != kinetics::kind())
        && !(storage && crate::glue::contraptions::actors::is_storage(lv, p))
        // A sliding door's block entity holds only its animation (`SlidingDoorBlockEntity`).
        && !(storage && super::doors::is_sliding_door(&name));
    let unmovable = matches!(
        name.as_str(),
        "minecraft:bedrock"
            | "minecraft:barrier"
            | "minecraft:end_portal_frame"
            | "minecraft:command_block"
            | "minecraft:chain_command_block"
            | "minecraft:repeating_command_block"
            | "minecraft:structure_block"
            | "minecraft:jigsaw"
    ) || foreign_entity;
    let dim = lv.dim;
    let range = with_world(|w| w.dims[dim].chassis_range.get(&p).copied())
        .flatten()
        .unwrap_or(CHASSIS_RANGE);
    let chassis = match name.as_str() {
        // LinearChassisBlock: its kind, axis and glued ends.
        "create:linear_chassis" | "create:secondary_linear_chassis" => Some(Chassis {
            secondary: name == "create:secondary_linear_chassis",
            axis: axis_named(prop("axis")),
            sticky_top: prop("sticky_top") == Some("true"),
            sticky_bottom: prop("sticky_bottom") == Some("true"),
            range,
            radial: None,
        }),
        // RadialChassisBlock: its axis and glued sides.
        "create:radial_chassis" => {
            let axis = axis_named(prop("axis"));
            let sides = Dir::ALL.into_iter().fold(0u8, |acc, d| {
                let glued = radial_side(axis, d).and_then(prop) == Some("true");
                if glued { acc | d.bit() } else { acc }
            });
            Some(Chassis {
                secondary: false,
                axis,
                sticky_top: false,
                sticky_bottom: false,
                range,
                radial: Some(sides),
            })
        }
        _ => None,
    };
    Some(Found {
        state: super::car_id(state),
        sail_facing,
        sticky: match name.as_str() {
            "minecraft:slime_block" => Some(Sticky::Slime),
            "minecraft:honey_block" => Some(Sticky::Honey),
            _ => None,
        },
        glued: with_world(|w| create_engine::glue::glued_faces(&w.dims[dim].glue, p)).unwrap_or(0)
            // `AbstractBogeyBlock.getStickySurfaces`: a bogey holds the block on top of it.
            | if matches!(name.as_str(), "create:small_bogey" | "create:large_bogey") {
                Dir::Up.bit()
            } else {
                0
            },
        // An extended sticker's facing (`StickerBlock.EXTENDED`).
        sticker: (name == "create:sticker" && prop("extended") == Some("true"))
            .then(|| prop("facing").and_then(dir_named))
            .flatten(),
        movable: !unmovable,
        chassis,
        attached: crate::glue::contraptions::attached_sides(&name, &props),
        unsupportive: crate::glue::contraptions::unsupportive_faces(&name, &props),
        belt: (name == "create:belt")
            .then(|| {
                let id = super::car_id(state);
                crate::glue::contraptions::create_info(id).and_then(|(_, i)| i.belt)
            })
            .flatten(),
    })
}

/// A block state turned `quarter`° about `axis` (`StructureTransform`): its `facing` or `axis`
/// turned with it (`rotateFacing`: `getClockWise(axis)`, `rotation.ordinal()` times); a state
/// that has no such turned form stays as it was.
pub fn turned_state(host: &dyn Host, state: BlockState, quarter: i32, axis: Axis) -> BlockState {
    let turns = create_engine::contraption::clockwise_turns(quarter);
    if turns == 0 {
        return state;
    }
    let turn_dir = |d: Dir| (0..turns).fold(d, |d, _| create_engine::wrench::clockwise(d, axis));
    if let Some(d) = super::property(host, state, "facing").and_then(|f| dir_named(&f)) {
        let name = super::super::kinetics::dir_name(turn_dir(d));
        return super::with_props(host, state, &[("facing", name)]).unwrap_or(state);
    }
    if let Some(a) = super::property(host, state, "axis") {
        let d = match a.as_str() {
            "x" => Dir::East,
            "y" => Dir::Up,
            _ => Dir::South,
        };
        let name = match turn_dir(d).axis() {
            Axis::X => "x",
            Axis::Y => "y",
            Axis::Z => "z",
        };
        return super::with_props(host, state, &[("axis", name)]).unwrap_or(state);
    }
    state
}
