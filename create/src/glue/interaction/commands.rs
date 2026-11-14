//! `/create_debug speed <pos>`: the kinetic speed the server solved at a block, for tests and
//! for players who want the number (operators only).

use super::super::world::{level_index, with_world};
use mod_api::{CommandArgumentValue, Flow, RunModCommandEvent, ServerHost, Text};

pub(super) fn register(commands: &mut Vec<mod_api::Command>) {
    commands.push(mod_api::Command::op(
        "create_debug speed <pos:block_pos>",
        2,
    ));
}

pub(super) fn run(host: &mut dyn ServerHost, event: &mut RunModCommandEvent<'_>) -> Flow {
    let command = event.command;
    if command.literals != ["create_debug", "speed"] {
        return Flow::Continue;
    }
    let Some(&CommandArgumentValue::BlockPos([x, y, z])) = command.argument("pos") else {
        return Flow::Continue;
    };
    let dim = command
        .dimension
        .id(&*host)
        .and_then(|d| level_index(&d))
        .unwrap_or(0);
    let found = with_world(|w| {
        let speed = w.speed(dim, [x, y, z])?;
        Some((speed, w.dims.get(dim)?.kinetics.network([x, y, z], 0)))
    })
    .flatten();
    let line = match found {
        Some((speed, Some((_, stress, capacity, over)))) => format!(
            "create speed at {x} {y} {z}: {speed} rpm, stress {stress} / {capacity} su{}",
            if over { ", overstressed" } else { "" }
        ),
        Some((speed, None)) => format!("create speed at {x} {y} {z}: {speed} rpm"),
        None => format!("create speed at {x} {y} {z}: not kinetic"),
    };
    let _ = host.commands().feedback(Text::literal(line));
    Flow::Cancel
}
