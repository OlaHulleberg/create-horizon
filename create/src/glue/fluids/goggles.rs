//! The goggles' lines for the fluid blocks on a client (`addToGoggleTooltip`): a fluid tank's
//! boiler status, else its contents (`containedFluidTooltip`), a spout's and an item drain's
//! contents (a creative tank shows none), read from what their block entities show clients (`TANK`,
//! `BOILER`).

use super::super::states;
use super::{BOILER, TANK, tank_in};
use create_engine::Block;
use create_engine::tank::{MAX_HEIGHT, MAX_WIDTH, Multi, Tank};
use mod_api::{ClientHost, Compound, Tag};

/// The block entity data of the block at `pos` this client has.
fn data_at(host: &dyn ClientHost, pos: [i32; 3]) -> Option<Compound> {
    let level = host.level(host.dimension())?;
    host.block_entity(level, pos)?.data(host)
}

/// The tank a block entity's data shows.
fn tank_of(data: Compound) -> Option<Multi> {
    tank_in(&data)
}

/// The Create block at `pos` this client shows.
fn block_at(host: &dyn ClientHost, pos: [i32; 3]) -> Option<Block> {
    let state = host
        .level(host.dimension())?
        .block_state(host, pos)
        .and_then(states::id)?;
    super::super::client_table(host, |t| t.get(state).map(|(b, _)| b)).flatten()
}

/// The controller of the tank the tank block at `pos` is part of, and its tank: below the
/// lowest block of its column, the corner toward negative x and z whose tank reaches `pos`.
pub(super) fn tank_controller(host: &dyn ClientHost, pos: [i32; 3]) -> Option<([i32; 3], Multi)> {
    let tank = |p: [i32; 3]| block_at(host, p) == Some(Block::FluidTank);
    let mut bottom = pos;
    for _ in 0..MAX_HEIGHT {
        let below = [bottom[0], bottom[1] - 1, bottom[2]];
        if !tank(below) {
            break;
        }
        bottom = below;
    }
    for dx in 0..MAX_WIDTH {
        for dz in 0..MAX_WIDTH {
            let c = [bottom[0] - dx, bottom[1], bottom[2] - dz];
            let Some(m) = data_at(host, c).and_then(tank_of) else {
                continue;
            };
            let w = i32::from(m.width);
            if dx < w && dz < w && pos[1] - c[1] < i32::from(m.height) {
                return Some((c, m));
            }
        }
    }
    None
}

/// The boiler the controller's data at `c` shows, if it is one.
pub(super) fn boiler_at(
    host: &dyn ClientHost,
    c: [i32; 3],
) -> Option<create_engine::steam::Boiler> {
    let data = data_at(host, c)?;
    let Some(Tag::IntArray(v)) = data.get(&BOILER.into()) else {
        return None;
    };
    let &[heat, passive, water, engines, whistles, ..] = v.as_slice() else {
        return None;
    };
    Some(create_engine::steam::Boiler {
        active_heat: heat,
        passive_heat: passive != 0,
        water_supply: f32::from_bits(water as u32),
        attached_engines: engines,
        attached_whistles: whistles,
        ..Default::default()
    })
}

pub(super) fn tooltip(
    host: &dyn ClientHost,
    pos: [i32; 3],
    block: Block,
    lines: &mut Vec<Vec<(String, u32)>>,
) {
    match block {
        // `FluidTankBlockEntity.addToGoggleTooltip`, by its controller.
        Block::FluidTank => {
            let Some((c, m)) = tank_controller(host, pos) else {
                return;
            };
            if let Some(b) = boiler_at(host, c).and_then(|b| b.tooltip(m.blocks())) {
                lines.extend(b);
                return;
            }
            lines.extend(create_engine::tank::tooltip(m.tank, m.capacity()));
        }
        // `CreativeFluidTankBlockEntity.addToGoggleTooltip` shows nothing.
        // `SteamEngineBlockEntity.addToGoggleTooltip`: its shaft's generator stats
        // (`PoweredShaftBlockEntity.addToEngineTooltip`), as the shaft shows them.
        Block::SteamEngine => {
            if let Some(l) = engine_lines(host, pos) {
                lines.extend(l);
            }
        }
        // `WhistleBlockEntity.addToGoggleTooltip`: its note, from the extensions above it.
        Block::SteamWhistle => {
            let pitch = create_engine::whistle::pitch(|h| {
                let p = [pos[0], pos[1] + h, pos[2]];
                let level = host.level(host.dimension())?;
                let id = level.block_state(host, p).and_then(states::id)?;
                super::super::client_table(host, |t| match t.get(id) {
                    Some((Block::SteamWhistleExtension, i)) => i.extender,
                    _ => None,
                })
                .flatten()
            });
            lines.push(create_engine::gauge::goggles(
                0,
                vec![(
                    format!("Pitch: {}", create_engine::whistle::note(pitch)),
                    0xFF_FFFF,
                )],
            ));
        }
        Block::Spout | Block::ItemDrain => {
            let Some(data) = data_at(host, pos) else {
                return;
            };
            let Some(Tag::IntArray(v)) = data.get(&TANK.into()) else {
                return;
            };
            let &[fluid, amount, capacity] = v.as_slice() else {
                return;
            };
            let tank = Tank {
                fluid: create_engine::tank::Codes::fluid(&super::fluid_keys(&data, TANK), fluid),
                amount: amount.max(0) as u32,
            };
            lines.extend(create_engine::tank::tooltip(tank, capacity.max(0) as u32));
        }
        _ => {}
    }
}

/// The generator stats of the powered shaft the engine at `pos` drives
/// (`GeneratingKineticBlockEntity.addToGoggleTooltip`): its capacity at the speed it turns.
fn engine_lines(host: &dyn ClientHost, pos: [i32; 3]) -> Option<Vec<Vec<(String, u32)>>> {
    let level = host.level(host.dimension())?;
    let info_at = |p: [i32; 3]| {
        let id = level.block_state(host, p).and_then(states::id)?;
        super::super::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
    };
    let (_, info) = info_at(pos)?;
    let facing = create_engine::steam::connected_direction(info.face?, info.facing?);
    let shaft = create_engine::steam::shaft_pos(pos, facing);
    let (Block::PoweredShaft, shaft_info) = info_at(shaft)? else {
        return None;
    };
    let data = host.block_entity(level, shaft).and_then(|e| e.data(host));
    let settings = create_engine::parts::Settings {
        steam: data
            .as_ref()
            .map(super::engine::shown_steam)
            .unwrap_or_default(),
        ..Default::default()
    };
    let [part, _] = create_engine::parts::parts(Block::PoweredShaft, &shaft_info, &settings);
    let capacity = part.map_or(0.0, |p| p.capacity);
    let (speed, _) = super::super::kinetics::shown_theory(host, level, shaft);
    let generated = data
        .as_ref()
        .and_then(super::super::sources::generated_of)
        .unwrap_or(speed);
    let total = if speed == 0.0 {
        0.0
    } else {
        capacity * generated.abs()
    };
    Some(vec![
        vec![("    Generator Stats:".into(), 0xFF_FFFF)],
        vec![("    Kinetic Stress Capacity:".into(), 0xAA_AAAA)],
        vec![
            (
                format!("     {}su", create_engine::gauge::number(f64::from(total))),
                0x55_FFFF,
            ),
            (" at current speed".to_owned(), 0x55_5555),
        ],
    ])
}
