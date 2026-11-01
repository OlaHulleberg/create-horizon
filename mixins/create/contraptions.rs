// Create's contraptions in the game: bearings and windmills, pistons, chassis and gantries
// moving blocks as one, and their save data. A sibling of the `create` module (`runtime.rs`).
use super::Game;
use super::create::*;
use super::create_logistics::*;
use super::create_processing::*;
use create_engine::kinetics::{Axis, Dir};
use create_engine::world::dimension_index;
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
            "create:linear_chassis" | "create:secondary_linear_chassis"
        )
        .then(|| (s.name.clone(), s.properties.get("axis").cloned()))
    };
    let Some(own) = kind(pos) else {
        return Vec::new();
    };
    let mut group = vec![pos];
    let mut i = 0;
    while i < group.len() && group.len() < 256 {
        let p = group[i];
        i += 1;
        for d in Dir::ALL {
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
    // getGlueableSide: an end on its axis, sticky_top toward the positive end.
    let side_of = |f: [i32; 3]| -> Option<&'static str> {
        let i = ["x", "y", "z"].iter().position(|a| *a == axis)?;
        match f[i] {
            1 => Some("sticky_top"),
            -1 => Some("sticky_bottom"),
            _ => None,
        }
    };
    let Some(side) = side_of(face) else {
        return;
    };
    let allowed = |game: &Game<'_>, f: [i32; 3]| {
        registry
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
    ) || game.world.block_entities.contains_key(&(p[0], p[1], p[2]));
    Some(Found {
        state: id.0,
        sail_facing,
        sticky: match s.name.as_str() {
            "minecraft:slime_block" => Some(Sticky::Slime),
            "minecraft:honey_block" => Some(Sticky::Honey),
            _ => None,
        },
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
            })
        });
    }
    let list = with_world(|w| std::mem::take(&mut w.dims[dim].gantries)).unwrap_or_default();
    let mut keep = Vec::new();
    for mut g in list {
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
        let end = with_world(|w| {
            let m = w.dims[dim].pistons.get_mut(&pos)?;
            m.offset += create_engine::piston::movement(speed, facing, m.offset, m.range);
            Some(m.offset <= 0.0 || m.offset >= m.range as f32)
        })
        .flatten();
        if end == Some(true) {
            disassemble_piston(game, dimension, dim, pos);
        }
    }
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
    // Off the world: everything but the piston itself.
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
    let per_tick = create_engine::piston::movement(speed, facing, m.offset, m.range);
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
