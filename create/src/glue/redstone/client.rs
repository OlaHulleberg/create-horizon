//! What clients draw of the redstone blocks, as Create's renderers do: a redstone link's two
//! frequency items (`LinkRenderer`), an analog lever's handle (`AnalogLeverRenderer`), and a
//! cuckoo clock's hands, doors and the pig or creeper coming out (`CuckooClockRenderer`), with
//! the clock's sounds and the surprise's smoke. The server sends what changes on the legacy
//! channels; each block entity draws from what its client was told, every frame.

use super::pose;
use super::ticks::CUCKOO_CHANNEL;
use crate::glue::{client_table, kinetics, states};
use create_engine::Block;
use create_engine::cuckoo::{self, Animation};
use create_engine::kinetics::{Axis, Dir};
use create_engine::steam::Face;
use mod_api::{
    BlockEntityDrawContext, BlockEntityPart, BlockEntitySubmitEvent, BlockPart, ClientHost,
    Compound, CubeFace, Identifier, ItemDisplayContext, ItemStack, ParticleKey, ParticleOptions,
    Registrar, SoundPlay, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

static LARGE_SMOKE: OnceLock<ParticleKey> = OnceLock::new();

pub(super) fn register(r: &mut dyn Registrar) {
    let _ = LARGE_SMOKE.set(r.particle_key("minecraft:large_smoke"));
    let Some(kind) = kinetics::kind() else {
        return;
    };
    if let Err(e) = r.block_entity_part(&kind, RedstoneEntity) {
        crate::create_log!("[create] the redstone block entity's part was refused: {e:?}");
    }
}

/// What a client was told and keeps for drawing.
#[derive(Default)]
struct Client {
    /// Each link's facing and frequency items.
    links: HashMap<[i32; 3], (Dir, create_engine::link::Key)>,
    /// Each lever's state, face and facing.
    levers: HashMap<[i32; 3], (u8, Face, Dir)>,
    /// The animations playing: the clock, what, and its tick.
    cuckoos: Vec<([i32; 3], Animation, u32)>,
    /// Each clock's hands' angles as last shown (they stay while it stops).
    hands: HashMap<[i32; 3], (f32, f32)>,
    /// The smoke's scatter.
    rng: Option<create_engine::processing::Rng>,
}

thread_local! {
    static CLIENT: RefCell<Client> = RefCell::new(Client::default());
}

pub(super) fn reset() {
    CLIENT.with_borrow_mut(|c| *c = Client::default());
}

/// The redstone channels, on the client.
pub(super) fn server_message(_host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel == create_engine::link::CHANNEL {
        CLIENT.with_borrow_mut(|c| {
            for (pos, facing, key) in create_engine::link::decode(data) {
                if key.0.is_empty() && key.1.is_empty() {
                    c.links.remove(&pos);
                } else {
                    c.links.insert(pos, (facing, key));
                }
            }
        });
        return true;
    }
    if channel == create_engine::lever::CHANNEL {
        CLIENT.with_borrow_mut(|c| {
            for (pos, state, face, facing) in create_engine::lever::decode(data) {
                match state {
                    Some(s) => c.levers.insert(pos, (s, face, facing)),
                    None => c.levers.remove(&pos),
                };
            }
        });
        return true;
    }
    if channel == CUCKOO_CHANNEL {
        cuckoo_from_server(data);
        return true;
    }
    false
}

/// A clock's animation started (`read` with `Animation`: from 0).
fn cuckoo_from_server(data: &[u8]) {
    if data.len() != 13 {
        return;
    }
    let i = |k: usize| i32::from_le_bytes([data[k], data[k + 1], data[k + 2], data[k + 3]]);
    let Some(kind) = Animation::from_index(data[12]) else {
        return;
    };
    let pos = [i(0), i(4), i(8)];
    CLIENT.with_borrow_mut(|c| {
        c.cuckoos.retain(|a| a.0 != pos);
        c.cuckoos.push((pos, kind, 0));
    });
}

/// The engine's block and facing at `pos` in the client's level, for Create's blocks.
fn client_block(host: &dyn ClientHost, pos: [i32; 3]) -> Option<(Block, Option<Dir>)> {
    let level = host.local_player()?.level(host)?;
    let id = states::id(level.block_state(host, pos)?)?;
    client_table(host, |t| t.get(id).map(|(b, i)| (b, i.facing))).flatten()
}

/// `tick` on the client while an animation runs: its sounds at the clock (`playLocalSound`) and
/// the surprise's smoke after 30; it ends past 100. Nothing runs without one.
pub(super) fn tick(host: &mut dyn ClientHost) {
    if CLIENT.with_borrow(|c| c.cuckoos.is_empty()) {
        return;
    }
    let mut running = CLIENT.with_borrow_mut(|c| std::mem::take(&mut c.cuckoos));
    running.retain_mut(|(pos, kind, value)| {
        let clock = client_block(&*host, *pos)
            .is_some_and(|(b, _)| matches!(b, Block::CuckooClock | Block::MysteriousCuckooClock));
        if !clock {
            // The clock is gone (the surprise broke it).
            return false;
        }
        let center = pos.map(|v| f64::from(v) + 0.5);
        for (sound, volume, pitch) in cuckoo::sounds(*kind, *value) {
            let play = SoundPlay::at(sound, mod_api::SoundSource::Blocks, center)
                .with_volume(volume)
                .with_pitch(pitch);
            host.play_local_sound(&play, 0);
        }
        if *value > 30
            && *kind == Animation::Surprise
            && let Some(&smoke) = LARGE_SMOKE.get()
        {
            // `ParticleTypes.LARGE_SMOKE` within half a block of the clock's centre
            // (`VecHelper.offsetRandomly`).
            let r = || {
                CLIENT.with_borrow_mut(|c| {
                    let rng = c
                        .rng
                        .get_or_insert_with(|| create_engine::processing::Rng::new(0x5EED));
                    f64::from((rng.next_float() - 0.5) * 2.0 * 0.5)
                })
            };
            let at = [center[0] + r(), center[1] + r(), center[2] + r()];
            host.add_particle(&ParticleOptions::simple(smoke), at, [0.0; 3]);
        }
        *value += 1;
        *value <= 101
    });
    CLIENT.with_borrow_mut(|c| {
        running.append(&mut c.cuckoos);
        c.cuckoos = running;
    });
}

/// The redstone blocks' renderers, on the kinetic block entity type.
struct RedstoneEntity;

#[mod_api::horizon_part]
impl BlockPart for RedstoneEntity {}

#[mod_api::horizon_part]
impl BlockEntityPart for RedstoneEntity {
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some(id) = states::id(event.state) else {
            return;
        };
        let Some((block, facing)) =
            client_table(host, |t| t.get(id).map(|(b, i)| (b, i.facing))).flatten()
        else {
            return;
        };
        let pos = event.pos;
        match block {
            Block::RedstoneLink => draw_link(host, event.draw, pos),
            Block::AnalogLever => draw_lever(event.draw, pos),
            Block::PulseRepeater | Block::PulseExtender | Block::PulseTimer => {
                draw_diode_indicator(event.draw, block, &event.entity.nbt);
            }
            Block::CuckooClock | Block::MysteriousCuckooClock => {
                let Some(facing) = facing else {
                    return;
                };
                let turning = speed_of(&event.entity.nbt) != 0.0;
                draw_clock_hands(host, event.draw, pos, facing, turning);
                let running = CLIENT.with_borrow(|c| {
                    c.cuckoos
                        .iter()
                        .find(|a| a.0 == pos)
                        .map(|a| (a.1, a.2))
                        .filter(|a| a.1 <= 101)
                });
                draw_clock_doors(event.draw, facing, running);
            }
            _ => {}
        }
    }
}

/// The first part's speed the kinetic block entity's data holds (`Speeds`).
fn speed_of(nbt: &mod_api::Compound) -> f32 {
    match nbt.get(&"Speeds".into()) {
        Some(Tag::List(l)) => match l.first() {
            Some(Tag::Float(bits)) => f32::from_bits(*bits),
            _ => 0.0,
        },
        _ => 0.0,
    }
}

/// Draws block `name` (`part_cuckoo_pig`, one of the content's part models) facing up.
fn draw_part(draw: &mut dyn BlockEntityDrawContext, name: &str) {
    if let Some(b) = states::partial(name) {
        draw.block(b, &[("facing", "up")]);
    }
}

/// `LinkRenderer`: each slot's item drawn in it.
fn draw_link(host: &dyn ClientHost, draw: &mut dyn BlockEntityDrawContext, pos: [i32; 3]) {
    let Some((facing, key)) = CLIENT.with_borrow(|c| c.links.get(&pos).cloned()) else {
        return;
    };
    for (i, (first, item)) in [(true, &key.0), (false, &key.1)].into_iter().enumerate() {
        if item.is_empty() {
            continue;
        }
        let Some(item_key) = Identifier::parse(item)
            .ok()
            .and_then(|id| host.item_key(&id))
        else {
            continue;
        };
        // `isGui3d`: an item that places a block is drawn whole.
        let block_item = host.item_block(item_key).is_some();
        let pose = create_engine::link::slot_pose(facing, first, block_item);
        draw.push();
        pose::apply(draw, &pose);
        draw.item(
            &ItemStack::of(item_key),
            ItemDisplayContext::Fixed,
            None,
            i as i32,
        );
        draw.pop();
    }
}

/// `BrassDiodeRenderer`: the indicator (`AllPartialModels.FLEXPEATER_INDICATOR`, the model
/// `diodes/indicator`) over the diode, from dark to bright red as its count nears its delay. The
/// count goes on from what the server last put in the block entity (`ticks::DIODE_SHOWN`).
fn draw_diode_indicator(draw: &mut dyn BlockEntityDrawContext, block: Block, nbt: &Compound) {
    use create_engine::diode::{self, Kind};
    let kind = match block {
        Block::PulseExtender => Kind::Extender,
        Block::PulseTimer => Kind::Timer,
        _ => Kind::Repeater,
    };
    let progress = match nbt.get(&super::ticks::DIODE_SHOWN.into()) {
        Some(Tag::LongArray(v)) if v.len() == 4 => diode::progress_after(
            kind,
            v[0] as i32,
            v[1] as i32,
            v[2] != 0,
            draw.game_time() - v[3],
        ),
        _ => 0.0,
    };
    const SPRITE: &str = "create:block/diodes/pulse_repeater/idle";
    let face = |uv: [f32; 4]| Some(CubeFace::new(SPRITE).with_uv(uv));
    // (removed)
    let lo = 0.0;
    let hi = 0.0;
    draw.cube(
        [lo, 0.0, lo],
        [hi, 0.0, hi],
        [
            None,
            face([0.0, 0.0, 0.0, 0.0]),
            face([0.0, 0.0, 0.0, 0.0]),
            face([0.0, 0.0, 0.0, 0.0]),
            face([0.0, 0.0, 0.0, 0.0]),
            face([0.0, 0.0, 0.0, 0.0]),
        ],
        diode::indicator_color(progress),
    );
}

/// `AnalogLeverRenderer`'s handle, turned by the lever's state.
fn draw_lever(draw: &mut dyn BlockEntityDrawContext, pos: [i32; 3]) {
    let Some((state, face, facing)) = CLIENT.with_borrow(|c| c.levers.get(&pos).copied()) else {
        return;
    };
    draw.push();
    pose::apply(
        draw,
        &create_engine::lever::handle_pose(face, facing, state),
    );
    draw_part(draw, "part_analog_lever_handle");
    draw.pop();
}

/// The unit vector of `axis`.
fn axis_vector(axis: Axis) -> [f32; 3] {
    match axis {
        Axis::X => [1.0, 0.0, 0.0],
        Axis::Y => [0.0, 1.0, 0.0],
        Axis::Z => [0.0, 0.0, 1.0],
    }
}

/// The hours and minutes a turning clock shows in this client's level now (only the
/// overworld's dimension type is `natural`).
fn clock_time(host: &dyn ClientHost) -> Option<(i32, i32)> {
    let level = host.local_player()?.level(host)?;
    let natural = level.dimension().id(host).as_deref() == Some("minecraft:overworld");
    Some(cuckoo::time(level.day_time(host)?, natural))
}

/// `CuckooClockRenderer`'s hands of the clock at `pos` facing `facing`: where the time puts them
/// while it turns (`moveHands`), else where they were.
fn draw_clock_hands(
    host: &dyn ClientHost,
    draw: &mut dyn BlockEntityDrawContext,
    pos: [i32; 3],
    facing: Dir,
    turning: bool,
) {
    let now = if turning { clock_time(host) } else { None };
    let (hour, minute) = CLIENT.with_borrow_mut(|c| {
        let h = c.hands.entry(pos).or_insert((0.0, 0.0));
        if let Some((hours, minutes)) = now {
            *h = cuckoo::hand_angles(hours, minutes);
        }
        *h
    });
    let hp = cuckoo::hand_pose(facing);
    for (name, angle) in [
        ("part_cuckoo_hour_hand", hour),
        ("part_cuckoo_minute_hand", minute),
    ] {
        draw.push();
        draw.translate(hp.offset);
        draw.rotate_about(axis_vector(hp.axis), hp.sign * angle, [0.5; 3]);
        pose::apply(draw, &hp.pose);
        draw_part(draw, name);
        draw.pop();
    }
}

/// `CuckooClockRenderer`'s doors and figure of the clock facing `facing`, at its animation's
/// tick (`running`): the doors at their angle about their hinges, the pig or creeper standing out
/// as far as they are open; without one the doors are shut and no figure.
fn draw_clock_doors(
    draw: &mut dyn BlockEntityDrawContext,
    facing: Dir,
    running: Option<(Animation, u32)>,
) {
    let angle = running.map_or(0.0, |(kind, value)| cuckoo::door_angle(kind, value as f32));
    let mut put = |name: &str, p: cuckoo::YPose| {
        draw.push();
        draw.translate(p.offset);
        draw.rotate_about([0.0, 1.0, 0.0], p.angle, [0.5; 3]);
        pose::apply(draw, &p.pose);
        draw_part(draw, name);
        draw.pop();
    };
    put(
        "part_cuckoo_left_door",
        cuckoo::door_pose(facing, true, angle),
    );
    put(
        "part_cuckoo_right_door",
        cuckoo::door_pose(facing, false, angle),
    );
    if let Some((kind, _)) = running {
        let name = if kind == Animation::Pig {
            "part_cuckoo_pig"
        } else {
            "part_cuckoo_creeper"
        };
        put(
            name,
            cuckoo::figure_pose(facing, cuckoo::figure_offset(angle)),
        );
    }
}
