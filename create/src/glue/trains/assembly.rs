//! A station's assembly (`StationBlockEntity.assemble` / `disassemble`): a click on the station
//! takes a train standing at it apart, else builds one from the bogeys along its track.

use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::super::world::with_world;
use super::railway::{railway_ready, send_train_clear, send_trains};
use super::structure::{found, turned_state};
use create_engine::Block;
use create_engine::kinetics::{Axis, Dir};
use create_engine::schedule::Runtime;
use create_engine::trains::{
    AssemblyError, BogeySize, MAX_ASSEMBLY_LENGTH, MAX_BOGEY_COUNT, Structure, Train, TrainOnTrack,
    layout,
};
use mod_api::{BlockPart, InteractionResult, PlayerRef, ServerHost, Text, UseWithoutItemEvent};
use std::collections::HashMap;

/// The station block's part: its click.
pub struct Station;

#[mod_api::horizon_part]
impl BlockPart for Station {
    fn use_without_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseWithoutItemEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let dimension = click.dimension.key().id().to_owned();
        let player = host.player(click.player);
        let mut lv = ServerLevel::new(host, &dimension)?;
        use_station(&mut lv, click.pos, player);
        Some(InteractionResult::SuccessServer)
    }
}

/// The size of the bogey whose state is engine id `state`, if it is one.
fn bogey_size(state: u32) -> Option<BogeySize> {
    match with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()? {
        Block::SmallBogey => Some(BogeySize::Small),
        Block::LargeBogey => Some(BogeySize::Large),
        _ => None,
    }
}

/// Tells the acting player why the station can't assemble (`AssemblyException`, shown on the
/// station's screen in Create).
fn assembly_failed(lv: &mut ServerLevel<'_>, player: Option<PlayerRef>, e: AssemblyError) {
    let key = format!("create.train_assembly.{}", e.key());
    let args = e.args().into_iter().map(Text::literal).collect();
    if let Some(p) = player {
        super::action_bar(lv.host, p, Text::translate(key, args));
    }
}

/// The station's assembly: a train standing at it goes back into the world, each carriage
/// where it stands (turned to its quarter; `canDisassemble`: only level and square to the
/// grid); else the bogeys along its track and what each carries become carriages of a train on
/// the track graph, by Create's rules. The station's track runs on toward its assembly
/// direction: the way whose first track block has a bogey on it (Create's frontmost bogey at
/// the station; the direction Create takes from the click that targets the track).
pub fn use_station(lv: &mut ServerLevel<'_>, pos: [i32; 3], player: Option<PlayerRef>) {
    let dim = lv.dim;
    railway_ready(dim);
    // A train assembled or taken apart here: its comparators read `trainPresent` again.
    crate::glue::redstone::analog_touched(dim, pos);
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
        // Not square to the grid: it stays a train.
        Some(Err(())) => return,
        Some(Ok((t, poses))) => {
            for (car, (pivot, front)) in t.cars.iter().zip(poses) {
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
                    let Some(state) = super::car_state(*s) else {
                        continue;
                    };
                    let state = turned_state(&*lv.host, state, quarter, Axis::Y);
                    lv.set_state(p, state, update_all());
                }
            }
            super::super::persist::mark_unsaved(dim, [t.column.0, t.column.1]);
            super::controls::disassembled(lv, t.id);
            super::riding::train_gone(lv, t.id);
            send_train_clear(lv, t.id);
            return;
        }
        None => {}
    }
    // The straight track the station looks at, and which way along it the train is.
    let Some((target, shape)) = with_world(|w| {
        let r = &w.dims[dim].railway;
        let target = create_engine::network::target_track(&r.tracks, pos)?;
        Some((target, r.tracks[&target]))
    })
    .flatten() else {
        return;
    };
    let along = if shape == create_engine::track::TrackShape::Xo {
        [1, 0, 0]
    } else {
        [0, 0, 1]
    };
    let bogey_at = |lv: &ServerLevel<'_>, p: [i32; 3]| bogey_size(lv.block([p[0], p[1] + 1, p[2]]));
    let direction = [1, -1]
        .into_iter()
        .map(|s| [along[0] * s, 0, along[2] * s])
        .find(|d| bogey_at(lv, [target[0] + d[0], target[1], target[2] + d[2]]).is_some())
        .unwrap_or(along);
    // `refreshAssemblyInfo`: the track on from the station's, while it runs the same way; a
    // bogey above it at each place counts.
    let mut run = Vec::new();
    let mut bogeys: Vec<(i32, [i32; 3], BogeySize)> = Vec::new();
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
        if bogeys.len() < MAX_BOGEY_COUNT
            && let Some(size) = bogey_at(lv, p)
        {
            bogeys.push((i, [p[0], p[1] + 1, p[2]], size));
        }
    }
    let locations: Vec<i32> = bogeys.iter().map(|b| b.0).collect();
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
    let controls = states::key(Block::Controls);
    let laid = layout(&locations, |i| {
        let b = bogeys[i].1;
        let Ok(blocks) = create_engine::piston::gather(b, Dir::Down, &run, |p| found(lv, p)) else {
            refused = true;
            return Structure::default();
        };
        let mut s = Structure {
            blocks: blocks.len(),
            ..Structure::default()
        };
        for (o, state) in &blocks {
            let p = [b[0] + o[0], b[1] + o[1], b[2] + o[2]];
            if let Some(k) = bogeys.iter().find(|f| f.1 == p) {
                s.bogeys.push(k.0);
            }
            // `CarriageContraption.capture`: train controls by their facing.
            if let Some(st) = super::car_state(*state)
                && st.block() == controls
                && let Some(f) = super::property(&*lv.host, st, "facing")
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
        // Something on it can't move: nothing is assembled.
        Ok(_) if refused => return,
        Ok(l) => l,
        Err(e) => return assembly_failed(lv, player, e),
    };
    let sizes: Vec<_> = bogeys.iter().map(|b| b.2).collect();
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
    let origin_block = bogeys.first().map_or(target, |f| f.1);
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
        return assembly_failed(lv, player, AssemblyError::NoBogeys);
    };
    let Some(air) = super::default_state(&*lv.host, "minecraft:air") else {
        return;
    };
    for (c, car) in laid.carriages.iter().zip(&cars) {
        let b = bogeys[c.first].1;
        for (o, _) in car {
            let p = [b[0] + o[0], b[1] + o[1], b[2] + o[2]];
            lv.set_state(p, air, update_all());
            super::super::persist::mark_pos_unsaved(dim, p);
        }
    }
    send_trains(lv, &[id], true);
}
