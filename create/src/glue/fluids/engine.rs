//! What clients draw of steam engines (`SteamEngineRenderer`): the piston, its linkage and the
//! connector on the powered shaft two blocks in front, moving with the shaft's turn; and once a
//! turn the steam it lets out (`SteamEngineBlockEntity.spawnParticles`): a jet drawn as Create's
//! `SteamJetParticle` draws its quads (by the engine, lit as the engine, while it is drawn) and
//! the boiler's hiss and steam sounds, at most four a side each two ticks (`SoundPool`), at the
//! volume of a boiler with up to eleven engines.

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
    let now = draw.animation_time() as f32;
    if speed != 0.0 && stroked(event.pos, angle.to_degrees()) {
        let (jet, volume) = (new_jet(facing, now), 3.0 / 2.0);
        JETS.with_borrow_mut(|j| j.entry(event.pos).or_default().push(jet));
        // `BoilerData`'s sound for the side: the candle's hiss and Create's steam, four at most
        // each two ticks a side (`SoundPool(4, 2, ..)`).
        let window = (now / 2.0).floor() as i64;
        let pooled = POOL.with_borrow_mut(|pool| {
            let (w, n) = pool.entry(facing as u8).or_insert((window, 0));
            if *w != window {
                *w = window;
                *n = 0;
            }
            *n += 1;
            *n <= 4
        });
        if !pooled {
            draw_jets(draw, event.pos, now);
            return;
        }
        let at = event.pos.map(f64::from);
        let r = next_float();
        let hiss = mod_api::SoundPlay::at(
            "minecraft:block.candle.extinguish",
            mod_api::SoundSource::Blocks,
            at,
        )
        .with_volume(volume)
        .with_pitch(1.18 - r * 0.25);
        let _ = host.play_local_sound(&hiss, 0);
        let steam = mod_api::SoundPlay::at(
            "create:steam",
            mod_api::SoundSource::Blocks,
            at.map(|v| v + 0.5),
        )
        .with_volume(volume / 16.0)
        .with_pitch(0.8);
        let _ = host.play_local_sound(&steam, 0);
    }
    draw_jets(draw, event.pos, now);
}

/// `SteamJetParticle`'s life in ticks, and its sprites (`particles/steam_jet.json`).
const JET_LIFETIME: f32 = 21.0;
const JET_SPRITES: u32 = 23;

/// A steam jet an engine let out (`SteamJetParticle`): where in the engine's block, when (on
/// the animation clock), its roll about its own axis and the way it points.
#[derive(Clone, Copy, Debug)]
struct Jet {
    at: [f32; 3],
    start: f32,
    roll: f32,
    yaw: f32,
    pitch: f32,
}

thread_local! {
    /// The engines' last stroke angle (`prevAngle`) and their jets, by engine.
    static PREV: std::cell::RefCell<std::collections::HashMap<[i32; 3], f32>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    static JETS: std::cell::RefCell<std::collections::HashMap<[i32; 3], Vec<Jet>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    /// The sounds played each side in the current two ticks, by side.
    static POOL: std::cell::RefCell<std::collections::HashMap<u8, (i64, u32)>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    /// The client level's random source, for the jets.
    static RNG: std::cell::RefCell<create_engine::processing::Rng> =
        std::cell::RefCell::new(create_engine::processing::Rng::new(0x057E_A3E7));
}

fn next_float() -> f32 {
    RNG.with_borrow_mut(|r| r.next_float())
}

/// `SteamEngineBlockEntity.spawnParticles`' timing: the shaft's turn (`getTargetAngle`, in
/// degrees) shifted by 75° passes 180° (or -180° turning back) once a turn, when the piston
/// lets its steam out.
fn stroked(pos: [i32; 3], target: f32) -> bool {
    let mut angle = target
        + if target < 0.0 {
            -180.0 + 75.0
        } else {
            360.0 - 75.0
        };
    angle %= 360.0;
    PREV.with_borrow_mut(|prev| {
        let p = prev.insert(pos, angle).unwrap_or(angle);
        if angle >= 0.0 {
            p > 180.0 && angle < 180.0
        } else {
            p < -180.0 && angle > -180.0
        }
    })
}

/// Catnip's `VecHelper.rotate` (degrees).
fn rotate(v: [f32; 3], deg: f32, axis: Axis) -> [f32; 3] {
    let a = deg.to_radians();
    let (sin, cos) = (a.sin(), a.cos());
    let [x, y, z] = v;
    match axis {
        Axis::X => [x, y * cos - z * sin, z * cos + y * sin],
        Axis::Y => [x * cos + z * sin, y, z * cos - x * sin],
        Axis::Z => [x * cos - y * sin, y * cos + x * sin, z],
    }
}

/// The jet `spawnParticles` adds: half a block out of the engine's front, a little off its
/// axis at random, pointing out and back toward the boiler (its velocity, which only aims it).
fn new_jet(facing: Dir, now: f32) -> Jet {
    // `VecHelper.offsetRandomly(ZERO, random, 1).multiply(1, 1, 0).normalize().scale(.5)`.
    let (rx, ry) = ((next_float() - 0.5) * 2.0, (next_float() - 0.5) * 2.0);
    let _ = next_float();
    let len = (rx * rx + ry * ry).sqrt().max(1.0e-4);
    let v = [rx / len * 0.5, ry / len * 0.5, 1.0];
    let offset = rotate(
        rotate(v, vertical_angle(facing), Axis::X),
        horizontal_angle(facing),
        Axis::Y,
    );
    let n = facing.offset().map(|c| c as f32);
    let m = [0, 1, 2].map(|i| offset[i] - n[i] * 0.75);
    let roll = next_float() * std::f32::consts::PI;
    Jet {
        at: offset.map(|c| c * 0.5 + 0.5),
        start: now,
        roll,
        yaw: m[0].atan2(m[2]) - std::f32::consts::PI,
        pitch: m[1].atan2((m[0] * m[0] + m[2] * m[2]).sqrt()) - std::f32::consts::FRAC_PI_2,
    }
}

/// `SteamJetParticle.render`: four quads about the jet's axis, each from its origin out along
/// it, the sprite of its age (`setSpriteFromAge`).
fn draw_jets(draw: &mut dyn mod_api::BlockEntityDrawContext, pos: [i32; 3], now: f32) {
    JETS.with_borrow_mut(|all| {
        let Some(jets) = all.get_mut(&pos) else {
            return;
        };
        jets.retain(|j| now >= j.start && now - j.start < JET_LIFETIME);
        if jets.is_empty() {
            all.remove(&pos);
            return;
        }
        for j in jets.iter() {
            let age = (now - j.start).floor();
            let frame = (age as u32 * (JET_SPRITES - 1) / JET_LIFETIME as u32).min(JET_SPRITES - 1);
            let sprite = format!("create:particle/steam_jet_{frame}");
            for i in 0..4 {
                let turn = 2.0 * j.roll + std::f32::consts::FRAC_PI_2 * i as f32;
                let corner = |x: f32, y: f32| {
                    // Ry(turn), then Rx(pitch), then Ry(yaw), as the quaternion's product.
                    let (s, c) = turn.sin_cos();
                    let p = [x * c, y + 1.0, -x * s];
                    let (s, c) = j.pitch.sin_cos();
                    let p = [p[0], p[1] * c - p[2] * s, p[1] * s + p[2] * c];
                    let (s, c) = j.yaw.sin_cos();
                    let p = [p[0] * c + p[2] * s, p[1], -p[0] * s + p[2] * c];
                    [0, 1, 2].map(|k| j.at[k] + p[k] * 0.375)
                };
                // The particle's corners (-1,-1) (-1,1) (1,1) (1,-1) take u1v1, u1v0, u0v0 and
                // u0v1: from the front, (1,1) is the texture's top left.
                let corners = [
                    corner(1.0, 1.0),
                    corner(1.0, -1.0),
                    corner(-1.0, -1.0),
                    corner(-1.0, 1.0),
                ];
                draw.quad(&sprite, corners, [0.0, 0.0, 16.0, 16.0], 0xFF_FFFF);
            }
        }
    });
}
