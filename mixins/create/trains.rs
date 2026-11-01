// Create's trains in the game: stations, tracks, the trains running between them over the track
// graph and their save data, over the engine's `trains` and `network`. A sibling of the `create`
// module (`runtime.rs`).
use super::Game;
use super::create::*;
use super::create_contraptions::*;
use super::create_logistics::*;
use create_engine::kinetics::{Axis, Dir};
use create_engine::world::dimension_index;
use horizon_world::registry::BlockStateId;
use std::cell::RefCell;
use std::collections::HashMap;

pub(super) const TRAIN_CHANNEL: &str = create_engine::trains::CHANNEL;

/// The graph as the world has it now (built again only if track changed).
fn railway_ready(dim: usize) {
    with_world(|w| {
        let d = &mut w.dims[dim];
        d.railway.ensure(&d.track_curves);
    });
}

/// The station's assembly (`StationBlockEntity.assemble` / `disassemble` from its screen):
/// a train standing at it goes back into the world (turned as it stands, to the nearest
/// quarter); else the bogeys on its track (a block above it) and what sticks to them become a
/// train, standing on the track graph.
pub(super) fn use_station(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    use create_engine::schedule::Runtime;
    use create_engine::trains::{Train, TrainOnTrack};
    railway_ready(dim);
    let dimension = game.dim.id().to_owned();
    let stopped = with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let i = r.trains.iter().position(|t| {
            t.train.bound
                && t.train.nav.is_none()
                && t.train.speed == 0.0
                && t.train.at_station == Some(pos)
        })?;
        let t = r.trains.remove(i);
        let (pivot, front) = t.train.pose(&r.net.graph, t.pivot);
        Some((t, pivot, front))
    })
    .flatten();
    if let Some((t, pivot, front)) = stopped {
        let (yaw, _) = create_engine::trains::yaw_pitch(front, t.heading);
        let quarter = ((yaw / 90.0).round() as i32 * 90).rem_euclid(360);
        for (o, s) in &t.blocks {
            // The block's corner from the pivot (the track under the bogey), in half blocks.
            let c = [2 * o[0] - 1, 2 * o[1] + 2, 2 * o[2] - 1];
            let r = create_engine::contraption::rotate(c, quarter, Axis::Y);
            let p: [i32; 3] =
                std::array::from_fn(|k| (pivot[k] + f64::from(r[k]) / 2.0).round() as i32);
            let state = turned_state(game, *s, quarter, Axis::Y);
            game.native_set_block(
                &dimension,
                p,
                BlockStateId(state),
                super::native_hooks::flags::UPDATE_ALL,
            );
        }
        game.native_mark_chunk_unsaved(&dimension, [t.column.0, t.column.1]);
        send_train_clear(game, &dimension, t.id);
        return;
    }
    // The straight track the station looks at, as far as it runs straight.
    let Some((target, shape, tracks)) = with_world(|w| {
        let r = &w.dims[dim].railway;
        let target = create_engine::network::target_track(&r.tracks, pos)?;
        let shape = r.tracks[&target];
        let step = if shape == create_engine::track::TrackShape::Xo {
            [1, 0, 0]
        } else {
            [0, 0, 1]
        };
        let mut line = vec![target];
        for sign in [-1, 1] {
            let mut p = target;
            loop {
                p = [p[0] + sign * step[0], p[1], p[2] + sign * step[2]];
                if r.tracks.get(&p) != Some(&shape) {
                    break;
                }
                line.push(p);
            }
        }
        line.sort_unstable();
        Some((target, shape, line))
    })
    .flatten() else {
        return;
    };
    let _ = target;
    // Bogeys along the track, one above it, lowest first.
    let bogeys: Vec<[i32; 3]> = tracks
        .iter()
        .map(|t| [t[0], t[1] + 1, t[2]])
        .filter(|p| {
            let s = game.world.block(p[0], p[1], p[2]);
            matches!(
                with_world(|w| w.table.get(s.0).map(|(b, _)| b)).flatten(),
                Some(create_engine::Block::SmallBogey | create_engine::Block::LargeBogey)
            )
        })
        .collect();
    let Some(&origin) = bogeys.first() else {
        return;
    };
    // The bogeys and what is stuck to them (slime, honey; glue is not ported), not the track.
    let mut blocks: Vec<([i32; 3], u32)> = Vec::new();
    for b in &bogeys {
        // Down is the track, left out: only what sticks is taken.
        let Ok(found) =
            create_engine::piston::gather(*b, Dir::Down, &tracks, |p| contraption_found(game, p))
        else {
            return;
        };
        for (o, s) in found {
            let l = [
                b[0] + o[0] - origin[0],
                b[1] + o[1] - origin[1],
                b[2] + o[2] - origin[2],
            ];
            if !blocks.iter().any(|(x, _)| *x == l) {
                blocks.push((l, s));
            }
        }
    }
    // Its front is the far end along the track's axis: the bogeys front first.
    let heading = if shape == create_engine::track::TrackShape::Xo {
        [1, 0]
    } else {
        [0, 1]
    };
    let front = [f64::from(heading[0]), 0.0, f64::from(heading[1])];
    let under: Vec<[i32; 3]> = bogeys
        .iter()
        .rev()
        .map(|b| [b[0], b[1] - 1, b[2]])
        .collect();
    let Some(id) = with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let mut train = Train::on_bogeys(&r.net, &under, front)?;
        train.at_station = Some(pos);
        let id = r.next_id();
        let mut t = TrainOnTrack {
            id,
            train,
            blocks: blocks.clone(),
            pivot: under.len() - 1,
            heading,
            schedule: None,
            runtime: Runtime::new(),
            idle: 0,
            column: (origin[0] >> 4, origin[2] >> 4),
            sent_tick: 0,
            sent_edge: usize::MAX,
            fresh: true,
        };
        t.column = t.column_on(&r.net);
        r.trains.push(t);
        Some(id)
    })
    .flatten() else {
        return;
    };
    let air = BlockStateId(0);
    for (l, _) in &blocks {
        let p = [origin[0] + l[0], origin[1] + l[1], origin[2] + l[2]];
        game.native_set_block(&dimension, p, air, super::native_hooks::flags::UPDATE_ALL);
    }
    game.native_mark_chunk_unsaved(&dimension, [origin[0] >> 4, origin[2] >> 4]);
    send_trains(game, dim, &[id], true);
}

/// The train nearest `pos` that stands on the graph, if any.
fn nearest_train(dim: usize, pos: [i32; 3], at_station_first: bool) -> Option<usize> {
    with_world(|w| {
        let r = &w.dims[dim].railway;
        if at_station_first
            && let Some(i) = r
                .trains
                .iter()
                .position(|t| t.train.at_station == Some(pos))
        {
            return Some(i);
        }
        let c = [
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]),
            f64::from(pos[2]) + 0.5,
        ];
        r.trains
            .iter()
            .enumerate()
            .filter(|(_, t)| t.train.bound)
            .map(|(i, t)| {
                let (p, _) = t.train.pose(&r.net.graph, t.pivot);
                let d = (p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2);
                (i, d)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    })
    .flatten()
}

/// `create_debug train <station>`: the nearest train that can reach that station heads there
/// (`findPathTo` over the graph); a test driver standing in for schedules and the screen.
pub(super) fn send_train(game: &mut Game<'_>, dim: usize, station: [i32; 3]) -> String {
    railway_ready(dim);
    let found = with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let s = r.station_index(station)?;
        let c = [
            f64::from(station[0]) + 0.5,
            f64::from(station[1]),
            f64::from(station[2]) + 0.5,
        ];
        let mut order: Vec<(usize, f64)> = r
            .trains
            .iter()
            .enumerate()
            .filter(|(_, t)| t.train.bound)
            .map(|(i, t)| {
                let (p, _) = t.train.pose(&r.net.graph, t.pivot);
                (i, (p[0] - c[0]).powi(2) + (p[2] - c[2]).powi(2))
            })
            .collect();
        order.sort_by(|a, b| a.1.total_cmp(&b.1));
        order.into_iter().find_map(|(i, _)| {
            r.navigate(i, &[s]).then(|| {
                let t = &r.trains[i];
                (t.id, t.train.nav.as_ref().map_or(0.0, |n| n.distance))
            })
        })
    })
    .flatten();
    match found {
        Some((id, distance)) => {
            send_trains(game, dim, &[id], false);
            format!("train {id} heading for the station, {distance:.1} blocks along the track")
        }
        None => "no train can reach that station (no track beside it, or no path)".into(),
    }
}

/// `create_debug schedule <station> <entries>`: the train at that station (else the nearest)
/// runs a cyclic schedule (`Schedule`, `DestinationInstruction`, `ScheduledDelay`): each entry a
/// station name filter (`*` any text), waiting `@seconds` there (5 by default).
pub(super) fn schedule_train(dim: usize, station: [i32; 3], entries: &str) -> String {
    use create_engine::schedule::{Condition, Entry, Runtime, SECONDS, Schedule};
    railway_ready(dim);
    let entries: Vec<Entry> = entries
        .split_whitespace()
        .map(|e| {
            let (name, wait) = match e.rsplit_once('@') {
                Some((n, s)) => (n, s.parse::<i32>().unwrap_or(5)),
                None => (e, 5),
            };
            Entry {
                station: name.to_owned(),
                conditions: vec![vec![Condition::Delay(wait.max(0) * SECONDS)]],
            }
        })
        .collect();
    if entries.is_empty() {
        return "usage: create_debug schedule <x> <y> <z> <station[@seconds]>...".into();
    }
    let Some(i) = nearest_train(dim, station, true) else {
        return "no train".into();
    };
    with_world(|w| {
        let t = &mut w.dims[dim].railway.trains[i];
        let names: Vec<String> = entries.iter().map(|e| e.station.clone()).collect();
        t.schedule = Some(Schedule {
            entries,
            cyclic: true,
        });
        t.runtime = Runtime::new();
        format!("train {} runs {}", t.id, names.join(" → "))
    })
    .unwrap_or_default()
}

/// `create_debug station_name <station> <name>`: names a station (its screen's name box).
pub(super) fn name_station(dim: usize, station: [i32; 3], name: &str) -> String {
    let name = name.trim();
    with_world(|w| {
        let r = &mut w.dims[dim].railway;
        match r.stations.get_mut(&station) {
            Some(n) if !name.is_empty() => {
                *n = name.to_owned();
                w.mark_unsaved(dim, station);
                format!("station named {name}")
            }
            _ => "no station there".into(),
        }
    })
    .unwrap_or_default()
}

/// Sends trains `ids` to the players in this dimension as they are (with their blocks when
/// `full`, for clients that don't know them yet).
pub(super) fn send_trains(game: &mut Game<'_>, dim: usize, ids: &[u32], full: bool) {
    use create_engine::trains::CURVE_UPDATE_TICKS;
    let tick = game.world.game_time.max(0) as u64;
    let messages: Vec<Vec<u8>> = with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let mut out = Vec::new();
        for t in r.trains.iter_mut().filter(|t| ids.contains(&t.id)) {
            if !t.train.bound {
                continue;
            }
            let full = full || t.fresh;
            t.fresh = false;
            t.sent_tick = tick;
            t.sent_edge = t.train.points.first().map_or(0, |p| p.edge);
            out.push(t.motion(&r.net, CURVE_UPDATE_TICKS, full).encode());
        }
        out
    })
    .unwrap_or_default();
    if messages.is_empty() {
        return;
    }
    let dimension = game.dim.id().to_owned();
    let players = players_in(game, &dimension);
    for msg in &messages {
        for &p in &players {
            let _ = game.native_net_send_to_player(p, TRAIN_CHANNEL, msg);
        }
    }
}

pub(super) fn send_train_clear(game: &mut Game<'_>, dimension: &str, id: u32) {
    for p in players_in(game, dimension) {
        let _ = game.native_net_send_to_player(p, TRAIN_CHANNEL, &id.to_le_bytes());
    }
}

thread_local! {
    /// Scratch for a tick's train events, kept to not allocate.
    static EVENTS: RefCell<Vec<create_engine::trains::Event>> = const { RefCell::new(Vec::new()) };
}

/// The trains (`Train.tick`, `Navigation.tick`, `ScheduleRuntime.tick`): nothing without
/// trains; the graph is built again first if track changed. Clients are told of a train when it
/// starts, stops or arrives, every few ticks on a curve, as it passes onto another edge, and every
/// ten ticks while its speed changes; trains found on the graph again (loaded) are sent whole.
pub(super) fn tick_trains(game: &mut Game<'_>, dimension: &str, dim: usize) {
    if with_world(|w| w.dims[dim].railway.trains.is_empty()) != Some(false) {
        return;
    }
    let tick = game.world.game_time.max(0) as u64;
    let mut events = EVENTS.take();
    let (due, fresh) = with_world(|w| {
        let d = &mut w.dims[dim];
        d.railway.ensure(&d.track_curves);
        d.railway.tick(&mut events);
        let r = &d.railway;
        let due: Vec<u32> = events
            .iter()
            .filter(|e| {
                r.trains
                    .iter()
                    .find(|t| t.id == e.id)
                    .is_some_and(|t| t.due(&r.net, tick, e))
            })
            .map(|e| e.id)
            .collect();
        let fresh: Vec<u32> = r
            .trains
            .iter()
            .filter(|t| t.fresh && t.train.bound)
            .map(|t| t.id)
            .collect();
        (due, fresh)
    })
    .unwrap_or_default();
    // Saved with the column it stands in: the one it left and the one it is in save again.
    for e in &events {
        if e.was != e.now || e.run == create_engine::trains::Run::Arrived {
            game.native_mark_chunk_unsaved(dimension, [e.was.0, e.was.1]);
            game.native_mark_chunk_unsaved(dimension, [e.now.0, e.now.1]);
        }
    }
    EVENTS.set(events);
    if !fresh.is_empty() {
        send_trains(game, dim, &fresh, true);
    }
    if !due.is_empty() {
        send_trains(game, dim, &due, false);
    }
}

/// A train a client draws: its blocks (as assembled, from its pivot bogey), the way it faced
/// then, and its part models by pitch (baked when a new pitch is needed).
struct ClientTrain {
    blocks: Vec<([i32; 3], u32)>,
    heading: [i32; 2],
    models: Vec<(i32, u32)>,
}

thread_local! {
    /// Trains a client draws, by id.
    static TRAINS: RefCell<HashMap<u32, ClientTrain>> = RefCell::new(HashMap::new());
}

/// Pitch steps a client bakes a train's model at, degrees.
const PITCH_STEP: f32 = 3.0;

/// A train on the client (`CarriageContraptionEntity`'s motion): one part model, tilted to its
/// pitch in the model, turned to its yaw about its pivot (the track under its pivot bogey) and
/// gliding at its velocity while turning at its yaw rate, on the GPU until the next message.
pub(super) fn trains_from_server(game: &mut Game<'_>, d: &[u8]) {
    use create_engine::trains::Motion;
    let m = match Motion::decode(d) {
        Ok(m) => m,
        Err(id) => {
            if let Some(id) = id
                && let Some(old) = TRAINS.with_borrow_mut(|t| t.remove(&id))
            {
                game.native_part_clear(depot_key([id as i32, 0, 0], 222, 0));
                for (_, model) in old.models {
                    game.native_part_model_free(model);
                }
            }
            return;
        }
    };
    let key = depot_key([m.id as i32, 0, 0], 222, 0);
    if let Some(blocks) = m.blocks {
        let old = TRAINS.with_borrow_mut(|t| {
            t.insert(
                m.id,
                ClientTrain {
                    blocks,
                    heading: m.heading,
                    models: Vec::new(),
                },
            )
        });
        if let Some(old) = old {
            for (_, model) in old.models {
                game.native_part_model_free(model);
            }
        }
    }
    let bucket = (m.pitch / PITCH_STEP).round() as i32;
    let Some(known) = TRAINS.with_borrow(|t| {
        t.get(&m.id).map(|c| {
            (
                c.models.iter().find(|(b, _)| *b == bucket).map(|(_, x)| *x),
                c.heading,
            )
        })
    }) else {
        return;
    };
    let model = match known.0 {
        Some(model) => model,
        None => {
            // The model about its pivot at the centre of its part block: each block's corner
            // half a block back and up one from the track, tilted about the train's side.
            let h = glam::Vec3::new(known.1[0] as f32, 0.0, known.1[1] as f32);
            let side = h.cross(glam::Vec3::Y).normalize_or(glam::Vec3::X);
            let tilt = glam::Mat4::from_translation(glam::Vec3::splat(0.5))
                * glam::Mat4::from_axis_angle(side, (bucket as f32 * PITCH_STEP).to_radians())
                * glam::Mat4::from_translation(glam::Vec3::splat(-0.5));
            let blocks: Vec<(BlockStateId, glam::Mat4)> = TRAINS.with_borrow(|t| {
                t.get(&m.id).map_or_else(Vec::new, |c| {
                    c.blocks
                        .iter()
                        .map(|(o, s)| {
                            let at = glam::Vec3::new(o[0] as f32, o[1] as f32 + 1.5, o[2] as f32);
                            (BlockStateId(*s), tilt * glam::Mat4::from_translation(at))
                        })
                        .collect()
                })
            });
            let Some(model) = game.native_part_model_compound(&blocks) else {
                return;
            };
            TRAINS.with_borrow_mut(|t| {
                if let Some(c) = t.get_mut(&m.id) {
                    c.models.push((bucket, model));
                }
            });
            model
        }
    };
    let block: [i32; 3] = std::array::from_fn(|k| m.pivot[k].floor() as i32);
    let now = game.native_parts_seconds();
    let offset = [
        (m.pivot[0] - f64::from(block[0]) - 0.5) as f32,
        (m.pivot[1] - f64::from(block[1]) - 0.5) as f32,
        (m.pivot[2] - f64::from(block[2]) - 0.5) as f32,
        now,
    ];
    // The yaw turns on from `yaw` now at `yaw_rate`: the part's phase is its angle at time 0.
    let phase = (m.yaw - m.yaw_rate * now).rem_euclid(360.0);
    game.native_part_glide(
        key,
        model,
        block,
        1,
        m.yaw_rate / 6.0,
        phase,
        offset,
        m.velocity,
    );
}

/// Trains as saved (`Train.write`, here with the column their pivot bogey stands in): their
/// points by the graph's nodes (and where they were), motion, destination, schedule, the
/// blocks they carry and the way they faced when assembled.
pub(super) fn trains_to_nbt(
    game: &Game<'_>,
    all: &[create_engine::world::TrainOnTrack],
) -> horizon_world::nbt::Tag {
    use horizon_world::nbt::{Compound, Tag};
    let registry = game.pack.assets.registry;
    let f = |v: f64| (v as f32).to_bits() as i32;
    let pos = |p: Option<[i32; 3]>| {
        let p = p.unwrap_or([0; 3]);
        [p[0], p[1], p[2]]
    };
    Tag::List(
        all.iter()
            .map(|t| {
                let mut c = Compound::new();
                c.insert(
                    "points".into(),
                    Tag::IntArray(
                        t.train
                            .placed
                            .iter()
                            .flat_map(|p| {
                                let mut v = Vec::with_capacity(10);
                                v.extend(p.from);
                                v.extend(p.to);
                                v.push(f(p.position));
                                v.extend(p.at.map(f));
                                v
                            })
                            .collect(),
                    ),
                );
                let nav = t.train.nav.as_ref().map(|n| n.station);
                let mut motion = vec![
                    f(t.train.speed),
                    t.pivot as i32,
                    t.heading[0],
                    t.heading[1],
                    i32::from(t.train.at_station.is_some()),
                ];
                motion.extend(pos(t.train.at_station));
                motion.push(i32::from(nav.is_some()));
                motion.extend(pos(nav));
                motion.push(i32::from(t.train.nav.as_ref().is_some_and(|n| n.behind)));
                c.insert("motion".into(), Tag::IntArray(motion));
                c.insert(
                    "blocks".into(),
                    Tag::IntArray(t.blocks.iter().flat_map(|(o, _)| *o).collect()),
                );
                c.insert(
                    "states".into(),
                    Tag::List(
                        t.blocks
                            .iter()
                            .map(|(_, s)| Tag::String(state_text(registry, *s).as_str().into()))
                            .collect(),
                    ),
                );
                if let Some(s) = &t.schedule {
                    c.insert(
                        "schedule".into(),
                        Tag::List(
                            s.entries
                                .iter()
                                .map(|e| {
                                    let mut entry = Compound::new();
                                    entry.insert(
                                        "station".into(),
                                        Tag::String(e.station.as_str().into()),
                                    );
                                    entry.insert(
                                        "conditions".into(),
                                        Tag::IntArray(conditions_to_ints(&e.conditions)),
                                    );
                                    Tag::Compound(entry)
                                })
                                .collect(),
                        ),
                    );
                    let state = t.runtime.state as i32;
                    c.insert(
                        "runtime".into(),
                        Tag::IntArray(vec![
                            i32::from(s.cyclic),
                            t.runtime.current as i32,
                            state,
                            i32::from(t.runtime.paused),
                        ]),
                    );
                }
                Tag::Compound(c)
            })
            .collect(),
    )
}

/// A schedule entry's condition groups as ints: each group's length, then each condition's kind
/// (0 delay, 1 idle cargo) and ticks.
fn conditions_to_ints(groups: &[Vec<create_engine::schedule::Condition>]) -> Vec<i32> {
    use create_engine::schedule::Condition;
    let mut out = Vec::new();
    for g in groups {
        out.push(g.len() as i32);
        for c in g {
            match c {
                Condition::Delay(t) => out.extend([0, *t]),
                Condition::IdleCargo(t) => out.extend([1, *t]),
            }
        }
    }
    out
}

fn conditions_from_ints(v: &[i32]) -> Vec<Vec<create_engine::schedule::Condition>> {
    use create_engine::schedule::Condition;
    let mut out = Vec::new();
    let mut k = 0;
    while k < v.len() {
        let n = v[k].max(0) as usize;
        k += 1;
        let mut g = Vec::with_capacity(n);
        for _ in 0..n {
            if k + 1 >= v.len() {
                break;
            }
            g.push(if v[k] == 1 {
                Condition::IdleCargo(v[k + 1])
            } else {
                Condition::Delay(v[k + 1])
            });
            k += 2;
        }
        out.push(g);
    }
    out
}

pub(super) fn trains_from_nbt(
    game: &Game<'_>,
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<create_engine::world::TrainOnTrack> {
    use create_engine::schedule::{Entry, Runtime, Schedule, State};
    use create_engine::trains::{Nav, Placed, Train, TrainOnTrack};
    use horizon_world::nbt::Tag;
    let registry = game.pack.assets.registry;
    let Some(Tag::List(list)) = data.and_then(|d| d.get(&"trains".into())) else {
        return Vec::new();
    };
    let f = |v: i32| f64::from(f32::from_bits(v as u32));
    list.iter()
        .filter_map(|c| {
            let Tag::Compound(c) = c else { return None };
            let (
                Some(Tag::IntArray(p)),
                Some(Tag::IntArray(m)),
                Some(Tag::IntArray(o)),
                Some(Tag::List(s)),
            ) = (
                c.get(&"points".into()),
                c.get(&"motion".into()),
                c.get(&"blocks".into()),
                c.get(&"states".into()),
            )
            else {
                return None;
            };
            if m.len() < 14 || p.len() < 20 {
                return None;
            }
            let placed: Vec<Placed> = p
                .chunks_exact(10)
                .map(|v| Placed {
                    from: [v[0], v[1], v[2]],
                    to: [v[3], v[4], v[5]],
                    position: f(v[6]),
                    at: [f(v[7]), f(v[8]), f(v[9])],
                })
                .collect();
            let blocks = o
                .chunks_exact(3)
                .zip(s)
                .filter_map(|(o, s)| {
                    let Tag::String(s) = s else { return None };
                    Some((
                        [o[0], o[1], o[2]],
                        state_of_text(registry, &s.to_string_lossy())?,
                    ))
                })
                .collect();
            let schedule = match c.get(&"schedule".into()) {
                Some(Tag::List(entries)) => Some(Schedule {
                    entries: entries
                        .iter()
                        .filter_map(|e| {
                            let Tag::Compound(e) = e else { return None };
                            let Some(Tag::String(station)) = e.get(&"station".into()) else {
                                return None;
                            };
                            let conditions = match e.get(&"conditions".into()) {
                                Some(Tag::IntArray(v)) => conditions_from_ints(v),
                                _ => Vec::new(),
                            };
                            Some(Entry {
                                station: station.to_string_lossy(),
                                conditions,
                            })
                        })
                        .collect(),
                    cyclic: true,
                }),
                _ => None,
            };
            let mut runtime = Runtime::new();
            let mut schedule = schedule;
            if let (Some(s), Some(Tag::IntArray(r))) = (&mut schedule, c.get(&"runtime".into()))
                && r.len() >= 4
            {
                s.cyclic = r[0] != 0;
                runtime.current = r[1].max(0) as usize;
                // In transit it looks for its way again; waiting, it waits anew.
                runtime.state = match r[2] {
                    2 => State::PostTransit,
                    _ => State::PreTransit,
                };
                runtime.paused = r[3] != 0;
                if runtime.state == State::PostTransit {
                    runtime.progress = s
                        .entries
                        .get(runtime.current)
                        .map_or_else(Vec::new, |e| vec![(0, 0); e.conditions.len()]);
                }
            }
            let at = |k: usize| [m[k], m[k + 1], m[k + 2]];
            // A train in transit without a schedule keeps its destination.
            let nav = (m[9] != 0 && schedule.is_none()).then(|| Nav {
                station: at(10),
                distance: f64::MAX,
                path: Vec::new(),
                behind: m[13] != 0,
                signal: None,
                ticks_waiting: 0,
            });
            Some(TrainOnTrack {
                id: 0,
                train: Train {
                    points: Vec::new(),
                    speed: f(m[0]),
                    nav,
                    at_station: (m[4] != 0).then(|| at(5)),
                    placed,
                    bound: false,
                },
                blocks,
                pivot: m[1].max(0) as usize,
                heading: [m[2], m[3]],
                schedule,
                runtime,
                idle: 0,
                column: (0, 0),
                sent_tick: 0,
                sent_edge: usize::MAX,
                fresh: true,
            })
        })
        .collect()
}

/// Station names saved with a column (`GlobalStation.name`).
pub(super) fn station_names_to_nbt(list: &[([i32; 3], String)]) -> horizon_world::nbt::Tag {
    use horizon_world::nbt::{Compound, Tag};
    Tag::List(
        list.iter()
            .map(|(p, n)| {
                let mut c = Compound::new();
                c.insert("pos".into(), Tag::IntArray(p.to_vec()));
                c.insert("name".into(), Tag::String(n.as_str().into()));
                Tag::Compound(c)
            })
            .collect(),
    )
}

pub(super) fn station_names_from_nbt(
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<([i32; 3], String)> {
    use horizon_world::nbt::Tag;
    let Some(Tag::List(list)) = data.and_then(|d| d.get(&"station_names".into())) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else { return None };
            let (Some(Tag::IntArray(p)), Some(Tag::String(n))) =
                (c.get(&"pos".into()), c.get(&"name".into()))
            else {
                return None;
            };
            let p: [i32; 3] = p.as_slice().try_into().ok()?;
            Some((p, n.to_string_lossy()))
        })
        .collect()
}
thread_local! {
    /// The track each player picked first with the track item (`TRACK_CONNECTING_FROM`, kept
    /// in Create on the item; here per player until used or cleared), with its dimension.
    pub(super) static TRACK_FROM: RefCell<HashMap<u128, (String, create_engine::track::End)>> =
        RefCell::new(HashMap::new());
    /// Curves a client draws, by their first track block: their part models.
    pub(super) static CURVE_MODELS: RefCell<HashMap<[i32; 3], Vec<u32>>> =
        RefCell::new(HashMap::new());
}

/// A block state of `name` with `props` changed, if the registry has it.
fn state_with_props(
    game: &Game<'_>,
    base: BlockStateId,
    props: &[(&str, &str)],
) -> Option<BlockStateId> {
    let registry = game.pack.assets.registry;
    let mut s = registry.state(base)?.clone();
    for (k, v) in props {
        s.properties.insert((*k).to_owned(), (*v).to_owned());
    }
    registry.id(&s)
}

/// `TrackBlockItem.useOn` with a track in hand, on the server: the first click on a track picks
/// it (`select`; a junction can't start a curve); the next, on another track or where a new
/// one goes (`getPlacementState`), joins the two as `TrackPlacement.tryConnect` decides and lays
/// the track (`placeTracks`); sneaking with a pick clears it. Create's message for a failure
/// shows on the action bar. The items a survival player spends are not taken yet.
pub(super) fn use_track(game: &mut Game<'_>, pos: [i32; 3], face: [i32; 3], claimed: &mut bool) {
    use create_engine::track::{End, TrackShape};
    let Some(player) = game.native_acting_player() else {
        return;
    };
    let (uuid, dimension) = (player.uuid, player.dimension.to_owned());
    let Some(dim) = dimension_index(&dimension) else {
        return;
    };
    let Some((_, look)) = game.aim() else {
        return;
    };
    let registry = game.pack.assets.registry;
    let here = game.world.block(pos[0], pos[1], pos[2]);
    let shape_of = |id: BlockStateId| {
        registry
            .state(id)
            .filter(|s| s.name == "create:track")
            .and_then(|s| TrackShape::read(s.properties.get("shape")?))
    };
    let say = |game: &mut Game<'_>, key: &str| {
        let line = horizon_client::text::Text::translate(&format!("create.{key}"), &[]);
        game.native_action_bar(uuid, line);
    };
    let picked = TRACK_FROM
        .with_borrow(|m| m.get(&uuid).cloned())
        .filter(|(d, _)| *d == dimension)
        .map(|(_, e)| e);
    let Some(from) = picked else {
        let Some(shape) = shape_of(here) else {
            return;
        };
        *claimed = true;
        if shape.is_junction() {
            say(game, "track.junction_start");
            return;
        }
        let end = End::select(pos, shape, look);
        TRACK_FROM.with_borrow_mut(|m| m.insert(uuid, (dimension, end)));
        return;
    };
    *claimed = true;
    if game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft) {
        TRACK_FROM.with_borrow_mut(|m| m.remove(&uuid));
        say(game, "track.selection_cleared");
        return;
    }
    // The second end: the track clicked, else a new one where the click places it.
    let (pos2, state2) = match shape_of(here) {
        Some(_) => (pos, here),
        None => {
            let p = if game.res.behaviours().is_replaceable(here) {
                pos
            } else {
                [pos[0] + face[0], pos[1] + face[1], pos[2] + face[2]]
            };
            let Some(base) = registry.default_state("create:track") else {
                return;
            };
            let shape = TrackShape::for_placement(look, |_| false);
            let Some(s) = state_with_props(game, base, &[("shape", shape.name())]) else {
                return;
            };
            (p, s)
        }
    };
    let laid = lay_track(game, &dimension, dim, from, pos2, state2, look);
    if let Ok(Some(m)) | Err(Some(m)) = laid {
        say(game, &format!("track.{}", m.key()));
    }
    if laid.is_ok() {
        TRACK_FROM.with_borrow_mut(|m| m.remove(&uuid));
    }
}

/// `TrackPlacement.tryConnect` and `placeTracks` from end `from` to the track (or the track
/// state to place) `state2` at `pos2`, picked along `look`: the straight runs and the curve
/// between them laid. Ok with Create's message, if any; Err with the failure's message when
/// they can't be joined (None when there is nothing to join).
#[allow(clippy::too_many_arguments)]
fn lay_track(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    from: create_engine::track::End,
    pos2: [i32; 3],
    state2: BlockStateId,
    look: create_engine::bezier::Vec3,
) -> Result<Option<create_engine::track::Failure>, Option<create_engine::track::Failure>> {
    use create_engine::track::{self, End, Request, TrackShape};
    let registry = game.pack.assets.registry;
    let shape_of = |id: BlockStateId| {
        registry
            .state(id)
            .filter(|s| s.name == "create:track")
            .and_then(|s| TrackShape::read(s.properties.get("shape")?))
    };
    let Some(shape2) = shape_of(state2) else {
        return Err(None);
    };
    let state1 = game.world.block(from.pos[0], from.pos[1], from.pos[2]);
    let info = track::try_connect(
        from,
        End::select(pos2, shape2, look),
        Request {
            original_present: shape_of(state1).is_some(),
            second_tilted: false,
            max_length: track::MAX_PLACEMENT_LENGTH,
            maximise_turn: false,
            preview: false,
        },
    );
    if !info.valid {
        return Err(info.message);
    }
    // `placeTracks`: each end's state (a T junction's straight part) along its run.
    for (i, state) in [state1, state2].into_iter().enumerate() {
        let shape = match shape_of(state).map(TrackShape::name) {
            Some("te" | "tw") => "xo",
            Some("tn" | "ts") => "zo",
            Some(other) => other,
            None => continue,
        };
        let Some(place) = state_with_props(game, state, &[("shape", shape), ("turn", "false")])
        else {
            continue;
        };
        for p in info.straight(i).collect::<Vec<_>>() {
            let at = game.world.block(p[0], p[1], p[2]);
            if game.res.behaviours().is_replaceable(at) || shape_of(at).is_some() {
                game.native_set_block(dimension, p, place, super::native_hooks::flags::UPDATE_ALL);
            }
        }
    }
    let Some(curve) = info.curve.as_ref() else {
        return Ok(info.message);
    };
    for (t, state) in info.targets.into_iter().zip([state1, state2]) {
        let at = game.world.block(t[0], t[1], t[2]);
        let base = if shape_of(at).is_some() { at } else { state };
        if let Some(s) = state_with_props(game, base, &[("turn", "true")]) {
            game.native_set_block(dimension, t, s, super::native_hooks::flags::UPDATE_ALL);
        }
    }
    let saved = create_engine::track_curves::Saved {
        from: info.targets[0],
        to: info.targets[1],
        starts: curve.starts,
        axes: curve.axes,
        normals: curve.normals,
    };
    with_world(|w| {
        w.dims[dim].track_curves.add(saved);
        w.mark_unsaved(dim, saved.from);
        w.mark_unsaved(dim, saved.to);
    });
    Ok(info.message)
}

/// `create_debug curve <from> <to>`: joins two track blocks as the track item's two clicks do
/// (`tryConnect`, `placeTracks`), as if looking from the first toward the second: straight
/// lead-ins and the curve between them. A test driver standing in for the item's clicks.
pub(super) fn debug_curve(game: &mut Game<'_>, dim: usize, a: [i32; 3], b: [i32; 3]) -> String {
    use create_engine::track::{End, TrackShape};
    let registry = game.pack.assets.registry;
    let shape_at = |game: &Game<'_>, p: [i32; 3]| {
        registry
            .state(game.world.block(p[0], p[1], p[2]))
            .filter(|s| s.name == "create:track")
            .and_then(|s| TrackShape::read(s.properties.get("shape")?))
    };
    let (Some(shape1), Some(_)) = (shape_at(game, a), shape_at(game, b)) else {
        return "both ends must be track".into();
    };
    let d = [
        f64::from(b[0] - a[0]),
        f64::from(b[1] - a[1]),
        f64::from(b[2] - a[2]),
    ];
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-9);
    let look = [d[0] / l, d[1] / l, d[2] / l];
    let dimension = game.dim.id().to_owned();
    let state2 = game.world.block(b[0], b[1], b[2]);
    match lay_track(
        game,
        &dimension,
        dim,
        End::select(a, shape1, look),
        b,
        state2,
        look,
    ) {
        Ok(None) => "track joined".into(),
        Ok(Some(m)) => format!("track joined (track.{})", m.key()),
        Err(m) => format!("can't join: track.{}", m.map_or("missing", |m| m.key())),
    }
}

/// Curves that changed since the last tick: a track left with none is no longer `turn`
/// (`HAS_BE`), and players are sent what each block draws now.
pub(super) fn sync_curves(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let Some(changed) = with_world(|w| {
        let c = &mut w.dims[dim].track_curves.changed;
        if c.is_empty() {
            return Vec::new();
        }
        let mut c = std::mem::take(c);
        c.sort();
        c.dedup();
        c
    }) else {
        return;
    };
    if changed.is_empty() {
        return;
    }
    let registry = game.pack.assets.registry;
    let mut shown = Vec::with_capacity(changed.len());
    for pos in changed {
        let curves = with_world(|w| w.dims[dim].track_curves.drawn_from(pos)).unwrap_or_default();
        let none = with_world(|w| !w.dims[dim].track_curves.at.contains_key(&pos)).unwrap_or(true);
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let turned = registry.state(state).is_some_and(|s| {
            s.name == "create:track" && s.properties.get("turn").map(String::as_str) == Some("true")
        });
        if none && turned {
            if let Some(s) = state_with_props(game, state, &[("turn", "false")]) {
                game.native_set_block(dimension, pos, s, super::native_hooks::flags::UPDATE_ALL);
            }
        }
        shown.push((pos, curves));
    }
    let players = players_in(game, dimension);
    send_curves(game, &players, &shown);
}

pub(super) fn send_curves(
    game: &mut Game<'_>,
    players: &[u128],
    shown: &[([i32; 3], Vec<create_engine::track_curves::Saved>)],
) {
    if shown.is_empty() {
        return;
    }
    let data = create_engine::track_curves::encode(shown);
    for &p in players {
        let _ = game.native_net_send_to_player(p, create_engine::track_curves::CHANNEL, &data);
    }
}

/// Curves on the client (`TrackRenderer.renderBezierTurn`): each block's curves are one part
/// model of ties and rails, baked when they change (drawn on the GPU, never re-uploaded while
/// they stay).
pub(super) fn curves_from_server(game: &mut Game<'_>, data: &[u8]) {
    use create_engine::track_render::{Piece, segment_parts};
    let registry = game.pack.assets.registry;
    let part = |name: &str| {
        let d = registry.default_state(name)?;
        let mut s = registry.state(d)?.clone();
        s.properties.insert("facing".to_owned(), "south".to_owned());
        registry.id(&s)
    };
    let (Some(tie), Some(left), Some(right)) = (
        part("create:part_track_tie"),
        part("create:part_track_segment_left"),
        part("create:part_track_segment_right"),
    ) else {
        return;
    };
    for (pos, curves) in create_engine::track_curves::decode(data) {
        if let Some(old) = CURVE_MODELS.with_borrow_mut(|m| m.remove(&pos)) {
            for (k, model) in old.into_iter().enumerate() {
                game.native_part_clear(depot_key(pos, 120, k as u8));
                game.native_part_model_free(model);
            }
        }
        let mut models = Vec::new();
        for (k, s) in curves.iter().enumerate() {
            let blocks: Vec<(BlockStateId, glam::Mat4)> = segment_parts(&s.curve(), pos)
                .into_iter()
                .map(|(piece, m)| {
                    let state = match piece {
                        Piece::Tie => tie,
                        Piece::RailLeft => left,
                        Piece::RailRight => right,
                    };
                    (state, glam::Mat4::from_cols_array_2d(&m))
                })
                .collect();
            let Some(model) = game.native_part_model_compound(&blocks) else {
                continue;
            };
            game.native_part_set(depot_key(pos, 120, k as u8), model, pos, 0, 0.0, 0.0);
            models.push(model);
        }
        if !models.is_empty() {
            CURVE_MODELS.with_borrow_mut(|m| m.insert(pos, models));
        }
    }
}

/// A column's curves as saved (`TrackBlockEntity.write`'s `Connections`): ends and numbers.
pub(super) fn curves_to_nbt(
    list: &[create_engine::track_curves::Saved],
) -> horizon_world::nbt::Tag {
    use horizon_world::nbt::{Compound, Tag};
    Tag::List(
        list.iter()
            .map(|s| {
                let (i, f) = s.numbers();
                let mut c = Compound::new();
                c.insert("ends".into(), Tag::IntArray(i.to_vec()));
                c.insert(
                    "curve".into(),
                    Tag::List(f.iter().map(|v| Tag::Double(v.to_bits())).collect()),
                );
                Tag::Compound(c)
            })
            .collect(),
    )
}

pub(super) fn curves_from_nbt(
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<create_engine::track_curves::Saved> {
    use horizon_world::nbt::Tag;
    let Some(Tag::List(list)) = data.and_then(|d| d.get(&"track_curves".into())) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else { return None };
            let Some(Tag::IntArray(i)) = c.get(&"ends".into()) else {
                return None;
            };
            let Some(Tag::List(f)) = c.get(&"curve".into()) else {
                return None;
            };
            let i: [i32; 6] = i.as_slice().try_into().ok()?;
            let f: Vec<f64> = f
                .iter()
                .filter_map(|v| match v {
                    Tag::Double(b) => Some(f64::from_bits(*b)),
                    _ => None,
                })
                .collect();
            Some(create_engine::track_curves::Saved::from_numbers(
                i,
                f.try_into().ok()?,
            ))
        })
        .collect()
}
