// Create's contraptions in the game: bearings and windmills, pistons, chassis and gantries
// moving blocks as one, and their save data. A sibling of the `create` module (`runtime.rs`).
use super::Game;
use super::create::*;
use super::create_actors as actors;
use super::create_logistics::*;
use super::create_processing::*;
use super::create_redstone::*;
use create_engine::kinetics::{Axis, Dir};
use create_engine::world::dimension_index;
use create_engine::{client, states};
use horizon_world::registry::BlockStateId;
use std::cell::RefCell;
use std::collections::HashMap;

/// `ChassisBlockEntity.collectChassisGroup`: the linear chassis joined to `pos`, of its kind and
/// axis.
pub(super) fn chassis_group(game: &Game<'_>, pos: [i32; 3]) -> Vec<[i32; 3]> {
    let kind = |p: [i32; 3]| {
        let s = game
            .pack
            .assets
            .registry
            .state(game.world.block(p[0], p[1], p[2]))?;
        matches!(
            s.name.as_str(),
            "create:linear_chassis" | "create:secondary_linear_chassis" | "create:radial_chassis"
        )
        .then(|| (s.name.clone(), s.properties.get("axis").cloned()))
    };
    let Some(own) = kind(pos) else {
        return Vec::new();
    };
    // A radial chassis' group is the radial chassis on its axis either side.
    let radial = own.0 == "create:radial_chassis";
    let axis = match own.1.as_deref() {
        Some("x") => Axis::X,
        Some("z") => Axis::Z,
        _ => Axis::Y,
    };
    let mut group = vec![pos];
    let mut i = 0;
    while i < group.len() && group.len() < 256 {
        let p = group[i];
        i += 1;
        for d in Dir::ALL {
            if radial && d.axis() != axis {
                continue;
            }
            let o = d.offset();
            let n = [p[0] + o[0], p[1] + o[1], p[2] + o[2]];
            if !group.contains(&n) && kind(n).as_ref() == Some(&own) {
                group.push(n);
            }
        }
    }
    group
}

/// `AbstractChassisBlock.useItemOn` for a linear chassis: a slime ball on an end along its axis
/// glues it (on a glued end: every end that may be glued); sneaking with an empty hand unglues
/// it. An end against a chassis of its kind on its axis can't be glued (`glueAllowedOnSide`).
pub(super) fn use_chassis(
    game: &mut Game<'_>,
    pos: [i32; 3],
    face: [i32; 3],
    sneaking: bool,
    canceled: &mut bool,
) {
    let registry = game.pack.assets.registry;
    let id = game.world.block(pos[0], pos[1], pos[2]);
    let Some(mut s) = registry.state(id).cloned() else {
        return;
    };
    let held = game.native_held().map(|h| h.0);
    let slime = held.as_deref() == Some("minecraft:slime_ball")
        || held.as_deref() == Some("create:super_glue");
    let axis = s.properties.get("axis").cloned().unwrap_or_default();
    let radial = s.name == "create:radial_chassis";
    // getGlueableSide: a linear chassis' end on its axis, sticky_top toward the positive end;
    // a radial chassis' side across it (`RadialChassisBlock.getGlueableSide`).
    let side_of = |f: [i32; 3]| -> Option<&'static str> {
        let i = ["x", "y", "z"].iter().position(|a| *a == axis)?;
        if radial {
            let a = [Axis::X, Axis::Y, Axis::Z][i];
            return create_engine::contraption::radial_side(a, Dir::from_offset(f)?);
        }
        match f[i] {
            1 => Some("sticky_top"),
            -1 => Some("sticky_bottom"),
            _ => None,
        }
    };
    let Some(side) = side_of(face) else {
        return;
    };
    // `glueAllowedOnSide`: a radial chassis may be glued on any side.
    let allowed = |game: &Game<'_>, f: [i32; 3]| {
        radial
            || registry
                .state(
                    game.world
                        .block(pos[0] + f[0], pos[1] + f[1], pos[2] + f[2]),
                )
                .is_none_or(|o| o.name != s.name || o.properties.get("axis") != Some(&axis))
    };
    let glued = s.properties.get(side).is_some_and(|v| v == "true");
    if slime && glued {
        for f in [
            [1, 0, 0],
            [-1, 0, 0],
            [0, 1, 0],
            [0, -1, 0],
            [0, 0, 1],
            [0, 0, -1],
        ] {
            if let Some(other) = side_of(f)
                && allowed(game, f)
            {
                s.properties.insert(other.into(), "true".into());
            }
        }
    } else if (slime || (sneaking && held.is_none())) && glued != slime && allowed(game, face) {
        s.properties
            .insert(side.into(), if slime { "true" } else { "false" }.into());
    } else {
        return;
    }
    *canceled = true;
    if let Some(new) = registry.id(&s)
        && new != id
    {
        let dimension = game.dim.id().to_owned();
        game.native_set_block(&dimension, pos, new, super::native_hooks::flags::UPDATE_ALL);
    }
}

/// `WindmillBearingBlock.useItemOn` with an empty hand, not sneaking: assembled sails come
/// apart now; otherwise they assemble on the next tick (`assembleNextTick`).
pub(super) fn use_windmill(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    sneaking: bool,
    canceled: &mut bool,
) {
    if sneaking {
        *canceled = true;
        return;
    }
    if game.native_held().is_some_and(|h| h.0 != "minecraft:air") {
        return;
    }
    *canceled = true;
    let running = with_world(|w| w.dims[dim].windmills.contains_key(&pos)) == Some(true);
    if running {
        let dimension = game.dim.id().to_owned();
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        if let Some(w) = with_world(|w| w.windmill_disassembled(dim, pos, state)).flatten() {
            put_back(game, &dimension, pos, &w);
        }
    } else {
        with_world(|w| {
            if !w.dims[dim].windmill_assemble.contains(&pos) {
                w.dims[dim].windmill_assemble.push(pos);
            }
        });
    }
}

/// `ControlledContraptionEntity.disassemble` at angle 0 (a windmill turns back first): each
/// block where it was, or dropped where something else took its place.
pub(super) fn put_back(
    game: &mut Game<'_>,
    dimension: &str,
    bearing: [i32; 3],
    w: &create_engine::world::Windmill,
) {
    let registry = game.pack.assets.registry;
    for (o, state) in &w.blocks {
        let p = [bearing[0] + o[0], bearing[1] + o[1], bearing[2] + o[2]];
        let here = game.world.block(p[0], p[1], p[2]);
        let free = registry
            .state(here)
            .is_some_and(|s| s.name == "minecraft:air" || s.name == "minecraft:cave_air");
        if free {
            game.native_set_block(
                dimension,
                p,
                BlockStateId(*state),
                super::native_hooks::flags::UPDATE_ALL,
            );
        } else if let Some(s) = registry.state(BlockStateId(*state)) {
            game.native_run_command(&format!(
                "execute in {dimension} run summon minecraft:item {}.5 {}.5 {}.5 {{Item:{{id:'{}',count:1}}}}",
                p[0], p[1], p[2], s.name
            ));
        }
    }
}

/// A radial chassis as the search reads it: its axis, glued sides and range.
fn radial_chassis(
    game: &Game<'_>,
    p: [i32; 3],
    props: &std::collections::BTreeMap<String, String>,
) -> create_engine::contraption::Chassis {
    let axis = match props.get("axis").map(String::as_str) {
        Some("x") => Axis::X,
        Some("z") => Axis::Z,
        _ => Axis::Y,
    };
    let sides = Dir::ALL.into_iter().fold(0u8, |acc, d| {
        let glued = create_engine::contraption::radial_side(axis, d)
            .and_then(|k| props.get(k))
            .is_some_and(|v| v == "true");
        if glued { acc | d.bit() } else { acc }
    });
    create_engine::contraption::Chassis {
        secondary: false,
        axis,
        sticky_top: false,
        sticky_bottom: false,
        range: dimension_index(game.dim.id())
            .and_then(|dim| with_world(|w| w.dims[dim].chassis_range.get(&p).copied()))
            .flatten()
            .unwrap_or(create_engine::contraption::CHASSIS_RANGE),
        radial: Some(sides),
    }
}

/// Blocks Create can't move (unbreakable ones) and blocks whose block entity would be lost
/// (moving block entities isn't ported: refused rather than emptied).
pub(super) fn contraption_found(
    game: &Game<'_>,
    p: [i32; 3],
) -> Option<create_engine::contraption::Found> {
    use create_engine::contraption::{Found, Sticky};
    let registry = game.pack.assets.registry;
    let id = game.world.block(p[0], p[1], p[2]);
    let s = registry.state(id)?;
    if matches!(
        s.name.as_str(),
        "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:water"
            | "minecraft:lava"
    ) {
        return None;
    }
    let sail = s.name == "create:sail_frame"
        || (s.name.starts_with("create:") && s.name.ends_with("_sail"));
    let sail_facing = sail
        .then(|| {
            s.properties
                .get("facing")
                .and_then(|f| Dir::ALL.into_iter().find(|d| dir_name(*d) == f))
        })
        .flatten();
    let unmovable = matches!(
        s.name.as_str(),
        "minecraft:bedrock"
            | "minecraft:barrier"
            | "minecraft:end_portal_frame"
            | "minecraft:command_block"
            | "minecraft:chain_command_block"
            | "minecraft:repeating_command_block"
            | "minecraft:structure_block"
            | "minecraft:jigsaw"
    ) || (game.world.block_entities.contains_key(&(p[0], p[1], p[2]))
        && !actors::carries_storage(game, p));
    Some(Found {
        state: id.0,
        sail_facing,
        sticky: match s.name.as_str() {
            "minecraft:slime_block" => Some(Sticky::Slime),
            "minecraft:honey_block" => Some(Sticky::Honey),
            _ => None,
        },
        glued: dimension_index(game.dim.id()).map_or(0, |dim| actors::glued_at(dim, p)),
        sticker: actors::sticker_facing(game, id),
        movable: !unmovable,
        // LinearChassisBlock: its kind, axis and glued ends.
        chassis: matches!(
            s.name.as_str(),
            "create:linear_chassis" | "create:secondary_linear_chassis"
        )
        .then(|| create_engine::contraption::Chassis {
            secondary: s.name == "create:secondary_linear_chassis",
            axis: match s.properties.get("axis").map(String::as_str) {
                Some("x") => Axis::X,
                Some("z") => Axis::Z,
                _ => Axis::Y,
            },
            sticky_top: s.properties.get("sticky_top").is_some_and(|v| v == "true"),
            sticky_bottom: s
                .properties
                .get("sticky_bottom")
                .is_some_and(|v| v == "true"),
            range: dimension_index(game.dim.id())
                .and_then(|dim| with_world(|w| w.dims[dim].chassis_range.get(&p).copied()))
                .flatten()
                .unwrap_or(create_engine::contraption::CHASSIS_RANGE),
            radial: None,
        })
        .or_else(|| {
            (s.name == "create:radial_chassis").then(|| radial_chassis(game, p, &s.properties))
        }),
    })
}

/// Windmills: structures whose bearing broke go back; right-clicked bearings assemble their
/// sails (`assemble`), the blocks leaving the world; clients see the changes. Nothing runs
/// without either.
pub(super) fn tick_windmills(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let Some((orphans, queued)) = with_world(|w| {
        let d = &mut w.dims[dim];
        (
            std::mem::take(&mut d.windmill_orphans),
            std::mem::take(&mut d.windmill_assemble),
        )
    }) else {
        return;
    };
    for (pos, w) in orphans {
        put_back(game, dimension, pos, &w);
    }
    for pos in queued {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let facing = with_world(|w| {
            w.table
                .get(state)
                .filter(|(b, _)| *b == create_engine::Block::WindmillBearing)
                .and_then(|(_, i)| i.facing)
        })
        .flatten();
        let Some(facing) = facing else {
            continue;
        };
        let gathered = create_engine::contraption::gather_windmill(pos, facing, |p| {
            contraption_found(game, p)
        });
        let Ok((blocks, sails)) = gathered else {
            continue;
        };
        if let Some(air) = game.pack.assets.registry.default_state("minecraft:air") {
            for (o, _) in &blocks {
                let p = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
                game.native_set_block(
                    dimension,
                    p,
                    air,
                    super::native_hooks::flags::UPDATE_CLIENTS,
                );
            }
        }
        let w = create_engine::world::Windmill {
            blocks,
            sails,
            clockwise: true,
            bearing: None,
        };
        with_world(|world| world.windmill_assembled(dim, pos, state, w));
    }
    let changed = with_world(|w| {
        let mut v = std::mem::take(&mut w.dims[dim].windmill_changes);
        v.sort();
        v.dedup();
        v
    })
    .unwrap_or_default();
    if !changed.is_empty() {
        let players = players_in(game, dimension);
        send_windmills(game, &players, dim, &changed);
    }
}

/// Mechanical bearings whose speed changed (`tick` after `onSpeedChanged`): a still one starts
/// turning what is in front of it (`assemble`); a turning one stopped puts it back turned to the
/// nearest quarter turn (`disassemble`, `ROTATE_PLACE`); otherwise the angle runs on at the new
/// speed. Nothing runs without a change.
pub(super) fn tick_bearings(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::contraption::{self, Turn};
    let Some(mut checks) = with_world(|w| std::mem::take(&mut w.dims[dim].bearing_checks)) else {
        return;
    };
    if checks.is_empty() {
        return;
    }
    checks.sort();
    checks.dedup();
    let now = game.world.game_time;
    for pos in checks {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let facing = with_world(|w| {
            w.table
                .get(state)
                .filter(|(b, _)| *b == create_engine::Block::MechanicalBearing)
                .and_then(|(_, i)| i.facing)
        })
        .flatten();
        let Some(facing) = facing else {
            continue;
        };
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let running = with_world(|w| w.dims[dim].windmills.get(&pos).map(|m| m.bearing)).flatten();
        match running {
            Some(Some(turn)) if speed == 0.0 => {
                let quarter = contraption::quarter(turn.angle_at(now));
                if let Some(w) = with_world(|w| w.windmill_disassembled(dim, pos, state)).flatten()
                {
                    put_back_turned(game, dimension, pos, facing, quarter, &w);
                }
            }
            Some(Some(mut turn)) => {
                turn.rebase(now, speed);
                with_world(|w| {
                    if let Some(m) = w.dims[dim].windmills.get_mut(&pos) {
                        m.bearing = Some(turn);
                    }
                    w.dims[dim].windmill_changes.push(pos);
                });
            }
            Some(None) => {}
            None if speed != 0.0 => {
                let gathered =
                    contraption::gather_bearing(pos, facing, 0, |p| contraption_found(game, p));
                let Ok((blocks, sails)) = gathered else {
                    continue;
                };
                if let Some(air) = game.pack.assets.registry.default_state("minecraft:air") {
                    for (o, _) in &blocks {
                        let p = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
                        game.native_set_block(
                            dimension,
                            p,
                            air,
                            super::native_hooks::flags::UPDATE_CLIENTS,
                        );
                    }
                }
                let w = create_engine::world::Windmill {
                    blocks,
                    sails,
                    clockwise: true,
                    bearing: Some(Turn {
                        angle: 0.0,
                        since: now,
                        rpm: speed,
                    }),
                };
                with_world(|world| world.bearing_assembled(dim, pos, w));
            }
            None => {}
        }
    }
}

/// A block state turned `quarter`° about `axis` (`StructureTransform`): its `facing` or `axis`
/// turned with it (`rotateFacing`: `getClockWise(axis)`, `rotation.ordinal()` times); a state
/// that has no such turned form stays as it was.
pub(super) fn turned_state(game: &Game<'_>, state: u32, quarter: i32, axis: Axis) -> u32 {
    let turns = create_engine::contraption::clockwise_turns(quarter);
    if turns == 0 {
        return state;
    }
    let registry = game.pack.assets.registry;
    let Some(mut s) = registry.state(BlockStateId(state)).cloned() else {
        return state;
    };
    let turn_dir = |d: Dir| (0..turns).fold(d, |d, _| create_engine::wrench::clockwise(d, axis));
    if let Some(d) = s
        .properties
        .get("facing")
        .and_then(|v| Dir::ALL.into_iter().find(|d| dir_name(*d) == v))
    {
        s.properties
            .insert("facing".into(), dir_name(turn_dir(d)).into());
    } else if let Some(a) = s.properties.get("axis").cloned() {
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
        s.properties.insert("axis".into(), name.into());
    }
    registry.id(&s).map_or(state, |t| t.0)
}

/// `ControlledContraptionEntity.disassemble` with its `StructureTransform`: each block turned
/// `quarter`° about the bearing's axis through the block in front of it ([`turned_state`]). A
/// block whose place is taken drops.
pub(super) fn put_back_turned(
    game: &mut Game<'_>,
    dimension: &str,
    bearing: [i32; 3],
    facing: Dir,
    quarter: i32,
    w: &create_engine::world::Windmill,
) {
    use create_engine::contraption;
    let axis = facing.axis();
    let registry = game.pack.assets.registry;
    let f = facing.offset();
    let anchor = [bearing[0] + f[0], bearing[1] + f[1], bearing[2] + f[2]];
    let turned: Vec<([i32; 3], u32)> = w
        .blocks
        .iter()
        .map(|(o, state)| {
            let local = [o[0] - f[0], o[1] - f[1], o[2] - f[2]];
            let r = contraption::rotate(local, quarter, axis);
            let p = [anchor[0] + r[0], anchor[1] + r[1], anchor[2] + r[2]];
            (p, turned_state(game, *state, quarter, axis))
        })
        .collect();
    for (p, id) in turned {
        let here = game.world.block(p[0], p[1], p[2]);
        let free = registry
            .state(here)
            .is_some_and(|s| s.name == "minecraft:air" || s.name == "minecraft:cave_air");
        if free {
            game.native_set_block(
                dimension,
                p,
                BlockStateId(id),
                super::native_hooks::flags::UPDATE_ALL,
            );
        } else if let Some(s) = registry.state(BlockStateId(id)) {
            game.native_run_command(&format!(
                "execute in {dimension} run summon minecraft:item {}.5 {}.5 {}.5 {{Item:{{id:'{}',count:1}}}}",
                p[0], p[1], p[2], s.name
            ));
        }
    }
}

/// A gantry shaft's facing, part and power; None for any other block.
pub(super) fn gantry_shaft(
    game: &Game<'_>,
    p: [i32; 3],
) -> Option<(Dir, create_engine::gantry::Part, bool)> {
    let s = game
        .pack
        .assets
        .registry
        .state(game.world.block(p[0], p[1], p[2]))?;
    if s.name != "create:gantry_shaft" {
        return None;
    }
    let facing = s
        .properties
        .get("facing")
        .and_then(|v| Dir::ALL.into_iter().find(|d| dir_name(*d) == v))?;
    let part = create_engine::gantry::Part::read(s.properties.get("part")?)?;
    Some((
        facing,
        part,
        s.properties.get("powered").is_some_and(|v| v == "true"),
    ))
}

/// `checkAttachedCarriageBlocks`: carriages beside the shaft facing away from it, off its axis.
pub(super) fn gantry_carriages_on(game: &Game<'_>, dim: usize, shaft: [i32; 3]) {
    let Some((facing, ..)) = gantry_shaft(game, shaft) else {
        return;
    };
    let registry = game.pack.assets.registry;
    for d in Dir::ALL {
        if d.axis() == facing.axis() {
            continue;
        }
        let o = d.offset();
        let c = [shaft[0] + o[0], shaft[1] + o[1], shaft[2] + o[2]];
        let carriage = registry
            .state(game.world.block(c[0], c[1], c[2]))
            .is_some_and(|s| {
                s.name == "create:gantry_carriage"
                    && s.properties.get("facing").map(String::as_str) == Some(dir_name(d))
            });
        if carriage {
            with_world(|w| w.dims[dim].gantry_assemble.push(c));
        }
    }
}

/// Gantries: shafts take their place in their line (`updateShape`); queued carriages on a shaft
/// they may start from take their structure out of the world (`tryAssemble`); moving ones run
/// along the shafts at their speed (`checkPinionShaft`) and go back into the world, at the
/// nearest whole block, where they stop. Nothing runs without one of them.
pub(super) fn tick_gantries(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::gantry::{self, Moving, Shaft, Step};
    let Some((mut checks, mut queued, moving)) = with_world(|w| {
        let d = &mut w.dims[dim];
        (
            std::mem::take(&mut d.gantry_checks),
            std::mem::take(&mut d.gantry_assemble),
            !d.gantries.is_empty(),
        )
    }) else {
        return;
    };
    if checks.is_empty() && queued.is_empty() && !moving {
        return;
    }
    checks.sort();
    checks.dedup();
    for pos in checks {
        let Some((facing, part, _)) = gantry_shaft(game, pos) else {
            continue;
        };
        let along = |d: Dir| {
            let o = d.offset();
            gantry_shaft(game, [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]])
                .is_some_and(|(f, ..)| f == facing)
        };
        let now = gantry::Part::of(along(facing), along(facing.opposite()));
        if now != part {
            game.native_set_block_property_with(
                pos,
                "part",
                now.name(),
                super::native_hooks::flags::UPDATE_ALL,
            );
        }
    }
    queued.sort();
    queued.dedup();
    for c in queued {
        let state = game
            .pack
            .assets
            .registry
            .state(game.world.block(c[0], c[1], c[2]));
        let Some(cf) = state
            .filter(|s| s.name == "create:gantry_carriage")
            .and_then(|s| s.properties.get("facing"))
            .and_then(|v| Dir::ALL.into_iter().find(|d| dir_name(*d) == v))
        else {
            continue;
        };
        let b = cf.opposite().offset();
        let shaft = [c[0] + b[0], c[1] + b[1], c[2] + b[2]];
        let Some((sf, part, powered)) = gantry_shaft(game, shaft) else {
            continue;
        };
        let rpm = with_world(|w| w.speed(dim, shaft)).flatten().unwrap_or(0.0);
        let v = gantry::pinion_speed(rpm);
        if !gantry::can_assemble_on(part, powered, v) {
            continue;
        }
        let moving_dir = if v > 0.0 { sf } else { sf.opposite() };
        let gathered = create_engine::contraption::gather_gantry(c, cf, moving_dir, |p| {
            contraption_found(game, p)
        });
        let Ok(blocks) = gathered else {
            continue;
        };
        let carried = actors::carry(game, dimension, dim, c, c, &blocks, |p| p == shaft);
        if let Some(air) = game.pack.assets.registry.default_state("minecraft:air") {
            for (o, _) in &blocks {
                let p = [c[0] + o[0], c[1] + o[1], c[2] + o[2]];
                game.native_set_block(
                    dimension,
                    p,
                    air,
                    super::native_hooks::flags::UPDATE_CLIENTS,
                );
            }
        }
        with_world(|w| {
            w.dims[dim].gantries.push(Moving {
                start: c,
                carriage_facing: cf,
                shaft_facing: sf,
                offset: 0.0,
                per_tick: 0.0,
                blocks,
                carried,
            })
        });
    }
    let list = with_world(|w| std::mem::take(&mut w.dims[dim].gantries)).unwrap_or_default();
    let mut keep = Vec::new();
    for mut g in list {
        // Its actors first; while one stalls, the gantry holds (`isStalled`).
        if !g.carried.actors.is_empty() {
            let o = g.shaft_facing.offset();
            let at =
                [0, 1, 2].map(|a| f64::from(g.start[a]) + f64::from(o[a]) * f64::from(g.offset));
            let stalled = actors::tick_actors(game, dimension, dim, g.start, &mut g.carried, at);
            g.carried.stalled = stalled;
            if stalled {
                if g.per_tick != 0.0 {
                    g.per_tick = 0.0;
                    send_gantry(game, dimension, &g, false);
                }
                keep.push(g);
                continue;
            }
        }
        let sp = g.shaft_pos();
        let shaft = gantry_shaft(game, sp).map(|(facing, part, powered)| Shaft {
            facing,
            part,
            powered,
            speed: with_world(|w| w.speed(dim, sp)).flatten().unwrap_or(0.0),
        });
        match g.step(shaft) {
            Step::Move(v) => {
                if v != g.per_tick {
                    g.per_tick = v;
                    send_gantry(game, dimension, &g, false);
                }
                g.offset += v;
                keep.push(g);
            }
            Step::Stop => {
                // `disassemble` at `BlockPos.containing(anchor + .5)`.
                let at = g.carriage();
                let shift = [at[0] - g.start[0], at[1] - g.start[1], at[2] - g.start[2]];
                let registry = game.pack.assets.registry;
                for (o, id) in &g.blocks {
                    let p = [
                        g.start[0] + o[0] + shift[0],
                        g.start[1] + o[1] + shift[1],
                        g.start[2] + o[2] + shift[2],
                    ];
                    let here = game.world.block(p[0], p[1], p[2]);
                    let free = registry.state(here).is_some_and(|s| {
                        s.name == "minecraft:air" || s.name == "minecraft:cave_air"
                    });
                    if free {
                        game.native_set_block(
                            dimension,
                            p,
                            BlockStateId(*id),
                            super::native_hooks::flags::UPDATE_ALL,
                        );
                    } else if let Some(s) = registry.state(BlockStateId(*id)) {
                        game.native_run_command(&format!(
                            "execute in {dimension} run summon minecraft:item {}.5 {}.5 {}.5 {{Item:{{id:'{}',count:1}}}}",
                            p[0], p[1], p[2], s.name
                        ));
                    }
                }
                actors::release(game, dimension, dim, g.start, at, &mut g.carried);
                send_gantry(game, dimension, &g, true);
            }
        }
    }
    with_world(|w| {
        let d = &mut w.dims[dim];
        keep.extend(std::mem::take(&mut d.gantries));
        d.gantries = keep;
    });
}

/// A moving gantry to clients, as a piston's structure is (`create:piston`, keyed by where its
/// carriage started): gliding along the shaft line from its offset at its speed; `gone` clears
/// it.
pub(super) fn send_gantry(
    game: &mut Game<'_>,
    dimension: &str,
    g: &create_engine::gantry::Moving,
    gone: bool,
) {
    let mut msg = Vec::new();
    for v in g.start {
        msg.extend_from_slice(&v.to_le_bytes());
    }
    if !gone {
        for v in g.start {
            msg.extend_from_slice(&v.to_le_bytes());
        }
        msg.push(g.shaft_facing as u8);
        msg.extend_from_slice(&g.offset.to_le_bytes());
        msg.extend_from_slice(&g.per_tick.to_le_bytes());
        let blocks = &g.blocks[..g.blocks.len().min((30_000 - 33) / 16)];
        msg.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
        for (o, s) in blocks {
            for v in o {
                msg.extend_from_slice(&v.to_le_bytes());
            }
            msg.extend_from_slice(&s.to_le_bytes());
        }
    }
    for p in players_in(game, dimension) {
        let _ = game.native_net_send_to_player(p, PISTON_CHANNEL, &msg);
    }
}

pub(super) const CONTRAPTION_CHANNEL: &str = "create:contraption";
pub(super) const PISTON_CHANNEL: &str = "create:piston";

/// A block's world position on a piston's structure at `offset` blocks out.
pub(super) fn piston_world(
    anchor: [i32; 3],
    local: [i32; 3],
    facing: Dir,
    offset: i32,
) -> [i32; 3] {
    let o = facing.offset();
    [0, 1, 2].map(|a| anchor[a] + local[a] + o[a] * offset)
}

/// Mechanical pistons (`LinearActuatorBlockEntity.tick`): a speed change makes a still piston
/// assemble or a moving one stop (`assembleNextTick`); moving ones slide `getMovementSpeed`
/// a tick and are put back at their end (`tryDisassemble`). Clients glide the structure.
pub(super) fn tick_pistons(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].piston_checks)).unwrap_or_default();
    let moving: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].pistons.keys().copied().collect()).unwrap_or_default();
    if checks.is_empty() && moving.is_empty() {
        return;
    }
    checks.sort();
    checks.dedup();
    for pos in checks {
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let running = with_world(|w| w.dims[dim].pistons.contains_key(&pos)).unwrap_or(false);
        match (running, speed != 0.0) {
            (false, true) => assemble_piston(game, dimension, dim, pos, speed),
            (true, false) => disassemble_piston(game, dimension, dim, pos),
            (true, true) => send_piston(game, dimension, dim, pos),
            _ => {}
        }
    }
    for pos in moving {
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let Some(facing) =
            with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten()
        else {
            continue;
        };
        // Its actors first (the contraption entity ticks before the piston's block entity);
        // while one stalls, the piston holds (`isStalled`).
        let stalled = tick_piston_actors(game, dimension, dim, pos, facing);
        let end = with_world(|w| {
            let m = w.dims[dim].pistons.get_mut(&pos)?;
            if !stalled {
                m.offset += create_engine::piston::movement(speed, facing, m.offset, m.range);
            }
            Some(m.offset <= 0.0 || m.offset >= m.range as f32)
        })
        .flatten();
        if end == Some(true) {
            disassemble_piston(game, dimension, dim, pos);
        }
    }
}

/// A moving piston's actors at its offset now; clients hear when it stalls or moves on. Only
/// pistons carrying actors do anything.
fn tick_piston_actors(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    facing: Dir,
) -> bool {
    let Some((mut carried, anchor, offset)) = with_world(|w| {
        let m = w.dims[dim].pistons.get_mut(&pos)?;
        if m.carried.actors.is_empty() {
            return None;
        }
        Some((std::mem::take(&mut m.carried), m.anchor, m.offset))
    })
    .flatten() else {
        return false;
    };
    let o = facing.offset();
    let at = [0, 1, 2].map(|a| f64::from(anchor[a]) + f64::from(o[a]) * f64::from(offset));
    let stalled = actors::tick_actors(game, dimension, dim, pos, &mut carried, at);
    let changed = stalled != carried.stalled;
    carried.stalled = stalled;
    let back = with_world(|w| match w.dims[dim].pistons.get_mut(&pos) {
        Some(m) => {
            m.carried = carried;
            true
        }
        None => false,
    })
    .unwrap_or(false);
    if back && changed {
        send_piston(game, dimension, dim, pos);
    }
    stalled
}

/// `MechanicalPistonBlockEntity.assemble`: the head, poles and structure leave the world.
pub(super) fn assemble_piston(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    speed: f32,
) {
    use create_engine::piston::{self, PoleCell};
    let registry = game.pack.assets.registry;
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some((block, facing)) =
        with_world(|w| w.table.get(state.0).map(|(b, i)| (b, i.facing))).flatten()
    else {
        return;
    };
    let Some(facing) = facing else {
        return;
    };
    let extended = registry
        .state(state)
        .is_some_and(|s| s.properties.get("state").is_some_and(|v| v == "extended"));
    let cell = |p: [i32; 3]| {
        let s = game.world.block(p[0], p[1], p[2]);
        match with_world(|w| w.table.get(s.0).map(|(b, i)| (b, i.facing))).flatten() {
            Some((create_engine::Block::PistonExtensionPole, Some(f))) => PoleCell::Pole(f.axis()),
            Some((create_engine::Block::MechanicalPistonHead, Some(f))) => PoleCell::Head(f),
            _ => PoleCell::Other,
        }
    };
    let Some(c) = piston::collect(pos, facing, extended, cell) else {
        return;
    };
    let m = piston::movement(speed, facing, c.progress as f32, c.range);
    let result = c.progress as f32 + m.signum() * 0.5;
    if m == 0.0 || result <= 0.0 || result >= c.range as f32 {
        return;
    }
    // The poles' and head's own states, then the structure in front (searched from the anchor,
    // pushed the way it moves).
    let sticky = block == create_engine::Block::StickyMechanicalPiston;
    let head = state_with(
        registry,
        "create:mechanical_piston_head",
        &[
            ("facing", dir_name(facing)),
            ("type", if sticky { "sticky" } else { "normal" }),
        ],
    );
    let pole = state_with(
        registry,
        "create:piston_extension_pole",
        &[("facing", dir_name(facing))],
    );
    let (Some(head), Some(pole)) = (head, pole) else {
        return;
    };
    let pole_worlds: Vec<[i32; 3]> = c
        .poles
        .iter()
        .map(|(l, _)| piston_world(c.anchor, *l, facing, c.progress))
        .collect();
    let forced = if m > 0.0 { facing } else { facing.opposite() };
    let mut exclude = pole_worlds.clone();
    exclude.push(pos);
    let Ok(structure) = piston::gather(c.anchor, forced, &exclude, |p| contraption_found(game, p))
    else {
        return;
    };
    let o = facing.offset();
    let mut blocks: Vec<([i32; 3], u32)> = c
        .poles
        .iter()
        .map(|(l, is_head)| (*l, if *is_head { head.0 } else { pole.0 }))
        .collect();
    for (off, s) in &structure {
        blocks.push(([0, 1, 2].map(|a| off[a] - o[a] * c.progress), *s));
    }
    // What rides with it (actors, storage, glue; offsets as the blocks', from the anchor at
    // offset 0), then off the world: everything but the piston.
    let origin = piston_world(c.anchor, [0; 3], facing, c.progress);
    let local: Vec<([i32; 3], u32)> = blocks
        .iter()
        .filter(|(l, _)| piston_world(c.anchor, *l, facing, c.progress) != pos)
        .copied()
        .collect();
    let carried = actors::carry(game, dimension, dim, pos, origin, &local, |p| p == pos);
    let air = BlockStateId(0);
    for (l, _) in &blocks {
        let p = piston_world(c.anchor, *l, facing, c.progress);
        if p != pos {
            game.native_set_block(dimension, p, air, super::native_hooks::flags::UPDATE_ALL);
        }
    }
    game.native_set_block_property(pos, "state", "moving");
    with_world(|w| {
        w.dims[dim].pistons.insert(
            pos,
            create_engine::world::Moving {
                anchor: c.anchor,
                blocks,
                offset: c.progress as f32,
                range: c.range,
                carried,
            },
        )
    });
    send_piston(game, dimension, dim, pos);
}

/// `tryDisassemble`: the structure goes back into the world at the nearest whole offset; the
/// piston is extended unless all the way in.
pub(super) fn disassemble_piston(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    let Some(m) = with_world(|w| w.dims[dim].pistons.remove(&pos)).flatten() else {
        return;
    };
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some(facing) = with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten()
    else {
        return;
    };
    let mut m = m;
    let grid = m.offset.round() as i32;
    for (l, s) in &m.blocks {
        let p = piston_world(m.anchor, *l, facing, grid);
        if p == pos {
            continue;
        }
        game.native_set_block(
            dimension,
            p,
            BlockStateId(*s),
            super::native_hooks::flags::UPDATE_ALL,
        );
    }
    game.native_set_block_property(
        pos,
        "state",
        if grid > 0 { "extended" } else { "retracted" },
    );
    let o = facing.offset();
    let origin = [0, 1, 2].map(|a| m.anchor[a] + o[a] * grid);
    actors::release(game, dimension, dim, pos, origin, &mut m.carried);
    let players = players_in(game, dimension);
    let mut msg = Vec::new();
    for v in pos {
        msg.extend_from_slice(&v.to_le_bytes());
    }
    for p in players {
        let _ = game.native_net_send_to_player(p, PISTON_CHANNEL, &msg);
    }
}

/// A moving piston for clients: piston, anchor, facing, offset now, blocks a tick, then each
/// block (offset from the anchor at offset 0, state: 16 bytes). Just the position: it stopped.
pub(super) fn send_piston(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    let Some(m) = with_world(|w| w.dims[dim].pistons.get(&pos).cloned()).flatten() else {
        return;
    };
    let state = game.world.block(pos[0], pos[1], pos[2]);
    let Some(facing) = with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten()
    else {
        return;
    };
    let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
    // A stalled contraption holds still (`ContraptionStallPacket`).
    let per_tick = if m.carried.stalled {
        0.0
    } else {
        create_engine::piston::movement(speed, facing, m.offset, m.range)
    };
    let mut msg = Vec::new();
    for v in pos.iter().chain(m.anchor.iter()) {
        msg.extend_from_slice(&v.to_le_bytes());
    }
    msg.push(facing as u8);
    msg.extend_from_slice(&m.offset.to_le_bytes());
    msg.extend_from_slice(&per_tick.to_le_bytes());
    let blocks = &m.blocks[..m.blocks.len().min((30_000 - 33) / 16)];
    msg.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    for (o, s) in blocks {
        for v in o {
            msg.extend_from_slice(&v.to_le_bytes());
        }
        msg.extend_from_slice(&s.to_le_bytes());
    }
    for p in players_in(game, dimension) {
        let _ = game.native_net_send_to_player(p, PISTON_CHANNEL, &msg);
    }
}

thread_local! {
    /// Pistons' moving structures a client draws: their part model.
    pub(super) static PISTONS: RefCell<HashMap<[i32; 3], u32>> = RefCell::new(HashMap::new());
}

/// A piston's structure on the client: one part model gliding along the facing on the GPU from
/// its offset now, at its speed (`ControlledContraptionEntity`'s motion).
pub(super) fn pistons_from_server(game: &mut Game<'_>, d: &[u8]) {
    let i = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    if d.len() < 12 {
        return;
    }
    let pos = [i(&d[0..4]), i(&d[4..8]), i(&d[8..12])];
    let key = depot_key(pos, 221, 0);
    if let Some(old) = PISTONS.with_borrow_mut(|c| c.remove(&pos)) {
        game.native_part_clear(key);
        game.native_part_model_free(old);
    }
    if d.len() < 37 {
        return;
    }
    let anchor = [i(&d[12..16]), i(&d[16..20]), i(&d[20..24])];
    let facing = Dir::ALL[usize::from(d[24].min(5))];
    let f = |b: &[u8]| f32::from_le_bytes(b.try_into().unwrap());
    let (offset, per_tick) = (f(&d[25..29]), f(&d[29..33]));
    let n = u32::from_le_bytes(d[33..37].try_into().unwrap()) as usize;
    let blocks: Vec<(BlockStateId, glam::Mat4)> = d[37..]
        .chunks_exact(16)
        .take(n)
        .map(|r| {
            let o = glam::Vec3::new(i(&r[0..4]) as f32, i(&r[4..8]) as f32, i(&r[8..12]) as f32);
            (
                BlockStateId(u32::from_le_bytes(r[12..16].try_into().unwrap())),
                glam::Mat4::from_translation(o),
            )
        })
        .collect();
    let Some(model) = game.native_part_model_compound(&blocks) else {
        return;
    };
    PISTONS.with_borrow_mut(|c| c.insert(pos, model));
    let o = facing.offset().map(|v| v as f32);
    let now = game.native_parts_seconds();
    game.native_part_glide(
        key,
        model,
        anchor,
        0,
        0.0,
        0.0,
        [o[0] * offset, o[1] * offset, o[2] * offset, now],
        [
            o[0] * per_tick * 20.0,
            o[1] * per_tick * 20.0,
            o[2] * per_tick * 20.0,
        ],
    );
}

/// A windmill's structure for clients: bearing, facing, then each block's offset and state (16
/// bytes); none when it came apart.
pub(super) fn send_windmills(
    game: &mut Game<'_>,
    players: &[u128],
    dim: usize,
    bearings: &[[i32; 3]],
) {
    for &pos in bearings {
        let w = with_world(|w| w.dims[dim].windmills.get(&pos).cloned()).flatten();
        let facing = with_world(|w| {
            w.table
                .get(game.world.block(pos[0], pos[1], pos[2]).0)
                .and_then(|(_, i)| i.facing)
        })
        .flatten()
        .unwrap_or(Dir::Up);
        let mut m = Vec::new();
        for v in pos {
            m.extend_from_slice(&v.to_le_bytes());
        }
        m.push(facing as u8);
        let w_turn = w.as_ref().and_then(|w| w.bearing);
        let blocks = w.map(|w| w.blocks).unwrap_or_default();
        let blocks = &blocks[..blocks.len().min((30_000 - 17) / 16)];
        m.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
        for (o, s) in blocks {
            for v in o {
                m.extend_from_slice(&v.to_le_bytes());
            }
            m.extend_from_slice(&s.to_le_bytes());
        }
        // Its angle now (a windmill starts at 0), for the client to turn on from.
        let now = game.world.game_time;
        let angle = w_turn.map_or(0.0, |t| t.angle_at(now));
        m.extend_from_slice(&angle.to_le_bytes());
        for &p in players {
            let _ = game.native_net_send_to_player(p, CONTRAPTION_CHANNEL, &m);
        }
    }
}

thread_local! {
    /// Structures a client draws, by bearing: their part model, the axis they turn on, and their
    /// angle at a clock time with the speed since, so a speed change turns on from where it is.
    pub(super) static CONTRAPTIONS: RefCell<HashMap<[i32; 3], (u32, Dir, f32, f32, f32)>> = RefCell::new(HashMap::new());
}

pub(super) fn contraptions_from_server(game: &mut Game<'_>, d: &[u8]) {
    let i = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    if d.len() < 17 {
        return;
    }
    let pos = [i(&d[0..4]), i(&d[4..8]), i(&d[8..12])];
    let facing = Dir::ALL[usize::from(d[12].min(5))];
    let n = u32::from_le_bytes(d[13..17].try_into().unwrap()) as usize;
    let key = depot_key(pos, 220, 0);
    if let Some((old, ..)) = CONTRAPTIONS.with_borrow_mut(|c| c.remove(&pos)) {
        game.native_part_clear(key);
        game.native_part_model_free(old);
    }
    let blocks: Vec<(BlockStateId, glam::Mat4)> = d[17..]
        .chunks_exact(16)
        .take(n)
        .map(|r| {
            let o = glam::Vec3::new(i(&r[0..4]) as f32, i(&r[4..8]) as f32, i(&r[8..12]) as f32);
            (
                BlockStateId(u32::from_le_bytes(r[12..16].try_into().unwrap())),
                glam::Mat4::from_translation(o),
            )
        })
        .collect();
    if blocks.is_empty() {
        return;
    }
    let Some(model) = game.native_part_model_compound(&blocks) else {
        return;
    };
    let angle = d
        .get(17 + 16 * n..21 + 16 * n)
        .map_or(0.0, |b| f32::from_le_bytes(b.try_into().unwrap()));
    let now = game.native_parts_seconds();
    let rpm = press_speed(pos);
    CONTRAPTIONS.with_borrow_mut(|c| c.insert(pos, (model, facing, angle, now, rpm)));
    turn_contraption(game, pos, rpm);
}

/// The structure turns with its bearing about the bearing's axis (through its centre).
pub(super) fn turn_contraption(game: &mut Game<'_>, pos: [i32; 3], rpm: f32) {
    let Some((model, facing, angle, at, old)) = CONTRAPTIONS.with_borrow(|c| c.get(&pos).copied())
    else {
        return;
    };
    // Where it is now at its old speed (6° a second per rpm), then on from there.
    let now = game.native_parts_seconds();
    let here = (angle + old * 6.0 * (now - at)) % 360.0;
    CONTRAPTIONS.with_borrow_mut(|c| c.insert(pos, (model, facing, here, now, rpm)));
    game.native_part_set(
        depot_key(pos, 220, 0),
        model,
        pos,
        axis_index(facing.axis()),
        rpm,
        (here - rpm * 6.0 * now) % 360.0,
    );
}

pub(super) fn windmills_to_nbt(
    game: &Game<'_>,
    all: &[([i32; 3], create_engine::world::Windmill)],
) -> horizon_world::nbt::Tag {
    use horizon_world::nbt::{Compound, Tag};
    let registry = game.pack.assets.registry;
    Tag::List(
        all.iter()
            .map(|(p, w)| {
                let mut c = Compound::new();
                c.insert("pos".into(), Tag::IntArray(p.to_vec()));
                c.insert(
                    "blocks".into(),
                    Tag::IntArray(w.blocks.iter().flat_map(|(o, _)| *o).collect()),
                );
                c.insert(
                    "states".into(),
                    Tag::List(
                        w.blocks
                            .iter()
                            .map(|(_, s)| Tag::String(state_text(registry, *s).as_str().into()))
                            .collect(),
                    ),
                );
                // A mechanical bearing's `Angle` (now) and that it is one.
                let now = game.world.game_time;
                let angle = w.bearing.map_or(0.0, |t| t.angle_at(now));
                c.insert(
                    "sails".into(),
                    Tag::IntArray(vec![
                        w.sails as i32,
                        i32::from(w.clockwise),
                        angle.to_bits() as i32,
                        i32::from(w.bearing.is_some()),
                    ]),
                );
                Tag::Compound(c)
            })
            .collect(),
    )
}

pub(super) fn windmills_from_nbt(
    game: &Game<'_>,
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<([i32; 3], create_engine::world::Windmill)> {
    use horizon_world::nbt::Tag;
    let registry = game.pack.assets.registry;
    let Some(Tag::List(list)) = data.and_then(|d| d.get(&"windmills".into())) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|c| {
            let Tag::Compound(c) = c else { return None };
            let Some(Tag::IntArray(p)) = c.get(&"pos".into()) else {
                return None;
            };
            let Some(Tag::IntArray(offsets)) = c.get(&"blocks".into()) else {
                return None;
            };
            let Some(Tag::List(states)) = c.get(&"states".into()) else {
                return None;
            };
            let Some(Tag::IntArray(sails)) = c.get(&"sails".into()) else {
                return None;
            };
            let blocks = offsets
                .chunks_exact(3)
                .zip(states)
                .filter_map(|(o, s)| {
                    let Tag::String(s) = s else { return None };
                    Some((
                        [o[0], o[1], o[2]],
                        state_of_text(registry, &s.to_string_lossy())?,
                    ))
                })
                .collect();
            Some((
                [*p.first()?, *p.get(1)?, *p.get(2)?],
                create_engine::world::Windmill {
                    blocks,
                    sails: (*sails.first()?).max(0) as u32,
                    clockwise: sails.get(1) != Some(&0),
                    bearing: (sails.get(3) == Some(&1)).then(|| create_engine::contraption::Turn {
                        angle: f32::from_bits(sails.get(2).copied().unwrap_or(0) as u32),
                        since: game.world.game_time,
                        rpm: 0.0,
                    }),
                },
            ))
        })
        .collect()
}

// Clockwork bearings, rope pulleys and elevators (`lifts.rs` in the engine): their structures
// leave the world while they run and come back where they stop; clients draw each as one part
// turning or gliding on the GPU, told when it starts, stops or changes speed.

pub(super) const LIFT_CHANNEL: &str = "create:lift";
const CLOCK_FULL: u8 = 0;
const CLOCK_UPDATE: u8 = 1;
const PULLEY: u8 = 2;
const ELEVATOR_FULL: u8 = 3;
const ELEVATOR_UPDATE: u8 = 4;
const LIFT_GONE: u8 = 5;
/// The most blocks a message carries (Horizon's mod messages stay under 30,000 bytes).
const LIFT_BLOCKS: usize = (30_000 - 96) / 16;

/// `DimensionType.natural`: the overworld's and other dimensions' clocks, not the Nether's and
/// the End's (they run 24 times as fast).
fn natural(dim: usize) -> bool {
    dim == 0
}

/// A cell a moving structure passes (air, or a fluid; `canBeReplaced` with no collision).
fn passable(game: &Game<'_>, p: [i32; 3]) -> bool {
    let id = game.world.block(p[0], p[1], p[2]);
    id.0 == 0
        || game.pack.assets.registry.state(id).is_some_and(|s| {
            matches!(
                s.name.as_str(),
                "minecraft:air"
                    | "minecraft:cave_air"
                    | "minecraft:void_air"
                    | "minecraft:water"
                    | "minecraft:lava"
            )
        })
}

fn below(p: [i32; 3], n: i32) -> [i32; 3] {
    [p[0], p[1] - n, p[2]]
}

/// A Create block state's block and properties; the first line is the miss path of the
/// signal hooks.
fn lift_block(state: u32) -> Option<(create_engine::Block, create_engine::parts::StateInfo)> {
    states::index(state)?;
    WORLD.with(|w| {
        let w = w.try_borrow().ok()?;
        w.as_ref()?.table.get(state).map(|(b, i)| (b, *i))
    })
}

/// Blocks a structure took go back: each where it was, or dropped where something else took
/// its place (`Contraption.addBlocksToWorld`).
fn lift_put_back(game: &mut Game<'_>, dimension: &str, blocks: &[([i32; 3], u32)]) {
    let registry = game.pack.assets.registry;
    for (p, state) in blocks {
        if passable(game, *p) {
            game.native_set_block(
                dimension,
                *p,
                BlockStateId(*state),
                super::native_hooks::flags::UPDATE_ALL,
            );
        } else if let Some(s) = registry.state(BlockStateId(*state)) {
            game.native_run_command(&format!(
                "execute in {dimension} run summon minecraft:item {}.5 {}.5 {}.5 {{Item:{{id:'{}',count:1}}}}",
                p[0], p[1], p[2], s.name
            ));
        }
    }
}

/// `removeBlocksFromWorld`.
fn lift_take(game: &mut Game<'_>, dimension: &str, base: [i32; 3], blocks: &[([i32; 3], u32)]) {
    for (o, _) in blocks {
        let p = [base[0] + o[0], base[1] + o[1], base[2] + o[2]];
        game.native_set_block(
            dimension,
            p,
            BlockStateId(0),
            super::native_hooks::flags::UPDATE_CLIENTS,
        );
    }
}

fn mark_lift_unsaved(game: &mut Game<'_>, dimension: &str, pos: [i32; 3]) {
    game.native_mark_chunk_unsaved(dimension, [pos[0] >> 4, pos[2] >> 4]);
}

/// Clockwork bearings, rope pulleys, elevators and contacts, after the kinetic solve. Nothing
/// runs while none of them has anything to do.
pub(super) fn tick_lifts(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let busy = with_world(|w| {
        let l = &w.dims[dim].lifts;
        !(l.orphans.is_empty()
            && l.clock_checks.is_empty()
            && l.clocks.is_empty()
            && l.pulley_checks.is_empty()
            && l.pulleys.is_empty()
            && l.elevator_clicks.is_empty()
            && l.travelling.is_empty()
            && l.contact_checks.is_empty()
            && l.contact_ticks.is_empty()
            && l.changes.is_empty())
    });
    if busy != Some(true) {
        return;
    }
    let orphans =
        with_world(|w| std::mem::take(&mut w.dims[dim].lifts.orphans)).unwrap_or_default();
    for (pos, held) in orphans {
        let origin = match &held {
            create_engine::lifts::Held::Pulley(p) => {
                Some(p.placed_at(create_engine::pulley::grid_offset(p.offset, p.range)))
            }
            create_engine::lifts::Held::Elevator(e) => Some(e.placed_at()),
            _ => None,
        };
        lift_put_back(game, dimension, &held.put_back(pos));
        if let Some(origin) = origin
            && let Some(mut carried) = PULLEY_CARRIED.with_borrow_mut(|c| c.remove(&(dim, pos)))
        {
            actors::release(game, dimension, dim, pos, origin, &mut carried);
        }
        send_lift_gone(game, dimension, pos);
        mark_lift_unsaved(game, dimension, pos);
    }
    tick_clocks(game, dimension, dim);
    tick_pulleys(game, dimension, dim);
    tick_elevators(game, dimension, dim);
    tick_contacts(game, dimension, dim);
    let changes = with_world(|w| {
        let mut v = std::mem::take(&mut w.dims[dim].lifts.changes);
        v.sort();
        v.dedup();
        v
    })
    .unwrap_or_default();
    if !changes.is_empty() {
        let players = players_in(game, dimension);
        send_lifts(game, &players, dim, &changes);
    }
}

/// `ClockworkBearingBlockEntity.tick`: a bearing whose speed changed or that was clicked
/// (`assembleNextTick`) assembles if it isn't running, comes apart if it is and stopped; running
/// clocks turn their hands toward the day time.
fn tick_clocks(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::clock::Sky;
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].lifts.clock_checks)).unwrap_or_default();
    checks.sort();
    checks.dedup();
    for pos in checks {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let Some((create_engine::Block::ClockworkBearing, info)) = lift_block(state) else {
            continue;
        };
        let Some(facing) = info.facing else {
            continue;
        };
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let running = with_world(|w| w.dims[dim].lifts.clocks.contains_key(&pos)) == Some(true);
        if running {
            if speed == 0.0 {
                disassemble_clock(game, dimension, dim, pos);
            } else {
                with_world(|w| {
                    if let Some(c) = w.dims[dim].lifts.clocks.get_mut(&pos) {
                        c.rpm = speed;
                    }
                });
                send_clock(game, dimension, dim, pos, false);
            }
        } else {
            assemble_clock(game, dimension, dim, pos, facing, speed);
        }
    }
    let day_time = game.world.day_time;
    let natural = natural(dim);
    with_world(|w| {
        for c in w.dims[dim].lifts.clocks.values_mut() {
            if c.rpm != 0.0 {
                c.clock.tick(&Sky {
                    rpm: c.rpm,
                    day_time,
                    natural,
                    positive: c.facing.positive(),
                });
            }
        }
    });
}

/// `ClockworkBearingBlockEntity.assemble`: the hour hand and the minute hand leave the world.
fn assemble_clock(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    facing: Dir,
    speed: f32,
) {
    let Ok(Some(hands)) = create_engine::clock::gather(pos, facing, |p| contraption_found(game, p))
    else {
        return;
    };
    lift_take(game, dimension, pos, &hands.hour);
    if let Some((_, m)) = &hands.minute {
        lift_take(game, dimension, pos, m);
    }
    let c = create_engine::lifts::RunningClock {
        clock: create_engine::clock::Clock::default(),
        facing,
        rpm: speed,
        hour: hands.hour,
        minute: hands.minute,
    };
    with_world(|w| {
        w.dims[dim].lifts.clocks.insert(pos, c);
    });
    send_clock(game, dimension, dim, pos, true);
    mark_lift_unsaved(game, dimension, pos);
}

/// `ClockworkBearingBlockEntity.disassemble`: the hands turn back to 0 and go back.
fn disassemble_clock(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    let Some(c) = with_world(|w| w.dims[dim].lifts.clocks.remove(&pos)).flatten() else {
        return;
    };
    let held = create_engine::lifts::Held::Clock(c);
    lift_put_back(game, dimension, &held.put_back(pos));
    send_lift_gone(game, dimension, pos);
    mark_lift_unsaved(game, dimension, pos);
}

/// For a pulley not yet started: whether its block at `local` (offsets from `anchor`) is a
/// breaker that can break the block at `cell`, as `breaker_clears` once it runs.
fn breaker_below(
    game: &Game<'_>,
    dim: usize,
    blocks: &[([i32; 3], u32)],
    anchor: [i32; 3],
    local: [i32; 3],
    cell: [i32; 3],
) -> bool {
    use create_engine::actors::{Actor, Carried, Kind};
    let Some((_, state)) = blocks.iter().find(|(o, _)| *o == local) else {
        return false;
    };
    let Some((block, Some(facing))) =
        with_world(|w| w.table.get(*state).map(|(b, i)| (b, i.facing))).flatten()
    else {
        return false;
    };
    let Some(kind) = Kind::of(block) else {
        return false;
    };
    let one = Carried {
        actors: vec![Actor::new(local, kind, facing, 0)],
        ..Carried::default()
    };
    actors::breaker_clears(game, dim, anchor, &one, local, cell)
}

thread_local! {
    /// What running rope pulleys and elevators carry besides their blocks (`Contraption.actors`,
    /// storage, glue, controls), by dimension and pulley; only those whose structure has any.
    static PULLEY_CARRIED: RefCell<HashMap<(usize, [i32; 3]), create_engine::actors::Carried>> =
        RefCell::new(HashMap::new());
}

/// Where offset 0 of a pulley's structure is now: its anchor, `offset - initial` lower.
fn pulley_origin(p: &create_engine::pulley::Pulley, offset: f32) -> [f64; 3] {
    [
        f64::from(p.anchor[0]),
        f64::from(p.anchor[1]) - f64::from(offset - p.initial as f32),
        f64::from(p.anchor[2]),
    ]
}

/// `LinearActuatorBlockEntity.tick` for rope pulleys: speed changes start, stop or turn them
/// (`assembleNextTick`); running ones move and stop at their ends, against the world, or (an
/// empty rope) at the block they reach.
fn tick_pulleys(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::pulley::{Step, grid_offset};
    let mut checks =
        with_world(|w| std::mem::take(&mut w.dims[dim].lifts.pulley_checks)).unwrap_or_default();
    checks.sort();
    checks.dedup();
    for &pos in &checks {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        if !matches!(
            lift_block(state),
            Some((create_engine::Block::RopePulley, _))
        ) {
            continue;
        }
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let running = with_world(|w| {
            let p = w.dims[dim].lifts.pulleys.get_mut(&pos)?;
            if speed != 0.0 {
                p.speed_changed(speed);
            }
            Some(grid_offset(p.offset, p.range))
        })
        .flatten();
        match running {
            Some(grid) if speed == 0.0 => stop_pulley(game, dimension, dim, pos, grid),
            Some(_) => send_pulley(game, dimension, dim, pos),
            None if speed != 0.0 => assemble_pulley(game, dimension, dim, pos, speed),
            None => {}
        }
    }
    let moving: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].lifts.pulleys.keys().copied().collect()).unwrap_or_default();
    for pos in moving {
        // A pulley told of its speed this tick only answers it (`sendData`).
        if checks.contains(&pos) {
            continue;
        }
        let Some(mut p) = with_world(|w| w.dims[dim].lifts.pulleys.remove(&pos)).flatten() else {
            continue;
        };
        // Its actors first (`tickActors`); while one stalls, the pulley holds (`isStalled`).
        if let Some(mut carried) = PULLEY_CARRIED.with_borrow_mut(|c| c.remove(&(dim, pos))) {
            let at = pulley_origin(&p, p.offset);
            let stalled = actors::tick_actors(game, dimension, dim, pos, &mut carried, at);
            carried.stalled = stalled;
            PULLEY_CARRIED.with_borrow_mut(|c| c.insert((dim, pos), carried));
            if stalled {
                with_world(|w| w.dims[dim].lifts.pulleys.insert(pos, p));
                continue;
            }
        }
        let registry = game.pack.assets.registry;
        let step = p.step(
            p.rpm,
            pos,
            |c, l| {
                passable(game, c)
                    || PULLEY_CARRIED.with_borrow(|m| {
                        m.get(&(dim, pos))
                            .is_some_and(|k| actors::breaker_clears(game, dim, pos, k, l, c))
                    })
            },
            |c| {
                contraption_found(game, c).is_some_and(|f| f.movable)
                    && registry
                        .state(game.world.block(c[0], c[1], c[2]))
                        .is_none_or(|s| s.name != "create:rope" && s.name != "create:pulley_magnet")
            },
        );
        with_world(|w| w.dims[dim].lifts.pulleys.insert(pos, p));
        match step {
            Step::Move(_) => {}
            Step::Stop(grid) => stop_pulley(game, dimension, dim, pos, grid),
            Step::Grab(grid) => {
                stop_pulley(game, dimension, dim, pos, grid);
                with_world(|w| w.dims[dim].lifts.pulley_checks.push(pos));
            }
        }
    }
}

/// `PulleyBlockEntity.assemble`: the rope and magnet under the pulley set its offset; the
/// structure under them and the rope leave the world.
fn assemble_pulley(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3], speed: f32) {
    use create_engine::pulley::{self, MAX_ROPE_LENGTH, Pulley};
    let registry = game.pack.assets.registry;
    let rope_at = |game: &Game<'_>, p: [i32; 3]| {
        registry
            .state(game.world.block(p[0], p[1], p[2]))
            .filter(|s| s.name == "create:rope" || s.name == "create:pulley_magnet")
            .map(|s| s.properties.get("waterlogged").is_some_and(|v| v == "true"))
    };
    let mut i = 1;
    while i <= MAX_ROPE_LENGTH && rope_at(game, below(pos, i)).is_some() {
        i += 1;
    }
    let offset = i - 1;
    let range = pulley::extension_range(pos[1], game.world.shape.min_y);
    if (offset >= range && speed > 0.0) || (offset <= 0 && speed < 0.0) {
        return;
    }
    let anchor = below(pos, offset + 1);
    let Ok(blocks) = pulley::gather(anchor, offset, |p| contraption_found(game, p)) else {
        return;
    };
    let mut p = Pulley::new(anchor, offset, range, blocks);
    p.rpm = speed;
    // `isCollidingWithWorld` a block further down: it doesn't start.
    if speed > 0.0
        && !p.blocks.is_empty()
        && p.collides(offset as f32 + 1.0, true, &|c, l| {
            passable(game, c) || breaker_below(game, dim, &p.blocks, anchor, l, c)
        })
    {
        return;
    }
    // `removeRopes`: rope in water leaves water.
    let water = registry.default_state("minecraft:water");
    for j in 1..=offset {
        let c = below(pos, j);
        let wet = rope_at(game, c) == Some(true);
        let to = if wet {
            water.unwrap_or(BlockStateId(0))
        } else {
            BlockStateId(0)
        };
        game.native_set_block(dimension, c, to, super::native_hooks::flags::UPDATE_CLIENTS);
    }
    // Its actors, storage, glue and controls ride with it (`Contraption.addBlock`).
    let carried = actors::carry(game, dimension, dim, pos, anchor, &p.blocks, |_| false);
    if !carried.actors.is_empty() || !carried.controls.is_empty() || !carried.glue.is_empty() {
        PULLEY_CARRIED.with_borrow_mut(|c| c.insert((dim, pos), carried));
    }
    lift_take(game, dimension, anchor, &p.blocks);
    with_world(|w| w.dims[dim].lifts.pulleys.insert(pos, p));
    send_pulley(game, dimension, dim, pos);
    mark_lift_unsaved(game, dimension, pos);
}

/// `PulleyBlockEntity.disassemble` at whole offset `grid`: the magnet at the rope's end and
/// rope above it (`shouldCreateRopes`), then the structure under them.
fn stop_pulley(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3], grid: i32) {
    let Some(mut p) = with_world(|w| w.dims[dim].lifts.pulleys.remove(&pos)).flatten() else {
        return;
    };
    let registry = game.pack.assets.registry;
    let (rope_cells, magnet) = create_engine::pulley::rope_cells(pos, grid);
    for (c, name) in magnet
        .map(|m| (m, "create:pulley_magnet"))
        .into_iter()
        .chain(rope_cells.into_iter().map(|r| (r, "create:rope")))
    {
        // `getDestroySpeed != -1`: what is there breaks, unless it can't.
        if contraption_found(game, c).is_some_and(|f| !f.movable) {
            continue;
        }
        let wet = registry
            .state(game.world.block(c[0], c[1], c[2]))
            .is_some_and(|s| s.name == "minecraft:water");
        let w = if wet { "true" } else { "false" };
        if let Some(s) = state_with(registry, name, &[("waterlogged", w)]) {
            game.native_set_block(dimension, c, s, super::native_hooks::flags::UPDATE_CLIENTS);
        }
    }
    p.offset = grid as f32;
    let origin = p.placed_at(create_engine::pulley::grid_offset(p.offset, p.range));
    let held = create_engine::lifts::Held::Pulley(p);
    lift_put_back(game, dimension, &held.put_back(pos));
    // What rode with it goes back with its blocks (`disassemble`).
    let mut carried = PULLEY_CARRIED
        .with_borrow_mut(|c| c.remove(&(dim, pos)))
        .unwrap_or_default();
    actors::release(game, dimension, dim, pos, origin, &mut carried);
    send_lift_gone(game, dimension, pos);
    mark_lift_unsaved(game, dimension, pos);
}

/// Elevators: clicked pulleys assemble or come apart (`clicked`); travelling ones move toward
/// their column's floor and light its contact when they arrive; settled ones wait for a call.
fn tick_elevators(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let mut clicks =
        with_world(|w| std::mem::take(&mut w.dims[dim].lifts.elevator_clicks)).unwrap_or_default();
    clicks.sort();
    clicks.dedup();
    for pos in clicks {
        let running = with_world(|w| w.dims[dim].lifts.elevators.contains_key(&pos)) == Some(true);
        if running {
            disassemble_elevator(game, dimension, dim, pos);
        } else {
            assemble_elevator(game, dimension, dim, pos);
        }
    }
    let travelling: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].lifts.travelling.iter().copied().collect()).unwrap_or_default();
    let now = game.world.game_time;
    for pos in travelling {
        // Its actors first (`tickActors`); while one stalls, the elevator holds.
        if let Some(mut carried) = PULLEY_CARRIED.with_borrow_mut(|c| c.remove(&(dim, pos))) {
            let at = with_world(|w| {
                w.dims[dim].lifts.elevators.get(&pos).map(|e| {
                    [
                        f64::from(e.anchor[0]),
                        f64::from(e.anchor[1] + e.initial) - f64::from(e.offset),
                        f64::from(e.anchor[2]),
                    ]
                })
            })
            .flatten();
            let stalled = at
                .is_some_and(|at| actors::tick_actors(game, dimension, dim, pos, &mut carried, at));
            carried.stalled = stalled;
            PULLEY_CARRIED.with_borrow_mut(|c| c.insert((dim, pos), carried));
            if stalled {
                continue;
            }
        }
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let done = with_world(|w| {
            let l = &mut w.dims[dim].lifts;
            let e = l.elevators.get_mut(&pos)?;
            let target = l.columns.get(&e.column).and_then(|c| c.target);
            let t = e.tick(pos[1], speed, target);
            let column = e.column;
            let settled = e.settled(target);
            if settled {
                l.travelling.remove(&pos);
            }
            Some((t.reached.map(|y| [column.x, y, column.z]), settled))
        })
        .flatten();
        let Some((reached, settled)) = done else {
            with_world(|w| w.dims[dim].lifts.travelling.remove(&pos));
            continue;
        };
        // `triggerContact`: an elevator contact there that isn't lit lights on its next tick.
        if let Some(c) = reached
            && let Some((create_engine::Block::ElevatorContact, info)) =
                lift_block(game.world.block(c[0], c[1], c[2]).0)
            && !info.powering
        {
            with_world(|w| {
                let l = &mut w.dims[dim].lifts;
                l.contact_lit.insert(c);
                l.schedule(c, now);
            });
        }
        if settled {
            send_elevator(game, dimension, dim, pos, false);
            mark_lift_unsaved(game, dimension, pos);
        }
    }
}

/// `ElevatorPulleyBlockEntity.assemble`: the structure under the free blocks below the pulley,
/// with exactly one redstone contact facing sideways, leaves the world; its column's redstone
/// contacts become elevator contacts (`gatherAll`) and it is called to the floor it is at.
fn assemble_elevator(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    use create_engine::elevator::{self, Elevator};
    use create_engine::pulley::{self, MAX_ROPE_LENGTH};
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    if !matches!(
        lift_block(state),
        Some((create_engine::Block::ElevatorPulley, _))
    ) {
        return;
    }
    let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
    if speed == 0.0 {
        return;
    }
    let mut i = 1;
    while i <= MAX_ROPE_LENGTH && passable(game, below(pos, i)) {
        i += 1;
    }
    let offset = i - 1;
    let anchor = below(pos, offset + 1);
    let Ok(blocks) = pulley::gather(anchor, offset, |p| contraption_found(game, p)) else {
        return;
    };
    // `ElevatorContraption.capture`: its redstone contacts facing sideways; one, no more.
    let contacts: Vec<([i32; 3], Dir)> = blocks
        .iter()
        .filter_map(|(o, s)| match lift_block(*s) {
            Some((create_engine::Block::RedstoneContact, i)) => {
                i.facing.filter(|f| f.axis() != Axis::Y).map(|f| (*o, f))
            }
            _ => None,
        })
        .collect();
    let [(local, facing)] = contacts[..] else {
        return;
    };
    let (key, contact_y) = elevator::contraption_column(anchor, local, facing);
    let conflict = with_world(|w| {
        w.dims[dim]
            .lifts
            .columns
            .get(&key)
            .is_some_and(|c| c.active)
    });
    if conflict != Some(false) {
        return;
    }
    let carried = actors::carry(game, dimension, dim, pos, anchor, &blocks, |_| false);
    if !carried.actors.is_empty() || !carried.controls.is_empty() || !carried.glue.is_empty() {
        PULLEY_CARRIED.with_borrow_mut(|c| c.insert((dim, pos), carried));
    }
    lift_take(game, dimension, anchor, &blocks);
    let range = pulley::extension_range(pos[1], game.world.shape.min_y);
    let e = Elevator::new(pos[1], anchor, offset, range, key, contact_y, blocks);
    let target = e.floor_now(pos[1]);
    // `ElevatorColumn.gatherAll`: redstone contacts in the column facing it join it.
    let registry = game.pack.assets.registry;
    let (min_y, height) = (game.world.shape.min_y, game.world.shape.height);
    for y in min_y..min_y + height {
        let p = [key.x, y, key.z];
        let Some((create_engine::Block::RedstoneContact, i)) =
            lift_block(game.world.block(p[0], p[1], p[2]).0)
        else {
            continue;
        };
        if i.facing != Some(key.side) {
            continue;
        }
        let powered = if i.powered { "true" } else { "false" };
        if let Some(s) = state_with(
            registry,
            "create:elevator_contact",
            &[("facing", dir_name(key.side)), ("powered", powered)],
        ) {
            game.native_set_block(dimension, p, s, super::native_hooks::flags::UPDATE_ALL);
        }
    }
    let now = game.world.game_time;
    let here = [key.x, target, key.z];
    let light = matches!(
        lift_block(game.world.block(here[0], here[1], here[2]).0),
        Some((create_engine::Block::ElevatorContact, i)) if !i.powering
    );
    with_world(|w| {
        let l = &mut w.dims[dim].lifts;
        l.columns.entry(key).or_default().target = Some(target);
        l.set_active(key, true);
        l.elevators.insert(pos, e);
        l.travelling.insert(pos);
        // `lazyTick`: standing at its floor, it lights that floor's contact.
        if light {
            l.contact_lit.insert(here);
            l.schedule(here, now);
        }
    });
    send_elevator(game, dimension, dim, pos, true);
    mark_lift_unsaved(game, dimension, pos);
}

/// `ElevatorPulleyBlockEntity.disassemble`: its column rests; the structure goes back at the
/// nearest whole block (no rope: `shouldCreateRopes` is false).
fn disassemble_elevator(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    let Some(e) = with_world(|w| {
        let l = &mut w.dims[dim].lifts;
        l.travelling.remove(&pos);
        let e = l.elevators.remove(&pos)?;
        l.set_active(e.column, false);
        Some(e)
    })
    .flatten() else {
        return;
    };
    let origin = e.placed_at();
    let held = create_engine::lifts::Held::Elevator(e);
    lift_put_back(game, dimension, &held.put_back(pos));
    if let Some(mut carried) = PULLEY_CARRIED.with_borrow_mut(|c| c.remove(&(dim, pos))) {
        actors::release(game, dimension, dim, pos, origin, &mut carried);
    }
    send_lift_gone(game, dimension, pos);
    mark_lift_unsaved(game, dimension, pos);
}

/// Whether the block `facing` of a contact is a contact facing back
/// (`RedstoneContactBlock.hasValidContact`).
fn contact_faces_back(game: &Game<'_>, pos: [i32; 3], facing: Dir) -> bool {
    let o = facing.offset();
    let at = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
    let other = match lift_block(game.world.block(at[0], at[1], at[2]).0) {
        Some((
            create_engine::Block::RedstoneContact | create_engine::Block::ElevatorContact,
            i,
        )) => i.facing,
        _ => None,
    };
    create_engine::elevator::faces_back(facing, other)
}

/// Contacts: redstone contacts power while one faces them (`updateShape`); an elevator contact
/// given a signal calls the elevator to its floor (`neighborChanged`,
/// `callToContactAndUpdate`), and lights while one faces it or an elevator arrived at it
/// (`tick` after `scheduleActivation`).
fn tick_contacts(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::Block;
    let now = game.world.game_time;
    let Some((mut checks, due)) = with_world(|w| {
        let l = &mut w.dims[dim].lifts;
        let (due, later): (Vec<_>, Vec<_>) = l.contact_ticks.drain(..).partition(|t| t.1 <= now);
        l.contact_ticks = later;
        (std::mem::take(&mut l.contact_checks), due)
    }) else {
        return;
    };
    checks.sort();
    checks.dedup();
    let clients = super::native_hooks::flags::UPDATE_CLIENTS;
    for pos in checks {
        let Some((block, info)) = lift_block(game.world.block(pos[0], pos[1], pos[2]).0) else {
            continue;
        };
        let Some(facing) = info.facing else {
            continue;
        };
        let valid = contact_faces_back(game, pos, facing);
        match block {
            Block::RedstoneContact if info.powered != valid => {
                set_flags(
                    game,
                    dimension,
                    pos,
                    &[("powered", valid)],
                    super::native_hooks::flags::UPDATE_ALL,
                );
            }
            Block::ElevatorContact => {
                let signal = neighbor_signal(game, pos);
                if info.powered != signal {
                    set_flags(game, dimension, pos, &[("powered", signal)], clients);
                    if signal && !info.calling {
                        call_contact(game, dimension, dim, pos, facing);
                    }
                }
                // `updateShape` (the block it faces changed): a contact an elevator lit keeps
                // its light through other neighbour changes.
                let held =
                    with_world(|w| w.dims[dim].lifts.contact_held.contains(&pos)) == Some(true);
                if info.powering != valid && !(info.powering && held) {
                    with_world(|w| w.dims[dim].lifts.schedule(pos, now));
                }
            }
            _ => {}
        }
    }
    for (pos, _) in due {
        let Some((Block::ElevatorContact, info)) =
            lift_block(game.world.block(pos[0], pos[1], pos[2]).0)
        else {
            continue;
        };
        let lit = with_world(|w| w.dims[dim].lifts.contact_lit.remove(&pos)) == Some(true);
        let should = lit
            || info
                .facing
                .is_some_and(|f| contact_faces_back(game, pos, f));
        with_world(|w| {
            let held = &mut w.dims[dim].lifts.contact_held;
            if lit {
                held.insert(pos);
            } else {
                held.remove(&pos);
            }
        });
        if info.powering || should {
            set_flags(game, dimension, pos, &[("powering", should)], clients);
        }
        game.native_update_signal_neighbors(dimension, pos);
    }
}

/// `ElevatorContactBlock.callToContactAndUpdate`: this contact calls, the others of its column
/// stop calling and go dark on their next tick, and the column's elevator sets off to it.
fn call_contact(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3], facing: Dir) {
    let clients = super::native_hooks::flags::UPDATE_CLIENTS;
    set_flags(
        game,
        dimension,
        pos,
        &[("calling", true), ("powered", true)],
        clients,
    );
    game.native_update_signal_neighbors(dimension, pos);
    let key = create_engine::elevator::column_of(pos, facing);
    let others: Vec<i32> = with_world(|w| {
        w.dims[dim]
            .lifts
            .columns
            .get(&key)
            .map(|c| c.contacts.clone())
    })
    .flatten()
    .unwrap_or_default();
    let now = game.world.game_time;
    for y in others {
        let p = [pos[0], y, pos[2]];
        if p == pos {
            continue;
        }
        if !matches!(
            lift_block(game.world.block(p[0], p[1], p[2]).0),
            Some((create_engine::Block::ElevatorContact, _))
        ) {
            continue;
        }
        set_flags(game, dimension, p, &[("calling", false)], clients);
        with_world(|w| w.dims[dim].lifts.schedule(p, now));
    }
    with_world(|w| w.dims[dim].lifts.call(key, pos[1]));
}

/// A right click on a clockwork bearing, rope pulley or elevator pulley with an empty hand,
/// not sneaking (`useItemOn`): a running clock or elevator comes apart, else it assembles on
/// the next tick; a rope pulley looks at its speed again (`assembleNextTick`).
pub(super) fn use_lift(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    block: create_engine::Block,
    sneaking: bool,
    canceled: &mut bool,
) {
    if sneaking || game.native_held().is_some_and(|h| h.0 != "minecraft:air") {
        return;
    }
    *canceled = true;
    let dimension = game.dim.id().to_owned();
    match block {
        create_engine::Block::ClockworkBearing => {
            let running = with_world(|w| w.dims[dim].lifts.clocks.contains_key(&pos)) == Some(true);
            if running {
                disassemble_clock(game, &dimension, dim, pos);
            } else {
                with_world(|w| w.dims[dim].lifts.clock_checks.push(pos));
            }
        }
        create_engine::Block::RopePulley => {
            with_world(|w| w.dims[dim].lifts.pulley_checks.push(pos));
        }
        create_engine::Block::ElevatorPulley => {
            with_world(|w| w.dims[dim].lifts.elevator_clicks.push(pos));
        }
        _ => *canceled = false,
    }
}

/// `RedstoneContactBlock.getStateForPlacement` for a contact placed from its item.
pub(super) fn place_contact(game: &mut Game<'_>, pos: [i32; 3], clicked_face: Dir) {
    let sneaking = game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft);
    let ctx = create_engine::place::Context {
        yaw: game.yaw,
        pitch: game.pitch,
        sneaking,
        clicked_face,
    };
    let (facing, powered) =
        create_engine::place::redstone_contact(ctx, |d| contact_faces_back(game, pos, d));
    game.native_set_block_property(pos, "facing", dir_name(facing));
    game.native_set_block_property(pos, "powered", if powered { "true" } else { "false" });
}

/// A contact's signal toward the reader on `face` (`getSignal`), or None for other blocks.
pub(super) fn lift_signal(
    dimension: horizon_world::level::Dimension,
    pos: [i32; 3],
    state: u32,
    face: Dir,
) -> Option<i32> {
    let (block, info) = lift_block(state)?;
    match block {
        create_engine::Block::ElevatorContact => {
            let o = face.offset();
            let reader = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
            let dim = level_index(dimension);
            let reader_is_contact = WORLD.with(|w| {
                w.try_borrow()
                    .ok()
                    .and_then(|w| w.as_ref()?.block_at(dim, reader))
            }) == Some(create_engine::Block::ElevatorContact);
            Some(create_engine::elevator::contact_signal(
                info.powering,
                reader_is_contact,
            ))
        }
        create_engine::Block::RedstoneContact => {
            Some(create_engine::elevator::redstone_contact_signal(
                info.powered,
                info.facing?,
                face.opposite(),
            ))
        }
        _ => None,
    }
}

/// `isSignalSource`: an elevator contact while powering, a redstone contact while powered.
pub(super) fn lift_signal_source(state: u32) -> bool {
    match lift_block(state) {
        Some((create_engine::Block::ElevatorContact, i)) => i.powering,
        Some((create_engine::Block::RedstoneContact, i)) => i.powered,
        _ => false,
    }
}

fn lift_blocks_bytes(m: &mut Vec<u8>, blocks: &[([i32; 3], u32)]) {
    let blocks = &blocks[..blocks.len().min(LIFT_BLOCKS)];
    m.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    for (o, s) in blocks {
        for v in o {
            m.extend_from_slice(&v.to_le_bytes());
        }
        m.extend_from_slice(&s.to_le_bytes());
    }
}

fn lift_send(game: &mut Game<'_>, players: &[u128], m: &[u8]) {
    for &p in players {
        let _ = game.native_net_send_to_player(p, LIFT_CHANNEL, m);
    }
}

fn lift_head(kind: u8, pos: [i32; 3]) -> Vec<u8> {
    let mut m = vec![kind];
    for v in pos {
        m.extend_from_slice(&v.to_le_bytes());
    }
    m
}

/// A clock for clients: speed and angles; a full one also its facing, mode, whether its
/// dimension is natural, how far its hour hand reaches, then both hands' blocks.
fn clock_message(dim: usize, pos: [i32; 3], full: bool) -> Option<Vec<u8>> {
    let c = with_world(|w| w.dims[dim].lifts.clocks.get(&pos).cloned()).flatten()?;
    let mut m = lift_head(if full { CLOCK_FULL } else { CLOCK_UPDATE }, pos);
    m.extend_from_slice(&c.rpm.to_le_bytes());
    m.extend_from_slice(&c.clock.hour.to_le_bytes());
    m.extend_from_slice(&c.clock.minute.to_le_bytes());
    if full {
        m.push(c.facing as u8);
        m.push(c.clock.hands.index());
        m.push(u8::from(natural(dim)));
        m.extend_from_slice(&c.minute.as_ref().map_or(0, |(r, _)| *r).to_le_bytes());
        lift_blocks_bytes(&mut m, &c.hour);
        lift_blocks_bytes(&mut m, c.minute.as_ref().map_or(&[][..], |(_, b)| b));
    }
    Some(m)
}

fn send_clock(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3], full: bool) {
    if let Some(m) = clock_message(dim, pos, full) {
        let players = players_in(game, dimension);
        lift_send(game, &players, &m);
    }
}

/// A rope pulley for clients: its anchor, offsets, blocks a tick (positive down) and blocks.
fn pulley_message(dim: usize, pos: [i32; 3]) -> Option<Vec<u8>> {
    let p = with_world(|w| w.dims[dim].lifts.pulleys.get(&pos).cloned()).flatten()?;
    let mut m = lift_head(PULLEY, pos);
    for v in p.anchor {
        m.extend_from_slice(&v.to_le_bytes());
    }
    m.extend_from_slice(&p.initial.to_le_bytes());
    m.extend_from_slice(&p.range.to_le_bytes());
    m.extend_from_slice(&p.offset.to_le_bytes());
    m.extend_from_slice(&create_engine::pulley::movement(p.rpm).to_le_bytes());
    lift_blocks_bytes(&mut m, &p.blocks);
    Some(m)
}

fn send_pulley(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3]) {
    if let Some(m) = pulley_message(dim, pos) {
        let players = players_in(game, dimension);
        lift_send(game, &players, &m);
    }
}

/// An elevator for clients: where it is, its speed, the floor it is called to; a full one also
/// its anchor, column and blocks. The client runs its ticks on from there.
fn elevator_message(dim: usize, pos: [i32; 3], full: bool) -> Option<Vec<u8>> {
    let (e, target, rpm) = with_world(|w| {
        let l = &w.dims[dim].lifts;
        let e = l.elevators.get(&pos)?.clone();
        let target = l.columns.get(&e.column).and_then(|c| c.target);
        Some((e, target, w.speed(dim, pos).unwrap_or(0.0)))
    })
    .flatten()?;
    let mut m = lift_head(if full { ELEVATOR_FULL } else { ELEVATOR_UPDATE }, pos);
    m.extend_from_slice(&e.offset.to_le_bytes());
    m.extend_from_slice(&e.y.to_le_bytes());
    m.extend_from_slice(&e.prev.to_le_bytes());
    m.push(u8::from(e.arrived));
    m.extend_from_slice(&rpm.to_le_bytes());
    m.push(u8::from(target.is_some()));
    m.extend_from_slice(&target.unwrap_or(0).to_le_bytes());
    if full {
        for v in e.anchor.into_iter().chain([
            e.initial,
            e.range,
            e.column.x,
            e.column.z,
            e.contact_y,
            e.min_contact,
            e.max_contact,
        ]) {
            m.extend_from_slice(&v.to_le_bytes());
        }
        m.push(e.column.side as u8);
        lift_blocks_bytes(&mut m, &e.blocks);
    }
    Some(m)
}

fn send_elevator(game: &mut Game<'_>, dimension: &str, dim: usize, pos: [i32; 3], full: bool) {
    if let Some(m) = elevator_message(dim, pos, full) {
        let players = players_in(game, dimension);
        lift_send(game, &players, &m);
    }
}

fn send_lift_gone(game: &mut Game<'_>, dimension: &str, pos: [i32; 3]) {
    let players = players_in(game, dimension);
    lift_send(game, &players, &lift_head(LIFT_GONE, pos));
}

/// Every clock, pulley and elevator at `positions` to `players` as it is now (a joining
/// player, or ones that came back with their column), or gone.
pub(super) fn send_lifts(
    game: &mut Game<'_>,
    players: &[u128],
    dim: usize,
    positions: &[[i32; 3]],
) {
    for &pos in positions {
        let m = clock_message(dim, pos, true)
            .or_else(|| pulley_message(dim, pos))
            .or_else(|| elevator_message(dim, pos, true))
            .unwrap_or_else(|| lift_head(LIFT_GONE, pos));
        lift_send(game, players, &m);
    }
}

/// Everything running in a dimension, for a player who joins.
pub(super) fn send_all_lifts(game: &mut Game<'_>, player: u128, dim: usize) {
    let all: Vec<[i32; 3]> = with_world(|w| {
        let l = &w.dims[dim].lifts;
        l.clocks
            .keys()
            .chain(l.pulleys.keys())
            .chain(l.elevators.keys())
            .copied()
            .collect()
    })
    .unwrap_or_default();
    send_lifts(game, &[player], dim, &all);
}

/// Running clocks, pulleys and elevators of a column, saved under `create_lifts` (their block
/// states as text): `kind` 0 a clock, 1 a pulley, 2 an elevator, its numbers in `nums` (floats
/// as their bits), and its blocks.
pub(super) fn lifts_chunk_save(
    game: &Game<'_>,
    dim: usize,
    chunk: [i32; 2],
    data: &mut horizon_world::nbt::Compound,
) {
    use create_engine::lifts::Held;
    use horizon_world::nbt::{Compound, Tag};
    let all = with_world(|w| {
        let l = &w.dims[dim].lifts;
        let v = l.column(chunk[0], chunk[1]);
        // A column's floor call is kept with its elevator.
        let targets: Vec<Option<i32>> = v
            .iter()
            .map(|(_, h)| match h {
                Held::Elevator(e) => l.columns.get(&e.column).and_then(|c| c.target),
                _ => None,
            })
            .collect();
        (v, targets)
    });
    let Some((all, targets)) = all else {
        return;
    };
    if all.is_empty() {
        data.remove(&"create_lifts".into());
        return;
    }
    let registry = game.pack.assets.registry;
    let blocks_tags = |c: &mut Compound, suffix: &str, blocks: &[([i32; 3], u32)]| {
        c.insert(
            format!("blocks{suffix}").as_str().into(),
            Tag::IntArray(blocks.iter().flat_map(|(o, _)| *o).collect()),
        );
        c.insert(
            format!("states{suffix}").as_str().into(),
            Tag::List(
                blocks
                    .iter()
                    .map(|(_, s)| Tag::String(state_text(registry, *s).as_str().into()))
                    .collect(),
            ),
        );
    };
    let bits = |f: f32| f.to_bits() as i32;
    let list = all
        .iter()
        .zip(targets)
        .map(|((pos, held), target)| {
            let mut c = Compound::new();
            c.insert("pos".into(), Tag::IntArray(pos.to_vec()));
            let (kind, nums) = match held {
                Held::Clock(k) => {
                    blocks_tags(&mut c, "", &k.hour);
                    if let Some((_, m)) = &k.minute {
                        blocks_tags(&mut c, "2", m);
                    }
                    (
                        0,
                        vec![
                            k.facing as i32,
                            i32::from(k.clock.hands.index()),
                            bits(k.clock.hour),
                            bits(k.clock.minute),
                            bits(k.rpm),
                            k.minute.as_ref().map_or(0, |(r, _)| *r),
                        ],
                    )
                }
                Held::Pulley(p) => {
                    blocks_tags(&mut c, "", &p.blocks);
                    (
                        1,
                        vec![
                            p.anchor[0],
                            p.anchor[1],
                            p.anchor[2],
                            p.initial,
                            bits(p.offset),
                            p.range,
                            bits(p.rpm),
                        ],
                    )
                }
                Held::Elevator(e) => {
                    blocks_tags(&mut c, "", &e.blocks);
                    let y = e.y.to_bits();
                    (
                        2,
                        vec![
                            e.anchor[0],
                            e.anchor[1],
                            e.anchor[2],
                            e.initial,
                            bits(e.offset),
                            (y >> 32) as i32,
                            y as i32,
                            bits(e.prev),
                            i32::from(e.arrived),
                            e.range,
                            e.column.x,
                            e.column.z,
                            e.column.side as i32,
                            e.contact_y,
                            e.min_contact,
                            e.max_contact,
                            i32::from(target.is_some()),
                            target.unwrap_or(0),
                        ],
                    )
                }
            };
            c.insert("kind".into(), Tag::IntArray(vec![kind]));
            c.insert("nums".into(), Tag::IntArray(nums));
            Tag::Compound(c)
        })
        .collect();
    data.insert("create_lifts".into(), Tag::List(list));
}

/// What `lifts_chunk_save` wrote, and the floor calls of the elevators' columns.
#[allow(clippy::type_complexity)]
pub(super) fn lifts_from_nbt(
    game: &Game<'_>,
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<(
    [i32; 3],
    create_engine::lifts::Held,
    Option<(create_engine::elevator::ColumnKey, i32)>,
)> {
    use create_engine::lifts::{Held, RunningClock};
    use horizon_world::nbt::{Compound, Tag};
    let Some(Tag::List(list)) = data.and_then(|d| d.get(&"create_lifts".into())) else {
        return Vec::new();
    };
    let registry = game.pack.assets.registry;
    let blocks = |c: &Compound, suffix: &str| -> Option<Vec<([i32; 3], u32)>> {
        let Some(Tag::IntArray(o)) = c.get(&format!("blocks{suffix}").as_str().into()) else {
            return None;
        };
        let Some(Tag::List(s)) = c.get(&format!("states{suffix}").as_str().into()) else {
            return None;
        };
        Some(
            o.chunks_exact(3)
                .zip(s)
                .filter_map(|(o, s)| {
                    let Tag::String(s) = s else { return None };
                    Some((
                        [o[0], o[1], o[2]],
                        state_of_text(registry, &s.to_string_lossy())?,
                    ))
                })
                .collect(),
        )
    };
    let f = |i: i32| f32::from_bits(i as u32);
    let dir = |i: i32| Dir::ALL[i.clamp(0, 5) as usize];
    list.iter()
        .filter_map(|c| {
            let Tag::Compound(c) = c else { return None };
            let Some(Tag::IntArray(p)) = c.get(&"pos".into()) else {
                return None;
            };
            let Some(Tag::IntArray(n)) = c.get(&"nums".into()) else {
                return None;
            };
            let Some(Tag::IntArray(kind)) = c.get(&"kind".into()) else {
                return None;
            };
            let kind = *kind.first()?;
            let pos = [*p.first()?, *p.get(1)?, *p.get(2)?];
            let at = |i: usize| n.get(i).copied();
            let mut call = None;
            let held = match kind {
                0 => Held::Clock(RunningClock {
                    clock: create_engine::clock::Clock {
                        hour: f(at(2)?),
                        minute: f(at(3)?),
                        hands: create_engine::clock::Hands::from_index(at(1)? as u8),
                    },
                    facing: dir(at(0)?),
                    rpm: f(at(4)?),
                    hour: blocks(c, "")?,
                    minute: blocks(c, "2").map(|b| (at(5).unwrap_or(0), b)),
                }),
                1 => {
                    let mut x = create_engine::pulley::Pulley::new(
                        [at(0)?, at(1)?, at(2)?],
                        at(3)?,
                        at(5)?,
                        blocks(c, "")?,
                    );
                    x.offset = f(at(4)?);
                    x.rpm = f(at(6)?);
                    Held::Pulley(x)
                }
                2 => {
                    let column = create_engine::elevator::ColumnKey {
                        x: at(10)?,
                        z: at(11)?,
                        side: dir(at(12)?),
                    };
                    if at(16) == Some(1) {
                        call = Some((column, at(17)?));
                    }
                    Held::Elevator(create_engine::elevator::Elevator {
                        anchor: [at(0)?, at(1)?, at(2)?],
                        initial: at(3)?,
                        offset: f(at(4)?),
                        y: f64::from_bits((at(5)? as u32 as u64) << 32 | at(6)? as u32 as u64),
                        prev: f(at(7)?),
                        arrived: at(8)? != 0,
                        range: at(9)?,
                        column,
                        contact_y: at(13)?,
                        min_contact: at(14)?,
                        max_contact: at(15)?,
                        blocks: blocks(c, "")?,
                    })
                }
                _ => return None,
            };
            Some((pos, held, call))
        })
        .collect()
}

/// Saved structures wait for their blocks; elevators' columns keep their floor calls.
pub(super) fn load_lifts(
    dim: usize,
    saved: Vec<(
        [i32; 3],
        create_engine::lifts::Held,
        Option<(create_engine::elevator::ColumnKey, i32)>,
    )>,
) {
    with_world(|w| {
        let l = &mut w.dims[dim].lifts;
        let mut held = Vec::with_capacity(saved.len());
        for (pos, h, call) in saved {
            if let Some((key, y)) = call {
                l.columns.entry(key).or_default().target = Some(y);
            }
            held.push((pos, h));
        }
        l.load(held.into_iter());
    });
}

/// Clocks, pulleys and elevators a client draws: each structure one part model, turning or
/// gliding on the GPU, set again only when its motion changes.
#[derive(Default)]
pub(super) struct ClientLifts {
    clocks: HashMap<[i32; 3], ClientClock>,
    pulleys: HashMap<[i32; 3], ClientPulley>,
    elevators: HashMap<[i32; 3], ClientElevator>,
}

struct ClientClock {
    hour: Option<u32>,
    minute: Option<u32>,
    axis: Axis,
    positive: bool,
    natural: bool,
    rpm: f32,
    clock: create_engine::clock::Clock,
    /// The degrees a tick each hand is drawn turning at.
    drawn: Option<(f32, f32)>,
}

struct ClientPulley {
    model: Option<u32>,
    range: i32,
    offset: f32,
    per_tick: f32,
    since: f32,
    /// Rope drawn below the pulley, 1 to `ropes` blocks down.
    ropes: i32,
}

struct ClientElevator {
    model: Option<u32>,
    e: create_engine::elevator::Elevator,
    rpm: f32,
    target: Option<i32>,
    /// The blocks a tick it is drawn gliding at.
    drawn: f64,
}

thread_local! {
    pub(super) static LIFTS: RefCell<ClientLifts> = RefCell::new(ClientLifts::default());
}

const HOUR_SLOT: u8 = 223;
const MINUTE_SLOT: u8 = 224;
const LIFT_SLOT: u8 = 225;
const ROPE_SLOT: u8 = 226;

/// Whether the client draws a running clock at `pos` (its bearing's top turns with the hour
/// hand, not on its own).
pub(super) fn clock_shown(pos: [i32; 3]) -> bool {
    LIFTS.with_borrow(|l| l.clocks.contains_key(&pos))
}

fn lift_compound(
    game: &mut Game<'_>,
    blocks: &[([i32; 3], u32)],
    extra: Option<(BlockStateId, [i32; 3])>,
) -> Option<u32> {
    let list: Vec<(BlockStateId, glam::Mat4)> = blocks
        .iter()
        .map(|(o, s)| (BlockStateId(*s), *o))
        .chain(extra.map(|(s, o)| (s, o)))
        .map(|(s, o)| {
            (
                s,
                glam::Mat4::from_translation(glam::Vec3::new(
                    o[0] as f32,
                    o[1] as f32,
                    o[2] as f32,
                )),
            )
        })
        .collect();
    if list.is_empty() {
        return None;
    }
    game.native_part_model_compound(&list)
}

fn read_blocks(d: &[u8], at: &mut usize) -> Vec<([i32; 3], u32)> {
    let i = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    let Some(n) = d.get(*at..*at + 4) else {
        return Vec::new();
    };
    let n = u32::from_le_bytes(n.try_into().unwrap()) as usize;
    *at += 4;
    let end = (*at + n * 16).min(d.len());
    let v = d[*at..end]
        .chunks_exact(16)
        .map(|r| {
            (
                [i(&r[0..4]), i(&r[4..8]), i(&r[8..12])],
                u32::from_le_bytes(r[12..16].try_into().unwrap()),
            )
        })
        .collect();
    *at = end;
    v
}

/// Hides a clock, pulley or elevator the client draws at `pos`.
fn forget_lift(game: &mut Game<'_>, pos: [i32; 3]) {
    let (clock, pulley, elevator) = LIFTS.with_borrow_mut(|l| {
        (
            l.clocks.remove(&pos),
            l.pulleys.remove(&pos),
            l.elevators.remove(&pos),
        )
    });
    if let Some(c) = clock {
        game.native_part_clear(depot_key(pos, HOUR_SLOT, 0));
        game.native_part_clear(depot_key(pos, MINUTE_SLOT, 0));
        for m in [c.hour, c.minute].into_iter().flatten() {
            game.native_part_model_free(m);
        }
        // Its bearing's top is drawn on its own again.
        if let Some(mut parts) = CLIENT.with_borrow_mut(Option::take) {
            show(game, &mut parts, pos);
            CLIENT.with_borrow_mut(|c| *c = Some(parts));
        }
    }
    if let Some(p) = pulley {
        game.native_part_clear(depot_key(pos, LIFT_SLOT, 0));
        if let Some(m) = p.model {
            game.native_part_model_free(m);
        }
        for i in 1..=p.ropes {
            game.native_part_clear(depot_key(below(pos, i), ROPE_SLOT, 0));
        }
    }
    if let Some(e) = elevator {
        game.native_part_clear(depot_key(pos, LIFT_SLOT, 0));
        if let Some(m) = e.model {
            game.native_part_model_free(m);
        }
    }
}

/// The server's clocks, pulleys and elevators (`create:lift`).
pub(super) fn lifts_from_server(game: &mut Game<'_>, d: &[u8]) {
    let i = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    let f = |b: &[u8]| f32::from_le_bytes(b.try_into().unwrap());
    if d.len() < 13 {
        return;
    }
    let pos = [i(&d[1..5]), i(&d[5..9]), i(&d[9..13])];
    let body = &d[13..];
    match d[0] {
        CLOCK_FULL if body.len() >= 23 => {
            forget_lift(game, pos);
            let (rpm, hour, minute) = (f(&body[0..4]), f(&body[4..8]), f(&body[8..12]));
            let facing = Dir::ALL[usize::from(body[12].min(5))];
            let hands = create_engine::clock::Hands::from_index(body[13]);
            let natural = body[14] != 0;
            let mut at = 13 + 19;
            let hour_blocks = read_blocks(d, &mut at);
            let minute_blocks = read_blocks(d, &mut at);
            // The bearing's top turns with the hour hand (`BearingRenderer`).
            let top = state_with(
                game.pack.assets.registry,
                "create:part_bearing_top",
                &[("facing", dir_name(facing))],
            );
            let hour_model = lift_compound(game, &hour_blocks, top.map(|t| (t, [0, 0, 0])));
            let minute_model = lift_compound(game, &minute_blocks, None);
            LIFTS.with_borrow_mut(|l| {
                l.clocks.insert(
                    pos,
                    ClientClock {
                        hour: hour_model,
                        minute: minute_model,
                        axis: facing.axis(),
                        positive: facing.positive(),
                        natural,
                        rpm,
                        clock: create_engine::clock::Clock {
                            hour,
                            minute,
                            hands,
                        },
                        drawn: None,
                    },
                )
            });
            game.native_part_clear(client::key(pos, 1));
        }
        CLOCK_UPDATE if body.len() >= 12 => LIFTS.with_borrow_mut(|l| {
            if let Some(c) = l.clocks.get_mut(&pos) {
                c.rpm = f(&body[0..4]);
                c.clock.hour = f(&body[4..8]);
                c.clock.minute = f(&body[8..12]);
                c.drawn = None;
            }
        }),
        PULLEY if body.len() >= 28 => {
            let old_ropes = LIFTS.with_borrow(|l| l.pulleys.get(&pos).map(|p| p.ropes));
            let anchor = [i(&body[0..4]), i(&body[4..8]), i(&body[8..12])];
            let (initial, range) = (i(&body[12..16]), i(&body[16..20]));
            let (offset, per_tick) = (f(&body[20..24]), f(&body[24..28]));
            let had = LIFTS.with_borrow_mut(|l| l.pulleys.get_mut(&pos).map(|p| p.model));
            let model = match had {
                // The same structure at a new speed: its model stays.
                Some(m) => m,
                None => {
                    let mut at = 13 + 28;
                    let blocks = read_blocks(d, &mut at);
                    let magnet = game
                        .pack
                        .assets
                        .registry
                        .default_state("create:pulley_magnet");
                    lift_compound(game, &blocks, magnet.map(|m| (m, [0, 1, 0])))
                }
            };
            let now = game.native_parts_seconds();
            LIFTS.with_borrow_mut(|l| {
                l.pulleys.insert(
                    pos,
                    ClientPulley {
                        model,
                        range,
                        offset,
                        per_tick,
                        since: now,
                        ropes: old_ropes.unwrap_or(0),
                    },
                )
            });
            if let Some(m) = model {
                let o = (initial as f32 - offset) * 1.0;
                game.native_part_glide(
                    depot_key(pos, LIFT_SLOT, 0),
                    m,
                    anchor,
                    0,
                    0.0,
                    0.0,
                    [0.0, o, 0.0, now],
                    [0.0, -per_tick * 20.0, 0.0],
                );
            }
        }
        ELEVATOR_FULL | ELEVATOR_UPDATE if body.len() >= 26 => {
            let offset = f(&body[0..4]);
            let y = f64::from_le_bytes(body[4..12].try_into().unwrap());
            let prev = f(&body[12..16]);
            let arrived = body[16] != 0;
            let rpm = f(&body[17..21]);
            let target = (body[21] != 0).then(|| i(&body[22..26]));
            if d[0] == ELEVATOR_FULL && body.len() >= 26 + 41 {
                forget_lift(game, pos);
                let n = |k: usize| i(&body[26 + 4 * k..30 + 4 * k]);
                let mut at = 13 + 26 + 41;
                let blocks = read_blocks(d, &mut at);
                let model = lift_compound(game, &blocks, None);
                let e = create_engine::elevator::Elevator {
                    anchor: [n(0), n(1), n(2)],
                    initial: n(3),
                    offset,
                    y,
                    prev,
                    arrived,
                    range: n(4),
                    column: create_engine::elevator::ColumnKey {
                        x: n(5),
                        z: n(6),
                        side: Dir::ALL[usize::from(body[26 + 40].min(5))],
                    },
                    contact_y: n(7),
                    min_contact: n(8),
                    max_contact: n(9),
                    blocks: Vec::new(),
                };
                LIFTS.with_borrow_mut(|l| {
                    l.elevators.insert(
                        pos,
                        ClientElevator {
                            model,
                            e,
                            rpm,
                            target,
                            drawn: f64::NAN,
                        },
                    )
                });
            } else {
                LIFTS.with_borrow_mut(|l| {
                    if let Some(c) = l.elevators.get_mut(&pos) {
                        c.e.offset = offset;
                        c.e.y = y;
                        c.e.prev = prev;
                        c.e.arrived = arrived;
                        c.rpm = rpm;
                        c.target = target;
                        c.drawn = f64::NAN;
                    }
                });
            }
        }
        LIFT_GONE => forget_lift(game, pos),
        _ => {}
    }
}

/// Each client tick: clock hands turn on toward the day time (set again only when a hand
/// starts or stops), rope follows a pulley's magnet down a block at a time, and elevators run
/// on as the server runs them (set again when their speed changes).
pub(super) fn tick_client_lifts(game: &mut Game<'_>) {
    use create_engine::clock::Sky;
    let busy = LIFTS
        .with_borrow(|l| !(l.clocks.is_empty() && l.pulleys.is_empty() && l.elevators.is_empty()));
    if !busy {
        return;
    }
    let now = game.native_parts_seconds();
    let day_time = game.world.day_time;
    let mut sets: Vec<(u64, u32, [i32; 3], u8, f32, f32)> = Vec::new();
    LIFTS.with_borrow_mut(|l| {
        for (pos, c) in l.clocks.iter_mut() {
            let sky = Sky {
                rpm: c.rpm,
                day_time,
                natural: c.natural,
                positive: c.positive,
            };
            let speeds = c.clock.speeds(&sky);
            if c.drawn != Some(speeds) {
                c.drawn = Some(speeds);
                let axis = axis_index(c.axis);
                for (slot, model, angle, per_tick) in [
                    (HOUR_SLOT, c.hour, c.clock.hour, speeds.0),
                    (MINUTE_SLOT, c.minute, c.clock.minute, speeds.1),
                ] {
                    if let Some(m) = model {
                        // Degrees a tick to rpm (`convertToAngular`: 0.3° a tick per rpm).
                        let rpm = per_tick / 0.3;
                        let phase = (angle - rpm * 6.0 * now) % 360.0;
                        sets.push((depot_key(*pos, slot, 0), m, *pos, axis, rpm, phase));
                    }
                }
            }
            c.clock.tick(&sky);
        }
    });
    for (key, model, pos, axis, rpm, phase) in sets {
        game.native_part_set(key, model, pos, axis, rpm, phase);
    }
    // Rope: whole blocks between the pulley and its magnet.
    let rope = game.pack.assets.registry.default_state("create:rope");
    let rope_model = rope.and_then(|r| game.native_part_model(r));
    let mut ropes: Vec<([i32; 3], i32, i32)> = Vec::new();
    LIFTS.with_borrow_mut(|l| {
        for (pos, p) in l.pulleys.iter_mut() {
            let offset =
                (p.offset + p.per_tick * (now - p.since) * 20.0).clamp(0.0, p.range as f32);
            let want = (offset.ceil() as i32 - 1).max(0);
            if want != p.ropes {
                ropes.push((*pos, p.ropes, want));
                p.ropes = want;
            }
        }
    });
    for (pos, from, to) in ropes {
        for i in to + 1..=from {
            game.native_part_clear(depot_key(below(pos, i), ROPE_SLOT, 0));
        }
        if let Some(m) = rope_model {
            for i in from + 1..=to {
                let c = below(pos, i);
                game.native_part_set(depot_key(c, ROPE_SLOT, 0), m, c, 1, 0.0, 0.0);
            }
        }
    }
    // Elevators: their ticks run on here; the glide is set again when it changes.
    let mut glides: Vec<([i32; 3], u32, [i32; 3], f32, f32)> = Vec::new();
    LIFTS.with_borrow_mut(|l| {
        for (pos, c) in l.elevators.iter_mut() {
            let before = c.e.y;
            if !c.e.settled(c.target) {
                c.e.tick(pos[1], c.rpm, c.target);
            }
            let v = c.e.y - before;
            if (v - c.drawn).abs() > 1e-7 || c.drawn.is_nan() {
                c.drawn = v;
                if let Some(m) = c.model {
                    glides.push((
                        *pos,
                        m,
                        c.e.anchor,
                        (before - f64::from(c.e.anchor[1])) as f32,
                        (v * 20.0) as f32,
                    ));
                }
            }
        }
    });
    for (pos, model, anchor, offset, velocity) in glides {
        game.native_part_glide(
            depot_key(pos, LIFT_SLOT, 0),
            model,
            anchor,
            0,
            0.0,
            0.0,
            [0.0, offset, 0.0, now],
            [0.0, velocity, 0.0],
        );
    }
}

/// The client left its world: nothing is drawn any more.
pub(super) fn reset_client_lifts() {
    LIFTS.with_borrow_mut(|l| *l = ClientLifts::default());
}
