//! Rotation indicators (`KineticEffectHandler.spawnRotationIndicators`,
//! `RotationIndicatorParticle`): two ticks after a player places a kinetic block, or after a source
//! that drives its own network changes speed level, twenty sparks of the speed level's colour
//! circle its axis, seen only through goggles.
//!
//! The server sends them as Create does (`sendParticles`), one at a time so each carries its data
//! in its velocity (the particle type declares no velocity of its own,
//! `assets/create/particles/rotation_indicator.json`): the colour, the speed, the axis and the two
//! radii. The client's provider reads it on the particle's first tick and turns it about its
//! origin from then on.

use super::super::server::ServerLevel;
use super::super::world::with_world;
use create_engine::Block;
use create_engine::gauge::SpeedLevel;
use create_engine::kinetics::Axis;
use mod_api::{ModParticle, ParticleKey, ParticleOptions, ParticleProvider, Registrar};
use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

/// The particle type's name: Create's.
pub const NAME: &str = "create:rotation_indicator";

static KEY: OnceLock<ParticleKey> = OnceLock::new();

thread_local! {
    /// Blocks whose indicators are due, by level, with the ticks left (`particleSpawnCountdown`).
    static DUE: RefCell<Vec<(usize, [i32; 3], u8)>> = const { RefCell::new(Vec::new()) };
}

/// The client's animation clock (`AnimationTickHolder.getTicks`) and whether its player wears
/// goggles, for the provider, which has no host.
static TICKS: AtomicI64 = AtomicI64::new(0);
static GOGGLES: AtomicBool = AtomicBool::new(false);

pub(super) fn register(r: &mut dyn Registrar) {
    let _ = KEY.set(r.particle_key(NAME));
}

/// `queueRotationIndicators`: the block at `pos` shows its indicators in two ticks.
pub(super) fn queue(dim: usize, pos: [i32; 3]) {
    DUE.with_borrow_mut(|d| {
        if !d.iter().any(|(dd, p, _)| *dd == dim && *p == pos) {
            d.push((dim, pos, 2));
        }
    });
}

/// `KineticEffectHandler.tick` on the server: indicators whose countdown ran out.
pub(super) fn level_tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let due: Vec<[i32; 3]> = DUE.with_borrow_mut(|d| {
        if d.is_empty() {
            return Vec::new();
        }
        let mut due = Vec::new();
        d.retain_mut(|(dd, pos, left)| {
            if *dd != dim {
                return true;
            }
            *left -= 1;
            if *left == 0 {
                due.push(*pos);
            }
            *left > 0
        });
        due
    });
    for pos in due {
        spawn(lv, pos);
    }
}

/// `KineticBlock.getParticleInitialRadius` and `getParticleTargetRadius`.
fn radii(block: Block) -> (f32, f32) {
    match block {
        Block::Shaft | Block::PoweredShaft => (0.125, 0.35),
        Block::GantryShaft => (0.25, 0.35),
        Block::Cogwheel => (0.75, 0.65),
        Block::LargeCogwheel => (1.0, 1.125),
        Block::MechanicalMixer | Block::MechanicalCrafter => (0.75, 0.85),
        Block::WaterWheel | Block::CrushingWheel => (1.0, 1.125),
        Block::LargeWaterWheel => (2.25, 2.5),
        Block::Flywheel => (1.75, 2.0),
        _ => (0.75, 0.65),
    }
}

/// `spawnRotationIndicators`: twenty sparks at the block's centre, none while it stands still.
fn spawn(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let Some(&key) = KEY.get() else {
        return;
    };
    let dim = lv.dim;
    let Some((speed, block, info)) = with_world(|w| {
        let speed = w.dims[dim].kinetics.speed(pos, 0)?;
        let (block, info) = w.table.get(w.state_at(dim, pos)?)?;
        Some((speed, block, *info))
    })
    .flatten() else {
        return;
    };
    if speed == 0.0 {
        return;
    }
    let Some(axis) =
        create_engine::parts::parts(block, &info, &Default::default())[0].map(|p| p.axis)
    else {
        return;
    };
    let level = SpeedLevel::of(speed);
    let particle_speed = level.particle_speed() * speed.signum() as i32;
    let (r1, r2) = radii(block);
    let data = encode(level.color(), particle_speed, axis, r1, r2);
    let center = pos.map(|v| f64::from(v) + 0.5);
    let options = ParticleOptions::simple(key);
    for _ in 0..20 {
        // One particle, its velocity the "spread" times the speed (vanilla's count 0).
        let _ = lv
            .level
            .send_particles(&mut *lv.host, &options, None, center, data, 1.0, 0);
    }
}

/// The data a spark carries in its velocity: its colour, and its speed, axis and radii (each
/// radius in fortieths), as whole numbers a float holds exactly.
fn encode(color: u32, speed: i32, axis: Axis, r1: f32, r2: f32) -> [f64; 3] {
    let axis = match axis {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    };
    let r = |v: f32| (v * 40.0).round() as i64;
    [
        f64::from(color),
        ((i64::from(speed) + 64) + 128 * axis + 512 * r(r1)) as f64,
        r(r2) as f64,
    ]
}

/// A spark's data: colour, speed, axis, radii.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Spark {
    color: u32,
    speed: i32,
    axis: Axis,
    r1: f64,
    r2: f64,
}

fn decode(v: [f64; 3]) -> Spark {
    let color = v[0].max(0.0) as u32 & 0xFF_FFFF;
    let packed = v[1].max(0.0) as i64;
    let axis = match (packed / 128) % 4 {
        0 => Axis::X,
        1 => Axis::Y,
        _ => Axis::Z,
    };
    Spark {
        color,
        speed: (packed % 128) as i32 - 64,
        axis,
        r1: (packed / 512) as f64 / 40.0,
        r2: v[2] / 40.0,
    }
}

/// The spark kept between ticks in one number: `decode`'s velocity, packed.
fn pack(v: [f64; 3]) -> f64 {
    // Colour (24 bits), then the second word (16 bits), then the third (8): 48 bits, and one
    // more so it is never 0 (a particle's state starts at 0).
    let (a, b, c) = (v[0] as u64, v[1] as u64, v[2] as u64);
    (1u64 << 48 | a << 24 | (b & 0xFFFF) << 8 | (c & 0xFF)) as f64
}

fn unpack(s: f64) -> [f64; 3] {
    let s = s as u64;
    [
        ((s >> 24) & 0xFF_FFFF) as f64,
        ((s >> 8) & 0xFFFF) as f64,
        (s & 0xFF) as f64,
    ]
}

/// The client's tick: the animation clock moves on, and goggles are worn or not.
pub(super) fn client_tick(goggles: bool) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    GOGGLES.store(goggles, Ordering::Relaxed);
}

/// Catnip's `VecHelper.rotate`.
fn rotate(v: [f64; 3], deg: f64, axis: Axis) -> [f64; 3] {
    let a = (deg / 180.0 * std::f64::consts::PI) as f32;
    let (sin, cos) = (f64::from(a.sin()), f64::from(a.cos()));
    let [x, y, z] = v;
    match axis {
        Axis::X => [x, y * cos - z * sin, z * cos + y * sin],
        Axis::Y => [x * cos + z * sin, y, z * cos - x * sin],
        Axis::Z => [x * cos - y * sin, y * cos + x * sin, z],
    }
}

/// `RotationIndicatorParticle`: on its first tick it takes its data from its velocity (and
/// stays only for a player wearing goggles); each tick it stands on its circle, the circle
/// closing from its first radius to its second, and fades in its second half
/// (`SimpleAnimatedParticle`).
pub struct Indicator;

impl ParticleProvider for Indicator {
    fn tick(&self, p: &mut ModParticle) {
        if p.state[3] == 0.0 {
            if !GOGGLES.load(Ordering::Relaxed) {
                p.removed = true;
                return;
            }
            let s = decode(p.velocity);
            // `lifeSpan + random.nextInt(32)`: the spawn's random size picks the 32.
            let random = p.size.to_bits() % 32;
            p.lifetime = 10 + random as i32;
            p.size *= 0.75;
            let white = create_engine::gauge::mix_colors(s.color, 0xFF_FFFF, 0.5);
            p.rgba = [
                ((white >> 16) & 0xFF) as f32 / 255.0,
                ((white >> 8) & 0xFF) as f32 / 255.0,
                (white & 0xFF) as f32 / 255.0,
                1.0,
            ];
            p.state = [
                p.position[0],
                p.position[1],
                p.position[2],
                pack(p.velocity),
            ];
            p.velocity = [0.0; 3];
        }
        let s = decode(unpack(p.state[3]));
        let age = f64::from(p.age);
        let lifetime = f64::from(p.lifetime.max(1));
        // `radius += (radius2 - radius) * .1` after each move, from radius1.
        let radius = s.r2 + (s.r1 - s.r2) * 0.9f64.powi(p.age - 1);
        let time = TICKS.load(Ordering::Relaxed) as f64;
        let speed = f64::from(s.speed);
        let mut angle = (time * speed) % 360.0 - speed / 2.0 * age * (age / lifetime);
        if s.speed < 0 && s.axis == Axis::Y {
            angle += 180.0;
        }
        let offset = if s.axis == Axis::Y {
            [radius, 0.0, 0.0]
        } else {
            [0.0, radius, 0.0]
        };
        let r = rotate(offset, angle, s.axis);
        p.position = [p.state[0] + r[0], p.state[1] + r[1], p.state[2] + r[2]];
        // `setSpriteFromAge` over the eight glitter sprites.
        p.frame = (p.age * 7 / p.lifetime.max(1)).clamp(0, 7) as u32;
        let half = p.lifetime / 2;
        if p.age > half {
            p.rgba[3] = 1.0 - (p.age - half) as f32 / p.lifetime as f32;
            // `setFadeColor`: a fifth of the way to the level's colour each tick.
            let fade = [16, 8, 0].map(|sh| ((s.color >> sh) & 0xFF) as f32 / 255.0);
            for (c, f) in p.rgba.iter_mut().zip(fade) {
                *c += (f - *c) * 0.2;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spark_keeps_its_data() {
        let v = encode(0xFF55FF, -30, Axis::Y, 2.25, 2.5);
        let s = decode(unpack(pack(v)));
        assert_eq!(
            s,
            Spark {
                color: 0xFF55FF,
                speed: -30,
                axis: Axis::Y,
                r1: 2.25,
                r2: 2.5
            }
        );
        assert!(
            v.iter().all(|c| f64::from(*c as f32) == *c),
            "floats carry it"
        );
        assert_eq!(decode(v), s);
    }
}
