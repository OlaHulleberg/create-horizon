//! What clients draw of steam engines (`SteamEngineRenderer`): the piston, its linkage and the
//! connector on the powered shaft two blocks in front, moving with the shaft's turn; and once a
//! turn the steam it lets out (`SteamEngineBlockEntity.spawnParticles`, `client_tick`): a steam
//! jet and the boiler's hiss and steam sounds, at most four a side each two ticks (`SoundPool`).
//! A jet (`SteamJetParticle`) is drawn as Create's renderer draws it, four quads turned along
//! its motion, its frame by its age, through the glue's world pipeline (`render_jets`): as world
//! quads, full bright (no particle light) and not sorted among the other particles.

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
    TOLD.with_borrow_mut(std::collections::HashMap::clear);
}

/// The key of what drives a powered shaft (its engine's direction, and efficiency as `f32`
/// bits) in its block entity's data, for its clients' goggles (`calculateAddedStressCapacity`).
pub const STEAM: &str = "Steam";

/// A powered shaft's engine direction and efficiency bits, by level and position.
type Told = std::collections::HashMap<(usize, [i32; 3]), (i8, u32)>;

thread_local! {
    /// What each powered shaft's clients were last told drives it.
    static TOLD: std::cell::RefCell<Told> = std::cell::RefCell::new(Told::new());
}

/// What drives the powered shaft whose block entity data is `data`, as its clients last heard.
pub fn shown_steam(data: &Compound) -> create_engine::steam::Output {
    match data.get(&STEAM.into()) {
        Some(Tag::IntArray(v)) if v.len() == 2 => create_engine::steam::Output {
            direction: v[0] as i8,
            efficiency: f32::from_bits(v[1] as u32),
        },
        _ => create_engine::steam::Output::default(),
    }
}

/// Powered shafts whose engine output changed tell their clients.
fn send_steam(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let changed: Vec<([i32; 3], (i8, u32))> = super::super::world::with_world(|w| {
        let shafts: Vec<[i32; 3]> = w.dims[dim].shaft_engine.keys().copied().collect();
        TOLD.with_borrow_mut(|told| {
            let mut out = Vec::new();
            for s in shafts {
                let o = w.shaft_steam(dim, s);
                let now = (o.direction, o.efficiency.to_bits());
                if told.insert((dim, s), now) != Some(now) {
                    out.push((s, now));
                }
            }
            told.retain(|(d, p), _| *d != dim || w.dims[dim].shaft_engine.contains_key(p));
            out
        })
    })
    .unwrap_or_default();
    for (pos, (direction, efficiency)) in changed {
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let v = vec![i32::from(direction), efficiency as i32];
        let data = Compound::from([(STEAM.into(), Tag::IntArray(v))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
}

/// Engines placed since the last tick show the speed of the shaft in front of them.
pub(super) fn tick(lv: &mut ServerLevel<'_>) {
    send_steam(lv);
    super::super::advancements::steam_engines(lv);
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

/// `SteamEngineBlockEntity.getShaft().hasSource()`: another source turns the shaft in front
/// of the engine at `pos` (its value box is then inactive).
pub(crate) fn engine_sourced(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    super::super::world::with_world(|w| !w.value_box_active(lv.dim, pos)) == Some(true)
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

/// A client's steam engines (`SteamEngineBlockEntity.spawnParticles`): each engine's last
/// angle, and each boiler side's sounds waiting to be merged (`BoilerData.pools`: a
/// `SoundPool` of 4 at most every 2 ticks).
#[derive(Default)]
struct Client {
    angles: std::collections::HashMap<[i32; 3], f32>,
    pools: std::collections::HashMap<([i32; 3], Dir), Pool>,
    /// The game time the pools last ticked at, so they tick once a tick.
    pooled_at: i64,
    rng: Option<create_engine::processing::Rng>,
}

#[derive(Default)]
struct Pool {
    queued: Vec<([i32; 3], i32)>,
    ticks: u32,
}

thread_local! {
    static CLIENT: std::cell::RefCell<Client> = std::cell::RefCell::new(Client::default());
}

/// A steam jet (`SteamJetParticle`): where and when it was spawned, turned along its motion
/// and rolled at random.
struct Jet {
    at: [f64; 3],
    born: i64,
    yaw: f32,
    pitch: f32,
    roll: f32,
}

thread_local! {
    static JETS: std::cell::RefCell<Vec<Jet>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// `SteamJetParticle`'s lifetime (`setLifetime(21)`), size (`quadSize`) and frames
/// (`particles/steam_jet.json`).
const JET_LIFETIME: i64 = 21;
const JET_SIZE: f64 = 0.375;
const JET_FRAMES: i64 = 23;
/// Frames a row of the grid sprite `create-data prepare` makes of them.
const JET_GRID: i64 = 5;

pub(super) fn register_client(_r: &mut dyn mod_api::Registrar) {}

pub(super) fn reset_client() {
    CLIENT.with_borrow_mut(|c| *c = Client::default());
    JETS.with_borrow_mut(Vec::clear);
}

/// `level.addParticle(new SteamJetParticleData(1), at, motion)`: a jet starting now.
fn spawn_jet(host: &dyn ClientHost, at: [f64; 3], motion: [f32; 3]) {
    let Some(born) = host.level(host.dimension()).and_then(|l| l.game_time(host)) else {
        return;
    };
    let [dx, dy, dz] = motion;
    let roll = next_float() * std::f32::consts::PI;
    JETS.with_borrow_mut(|j| {
        j.push(Jet {
            at,
            born,
            yaw: dx.atan2(dz) - std::f32::consts::PI,
            pitch: dy.atan2((dx * dx + dz * dz).sqrt()) - std::f32::consts::FRAC_PI_2,
            roll,
        })
    });
}

/// `SteamJetParticle.render` for every live jet: four quads turned along the jet, each a
/// quarter turn about it from the last, the frame its age picks (`setSpriteFromAge`).
pub(super) fn render_jets(host: &dyn ClientHost, event: &mut mod_api::RenderLevelStageEvent<'_>) {
    if event.stage != mod_api::RenderLevelStage::AfterTranslucentBlocks {
        return;
    }
    let now = event.view.game_time;
    let mut quads = Vec::new();
    JETS.with_borrow_mut(|jets| {
        jets.retain(|j| {
            let age = now - j.born;
            if !(0..JET_LIFETIME).contains(&age) {
                return age < 0;
            }
            let frame = age * (JET_FRAMES - 1) / JET_LIFETIME;
            // The frame's cell in the grid `create-data prepare` lays them out in.
            let cell = 1.0 / JET_GRID as f32;
            let (cx, cy) = (
                (frame % JET_GRID) as f32 * cell,
                (frame / JET_GRID) as f32 * cell,
            );
            let (u0, u1, v0, v1) = (cx, cx + cell, cy, cy + cell);
            for i in 0..4 {
                let turn = 2.0 * j.roll + std::f32::consts::FRAC_PI_2 * i as f32;
                let pose = crate::glue::pipelines::Pose::at(j.at)
                    .rotate([0.0, 1.0, 0.0], f64::from(j.yaw.to_degrees()), [0.0; 3])
                    .rotate([1.0, 0.0, 0.0], f64::from(j.pitch.to_degrees()), [0.0; 3])
                    .rotate([0.0, 1.0, 0.0], f64::from(turn.to_degrees()), [0.0; 3])
                    .scale([JET_SIZE; 3]);
                let c = |x: f64, y: f64| pose.apply([x, y + 1.0, 0.0]);
                quads.push(mod_api::PipelineQuad::new(
                    [c(-1.0, -1.0), c(-1.0, 1.0), c(1.0, 1.0), c(1.0, -1.0)],
                    [[u1, v1], [u1, v0], [u0, v0], [u0, v1]],
                    0xFFFF_FFFF,
                ));
            }
            true
        });
    });
    if !quads.is_empty()
        && let Some(p) =
            crate::glue::pipelines::resolve(host, crate::glue::pipelines::Textured::SteamJet)
    {
        let _ = event.draw.pipeline_quads(p, &quads);
    }
}

fn next_float() -> f32 {
    CLIENT.with_borrow_mut(|c| {
        c.rng
            .get_or_insert_with(|| create_engine::processing::Rng::new(0x57EA))
            .next_float()
    })
}

/// `SoundPool.play` for every boiler side, once a game tick: a pool waits 2 ticks for more,
/// then plays all it has if 4 or fewer, else 4 of them at random; each is the boiler's sound
/// (`BoilerData.sound`): a candle going out and Create's steam, quieter with more engines.
fn play_pools(host: &mut dyn ClientHost, now: i64) {
    let due: Vec<([i32; 3], i32)> = CLIENT.with_borrow_mut(|c| {
        if c.pooled_at == now {
            return Vec::new();
        }
        c.pooled_at = now;
        let mut due = Vec::new();
        c.pools.retain(|_, p| {
            if p.queued.is_empty() {
                return false;
            }
            p.ticks += 1;
            if p.ticks < 2 {
                return true;
            }
            p.ticks = 0;
            due.push(std::mem::take(&mut p.queued));
            true
        });
        let rng = c
            .rng
            .get_or_insert_with(|| create_engine::processing::Rng::new(0x57EA));
        due.into_iter()
            .flat_map(|mut q| {
                if q.len() <= 4 {
                    return q;
                }
                let mut out = Vec::with_capacity(4);
                while out.len() < 4 {
                    let i = (rng.next_float() * q.len() as f32) as usize;
                    out.push(q.swap_remove(i.min(q.len() - 1)));
                }
                out
            })
            .collect()
    });
    for (pos, engines) in due {
        let volume = 3.0 / (engines / 6).max(2) as f32;
        let pitch = 1.18 - next_float() * 0.25;
        let candle = mod_api::SoundPlay::at(
            "minecraft:block.candle.extinguish",
            mod_api::SoundSource::Blocks,
            pos.map(f64::from),
        )
        .with_volume(volume)
        .with_pitch(pitch);
        host.play_local_sound(&candle, 0);
        let steam = mod_api::SoundPlay::at(
            "create:steam",
            mod_api::SoundSource::Neutral,
            pos.map(|v| f64::from(v) + 0.5),
        )
        .with_volume(volume / 16.0)
        .with_pitch(0.8);
        host.play_local_sound(&steam, 0);
    }
}

/// A client tick of the engine at `pos` (`SteamEngineBlockEntity.tick` on the client): while its
/// boiler drives its shaft, each time the piston passes the end of its stroke a sound is queued
/// on the boiler's side and a jet of steam leaves the engine (`spawnParticles`).
pub(super) fn client_tick(
    host: &mut dyn ClientHost,
    event: &mut mod_api::BlockEntityClientTickEvent<'_>,
    info: &StateInfo,
) {
    let now = event
        .block_entity
        .level(&*host)
        .and_then(|l| l.game_time(&*host))
        .unwrap_or(0);
    play_pools(host, now);
    let pos = event.pos;
    let speed = shown_speed(event.entity);
    let Some((facing, at, axis)) = shaft(&*host, pos, info) else {
        return;
    };
    if speed == 0.0 || axis == facing.axis() {
        return;
    }
    // The tank behind it, as a working boiler (`isValid`, `engineEfficiency`).
    let o = facing.opposite().offset();
    let tank = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
    let Some((controller, m)) = super::goggles::tank_controller(&*host, tank) else {
        return;
    };
    let Some(boiler) = super::goggles::boiler_at(&*host, controller) else {
        return;
    };
    if boiler.engine_efficiency(m.blocks()) == 0.0 {
        return;
    }
    // `getTargetAngle` in degrees, then turned so the stroke's end is at 0 / ±180.
    let offset = create_engine::client::rotation_offset(Block::PoweredShaft, axis, at);
    let mut angle = (now as f32 * speed * 0.3 + offset) % 360.0;
    if axis != Axis::Y && ((facing.axis() == Axis::X) ^ facing.positive()) {
        angle = -angle;
    }
    if axis == Axis::X && facing == Dir::Down {
        angle = -angle;
    }
    angle += if angle < 0.0 {
        -180.0 + 75.0
    } else {
        360.0 - 75.0
    };
    angle %= 360.0;
    let Some(prev) = CLIENT.with_borrow_mut(|c| c.angles.insert(pos, angle)) else {
        return;
    };
    let crossed = if angle >= 0.0 {
        prev > 180.0 && angle < 180.0
    } else {
        prev < -180.0 && angle > -180.0
    };
    if !crossed {
        return;
    }
    CLIENT.with_borrow_mut(|c| {
        c.pools
            .entry((controller, facing))
            .or_default()
            .queued
            .push((pos, boiler.attached_engines));
    });
    // The jet, half a block out of the engine's face and up to a quarter off its middle,
    // moving out of it (`offset - facing * 0.75`).
    let (mut x, mut y) = (next_float() - 0.5, next_float() - 0.5);
    let len = (x * x + y * y).sqrt();
    if len > 0.0 {
        x /= len;
        y /= len;
    }
    let f = facing.offset().map(|v| v as f32);
    let (u, v) = match facing.axis() {
        Axis::X => ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        Axis::Y => ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        Axis::Z => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    };
    let offset = [0, 1, 2].map(|i| f[i] + (u[i] * x + v[i] * y) * 0.5);
    let at = [0, 1, 2].map(|i| f64::from(pos[i]) + 0.5 + f64::from(offset[i] * 0.5));
    spawn_jet(host, at, [0, 1, 2].map(|i| offset[i] - f[i] * 0.75));
}

/// `WhistleBlockEntity.tickAudio`'s jet of steam out of a blowing whistle's mouth: up and out
/// of its face, lower and further out for a bigger whistle.
pub(super) fn whistle_jet(host: &mut dyn ClientHost, pos: [i32; 3], info: &StateInfo) {
    let facing = info.facing.unwrap_or(Dir::South);
    let size = match info.size.unwrap_or_default() {
        create_engine::whistle::Size::Small => 0.0,
        create_engine::whistle::Size::Medium => 1.0,
        create_engine::whistle::Size::Large => 2.0,
    };
    // `VecHelper.rotate(v, 180 + horizontalAngle(facing), Y)`.
    let (sin, cos) = (180.0 + horizontal_angle(facing)).to_radians().sin_cos();
    let rotate = |v: [f32; 3]| [v[0] * cos + v[2] * sin, v[1], v[2] * cos - v[0] * sin];
    let size_offset = rotate([0.0, -0.4, size / 16.0]);
    let offset = rotate([0.0, 1.0, 0.75]);
    let at =
        [0, 1, 2].map(|i| f64::from(pos[i]) + 0.5 + f64::from(offset[i] * 0.45 + size_offset[i]));
    let f = facing.offset().map(|v| v as f32);
    spawn_jet(host, at, [0, 1, 2].map(|i| offset[i] - f[i] * 0.75));
}
