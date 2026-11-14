//! What clients draw of the fluids: tanks' contents (`FluidTankRenderer`), hose pulleys' hoses
//! (`HosePulleyRenderer`) and valves' pointers (`FluidValveRenderer`), on the `create:kinetic`
//! block entity type.

use super::super::kinetics::{self, axis_vector, dir_name, speeds_of};
use super::super::states;
use super::{HOSE, shown_tank};
use create_engine::Block;
use create_engine::fluids::{Pointer, pointer_offset, valve_pipe_axis};
use create_engine::hose::Piece;
use create_engine::kinetics::Dir;
use create_engine::parts::StateInfo;
use create_engine::tank::Fluid;
use mod_api::{
    BlockEntityClientTickEvent, BlockEntityClientTicksEvent, BlockEntityPart,
    BlockEntityRenderBoundingBoxEvent, BlockEntitySubmitEvent, BlockPart, ClientHost,
    FluidBoxOptions, FluidStack, ModBlockEntity, Registrar, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;

pub(super) fn register(r: &mut dyn Registrar) {
    let Some(kind) = kinetics::kind() else {
        return;
    };
    if let Err(e) = r.block_entity_part(&kind, FluidEntity) {
        eprintln!("[create] the fluids' block entity part was refused: {e:?}");
    }
}

/// A valve's pointer on a client: where it is, where it was a tick ago, the speed it last
/// took (`onSpeedChanged`).
#[derive(Clone, Copy)]
struct Valve {
    pointer: Pointer,
    prev: f32,
    speed: f32,
}

thread_local! {
    static VALVES: RefCell<HashMap<[i32; 3], Valve>> = RefCell::new(HashMap::new());
}

pub(super) fn reset() {
    VALVES.with_borrow_mut(HashMap::clear);
}

/// A pulley's hose as its block entity shows it: offset, blocks a tick it moves, and the game
/// time it was at.
fn shown_hose(entity: &ModBlockEntity) -> Option<(f32, f32, i64)> {
    let Some(Tag::List(l)) = entity.nbt.get(&HOSE.into()) else {
        return None;
    };
    match l.as_slice() {
        [Tag::Float(o), Tag::Float(v), Tag::Long(t)] => {
            Some((f32::from_bits(*o), f32::from_bits(*v), *t))
        }
        _ => None,
    }
}

/// Whether the block entity of `block` at `pos` moves this frame: a valve's pointer turning, a
/// hose going up or down.
pub fn animated(block: Block, pos: [i32; 3], entity: &ModBlockEntity) -> bool {
    match block {
        Block::FluidValve => VALVES.with_borrow(|v| {
            v.get(&pos)
                .is_some_and(|v| v.pointer.moving() || v.prev != v.pointer.angle())
        }),
        Block::HosePulley => shown_hose(entity).is_some_and(|(_, v, _)| v != 0.0),
        _ => false,
    }
}

fn client_block(host: &dyn ClientHost, state: mod_api::BlockState) -> Option<(Block, StateInfo)> {
    let id = states::id(state)?;
    super::super::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
}

struct FluidEntity;

#[mod_api::horizon_part]
impl BlockPart for FluidEntity {}

#[mod_api::horizon_part]
impl BlockEntityPart for FluidEntity {
    fn block_entity_client_ticks(
        &self,
        _host: &dyn ClientHost,
        event: &mut BlockEntityClientTicksEvent<'_>,
    ) {
        if event.block == "create:fluid_valve" {
            event.ticks = true;
        }
    }

    /// Each client tick: a valve's pointer sets off at a new speed and takes a step
    /// (`tickChaser`).
    fn block_entity_client_tick(
        &self,
        host: &mut dyn ClientHost,
        event: &mut BlockEntityClientTickEvent<'_>,
    ) {
        let Some((Block::FluidValve, info)) = client_block(&*host, event.state) else {
            return;
        };
        let speed = speeds_of(&event.entity.nbt)[0];
        VALVES.with_borrow_mut(|v| {
            let v = v.entry(event.pos).or_insert_with(|| Valve {
                pointer: Pointer::at(info.enabled),
                prev: Pointer::at(info.enabled).angle(),
                speed: 0.0,
            });
            if v.speed != speed {
                v.speed = speed;
                v.pointer.speed_changed(speed);
            }
            v.prev = v.pointer.angle();
            v.pointer.tick(info.enabled);
        });
    }

    fn block_entity_render_bounding_box(
        &self,
        host: &dyn ClientHost,
        event: &mut BlockEntityRenderBoundingBoxEvent<'_>,
    ) {
        let Some((block, _)) = client_block(host, event.state) else {
            return;
        };
        match block {
            Block::FluidTank | Block::CreativeFluidTank => {
                if let Some(m) = shown_tank(event.entity) {
                    let (w, h) = (f32::from(m.width), f32::from(m.height));
                    *event.bounds = [-1.0, -1.0, -1.0, w + 1.0, h + 1.0, w + 1.0];
                }
            }
            Block::HosePulley => {
                if let Some((offset, _, _)) = shown_hose(event.entity) {
                    event.bounds[1] = event.bounds[1].min(-offset - 2.0);
                }
            }
            _ => {}
        }
    }

    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some((block, info)) = client_block(host, event.state) else {
            return;
        };
        match block {
            Block::FluidTank | Block::CreativeFluidTank => draw_tank(host, event),
            Block::HosePulley => draw_hose(event),
            Block::FluidValve => draw_pointer(event, &info),
            _ => {}
        }
    }
}

/// `FluidTankRenderer`: the fluid's box over the whole tank from its controller. Water and lava
/// are drawn as the game draws a still fluid (water in its default colour); Create's own as their
/// fluid's still texture on a block model stretched to the box.
fn draw_tank(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let Some(m) = shown_tank(event.entity) else {
        return;
    };
    let Some((min, max)) = create_engine::tank::fluid_box(&m) else {
        return;
    };
    let Some(fluid) = m.tank.fluid else {
        return;
    };
    let draw = &mut *event.draw;
    let bounds = [min[0], min[1], min[2], max[0], max[1], max[2]];
    let drawn = match fluid {
        Fluid::Water | Fluid::Lava => host.fluid_key(fluid.id()).is_some_and(|key| {
            let options = FluidBoxOptions::new(true, 0.0);
            let options = if fluid == Fluid::Water {
                options.with_argb(0xFF3F_76E4)
            } else {
                options
            };
            draw.fluid_box(&FluidStack::new(key, m.tank.amount), bounds, options)
                .is_ok()
        }),
        _ => false,
    };
    if !drawn {
        let name = match fluid {
            Fluid::Honey => "part_fluid_honey",
            Fluid::Chocolate => "part_fluid_chocolate",
            Fluid::Milk => "part_fluid_milk",
            Fluid::Water => "part_fluid_water",
            Fluid::Lava => "part_fluid_lava",
        };
        if let Some(part) = Block::from_name(name) {
            draw.push();
            draw.translate(min);
            draw.scale([max[0] - min[0], max[1] - min[1], max[2] - min[2]]);
            draw.block(states::key(part), &[("facing", "up")]);
            draw.pop();
        }
    }
    draw.keep();
}

/// `AbstractPulleyRenderer`: the magnet (or its half near the top) at the hose's end, gliding
/// while it moves, and the hose's pieces down to it.
fn draw_hose(event: &mut BlockEntitySubmitEvent<'_>) {
    let Some((offset, per_tick, at)) = shown_hose(event.entity) else {
        return;
    };
    let draw = &mut *event.draw;
    let moved = if per_tick == 0.0 {
        0.0
    } else {
        let ticks = (draw.game_time() - at) as f32 + draw.partial();
        per_tick * ticks.max(0.0)
    };
    for piece in create_engine::hose::pieces(offset) {
        let (name, down) = match piece {
            Piece::Magnet(d) => ("part_hose_magnet", d + moved),
            Piece::HalfMagnet(d) => ("part_hose_half_magnet", d + moved),
            Piece::HalfRope(d) => ("part_hose_half", d),
            Piece::Rope(_, d) => ("part_hose_rope", d),
        };
        let Some(part) = Block::from_name(name) else {
            continue;
        };
        draw.push();
        draw.translate([0.0, -down, 0.0]);
        draw.block(states::key(part), &[("facing", "up")]);
        draw.pop();
    }
    if per_tick == 0.0 {
        draw.keep();
    }
}

/// `FluidValveRenderer`'s pointer: turned to the valve's facing (`horizontalAngle`, 90° down
/// from up for a side, 180° for down) and its extra quarter turn, then about the facing by the
/// pointer's angle, between its last two ticks while it moves.
fn draw_pointer(event: &mut BlockEntitySubmitEvent<'_>, info: &StateInfo) {
    let Some(facing) = info.facing else {
        return;
    };
    let Some(part) = Block::from_name("part_fluid_valve_pointer") else {
        return;
    };
    let (angle, moving) = VALVES
        .with_borrow(|v| {
            v.get(&event.pos).map(|v| {
                let to = v.pointer.angle();
                if v.pointer.moving() || v.prev != to {
                    (v.prev + (to - v.prev) * event.draw.partial(), true)
                } else {
                    (to, false)
                }
            })
        })
        .unwrap_or((Pointer::at(info.enabled).angle(), false));
    let pipe = valve_pipe_axis(facing, info.along_first);
    let shaft = create_engine::parts::directional_axis(facing, info.along_first);
    let yaw = match facing {
        Dir::West => 90.0,
        Dir::North => 180.0,
        Dir::East => 270.0,
        _ => 0.0f32,
    };
    let pitch = match facing {
        Dir::Up => 0.0,
        Dir::Down => 180.0,
        _ => 90.0f32,
    };
    // About the facing's axis: its positive end turns the same way, its negative end back.
    let sign = if facing.offset().iter().sum::<i32>() > 0 {
        1.0
    } else {
        -1.0
    };
    let c = [0.5; 3];
    let draw = &mut *event.draw;
    draw.push();
    draw.rotate_about(axis_vector(facing.axis()), angle * sign, c);
    draw.rotate_about([0.0, 1.0, 0.0], yaw, c);
    draw.rotate_about([1.0, 0.0, 0.0], pitch, c);
    draw.rotate_about([0.0, 1.0, 0.0], pointer_offset(pipe, shaft), c);
    draw.block(states::key(part), &[("facing", dir_name(Dir::Up))]);
    draw.pop();
    if !moving {
        draw.keep();
    }
}
