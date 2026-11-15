//! The haunted bell's souls (`HauntedBellPulser`, `SoulPulseEffect`, `SoulPulseEffectHandler`,
//! `HauntedBellBlockEntity.tick`, `SoulParticle`, `SoulBaseParticle`; the rules are
//! `create_engine::soul`'s). A rung haunted bell sends a pulse 10 blocks out, and one held
//! (after 10 ticks) sends one 3 blocks out each tick but every eighth: each pulse's rings go out
//! a block every 6 ticks, and on each block of a ring where a mob could stand on the ground
//! (`SpawnPlacementTypes.ON_GROUND` for a zombie, and room for a soul) that is dark (no block
//! light), and that no live pulse showed yet, a soul rises (`create:soul`) over its base on the
//! ground (`create:soul_base`). For 20 ticks after a ring, the bell gives off vanilla soul
//! particles and `SOUL_ESCAPE` a quarter of the ticks.
//!
//! Create's client runs the pulses where its packet arrives; here the server runs them and sends
//! the souls to the players within 32 blocks. Left out (`API_GAPS.md`, Client): the souls are
//! drawn facing the camera, where Create turns a soul only about the vertical and lays its base
//! flat on the ground (`CustomRotationParticle.getCustomRotation`), and a soul that the light
//! reaches is not taken away (`SoulParticle.tick` reading the client level's light).

use super::super::server::ServerLevel;
use create_engine::soul::{self, Pulse, SoulAnimation};
use mod_api::{
    BlockFace, Hand, LightLayer, ModParticle, ParticleKey, ParticleOptions, ParticleProvider,
    PlayerRef, Registrar, ServerHost, SoundSource, SupportType,
};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

pub(super) const SOUL: &str = "create:soul";
pub(super) const SOUL_BASE: &str = "create:soul_base";

static KEYS: OnceLock<[ParticleKey; 3]> = OnceLock::new();

pub(super) fn register(r: &mut dyn Registrar) {
    let _ = KEYS.set([
        r.particle_key(SOUL),
        r.particle_key(SOUL_BASE),
        r.particle_key("minecraft:soul"),
    ]);
}

#[derive(Default)]
struct Level {
    pulses: Vec<Pulse>,
    occupied: HashSet<[i32; 3]>,
    /// Rung haunted bells' `effectTicks`.
    effects: HashMap<[i32; 3], u32>,
}

thread_local! {
    static LEVELS: RefCell<HashMap<usize, Level>> = RefCell::new(HashMap::new());
    static LAYERS: Vec<Vec<[i32; 3]>> = soul::layers();
    /// `HauntedBellPulser.WARMUP`: each holder's countdown and the tick it was last read (it
    /// is forgotten 250 ms, 5 ticks, after).
    static WARMUP: RefCell<HashMap<u128, (i32, i64)>> = RefCell::new(HashMap::new());
}

pub(super) fn forget() {
    LEVELS.with_borrow_mut(HashMap::clear);
    WARMUP.with_borrow_mut(HashMap::clear);
}

/// `HauntedBellBlockEntity.ring`: a pulse from the bell and its souls for `EFFECT_TICKS`.
pub(super) fn rung(dim: usize, pos: [i32; 3]) {
    LEVELS.with_borrow_mut(|l| {
        let l = l.entry(dim).or_default();
        l.pulses.push(Pulse::new(pos, soul::RUNG_DISTANCE));
        l.effects.insert(pos, soul::EFFECT_TICKS);
    });
}

/// `hauntedBellCreatesPulse`: a player (not a spectator) holding a haunted bell.
pub(super) fn player_tick(host: &mut dyn ServerHost, player: PlayerRef) {
    let Some(living) = host.living(player.entity()) else {
        return;
    };
    let holding = [Hand::Main, Hand::Off].into_iter().any(|h| {
        living
            .hand_item(&*host, h)
            .is_some_and(|s| s.item().id(&*host) == "create:haunted_bell")
    });
    if !holding || player.game_mode(&*host) == Some(mod_api::GameMode::Spectator) {
        return;
    }
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return;
    };
    let Some(lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    let now = lv.game_time();
    let key = player.uuid();
    let pulse = WARMUP.with_borrow_mut(|w| {
        let e = w.entry(key).or_insert((soul::HELD_WARMUP, now));
        if now - e.1 > 5 {
            e.0 = soul::HELD_WARMUP;
        }
        e.1 = now;
        soul::held_pulse(&mut e.0, now as u64)
    });
    if !pulse {
        return;
    }
    let Some(at) = player.entity().position(&*lv.host) else {
        return;
    };
    let pos = at.map(|v| v.floor() as i32);
    LEVELS.with_borrow_mut(|l| {
        l.entry(lv.dim)
            .or_default()
            .pulses
            .push(Pulse::new(pos, soul::HELD_DISTANCE))
    });
}

/// `SoulPulseEffect.isDark`: no block light.
fn dark(lv: &ServerLevel<'_>, at: [i32; 3]) -> bool {
    lv.level
        .brightness(&*lv.host, LightLayer::Block, at)
        .is_some_and(|b| b < 1)
}

/// `canSpawnSoulAt(.., ignoreLight)`: `ON_GROUND.isSpawnPositionOk` for a zombie (a sturdy top
/// below; at the block and above it no full collision cube and no fluid) and no collision in
/// a 0.2 by 0.75 box standing in the block's middle.
fn soul_fits(lv: &ServerLevel<'_>, at: [i32; 3]) -> bool {
    let host = &*lv.host;
    let below = [at[0], at[1] - 1, at[2]];
    if lv
        .level
        .block_face_sturdy(host, below, BlockFace::Up, SupportType::Full)
        != Some(true)
    {
        return false;
    }
    let empty = |p: [i32; 3]| {
        let lo = p.map(f64::from);
        let full = lv
            .level
            .collision_boxes(
                host,
                [lo[0], lo[1], lo[2], lo[0] + 1.0, lo[1] + 1.0, lo[2] + 1.0],
            )
            .unwrap_or_default()
            .iter()
            .any(|b| {
                b[0] <= lo[0]
                    && b[1] <= lo[1]
                    && b[2] <= lo[2]
                    && b[3] >= lo[0] + 1.0
                    && b[4] >= lo[1] + 1.0
                    && b[5] >= lo[2] + 1.0
            });
        let fluid = lv.level.fluid_state(host, p).is_some_and(|f| !f.is_empty());
        !full && !fluid
    };
    if !empty(at) || !empty([at[0], at[1] + 1, at[2]]) {
        return false;
    }
    let (x, y, z) = (
        f64::from(at[0]) + 0.5,
        f64::from(at[1]),
        f64::from(at[2]) + 0.5,
    );
    lv.level
        .collision_boxes(host, [x - 0.1, y, z - 0.1, x + 0.1, y + 0.75, z + 0.1])
        .is_some_and(|b| b.is_empty())
}

fn send(lv: &mut ServerLevel<'_>, key: ParticleKey, at: [f64; 3], v: [f64; 3]) {
    let _ = lv.level.send_particles(
        &mut *lv.host,
        &ParticleOptions::simple(key),
        None,
        at,
        v,
        1.0,
        0,
    );
}

/// The level's tick: the bells' souls after a ring, then each pulse's ring
/// (`SoulPulseEffectHandler.tick`).
pub(super) fn tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let Some(mut level) = LEVELS.with_borrow_mut(|l| l.remove(&dim)) else {
        return;
    };
    let Some(&[soul_key, base_key, vanilla_soul]) = KEYS.get() else {
        return;
    };
    // `HauntedBellBlockEntity.tick`.
    level.effects.retain(|_, t| {
        *t -= 1;
        *t > 0
    });
    let rung: Vec<[i32; 3]> = level.effects.keys().copied().collect();
    for pos in rung {
        use mod_api::RandomSource;
        let r: [f64; 8] = lv
            .level
            .with_random(&mut *lv.host, |r| std::array::from_fn(|_| r.next_double()))
            .unwrap_or([1.0; 8]);
        if r[0] > 0.25 {
            continue;
        }
        let at = [
            f64::from(pos[0]) + r[1],
            f64::from(pos[1]) + 0.5,
            f64::from(pos[2]) + r[2],
        ];
        send(
            lv,
            vanilla_soul,
            at,
            [r[3] * 0.04 - 0.02, 0.1, r[4] * 0.04 - 0.02],
        );
        // `rand.nextFloat() * 0.4F + rand.nextFloat() > 0.9F ? 0.6F : 0.0F`.
        let volume = if r[5] * 0.4 + r[6] > 0.9 { 0.6 } else { 0.0 };
        let centre = pos.map(|v| f64::from(v) + 0.5);
        let _ = lv.level.play_sound(
            &mut *lv.host,
            "minecraft:particle.soul_escape",
            centre,
            SoundSource::Blocks,
            volume,
            0.6 + r[7] as f32 * 0.4,
        );
    }
    // The pulses.
    let mut pulses = std::mem::take(&mut level.pulses);
    for pulse in &mut pulses {
        let spawns = LAYERS.with(|layers| pulse.tick(layers, |p| soul_fits(lv, p)));
        for p in spawns.unwrap_or_default() {
            if !level.occupied.insert(p) {
                continue;
            }
            pulse.added.push(p);
            // `spawnParticles`: where it is dark.
            if dark(lv, p) {
                let c = [
                    f64::from(p[0]) + 0.5,
                    f64::from(p[1]),
                    f64::from(p[2]) + 0.5,
                ];
                send(lv, soul_key, [c[0], c[1] + 0.5, c[2]], [0.0; 3]);
                send(lv, base_key, [c[0], c[1] + 0.01, c[2]], [0.0; 3]);
            }
        }
    }
    for pulse in pulses.iter().filter(|p| p.finished()) {
        for p in &pulse.added {
            level.occupied.remove(p);
        }
    }
    pulses.retain(|p| !p.finished());
    level.pulses = pulses;
    if !level.pulses.is_empty() || !level.effects.is_empty() {
        LEVELS.with_borrow_mut(|l| l.insert(dim, level));
    }
}

/// A number from 0 to 1 that stays with a particle, from where it spawned.
fn random(p: &ModParticle, salt: u64) -> f32 {
    let mut h = salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for v in p.position {
        h = (h ^ v.to_bits()).wrapping_mul(0x0100_0000_01B3);
    }
    h ^= h >> 29;
    (h % 10_000) as f32 / 10_000.0
}

/// `SoulParticle` (not a perimeter one): 0.5 big, still, its 53 frames in their start, loop
/// and end stages, two ticks a frame.
pub(super) struct Soul;

impl ParticleProvider for Soul {
    fn tick(&self, p: &mut ModParticle) {
        let mut a = if p.state[0] == 0.0 {
            p.size = 0.5;
            p.lifetime = i32::MAX;
            p.velocity = [0.0; 3];
            SoulAnimation {
                stage: 1,
                ticks: 0,
                loops: 0,
                // `startFrames + (int) (random.nextFloat() * 5f - 4f)` and the like.
                start_ticks: (17 + (random(p, 1) * 5.0 - 4.0) as i32) as u32,
                end_ticks: (20 + (random(p, 2) * 5.0 - 4.0) as i32) as u32,
                num_loops: (1.0 + random(p, 3) * 2.0) as u32,
            }
        } else {
            let s = p.state;
            SoulAnimation {
                stage: s[0] as u8,
                ticks: s[1] as u32,
                loops: s[2] as u32,
                start_ticks: (s[3] as u32) & 0xFF,
                end_ticks: ((s[3] as u32) >> 8) & 0xFF,
                num_loops: (s[3] as u32) >> 16,
            }
        };
        let frame = a.tick();
        if a.stage == 0 {
            p.removed = true;
            return;
        }
        // `animatedSprite.get(frame, 53)`.
        p.frame = frame * 52 / 53;
        p.state = [
            f64::from(a.stage),
            f64::from(a.ticks),
            f64::from(a.loops),
            f64::from(a.start_ticks | a.end_ticks << 8 | a.num_loops << 16),
        ];
    }
}

/// `SoulBaseParticle`: 0.5 big, still, 90 to 140 ticks, its 8 frames looping every 16 ticks.
pub(super) struct SoulBase;

impl ParticleProvider for SoulBase {
    fn tick(&self, p: &mut ModParticle) {
        if p.state[0] == 0.0 {
            p.size = 0.5;
            p.velocity = [0.0; 3];
            p.lifetime = (90.0 / (random(p, 4) * 0.36 + 0.64)) as i32;
            let loop_length = 16 + (random(p, 5) * 2.0 - 1.0) as i32;
            p.state[0] = f64::from(loop_length);
        }
        let loop_length = (p.state[0] as i32).max(1);
        // `selectSpriteLoopingWithAge`: `sprites.get(age % loopLength, loopLength)` of 8.
        p.frame = ((p.age % loop_length) * 7 / loop_length) as u32;
    }
}
