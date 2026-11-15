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

/// A carriage's pivot last tick and the entities its blocks met then.
type Touching = ([f64; 3], Vec<u32>);

thread_local! {
    /// Each carriage's actors (`MovementContext`s), by level, train and carriage.
    static CARRIED: RefCell<HashMap<(usize, u32, usize), Carried>> = RefCell::new(HashMap::new());
    /// Each carriage's pivot last tick and the entities touching it then
    /// (`collidingEntities`), for its motion and its run-over damage.
    static LAST: RefCell<HashMap<(usize, u32, usize), Touching>> = RefCell::new(HashMap::new());
}

/// The world closed.
pub fn forget() {
    CARRIED.with_borrow_mut(HashMap::clear);
    LAST.with_borrow_mut(HashMap::clear);
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
    LAST.with_borrow_mut(|m| {
        m.retain(|k, _| k.0 != dim || poses.iter().any(|(t, c, _)| (*t, *c) == (k.1, k.2)));
    });
    for (train, car, frame) in &poses {
        run_over(lv, *train, *car, frame);
    }
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

/// `ContraptionCollider.handleDamageFromTrain` for carriage `car` of `train`, posed `frame`:
/// an entity on the ground the carriage's blocks newly meet while it moves faster than 0.35
/// blocks a tick (and faster than the entity, by as much) is hurt by 16 times that difference
/// (twice for a monster; not a creative or spectating player), with a critical hit's sound, and
/// thrown along the carriage's way and up; one it kills gives the driver `TRAIN_ROADKILL`.
/// Create's run-over damage type is stood in for by `generic` (`API_GAPS.md`). The entity is
/// not stopped by the carriage: that is the moving-collider gap (`ContraptionCollider`).
fn run_over(lv: &mut ServerLevel<'_>, train: u32, car: usize, frame: &Frame) {
    let dim = lv.dim;
    let key = (dim, train, car);
    let (last, touching) = LAST
        .with_borrow(|m| m.get(&key).cloned())
        .unwrap_or((frame.pivot, Vec::new()));
    let motion: [f64; 3] = std::array::from_fn(|i| frame.pivot[i] - last[i]);
    let speed = (motion[0] * motion[0] + motion[1] * motion[1] + motion[2] * motion[2]).sqrt();
    let mut now_touching = Vec::new();
    if speed > 0.35 {
        let blocks = with_world(|w| {
            let t = w.dims[dim].railway.trains.iter().find(|t| t.id == train)?;
            t.cars.get(car).cloned()
        })
        .flatten()
        .unwrap_or_default();
        let solid: std::collections::HashSet<[i32; 3]> = blocks.iter().map(|(o, _)| *o).collect();
        // The carriage's box in the world, from its blocks' corners.
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        for o in &solid {
            for c in 0..8 {
                let corner = [
                    f64::from(o[0] + (c & 1)),
                    f64::from(o[1] + ((c >> 1) & 1)),
                    f64::from(o[2] + ((c >> 2) & 1)),
                ];
                let w = frame.to_world(corner);
                for i in 0..3 {
                    lo[i] = lo[i].min(w[i]);
                    hi[i] = hi[i].max(w[i]);
                }
            }
        }
        let found = if solid.is_empty() {
            Vec::new()
        } else {
            lv.level.entities_in(&*lv.host, lo, hi).unwrap_or_default()
        };
        let drivers = super::controls::drivers(dim, train);
        for e in found {
            let host: &dyn mod_api::Host = &*lv.host;
            if lv.host.item_entity(e).is_some() || lv.host.living(e).is_none() {
                continue;
            }
            let Some([emin, emax]) = e.bounding_box(host) else {
                continue;
            };
            // Any corner or the middle of its box inside one of the carriage's blocks.
            let meets = (0..9).any(|c| {
                let p = if c == 8 {
                    std::array::from_fn(|i| (emin[i] + emax[i]) / 2.0)
                } else {
                    [
                        if c & 1 == 0 { emin[0] } else { emax[0] },
                        if c & 2 == 0 { emin[1] + 0.05 } else { emax[1] },
                        if c & 4 == 0 { emin[2] } else { emax[2] },
                    ]
                };
                let l = frame.to_local(p);
                solid.contains(&l.map(|v| v.floor() as i32))
            });
            if !meets {
                continue;
            }
            now_touching.push(e.id());
            if touching.contains(&e.id()) || e.on_ground(host) != Some(true) {
                continue;
            }
            let v = e.velocity(host).unwrap_or_default();
            let diff: [f64; 3] = std::array::from_fn(|i| motion[i] - v[i]);
            let mut damage = (diff[0] * diff[0] + diff[1] * diff[1] + diff[2] * diff[2]).sqrt();
            if damage <= 0.35 {
                continue;
            }
            let player = e
                .kind(host)
                .is_some_and(|k| k.id(host) == "minecraft:player");
            if player
                && let Some(p) = e.uuid(&mut *lv.host).and_then(|u| lv.host.player(u))
                && (crate::glue::items::creative(&*lv.host, p)
                    || e.spectator(&*lv.host) == Some(true))
            {
                continue;
            }
            if e.classification(&*lv.host) == Some(mod_api::MobCategory::Monster) {
                damage *= 2.0;
            }
            let amount = (damage * 16.0).floor() as f32;
            let _ = e.hurt(&mut *lv.host, "minecraft:generic", amount, None, None);
            let at = e.position(&*lv.host).unwrap_or(frame.pivot);
            let _ = lv.level.play_sound(
                &mut *lv.host,
                "minecraft:entity.player.attack.crit",
                at,
                mod_api::SoundSource::Neutral,
                1.0,
                0.75,
            );
            let h = (motion[0] * motion[0] + motion[2] * motion[2])
                .sqrt()
                .max(1.0e-9);
            let push = [
                motion[0] / h * damage * 4.0,
                0.25 * damage * 4.0,
                motion[2] / h * damage * 4.0,
            ];
            e.push(&mut *lv.host, push);
            let dead = lv
                .host
                .living(e)
                .is_some_and(|l| l.alive(&*lv.host) != Some(true));
            if dead {
                for d in &drivers {
                    if let Some(p) = lv.host.player(*d) {
                        crate::glue::advancements::award(&mut *lv.host, p, "train_roadkill");
                    }
                }
            }
        }
    }
    LAST.with_borrow_mut(|m| m.insert(key, (frame.pivot, now_touching)));
}
