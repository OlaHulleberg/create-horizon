//! Gauges and flywheels: the kinetic parts whose angle the client animates itself.
//!
//! A gauge's dial target (`GaugeBlockEntity.dialTarget`) and what its goggle tooltip reads go to
//! its clients in its block entity's data (`Gauge`), after each solve that moved it
//! (`GaugeBlockEntity.sendData`); each client eases its needle toward it
//! (`GaugeBlockEntity.tick`) and draws the dials at it (`GaugeVisual.DialFace`). A flywheel's
//! wheel chases its shaft's speed on the client (`FlywheelBlockEntity.visualSpeed`). Both are
//! drawn here, on the `create:kinetic` type, in place of the parts `kinetics` draws at rest; while
//! a needle, wheel, valve pointer or hose moves, its block is drawn again every frame instead of
//! being kept.

use super::kinetics::{self, axis_vector, speeds_of};
use super::server::ServerLevel;
use super::states;
use super::subsystem::Subsystem;
use super::world::with_world;
use create_engine::Block;
use create_engine::client::{Draw, Model, WheelSpin};
use create_engine::gauge::{Needle, Shown, dial_faces, dial_pose};
use create_engine::parts::StateInfo;
use mod_api::{
    BlockEntityClientTickEvent, BlockEntityClientTicksEvent, BlockEntityPart,
    BlockEntitySubmitEvent, BlockPart, ClientHost, Compound, ModBlockEntity, ParticleOptions,
    Registrar, RegistrationContext, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

/// `minecraft:dust`, for the gauges' puffs.
static DUST: OnceLock<mod_api::ParticleKey> = OnceLock::new();

/// The key in a gauge's block entity data: dial, speed, stress, capacity, overstressed (0 or 1).
pub const GAUGE: &str = "Gauge";

/// The keys `kinetics` sends to clients besides the speeds: this module's and the fluids'.
pub const SYNCED: &[&str] = &[
    GAUGE,
    super::fluids::TANK,
    super::fluids::TANK_FLUIDS,
    super::fluids::HOSE,
];

pub struct Gauges;

impl Subsystem for Gauges {
    fn synced_keys(&self) -> &'static [&'static str] {
        SYNCED
    }

    fn draws_itself(&self, block: Block, part: &Draw) -> bool {
        draws_itself(block, part)
    }

    fn animated(&self, block: Block, pos: [i32; 3], entity: &ModBlockEntity) -> bool {
        animated(block, pos, entity)
    }

    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        let _ = DUST.set(r.particle_key("minecraft:dust"));
        let Some(kind) = kinetics::kind() else {
            return;
        };
        if let Err(e) = r.block_entity_part(&kind, Animated) {
            crate::create_log!("[create] the gauges' block entity part was refused: {e:?}");
        }
    }

    /// After the solve: gauges whose dial moved tell their clients, and what their comparators
    /// read may have changed too.
    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        let dim = lv.dim;
        let Some(changed) = with_world(|w| {
            let gauges = w.take_gauge_changes(dim);
            w.dims[dim].analog_checks.extend_from_slice(&gauges);
            gauges
                .iter()
                .filter_map(|p| Some((*p, w.dims[dim].gauges.get(p)?.shown)))
                .collect::<Vec<_>>()
        }) else {
            return;
        };
        for (pos, shown) in changed {
            let Some(entity) = lv.host.block_entity(lv.level, pos) else {
                continue;
            };
            let data = Compound::from([(GAUGE.into(), shown_to_tag(&shown))]);
            let _ = entity.merge_data(&mut *lv.host, data);
        }
    }

    fn client_reset(&self) {
        CLIENT.with_borrow_mut(|c| *c = Client::default());
    }

    /// `GaugeBlockEntity.addToGoggleTooltip`: what the server last said of the gauge, in its
    /// block entity's data.
    fn goggle_tooltip(
        &self,
        host: &dyn ClientHost,
        pos: [i32; 3],
        block: Block,
        lines: &mut Vec<Vec<(String, u32)>>,
    ) {
        let Some(kind) = create_engine::world::gauge_kind(block) else {
            return;
        };
        if block == Block::Stressometer {
            super::advancements::gauge_read(pos);
        }
        let shown = host
            .level(host.dimension())
            .and_then(|l| host.block_entity(l, pos))
            .and_then(|e| e.data(host))
            .and_then(|d| shown_of(&d))
            .unwrap_or_default();
        lines.extend(create_engine::gauge::tooltip(kind, &shown));
    }
}

fn shown_to_tag(s: &Shown) -> Tag {
    let over = if s.overstressed { 1.0f32 } else { 0.0 };
    Tag::List(
        [s.dial, s.speed, s.stress, s.capacity, over]
            .iter()
            .map(|v| Tag::Float(v.to_bits()))
            .collect(),
    )
}

/// What a gauge's block entity data says it shows (the goggles read it on the client).
pub fn shown_of(data: &Compound) -> Option<Shown> {
    let Some(Tag::List(l)) = data.get(&GAUGE.into()) else {
        return None;
    };
    let at = |i: usize| match l.get(i) {
        Some(Tag::Float(bits)) => f32::from_bits(*bits),
        _ => 0.0,
    };
    Some(Shown {
        dial: at(0),
        speed: at(1),
        stress: at(2),
        capacity: at(3),
        overstressed: at(4) != 0.0,
    })
}

/// A needle on a client, and whether it still moves (the only ones ticked and drawn each frame).
#[derive(Clone, Copy, Default)]
struct ClientNeedle {
    needle: Needle,
    moving: bool,
}

/// A flywheel's wheel on a client: its drawn speed, and its angle at an animation time.
#[derive(Clone, Copy, Default)]
struct ClientWheel {
    spin: WheelSpin,
    /// Degrees at `at` (ticks of the animation clock), turning at `drawn` RPM since.
    angle: f64,
    at: f64,
    drawn: f32,
    moving: bool,
}

#[derive(Default)]
struct Client {
    needles: HashMap<[i32; 3], ClientNeedle>,
    wheels: HashMap<[i32; 3], ClientWheel>,
}

thread_local! {
    static CLIENT: RefCell<Client> = RefCell::new(Client::default());
    /// The gauges' dust's random source (the client level's `random` in Create).
    static DUST_RNG: RefCell<create_engine::processing::Rng> =
        RefCell::new(create_engine::processing::Rng::new(0x6A06_E5D0));
}

/// Whether `kinetics` leaves part `d` of `block` to this module: a gauge's dials and a
/// flywheel's wheel.
pub fn draws_itself(block: Block, d: &Draw) -> bool {
    match block {
        Block::Speedometer | Block::Stressometer => matches!(d.model, Model::GaugeDial(_)),
        Block::Flywheel => d.slot == 1,
        _ => false,
    }
}

/// Whether the block entity of `block` at `pos` is animated this frame, so its drawing is not
/// kept: a moving needle, wheel, valve pointer or hose.
pub fn animated(block: Block, pos: [i32; 3], entity: &ModBlockEntity) -> bool {
    match block {
        // A needle still moving, or one whose target the server just changed (its client tick
        // sets it off after this drawing, which must not be kept).
        Block::Speedometer | Block::Stressometer => {
            let target = shown_of(&entity.nbt).map_or(0.0, |s| s.dial);
            CLIENT.with_borrow(|c| {
                c.needles
                    .get(&pos)
                    .map_or(target != 0.0, |n| n.moving || n.needle.target != target)
            })
        }
        Block::Flywheel => CLIENT.with_borrow(|c| c.wheels.get(&pos).is_none_or(|w| w.moving)),
        _ => super::fluids::animated(block, pos, entity),
    }
}

/// The block and state info of `state` on the client.
fn client_block(host: &dyn ClientHost, state: mod_api::BlockState) -> Option<(Block, StateInfo)> {
    let id = states::id(state)?;
    super::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
}

/// Draws gauges' dials and flywheels' wheels; ticks their needles and wheels on the client.
struct Animated;

#[mod_api::horizon_part]
impl BlockPart for Animated {}

#[mod_api::horizon_part]
impl BlockEntityPart for Animated {
    fn block_entity_client_ticks(
        &self,
        _host: &dyn ClientHost,
        event: &mut BlockEntityClientTicksEvent<'_>,
    ) {
        if matches!(
            event.block,
            "create:speedometer" | "create:stressometer" | "create:flywheel"
        ) {
            event.ticks = true;
        }
    }

    fn block_entity_client_tick(
        &self,
        host: &mut dyn ClientHost,
        event: &mut BlockEntityClientTickEvent<'_>,
    ) {
        let Some((block, _)) = client_block(&*host, event.state) else {
            return;
        };
        let pos = event.pos;
        match block {
            Block::Speedometer | Block::Stressometer => {
                let Some(shown) = shown_of(&event.entity.nbt) else {
                    return;
                };
                let time = event
                    .block_entity
                    .level(&*host)
                    .and_then(|l| l.game_time(&*host))
                    .unwrap_or(0) as u64;
                if let Some((block, info)) = client_block(&*host, event.state) {
                    dust(host, block, &info, pos, &shown);
                }
                if block == Block::Stressometer {
                    super::advancements::gauge_tick(host, pos, shown.dial);
                }
                CLIENT.with_borrow_mut(|c| {
                    let n = c.needles.entry(pos).or_default();
                    // The server's target: the needle sets off toward it.
                    if n.needle.target != shown.dial {
                        n.needle.target = shown.dial;
                        n.moving = true;
                    }
                    if !n.moving {
                        return;
                    }
                    let seed = time ^ create_engine::client::key(pos, 0);
                    let mut rng = create_engine::processing::Rng::new(seed);
                    n.needle.tick(&mut rng);
                    // `GaugeVisual.beginFrame` leaves a needle once `Mth.equal`.
                    n.moving = !n.needle.settled();
                });
            }
            Block::Flywheel => {
                let target = speeds_of(&event.entity.nbt)[0];
                CLIENT.with_borrow_mut(|c| {
                    let w = c.wheels.entry(pos).or_default();
                    // `tickChaser`: a step too small to move it leaves it for good.
                    w.moving = w.spin.tick(target);
                });
            }
            _ => {}
        }
    }

    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some((block, info)) = client_block(host, event.state) else {
            return;
        };
        match block {
            Block::Speedometer | Block::Stressometer => draw_dials(event, &info),
            Block::Flywheel => draw_wheel(host, event, &info),
            _ => {}
        }
    }
}

/// `GaugeBlock.animateTick`: a gauge whose dial is off zero puffs dust of its colour off each
/// face with a dial, one at a time a quarter of the times it is asked, four at a time past its
/// end. The game asks a block near the player about once in 44 ticks (`ClientLevel.animateTick`:
/// 667 random blocks within 16, and 667 within 32), so each client tick asks with that chance.
fn dust(host: &mut dyn ClientHost, block: Block, info: &StateInfo, pos: [i32; 3], shown: &Shown) {
    const ASKED: f32 = 667.0 / 32768.0 + 667.0 / 262_144.0;
    let (Some(&key), Some(facing)) = (DUST.get(), info.facing) else {
        return;
    };
    if shown.dial == 0.0 {
        return;
    }
    let next = || DUST_RNG.with_borrow_mut(|r| r.next_float());
    if next() >= ASKED {
        return;
    }
    let color = match block {
        Block::Speedometer => create_engine::gauge::speed_color(shown.speed),
        _ => create_engine::gauge::stress_color(shown.dial, 0),
    };
    let rgb = [16, 8, 0].map(|s| ((color >> s) & 0xFF) as f32 / 255.0);
    let options = ParticleOptions::dust(key, rgb, 1.0);
    let axis = create_engine::parts::directional_axis(facing, info.along_first);
    let count = if shown.dial > 1.0 { 4 } else { 1 };
    for face in dial_faces(facing, axis) {
        if count == 1 && next() > 0.25 {
            continue;
        }
        let normal = face.offset().map(f64::from);
        for _ in 0..count {
            // `VecHelper.offsetRandomly(ZERO, r, .25)` in the face's plane, normalised, at .3.
            let mut m = [0; 3].map(|_| f64::from((next() - 0.5) * 2.0 * 0.25));
            for (c, n) in m.iter_mut().zip(normal) {
                if n != 0.0 {
                    *c = 0.0;
                }
            }
            let len = m.iter().map(|c| c * c).sum::<f64>().sqrt();
            if len == 0.0 {
                continue;
            }
            let m = m.map(|c| c / len * 0.3);
            let at = [0, 1, 2].map(|i| f64::from(pos[i]) + 0.5 + normal[i] * 0.55 + m[i]);
            host.add_particle(&options, at, m);
        }
    }
}

/// `GaugeVisual.DialFace.updateTransform`: each shown face's dial turned about its pivot to the
/// needle, between its last two ticks while it moves.
fn draw_dials(event: &mut BlockEntitySubmitEvent<'_>, info: &StateInfo) {
    let Some(facing) = info.facing else {
        return;
    };
    // A gauge first seen sets off from 0 toward its target.
    let target = shown_of(&event.entity.nbt).map_or(0.0, |s| s.dial);
    let n = CLIENT.with_borrow_mut(|c| {
        *c.needles.entry(event.pos).or_insert_with(|| ClientNeedle {
            needle: Needle {
                target,
                ..Needle::default()
            },
            moving: target != 0.0,
        })
    });
    let progress = if n.moving {
        let t = event.draw.partial();
        n.needle.prev + (n.needle.state - n.needle.prev) * t
    } else {
        n.needle.state
    };
    draw_dials_at(event, facing, info.along_first, progress);
}

fn draw_dials_at(
    event: &mut BlockEntitySubmitEvent<'_>,
    facing: create_engine::kinetics::Dir,
    along_first: bool,
    progress: f32,
) {
    let axis = create_engine::parts::directional_axis(facing, along_first);
    let Some(dial) = states::partial("part_gauge_dial") else {
        return;
    };
    for face in dial_faces(facing, axis) {
        let pose = dial_pose(face, progress);
        let draw = &mut *event.draw;
        draw.push();
        draw.translate(pose.offset);
        draw.rotate_about(axis_vector(pose.axis), pose.angle, [0.5; 3]);
        draw.block(dial, &[("facing", kinetics::dir_name(face))]);
        draw.pop();
    }
}

/// `FlywheelRenderer`'s wheel: the block's own model turning at its drawn speed. Kept turning
/// on the animation clock once it has caught its shaft up; drawn at its angle each frame while
/// it eases.
fn draw_wheel(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>, info: &StateInfo) {
    let Some(axis) = info.axis else {
        return;
    };
    let now = event.draw.animation_time();
    let target = speeds_of(&event.entity.nbt)[0];
    let w = CLIENT.with_borrow_mut(|c| {
        // A wheel first seen starts still, from angle 0, and spins up to its shaft.
        let w = c.wheels.entry(event.pos).or_insert_with(|| ClientWheel {
            at: now,
            moving: target != 0.0,
            ..ClientWheel::default()
        });
        // The angle carried on to now at the speed it was drawn at.
        w.angle = (w.angle + f64::from(w.drawn) * 0.3 * (now - w.at)).rem_euclid(360.0);
        w.at = now;
        w.drawn = w.spin.visual;
        *w
    });
    let text = host.block_state_string(event.state).unwrap_or_default();
    let (_, own) = states::split(&text);
    let draw = &mut *event.draw;
    let v = axis_vector(axis);
    if w.moving {
        draw.push();
        draw.rotate_about(v, w.angle as f32, [0.5; 3]);
        draw.block(states::key(Block::Flywheel), &own);
        draw.pop();
    } else {
        let rate = w.spin.visual * 0.3;
        let phase = (w.angle - now * f64::from(rate)).rem_euclid(360.0) as f32;
        draw.spin(v, [0.5; 3], rate, phase);
        draw.block(states::key(Block::Flywheel), &own);
        draw.still();
        draw.keep();
    }
}
