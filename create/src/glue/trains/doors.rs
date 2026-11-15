//! Train doors at stations (`SlidingDoorMovementBehaviour` on a carriage, the station's
//! `DoorControlBehaviour`): while a train stands still at a station, the sliding doors on its
//! carriages facing a way the station's door control lets open (all, one side, or none) open,
//! as does a door in the world right in front of one; they close when it leaves. The mode is the
//! station's block entity data (`DoorControl`, Create's key), set from its screen. Doors are
//! looked at every 4 ticks, not every tick as Create's movement behaviour does.
//!
//! A carriage's blocks are vanilla block displays here, so an open door is shown as Create's
//! door once its panels are hidden (`open`, not `visible`), not sliding aside on the way.

use super::super::server::{ServerLevel, update_all};
use super::super::world::with_world;
use create_engine::kinetics::{Axis, Dir};
use mod_api::{Compound, Tag};

/// `DoorControlBehaviour`'s data key.
pub const KEY: &str = "DoorControl";

/// `DoorControl`'s ids in order (`ALL, NORTH, EAST, SOUTH, WEST, NONE`).
pub const MODES: [&str; 6] = ["all", "north", "east", "south", "west", "none"];

/// The station's door control (0: all).
pub fn mode(lv: &ServerLevel<'_>, pos: [i32; 3]) -> u8 {
    let Some(e) = lv.host.block_entity(lv.level, pos) else {
        return 0;
    };
    match e.data(&*lv.host).and_then(|d| d.get(&KEY.into()).cloned()) {
        Some(Tag::Int(m)) => m.rem_euclid(MODES.len() as i32) as u8,
        _ => 0,
    }
}

/// The station's door control set (`StationEditPacket`'s `doorControl`).
pub fn set_mode(lv: &mut ServerLevel<'_>, pos: [i32; 3], mode: u8) {
    let Some(e) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let data = Compound::from([(KEY.into(), Tag::Int(i32::from(mode) % MODES.len() as i32))]);
    let _ = e.merge_data(&mut *lv.host, data);
}

/// `DoorControl.matches` of a door facing `d`.
fn matches(mode: u8, d: Dir) -> bool {
    create_engine::lifts::door_control_matches(mode, d)
}

pub fn is_sliding_door(name: &str) -> bool {
    matches!(
        name,
        "create:andesite_door"
            | "create:brass_door"
            | "create:copper_door"
            | "create:train_door"
            | "create:framed_glass_door"
    )
}

fn dir_named(name: &str) -> Option<Dir> {
    Dir::ALL
        .into_iter()
        .find(|d| super::super::kinetics::dir_name(*d) == name)
}

/// A door to open or close: train, carriage, its block's offset, its state now, the state it
/// takes, where it stands in the world and the way it faces there.
type Change = (u32, usize, [i32; 3], u32, [i32; 3], Dir, bool);

/// Each standing train's doors follow its station's door control; moving trains' close.
pub fn tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let host = &*lv.host;
    // Each train: its id, the station it stands still at, and each carriage's doors (offset,
    // state, open now, local facing) with the carriage's centre, turn and pivot.
    type CarDoors = (
        usize,
        Vec<([i32; 3], u32, bool, Dir)>,
        [f64; 3],
        i32,
        [f64; 3],
    );
    let trains: Vec<(u32, Option<[i32; 3]>, Vec<CarDoors>)> = with_world(|w| {
        let r = &w.dims[dim].railway;
        r.trains
            .iter()
            .filter(|t| t.train.bound)
            .filter_map(|t| {
                let cars: Vec<CarDoors> = t
                    .cars
                    .iter()
                    .enumerate()
                    .filter_map(|(c, car)| {
                        let doors: Vec<([i32; 3], u32, bool, Dir)> = car
                            .iter()
                            .filter_map(|(o, s)| {
                                let st = super::car_state(*s)?;
                                let (name, props) = super::state_parts(host, st)?;
                                if !is_sliding_door(&name) {
                                    return None;
                                }
                                let prop = |k: &str| {
                                    props.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
                                };
                                let facing = prop("facing").and_then(dir_named)?;
                                Some((*o, *s, prop("open") == Some("true"), facing))
                            })
                            .collect();
                        if doors.is_empty() {
                            return None;
                        }
                        // `contraption.bounds.getCenter()`.
                        let mut lo = [f64::MAX; 3];
                        let mut hi = [f64::MIN; 3];
                        for (o, _) in car {
                            for i in 0..3 {
                                lo[i] = lo[i].min(f64::from(o[i]));
                                hi[i] = hi[i].max(f64::from(o[i]) + 1.0);
                            }
                        }
                        let centre = [0, 1, 2].map(|i| (lo[i] + hi[i]) / 2.0);
                        let (pivot, front) = t.train.carriage_pose(&r.net.graph, c);
                        let (yaw, _) = create_engine::trains::yaw_pitch(front, t.heading);
                        let quarter = ((yaw / 90.0).round() as i32 * 90).rem_euclid(360);
                        Some((c, doors, centre, quarter, pivot))
                    })
                    .collect();
                if cars.is_empty() {
                    return None;
                }
                // `canOpen`: standing still (`motion < 1/128`) at a station.
                let still = t.train.speed.abs() < 1.0 / 128.0;
                Some((t.id, t.train.at_station.filter(|_| still), cars))
            })
            .collect()
    })
    .unwrap_or_default();
    let mut changes: Vec<Change> = Vec::new();
    for (id, station, cars) in trains {
        let mode = station.map(|p| mode(lv, p));
        for (c, doors, centre, quarter, pivot) in cars {
            for (o, s, open, facing) in doors {
                // `getDoorFacing`: the positive way along its axis, turned outward from the
                // carriage's centre, then turned with the carriage.
                let axis = facing.axis();
                let mut local = match axis {
                    Axis::X => Dir::East,
                    Axis::Z => Dir::South,
                    Axis::Y => Dir::Up,
                };
                let f = facing.offset();
                let at = [0, 1, 2].map(|i| f64::from(o[i]) + 0.5 - f64::from(f[i]) * 0.45);
                let k = match axis {
                    Axis::X => 0,
                    Axis::Y => 1,
                    Axis::Z => 2,
                };
                if at[k] - centre[k] < 0.0 {
                    local = local.opposite();
                }
                let turned = create_engine::contraption::rotate(local.offset(), quarter, Axis::Y);
                let Some(world) = Dir::ALL.into_iter().find(|d| d.offset() == turned) else {
                    continue;
                };
                let should = mode.is_some_and(|m| matches(m, world));
                if should == open {
                    continue;
                }
                let r = create_engine::contraption::rotate([o[0], 0, o[2]], quarter, Axis::Y);
                let block = [
                    (pivot[0] + f64::from(r[0])).floor() as i32,
                    (pivot[1] + f64::from(o[1]) + 1.0).round() as i32,
                    (pivot[2] + f64::from(r[2])).floor() as i32,
                ];
                changes.push((id, c, o, s, block, world, should));
            }
        }
    }
    if changes.is_empty() {
        return;
    }
    let mut touched: Vec<(u32, usize)> = Vec::new();
    for (id, c, o, s, block, world, open) in changes {
        let Some(st) = super::car_state(s) else {
            continue;
        };
        let (o_, v) = if open {
            ("true", "false")
        } else {
            ("false", "true")
        };
        let Some(next) = super::with_props(&*lv.host, st, &[("open", o_), ("visible", v)]) else {
            continue;
        };
        let next = super::car_id(next);
        let column = with_world(|w| {
            let t = w.dims[dim].railway.trains.iter_mut().find(|t| t.id == id)?;
            if let Some(b) = t.cars.get_mut(c)?.iter_mut().find(|(p, _)| *p == o) {
                b.1 = next;
            }
            Some(t.column)
        })
        .flatten();
        if let Some(col) = column {
            super::super::persist::mark_unsaved(dim, [col.0, col.1]);
        }
        if !touched.contains(&(id, c)) {
            touched.push((id, c));
        }
        // A door in the world in front of it, along the same axis, opens and closes with it.
        let f = world.offset();
        let front = [block[0] + f[0], block[1] + f[1], block[2] + f[2]];
        if let Some(name) = lv.block_name(front)
            && name.ends_with("_door")
            && let Some(state) = lv.block_state(front)
            && super::property(&*lv.host, state, "facing")
                .and_then(|n| dir_named(&n))
                .is_some_and(|d| d.axis() == world.axis())
            && super::property(&*lv.host, state, "open").is_some()
        {
            lv.set_property(
                front,
                "open",
                if open { "true" } else { "false" },
                update_all(),
            );
        }
        if open {
            let p = [0, 1, 2].map(|i| f64::from(block[i]) + 0.5);
            lv.level.play_sound(
                &mut *lv.host,
                "minecraft:block.iron_door.open",
                p,
                mod_api::SoundSource::Blocks,
                0.125,
                1.0,
            );
        }
    }
    for (id, c) in touched {
        super::visuals::refresh_car(lv, id, c);
    }
}
