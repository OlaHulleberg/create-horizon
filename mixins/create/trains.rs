// Create's trains in the game: stations, tracks, signals, the trains running between them over
// the track graph and their save data, over the engine's `trains` and `network`. A sibling of
// the `create` module (`runtime.rs`).
//
// Riding a carriage's seats and driving by its controls are `train_controls.rs`
// (`create_train_controls`); `/create_debug drive` holds a train's controls by command.
use super::Game;
use super::create::*;
use super::create_contraptions::*;
use super::create_logistics::*;
use create_engine::kinetics::{Axis, Dir};
use create_engine::states;
use create_engine::world::dimension_index;
use horizon_world::registry::BlockStateId;
use std::cell::RefCell;
use std::collections::HashMap;

pub(super) const TRAIN_CHANNEL: &str = create_engine::trains::CHANNEL;
pub(super) const SIGNAL_CHANNEL: &str = create_engine::trains::SIGNAL_CHANNEL;

/// The graph as the world has it now (built again only if track changed).
fn railway_ready(dim: usize) {
    with_world(|w| {
        let d = &mut w.dims[dim];
        d.railway.ensure(&d.track_curves);
    });
}

/// The size of the bogey whose block state is `state`, if it is one.
fn bogey_size(state: BlockStateId) -> Option<create_engine::trains::BogeySize> {
    use create_engine::trains::BogeySize;
    match with_world(|w| w.table.get(state.0).map(|(b, _)| b)).flatten()? {
        create_engine::Block::SmallBogey => Some(BogeySize::Small),
        create_engine::Block::LargeBogey => Some(BogeySize::Large),
        _ => None,
    }
}

/// Tells the acting player why the station can't assemble (`AssemblyException`, shown on the
/// station's screen in Create), and gives the line back.
fn assembly_failed(game: &mut Game<'_>, e: create_engine::trains::AssemblyError) -> String {
    let key = format!("create.train_assembly.{}", e.key());
    let args: Vec<horizon_client::text::Text> = e
        .args()
        .into_iter()
        .map(horizon_client::text::Text::literal)
        .collect();
    if let Some(p) = game.native_acting_player() {
        game.native_action_bar(p.uuid, horizon_client::text::Text::translate(&key, &args));
    }
    format!("can't assemble: {key} {}", e.args().join(" "))
}

/// The station's assembly (`StationBlockEntity.assemble` / `disassemble` from its screen): a
/// train standing at it goes back into the world, each carriage where it stands (turned to its
/// quarter; `canDisassemble`: only level and square to the grid); else the bogeys along its
/// track and what each carries become carriages of a train on the track graph, by Create's
/// rules. The station's track runs on toward its assembly direction: the way whose first track
/// block has a bogey on it (Create's frontmost bogey at the station; the direction Create takes
/// from the click that targets the track). A line for the command.
pub(super) fn use_station(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) -> Option<String> {
    use create_engine::schedule::Runtime;
    use create_engine::trains::{
        AssemblyError, MAX_ASSEMBLY_LENGTH, MAX_BOGEY_COUNT, Structure, Train, TrainOnTrack, layout,
    };
    railway_ready(dim);
    // A train assembled or taken apart here: its comparators read `trainPresent` again.
    analog_touched(dim, pos);
    let dimension = game.dim.id().to_owned();
    let stopped = with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let i = r.trains.iter().position(|t| {
            t.train.bound
                && t.train.nav.is_none()
                && t.train.speed == 0.0
                && t.train.at_station == Some(pos)
        })?;
        let t = &r.trains[i];
        let g = &r.net.graph;
        let poses: Vec<_> = (0..t.train.carriages.len())
            .map(|c| t.train.carriage_pose(g, c))
            .collect();
        // `canDisassemble`.
        let square = poses.iter().all(|(_, front)| {
            let (yaw, pitch) = create_engine::trains::yaw_pitch(*front, t.heading);
            pitch.abs() < 1.0e-3 && ((yaw.rem_euclid(90.0) + 1.0e-3) % 90.0) < 2.0e-3
        });
        if !square {
            return Some(Err(()));
        }
        let t = r.trains.remove(i);
        Some(Ok((t, poses)))
    })
    .flatten();
    match stopped {
        Some(Err(())) => return Some("the train can't be disassembled here".into()),
        Some(Ok((t, poses))) => {
            // Its seat blocks: where each went, for their riders.
            let mut seats = Vec::new();
            for (c, (car, (pivot, front))) in t.cars.iter().zip(poses).enumerate() {
                let (yaw, _) = create_engine::trains::yaw_pitch(front, t.heading);
                let quarter = ((yaw / 90.0).round() as i32 * 90).rem_euclid(360);
                for (o, s) in car {
                    // The block's centre from the pivot (the middle of the track under the
                    // carriage's leading bogey), turned about it; its height from the track.
                    let r = create_engine::contraption::rotate([o[0], 0, o[2]], quarter, Axis::Y);
                    let p = [
                        (pivot[0] + f64::from(r[0])).floor() as i32,
                        (pivot[1] + f64::from(o[1]) + 1.0).round() as i32,
                        (pivot[2] + f64::from(r[2])).floor() as i32,
                    ];
                    let state = turned_state(game, *s, quarter, Axis::Y);
                    game.native_set_block(
                        &dimension,
                        p,
                        BlockStateId(state),
                        super::native_hooks::flags::UPDATE_ALL,
                    );
                    if states::index(*s).is_some()
                        && with_world(|w| w.table.get(*s).map(|(b, _)| b))
                            .flatten()
                            .is_some_and(create_engine::dye::is_seat)
                    {
                        seats.push((c as u16, *o, p));
                    }
                }
            }
            game.native_mark_chunk_unsaved(&dimension, [t.column.0, t.column.1]);
            super::create_train_controls::disassembled(game, dim, t.id, &seats);
            send_train_clear(game, &dimension, t.id);
            return Some(format!("train {} disassembled", t.id));
        }
        None => {}
    }
    // The straight track the station looks at, and which way along it the train is.
    let (target, shape) = with_world(|w| {
        let r = &w.dims[dim].railway;
        let target = create_engine::network::target_track(&r.tracks, pos)?;
        Some((target, r.tracks[&target]))
    })
    .flatten()?;
    let along = if shape == create_engine::track::TrackShape::Xo {
        [1, 0, 0]
    } else {
        [0, 0, 1]
    };
    let bogey_at =
        |game: &Game<'_>, p: [i32; 3]| bogey_size(game.world.block(p[0], p[1] + 1, p[2]));
    let direction = [1, -1]
        .into_iter()
        .map(|s| [along[0] * s, 0, along[2] * s])
        .find(|d| bogey_at(game, [target[0] + d[0], target[1], target[2] + d[2]]).is_some())
        .unwrap_or(along);
    // `refreshAssemblyInfo`: the track on from the station's, while it runs the same way; a
    // bogey above it at each place counts.
    let mut run = Vec::new();
    let mut found: Vec<(i32, [i32; 3], create_engine::trains::BogeySize)> = Vec::new();
    for i in 0..MAX_ASSEMBLY_LENGTH - 1 {
        let p = [
            target[0] + direction[0] * (i + 1),
            target[1],
            target[2] + direction[2] * (i + 1),
        ];
        if with_world(|w| w.dims[dim].railway.tracks.get(&p) != Some(&shape)) != Some(false) {
            break;
        }
        run.push(p);
        if found.len() < MAX_BOGEY_COUNT
            && let Some(size) = bogey_at(game, p)
        {
            found.push((i, [p[0], p[1] + 1, p[2]], size));
        }
    }
    let locations: Vec<i32> = found.iter().map(|b| b.0).collect();
    let dir_name = match direction {
        [1, _, 0] => "east",
        [-1, _, 0] => "west",
        [0, _, 1] => "south",
        _ => "north",
    };
    // Each carriage's structure from its first bogey (`CarriageContraption.assemble`): what
    // sticks to it, not the track.
    let mut gathered: HashMap<usize, Vec<([i32; 3], u32)>> = HashMap::new();
    let mut refused = false;
    let laid = layout(&locations, |i| {
        let b = found[i].1;
        let Ok(blocks) =
            create_engine::piston::gather(b, Dir::Down, &run, |p| contraption_found(game, p))
        else {
            refused = true;
            return Structure::default();
        };
        let registry = game.pack.assets.registry;
        let mut s = Structure {
            blocks: blocks.len(),
            ..Structure::default()
        };
        for (o, state) in &blocks {
            let p = [b[0] + o[0], b[1] + o[1], b[2] + o[2]];
            if let Some(k) = found.iter().find(|f| f.1 == p) {
                s.bogeys.push(k.0);
            }
            // `CarriageContraption.capture`: train controls by their facing.
            if let Some(st) = registry.state(BlockStateId(*state))
                && st.name == "create:controls"
                && let Some(f) = st.properties.get("facing")
            {
                let across = match direction[0] {
                    0 => matches!(f.as_str(), "east" | "west"),
                    _ => matches!(f.as_str(), "north" | "south"),
                };
                if across {
                    s.sideways = true;
                } else if f == dir_name {
                    s.forward = true;
                } else {
                    s.backward = true;
                }
            }
        }
        gathered.insert(i, blocks);
        s
    });
    let laid = match laid {
        Ok(_) if refused => return Some("can't assemble: something on it can't move".into()),
        Ok(l) => l,
        Err(e) => return Some(assembly_failed(game, e)),
    };
    let sizes: Vec<_> = found.iter().map(|b| b.2).collect();
    let dir = [f64::from(direction[0]), 0.0, f64::from(direction[2])];
    // The end of the station's track block toward the assembly direction.
    let origin = [
        f64::from(target[0]) + 0.5 + dir[0] * 0.5,
        f64::from(target[1]),
        f64::from(target[2]) + 0.5 + dir[2] * 0.5,
    ];
    let cars: Vec<Vec<([i32; 3], u32)>> = laid
        .carriages
        .iter()
        .map(|c| gathered.remove(&c.first).unwrap_or_default())
        .collect();
    let heading = [-direction[0], -direction[2]];
    let origin_block = found.first().map_or(target, |f| f.1);
    let made = with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let mut train = Train::assembled(&r.net, origin, dir, &laid, &sizes)?;
        train.at_station = Some(pos);
        let id = r.next_id();
        let mut t = TrainOnTrack {
            id,
            train,
            cars: cars.clone(),
            heading,
            schedule: None,
            runtime: Runtime::new(),
            idle: 0,
            column: (origin_block[0] >> 4, origin_block[2] >> 4),
            sent_tick: 0,
            sent_edge: usize::MAX,
            fresh: true,
        };
        t.column = t.column_on(&r.net);
        r.trains.push(t);
        Some(id)
    })
    .flatten();
    let Some(id) = made else {
        return Some(assembly_failed(game, AssemblyError::NoBogeys));
    };
    // `moveSeat`: whoever sits on a seat block it takes rides that carriage seat.
    let mut seats = Vec::new();
    for (n, (c, car)) in laid.carriages.iter().zip(&cars).enumerate() {
        let b = found[c.first].1;
        for (o, s) in car {
            if with_world(|w| w.table.get(*s).map(|(b, _)| b))
                .flatten()
                .is_some_and(create_engine::dye::is_seat)
            {
                seats.push((n as u16, *o, [b[0] + o[0], b[1] + o[1], b[2] + o[2]]));
            }
        }
    }
    super::create_train_controls::assembled(dim, id, &seats);
    let air = BlockStateId(0);
    for (c, car) in laid.carriages.iter().zip(&cars) {
        let b = found[c.first].1;
        for (o, _) in car {
            let p = [b[0] + o[0], b[1] + o[1], b[2] + o[2]];
            game.native_set_block(&dimension, p, air, super::native_hooks::flags::UPDATE_ALL);
            game.native_mark_chunk_unsaved(&dimension, [p[0] >> 4, p[2] >> 4]);
        }
    }
    send_trains(game, dim, &[id], true);
    Some(format!(
        "train {id} assembled: {} carriages on {} bogeys",
        laid.carriages.len(),
        sizes.len()
    ))
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
                let p = t.position(&r.net);
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
                let p = t.position(&r.net);
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
        t.train.manual = None;
        format!("train {} runs {}", t.id, names.join(" → "))
    })
    .unwrap_or_default()
}

/// `ScheduleItem.handScheduleTo` for the train standing at `station` (a conductor's click,
/// without the conductor): refused while it runs a schedule or for one without stops, else it
/// runs `entries` as `schedule_train` sets them.
pub(super) fn hand_schedule(
    dim: usize,
    station: [i32; 3],
    entries: &str,
) -> Result<String, &'static str> {
    railway_ready(dim);
    let found = with_world(|w| {
        let r = &w.dims[dim].railway;
        r.trains
            .iter()
            .find(|t| t.train.at_station == Some(station))
            .map(|t| t.schedule.is_some())
    })
    .flatten();
    match found {
        None => Err("no train at that station"),
        Some(true) => Err("create.schedule.remove_with_empty_hand"),
        Some(false) if entries.trim().is_empty() => Err("create.schedule.no_stops"),
        Some(false) => Ok(schedule_train(dim, station, entries)),
    }
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

/// `/create_debug drive <train> <throttle> [left|right|straight]`: holds train `<train>`'s
/// controls as a player riding it would (`ControlsHandler` sending `ControlsInputPacket`,
/// `CarriageContraptionEntity.control`): the throttle from -1 (back, at a quarter speed) to 1
/// (forward), 0 to let go; the steering at junctions. Standing in for riding the train, which
/// Horizon can't do yet (`Entity.startRiding`).
pub(super) fn train_command(game: &mut Game<'_>, command: &str) -> bool {
    use create_engine::trains::Manual;
    let usage = "usage: create_debug drive <train> <throttle -1..1> [left|right|straight]";
    if let Some(rest) = command.strip_prefix("create_debug assemble ") {
        let dim = game
            .native_acting_player()
            .and_then(|p| dimension_index(&p.dimension))
            .unwrap_or(0);
        let here = game.native_acting_player().map(|p| p.position);
        let parts: Vec<i32> = rest
            .split_whitespace()
            .enumerate()
            .filter_map(|(n, t)| match t.strip_prefix('~') {
                Some(off) => {
                    Some(here?.get(n % 3)?.floor() as i32 + off.parse::<i32>().unwrap_or(0))
                }
                None => t.parse().ok(),
            })
            .collect();
        let line = match parts.as_slice() {
            [x, y, z] => use_station(game, dim, [*x, *y, *z])
                .unwrap_or_else(|| "no station with track beside it there".into()),
            _ => "usage: create_debug assemble <x> <y> <z>".into(),
        };
        game.native_feedback(horizon_client::text::Text::literal(line));
        return true;
    }
    let Some(rest) = command.strip_prefix("create_debug drive ") else {
        return false;
    };
    let dim = game
        .native_acting_player()
        .and_then(|p| dimension_index(&p.dimension))
        .unwrap_or(0);
    let words: Vec<&str> = rest.split_whitespace().collect();
    let (Some(id), Some(throttle)) = (
        words.first().and_then(|w| w.parse::<u32>().ok()),
        words.get(1).and_then(|w| w.parse::<f64>().ok()),
    ) else {
        game.native_feedback(horizon_client::text::Text::literal(usage));
        return true;
    };
    let steer = match words.get(2).copied() {
        None | Some("straight" | "none" | "0") => 0,
        Some("left" | "1") => 1,
        Some("right" | "-1") => -1,
        Some(_) => {
            game.native_feedback(horizon_client::text::Text::literal(usage));
            return true;
        }
    };
    let throttle = if throttle.is_finite() {
        throttle.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let done = with_world(|w| {
        w.dims[dim]
            .railway
            .drive(
                id,
                Manual {
                    throttle,
                    steer,
                    inverted: false,
                },
            )
            .is_some()
    }) == Some(true);
    let line = if done {
        send_trains(game, dim, &[id], false);
        format!("train {id}: throttle {throttle}, steering {steer}")
    } else {
        format!("no train {id}")
    };
    game.native_feedback(horizon_client::text::Text::literal(line));
    true
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

/// Signal lights for clients: each block, then 0 dark, 1 red, 2 gone.
pub(super) fn send_signal_lights(
    game: &mut Game<'_>,
    players: &[u128],
    lights: &[([i32; 3], Option<bool>)],
) {
    if lights.is_empty() || players.is_empty() {
        return;
    }
    let mut data = Vec::with_capacity(lights.len() * 13);
    for (p, red) in lights {
        for v in p {
            data.extend_from_slice(&v.to_le_bytes());
        }
        data.push(match red {
            Some(false) => 0,
            Some(true) => 1,
            None => 2,
        });
    }
    for &p in players {
        let _ = game.native_net_send_to_player(p, SIGNAL_CHANNEL, &data);
    }
}

/// Every signal light in a dimension, for a player who joins it.
pub(super) fn sync_signal_lights(game: &mut Game<'_>, player: u128, dim: usize) {
    let lights: Vec<([i32; 3], Option<bool>)> = with_world(|w| {
        let mut v: Vec<_> = w.dims[dim]
            .railway
            .lights
            .iter()
            .map(|(p, r)| (*p, Some(*r)))
            .collect();
        v.sort();
        v
    })
    .unwrap_or_default();
    send_signal_lights(game, &[player], &lights);
}

thread_local! {
    /// Scratch for a tick's train events, kept to not allocate.
    static EVENTS: RefCell<Vec<create_engine::trains::Event>> = const { RefCell::new(Vec::new()) };
}

/// The trains (`Train.tick`, `Navigation.tick`, `ScheduleRuntime.tick`) and signals
/// (`SignalBoundary.tickState`): nothing without trains unless track or a signal changed; the
/// graph is built again first if track changed. Clients are told of a train when it starts,
/// stops or arrives, every few ticks on a curve, as it passes onto another edge, and every ten
/// ticks while its speed changes; trains found on the graph again (loaded) are sent whole;
/// signal lights only when one turns red or back.
pub(super) fn tick_trains(game: &mut Game<'_>, dimension: &str, dim: usize) {
    if with_world(|w| {
        let d = &w.dims[dim];
        d.railway.idle(&d.track_curves)
    }) != Some(false)
    {
        return;
    }
    let tick = game.world.game_time.max(0) as u64;
    let mut events = EVENTS.take();
    let mut observed = Vec::new();
    let (due, fresh, lights) = with_world(|w| {
        let d = &mut w.dims[dim];
        d.railway.ensure(&d.track_curves);
        // The stations a train stands at, before and after: a station's comparators read
        // `trainPresent`, a signal's whether it is red.
        let at = |r: &create_engine::trains::Railway| -> Vec<[i32; 3]> {
            r.trains.iter().filter_map(|t| t.train.at_station).collect()
        };
        let before = at(&d.railway);
        d.railway.tick(&mut events);
        let after = at(&d.railway);
        for p in before.iter().filter(|p| !after.contains(p)) {
            d.analog_checks.push(*p);
        }
        for p in after.iter().filter(|p| !before.contains(p)) {
            d.analog_checks.push(*p);
        }
        let lights = std::mem::take(&mut d.railway.lights_changed);
        d.analog_checks
            .extend(lights.iter().filter(|l| l.1.is_some()).map(|l| l.0));
        observed.append(&mut d.railway.observer_changes);
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
        (due, fresh, lights)
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
    // `TrackObserverBlockEntity.tick`: the block's `powered` follows its observer
    // (`setBlock(..., UPDATE_ALL)`).
    for (pos, lit) in observed {
        let state = game.world.block(pos[0], pos[1], pos[2]);
        if observer_powered(state.0).is_some_and(|p| p != lit) {
            game.native_set_block_property_with(
                pos,
                "powered",
                if lit { "true" } else { "false" },
                super::native_hooks::flags::UPDATE_ALL,
            );
        }
    }
    if !fresh.is_empty() {
        send_trains(game, dim, &fresh, true);
    }
    if !due.is_empty() {
        send_trains(game, dim, &due, false);
    }
    if !lights.is_empty() {
        let players = players_in(game, dimension);
        send_signal_lights(game, &players, &lights);
    }
}

/// A track observer's `powered`, None for any other block (a state test; usable from the signal
/// hooks, which may run inside the world's borrow).
pub(super) fn observer_powered(state: u32) -> Option<bool> {
    states::index(state)?;
    WORLD.with(|w| {
        let w = w.try_borrow().ok()?;
        let (b, info) = w.as_ref()?.table.get(state)?;
        (b == create_engine::Block::TrackObserver).then_some(info.powered)
    })
}

/// `TrackObserverBlock.getSignal`: 15 to every side while powered.
pub(super) fn observer_signal(state: u32) -> Option<i32> {
    observer_powered(state).map(|p| if p { 15 } else { 0 })
}

/// A click on a track observer: on its filter slot (`TrackObserverBlockEntity.createFilter`: on
/// top, 15.5 px up) the held item becomes its cargo filter (a filter item taken from the hand,
/// the one it held given back), an empty hand clears it (`onFilterChanged` →
/// `setFilterAndNotify`). Trains carry no cargo in this port: a filtered observer sees none.
pub(super) fn use_observer(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    if face != [0, 1, 0] || !may_use_value_box(game) {
        return;
    }
    let Some(hit) = game.pick() else {
        return;
    };
    let local = [0, 1, 2].map(|i| hit.point[i] - f64::from(pos[i]));
    if !create_engine::board::hits_box([0.5, 15.5 / 16.0, 0.5], local) {
        return;
    }
    if game
        .native_held()
        .is_some_and(|h| matches!(h.0.as_str(), "create:wrench" | "create:mechanical_arm"))
    {
        return;
    }
    *canceled = true;
    let filter = super::create_items::take_filter(game);
    let old = with_world(|w| {
        let d = &mut w.dims[dim];
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        d.railway.set_observer_filtered(pos, filter.is_some());
        match filter {
            Some(f) => d.observer_filters.insert(pos, f),
            None => d.observer_filters.remove(&pos),
        }
    })
    .flatten();
    super::create_items::return_filter(game, pos, old);
}

/// Signal lights on the client (`SignalRenderer`): Create's `indicator_on` partial while a
/// signal is red, `indicator_off` otherwise, as a still part set only when it changes.
pub(super) fn signals_from_server(game: &mut Game<'_>, d: &[u8]) {
    let registry = game.pack.assets.registry;
    let part = |name: &str| {
        let s = registry.default_state(name)?;
        let mut st = registry.state(s)?.clone();
        st.properties.insert("facing".to_owned(), "up".to_owned());
        registry.id(&st)
    };
    let (Some(on), Some(off)) = (
        part("create:part_signal_indicator_on"),
        part("create:part_signal_indicator_off"),
    ) else {
        return;
    };
    for r in d.chunks_exact(13) {
        let i = |k: usize| i32::from_le_bytes([r[k], r[k + 1], r[k + 2], r[k + 3]]);
        let pos = [i(0), i(4), i(8)];
        let key = depot_key(pos, 252, 0);
        let state = match r[12] {
            0 => off,
            1 => on,
            _ => {
                game.native_part_clear(key);
                continue;
            }
        };
        if let Some(model) = game.native_part_model(state) {
            game.native_part_set(key, model, pos, 0, 0.0, 0.0);
        }
    }
}

/// A train a client draws: each carriage's blocks (as assembled, from its leading bogey), the
/// way it faced then, its part models by carriage and pitch (baked when a new pitch is needed),
/// and each bogey's wheels as last set (their axis, rpm and phase), so they turn on smoothly.
struct ClientTrain {
    cars: Vec<Vec<([i32; 3], u32)>>,
    heading: [i32; 2],
    models: Vec<(usize, i32, u32)>,
    bogeys: usize,
    wheels: Vec<Option<(u8, f32, f32)>>,
    /// Each carriage's last pose and when it came (parts seconds), for clicks on it.
    poses: Vec<create_engine::trains::Pose>,
    since: f32,
}

/// `ContraptionHandlerClient.rightClickingOnContraptionsGetsHandledLocally`'s search: the
/// carriage block of the trains this client draws that the segment from `origin` to `target`
/// hits first (`rayTraceContraption`, each carriage posed where it glides now, each block by
/// its outline), with how far along. Nothing to look at without trains.
pub(super) fn client_carriage_hit(
    game: &Game<'_>,
    origin: [f64; 3],
    target: [f64; 3],
) -> Option<(create_engine::train_controls::Target, f64)> {
    use create_engine::train_controls::{Frame, Target, ray_trace};
    if TRAINS.with_borrow(HashMap::is_empty) {
        return None;
    }
    let now = game.native_parts_seconds();
    let facts = game.res.facts();
    TRAINS.with_borrow(|trains| {
        let mut best: Option<(Target, f64)> = None;
        for (&train, t) in trains {
            let dt = f64::from(now - t.since);
            for (c, (pose, blocks)) in t.poses.iter().zip(&t.cars).enumerate() {
                let frame = Frame {
                    pivot: std::array::from_fn(|k| pose.at[k] + f64::from(pose.velocity[k]) * dt),
                    yaw: f64::from(pose.yaw) + f64::from(pose.yaw_rate) * dt,
                    pitch: f64::from(pose.pitch),
                    heading: t.heading,
                };
                let shape = |s: u32| facts.get(BlockStateId(s)).outline.clone();
                if let Some((local, along)) = ray_trace(&frame, blocks, origin, target, shape)
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

thread_local! {
    /// Trains a client draws, by id.
    static TRAINS: RefCell<HashMap<u32, ClientTrain>> = RefCell::new(HashMap::new());
    /// Bogey frames with their wheels (drawn on curves, where the wheels don't turn), by size
    /// and pitch bucket: baked once.
    static BOGEY_MODELS: RefCell<HashMap<(bool, i32, [i32; 2]), u32>> = RefCell::new(HashMap::new());
}

/// Pitch steps a client bakes a train's model at, degrees.
const PITCH_STEP: f32 = 3.0;

/// A train's part keys: carriage `c`'s, and bogey `b`'s frame (`k` 0) and wheels (1, 2).
fn car_key(id: u32, c: usize) -> u64 {
    depot_key([id as i32, i32::MIN, c as i32], 222, 0)
}
fn bogey_key(id: u32, b: usize, k: u8) -> u64 {
    depot_key([id as i32, i32::MIN, b as i32], 250, k)
}

fn clear_client_train(game: &mut Game<'_>, id: u32, old: ClientTrain) {
    for c in 0..old.cars.len() {
        game.native_part_clear(car_key(id, c));
    }
    for b in 0..old.bogeys {
        for k in 0..3 {
            game.native_part_clear(bogey_key(id, b, k));
        }
    }
    for (_, _, model) in old.models {
        game.native_part_model_free(model);
    }
}

/// The part's block and its offset from the block's centre, for a point in the world.
fn placed(at: [f64; 3], now: f32) -> ([i32; 3], [f32; 4]) {
    let block: [i32; 3] = std::array::from_fn(|k| at[k].floor() as i32);
    (
        block,
        [
            (at[0] - f64::from(block[0]) - 0.5) as f32,
            (at[1] - f64::from(block[1]) - 0.5) as f32,
            (at[2] - f64::from(block[2]) - 0.5) as f32,
            now,
        ],
    )
}

/// A train on the client (`CarriageContraptionEntity`'s motion, `CarriageContraptionVisual`):
/// each carriage one part model, tilted to its pitch in the model, turned to its yaw about its
/// pivot (the track under its leading bogey) and gliding at its velocity while turning at its
/// yaw rate, on the GPU until the next message; each bogey its frame (Create's partial for its
/// size) the same way about its anchor, and its wheels (`StandardBogeyVisual`) turning about
/// their axle as they roll while the bogey runs square to the grid (one turning axis per part:
/// on a curve they ride with the frame, still).
pub(super) fn trains_from_server(game: &mut Game<'_>, d: &[u8]) {
    use create_engine::trains::{BogeySize, Motion};
    let m = match Motion::decode(d) {
        Ok(m) => m,
        Err(id) => {
            if let Some(id) = id
                && let Some(old) = TRAINS.with_borrow_mut(|t| t.remove(&id))
            {
                clear_client_train(game, id, old);
            }
            return;
        }
    };
    if let Some(cars) = m.blocks.clone() {
        let old = TRAINS.with_borrow_mut(|t| {
            t.insert(
                m.id,
                ClientTrain {
                    cars,
                    heading: m.heading,
                    models: Vec::new(),
                    bogeys: m.bogeys.len(),
                    wheels: vec![None; m.bogeys.len()],
                    poses: Vec::new(),
                    since: 0.0,
                },
            )
        });
        if let Some(old) = old {
            clear_client_train(game, m.id, old);
        }
    }
    let now = game.native_parts_seconds();
    let Some(heading) = TRAINS.with_borrow_mut(|t| {
        let ct = t.get_mut(&m.id)?;
        ct.poses.clone_from(&m.cars);
        ct.since = now;
        Some(ct.heading)
    }) else {
        return;
    };
    let h = glam::Vec3::new(heading[0] as f32, 0.0, heading[1] as f32);
    let side = h.cross(glam::Vec3::Y).normalize_or(glam::Vec3::X);
    let tilt = |pitch: f32| glam::Mat4::from_axis_angle(side, pitch.to_radians());
    for (c, pose) in m.cars.iter().enumerate() {
        let bucket = (pose.pitch / PITCH_STEP).round() as i32;
        let known = TRAINS.with_borrow(|t| {
            t.get(&m.id).and_then(|ct| {
                ct.models
                    .iter()
                    .find(|(k, b, _)| *k == c && *b == bucket)
                    .map(|x| x.2)
            })
        });
        let model = match known {
            Some(model) => model,
            None => {
                // The model about its pivot at the centre of its part block: each block's corner
                // half a block back and up one from the track, tilted about the train's side.
                let pose_m = glam::Mat4::from_translation(glam::Vec3::splat(0.5))
                    * tilt(bucket as f32 * PITCH_STEP)
                    * glam::Mat4::from_translation(glam::Vec3::splat(-0.5));
                let blocks: Vec<(BlockStateId, glam::Mat4)> = TRAINS.with_borrow(|t| {
                    t.get(&m.id)
                        .and_then(|ct| ct.cars.get(c))
                        .map_or_else(Vec::new, |car| {
                            car.iter()
                                .map(|(o, s)| {
                                    let at = glam::Vec3::new(
                                        o[0] as f32,
                                        o[1] as f32 + 1.5,
                                        o[2] as f32,
                                    );
                                    (BlockStateId(*s), pose_m * glam::Mat4::from_translation(at))
                                })
                                .collect()
                        })
                });
                let Some(model) = game.native_part_model_compound(&blocks) else {
                    continue;
                };
                TRAINS.with_borrow_mut(|t| {
                    if let Some(ct) = t.get_mut(&m.id) {
                        ct.models.push((c, bucket, model));
                    }
                });
                model
            }
        };
        let (block, offset) = placed(pose.at, now);
        // The yaw turns on from `yaw` now at `yaw_rate`: the part's phase is its angle at time 0.
        let phase = (pose.yaw - pose.yaw_rate * now).rem_euclid(360.0);
        game.native_part_glide(
            car_key(m.id, c),
            model,
            block,
            1,
            pose.yaw_rate / 6.0,
            phase,
            offset,
            pose.velocity,
        );
    }
    // Bogeys: the partials as hidden blocks of the content mod, from their unturned state.
    let registry = game.pack.assets.registry;
    let part = |name: &str| {
        let s = registry.default_state(name)?;
        let mut st = registry.state(s)?.clone();
        st.properties.insert("facing".to_owned(), "up".to_owned());
        registry.id(&st)
    };
    let (Some(small_frame), Some(small_wheel), Some(large_frame), Some(large_wheel)) = (
        part("create:part_bogey_frame"),
        part("create:part_bogey_wheel"),
        part("create:part_bogey_drive"),
        part("create:part_bogey_drive_wheel"),
    ) else {
        return;
    };
    // The heading's turn about up: the partials run along z.
    let face = glam::Mat4::from_rotation_y((heading[0] as f32).atan2(heading[1] as f32));
    for (b, (size, pose)) in m.bogeys.iter().enumerate() {
        let large = *size == BogeySize::Large;
        let (frame_state, wheel_state) = if large {
            (large_frame, large_wheel)
        } else {
            (small_frame, small_wheel)
        };
        // `StandardBogeyVisual`: the wheels 12 px up, a block before and behind the anchor; the
        // large bogey's one drive wheel a block up.
        let wheel_at: &[[f32; 3]] = if large {
            &[[0.0, 1.0, 0.0]]
        } else {
            &[[0.0, 0.75, -1.0], [0.0, 0.75, 1.0]]
        };
        let bucket = (pose.pitch / PITCH_STEP).round() as i32;
        let pitch_m = tilt(bucket as f32 * PITCH_STEP);
        let centre = glam::Mat4::from_translation(glam::Vec3::splat(0.5));
        // The bogey's direction now (from the heading turned by its yaw), and whether it runs
        // square to the grid, still turning.
        let absolute = (heading[0] as f32).atan2(heading[1] as f32) + pose.yaw.to_radians();
        let (s, c) = absolute.sin_cos();
        let square = pose.yaw_rate == 0.0 && (s.abs() < 0.01 || c.abs() < 0.01);
        let frame = if square {
            game.native_part_model_posed(frame_state, centre * pitch_m * face)
        } else {
            let cache = (large, bucket, heading);
            match BOGEY_MODELS.with_borrow(|m| m.get(&cache).copied()) {
                Some(model) => Some(model),
                None => {
                    let mut parts = vec![(frame_state, centre * pitch_m * face)];
                    for w in wheel_at {
                        parts.push((
                            wheel_state,
                            centre
                                * pitch_m
                                * face
                                * glam::Mat4::from_translation(glam::Vec3::from_array(*w)),
                        ));
                    }
                    let model = game.native_part_model_compound(&parts);
                    if let Some(model) = model {
                        BOGEY_MODELS.with_borrow_mut(|m| m.insert(cache, model));
                    }
                    model
                }
            }
        };
        let (block, offset) = placed(pose.at, now);
        let phase = (pose.yaw - pose.yaw_rate * now).rem_euclid(360.0);
        if let Some(frame) = frame {
            game.native_part_glide(
                bogey_key(m.id, b, 0),
                frame,
                block,
                1,
                pose.yaw_rate / 6.0,
                phase,
                offset,
                pose.velocity,
            );
        }
        if !square {
            for k in 0..2 {
                game.native_part_clear(bogey_key(m.id, b, k + 1));
            }
            TRAINS.with_borrow_mut(|t| {
                if let Some(w) = t.get_mut(&m.id).and_then(|ct| ct.wheels.get_mut(b)) {
                    *w = None;
                }
            });
            continue;
        }
        // Rolling: about z (axis 2) while running along x, about x (axis 0) along z; positive
        // counterclockwise looking down the axis.
        let v = pose.velocity;
        let radius = size.wheel_radius() as f32;
        let per_rad = 60.0 / std::f32::consts::TAU;
        let (axis, rpm) = if c.abs() < 0.01 {
            (2u8, -v[0] / radius * per_rad)
        } else {
            (0u8, v[2] / radius * per_rad)
        };
        // Square to the grid: the wheels' pose by the quarter it runs along.
        let quarter =
            (absolute / std::f32::consts::FRAC_PI_2).round() * std::f32::consts::FRAC_PI_2;
        let world = glam::Mat4::from_rotation_y(pose.yaw.to_radians()) * pitch_m * face;
        let last = TRAINS.with_borrow(|t| {
            t.get(&m.id)
                .and_then(|ct| ct.wheels.get(b).copied())
                .flatten()
        });
        // Turning on from where they are: the angle now stays.
        let angle_now = match last {
            Some((a, r, p)) if a == axis => p + r * 6.0 * now,
            _ => 0.0,
        };
        let wheel_phase = (angle_now - rpm * 6.0 * now).rem_euclid(360.0);
        TRAINS.with_borrow_mut(|t| {
            if let Some(w) = t.get_mut(&m.id).and_then(|ct| ct.wheels.get_mut(b)) {
                *w = Some((axis, rpm, wheel_phase));
            }
        });
        let Some(wheel) = game
            .native_part_model_posed(wheel_state, centre * glam::Mat4::from_rotation_y(quarter))
        else {
            continue;
        };
        for (k, w) in wheel_at.iter().enumerate() {
            let o = world.transform_vector3(glam::Vec3::from_array(*w));
            let at = [
                pose.at[0] + f64::from(o.x),
                pose.at[1] + f64::from(o.y),
                pose.at[2] + f64::from(o.z),
            ];
            let (block, offset) = placed(at, now);
            game.native_part_glide(
                bogey_key(m.id, b, k as u8 + 1),
                wheel,
                block,
                axis,
                rpm,
                wheel_phase,
                offset,
                pose.velocity,
            );
        }
        if wheel_at.len() == 1 {
            game.native_part_clear(bogey_key(m.id, b, 2));
        }
    }
}

/// Trains as saved (`Train.write`, here with the column their first carriage stands in): their
/// points by the graph's nodes (and where they were), bogeys, carriages and their spacing,
/// motion, destination, schedule, each carriage's blocks and the way it faced when assembled.
pub(super) fn trains_to_nbt(
    game: &Game<'_>,
    all: &[create_engine::world::TrainOnTrack],
) -> horizon_world::nbt::Tag {
    use create_engine::trains::BogeySize;
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
                c.insert(
                    "bogeys".into(),
                    Tag::IntArray(
                        t.train
                            .bogeys
                            .iter()
                            .map(|b| i32::from(*b == BogeySize::Large))
                            .collect(),
                    ),
                );
                c.insert(
                    "carriages".into(),
                    Tag::IntArray(
                        t.train
                            .carriages
                            .iter()
                            .flat_map(|s| [s.first as i32, i32::from(s.two), s.bogey_spacing])
                            .collect(),
                    ),
                );
                c.insert("spacing".into(), Tag::IntArray(t.train.spacing.clone()));
                let nav = t.train.nav.as_ref().map(|n| n.station);
                let mut motion = vec![
                    f(t.train.speed),
                    i32::from(t.train.double_ended),
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
                    "cars".into(),
                    Tag::List(
                        t.cars
                            .iter()
                            .map(|car| {
                                let mut k = Compound::new();
                                k.insert(
                                    "blocks".into(),
                                    Tag::IntArray(car.iter().flat_map(|(o, _)| *o).collect()),
                                );
                                k.insert(
                                    "states".into(),
                                    Tag::List(
                                        car.iter()
                                            .map(|(_, s)| {
                                                Tag::String(
                                                    state_text(registry, *s).as_str().into(),
                                                )
                                            })
                                            .collect(),
                                    ),
                                );
                                Tag::Compound(k)
                            })
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

/// Trains saved with a column (`trains_to_nbt`); saves from before carriages (one block list,
/// no `carriages`) are dropped.
pub(super) fn trains_from_nbt(
    game: &Game<'_>,
    data: Option<&horizon_world::nbt::Compound>,
) -> Vec<create_engine::world::TrainOnTrack> {
    use create_engine::schedule::{Entry, Runtime, Schedule, State};
    use create_engine::trains::{BogeySize, CarriageShape, Nav, Placed, Train, TrainOnTrack};
    use horizon_world::nbt::Tag;
    let registry = game.pack.assets.registry;
    let Some(Tag::List(list)) = data.and_then(|d| d.get(&"trains".into())) else {
        return Vec::new();
    };
    let f = |v: i32| f64::from(f32::from_bits(v as u32));
    let ints = |c: &horizon_world::nbt::Compound, k: &str| match c.get(&k.into()) {
        Some(Tag::IntArray(v)) => Some(v.clone()),
        _ => None,
    };
    list.iter()
        .filter_map(|c| {
            let Tag::Compound(c) = c else { return None };
            let (Some(p), Some(m), Some(b), Some(k), Some(sp), Some(Tag::List(cars))) = (
                ints(c, "points"),
                ints(c, "motion"),
                ints(c, "bogeys"),
                ints(c, "carriages"),
                ints(c, "spacing"),
                c.get(&"cars".into()),
            ) else {
                return None;
            };
            if m.len() < 14 || p.len() < 20 || p.len() != b.len() * 20 {
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
            let carriages: Vec<CarriageShape> = k
                .chunks_exact(3)
                .map(|v| CarriageShape {
                    first: v[0].max(0) as usize,
                    two: v[1] != 0,
                    bogey_spacing: v[2],
                })
                .collect();
            if carriages.is_empty()
                || carriages
                    .iter()
                    .any(|s| s.first + usize::from(s.two) >= b.len())
            {
                return None;
            }
            let cars: Vec<Vec<([i32; 3], u32)>> = cars
                .iter()
                .map(|car| {
                    let Tag::Compound(car) = car else {
                        return Vec::new();
                    };
                    let (Some(Tag::IntArray(o)), Some(Tag::List(s))) =
                        (car.get(&"blocks".into()), car.get(&"states".into()))
                    else {
                        return Vec::new();
                    };
                    o.chunks_exact(3)
                        .zip(s)
                        .filter_map(|(o, s)| {
                            let Tag::String(s) = s else { return None };
                            Some((
                                [o[0], o[1], o[2]],
                                state_of_text(registry, &s.to_string_lossy())?,
                            ))
                        })
                        .collect()
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
                    bogeys: b
                        .iter()
                        .map(|&l| {
                            if l != 0 {
                                BogeySize::Large
                            } else {
                                BogeySize::Small
                            }
                        })
                        .collect(),
                    carriages,
                    spacing: sp,
                    double_ended: m[1] != 0,
                    speed: f(m[0]),
                    nav,
                    at_station: (m[4] != 0).then(|| at(5)),
                    manual: None,
                    reserved: Vec::new(),
                    placed,
                    bound: false,
                },
                cars,
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
    lay_fake_track(game, dimension, dim);
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

/// `TrackBlockEntity.manageFakeTracksAlong` for curves added or gone: fake track (`Track
/// Marker for Maps`) laid in the replaceable blocks above a curve (waterlogged in a water
/// source, never in another fluid), taken up with it. Create also lays it again every lazy tick
/// and lets it decay by random ticks (`FakeTrackBlockEntity.keepAlive`); here it is laid once and
/// taken up when its curve goes, with nothing per tick.
fn lay_fake_track(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let fake = with_world(|w| {
        let f = &mut w.dims[dim].track_curves.fake;
        if f.is_empty() {
            Vec::new()
        } else {
            std::mem::take(f)
        }
    })
    .unwrap_or_default();
    if fake.is_empty() {
        return;
    }
    let registry = game.pack.assets.registry;
    let Some(dry) = registry.default_state("create:fake_track") else {
        return;
    };
    let wet = state_with_props(game, dry, &[("waterlogged", "true")]).unwrap_or(dry);
    let flags = super::native_hooks::flags::UPDATE_ALL;
    for (p, lay) in fake {
        if !game.world.loaded(p[0], p[2]) {
            continue;
        }
        let here = game.world.block(p[0], p[1], p[2]);
        let present = here == dry || here == wet;
        if !lay {
            if present {
                // `removeBlock`: its water stays.
                let left = if here == wet {
                    registry.default_state("minecraft:water")
                } else {
                    registry.default_state("minecraft:air")
                };
                if let Some(s) = left {
                    game.native_set_block(dimension, p, s, flags);
                }
            }
            continue;
        }
        if present {
            continue;
        }
        let b = game.res.behaviours();
        let water = registry.state(here).is_some_and(|s| {
            s.name == "minecraft:water" && s.properties.get("level").is_none_or(|l| l == "0")
        });
        let fluid = b.fluid(here).kind() != horizon_world::fluid::FluidKind::Empty;
        if (fluid && !water) || !b.is_replaceable(here) {
            continue;
        }
        game.native_set_block(dimension, p, if water { wet } else { dry }, flags);
    }
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
