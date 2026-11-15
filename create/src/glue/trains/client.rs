//! The trains a client knows (`CarriageContraptionEntity` on the client): each carriage's
//! blocks and its pose as the server last sent it, gliding on from there at its velocity and
//! yaw rate, for clicks on carriages and the train HUD's arrow. They are not drawn (see the
//! module's parent for why).

use super::super::states;
use super::railway::decode_state_names;
use create_engine::train_controls::{Frame, Target, ray_trace};
use create_engine::trains::{Motion, Pose};
use mod_api::{BlockState, ClientHost};
use std::cell::RefCell;
use std::collections::HashMap;

/// A train a client knows: each carriage's blocks (as assembled, from its leading bogey, in the
/// server's numbering), the way it faced then, each carriage's last pose and when it came
/// (client seconds).
struct ClientTrain {
    cars: Vec<Vec<([i32; 3], u32)>>,
    heading: [i32; 2],
    poses: Vec<Pose>,
    since: f32,
}

thread_local! {
    static TRAINS: RefCell<HashMap<u32, ClientTrain>> = RefCell::new(HashMap::new());
    /// The server's numbers of the states its trains carry that are not Create's.
    static STATES: RefCell<HashMap<u32, BlockState>> = RefCell::new(HashMap::new());
}

/// The client left its world.
pub fn reset() {
    TRAINS.with_borrow_mut(HashMap::clear);
    STATES.with_borrow_mut(HashMap::clear);
}

/// The client's clock, seconds.
fn now(host: &dyn ClientHost) -> f32 {
    (host.millis() % 1_000_000_000) as f32 / 1000.0
}

/// The state the server numbers `id`.
fn state_of(id: u32) -> Option<BlockState> {
    if id >= states::FIRST {
        return states::state(id);
    }
    STATES.with_borrow(|s| s.get(&id).copied())
}

/// The states the server's next trains name.
pub fn states_from_server(host: &dyn ClientHost, data: &[u8]) {
    for (id, text) in decode_state_names(data) {
        if let Some(state) = host.block_state_id(&text) {
            STATES.with_borrow_mut(|s| s.insert(id, state));
        }
    }
}

/// A train's motion from the server (`create_engine::trains::Motion`), or its id alone: it is
/// gone.
pub fn trains_from_server(host: &dyn ClientHost, d: &[u8]) {
    let m = match Motion::decode(d) {
        Ok(m) => m,
        Err(id) => {
            if let Some(id) = id {
                TRAINS.with_borrow_mut(|t| t.remove(&id));
            }
            return;
        }
    };
    let now = now(host);
    TRAINS.with_borrow_mut(|trains| {
        if let Some(cars) = m.blocks {
            trains.insert(
                m.id,
                ClientTrain {
                    cars,
                    heading: m.heading,
                    poses: Vec::new(),
                    since: now,
                },
            );
        }
        if let Some(t) = trains.get_mut(&m.id) {
            t.poses = m.cars;
            t.since = now;
        }
    });
}

/// Whether this client knows any train (a click has carriages to look at).
pub fn knows_trains() -> bool {
    !TRAINS.with_borrow(HashMap::is_empty)
}

/// A carriage's frame now: its last pose glided on for `dt` seconds.
fn frame(pose: &Pose, heading: [i32; 2], dt: f64) -> Frame {
    Frame {
        pivot: std::array::from_fn(|k| pose.at[k] + f64::from(pose.velocity[k]) * dt),
        yaw: f64::from(pose.yaw) + f64::from(pose.yaw_rate) * dt,
        pitch: f64::from(pose.pitch),
        heading,
    }
}

/// `ContraptionHandlerClient.rightClickingOnContraptionsGetsHandledLocally`'s search: the
/// carriage block of the trains this client knows that the segment from `origin` to `target`
/// hits first (`rayTraceContraption`, each carriage posed where it glides now, each block by
/// its outline), with how far along.
pub fn carriage_hit(
    host: &dyn ClientHost,
    origin: [f64; 3],
    target: [f64; 3],
) -> Option<(Target, f64)> {
    if !knows_trains() {
        return None;
    }
    let now = now(host);
    let shape = |s: u32| -> Vec<[f64; 6]> {
        state_of(s)
            .and_then(|st| host.state_outline(st))
            .unwrap_or_default()
    };
    TRAINS.with_borrow(|trains| {
        let mut best: Option<(Target, f64)> = None;
        for (&train, t) in trains {
            let dt = f64::from(now - t.since);
            for (c, (pose, blocks)) in t.poses.iter().zip(&t.cars).enumerate() {
                let f = frame(pose, t.heading, dt);
                if let Some((local, along)) = ray_trace(&f, blocks, origin, target, shape)
                    && best.is_none_or(|(_, b)| along < b)
                {
                    let car = c as u16;
                    best = Some((Target { train, car, local }, along));
                }
            }
        }
        best
    })
}

/// Whether train `train`, as this client knows it, carries a steam whistle
/// (`Train.determineHonk` finding one).
pub fn has_whistle(host: &dyn ClientHost, train: u32) -> bool {
    let whistle = host.block_key("create:steam_whistle");
    TRAINS.with_borrow(|trains| {
        trains.get(&train).is_some_and(|t| {
            t.cars
                .iter()
                .flatten()
                .any(|(_, s)| state_of(*s).is_some_and(|st| Some(st.block()) == whistle))
        })
    })
}

/// The point of train `train` nearest `to` (`CarriageSounds`' sound location: the nearest of
/// its carriages' pivots, as each glides now).
pub fn nearest_point(host: &dyn ClientHost, train: u32, to: [f64; 3]) -> Option<[f64; 3]> {
    let now = now(host);
    TRAINS.with_borrow(|trains| {
        let t = trains.get(&train)?;
        let dt = f64::from(now - t.since);
        t.poses
            .iter()
            .map(|p| frame(p, t.heading, dt).pivot)
            .min_by(|a, b| {
                let d = |p: &[f64; 3]| (0..3).map(|i| (p[i] - to[i]).powi(2)).sum::<f64>();
                d(a).total_cmp(&d(b))
            })
    })
}

/// For the train HUD's arrow, the carriage `at` points into as this client has it: its
/// entity's `yaw` now (`Carriage.alignEntity`: the leading bogey's way, `atan2(dz, dx) + 180`),
/// the way it faced when assembled, and the block state at the controls.
pub fn carriage_view(host: &dyn ClientHost, at: Target) -> Option<(f32, [i32; 2], BlockState)> {
    let now = now(host);
    TRAINS.with_borrow(|trains| {
        let t = trains.get(&at.train)?;
        let car = usize::from(at.car);
        let pose = t.poses.get(car)?;
        let id = t.cars.get(car)?.iter().find(|(o, _)| *o == at.local)?.1;
        // The pose's yaw turns counterclockwise from the heading; `angle` is the same measure
        // of the heading, and `atan2(dz, dx)` its negative.
        let heading = (-t.heading[1] as f32)
            .atan2(t.heading[0] as f32)
            .to_degrees();
        let turned = pose.yaw + pose.yaw_rate * (now - t.since);
        Some((180.0 - (turned + heading), t.heading, state_of(id)?))
    })
}
