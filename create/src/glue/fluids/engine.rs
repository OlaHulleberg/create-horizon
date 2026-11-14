//! What clients draw of steam engines (`SteamEngineRenderer`): the piston, its linkage and the
//! connector on the powered shaft two blocks in front, moving with the shaft's turn.

use super::super::server::ServerLevel;
use super::super::states;
use create_engine::Block;
use create_engine::kinetics::{Axis, Dir};
use create_engine::parts::StateInfo;
use mod_api::{BlockEntitySubmitEvent, ClientHost, Compound, ModBlockEntity, Tag};

/// The key of the speed of an engine's shaft in the engine's block entity data, for its
/// clients (the engine's drawing follows the shaft).
pub const ENGINE_SPEED: &str = "EngineSpeed";

/// A powered shaft at `pos` turns at `speed`: the engines driving it show it.
pub(super) fn shaft_speed_changed(lv: &mut ServerLevel<'_>, pos: [i32; 3], speed: f32) {
    if super::info_at(lv, pos).map(|(b, _)| b) != Some(Block::PoweredShaft) {
        return;
    }
    for d in Dir::ALL {
        let engine = create_engine::steam::shaft_pos(pos, d);
        let Some((Block::SteamEngine, info)) = super::info_at(lv, engine) else {
            continue;
        };
        let (Some(face), Some(f)) = (info.face, info.facing) else {
            continue;
        };
        if create_engine::steam::connected_direction(face, f) != d.opposite() {
            continue;
        }
        let Some(entity) = lv.host.block_entity(lv.level, engine) else {
            continue;
        };
        let data = Compound::from([(ENGINE_SPEED.into(), Tag::Float(speed.to_bits()))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
}

thread_local! {
    /// Engines placed (or turned) since the last tick, by level: they show their shaft's speed.
    static PLACED: std::cell::RefCell<Vec<(usize, [i32; 3])>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The block at `pos` became `new`: an engine shows its shaft's speed from the next tick.
pub(super) fn changed(dim: usize, pos: [i32; 3], new: u32) {
    let engine = super::super::world::with_world(|w| {
        w.table
            .get(new)
            .is_some_and(|(b, _)| b == Block::SteamEngine)
    });
    if engine == Some(true) {
        PLACED.with_borrow_mut(|p| p.push((dim, pos)));
    }
}

pub(super) fn reset() {
    PLACED.with_borrow_mut(Vec::clear);
}

/// Engines placed since the last tick show the speed of the shaft in front of them.
pub(super) fn tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let placed: Vec<[i32; 3]> = PLACED.with_borrow_mut(|p| {
        let (mine, rest): (Vec<_>, Vec<_>) = p.drain(..).partition(|(d, _)| *d == dim);
        *p = rest;
        mine.into_iter().map(|(_, q)| q).collect()
    });
    for pos in placed {
        let Some((Block::SteamEngine, info)) = super::info_at(lv, pos) else {
            continue;
        };
        let (Some(face), Some(f)) = (info.face, info.facing) else {
            continue;
        };
        let shaft = create_engine::steam::shaft_pos(
            pos,
            create_engine::steam::connected_direction(face, f),
        );
        let speed = super::super::world::with_world(|w| w.speed(dim, shaft))
            .flatten()
            .unwrap_or(0.0);
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let data = Compound::from([(ENGINE_SPEED.into(), Tag::Float(speed.to_bits()))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
}

/// The shaft speed an engine's data shows.
fn shown_speed(entity: &ModBlockEntity) -> f32 {
    match entity.nbt.get(&ENGINE_SPEED.into()) {
        Some(Tag::Float(b)) => f32::from_bits(*b),
        _ => 0.0,
    }
}

/// The engine's way (`getFacing`), and its shaft's position and axis when it has one.
fn shaft(host: &dyn ClientHost, pos: [i32; 3], info: &StateInfo) -> Option<(Dir, [i32; 3], Axis)> {
    let facing = create_engine::steam::connected_direction(info.face?, info.facing?);
    let at = create_engine::steam::shaft_pos(pos, facing);
    let level = host.level(host.dimension())?;
    let id = level.block_state(host, at).and_then(states::id)?;
    let (block, shaft) =
        super::super::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()?;
    if block != Block::PoweredShaft {
        return None;
    }
    Some((facing, at, shaft.axis?))
}

/// Whether an engine moves this frame (its shaft turns).
pub(super) fn animated(entity: &ModBlockEntity) -> bool {
    shown_speed(entity) != 0.0
}

/// `AngleHelper.horizontalAngle`.
fn horizontal_angle(d: Dir) -> f32 {
    match d {
        Dir::South => 0.0,
        Dir::North => 180.0,
        Dir::West => -90.0,
        Dir::East => -270.0,
        _ => 0.0,
    }
}

/// `AngleHelper.verticalAngle`.
fn vertical_angle(d: Dir) -> f32 {
    match d {
        Dir::Up => -90.0,
        Dir::Down => 90.0,
        _ => 0.0,
    }
}

pub(super) fn draw(
    host: &dyn ClientHost,
    event: &mut BlockEntitySubmitEvent<'_>,
    info: &StateInfo,
) {
    let Some((facing, at, axis)) = shaft(host, event.pos, info) else {
        return;
    };
    let speed = shown_speed(event.entity);
    // `getTargetAngle`: the shaft's turn (`getAngleForBe`).
    if axis == facing.axis() {
        return;
    }
    let draw = &mut *event.draw;
    let offset = create_engine::client::rotation_offset(Block::PoweredShaft, axis, at);
    let degrees = (draw.animation_time() as f32 * speed * 0.3 + offset) % 360.0;
    let mut angle = degrees.to_radians();
    if axis != Axis::Y && ((facing.axis() == Axis::X) ^ facing.positive()) {
        angle = -angle;
    }
    if axis == Axis::X && facing == Dir::Down {
        angle = -angle;
    }
    let roll90 = (facing.axis() != Axis::Y && axis == Axis::Y)
        || (facing.axis() == Axis::Y && axis == Axis::Z);
    let piston = 6.0 / 16.0 * angle.sin()
        - ((14.0f32 / 16.0).powi(2) - (6.0f32 / 16.0).powi(2) * angle.cos().powi(2)).sqrt();
    let distance = (piston - 6.0 / 16.0 * angle.sin()).abs();
    let sign = if angle.cos() >= 0.0 { 1.0 } else { -1.0 };
    let angle2 = (distance / (14.0 / 16.0)).clamp(-1.0, 1.0).acos() * sign;
    let c = [0.5; 3];
    let transformed = |draw: &mut dyn mod_api::BlockEntityDrawContext| {
        draw.rotate_about([0.0, 1.0, 0.0], horizontal_angle(facing), c);
        draw.rotate_about([1.0, 0.0, 0.0], vertical_angle(facing) + 90.0, c);
        if roll90 {
            draw.rotate_about([0.0, 1.0, 0.0], -90.0, c);
        }
    };
    let part = |name: &str| states::partial(name);
    if let Some(p) = part("part_engine_piston") {
        draw.push();
        transformed(draw);
        draw.translate([0.0, piston + 20.0 / 16.0, 0.0]);
        draw.block(p, &[("facing", "up")]);
        draw.pop();
    }
    if let Some(p) = part("part_engine_linkage") {
        draw.push();
        transformed(draw);
        draw.translate([0.0, 1.0 + piston + 20.0 / 16.0, 0.0]);
        draw.rotate_about(
            [1.0, 0.0, 0.0],
            angle2.to_degrees(),
            [0.0, 4.0 / 16.0, 8.0 / 16.0],
        );
        draw.block(p, &[("facing", "up")]);
        draw.pop();
    }
    if let Some(p) = part("part_engine_connector") {
        draw.push();
        transformed(draw);
        draw.translate([0.0, 2.0, 0.0]);
        draw.rotate_about(
            [1.0, 0.0, 0.0],
            -(angle + std::f32::consts::FRAC_PI_2).to_degrees(),
            c,
        );
        draw.block(p, &[("facing", "up")]);
        draw.pop();
    }
}
