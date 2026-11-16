//! The client's half of the encased fan's current (`AirCurrent`, which Create runs on both
//! sides): the server tells each client the turning fans' currents of its level (`create:air`),
//! and the client
//! - pushes its own player along them (`tickAffectedEntities`): a player's client moves it, so
//!   the server's push of a player does nothing, as in Create;
//! - plays the wind while its player is in a current (`AirCurrent.Client`, `AirCurrentSound`);
//! - spawns their air flow particles (`AirFlowParticle`, `create:air_flow`), drifting along the
//!   current and coloured, with their extra flame, lava, smoke, soul flame and bubble particles
//!   (`morphAirFlow`), by the processing type of the segment they cross.

use create_engine::fan::{self, Kind, Seen};
use create_engine::processing::Rng;
use mod_api::{
    ClientHost, ModParticle, ParticleKey, ParticleOptions, ParticleProvider, Registrar, SoundPlay,
    SoundRef, SoundSource, SoundState,
};
use std::cell::RefCell;
use std::sync::{Mutex, OnceLock};

/// The server's message: a level's turning fans and their currents.
pub(super) const CHANNEL: &str = "create:air";

/// The server's message: whether this client's player flies in creative (one byte).
pub(super) const FLYING_CHANNEL: &str = "create:air_flying";

/// Create's air flow particle type.
pub(super) const AIR_FLOW: &str = "create:air_flow";

/// `fanParticleDensity`'s default.
const DENSITY: f32 = 0.5;

/// The particle types the current spawns.
pub(super) struct Keys {
    pub air_flow: ParticleKey,
    pub flame: ParticleKey,
    pub block: ParticleKey,
    pub soul_fire_flame: ParticleKey,
    pub smoke: ParticleKey,
    pub large_smoke: ParticleKey,
    pub bubble: ParticleKey,
    pub bubble_pop: ParticleKey,
    pub poof: ParticleKey,
    pub spit: ParticleKey,
    pub dust: ParticleKey,
}

pub(super) static KEYS: OnceLock<Keys> = OnceLock::new();

pub(super) fn register(r: &mut dyn Registrar) {
    let _ = KEYS.set(Keys {
        air_flow: r.particle_key(AIR_FLOW),
        flame: r.particle_key("minecraft:flame"),
        block: r.particle_key("minecraft:block"),
        soul_fire_flame: r.particle_key("minecraft:soul_fire_flame"),
        smoke: r.particle_key("minecraft:smoke"),
        large_smoke: r.particle_key("minecraft:large_smoke"),
        bubble: r.particle_key("minecraft:bubble"),
        bubble_pop: r.particle_key("minecraft:bubble_pop"),
        poof: r.particle_key("minecraft:poof"),
        spit: r.particle_key("minecraft:spit"),
        dust: r.particle_key("minecraft:dust"),
    });
}

/// An extra particle an air flow particle asked for (`spawnExtraParticle`).
#[derive(Clone, Copy)]
enum Extra {
    Flame,
    Lava,
    SoulFireFlame,
    Smoke,
    LargeSmoke,
    Bubble,
    BubblePop,
}

/// What the client keeps, shared with the particle ticks (which have no host).
#[derive(Default)]
struct Shared {
    /// The turning fans of the level this client shows.
    fans: Vec<Seen>,
    /// Air flow particles spawned and not yet ticked: where, and their fan.
    spawned: Vec<([f64; 3], [i32; 3])>,
    /// Extra particles to add at the next client tick: which, where, how fast.
    extra: Vec<(Extra, [f64; 3], [f64; 3])>,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    fans: Vec::new(),
    spawned: Vec::new(),
    extra: Vec::new(),
});

fn shared<T>(f: impl FnOnce(&mut Shared) -> T) -> T {
    let mut s = SHARED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut s)
}

thread_local! {
    static RNG: RefCell<Rng> = RefCell::new(Rng::new(0xA1F));
    /// `entitySearchCooldown` of each fan for the local player, and whether its last search
    /// caught the player (`caughtEntities`).
    static CAUGHT: RefCell<Vec<([i32; 3], u8, bool)>> = const { RefCell::new(Vec::new()) };
    /// `isPlayerCreativeFlying` for the local player, as the server told it: the client's own
    /// abilities read back the server's last abilities packet, not its own flight (`API_GAPS.md`).
    static FLYING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `AirCurrent.Client`: whether a fan pushed the player since the last sound tick
    /// (`isClientPlayerInAirCurrent`), and the wind (`flyingSound`) with its volume.
    static IN_CURRENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static WIND: std::cell::Cell<Option<(SoundRef, f32)>> = const { std::cell::Cell::new(None) };
}

/// `AirCurrentSound`'s volume steps (`fadeIn`, `fadeOut`) and its starting volume.
const WIND_STEP: f32 = 0.05;
const WIND_START: f32 = 0.01;

/// `AirCurrent.Client.enableClientPlayerSound`: a fan pushed the player (to `motion`); the wind,
/// the elytra's (`SoundEvents.ELYTRA_FLYING`, looping, at the listener, on the block source),
/// starts if it isn't playing, at a pitch of half the player's speed (0.5 to 2), and grows a
/// step toward `max_volume`. A playing sound's pitch can't be changed (`API_GAPS.md`): it keeps
/// the pitch it started with.
fn enable_wind(host: &dyn ClientHost, motion: [f64; 3], max_volume: f32) {
    IN_CURRENT.set(true);
    let mut wind = WIND
        .get()
        .filter(|(s, _)| s.state(host) != Some(SoundState::Stopped));
    if wind.is_none() {
        let speed = motion.iter().map(|v| v * v).sum::<f64>().sqrt();
        let pitch = (speed * 0.5).clamp(0.5, 2.0) as f32;
        let mut play = SoundPlay::ui("minecraft:item.elytra.flying")
            .with_volume(WIND_START)
            .with_pitch(pitch)
            .looping(0);
        play.source = SoundSource::Blocks;
        wind = host.play_local_sound(&play, 0).map(|s| (s, WIND_START));
    }
    if let Some((sound, volume)) = wind {
        let next = max_volume.min(volume + WIND_STEP);
        if next != volume {
            sound.fade(host, next, 1);
        }
        WIND.set(Some((sound, next)));
    }
}

/// `AirCurrent.Client.tickClientPlayerSounds`, each client tick: out of every current, the wind
/// fades a step, and stops once silent.
fn tick_wind(host: &dyn ClientHost) {
    if !IN_CURRENT.replace(false)
        && let Some((sound, volume)) = WIND.get()
    {
        if volume <= 0.0 {
            sound.stop(host);
            WIND.set(None);
        } else {
            let next = (volume - WIND_STEP).max(0.0);
            // A fade to 0 stops the sound when it ends.
            sound.fade(host, next, 1);
            WIND.set(Some((sound, next)));
        }
    }
}

fn random() -> f32 {
    RNG.with_borrow_mut(Rng::next_float)
}

/// The client left its world.
pub(super) fn reset() {
    shared(|s| *s = Shared::default());
    IN_CURRENT.set(false);
    WIND.set(None);
    CAUGHT.with_borrow_mut(Vec::clear);
    FLYING.set(false);
}

/// The server's `create:air_flying`.
pub(super) fn flying_from_server(data: &[u8]) {
    FLYING.set(data.first() == Some(&1));
}

/// The server's `create:air`: the fans of the level this client shows, all of them.
pub(super) fn from_server(data: &[u8]) {
    if let Some(fans) = fan::decode_seen(data) {
        shared(|s| s.fans = fans);
    }
}

/// `AirCurrent.tickAffectedEntities` for the local player, at the start of its tick: each fan
/// looks for it every 6th tick (`entitySearchCooldown`, `findEntities`) and, while it stays in
/// the current, pushes it (`setDeltaMovement`, a sneaking player an eighth as hard). A creative
/// player flying is let go (`isPlayerCreativeFlying`).
pub(super) fn push_player(host: &mut dyn ClientHost, player: mod_api::PlayerRef) {
    if host.local_player() != Some(player) {
        return;
    }
    let fans = shared(|s| s.fans.clone());
    if fans.is_empty() {
        CAUGHT.with_borrow_mut(Vec::clear);
        return;
    }
    let h: &dyn mod_api::Host = &*host;
    let e = player.entity();
    let (Some([lo, hi]), Some(at), Some(mut motion)) =
        (e.bounding_box(h), e.position(h), e.velocity(h))
    else {
        return;
    };
    let sneaking = e.is_shift_key_down(h).unwrap_or(false);
    let creative_flying = FLYING.get();
    let start = motion;
    let mut winds = Vec::new();
    CAUGHT.with_borrow_mut(|caught| {
        caught.retain(|(p, _, _)| fans.iter().any(|f| f.pos == *p));
        for f in &fans {
            let Some((blo, bhi)) = f.bounds() else {
                continue;
            };
            let inside = (0..3).all(|a| lo[a] < bhi[a] && hi[a] > blo[a]);
            let i = match caught.iter().position(|(p, _, _)| *p == f.pos) {
                Some(i) => i,
                None => {
                    caught.push((f.pos, 0, false));
                    caught.len() - 1
                }
            };
            let (_, cooldown, held) = &mut caught[i];
            if *cooldown == 0 {
                *cooldown = 5;
                *held = inside;
            } else {
                *cooldown -= 1;
            }
            if !*held || !inside || creative_flying {
                *held = false;
                continue;
            }
            motion = f.push(at, motion, sneaking);
            winds.push((motion, (f.speed.abs() / 128.0 * 0.4).clamp(0.01, 0.4)));
        }
    });
    for (motion, max_volume) in winds {
        enable_wind(&*host, motion, max_volume);
    }
    if motion != start {
        let kick = [0, 1, 2].map(|a| (motion[a] - start[a]) as f32);
        host.push_player(kick);
    }
}

/// The client's tick: each turning fan's air flow particle (`AirCurrent.tick`, half the ticks at
/// the default density), and the extra particles the air flow particles asked for.
pub(super) fn tick(host: &mut dyn ClientHost) {
    tick_wind(&*host);
    let Some(keys) = KEYS.get() else {
        return;
    };
    let (fans, extra) = shared(|s| (s.fans.clone(), std::mem::take(&mut s.extra)));
    for (kind, at, v) in extra {
        let options = match kind {
            Extra::Flame => ParticleOptions::simple(keys.flame),
            Extra::Lava => ParticleOptions::block(keys.block, "block/lava_still".into(), 0xFF_FFFF),
            Extra::SoulFireFlame => ParticleOptions::simple(keys.soul_fire_flame),
            Extra::Smoke => ParticleOptions::simple(keys.smoke),
            Extra::LargeSmoke => ParticleOptions::simple(keys.large_smoke),
            Extra::Bubble => ParticleOptions::simple(keys.bubble),
            Extra::BubblePop => ParticleOptions::simple(keys.bubble_pop),
        };
        host.add_particle(&options, at, v);
    }
    let options = ParticleOptions::simple(keys.air_flow);
    for f in &fans {
        if f.current.max <= 0.0 || random() >= DENSITY {
            continue;
        }
        // The particle's `offsetRandomly(ZERO, random, .25f)`.
        let origin = f.particle_origin();
        let at = origin.map(|v| v + f64::from((random() - 0.5) * 0.5));
        // A particle the game turned away (too far, too many) never ticks: forget the oldest.
        shared(|s| {
            if s.spawned.len() >= 256 {
                s.spawned.remove(0);
            }
            s.spawned.push((at, f.pos));
        });
        if !host.add_particle(&options, at, [0.0; 3]) {
            shared(|s| s.spawned.retain(|(p, _)| *p != at));
        }
    }
}

/// A number from 0 to 1 for a particle's tick: its position and age hashed with `salt`.
fn roll(p: &ModParticle, salt: u64) -> f32 {
    let mut h = salt.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ p.age as u64;
    for v in p.position {
        h = (h ^ v.to_bits()).wrapping_mul(0x0100_0000_01B3);
    }
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h % 1_000_000) as f32 / 1_000_000.0
}

/// `Color.mixColors(a, b, t)` as the particle's colour.
fn mix(p: &mut ModParticle, a: u32, b: u32, t: f32) {
    for (i, s) in [16, 8, 0].into_iter().enumerate() {
        let (x, y) = (((a >> s) & 255) as f32, ((b >> s) & 255) as f32);
        p.rgba[i] = (x + (y - x) * t) as u32 as f32 / 255.0;
    }
}

/// `setColor(0xEEEEEE)`.
fn plain(p: &mut ModParticle) {
    let c = f32::from(0xEEu8) / 255.0;
    p.rgba = [c, c, c, 0.25];
}

/// `AirFlowParticle`: 40 ticks, a quarter smaller, its fan the one that spawned it; it goes
/// with the current, faster nearer the fan, and is removed when it leaves it or its fan stops.
/// In plain air it is a faint white puff (`generic_7`..`generic_0` by its distance); in a
/// processing segment it takes the type's colour and now and then lets out that type's
/// particle (`morphAirFlow`).
pub(super) struct AirFlow;

impl ParticleProvider for AirFlow {
    fn tick(&self, p: &mut ModParticle) {
        if p.state[3] == 0.0 {
            // The spawn: its fan, from where `tick` put it.
            let Some(fan) = shared(|s| {
                let i = s.spawned.iter().position(|(at, _)| *at == p.position)?;
                Some(s.spawned.swap_remove(i).1)
            }) else {
                p.removed = true;
                return;
            };
            p.state = [f64::from(fan[0]), f64::from(fan[1]), f64::from(fan[2]), 1.0];
            // `quadSize *= 0.75` of `Particle`'s `0.1 * (random * .5 + .5) * 2`.
            p.size = 0.1 * (roll(p, 1) * 0.5 + 0.5) * 2.0 * 0.75;
            p.lifetime = 40;
            p.velocity = [0.0; 3];
            plain(p);
            // `selectSprite(7)`: `sprites.get(7, 8)` of the 8.
            p.frame = 7 * 7 / 8;
        }
        let fan = [p.state[0] as i32, p.state[1] as i32, p.state[2] as i32];
        let Some(seen) = shared(|s| s.fans.iter().find(|f| f.pos == fan).cloned()) else {
            p.removed = true;
            return;
        };
        let Some((motion, kind)) = seen.particle_step(p.position) else {
            p.removed = true;
            return;
        };
        let pick = |p: &ModParticle, salt| roll(p, salt);
        match kind {
            None => {
                plain(p);
                let max = f64::from(seen.current.max).max(1e-3);
                let o = seen.facing.offset();
                let distance = (0..3)
                    .map(|a| {
                        ((p.position[a] - (f64::from(fan[a]) + 0.5)) * f64::from(o[a])).powi(2)
                    })
                    .sum::<f64>()
                    .sqrt()
                    - 0.5;
                let i = ((distance / max) * 8.0 + f64::from((pick(p, 2) * 4.0) as i32))
                    .clamp(0.0, 7.0) as u32;
                p.frame = i * 7 / 8;
            }
            Some(kind) => {
                let t = pick(p, 3);
                let mut extra = Vec::new();
                let speed = |m: f64| motion.map(|v| v * m);
                match kind {
                    Kind::Blasting => {
                        mix(p, 0xFF4400, 0xFF8855, t);
                        p.rgba[3] = 0.5;
                        if pick(p, 4) < 1.0 / 32.0 {
                            extra.push((Extra::Flame, speed(0.25)));
                        }
                        if pick(p, 5) < 1.0 / 16.0 {
                            extra.push((Extra::Lava, speed(0.25)));
                        }
                    }
                    Kind::Haunting => {
                        mix(p, 0x0, 0x126568, t);
                        p.rgba[3] = 1.0;
                        if pick(p, 4) < 1.0 / 128.0 {
                            extra.push((Extra::SoulFireFlame, speed(0.125)));
                        }
                        if pick(p, 5) < 1.0 / 32.0 {
                            extra.push((Extra::Smoke, speed(0.125)));
                        }
                    }
                    Kind::Smoking => {
                        mix(p, 0x0, 0x555555, t);
                        p.rgba[3] = 1.0;
                        if pick(p, 4) < 1.0 / 32.0 {
                            extra.push((Extra::Smoke, speed(0.125)));
                        }
                        if pick(p, 5) < 1.0 / 32.0 {
                            extra.push((Extra::LargeSmoke, speed(0.125)));
                        }
                    }
                    Kind::Splashing => {
                        mix(p, 0x4499FF, 0x2277FF, t);
                        p.rgba[3] = 1.0;
                        if pick(p, 4) < 1.0 / 32.0 {
                            extra.push((Extra::Bubble, speed(0.125)));
                        }
                        if pick(p, 5) < 1.0 / 32.0 {
                            extra.push((Extra::BubblePop, speed(0.125)));
                        }
                    }
                }
                // `selectSprite(random.nextInt(3))`.
                p.frame = ((pick(p, 6) * 3.0) as u32).min(2) * 7 / 8;
                if !extra.is_empty() {
                    let at = p.position;
                    shared(|s| s.extra.extend(extra.into_iter().map(|(e, v)| (e, at, v))));
                }
            }
        }
        // `move(xd, yd, zd)` without physics.
        p.velocity = motion;
        for (v, m) in p.position.iter_mut().zip(motion) {
            *v += m;
        }
    }
}
