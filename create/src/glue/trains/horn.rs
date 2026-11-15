//! The train's horn (`TrainHUD.tick`, `HonkPacket`, `CarriageSounds`): a driver holding the
//! sprint key honks through the first steam whistle the train carries (`determineHonk`), told to
//! the server every 5 ticks while held and once when let go; the server gives the driver
//! `TRAIN_WHISTLE`, makes the `resonate_15` game event and tells every client, which plays the
//! manual whistle (`whistle_train_manual`, or `_low` for a large whistle) at the train's nearest
//! carriage at the whistle's pitch, fading in and out, with its end sound as it stops.
//!
//! The sound stays where it started for as long as it plays: the API has no way to move a
//! playing sound (`API_GAPS.md`, `SoundInstance.getX/Y/Z` read each tick).

use super::super::net;
use super::super::server::ServerLevel;
use super::super::world::{level_index, with_world};
use mod_api::{ClientHost, PlayerRef, ServerHost, SoundPlay, SoundRef, SoundSource};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// `HonkPacket.Serverbound` (train, honk) and `HonkPacket.Clientbound` (train, honk, whether the
/// whistle is large, its pitch).
pub const CHANNEL: &str = "create:train_honk";

/// A train's honk on a client (`Train.honkTicks`, `lowHonk`, `honkPitch`) and its sound.
struct Honk {
    ticks: i32,
    low: bool,
    pitch: i32,
    sound: Option<SoundRef>,
}

thread_local! {
    /// `TrainHUD.honkPacketCooldown` and `usedToHonk`.
    static COOLDOWN: Cell<i32> = const { Cell::new(5) };
    static USED: Cell<bool> = const { Cell::new(false) };
    static HONKS: RefCell<HashMap<u32, Honk>> = RefCell::new(HashMap::new());
}

/// The client left its world.
pub fn reset() {
    HONKS.with_borrow_mut(HashMap::clear);
    COOLDOWN.set(5);
    USED.set(false);
}

/// `TrainHUD.tick`'s horn, while the player drives train `train`: the sprint key held asks for a
/// honk every 5 ticks when the train has a whistle; let go, it says so once.
pub fn driving_tick(host: &mut dyn ClientHost, train: u32) {
    let pressed = host
        .mapping("key.sprint")
        .ok()
        .flatten()
        .is_some_and(|m| m.down);
    if pressed {
        let left = COOLDOWN.get();
        COOLDOWN.set(left - 1);
        if left <= 0 && super::client::has_whistle(&*host, train) {
            send(host, train, true);
            COOLDOWN.set(5);
            USED.set(true);
        }
    }
    if !pressed && USED.get() {
        send(host, train, false);
        COOLDOWN.set(0);
        USED.set(false);
    }
}

fn send(host: &mut dyn ClientHost, train: u32, honk: bool) {
    let mut data = train.to_le_bytes().to_vec();
    data.push(u8::from(honk));
    net::send_to_server(host, CHANNEL, &data);
}

/// `HonkPacket.Serverbound.handle`: `TRAIN_WHISTLE` for the player, every client told, and the
/// game event at the player (from the train's first carriage in Create; the player stands for
/// it here).
pub fn from_client(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let [a, b, c, d, _honk] = data[..] else {
        return;
    };
    let train = u32::from_le_bytes([a, b, c, d]);
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return;
    };
    let Some(dim) = level_index(&dimension) else {
        return;
    };
    let whistle = with_world(|w| {
        let t = w.dims[dim].railway.trains.iter().find(|t| t.id == train)?;
        Some(t.cars.clone())
    })
    .flatten();
    let Some(cars) = whistle else {
        return;
    };
    crate::glue::advancements::award(host, player, "train_whistle");
    let (low, pitch) = first_whistle(&*host, &cars).unwrap_or((false, 0));
    let mut out = data[..5].to_vec();
    out.push(u8::from(low));
    out.push(pitch as i8 as u8);
    let players = host.players().server_players();
    net::send_to_players(host, &players, CHANNEL, &out);
    if let Some(lv) = ServerLevel::new(host, &dimension)
        && let Some(at) = player.entity().position(&*lv.host)
    {
        let context = mod_api::GameEventContext {
            source_entity: Some(player.entity()),
            affected_state: None,
        };
        lv.level
            .game_event(&mut *lv.host, "minecraft:resonate_15", at, context);
    }
}

/// `ArrivalSoundQueue.getFirstWhistle`: the first steam whistle the carriages carry, whether it
/// is large, and its honk pitch (12 up for a small one, less its own `Pitch`, which its
/// extensions above it give).
fn first_whistle(host: &dyn mod_api::Host, cars: &[Vec<([i32; 3], u32)>]) -> Option<(bool, i32)> {
    for blocks in cars {
        let mut sorted: Vec<&([i32; 3], u32)> = blocks.iter().collect();
        sorted.sort_by_key(|(o, _)| *o);
        let parts = |o: [i32; 3]| {
            let (_, s) = blocks.iter().find(|(p, _)| *p == o)?;
            super::state_parts(host, super::car_state(*s)?)
        };
        for (o, s) in sorted {
            let Some((name, props)) =
                super::car_state(*s).and_then(|st| super::state_parts(host, st))
            else {
                continue;
            };
            if name != "create:steam_whistle" {
                continue;
            }
            let size = props
                .iter()
                .find(|(k, _)| k == "size")
                .map(|(_, v)| v.as_str());
            let pitch = create_engine::whistle::pitch(|h| {
                let (n, p) = parts([o[0], o[1] + h, o[2]])?;
                if n != "create:steam_whistle_extension" {
                    return None;
                }
                let shape = p.iter().find(|(k, _)| k == "shape")?.1.clone();
                create_engine::whistle::Shape::read(&shape)
            });
            let small = size == Some("small");
            return Some((size == Some("large"), if small { 12 } else { 0 } - pitch));
        }
    }
    None
}

/// `HonkPacket.Clientbound.handle`: a honk starts a train's horn (20 ticks, or 13 while it
/// sounds); a release lets it end (6 ticks, or at once when nearly done).
pub fn from_server(data: &[u8]) {
    let [a, b, c, d, honk, low, pitch] = data[..] else {
        return;
    };
    let train = u32::from_le_bytes([a, b, c, d]);
    HONKS.with_borrow_mut(|h| {
        let e = h.entry(train).or_insert(Honk {
            ticks: 0,
            low: false,
            pitch: 0,
            sound: None,
        });
        e.low = low != 0;
        e.pitch = i32::from(pitch as i8);
        e.ticks = if honk != 0 {
            if e.ticks == 0 { 20 } else { 13 }
        } else if e.ticks > 5 {
            6
        } else {
            0
        };
    });
}

/// `CarriageSounds.tick`'s horn on a client: each honking train's continuous whistle at its
/// nearest carriage, its volume `1 - fadeout - fadein`, its end sound at tick 5 (and a quiet one
/// at 19); it stops when the honk ends.
pub fn client_tick(host: &mut dyn ClientHost) {
    if HONKS.with_borrow(HashMap::is_empty) {
        return;
    }
    let me = host
        .local_player()
        .and_then(|p| p.entity().position(&*host))
        .unwrap_or_default();
    let mut honks = HONKS.take();
    honks.retain(|&train, h| {
        if h.ticks == 0 {
            if let Some(s) = h.sound.take() {
                s.stop(&*host);
            }
            return false;
        }
        h.ticks -= 1;
        let Some(at) = super::client::nearest_point(&*host, train, me) else {
            return true;
        };
        let pitch = 2f32.powf(h.pitch as f32 / 12.0);
        let end = if h.low {
            "create:whistle_train_manual_low_end"
        } else {
            "create:whistle_train_manual_end"
        };
        let play = |id: &str, volume: f32, looping: bool| {
            let mut p = SoundPlay::at(id, SoundSource::Neutral, at)
                .with_volume(volume)
                .with_pitch(pitch);
            p.looping = looping;
            p
        };
        if h.ticks == 5 {
            host.play_local_sound(&play(end, 1.0, false), 0);
        }
        if h.ticks == 19 {
            host.play_local_sound(&play(end, 0.5, false), 0);
        }
        let fadeout = ((3 - h.ticks) as f32 / 3.0).clamp(0.0, 1.0);
        let fadein = ((h.ticks - 17) as f32 / 3.0).clamp(0.0, 1.0);
        let volume = 1.0 - fadeout - fadein;
        match h.sound {
            Some(s) => {
                s.fade(&*host, volume, 1);
            }
            None => {
                let continuous = if h.low {
                    "create:whistle_train_manual_low"
                } else {
                    "create:whistle_train_manual"
                };
                h.sound = host.play_local_sound(&play(continuous, volume.max(0.01), true), 0);
            }
        }
        true
    });
    HONKS.with_borrow_mut(|h| {
        for (k, v) in honks {
            h.insert(k, v);
        }
    });
}
