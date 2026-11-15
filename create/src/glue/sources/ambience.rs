//! Turning machinery's ambient sound (`SoundScapes`, `KineticBlockEntity.tickAudio`): every
//! turning kinetic block within 16 blocks of the camera hums (`AmbienceGroup.KINETIC`, the
//! minecart's inside sound; not plain shafts, cogs, encased cogs, gearboxes and gantry shafts,
//! `isNoisy`), and cogs, encased cogs, gearboxes and the machines built on a cog click
//! (`AmbienceGroup.COG`, `create:cogs`). Each group and pitch group is one looping sound at
//! the mean of its blocks, louder the more there are and the nearer they are
//! (`SoundScape.getVolume`).
//!
//! Create's sound plays at its blocks' mean, which it follows every tick (`ContinuousSound.getX`),
//! fading with distance as any sound does. A playing sound cannot be moved through the API, so
//! it plays at the listener at the volume that fade gives it (`Attenuation.LINEAR` over 16
//! blocks), heard from no side (see `API_GAPS.md`).

use super::super::kinetics;
use create_engine::Block;
use mod_api::{
    BlockEntityDataReceivedEvent, BlockEntityPart, BlockPart, ClientHost, HostSide,
    OnChunkUnloadedEvent, Registrar, SoundPlay, SoundRef, SoundSource, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// `SoundScapes.MAX_AMBIENT_SOURCE_DISTANCE`, `UPDATE_INTERVAL`, `SOUND_VOLUME_ARG_MAX`.
const RANGE: i32 = 16;
const UPDATE_INTERVAL: u64 = 5;
const VOLUME_ARG_MAX: f32 = 15.0;
/// `CClient.ambientVolumeCap`'s default.
const VOLUME_CAP: f32 = 0.1;

/// `AmbienceGroup`'s two kinetic groups: their sound and its volume (`SoundScapes.kinetic`,
/// `cogwheel`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Group {
    Kinetic,
    Cog,
}

impl Group {
    fn sound(self) -> (&'static str, f32) {
        match self {
            Group::Kinetic => ("minecraft:entity.minecart.inside", 0.25),
            Group::Cog => ("create:cogs", 1.5),
        }
    }
}

/// `SoundScapes.getGroupFromPitch`.
fn pitch_group(pitch: f32) -> u8 {
    match pitch {
        p if p < 0.70 => 0,
        p if p < 0.90 => 1,
        p if p < 1.10 => 2,
        p if p < 1.30 => 3,
        _ => 4,
    }
}

/// `KineticBlockEntity.isNoisy`: false for `SimpleKineticBlockEntity` (shafts, cogs, encased
/// cogs), gearboxes, gantry shafts and display boards.
fn noisy(block: Block) -> bool {
    !matches!(
        block,
        Block::Shaft
            | Block::Cogwheel
            | Block::LargeCogwheel
            | Block::AndesiteEncasedCogwheel
            | Block::BrassEncasedCogwheel
            | Block::AndesiteEncasedLargeCogwheel
            | Block::BrassEncasedLargeCogwheel
            | Block::Gearbox
            | Block::GantryShaft
            | Block::DisplayBoard
    )
}

/// `ICogWheel.isSmallCog || isLargeCog || instanceof GearboxBlock`: the blocks that are cogs
/// (`ICogWheel`) and the gearbox.
fn clicks(block: Block) -> bool {
    matches!(
        block,
        Block::Cogwheel
            | Block::LargeCogwheel
            | Block::AndesiteEncasedCogwheel
            | Block::BrassEncasedCogwheel
            | Block::AndesiteEncasedLargeCogwheel
            | Block::BrassEncasedLargeCogwheel
            | Block::Gearbox
            | Block::MechanicalPump
            | Block::MechanicalCrafter
            | Block::MechanicalMixer
            | Block::MechanicalArm
            | Block::Millstone
            | Block::DisplayBoard
    )
}

/// The blocks heard in a group this tick, and the pitch of the first.
type Heard = (f32, Vec<[i32; 3]>);

/// One playing `SoundScape`: its sound, the pitch it started with and the volume it was last
/// given.
struct Scape {
    sound: Option<SoundRef>,
    pitch: f32,
    volume: f32,
}

#[derive(Default)]
struct Client {
    /// Each kinetic block's speed (`getSpeed`) as its data last told it, turning ones only.
    speeds: HashMap<[i32; 3], f32>,
    scapes: HashMap<(Group, u8), Scape>,
    ticks: u64,
}

thread_local! {
    static CLIENT: RefCell<Client> = RefCell::new(Client::default());
}

pub(super) fn register(r: &mut dyn Registrar) {
    let Some(kind) = kinetics::kind() else {
        return;
    };
    if let Err(e) = r.block_entity_part(&kind, Listen) {
        crate::create_log!("[create] the ambience's block entity part was refused: {e:?}");
    }
}

pub(super) fn client_reset() {
    CLIENT.with_borrow_mut(|c| *c = Client::default());
}

/// `SoundScape.getVolume`: louder the more blocks (up to the cap) and nearer the camera.
fn volume(count: usize, distance: f64) -> f32 {
    let near = (2.0 - 2.0 * (distance / f64::from(RANGE))) as f32;
    (count as f32 / (VOLUME_ARG_MAX * 10.0)).clamp(0.025, VOLUME_CAP) * near
}

/// `SoundScapes.tick` with the blocks' `tickAudio` of this tick: the groups heard now play,
/// at the mean of their blocks and their volume; the others stop.
pub(super) fn client_tick(host: &mut dyn ClientHost) {
    let Some(camera) = host.camera_entity().and_then(|e| e.position(&*host)) else {
        return;
    };
    let cam_block = camera.map(|v| v.floor() as i32);
    let Some(level) = host.level(host.dimension()) else {
        return;
    };
    let near: Vec<([i32; 3], f32)> = CLIENT.with_borrow(|c| {
        c.speeds
            .iter()
            .filter(|(p, _)| {
                let d = [0, 1, 2].map(|i| i64::from(p[i] - cam_block[i]));
                d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < i64::from(RANGE * RANGE)
            })
            .map(|(p, s)| (*p, *s))
            .collect()
    });
    // (group, pitch group) -> the pitch of the first block heard, and the blocks.
    let mut heard: HashMap<(Group, u8), Heard> = HashMap::new();
    let mut gone = Vec::new();
    for (pos, speed) in near {
        let state = level
            .block_state(&*host, pos)
            .map_or(0, super::super::states::id_or_none);
        let Some(block) =
            super::super::client_table(&*host, |t| t.get(state).map(|(b, _)| b)).flatten()
        else {
            gone.push(pos);
            continue;
        };
        let pitch = (speed.abs() / 256.0 + 0.45).clamp(0.85, 1.0);
        let pg = pitch_group(pitch);
        for (group, on) in [(Group::Kinetic, noisy(block)), (Group::Cog, clicks(block))] {
            if on {
                heard
                    .entry((group, pg))
                    .or_insert_with(|| (pitch, Vec::new()))
                    .1
                    .push(pos);
            }
        }
    }
    CLIENT.with_borrow_mut(|c| {
        for pos in gone {
            c.speeds.remove(&pos);
        }
        c.ticks += 1;
        let update = c.ticks.is_multiple_of(UPDATE_INTERVAL);
        // Groups no block is heard in stop (`SoundScapes.tick`, every 5 ticks).
        if update {
            c.scapes.retain(|key, s| {
                let keep = heard.contains_key(key);
                if !keep && let Some(sound) = s.sound {
                    sound.stop(&*host);
                }
                keep
            });
        }
        for (key, (pitch, blocks)) in heard {
            let n = blocks.len() as f64;
            let mean =
                [0, 1, 2].map(|i| blocks.iter().map(|p| f64::from(p[i]) + 0.5).sum::<f64>() / n);
            let distance = (0..3)
                .map(|i| (camera[i] - mean[i]).powi(2))
                .sum::<f64>()
                .sqrt();
            let (event, relative) = key.0.sound();
            // `ContinuousSound.getVolume`, then the linear fade over 16 blocks the sound engine
            // gives a sound at the mean.
            let fade = (1.0 - distance / f64::from(RANGE)).max(0.0) as f32;
            let vol = volume(blocks.len(), distance) * relative * fade;
            let fresh = !c.scapes.contains_key(&key);
            let scape = c.scapes.entry(key).or_insert(Scape {
                sound: None,
                pitch,
                volume: vol,
            });
            // A new group starts at once; one whose sound ended starts again on an update.
            let stopped = scape.sound.is_none_or(|s| {
                matches!(s.state(&*host), Some(mod_api::SoundState::Stopped) | None)
            });
            if fresh || (stopped && update) {
                if let Some(s) = scape.sound.take() {
                    s.stop(&*host);
                }
                scape.volume = vol;
                let mut play = SoundPlay::ui(event)
                    .with_volume(vol.max(0.0001))
                    .with_pitch(scape.pitch)
                    .looping(0);
                play.source = SoundSource::Ambient;
                scape.sound = host.play_local_sound(&play, 0);
            } else if (scape.volume - vol).abs() > 1e-4
                && let Some(s) = scape.sound
            {
                scape.volume = vol;
                s.fade(&*host, vol.max(0.0001), 1);
            }
        }
    });
}

/// Hears each kinetic block's speed.
struct Listen;

#[mod_api::horizon_part]
impl BlockPart for Listen {}

#[mod_api::horizon_part]
impl BlockEntityPart for Listen {
    fn block_entity_data_received(
        &self,
        host: &dyn ClientHost,
        event: &mut BlockEntityDataReceivedEvent<'_>,
    ) {
        let Some(Tag::List(speeds)) = event.tag.get(&kinetics::SPEEDS.into()) else {
            return;
        };
        let Some(pos) = event.entity.position(host) else {
            return;
        };
        let speed = match speeds.first() {
            Some(Tag::Float(bits)) => f32::from_bits(*bits),
            _ => 0.0,
        };
        CLIENT.with_borrow_mut(|c| {
            if speed == 0.0 {
                c.speeds.remove(&pos);
            } else {
                c.speeds.insert(pos, speed);
            }
        });
    }

    fn on_chunk_unloaded(&self, host: HostSide, event: &mut OnChunkUnloadedEvent<'_>) {
        if let HostSide::Client(_) = host {
            CLIENT.with_borrow_mut(|c| {
                c.speeds.remove(&event.pos);
            });
        }
    }
}
