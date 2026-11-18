//! Client rendering and publication of the engine-owned pipe connections.

use super::super::server::ServerLevel;
use super::super::world::with_world;
use create_engine::fluids::transport::Source;
use create_engine::kinetics::Dir;
use create_engine::tank::{Codes, Fluid};
use mod_api::{
    BlockEntitySubmitEvent, ClientHost, ModBlockEntity, ParticleKey, ParticleOptions, Registrar,
    Tag,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Client flow samples: side (`Dir::ALL` index), fluid (`tank::Codes`, with
/// potion keys in `FLOW_FLUIDS`), inbound/open-end flags, publication time,
/// negative current progress divided by speed, speed bits, and an optional end
/// time. Each sample comes from the live connection; the client interpolates
/// its stored progress between publications.
pub const FLOW: &str = "Flow";

/// The key of the potions' keys beside `FLOW` (`super::fluid_keys`).
pub const FLOW_FLUIDS: &str = "Flow_fluids";

/// Ints per flow in `FLOW`.
const PER_FLOW: usize = 7;

type At = (usize, [i32; 3]);
type Published = Vec<(Dir, create_engine::fluids::transport::SavedConnection)>;
thread_local! {
    static SHOWN: RefCell<HashMap<At,Published>> = RefCell::new(HashMap::new());
}
pub(super) fn reset() {
    SHOWN.with_borrow_mut(HashMap::clear);
}
pub(super) fn forget(dim: usize, pos: [i32; 3]) {
    SHOWN.with_borrow_mut(|s| {
        s.remove(&(dim, pos));
    });
}
pub(super) fn forget_column(dim: usize, chunk: [i32; 2]) {
    SHOWN.with_borrow_mut(|s| {
        s.retain(|(d, p), _| *d != dim || (p[0] >> 4, p[2] >> 4) != (chunk[0], chunk[1]))
    });
}
pub(super) fn has_flow(dim: usize, pos: [i32; 3]) -> bool {
    with_world(|w| {
        w.dims[dim]
            .fluid_transport
            .pipes
            .get(&pos)
            .is_some_and(|p| p.interfaces.iter().any(|(_, c)| c.flow.is_some()))
    })
    .unwrap_or(false)
}
pub(super) fn provided(dim: usize, pos: [i32; 3], _now: i64) -> Vec<(Dir, Fluid)> {
    with_world(|w| {
        w.dims[dim].fluid_transport.pipes.get(&pos).map(|p| {
            p.interfaces
                .iter()
                .filter_map(|(d, c)| c.provided_fluid(true).map(|f| (*d, f)))
                .collect()
        })
    })
    .flatten()
    .unwrap_or_default()
}
pub(super) fn send(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let now = lv.game_time();
    let records = with_world(|w| {
        w.dims[dim]
            .fluid_transport
            .pipes
            .iter()
            .map(|(p, b)| (*p, b.clone()))
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    for (pos, b) in records {
        let saved = b.write();
        if SHOWN.with_borrow(|s| s.get(&(dim, pos)).is_some_and(|old| *old == saved)) {
            continue;
        }
        let mut v = Vec::new();
        let mut codes = Codes::default();
        for (d, c) in &b.interfaces {
            let Some(f) = c.flow else {
                continue;
            };
            let speed = create_engine::fluids::flow_speed(c.pressure()[usize::from(!f.inbound)]);
            v.extend([
                Dir::ALL.iter().position(|x| *x == *d).unwrap() as i32,
                codes.code(Some(f.fluid)),
                i32::from(f.inbound) | (i32::from(matches!(c.source, Some(Source::Open(_)))) << 1),
                now as i32,
                (-f.progress / speed).to_bits() as i32,
                speed.to_bits() as i32,
                i32::MAX,
            ]);
        }
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let mut data = super::connections::encode(&b);
        data.insert(FLOW.into(), Tag::IntArray(v.clone()));
        data.insert(
            FLOW_FLUIDS.into(),
            Tag::List(
                codes
                    .keys
                    .iter()
                    .map(|k| Tag::String(k.as_str().into()))
                    .collect(),
            ),
        );
        let _ = entity.merge_data(&mut *lv.host, data);
        SHOWN.with_borrow_mut(|s| {
            s.insert((dim, pos), saved);
        });
    }
}

/// A flow as a client reads it from its pipe's block entity.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shown {
    side: Dir,
    fluid: Fluid,
    inbound: bool,
    open: bool,
    /// The game time it starts.
    from: f64,
    speed: f32,
    /// The game time it goes.
    to: Option<i64>,
}

impl Shown {
    /// How far it is at game time `now` (with the partial tick); none before it starts or
    /// once it is gone.
    fn progress(&self, now: f64) -> Option<f32> {
        if now < self.from || self.to.is_some_and(|t| now >= t as f64) {
            return None;
        }
        Some((((now - self.from) * f64::from(self.speed)) as f32).clamp(0.0, 1.0))
    }
}

fn shown(entity: &ModBlockEntity) -> Vec<Shown> {
    let Some(Tag::IntArray(v)) = entity.nbt.get(&FLOW.into()) else {
        return Vec::new();
    };
    let keys = super::fluid_keys(&entity.nbt, FLOW);
    v.as_chunks::<PER_FLOW>()
        .0
        .iter()
        .filter_map(|c| {
            let side = *Dir::ALL.get(c[0].clamp(0, 5) as usize)?;
            let fluid = Codes::fluid(&keys, c[1])?;
            let speed = f32::from_bits(c[5] as u32);
            let start = f32::from_bits(c[4] as u32);
            if !(speed.is_finite() && speed > 0.0 && start.is_finite()) {
                return None;
            }
            Some(Shown {
                side,
                fluid,
                inbound: c[2] & 1 != 0,
                open: c[2] & 2 != 0,
                from: f64::from(c[3]) + f64::from(start),
                speed,
                to: (c[6] != i32::MAX).then_some(i64::from(c[6])),
            })
        })
        .collect()
}

thread_local! {
    /// The client level's game time at its last tick.
    static CLIENT_NOW: Cell<i64> = const { Cell::new(0) };
}

/// The client's tick: the time its pipes' flows are measured by.
pub(super) fn client_tick(host: &dyn ClientHost) {
    if let Some(t) = host.level(host.dimension()).and_then(|l| l.game_time(host)) {
        CLIENT_NOW.set(t);
    }
}

/// Whether a pipe's flows move this frame: one growing or yet to start or go.
pub(super) fn animated(entity: &ModBlockEntity) -> bool {
    let now = CLIENT_NOW.get() as f64;
    shown(entity).iter().any(|s| {
        let grown = s.from + 1.0 / f64::from(s.speed);
        now <= grown + 1.0 || s.to.is_some()
    })
}

/// `TransparentStraightPipeRenderer`: the stream in a glass pipe, each flow from the side it
/// comes in by to the middle, or from the middle out, as far as it got (`renderFluidStream`, 3/16
/// of a block from the pipe's middle). Two whole halves along the axis are drawn as one.
pub(super) fn draw(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let flows = shown(event.entity);
    if flows.is_empty() {
        return;
    }
    let now = event.draw.game_time() as f64 + f64::from(event.draw.partial());
    let drawn: Vec<(Shown, f32)> = flows
        .iter()
        .filter_map(|s| s.progress(now).filter(|p| *p > 0.0).map(|p| (*s, p)))
        .collect();
    let r = 3.0 / 16.0;
    let mut done = 0u8;
    for (s, p) in &drawn {
        if done & s.side.bit() != 0 {
            continue;
        }
        let i = axis_index(s.side);
        let (mut min, mut max) = ([0.5 - r; 3], [0.5 + r; 3]);
        // Its whole other half: one box through the pipe.
        let other = drawn
            .iter()
            .find(|(o, q)| o.side == s.side.opposite() && *q >= 1.0 && o.fluid == s.fluid);
        if *p >= 1.0 && other.is_some() {
            min[i] = 0.0;
            max[i] = 1.0;
            done |= s.side.bit() | s.side.opposite().bit();
        } else {
            let face = if s.side.offset()[i] > 0 { 1.0 } else { 0.0 };
            let length = 0.5 * p;
            // Inbound grows from the face in, outward from the middle out.
            let (a, b) = if s.inbound {
                (face, face + (0.5 - face) * 2.0 * length)
            } else {
                (0.5, 0.5 + (face - 0.5) * 2.0 * length)
            };
            min[i] = a.min(b);
            max[i] = a.max(b);
            done |= s.side.bit();
        }
        super::client::fluid_box(host, &mut *event.draw, s.fluid, 250, min, max);
    }
}

fn axis_index(d: Dir) -> usize {
    match d.axis() {
        create_engine::kinetics::Axis::X => 0,
        create_engine::kinetics::Axis::Y => 1,
        create_engine::kinetics::Axis::Z => 2,
    }
}

/// The particle types the flows spawn.
struct Keys {
    item: ParticleKey,
    block: ParticleKey,
    dripping_water: ParticleKey,
    dripping_lava: ParticleKey,
}

static KEYS: OnceLock<Keys> = OnceLock::new();

pub(super) fn register(r: &mut dyn Registrar) {
    let _ = KEYS.set(Keys {
        item: r.particle_key("minecraft:item"),
        block: r.particle_key("minecraft:block"),
        dripping_water: r.particle_key("minecraft:dripping_water"),
        dripping_lava: r.particle_key("minecraft:dripping_lava"),
    });
}

thread_local! {
    static RNG: RefCell<create_engine::processing::Rng> =
        RefCell::new(create_engine::processing::Rng::new(0xF10D));
}

fn random() -> f64 {
    RNG.with_borrow_mut(|r| f64::from(r.next_float()))
}

/// `PipeConnection.MAX_PARTICLE_RENDER_DISTANCE`.
const PARTICLE_DISTANCE: f64 = 20.0;
/// `PipeConnection.IDLE_PARTICLE_SPAWN_CHANCE`.
const IDLE_CHANCE: f64 = 1.0 / 1000.0;
/// `PipeConnection.RIM_RADIUS`.
const RIM_RADIUS: f64 = 1.0 / 4.0 + 1.0 / 64.0;

/// `FluidFX.getFluidParticle`: Create's fluid particle (see the module's doc).
fn fluid_particle(keys: &Keys, fluid: Fluid) -> Option<ParticleOptions> {
    let tint = match fluid {
        Fluid::Water => Some(0x3F_76E4),
        Fluid::Potion(p) => Some(p.color()),
        _ => None,
    };
    let sprite = still_sprite(fluid);
    Some(match tint {
        Some(tint) => ParticleOptions::block(keys.block, sprite.into(), tint),
        None => ParticleOptions::item(keys.item, vec![sprite.into()]),
    })
}

/// The fluid's still sprite (`IClientFluidTypeExtensions.getStillTexture`).
fn still_sprite(fluid: Fluid) -> &'static str {
    match fluid {
        Fluid::Water => "block/water_still",
        Fluid::Lava => "block/lava_still",
        Fluid::Honey => "create:block/fluid/honey_still",
        Fluid::Chocolate => "create:block/fluid/chocolate_still",
        Fluid::Milk => "create:block/fluid/milk_still",
        Fluid::Tea => "create:block/fluid/tea_still",
        Fluid::Potion(_) => "create:block/fluid/potion_still",
    }
}

/// `FluidFX.getDrippingParticle`: the game's dripping water or lava, else the fluid particle.
fn dripping_particle(keys: &Keys, fluid: Fluid) -> Option<ParticleOptions> {
    match fluid {
        Fluid::Water => Some(ParticleOptions::simple(keys.dripping_water)),
        Fluid::Lava => Some(ParticleOptions::simple(keys.dripping_lava)),
        f => fluid_particle(keys, f),
    }
}

/// A pipe's client tick (`tickFlowProgress` on the client): each flow there pours at an open end
/// or now and then drips on its rim (`spawnParticles`), and splashes on its rim the tick it
/// arrives (`spawnSplashOnRim`), within 20 blocks of the camera.
pub(super) fn client_pipe_tick(host: &mut dyn ClientHost, pos: [i32; 3], entity: &ModBlockEntity) {
    let flows = shown(entity);
    if flows.is_empty() {
        return;
    }
    let Some(keys) = KEYS.get() else {
        return;
    };
    let center = pos.map(|c| f64::from(c) + 0.5);
    let near = host.camera().is_some_and(|c| {
        let d = [0, 1, 2].map(|i| c.position[i] - center[i]);
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() <= PARTICLE_DISTANCE
    });
    if !near {
        return;
    }
    let now = CLIENT_NOW.get() as f64;
    for s in flows {
        if s.progress(now).is_none() {
            continue;
        }
        let arrived = now - 1.0 < s.from;
        if s.open {
            if let Some(o) = fluid_particle(keys, s.fluid) {
                pour(host, &o, center, s.side, s.inbound);
            }
            continue;
        }
        if (arrived || random() < IDLE_CHANCE)
            && let Some(o) = dripping_particle(keys, s.fluid)
        {
            rim(host, &o, center, s.side);
        }
    }
}

/// `FluidFX.spawnRimParticles`, one: somewhere on the rim of `side`, drifting out.
fn rim(host: &mut dyn ClientHost, o: &ParticleOptions, center: [f64; 3], side: Dir) {
    let dir = side.offset().map(f64::from);
    // `offsetRandomly(ZERO, r, 1).normalize()`, clamped to the rim, on the side's plane.
    let mut v = [0.0; 3].map(|_: f64| (random() - 0.5) * 2.0);
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
    let out = 0.45 + random() / 16.0;
    for i in 0..3 {
        let c = (v[i] / len).clamp(-RIM_RADIUS, RIM_RADIUS);
        v[i] = if dir[i] != 0.0 { 0.0 } else { c } + dir[i] * out;
    }
    let m = v.map(|c| c * 0.05);
    let at = [
        center[0] + v[0],
        center[1] + v[1] - 1.0 / 16.0,
        center[2] + v[2],
    ];
    host.add_particle(o, at, m);
}

/// `FluidFX.spawnPouringLiquid`, one: a drop leaving the rim of the open `side`, or flying back
/// into the pipe when it is drawn in (`inbound`).
fn pour(
    host: &mut dyn ClientHost,
    o: &ParticleOptions,
    center: [f64; 3],
    side: Dir,
    inbound: bool,
) {
    let dir = side.offset().map(f64::from);
    // `offsetRandomly(ZERO, r, RIM_RADIUS * .75)` across the side, then out to the rim.
    let across = RIM_RADIUS * 0.75;
    let mut v = [0.0; 3];
    for i in 0..3 {
        let a = (random() - 0.5) * 2.0 * across;
        v[i] = if dir[i] != 0.0 { 0.0 } else { a };
    }
    let out = 0.5 + random() / 4.0;
    for i in 0..3 {
        v[i] += dir[i] * out;
    }
    let mut m = v.map(|c| c / 4.0);
    let mut at = [0, 1, 2].map(|i| center[i] + v[i]);
    if inbound {
        at = [0, 1, 2].map(|i| at[i] + m[i]);
        m = [0, 1, 2].map(|i| (center[i] + dir[i] * 0.5 - at[i]) / 16.0);
    }
    at[1] -= 1.0 / 16.0;
    host.add_particle(o, at, m);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(from: f64, speed: f32, to: Option<i64>) -> Shown {
        Shown {
            side: Dir::East,
            fluid: Fluid::Water,
            inbound: true,
            open: false,
            from,
            speed,
            to,
        }
    }

    #[test]
    fn a_flow_grows_from_when_it_starts_and_is_gone_when_it_goes() {
        let s = shown(10.0, 0.25, Some(20));
        assert_eq!(s.progress(9.0), None);
        assert_eq!(s.progress(10.0), Some(0.0));
        assert_eq!(s.progress(12.0), Some(0.5));
        assert_eq!(s.progress(19.0), Some(1.0));
        assert_eq!(s.progress(20.0), None);
    }
}
