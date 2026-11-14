//! The goggles' lines for the fluid blocks on a client (`addToGoggleTooltip`): a fluid tank's
//! boiler status, else its contents (`containedFluidTooltip`), a creative tank's, a spout's and
//! an item drain's contents, read from what their block entities show clients (`TANK`,
//! `BOILER`).

use super::super::states;
use super::{BOILER, TANK, tank_in};
use create_engine::Block;
use create_engine::tank::{CAPACITY, Fluid, MAX_HEIGHT, MAX_WIDTH, Multi, Tank};
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
fn tank_controller(host: &dyn ClientHost, pos: [i32; 3]) -> Option<([i32; 3], Multi)> {
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
fn boiler_at(host: &dyn ClientHost, c: [i32; 3]) -> Option<create_engine::steam::Boiler> {
    let data = data_at(host, c)?;
    let Some(Tag::IntArray(v)) = data.get(&BOILER.into()) else {
        return None;
    };
    let &[heat, passive, water, engines, whistles] = v.as_slice() else {
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
        Block::CreativeFluidTank => {
            let tank = data_at(host, pos)
                .and_then(tank_of)
                .map_or_else(Tank::default, |m| m.tank);
            lines.extend(create_engine::tank::tooltip(tank, CAPACITY));
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
                fluid: Fluid::from_index(fluid.clamp(0, 255) as u8),
                amount: amount.max(0) as u32,
            };
            lines.extend(create_engine::tank::tooltip(tank, capacity.max(0) as u32));
        }
        _ => {}
    }
}
