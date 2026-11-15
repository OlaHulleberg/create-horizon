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
        crate::create_log!("[create] the fluids' block entity part was refused: {e:?}");
    }
    super::engine::register_client(r);
    // Equipment registers before kinetics makes the type.
    super::super::equipment::register_cogs(r, &kind);
}

/// A valve's pointer on a client: where it is, where it was a tick ago, the speed it last
/// took (`onSpeedChanged`).
#[derive(Clone, Copy)]
struct Valve {
    pointer: Pointer,
    prev: f32,
    speed: f32,
}

/// A boiler's gauge on a client (`BoilerData.gauge`, chased at 0.125 a tick, `Chaser.EXP`): where
/// it set off from and when, the level it chases, and whether it is there.
#[derive(Clone, Copy)]
struct Gauge {
    from: f32,
    since: f64,
    target: f32,
    settled: bool,
}

impl Gauge {
    /// Where the dial is at animation time `now`: each tick it moves an eighth of the way left.
    fn at(&self, now: f64) -> f32 {
        let ticks = (now - self.since).max(0.0);
        self.target + (self.from - self.target) * 0.875f64.powf(ticks) as f32
    }
}

thread_local! {
    static VALVES: RefCell<HashMap<[i32; 3], Valve>> = RefCell::new(HashMap::new());
    static GAUGES: RefCell<HashMap<[i32; 3], Gauge>> = RefCell::new(HashMap::new());
}

pub(super) fn reset() {
    VALVES.with_borrow_mut(HashMap::clear);
    GAUGES.with_borrow_mut(HashMap::clear);
    super::engine::reset_client();
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
        Block::ItemDrain => super::drawn::animated(entity),
        Block::SteamEngine => super::engine::animated(entity),
        Block::Spout => super::spouted::animated(entity),
        Block::SteamWhistle => whistle_blows(entity).is_some_and(|(on, _)| on),
        Block::FluidTank => GAUGES.with_borrow(|g| g.get(&pos).is_some_and(|g| !g.settled)),
        Block::GlassFluidPipe => super::flows::animated(entity),
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
        // Pipes, valves and pumps spawn their flows' particles (`flows`).
        if matches!(
            event.block,
            "create:fluid_valve"
                | "create:steam_engine"
                | "create:steam_whistle"
                | "create:fluid_pipe"
                | "create:encased_fluid_pipe"
                | "create:glass_fluid_pipe"
                | "create:smart_fluid_pipe"
                | "create:mechanical_pump"
        ) {
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
        let info = match client_block(&*host, event.state) {
            Some((Block::SteamEngine, info)) => {
                return super::engine::client_tick(host, event, &info);
            }
            Some((Block::SteamWhistle, info)) => {
                if let Some((true, since)) = whistle_blows(event.entity) {
                    let now = event
                        .block_entity
                        .level(&*host)
                        .and_then(|l| l.game_time(&*host))
                        .unwrap_or(0);
                    // `tickAudio`: a jet every 8 ticks, and as it starts.
                    if now % 8 == 0 || i64::from(now as i32) == since {
                        super::engine::whistle_jet(host, event.pos, &info);
                    }
                }
                return;
            }
            Some((Block::FluidValve, info)) => {
                super::flows::client_pipe_tick(host, event.pos, event.entity);
                info
            }
            Some((
                Block::FluidPipe
                | Block::EncasedFluidPipe
                | Block::GlassFluidPipe
                | Block::SmartFluidPipe
                | Block::MechanicalPump,
                _,
            )) => {
                return super::flows::client_pipe_tick(host, event.pos, event.entity);
            }
            _ => return,
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
            Block::SteamWhistle => draw_whistle(event, &info),
            Block::ItemDrain => super::drawn::draw(host, event),
            Block::SteamEngine => super::engine::draw(host, event, &info),
            Block::Spout => super::spouted::draw(host, event),
            Block::HosePulley => draw_hose(event),
            Block::FluidValve => {
                super::rims::draw(event);
                draw_pointer(event, &info);
            }
            Block::SmartFluidPipe => {
                super::rims::draw(event);
                draw_pipe_filter(host, event, &info);
            }
            Block::GlassFluidPipe => {
                super::rims::draw(event);
                super::flows::draw(host, event);
            }
            Block::FluidPipe | Block::EncasedFluidPipe | Block::MechanicalPump => {
                super::rims::draw(event)
            }
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
    if !m.window {
        if draw_boiler(event, &m) {
            event.draw.keep();
        }
        return;
    }
    let Some((min, max)) = create_engine::tank::fluid_box(&m) else {
        return;
    };
    let Some(fluid) = m.tank.fluid else {
        return;
    };
    fluid_box(host, &mut *event.draw, fluid, m.tank.amount, min, max);
    event.draw.keep();
}

/// A fluid's box from `min` to `max` (in blocks from the block's corner), as `FluidRenderer`
/// draws one: as the game draws a still fluid (water in its default colour), else (a fluid the
/// game has no key for) the fluid's still texture on a block model stretched to the box. Tanks,
/// spouts, drains and basins draw theirs with it.
pub(crate) fn fluid_box(
    host: &dyn ClientHost,
    draw: &mut dyn mod_api::BlockEntityDrawContext,
    fluid: Fluid,
    amount: u32,
    min: [f32; 3],
    max: [f32; 3],
) {
    let bounds = [min[0], min[1], min[2], max[0], max[1], max[2]];
    let drawn = host.fluid_key(fluid.id()).is_some_and(|key| {
        let options = FluidBoxOptions::new(true, 0.0);
        let options = if fluid == Fluid::Water {
            options.with_argb(0xFF3F_76E4)
        } else {
            options
        };
        draw.fluid_box(&FluidStack::new(key, amount.max(1)), bounds, options)
            .is_ok()
    });
    if drawn {
        return;
    }
    // Create's virtual fluids: their still texture tinted, builder's tea as it is and a potion
    // in its colour (`PotionFluidType.getTintColor`).
    let tinted = match fluid {
        Fluid::Tea => Some(("create:block/fluid/tea_still", 0xFF_FFFF)),
        Fluid::Potion(p) => Some(("create:block/fluid/potion_still", p.color())),
        _ => None,
    };
    if let Some((sprite, tint)) = tinted {
        let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        // Up to a block, a face shows the part of the texture its place covers; a bigger box
        // stretches the texture over each face, as the other fluids' models do.
        let face = if size.iter().all(|s| *s <= 1.0) {
            mod_api::CubeFace::new(sprite)
        } else {
            mod_api::CubeFace::new(sprite).with_uv([0.0, 0.0, 16.0, 16.0])
        };
        let from = min.map(|v| v * 16.0);
        let to = max.map(|v| v * 16.0);
        draw.cube(from, to, [Some(face); 6], tint);
        return;
    }
    {
        let name = match fluid {
            Fluid::Honey => "part_fluid_honey",
            Fluid::Chocolate => "part_fluid_chocolate",
            Fluid::Milk => "part_fluid_milk",
            Fluid::Water => "part_fluid_water",
            Fluid::Lava => "part_fluid_lava",
            Fluid::Tea | Fluid::Potion(_) => return,
        };
        if let Some(part) = states::partial(name) {
            draw.push();
            draw.translate(min);
            draw.scale([max[0] - min[0], max[1] - min[1], max[2] - min[2]]);
            draw.block(part, &[("facing", "up")]);
            draw.pop();
        }
    }
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
        let Some(part) = states::partial(name) else {
            continue;
        };
        draw.push();
        draw.translate([0.0, -down, 0.0]);
        draw.block(part, &[("facing", "up")]);
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
    let Some(part) = states::partial("part_fluid_valve_pointer") else {
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
    draw.block(part, &[("facing", dir_name(Dir::Up))]);
    draw.pop();
    if !moving {
        draw.keep();
    }
}

/// `FilteringRenderer` on a smart pipe: the item in its filter slot (`SmartPipeFilterSlot`'s
/// place, turn and tilt, the value box's scale), drawn as `ValueBoxRenderer
/// .renderItemIntoValueBox` does: a block at its full size, a flat item at half pushed back.
fn draw_pipe_filter(
    host: &dyn ClientHost,
    event: &mut BlockEntitySubmitEvent<'_>,
    info: &StateInfo,
) {
    let Some(Tag::String(id)) = event.entity.nbt.get(&super::plumbing::FILTER.into()) else {
        return;
    };
    let id = id.to_string_lossy();
    let (Some(face), Some(facing)) = (info.face, info.facing) else {
        return;
    };
    let Some(stack) = super::super::logistics::items::stack_of(host, &id, 1) else {
        return;
    };
    let solid = super::super::logistics::items::is_block_item(host, &id);
    let (slot, angle, tilt) = create_engine::fluids::smart_pipe_filter_slot(face, facing);
    let scale =
        create_engine::fluids::SMART_PIPE_SLOT_SCALE * (if solid { 1.0 } else { 0.5 } + 1.0 / 64.0);
    let draw = &mut *event.draw;
    draw.push();
    draw.translate(slot.map(|v| v as f32));
    draw.rotate_about([0.0, 1.0, 0.0], angle, [0.0; 3]);
    draw.rotate_about([1.0, 0.0, 0.0], tilt, [0.0; 3]);
    draw.scale([scale; 3]);
    if !solid {
        draw.translate([0.0, 0.0, -0.15]);
    }
    draw.item(&stack, mod_api::ItemDisplayContext::Fixed, None, 0);
    draw.pop();
}

/// Whether a whistle blows, and the game time it started or stopped (`plumbing::WHISTLE`).
fn whistle_blows(entity: &ModBlockEntity) -> Option<(bool, i64)> {
    let Some(Tag::IntArray(v)) = entity.nbt.get(&super::plumbing::WHISTLE.into()) else {
        return None;
    };
    match v.as_slice() {
        [on, at] => Some((*on != 0, i64::from(*at))),
        _ => None,
    }
}

/// `WhistleRenderer`: the mouth for the whistle's size, turned to its facing, lifted by its
/// `animation` (chasing 1 by half the way a tick while it blows, falling 0.4 a tick after) and,
/// once more than half open, wiggling eight ticks a cycle times 4 less its size's ordinal.
fn draw_whistle(event: &mut BlockEntitySubmitEvent<'_>, info: &StateInfo) {
    use create_engine::whistle::Size;
    let size = info.size.unwrap_or_default();
    let name = match size {
        Size::Small => "part_whistle_mouth_small",
        Size::Medium => "part_whistle_mouth_medium",
        Size::Large => "part_whistle_mouth_large",
    };
    let Some(part) = states::partial(name) else {
        return;
    };
    let Some(facing) = info.facing else {
        return;
    };
    let draw = &mut *event.draw;
    let now = draw.game_time() as f32 + draw.partial();
    let (on, at) = whistle_blows(event.entity).unwrap_or((false, 0));
    let t = (now - at as f32).max(0.0);
    let mut offset = if on {
        1.0 - 0.5f32.powf(t)
    } else {
        (1.0 - 0.4 * t).max(0.0)
    };
    if on && offset > 0.5 {
        let ordinal = match size {
            Size::Small => 0.0,
            Size::Medium => 1.0,
            Size::Large => 2.0,
        };
        let wiggle = now / 8.0;
        offset -= (wiggle * std::f32::consts::TAU * (4.0 - ordinal)).sin() / 16.0;
    }
    draw.push();
    draw.translate([0.0, offset * 4.0 / 16.0, 0.0]);
    draw.block(part, &[("facing", dir_name(facing))]);
    draw.pop();
    if !on {
        draw.keep();
    }
}

/// `FluidTankRenderer.renderAsBoiler`: on each side its gauge is not hidden on, the gauge and its
/// dial at the middle of the bottom layer, the dial turned by the boiler's level (a passive
/// boiler an eighth, else its heat over the most its size allows). The dial is drawn at that
/// level, without `gauge`'s chase to it.
/// `BoilerRenderer` for a controller without windows: its gauges and their dials, each dial
/// swinging to the boiler's level (`gauge.chase(target, .125, EXP)` as the boiler's data comes).
/// Whether it stands still (it may be kept).
fn draw_boiler(event: &mut BlockEntitySubmitEvent<'_>, m: &create_engine::tank::Multi) -> bool {
    let Some(Tag::IntArray(v)) = event.entity.nbt.get(&super::BOILER.into()) else {
        GAUGES.with_borrow_mut(|g| g.remove(&event.pos));
        return true;
    };
    let &[heat, passive, water, engines, whistles, occluded] = v.as_slice() else {
        return true;
    };
    let boiler = create_engine::steam::Boiler {
        active_heat: heat,
        passive_heat: passive != 0,
        water_supply: f32::from_bits(water as u32),
        attached_engines: engines,
        attached_whistles: whistles,
        ..Default::default()
    };
    if !boiler.is_active() {
        GAUGES.with_borrow_mut(|g| g.remove(&event.pos));
        return true;
    }
    let (Some(gauge), Some(dial)) = (
        states::partial("part_boiler_gauge"),
        states::partial("part_boiler_gauge_dial"),
    ) else {
        return true;
    };
    let size = i32::from(m.width) * i32::from(m.width) * i32::from(m.height);
    let for_size = create_engine::steam::max_heat_for_size(size);
    let progress = if boiler.is_passive(size) {
        1.0 / 8.0
    } else if for_size == 0 {
        0.0
    } else {
        boiler.level(size) as f32 / for_size as f32
    };
    let now = event.draw.animation_time();
    let (progress, settled) = GAUGES.with_borrow_mut(|g| {
        // A gauge first seen sets off from empty (`LerpedFloat` starts at 0).
        let gauge = g.entry(event.pos).or_insert(Gauge {
            from: 0.0,
            since: now,
            target: progress,
            settled: false,
        });
        if gauge.target != progress {
            *gauge = Gauge {
                from: gauge.at(now),
                since: now,
                target: progress,
                settled: false,
            };
        }
        let at = gauge.at(now);
        gauge.settled = (at - gauge.target).abs() < 1.0 / 512.0;
        (if gauge.settled { gauge.target } else { at }, gauge.settled)
    });
    let w = f32::from(m.width);
    let draw = &mut *event.draw;
    // `Iterate.horizontalDirections` by `get2DDataValue` (south, west, north, east) and their
    // `toYRot`.
    for (i, y_rot) in [0.0f32, 90.0, 180.0, 270.0].into_iter().enumerate() {
        if occluded & (1 << i) != 0 {
            continue;
        }
        draw.push();
        draw.translate([w / 2.0, 0.5, w / 2.0]);
        draw.rotate_about([0.0, 1.0, 0.0], -y_rot - 90.0, [0.0; 3]);
        draw.translate([-0.5, -0.5, -0.5]);
        draw.translate([w / 2.0 - 6.0 / 16.0, 0.0, 0.0]);
        draw.block(gauge, &[("facing", "south")]);
        draw.rotate_about(
            [1.0, 0.0, 0.0],
            -145.0 * progress + 90.0,
            [0.0, 6.0 / 16.0, 8.0 / 16.0],
        );
        draw.block(dial, &[("facing", "south")]);
        draw.pop();
    }
    settled
}

/// What a fluid tank, spout, item drain or smart pipe shows while it rides a moving structure
/// (`FluidTankRenderer`, `SpoutRenderer`, `ItemDrainRenderer`, the pipe's filter, in
/// `renderBlockEntities`), from the data it had when the structure took it.
pub(crate) fn draw_carried(
    host: &dyn ClientHost,
    block: Block,
    event: &mut BlockEntitySubmitEvent<'_>,
) {
    if matches!(
        block,
        Block::FluidTank
            | Block::CreativeFluidTank
            | Block::Spout
            | Block::ItemDrain
            | Block::SmartFluidPipe
    ) {
        FluidEntity.block_entity_submit(host, event);
    }
}
