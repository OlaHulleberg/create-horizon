//! The handles of hand cranks and valve handles, drawn at an angle of their own
//! (`HandCrankBlockEntity.getIndependentAngle`, `ValveHandleBlockEntity.getIndependentAngle`)
//! rather than at their shaft's.
//!
//! A crank's handle chases its speed on the client (`chasingAngularVelocity`, a quarter of the
//! way each tick) and stays where it stopped. A valve handle turns from 0 to its use's target (a
//! quarter or half turn) over the ticks of the use, and stays there; while something else turns
//! it, it turns with its network. What a use started (`ValveUse`) and the angle its value box set
//! (`ScrollValue`, as Create saves `angleInput`) are kept in its block entity data, which its
//! clients get.

use super::super::kinetics::{self, axis_vector, speeds_of};
use super::super::server::ServerLevel;
use super::super::states;
use super::super::world::with_world;
use create_engine::Block;
use create_engine::client::{Draw, Model};
use create_engine::dye;
use create_engine::kinetics::Dir;
use create_engine::valve;
use mod_api::{
    BlockEntityClientTickEvent, BlockEntityClientTicksEvent, BlockEntityPart,
    BlockEntitySubmitEvent, BlockPart, CanSurviveEvent, ClientHost, Compound, HostSide, LevelHost,
    ModBlockEntity, OnChunkUnloadedEvent, Registrar, SoundPlay, SoundSource, Tag,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// A valve handle's angle setting in its block entity data (`ScrollValueBehaviour`'s key).
pub const SCROLL_VALUE: &str = "ScrollValue";

/// What a valve handle's last use started: `Start` (game time), `Ticks`, `Target` (degrees),
/// `Back` (used sneaking).
pub const VALVE_USE: &str = "ValveUse";

/// Ticks after a use's end during which the handle still stands at its target, not turning with
/// its network: the speeds that stop it reach the client a tick or two after the use's time.
const SETTLE: i64 = 3;

/// A valve handle's angle setting: its value box's, else Create's 45°.
pub fn valve_angle(data: &Compound) -> i32 {
    match data.get(&SCROLL_VALUE.into()) {
        Some(Tag::Int(v)) if *v != 0 => (*v).clamp(-180, 180),
        _ => valve::DEFAULT_ANGLE,
    }
}

/// The data a valve handle's use at game time `start` with angle `value` gives its block entity.
pub fn valve_use(start: i64, value: i32, back: bool) -> Compound {
    let used = Compound::from([
        ("Start".into(), Tag::Long(start)),
        ("Ticks".into(), Tag::Int(i32::from(valve::use_ticks(value)))),
        ("Target".into(), Tag::Int(valve::target_angle(value))),
        ("Back".into(), Tag::Byte(i8::from(back))),
    ]);
    Compound::from([(VALVE_USE.into(), Tag::Compound(used))])
}

/// A valve handle's last use: start, ticks, target, backwards.
fn use_of(data: &Compound) -> Option<(i64, u8, i32, bool)> {
    let Some(Tag::Compound(u)) = data.get(&VALVE_USE.into()) else {
        return None;
    };
    let int = |k: &str| match u.get(&k.into()) {
        Some(Tag::Int(v)) => Some(*v),
        _ => None,
    };
    let start = match u.get(&"Start".into()) {
        Some(Tag::Long(v)) => *v,
        _ => return None,
    };
    let back = matches!(u.get(&"Back".into()), Some(Tag::Byte(b)) if *b != 0);
    Some((
        start,
        int("Ticks")?.clamp(0, 255) as u8,
        int("Target")?,
        back,
    ))
}

/// `ValveHandleBlockEntity.showValue`: the box returns exactly when `inUse` ends.
pub(crate) fn value_active(data: &Compound) -> bool {
    use_of(data).is_none_or(|(start, ticks, ..)| NOW.get() >= start + i64::from(ticks))
}

/// A crank's handle on a client: its angle, the speed it chases at, and the ticks left of its
/// use as this client knows it (`inUse`: 10 from its own player's click, else while it generates).
#[derive(Clone, Copy, Default)]
struct Crank {
    angle: f32,
    chase: f32,
    moving: bool,
    in_use: u8,
}

/// This client's player turned the crank at `pos` (`HandCrankBlockEntity.turn`: `inUse = 10`).
pub(crate) fn cranked(pos: [i32; 3]) {
    CRANKS.with_borrow_mut(|c| c.entry(pos).or_default().in_use = 10);
}

thread_local! {
    static CRANKS: RefCell<HashMap<[i32; 3], Crank>> = RefCell::new(HashMap::new());
    /// The client level's game time at its last tick.
    static NOW: Cell<i64> = const { Cell::new(0) };
}

pub(super) fn register(r: &mut dyn Registrar) {
    for &block in Block::ALL.iter() {
        if block == Block::HandCrank || dye::is_valve_handle(block) {
            r.block_part(states::key(block), Supported);
        }
    }
    let Some(kind) = kinetics::kind() else {
        return;
    };
    if let Err(e) = r.block_entity_part(&kind, Handles) {
        crate::create_log!("[create] the handles' block entity part was refused: {e:?}");
    }
}

pub(super) fn client_tick(host: &dyn ClientHost) {
    if let Some(t) = host.level(host.dimension()).and_then(|l| l.game_time(host)) {
        NOW.set(t);
    }
}

pub(super) fn client_reset() {
    CRANKS.with_borrow_mut(HashMap::clear);
}

/// Whether part `part` of `block` is a handle this module draws.
pub(super) fn draws_itself(block: Block, part: &Draw) -> bool {
    match block {
        Block::HandCrank => matches!(part.model, Model::CrankHandle(_)),
        b if dye::is_valve_handle(b) => part.model == Model::Own,
        _ => false,
    }
}

/// Whether the handle of `block` at `pos` moves this frame: a crank's while it turns or slows, a
/// valve handle's while its use lasts.
pub(super) fn animated(block: Block, pos: [i32; 3], entity: &ModBlockEntity) -> bool {
    match block {
        Block::HandCrank => {
            let turning = speeds_of(&entity.nbt)[0] != 0.0;
            turning || CRANKS.with_borrow(|c| c.get(&pos).is_some_and(|c| c.moving))
        }
        b if dye::is_valve_handle(b) => use_of(&entity.nbt)
            .is_some_and(|(start, ticks, ..)| NOW.get() < start + i64::from(ticks) + SETTLE),
        _ => false,
    }
}

thread_local! {
    /// Cranks and valve handles that may have lost what holds them, by level.
    static SUPPORT_CHECKS: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
}

/// A neighbour of the Create block `state` at `pos` changed: a crank or valve handle looks at
/// what it hangs on in the level tick (`HandCrankBlock.neighborChanged`).
pub(super) fn neighbor_changed(dim: usize, pos: [i32; 3], state: u32) {
    let handle = with_world(|w| {
        w.table
            .get(state)
            .is_some_and(|(b, _)| b == Block::HandCrank || dye::is_valve_handle(b))
    });
    if handle == Some(true) {
        SUPPORT_CHECKS.with_borrow_mut(|c| c.push((dim, pos)));
    }
}

/// `HandCrankBlock.neighborChanged`: a crank or valve handle whose block behind it has no
/// collision shape left (`canSurvive`) breaks with its drops.
pub(super) fn level_tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let due: Vec<[i32; 3]> = SUPPORT_CHECKS.with_borrow_mut(|c| {
        if c.is_empty() {
            return Vec::new();
        }
        let (now, later): (Vec<_>, Vec<_>) = c.drain(..).partition(|(d, _)| *d == dim);
        *c = later;
        now.into_iter().map(|(_, p)| p).collect()
    });
    for pos in due {
        let facing = with_world(|w| {
            let (b, i) = w.table.get(lv.block(pos))?;
            (b == Block::HandCrank || dye::is_valve_handle(b))
                .then_some(i.facing)
                .flatten()
        })
        .flatten();
        let Some(facing) = facing else {
            continue;
        };
        let o = facing.opposite().offset();
        let behind = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]].map(f64::from);
        // The block behind's collision shape: any box within it.
        const IN: f64 = 1.0 / 64.0;
        let aabb = [
            behind[0] + IN,
            behind[1] + IN,
            behind[2] + IN,
            behind[0] + 1.0 - IN,
            behind[1] + 1.0 - IN,
            behind[2] + 1.0 - IN,
        ];
        let empty = lv
            .level
            .collision_boxes(&*lv.host, aabb)
            .is_some_and(|b| b.is_empty());
        if empty {
            lv.destroy_block(pos, true);
        }
    }
}

/// `HandCrankBlock.canSurvive` as one is placed: not against air or a fluid. (A level read here
/// has no collision shapes; the neighbour check above has.)
struct Supported;

#[mod_api::horizon_part]
impl BlockPart for Supported {
    fn can_survive(&self, level: &dyn LevelHost, event: &mut CanSurviveEvent) -> Option<bool> {
        let id = states::id(event.state)?;
        let facing = with_world(|w| w.table.get(id).and_then(|(_, i)| i.facing)).flatten()?;
        let o = facing.opposite().offset();
        let p = [event.pos.x + o[0], event.pos.y + o[1], event.pos.z + o[2]];
        let behind = level.block_state(p)?;
        let name = level.state_block(behind)?;
        let holds = !matches!(
            name,
            "minecraft:air"
                | "minecraft:cave_air"
                | "minecraft:void_air"
                | "minecraft:water"
                | "minecraft:lava"
                | "minecraft:bubble_column"
        );
        (!holds).then_some(false)
    }
}

/// Draws the handles; ticks the cranks' on the client.
struct Handles;

#[mod_api::horizon_part]
impl BlockPart for Handles {}

#[mod_api::horizon_part]
impl BlockEntityPart for Handles {
    fn block_entity_client_ticks(
        &self,
        _host: &dyn ClientHost,
        event: &mut BlockEntityClientTicksEvent<'_>,
    ) {
        if event.block == "create:hand_crank" {
            event.ticks = true;
        }
    }

    /// `HandCrankBlockEntity.tick`: the handle's speed moves a quarter of the way to the
    /// crank's (`convertToAngular(getSpeed())`), and its angle by that speed.
    fn block_entity_client_tick(
        &self,
        host: &mut dyn ClientHost,
        event: &mut BlockEntityClientTickEvent<'_>,
    ) {
        if event.state.block() != states::key(Block::HandCrank) {
            return;
        }
        let target = speeds_of(&event.entity.nbt)[0] * 0.3;
        let generating = super::generated_of(&event.entity.nbt).is_some_and(|g| g != 0.0);
        let in_use = CRANKS.with_borrow_mut(|c| {
            let c = c.entry(event.pos).or_default();
            // Another player's crank: in use while it generates, at half its time.
            if c.in_use == 0 && generating {
                c.in_use = 5;
            }
            let in_use = c.in_use;
            c.in_use = c.in_use.saturating_sub(1);
            in_use
        });
        // `tickAudio`: every ten ticks while in use, the cranking sound (wood placed and a
        // wooden button let go), louder and lower the newer the turn.
        if in_use > 0 && NOW.get().rem_euclid(10) == 0 {
            let center = event.pos.map(|v| f64::from(v) + 0.5);
            let volume = f32::from(in_use) / 2.5;
            let pitch = 0.65 + f32::from(10 - in_use.min(10)) / 10.0;
            for (sound, v, p) in [
                ("minecraft:block.wood.place", 0.075, 0.5),
                ("minecraft:block.wooden_button.click_off", 0.025, 0.5),
            ] {
                let play = SoundPlay::at(sound, SoundSource::Blocks, center)
                    .with_volume(v * volume)
                    .with_pitch(p * pitch);
                let _ = host.play_local_sound(&play, 0);
            }
        }
        CRANKS.with_borrow_mut(|c| {
            let c = c.entry(event.pos).or_default();
            c.chase += (target - c.chase) / 4.0;
            c.angle = (c.angle + c.chase).rem_euclid(360.0);
            // Create chases for ever; a handle this slow no longer shows moving.
            c.moving = target != 0.0 || c.chase.abs() > 0.005;
            if !c.moving {
                c.chase = 0.0;
            }
        });
    }

    fn on_chunk_unloaded(&self, host: HostSide, event: &mut OnChunkUnloadedEvent<'_>) {
        if let HostSide::Client(_) = host {
            CRANKS.with_borrow_mut(|c| {
                c.remove(&event.pos);
            });
        }
    }

    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some(id) = states::id(event.state) else {
            return;
        };
        let Some((block, info)) =
            super::super::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
        else {
            return;
        };
        let Some(facing) = info.facing else {
            return;
        };
        if block == Block::HandCrank {
            draw_crank(event, facing);
        } else if dye::is_valve_handle(block) {
            let text = host.block_state_string(event.state).unwrap_or_default();
            let (_, own) = states::split(&text);
            let speeds = speeds_of(&event.entity.nbt);
            let Some(d) = create_engine::client::draws(block, &info, event.pos, speeds)
                .into_iter()
                .find(|d| d.model == Model::Own)
            else {
                return;
            };
            super::super::interaction::value_text::draw(host, event.pos, event.draw);
            draw_valve(event, block, &own, facing, &d);
        }
    }
}

/// `HandCrankVisual.rotateCrank`: the handle turned about its facing's positive axis.
fn draw_crank(event: &mut BlockEntitySubmitEvent<'_>, facing: Dir) {
    let Some(part) = states::partial("part_hand_crank_handle") else {
        return;
    };
    let target = speeds_of(&event.entity.nbt)[0];
    let c = CRANKS.with_borrow_mut(|c| {
        *c.entry(event.pos).or_insert_with(|| Crank {
            moving: target != 0.0,
            ..Crank::default()
        })
    });
    let angle = if c.moving {
        c.angle + event.draw.partial() * c.chase
    } else {
        c.angle
    };
    let draw = &mut *event.draw;
    draw.push();
    draw.rotate_about(axis_vector(facing.axis()), angle, [0.5; 3]);
    // `rotateTo(0, 0, -1 -> facing)`: the model's north (its grip's end) points the crank's
    // way, which is the partial's `facing` variant.
    draw.block(part, &[("facing", kinetics::dir_name(facing))]);
    draw.pop();
    if !c.moving && target == 0.0 {
        draw.keep();
    }
}

/// `ValveHandleVisual.rotateCrank`: the handle turning through its use, standing at its target
/// after, or turning with the network something else drives.
fn draw_valve(
    event: &mut BlockEntitySubmitEvent<'_>,
    block: Block,
    own: &[(&str, &str)],
    facing: Dir,
    d: &Draw,
) {
    let model = states::key(block);
    let positive = Dir::from_axis(facing.axis(), true) == facing;
    let now = event.draw.game_time();
    let used = use_of(&event.entity.nbt);
    // Its value box's number is drawn while it is looked at.
    let keep = !super::super::interaction::value_text::animated(event.pos);
    let draw = &mut *event.draw;
    match used {
        Some((start, ticks, target, back)) if now < start + i64::from(ticks) + SETTLE => {
            let elapsed = (now - start) as f32 + draw.partial();
            let angle = valve::drawn_angle(target, ticks, elapsed, back, positive);
            draw.push();
            draw.rotate_about(axis_vector(d.axis), angle, [0.5; 3]);
            draw.block(model, own);
            draw.pop();
        }
        _ if d.rpm != 0.0 => {
            draw.spin(axis_vector(d.axis), [0.5; 3], d.rpm * 0.3, d.phase);
            draw.block(model, own);
            draw.still();
            if keep {
                draw.keep();
            }
        }
        _ => {
            let angle = used.map_or(0.0, |(_, ticks, target, back)| {
                valve::drawn_angle(target, ticks, f32::from(ticks), back, positive)
            });
            draw.push();
            draw.rotate_about(axis_vector(d.axis), angle, [0.5; 3]);
            draw.block(model, own);
            draw.pop();
            if keep {
                draw.keep();
            }
        }
    }
}
