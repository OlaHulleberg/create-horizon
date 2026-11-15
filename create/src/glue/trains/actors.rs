//! The actors trains carry (`CarriageContraptionEntity.tickActors`): drills, saws, harvesters,
//! ploughs, rollers, deployers and contacts on a carriage work where the carriage is each tick
//! (`contraptions::actors::tick_posed_actors`, posed by the carriage's frame as its blocks are
//! drawn), while the chunk under it is loaded (Create's carriage entity is there only then);
//! what they collect goes into the carriage's storage. A carriage an actor stalls holds its
//! train (`Carriage.stalled`, `Train.speedBeforeStall`). Each carriage's actors are made again
//! from its blocks when the world loads, as a carriage entity's contraption is.

use super::super::server::ServerLevel;
use super::super::world::with_world;
use crate::glue::contraptions::actors;
use create_engine::actors::Carried;
use create_engine::train_controls::Frame;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Each carriage's actors (`MovementContext`s), by level, train and carriage.
    static CARRIED: RefCell<HashMap<(usize, u32, usize), Carried>> = RefCell::new(HashMap::new());
}

/// The world closed.
pub fn forget() {
    CARRIED.with_borrow_mut(HashMap::clear);
}

/// After the trains moved: each loaded carriage's actors tick where it is now; a train whose
/// carriage an actor stalls stands still on its next tick.
pub fn tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let poses: Vec<(u32, usize, Frame)> = with_world(|w| {
        let r = &w.dims[dim].railway;
        let g = &r.net.graph;
        let mut out = Vec::new();
        for t in r.trains.iter().filter(|t| t.train.bound) {
            for c in 0..t.train.carriages.len().min(t.cars.len()) {
                let (pivot, front) = t.train.carriage_pose(g, c);
                out.push((t.id, c, Frame::new(pivot, front, t.heading)));
            }
        }
        out
    })
    .unwrap_or_default();
    CARRIED.with_borrow_mut(|m| {
        m.retain(|k, _| k.0 != dim || poses.iter().any(|(t, c, _)| (*t, *c) == (k.1, k.2)));
    });
    if poses.is_empty() {
        return;
    }
    let mut stalled: HashMap<u32, bool> = HashMap::new();
    for (train, car, frame) in poses {
        let key = (dim, train, car);
        let mut carried = match CARRIED.with_borrow_mut(|m| m.remove(&key)) {
            Some(c) => c,
            None => {
                let blocks = with_world(|w| {
                    let t = w.dims[dim].railway.trains.iter().find(|t| t.id == train)?;
                    t.cars.get(car).cloned()
                })
                .flatten()
                .unwrap_or_default();
                actors::bearing_actors(&blocks)
            }
        };
        if carried.actors.is_empty() {
            CARRIED.with_borrow_mut(|m| m.insert(key, carried));
            continue;
        }
        let under = frame.pivot.map(|v| v.floor() as i32);
        if !lv.is_loaded(under) {
            CARRIED.with_borrow_mut(|m| m.insert(key, carried));
            continue;
        }
        let (at, turns) = frame.actor_pose();
        let storage = super::cargo::key(train, car);
        let s = actors::on_vehicle(&[], || {
            actors::tick_posed_actors(lv, storage, &mut carried, at, &turns)
        });
        carried.stalled = s;
        *stalled.entry(train).or_default() |= s;
        CARRIED.with_borrow_mut(|m| m.insert(key, carried));
    }
    with_world(|w| {
        for t in &mut w.dims[dim].railway.trains {
            if let Some(&s) = stalled.get(&t.id) {
                t.train.stalled = s;
            }
        }
    });
}

/// Train `train` comes apart: its actors stop (`Contraption.stop`: their cracks go, deployers
/// let go of what they broke).
pub fn disassembled(lv: &mut ServerLevel<'_>, train: u32) {
    let dim = lv.dim;
    let list: Vec<((usize, u32, usize), Carried)> = CARRIED.with_borrow_mut(|m| {
        let keys: Vec<_> = m
            .keys()
            .filter(|k| k.0 == dim && k.1 == train)
            .copied()
            .collect();
        keys.into_iter()
            .filter_map(|k| m.remove(&k).map(|c| (k, c)))
            .collect()
    });
    for ((_, t, c), mut carried) in list {
        actors::cancel_actors(lv, super::cargo::key(t, c), &mut carried);
    }
}
