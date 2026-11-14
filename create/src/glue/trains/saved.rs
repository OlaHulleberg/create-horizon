//! What trains, station names and curves keep with their columns, in the legacy layouts
//! (`trains`, `station_names`, `track_curves` of the column's compound).

use create_engine::schedule::{Condition, Entry, Runtime, Schedule, State};
use create_engine::track_curves::Saved;
use create_engine::trains::{
    BogeySize, CarriageShape, Nav, Placed, SavedMotion, Train, TrainOnTrack,
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
pub fn trains_to_nbt(host: &dyn Host, all: &[TrainOnTrack]) -> Tag {
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
                    if !s.source.is_empty() {
                        c.insert("schedule_item".into(), Tag::ByteArray(s.source.clone()));
                    }
                    c.insert(
                        "runtime".into(),
                        Tag::IntArray(vec![
                            i32::from(s.cyclic),
                            t.runtime.current as i32,
                            t.runtime.state as i32,
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
fn conditions_to_ints(groups: &[Vec<Condition>]) -> Vec<i32> {
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

fn conditions_from_ints(v: &[i32]) -> Vec<Vec<Condition>> {
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
            let mut schedule = match c.get(&"schedule".into()) {
                Some(Tag::List(entries)) => Some(Schedule {
                    entries: entries
                        .iter()
                        .filter_map(|e| {
                            let Tag::Compound(e) = e else { return None };
                            let Some(Tag::String(station)) = e.get(&"station".into()) else {
                                return None;
                            };
                            let conditions = ints(e, "conditions")
                                .map_or_else(Vec::new, |v| conditions_from_ints(v));
                            Some(Entry {
                                station: station.to_string_lossy(),
                                conditions,
                            })
                        })
                        .collect(),
                    cyclic: true,
                    source: match c.get(&"schedule_item".into()) {
                        Some(Tag::ByteArray(b)) => b.clone(),
                        _ => Vec::new(),
                    },
                }),
                _ => None,
            };
            let mut runtime = Runtime::new();
            if let (Some(s), Some(r)) = (&mut schedule, ints(c, "runtime"))
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
            // A train in transit without a schedule keeps its destination.
            let nav = m.nav.filter(|_| schedule.is_none()).map(|station| Nav {
                station,
                distance: f64::MAX,
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
            })
        })
        .collect()
}

pub fn station_names_to_nbt(list: &[([i32; 3], String)]) -> Tag {
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

pub fn station_names_from_nbt(data: &Compound) -> Vec<([i32; 3], String)> {
    let Some(Tag::List(list)) = data.get(&"station_names".into()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else { return None };
            let (Some(p), Some(Tag::String(n))) = (ints(c, "pos"), c.get(&"name".into())) else {
                return None;
            };
            let p: [i32; 3] = p.as_slice().try_into().ok()?;
            Some((p, n.to_string_lossy()))
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
            Some(Saved::from_numbers(i, f.try_into().ok()?))
        })
        .collect()
}

pub fn curves_from_nbt(data: &Compound) -> Vec<Saved> {
    curves_of(data.get(&"track_curves".into()))
}

#[cfg(test)]
mod tests {
    use super::{conditions_from_ints, conditions_to_ints, curves_of, curves_to_nbt};
    use create_engine::schedule::Condition;
    use create_engine::track_curves::Saved;

    #[test]
    fn schedule_conditions_keep_their_groups() {
        let groups = vec![
            vec![Condition::Delay(100)],
            vec![Condition::IdleCargo(40), Condition::Delay(20)],
        ];
        assert_eq!(conditions_from_ints(&conditions_to_ints(&groups)), groups);
    }

    #[test]
    fn curves_keep_their_ends_and_numbers() {
        let s = Saved {
            from: [0, 64, 0],
            to: [8, 64, 8],
            starts: [[0.5, 64.0, 1.0], [8.0, 64.0, 8.5]],
            axes: [[0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]],
            normals: [[0.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
        };
        let back = curves_of(Some(&curves_to_nbt(&[s])));
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].numbers(), s.numbers());
    }
}
