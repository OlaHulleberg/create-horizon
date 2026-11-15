//! What trains, station names and curves keep with their columns
//! (`trains`, `station_names`, `track_curves` of the column's compound).

use create_engine::schedule::{
    Condition, Entry, Instruction, Op, Runtime, Schedule, Stack, State, TimeUnit,
};
use create_engine::track_curves::Saved;
use create_engine::trains::{
    BogeySize, CarriageShape, Nav, Placed, SavedMotion, SavedStation, Train, TrainOnTrack,
};
use mod_api::{Compound, Host, Tag};

fn ints<'a>(c: &'a Compound, k: &str) -> Option<&'a Vec<i32>> {
    match c.get(&k.into()) {
        Some(Tag::IntArray(v)) => Some(v),
        _ => None,
    }
}

/// Trains as saved (`Train.write`, here with the column their first carriage stands in): their
/// points by the graph's nodes (and where they were), bogeys, carriages and their spacing,
/// motion, destination, schedule, each carriage's blocks and the way it faced when assembled.
pub fn trains_to_nbt(
    host: &dyn Host,
    all: &[TrainOnTrack],
    riders: &dyn Fn(u32) -> Vec<(u16, [i32; 3], u128)>,
    cargo: &dyn Fn(&TrainOnTrack) -> Option<Tag>,
) -> Tag {
    let f = |v: f64| (v as f32).to_bits() as i32;
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
                let motion = SavedMotion {
                    speed: t.train.speed,
                    double_ended: t.train.double_ended,
                    heading: t.heading,
                    at_station: t.train.at_station,
                    nav: t.train.nav.as_ref().map(|n| n.station),
                    behind: t.train.nav.as_ref().is_some_and(|n| n.behind),
                    throttle: t.train.throttle,
                };
                c.insert("motion".into(), Tag::IntArray(motion.encode()));
                // `Derailed`.
                if t.train.derailed {
                    c.insert("derailed".into(), Tag::Byte(1));
                }
                if !t.name.is_empty() {
                    c.insert("name".into(), Tag::String(t.name.as_str().into()));
                }
                c.insert("icon".into(), Tag::Int(i32::from(t.icon)));
                // Who sits on its seats: carriage, seat, UUID as four ints.
                let seated: Vec<i32> = riders(t.id)
                    .into_iter()
                    .flat_map(|(car, l, u)| {
                        [
                            i32::from(car),
                            l[0],
                            l[1],
                            l[2],
                            (u >> 96) as i32,
                            (u >> 64) as i32,
                            (u >> 32) as i32,
                            u as i32,
                        ]
                    })
                    .collect();
                if !seated.is_empty() {
                    c.insert("riders".into(), Tag::IntArray(seated));
                }
                // What its carriages' storage blocks hold (`cargo`).
                if let Some(cargo) = cargo(t) {
                    c.insert("cargo".into(), cargo);
                }
                c.insert(
                    "cars".into(),
                    Tag::List(
                        t.cars
                            .iter()
                            .map(|car| {
                                // A block whose state this side can't name is left out.
                                let named: Vec<([i32; 3], String)> = car
                                    .iter()
                                    .filter_map(|(o, s)| Some((*o, super::car_text(host, *s)?)))
                                    .collect();
                                let mut k = Compound::new();
                                k.insert(
                                    "blocks".into(),
                                    Tag::IntArray(named.iter().flat_map(|(o, _)| *o).collect()),
                                );
                                k.insert(
                                    "states".into(),
                                    Tag::List(
                                        named
                                            .iter()
                                            .map(|(_, s)| Tag::String(s.as_str().into()))
                                            .collect(),
                                    ),
                                );
                                Tag::Compound(k)
                            })
                            .collect(),
                    ),
                );
                if let Some(s) = &t.schedule {
                    c.insert("schedule2".into(), Tag::Compound(schedule_to_nbt(s)));
                    c.insert(
                        "runtime".into(),
                        Tag::IntArray(vec![
                            i32::from(t.runtime.auto),
                            t.runtime.current as i32,
                            t.runtime.state as i32,
                            i32::from(t.runtime.paused),
                        ]),
                    );
                    if !t.runtime.title.is_empty() {
                        c.insert("title".into(), Tag::String(t.runtime.title.as_str().into()));
                    }
                    // `TransitTimes`.
                    c.insert(
                        "transit".into(),
                        Tag::IntArray(t.runtime.predictions.clone()),
                    );
                }
                Tag::Compound(c)
            })
            .collect(),
    )
}

/// A ghost slot's stack as Create saves it (`ItemStack.saveOptional`: an empty compound for
/// none), from the bytes the engine keeps (the stack's network NBT).
fn stack_tag(s: &Stack) -> Tag {
    if s.is_empty() {
        return Tag::Compound(Compound::new());
    }
    match Tag::from_network_bytes(s, None) {
        Ok((t @ Tag::Compound(_), _)) => t,
        _ => Tag::Compound(Compound::new()),
    }
}

/// The bytes the engine keeps for a saved stack (empty for an empty compound).
fn stack_of_tag(t: Option<&Tag>) -> Stack {
    match t {
        Some(t @ Tag::Compound(c)) if !c.is_empty() => t.to_network_bytes().unwrap_or_default(),
        _ => Stack::new(),
    }
}

/// A schedule as Create writes it (`Schedule.write`): `Entries`, each with its `Instruction`
/// and `Conditions` (groups of `{Id, Data}`), `Cyclic` and `Progress`; each part's `Data` keyed
/// as Create keys it.
pub fn schedule_to_nbt(s: &Schedule) -> Compound {
    let part = |id: &str, data: Compound| {
        let mut c = Compound::new();
        c.insert(
            "Id".into(),
            Tag::String(format!("create:{id}").as_str().into()),
        );
        c.insert("Data".into(), Tag::Compound(data));
        Tag::Compound(c)
    };
    let mut entries = Vec::new();
    for e in &s.entries {
        let mut data = Compound::new();
        match &e.instruction {
            Instruction::Destination(t)
            | Instruction::Rename(t)
            | Instruction::FetchPackages(t) => {
                data.insert("Text".into(), Tag::String(t.as_str().into()));
            }
            Instruction::Throttle(v) => {
                data.insert("Value".into(), Tag::Int(*v));
            }
            Instruction::DeliverPackages => {}
        }
        let mut entry = Compound::new();
        entry.insert("Instruction".into(), part(e.instruction.id(), data));
        let groups = e
            .conditions
            .iter()
            .map(|g| {
                Tag::List(
                    g.iter()
                        .map(|c| {
                            let mut d = Compound::new();
                            let mut int = |k: &str, v: i32| {
                                d.insert(k.into(), Tag::Int(v));
                            };
                            match c {
                                Condition::Delay { value, unit }
                                | Condition::Idle { value, unit } => {
                                    int("Value", *value);
                                    int("TimeUnit", unit.ordinal());
                                }
                                Condition::TimeOfDay {
                                    hour,
                                    minute,
                                    rotation,
                                } => {
                                    int("Hour", *hour);
                                    int("Minute", *minute);
                                    int("Rotation", *rotation);
                                }
                                Condition::PlayerCount { count, exact } => {
                                    int("Count", *count);
                                    int("Exact", i32::from(*exact));
                                }
                                // `CargoThresholdCondition`: the operator, the threshold as
                                // text, the measure; the item (`Item`) or bucket (`Bucket`).
                                Condition::ItemThreshold {
                                    op,
                                    threshold,
                                    stacks,
                                    item,
                                } => {
                                    int("Operator", op.ordinal());
                                    int("Measure", i32::from(*stacks));
                                    d.insert(
                                        "Threshold".into(),
                                        Tag::String(threshold.to_string().as_str().into()),
                                    );
                                    d.insert("Item".into(), stack_tag(item));
                                }
                                Condition::FluidThreshold {
                                    op,
                                    threshold,
                                    item,
                                } => {
                                    int("Operator", op.ordinal());
                                    int("Measure", 0);
                                    d.insert(
                                        "Threshold".into(),
                                        Tag::String(threshold.to_string().as_str().into()),
                                    );
                                    d.insert("Bucket".into(), stack_tag(item));
                                }
                                // `RedstoneLinkCondition`: `Inverted`, `Frequency` (both stacks).
                                Condition::RedstoneLink { freq, inverted } => {
                                    int("Inverted", i32::from(*inverted));
                                    d.insert(
                                        "Frequency".into(),
                                        Tag::List(freq.iter().map(stack_tag).collect()),
                                    );
                                }
                                Condition::Unloaded | Condition::Powered => {}
                            }
                            part(c.id(), d)
                        })
                        .collect(),
                )
            })
            .collect();
        entry.insert("Conditions".into(), Tag::List(groups));
        entries.push(Tag::Compound(entry));
    }
    let mut c = Compound::new();
    c.insert("Entries".into(), Tag::List(entries));
    c.insert("Cyclic".into(), Tag::Byte(i8::from(s.cyclic)));
    c.insert("Progress".into(), Tag::Int(s.saved_progress as i32));
    c
}

/// A schedule as [`schedule_to_nbt`] wrote it; parts of a type the port doesn't run are left
/// out.
pub fn schedule_from_nbt(c: &Compound) -> Schedule {
    let int = |c: &Compound, k: &str| match c.get(&k.into()) {
        Some(Tag::Int(i)) => *i,
        Some(Tag::Byte(b)) => i32::from(*b),
        _ => 0,
    };
    let text = |c: &Compound, k: &str| match c.get(&k.into()) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    let part = |t: &Tag| -> Option<(String, Compound)> {
        let Tag::Compound(p) = t else { return None };
        let id = text(p, "Id");
        let id = id.strip_prefix("create:").unwrap_or(&id).to_owned();
        let data = match p.get(&"Data".into()) {
            Some(Tag::Compound(d)) => d.clone(),
            _ => Compound::new(),
        };
        Some((id, data))
    };
    let mut entries = Vec::new();
    if let Some(Tag::List(list)) = c.get(&"Entries".into()) {
        for e in list {
            let Tag::Compound(e) = e else { continue };
            let Some((id, d)) = e.get(&"Instruction".into()).and_then(part) else {
                continue;
            };
            let instruction = match id.as_str() {
                "destination" => Instruction::Destination(text(&d, "Text")),
                "package_delivery" => Instruction::DeliverPackages,
                "package_retrieval" => Instruction::FetchPackages(text(&d, "Text")),
                "rename" => Instruction::Rename(text(&d, "Text")),
                "throttle" => Instruction::Throttle(int(&d, "Value").clamp(5, 100)),
                _ => continue,
            };
            let mut conditions = Vec::new();
            if let Some(Tag::List(groups)) = e.get(&"Conditions".into()) {
                for g in groups {
                    let Tag::List(g) = g else { continue };
                    let group: Vec<Condition> = g
                        .iter()
                        .filter_map(part)
                        .filter_map(|(id, d)| {
                            let unit = TimeUnit::from_ordinal(int(&d, "TimeUnit"));
                            Some(match id.as_str() {
                                "delay" => Condition::Delay {
                                    value: int(&d, "Value"),
                                    unit,
                                },
                                "idle" => Condition::Idle {
                                    value: int(&d, "Value"),
                                    unit,
                                },
                                "time_of_day" => Condition::TimeOfDay {
                                    hour: int(&d, "Hour"),
                                    minute: int(&d, "Minute"),
                                    rotation: int(&d, "Rotation"),
                                },
                                "player_count" => Condition::PlayerCount {
                                    count: int(&d, "Count"),
                                    exact: int(&d, "Exact") != 0,
                                },
                                "item_threshold" => Condition::ItemThreshold {
                                    op: Op::from_ordinal(int(&d, "Operator") % 3),
                                    threshold: text(&d, "Threshold").trim().parse().unwrap_or(0),
                                    stacks: int(&d, "Measure") == 1,
                                    item: stack_of_tag(d.get(&"Item".into())),
                                },
                                "fluid_threshold" => Condition::FluidThreshold {
                                    op: Op::from_ordinal(int(&d, "Operator") % 3),
                                    threshold: text(&d, "Threshold").trim().parse().unwrap_or(0),
                                    item: stack_of_tag(d.get(&"Bucket".into())),
                                },
                                "redstone_link" => {
                                    let mut freq = [Stack::new(), Stack::new()];
                                    if let Some(Tag::List(l)) = d.get(&"Frequency".into()) {
                                        for (f, t) in freq.iter_mut().zip(l) {
                                            *f = stack_of_tag(Some(t));
                                        }
                                    }
                                    Condition::RedstoneLink {
                                        freq,
                                        inverted: int(&d, "Inverted") == 1,
                                    }
                                }
                                "unloaded" => Condition::Unloaded,
                                "powered" => Condition::Powered,
                                _ => return None,
                            })
                        })
                        .collect();
                    if !group.is_empty() {
                        conditions.push(group);
                    }
                }
            }
            entries.push(Entry {
                instruction,
                conditions,
            });
        }
    }
    Schedule {
        saved_progress: int(c, "Progress").max(0) as usize,
        cyclic: int(c, "Cyclic") != 0,
        entries,
    }
}

/// The riders saved with each train of the column, in the order of `trains_from_nbt`.
pub fn riders_from_nbt(data: &Compound) -> Vec<Vec<(u16, [i32; 3], u128)>> {
    let Some(Tag::List(list)) = data.get(&"trains".into()) else {
        return Vec::new();
    };
    list.iter()
        .filter(|t| matches!(t, Tag::Compound(c) if c.contains_key(&"carriages".into())))
        .map(|t| {
            let Tag::Compound(c) = t else {
                return Vec::new();
            };
            ints(c, "riders").map_or_else(Vec::new, |v| {
                v.as_chunks::<8>()
                    .0
                    .iter()
                    .map(|r| {
                        let u = (u128::from(r[4] as u32) << 96)
                            | (u128::from(r[5] as u32) << 64)
                            | (u128::from(r[6] as u32) << 32)
                            | u128::from(r[7] as u32);
                        (r[0] as u16, [r[1], r[2], r[3]], u)
                    })
                    .collect()
            })
        })
        .collect()
}

/// The cargo saved with each train of the column, in the order of `trains_from_nbt`.
pub fn cargo_from_nbt(data: &Compound) -> Vec<Option<Tag>> {
    let Some(Tag::List(list)) = data.get(&"trains".into()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| match t {
            Tag::Compound(c) if c.contains_key(&"carriages".into()) => {
                Some(c.get(&"cargo".into()).cloned())
            }
            _ => None,
        })
        .collect()
}

/// Trains saved with a column (`trains_to_nbt`); saves from before carriages (one block list,
/// no `carriages`) are dropped.
pub fn trains_from_nbt(host: &dyn Host, data: &Compound) -> Vec<TrainOnTrack> {
    let Some(Tag::List(list)) = data.get(&"trains".into()) else {
        return Vec::new();
    };
    let f = |v: i32| f64::from(f32::from_bits(v as u32));
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
            let m = SavedMotion::decode(m)?;
            if p.len() < 20 || p.len() != b.len() * 20 {
                return None;
            }
            let placed: Vec<Placed> = p
                .as_chunks::<10>()
                .0
                .iter()
                .map(|v| Placed {
                    from: [v[0], v[1], v[2]],
                    to: [v[3], v[4], v[5]],
                    position: f(v[6]),
                    at: [f(v[7]), f(v[8]), f(v[9])],
                })
                .collect();
            let carriages: Vec<CarriageShape> = k
                .as_chunks::<3>()
                .0
                .iter()
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
                    o.as_chunks::<3>()
                        .0
                        .iter()
                        .zip(s)
                        .filter_map(|(o, s)| {
                            let Tag::String(s) = s else { return None };
                            Some((
                                [o[0], o[1], o[2]],
                                super::car_of_text(host, &s.to_string_lossy())?,
                            ))
                        })
                        .collect()
                })
                .collect();
            let schedule = match c.get(&"schedule2".into()) {
                Some(Tag::Compound(s)) => Some(schedule_from_nbt(s)),
                _ => None,
            };
            let mut runtime = Runtime::new();
            if let Some(Tag::String(t)) = c.get(&"title".into()) {
                runtime.title = t.to_string_lossy();
            }
            if let (Some(s), Some(r)) = (&schedule, ints(c, "runtime"))
                && r.len() >= 4
            {
                runtime.auto = r[0] != 0;
                runtime.current = r[1].max(0) as usize;
                // In transit it looks for its way again; waiting, it waits anew.
                runtime.state = match r[2] {
                    2 => State::PostTransit,
                    _ => State::PreTransit,
                };
                runtime.paused = r[3] != 0;
                // `TransitTimes`, kept when there is one an entry.
                runtime.predictions = vec![create_engine::schedule::TBD; s.entries.len()];
                if let Some(t) = ints(c, "transit")
                    && t.len() == s.entries.len()
                {
                    runtime.predictions.clone_from(t);
                }
                if runtime.state == State::PostTransit {
                    runtime.progress = s
                        .entries
                        .get(runtime.current)
                        .map_or_else(Vec::new, |e| vec![(0, 0); e.conditions.len()]);
                }
            }
            // A train in transit without a schedule keeps its destination.
            let nav = m.nav.filter(|_| schedule.is_none()).map(|station| Nav {
                station,
                distance: f64::MAX,
                started_at: 0.0,
                path: Vec::new(),
                behind: m.behind,
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
                    spacing: sp.clone(),
                    double_ended: m.double_ended,
                    speed: m.speed,
                    nav,
                    at_station: m.at_station,
                    manual: None,
                    throttle: m.throttle,
                    reserved: Vec::new(),
                    placed,
                    bound: false,
                    stalled: false,
                    speed_before_stall: None,
                    derailed: matches!(c.get(&"derailed".into()), Some(Tag::Byte(1))),
                    backwards: false,
                },
                cars,
                heading: m.heading,
                schedule,
                runtime,
                idle: 0,
                column: (0, 0),
                sent_tick: 0,
                sent_edge: usize::MAX,
                fresh: true,
                name: match c.get(&"name".into()) {
                    Some(Tag::String(n)) => n.to_string_lossy(),
                    _ => String::new(),
                },
                icon: match c.get(&"icon".into()) {
                    Some(Tag::Int(i)) => (*i).clamp(0, 2) as u8,
                    _ => 0,
                },
                players: 0,
                probes: Default::default(),
            })
        })
        .collect()
}

/// The column's stations: position, name, the assembly direction while in assembly mode
/// (`assembling`) and the name of the train last taken apart there (`prev_train`).
pub fn stations_to_nbt(list: &[SavedStation]) -> Tag {
    Tag::List(
        list.iter()
            .map(|s| {
                let mut c = Compound::new();
                c.insert("pos".into(), Tag::IntArray(s.pos.to_vec()));
                c.insert("name".into(), Tag::String(s.name.as_str().into()));
                if let Some(d) = s.assembling {
                    c.insert("assembling".into(), Tag::IntArray(d.to_vec()));
                }
                if let Some(n) = &s.prev_train {
                    c.insert("prev_train".into(), Tag::String(n.as_str().into()));
                }
                if let Some(a) = &s.auto_schedule {
                    c.insert("auto_schedule".into(), Tag::Compound(schedule_to_nbt(a)));
                }
                Tag::Compound(c)
            })
            .collect(),
    )
}

pub fn stations_from_nbt(data: &Compound) -> Vec<SavedStation> {
    let Some(Tag::List(list)) = data.get(&"station_names".into()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else { return None };
            let (Some(p), Some(Tag::String(n))) = (ints(c, "pos"), c.get(&"name".into())) else {
                return None;
            };
            let pos: [i32; 3] = p.as_slice().try_into().ok()?;
            Some(SavedStation {
                pos,
                name: n.to_string_lossy(),
                assembling: ints(c, "assembling").and_then(|d| d.as_slice().try_into().ok()),
                prev_train: match c.get(&"prev_train".into()) {
                    Some(Tag::String(n)) => Some(n.to_string_lossy()),
                    _ => None,
                },
                auto_schedule: match c.get(&"auto_schedule".into()) {
                    Some(Tag::Compound(a)) => Some(schedule_from_nbt(a)),
                    _ => None,
                },
            })
        })
        .collect()
}

/// A column's curves as saved (`TrackBlockEntity.write`'s `Connections`): ends and numbers.
pub fn curves_to_nbt(list: &[Saved]) -> Tag {
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
                if s.girder {
                    c.insert("Girder".into(), Tag::Byte(1));
                }
                Tag::Compound(c)
            })
            .collect(),
    )
}

/// Curves in `tag` (`curves_to_nbt`'s list).
pub fn curves_of(tag: Option<&Tag>) -> Vec<Saved> {
    let Some(Tag::List(list)) = tag else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else { return None };
            let i: [i32; 6] = ints(c, "ends")?.as_slice().try_into().ok()?;
            let Some(Tag::List(f)) = c.get(&"curve".into()) else {
                return None;
            };
            let f: Vec<f64> = f
                .iter()
                .filter_map(|v| match v {
                    Tag::Double(b) => Some(f64::from_bits(*b)),
                    _ => None,
                })
                .collect();
            let mut s = Saved::from_numbers(i, f.try_into().ok()?);
            s.girder = matches!(c.get(&"Girder".into()), Some(Tag::Byte(1)));
            Some(s)
        })
        .collect()
}

pub fn curves_from_nbt(data: &Compound) -> Vec<Saved> {
    curves_of(data.get(&"track_curves".into()))
}

#[cfg(test)]
mod tests {
    use super::{curves_of, curves_to_nbt, schedule_from_nbt, schedule_to_nbt};
    use create_engine::schedule::{Condition, Entry, Instruction, Schedule, TimeUnit};
    use create_engine::track_curves::Saved;

    #[test]
    fn schedules_keep_their_entries_and_conditions() {
        let s = Schedule {
            entries: vec![
                Entry::station(
                    "Mine*",
                    vec![
                        vec![Condition::Delay {
                            value: 5,
                            unit: TimeUnit::Seconds,
                        }],
                        vec![
                            Condition::Idle {
                                value: 2,
                                unit: TimeUnit::Minutes,
                            },
                            Condition::TimeOfDay {
                                hour: 13,
                                minute: 30,
                                rotation: 2,
                            },
                            Condition::PlayerCount {
                                count: 3,
                                exact: true,
                            },
                            Condition::Powered,
                        ],
                    ],
                ),
                Entry {
                    instruction: Instruction::Throttle(40),
                    conditions: Vec::new(),
                },
                Entry {
                    instruction: Instruction::Rename("Express".into()),
                    conditions: Vec::new(),
                },
            ],
            cyclic: true,
            saved_progress: 1,
        };
        assert_eq!(schedule_from_nbt(&schedule_to_nbt(&s)), s);
    }

    #[test]
    fn curves_keep_their_ends_and_numbers() {
        let s = Saved {
            from: [0, 64, 0],
            to: [8, 64, 8],
            starts: [[0.5, 64.0, 1.0], [8.0, 64.0, 8.5]],
            axes: [[0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]],
            normals: [[0.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
            girder: true,
        };
        let back = curves_of(Some(&curves_to_nbt(&[s])));
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].numbers(), s.numbers());
        assert!(back[0].girder);
    }
}
