// Create's trains in the game: stations, tracks, the trains running between them and their
// save data, over the engine's `trains`. A sibling of the `create` module (`runtime.rs`).
use super::Game;
use super::create::*;
use super::create_contraptions::*;
use create_engine::kinetics::{Axis, Dir};
use horizon_world::registry::BlockStateId;
use std::cell::RefCell;
use std::collections::HashMap;

pub(super) const TRAIN_CHANNEL: &str = "create:train";

/// The straight track a station targets (`TrackTargetingBehaviour`): the track block beside it,
/// and the line through it.
pub(super) fn station_track(
    game: &Game<'_>,
    pos: [i32; 3],
) -> Option<([i32; 3], create_engine::trains::Line)> {
    use create_engine::trains::Line;
    let registry = game.pack.assets.registry;
    let axis_at = |p: [i32; 3]| -> Option<Axis> {
        let s = registry.state(game.world.block(p[0], p[1], p[2]))?;
        if s.name != "create:track" {
            return None;
        }
        match s.properties.get("shape").map(String::as_str) {
            Some("zo") => Some(Axis::Z),
            Some("xo") => Some(Axis::X),
            _ => None,
        }
    };
    for d in [Dir::North, Dir::South, Dir::West, Dir::East] {
        let o = d.offset();
        let t = [pos[0] + o[0], pos[1], pos[2] + o[2]];
        if let Some(axis) = axis_at(t) {
            return Some((t, Line::trace(t, axis, |p| axis_at(p) == Some(axis))));
        }
    }
    None
}

/// The station's assembly (`StationBlockEntity.assemble` / `disassemble` from its screen):
/// a train standing at it goes back into the world; else the bogeys on its track (a block above
/// it) and what sticks to them become a train.
pub(super) fn use_station(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    use create_engine::trains::{Line, Train};
    let Some((target, line)) = station_track(game, pos) else {
        return;
    };
    let here = Line::along(line.axis, target) as f64;
    let dimension = game.dim.id().to_owned();
    let stopped = with_world(|w| {
        let d = &mut w.dims[dim];
        let i = d.trains.iter().position(|t| {
            t.train.line == line
                && t.train.destination.is_none()
                && ((t.train.position - here).abs() < 0.5
                    // Still where it was assembled (its bogey beside this station).
                    || (t.train.position == t.start && (t.start - here).abs() <= 2.0))
        })?;
        Some(d.trains.remove(i))
    })
    .flatten();
    if let Some(t) = stopped {
        let shift = (t.train.position - t.start).round() as i32;
        let along = |p: [i32; 3]| {
            let mut p = p;
            match line.axis {
                Axis::X => p[0] += shift,
                Axis::Y => p[1] += shift,
                Axis::Z => p[2] += shift,
            }
            p
        };
        for (l, s) in &t.blocks {
            let p = along([t.origin[0] + l[0], t.origin[1] + l[1], t.origin[2] + l[2]]);
            game.native_set_block(
                &dimension,
                p,
                BlockStateId(*s),
                super::native_hooks::flags::UPDATE_ALL,
            );
        }
        send_train_clear(game, &dimension, t.id);
        return;
    }
    // Bogeys along the track, one above it.
    let step = |a: i32| match line.axis {
        Axis::X => [a, target[1] + 1, target[2]],
        Axis::Y => [target[0], a, target[2]],
        Axis::Z => [target[0], target[1] + 1, a],
    };
    let bogeys: Vec<[i32; 3]> = (line.from..=line.to)
        .map(step)
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
    let track: Vec<[i32; 3]> = (line.from..=line.to)
        .map(|a| [step(a)[0], target[1], step(a)[2]])
        .collect();
    let mut blocks: Vec<([i32; 3], u32)> = Vec::new();
    for b in &bogeys {
        // Down is the track, left out: only what sticks is taken.
        let Ok(found) =
            create_engine::piston::gather(*b, Dir::Down, &track, |p| contraption_found(game, p))
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
    let air = BlockStateId(0);
    for (l, _) in &blocks {
        let p = [origin[0] + l[0], origin[1] + l[1], origin[2] + l[2]];
        game.native_set_block(&dimension, p, air, super::native_hooks::flags::UPDATE_ALL);
    }
    let start = Line::along(line.axis, origin) as f64;
    let id = with_world(|w| {
        let d = &mut w.dims[dim];
        let id = d.trains.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        d.trains.push(create_engine::world::TrainOnTrack {
            train: Train {
                line,
                position: start,
                speed: 0.0,
                destination: None,
            },
            blocks,
            origin,
            start,
            id,
        });
        id
    })
    .unwrap_or(0);
    game.native_mark_chunk_unsaved(&dimension, [origin[0] >> 4, origin[2] >> 4]);
    send_train(game, dim, [i32::MIN, 0, id as i32]);
}

/// `create_debug train <station>`: the train on that station's track heads for it. With a
/// position of `[i32::MIN, 0, id]`, sends train `id` to clients as it is.
pub(super) fn send_train(game: &mut Game<'_>, dim: usize, station: [i32; 3]) -> String {
    use create_engine::trains::Line;
    let id = if station[0] == i32::MIN {
        Some(station[2] as u32)
    } else {
        let Some((target, line)) = station_track(game, station) else {
            return "no track beside that station".into();
        };
        let dest = Line::along(line.axis, target) as f64;
        with_world(|w| {
            let t = w.dims[dim]
                .trains
                .iter_mut()
                .find(|t| t.train.line == line)?;
            t.train.destination = Some(dest);
            Some(t.id)
        })
        .flatten()
    };
    let Some(id) = id else {
        return "no train on that track".into();
    };
    let Some(t) = with_world(|w| w.dims[dim].trains.iter().find(|t| t.id == id).cloned()).flatten()
    else {
        return "no such train".into();
    };
    let mut msg = Vec::new();
    msg.extend_from_slice(&t.id.to_le_bytes());
    for v in t.origin {
        msg.extend_from_slice(&v.to_le_bytes());
    }
    msg.push(axis_index(t.train.line.axis));
    msg.extend_from_slice(&((t.train.position - t.start) as f32).to_le_bytes());
    msg.extend_from_slice(&(t.train.speed as f32).to_le_bytes());
    let blocks = &t.blocks[..t.blocks.len().min((30_000 - 29) / 16)];
    msg.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    for (o, s) in blocks {
        for v in o {
            msg.extend_from_slice(&v.to_le_bytes());
        }
        msg.extend_from_slice(&s.to_le_bytes());
    }
    let dimension = game.dim.id().to_owned();
    for p in players_in(game, &dimension) {
        let _ = game.native_net_send_to_player(p, TRAIN_CHANNEL, &msg);
    }
    format!(
        "train {id} heading for {:.1}",
        t.train.destination.unwrap_or(t.train.position)
    )
}

pub(super) fn send_train_clear(game: &mut Game<'_>, dimension: &str, id: u32) {
    for p in players_in(game, dimension) {
        let _ = game.native_net_send_to_player(p, TRAIN_CHANNEL, &id.to_le_bytes());
    }
}

/// Trains running to a destination (`Train.tick`, `Navigation.tick`); clients get each one's
/// motion when it starts, every ten ticks while its speed changes, and when it stops.
pub(super) fn tick_trains(game: &mut Game<'_>, _dimension: &str, dim: usize) {
    use create_engine::trains::{Line, Run};
    // Signals by the line they stand by and where along it (`TrackTargetingBehaviour`).
    let signals: Vec<(Line, f64)> = with_world(|w| w.dims[dim].signals.clone())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| {
            let (target, line) = station_track(game, p)?;
            Some((line, f64::from(Line::along(line.axis, target))))
        })
        .collect();
    // Each train's span along its line (its blocks where it is now).
    let span = |t: &create_engine::world::TrainOnTrack| {
        let shift = t.train.position - t.start;
        let along = |o: [i32; 3]| {
            f64::from(Line::along(
                t.train.line.axis,
                [t.origin[0] + o[0], t.origin[1] + o[1], t.origin[2] + o[2]],
            ))
        };
        let lo = t
            .blocks
            .iter()
            .map(|(o, _)| along(*o))
            .fold(f64::INFINITY, f64::min);
        let hi = t
            .blocks
            .iter()
            .map(|(o, _)| along(*o))
            .fold(f64::NEG_INFINITY, f64::max);
        (lo + shift, hi + 1.0 + shift)
    };
    let running: Vec<(u32, Run, f64, (i32, i32), (i32, i32))> = with_world(|w| {
        let spans: Vec<(u32, Line, (f64, f64))> = w.dims[dim]
            .trains
            .iter()
            .map(|t| (t.id, t.train.line, span(t)))
            .collect();
        w.dims[dim]
            .trains
            .iter_mut()
            .filter(|t| t.train.destination.is_some())
            .map(|t| {
                let before = t.train.speed;
                let was = create_engine::world::CreateWorld::train_column(t);
                // A red signal ahead: it waits before it (`Navigation`'s `waitingForSignal`).
                let dest = t.train.destination.unwrap_or(t.train.position);
                let dir = if dest < t.train.position { -1.0 } else { 1.0 };
                let (lo, hi) = span(t);
                let front = if dir > 0.0 { hi } else { lo };
                let own: Vec<f64> = signals
                    .iter()
                    .filter(|(l, _)| *l == t.train.line)
                    .map(|(_, s)| *s + 0.5)
                    .collect();
                let others: Vec<(f64, f64)> = spans
                    .iter()
                    .filter(|(id, l, _)| *id != t.id && *l == t.train.line)
                    .map(|(_, _, s)| *s)
                    .collect();
                let stop = create_engine::trains::blocking_signal(
                    front,
                    dir,
                    front + (dest - t.train.position),
                    &own,
                    &others,
                )
                .map(|s| t.train.position + (s - dir * 0.5 - front));
                let run = t.train.tick_until(stop);
                (
                    t.id,
                    run,
                    (t.train.speed - before).abs(),
                    was,
                    create_engine::world::CreateWorld::train_column(t),
                )
            })
            .collect()
    })
    .unwrap_or_default();
    // Saved with the column it stands in: the one it left and the one it is in save again.
    for (_, _, _, was, now) in &running {
        if was != now {
            game.native_mark_chunk_unsaved(_dimension, [was.0, was.1]);
            game.native_mark_chunk_unsaved(_dimension, [now.0, now.1]);
        }
    }
    let running: Vec<(u32, create_engine::trains::Run, f64)> = running
        .into_iter()
        .map(|(id, run, change, _, now)| {
            if run == create_engine::trains::Run::Arrived {
                game.native_mark_chunk_unsaved(_dimension, [now.0, now.1]);
            }
            (id, run, change)
        })
        .collect();
    let tick = game.world.game_time;
    for (id, run, change) in running {
        let stopped = with_world(|w| {
            w.dims[dim]
                .trains
                .iter()
                .any(|t| t.id == id && t.train.speed == 0.0)
        }) == Some(true);
        if run == Run::Arrived || (change > 0.0 && (tick % 10 == 0 || stopped)) {
            send_train(game, dim, [i32::MIN, 0, id as i32]);
        }
    }
}

thread_local! {
    /// Trains a client draws, by id: their part model.
    pub(super) static TRAINS: RefCell<HashMap<u32, u32>> = RefCell::new(HashMap::new());
}

/// A train on the client: one part model gliding along its track at its speed from where it is
/// now (`CarriageContraptionEntity`'s motion), until the next message.
pub(super) fn trains_from_server(game: &mut Game<'_>, d: &[u8]) {
    let i = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    if d.len() < 4 {
        return;
    }
    let id = u32::from_le_bytes(d[0..4].try_into().unwrap());
    let key = depot_key([id as i32, 0, 0], 222, 0);
    if d.len() < 29 {
        if let Some(old) = TRAINS.with_borrow_mut(|t| t.remove(&id)) {
            game.native_part_clear(key);
            game.native_part_model_free(old);
        }
        return;
    }
    let origin = [i(&d[4..8]), i(&d[8..12]), i(&d[12..16])];
    let axis = d[16].min(2) as usize;
    let f = |b: &[u8]| f32::from_le_bytes(b.try_into().unwrap());
    let (offset, speed) = (f(&d[17..21]), f(&d[21..25]));
    let n = u32::from_le_bytes(d[25..29].try_into().unwrap()) as usize;
    let model = match TRAINS.with_borrow(|t| t.get(&id).copied()) {
        Some(m) => m,
        None => {
            let blocks: Vec<(BlockStateId, glam::Mat4)> = d[29..]
                .chunks_exact(16)
                .take(n)
                .map(|r| {
                    let o = glam::Vec3::new(
                        i(&r[0..4]) as f32,
                        i(&r[4..8]) as f32,
                        i(&r[8..12]) as f32,
                    );
                    (
                        BlockStateId(u32::from_le_bytes(r[12..16].try_into().unwrap())),
                        glam::Mat4::from_translation(o),
                    )
                })
                .collect();
            let Some(m) = game.native_part_model_compound(&blocks) else {
                return;
            };
            TRAINS.with_borrow_mut(|t| t.insert(id, m));
            m
        }
    };
    let mut at = [0.0f32; 4];
    let mut v = [0.0f32; 3];
    at[axis] = offset;
    at[3] = game.native_parts_seconds();
    v[axis] = speed * 20.0;
    game.native_part_glide(key, model, origin, 0, 0.0, 0.0, at, v);
}

/// Trains as saved (`Train.write`, here with the column they stand in): their line, motion,
/// first bogey and the blocks they carry.
pub(super) fn trains_to_nbt(
    game: &Game<'_>,
    all: &[create_engine::world::TrainOnTrack],
) -> horizon_world::nbt::Tag {
    use horizon_world::nbt::{Compound, Tag};
    let registry = game.pack.assets.registry;
    let f = |v: f64| (v as f32).to_bits() as i32;
    Tag::List(
        all.iter()
            .map(|t| {
                let l = t.train.line;
                let mut c = Compound::new();
                c.insert(
                    "line".into(),
                    Tag::IntArray(vec![
                        i32::from(axis_index(l.axis)),
                        l.fixed[0],
                        l.fixed[1],
                        l.fixed[2],
                        l.from,
                        l.to,
                    ]),
                );
                c.insert(
                    "motion".into(),
                    Tag::IntArray(vec![
                        f(t.train.position),
                        f(t.train.speed),
                        f(t.train.destination.unwrap_or(0.0)),
                        i32::from(t.train.destination.is_some()),
                        t.origin[0],
                        t.origin[1],
                        t.origin[2],
                        f(t.start),
                    ]),
                );
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
                Tag::Compound(c)
            })
            .collect(),
    )
}

pub(super) fn trains_from_nbt(
    game: &Game<'_>,
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<create_engine::world::TrainOnTrack> {
    use create_engine::trains::{Line, Train};
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
                Some(Tag::IntArray(l)),
                Some(Tag::IntArray(m)),
                Some(Tag::IntArray(o)),
                Some(Tag::List(s)),
            ) = (
                c.get(&"line".into()),
                c.get(&"motion".into()),
                c.get(&"blocks".into()),
                c.get(&"states".into()),
            )
            else {
                return None;
            };
            if l.len() < 6 || m.len() < 8 {
                return None;
            }
            let axis = [Axis::X, Axis::Y, Axis::Z][l[0].clamp(0, 2) as usize];
            let line = Line {
                axis,
                fixed: [l[1], l[2], l[3]],
                from: l[4],
                to: l[5],
            };
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
            Some(create_engine::world::TrainOnTrack {
                train: Train {
                    line,
                    position: f(m[0]),
                    speed: f(m[1]),
                    destination: (m[3] != 0).then(|| f(m[2])),
                },
                blocks,
                origin: [m[4], m[5], m[6]],
                start: f(m[7]),
                id: 0,
            })
        })
        .collect()
}
