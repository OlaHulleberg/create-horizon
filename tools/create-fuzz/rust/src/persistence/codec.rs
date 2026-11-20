//! MIT glue codecs over a local NBT value tree.
pub fn belt_segment_to_nbt(saved: create_engine::belt::SavedSegment) -> Compound {
    Compound::from([
        (
            "Controller".into(),
            Tag::IntArray(saved.controller.to_vec()),
        ),
        ("Index".into(), Tag::Int(saved.index)),
        ("Length".into(), Tag::Int(saved.length)),
    ])
}
pub fn belt_segment_from_nbt(data: &Compound) -> Option<create_engine::belt::SavedSegment> {
    let (Some(Tag::IntArray(c)), Some(Tag::Int(index)), Some(Tag::Int(length))) = (
        data.get("Controller"),
        data.get("Index"),
        data.get("Length"),
    ) else {
        return None;
    };
    let [x, y, z] = c.as_slice() else { return None };
    Some(create_engine::belt::SavedSegment {
        controller: [*x, *y, *z],
        index: *index,
        length: *length,
    })
}
pub fn depot_to_nbt(saved: create_engine::depot::SavedDepot) -> Compound {
    let Some(h) = saved.held else {
        return Compound::new();
    };
    let mut held = Compound::from([
        ("Pos".into(), Tag::Float(h.position.to_bits())),
        ("Offset".into(), Tag::Float(h.side_offset.to_bits())),
        ("Angle".into(), Tag::Int(h.angle)),
        (
            "InDirection".into(),
            Tag::Int(h.from.map_or(-1, |s| i32::from(s.index()))),
        ),
        ("Locked".into(), Tag::Byte(i8::from(h.locked))),
    ]);
    if let Some(by) = h.fan.by {
        use create_engine::fan::Kind;
        let id = match by {
            Kind::Splashing => "create:splashing",
            Kind::Haunting => "create:haunting",
            Kind::Smoking => "create:smoking",
            Kind::Blasting => "create:blasting",
        };
        held.insert("FanProcessingType".into(), Tag::String(id.into()));
        held.insert("FanProcessingTime".into(), Tag::Int(h.fan.time));
    }
    Compound::from([("HeldItem".into(), Tag::Compound(held))])
}

pub fn depot_from_nbt(data: &Compound) -> create_engine::depot::SavedDepot {
    let Some(Tag::Compound(h)) = data.get("HeldItem") else {
        return create_engine::depot::SavedDepot { held: None };
    };
    let int = |k: &str| match h.get(k) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    };
    let float = |k: &str| match h.get(k) {
        Some(Tag::Float(v)) => f32::from_bits(*v),
        _ => 0.0,
    };
    create_engine::depot::SavedDepot {
        held: Some(create_engine::depot::Held {
            position: float("Pos"),
            side_offset: float("Offset"),
            angle: int("Angle"),
            from: u8::try_from(int("InDirection"))
                .ok()
                .and_then(create_engine::depot::Side::from_index),
            locked: matches!(h.get("Locked"), Some(Tag::Byte(v)) if *v != 0),
            fan: create_engine::fan::Progress {
                by: match h.get("FanProcessingType") {
                    Some(Tag::String(id)) => {
                        use create_engine::fan::Kind;
                        match id.as_str() {
                            "create:splashing" => Some(Kind::Splashing),
                            "create:haunting" => Some(Kind::Haunting),
                            "create:smoking" => Some(Kind::Smoking),
                            "create:blasting" => Some(Kind::Blasting),
                            _ => None,
                        }
                    }
                    _ => None,
                },
                time: int("FanProcessingTime"),
            },
        }),
    }
}

/// FluidValveBlockEntity.write/read delegates Pointer to LerpedFloat's three float fields.
pub fn valve_pointer_to_nbt(pointer: create_engine::fluids::Pointer) -> Compound {
    Compound::from([
        ("Value".into(), Tag::Float(pointer.value.to_bits())),
        ("Target".into(), Tag::Float(pointer.target.to_bits())),
        ("Speed".into(), Tag::Float(pointer.speed.to_bits())),
    ])
}
pub fn valve_pointer_from_nbt(data: &Compound) -> create_engine::fluids::Pointer {
    let float = |key: &str| match data.get(key) {
        Some(Tag::Float(bits)) => f32::from_bits(*bits),
        _ => 0.0,
    };
    create_engine::fluids::Pointer {
        value: float("Value"),
        target: float("Target"),
        speed: float("Speed"),
    }
}

pub fn handle_to_nbt(h: create_engine::valve::SavedHandle) -> Compound {
    Compound::from([
        ("InUse".into(), Tag::Int(i32::from(h.in_use))),
        ("Backwards".into(), Tag::Byte(i8::from(h.backwards))),
        ("ScrollValue".into(), Tag::Int(h.angle)),
        (
            "TotalUseTicks".into(),
            Tag::Int(i32::from(h.total_use_ticks)),
        ),
        ("StartAngle".into(), Tag::Int(h.start_angle)),
        ("TargetAngle".into(), Tag::Int(h.target_angle)),
    ])
}
pub fn handle_from_nbt(data: &Compound) -> create_engine::valve::SavedHandle {
    let int = |key: &str| match data.get(key) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    };
    create_engine::valve::SavedHandle {
        in_use: int("InUse").clamp(0, 255) as u8,
        backwards: matches!(data.get("Backwards"), Some(Tag::Byte(v)) if *v != 0),
        angle: int("ScrollValue"),
        total_use_ticks: int("TotalUseTicks").clamp(0, 255) as u8,
        start_angle: int("StartAngle"),
        target_angle: int("TargetAngle"),
    }
}

use create_engine::kinetics::{Context, PartLocation, SavedKineticState, SavedNetwork};
use serde::{Deserialize, Serialize};
pub type Compound = std::collections::BTreeMap<String, Tag>;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Tag {
    Byte(i8),
    Int(i32),
    Long(i64),
    Float(u32),
    IntArray(Vec<i32>),
    Compound(Compound),
    List(Vec<Tag>),
    String(String),
}
pub fn saved_to_nbt(saved: SavedKineticState) -> Compound {
    let mut out = Compound::from([
        ("Speed".into(), Tag::Float(saved.speed.to_bits())),
        (
            "NeedsSpeedUpdate".into(),
            Tag::Byte(i8::from(saved.needs_speed_update)),
        ),
    ]);
    if let Some(source) = saved.source {
        out.insert("Source".into(), Tag::IntArray(source.pos.to_vec()));
        out.insert("SourcePort".into(), Tag::Byte(source.sub as i8));
    }
    if let Some(n) = saved.network {
        out.insert(
            "Network".into(),
            Tag::Compound(Compound::from([
                ("Id".into(), Tag::Long(n.id)),
                ("Stress".into(), Tag::Float(n.stress.to_bits())),
                ("Capacity".into(), Tag::Float(n.capacity.to_bits())),
                ("Size".into(), Tag::Int(n.size as i32)),
                ("AddedStress".into(), Tag::Float(n.added_stress.to_bits())),
                (
                    "AddedCapacity".into(),
                    Tag::Float(n.added_capacity.to_bits()),
                ),
            ])),
        );
    }
    if let Some((context, relative)) = saved.sequence {
        out.insert(
            "Sequence".into(),
            Tag::Compound(Compound::from([
                ("Distance".into(), Tag::Byte(i8::from(context.distance))),
                ("Value".into(), Tag::Float(context.value.to_bits())),
                ("Run".into(), Tag::Long(context.run as i64)),
                ("RelativeValue".into(), Tag::Float(relative.to_bits())),
            ])),
        );
    }
    out
}
/// KineticBlockEntity.read; no saved data means a freshly placed entity or an older save.
pub fn saved_from_nbt(data: &Compound) -> Option<SavedKineticState> {
    fn float(c: &Compound, k: &str) -> f32 {
        match c.get(k) {
            Some(Tag::Float(v)) => f32::from_bits(*v),
            _ => 0.0,
        }
    }
    fn int(c: &Compound, k: &str) -> i32 {
        match c.get(k) {
            Some(Tag::Int(v)) => *v,
            _ => 0,
        }
    }
    let Some(Tag::Float(speed)) = data.get("Speed") else {
        return None;
    };
    let source = match data.get("Source") {
        Some(Tag::IntArray(p)) if p.len() == 3 => Some(PartLocation {
            pos: [p[0], p[1], p[2]],
            sub: match data.get("SourcePort") {
                Some(Tag::Byte(1)) => 1,
                _ => 0,
            },
        }),
        _ => None,
    };
    let network = match data.get("Network") {
        Some(Tag::Compound(n)) => match n.get("Id") {
            Some(Tag::Long(id)) => Some(SavedNetwork {
                id: *id,
                stress: float(n, "Stress"),
                capacity: float(n, "Capacity"),
                size: int(n, "Size").max(0) as u32,
                added_stress: float(n, "AddedStress"),
                added_capacity: float(n, "AddedCapacity"),
            }),
            _ => None,
        },
        _ => None,
    };
    let sequence = match data.get("Sequence") {
        Some(Tag::Compound(c)) => Some((
            Context {
                distance: matches!(c.get("Distance"), Some(Tag::Byte(1))),
                value: float(c, "Value"),
                run: match c.get("Run") {
                    Some(Tag::Long(v)) => *v as u32,
                    _ => 0,
                },
            },
            float(c, "RelativeValue"),
        )),
        _ => None,
    };
    Some(SavedKineticState {
        speed: f32::from_bits(*speed),
        source,
        network,
        sequence,
        needs_speed_update: matches!(data.get("NeedsSpeedUpdate"), Some(Tag::Byte(1))),
    })
}

pub fn belts_to_nbt(belts: &[([i32; 3], create_engine::belt::Inventory)]) -> Tag {
    Tag::List(
        belts
            .iter()
            .map(|(c, inv)| {
                let mut b = Compound::new();
                b.insert("pos".into(), Tag::IntArray(c.to_vec()));
                let numbers = inv
                    .items
                    .iter()
                    .flat_map(|i| {
                        [
                            i.position.to_bits() as i32,
                            i.side_offset.to_bits() as i32,
                            i.angle,
                            i32::from(i.count),
                            i32::from(i.locked),
                        ]
                    })
                    .collect();
                b.insert("items".into(), Tag::IntArray(numbers));
                b.insert(
                    "ids".into(),
                    Tag::List(
                        inv.items
                            .iter()
                            .map(|i| {
                                let id = if i.count == 0 {
                                    "minecraft:air"
                                } else {
                                    i.item.as_str()
                                };
                                Tag::String(id.into())
                            })
                            .collect(),
                    ),
                );
                b.insert(
                    "positive".into(),
                    Tag::IntArray(vec![i32::from(inv.positive)]),
                );
                b.insert(
                    "insertion".into(),
                    Tag::IntArray(
                        inv.items
                            .iter()
                            .flat_map(|i| [i.inserted_at, i.inserted_from.map_or(6, |d| d as i32)])
                            .collect(),
                    ),
                );
                // Items in a sequenced assembly: their index and step, and the recipe.
                let assembly: Vec<Tag> = inv
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(n, i)| {
                        let p = i.progress.as_ref()?;
                        let mut a = Compound::new();
                        a.insert("at".into(), Tag::IntArray(vec![n as i32, p.step as i32]));
                        a.insert("id".into(), Tag::String(p.id.as_str().into()));
                        Some(Tag::Compound(a))
                    })
                    .collect();
                if !assembly.is_empty() {
                    b.insert("assembly".into(), Tag::List(assembly));
                }
                Tag::Compound(b)
            })
            .collect(),
    )
}

/// What `belts_to_nbt` wrote.
pub fn belts_from_nbt(data: &Compound) -> Vec<([i32; 3], create_engine::belt::Inventory)> {
    let Some(Tag::List(list)) = data.get("belts") else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|b| {
            let Tag::Compound(b) = b else { return None };
            let Some(Tag::IntArray(pos)) = b.get("pos") else {
                return None;
            };
            let Some(Tag::IntArray(numbers)) = b.get("items") else {
                return None;
            };
            let Some(Tag::List(ids)) = b.get("ids") else {
                return None;
            };
            let positive =
                matches!(b.get("positive"), Some(Tag::IntArray(v)) if v.first() == Some(&1));
            let mut items: Vec<create_engine::belt::Item> = numbers
                .as_chunks::<5>()
                .0
                .iter()
                .zip(ids)
                .filter_map(|(n, id)| {
                    let Tag::String(id) = id else { return None };
                    let mut item =
                        create_engine::belt::Item::from_saved(id.as_str(), n[3], n[2], false);
                    item.position = f32::from_bits(n[0] as u32);
                    item.side_offset = f32::from_bits(n[1] as u32);
                    item.locked = n[4] != 0;
                    Some(item)
                })
                .collect();
            if let Some(Tag::IntArray(context)) = b.get("insertion") {
                for (item, pair) in items.iter_mut().zip(context.as_chunks::<2>().0) {
                    item.inserted_at = pair[0];
                    item.inserted_from = usize::try_from(pair[1])
                        .ok()
                        .and_then(|n| create_engine::kinetics::Dir::ALL.get(n))
                        .copied();
                }
            }
            if let Some(Tag::List(assembly)) = b.get("assembly") {
                for a in assembly {
                    let Tag::Compound(a) = a else { continue };
                    let (Some(Tag::IntArray(at)), Some(Tag::String(id))) =
                        (a.get("at"), a.get("id"))
                    else {
                        continue;
                    };
                    let (Some(n), Some(step)) = (at.first(), at.get(1)) else {
                        continue;
                    };
                    if let Some(item) = items.get_mut(*n as usize) {
                        item.progress = Some(create_engine::sequenced::Progress {
                            id: id.to_string(),
                            step: (*step).max(0) as u32,
                        });
                    }
                }
            }
            let c = [*pos.first()?, *pos.get(1)?, *pos.get(2)?];
            Some((
                c,
                create_engine::belt::Inventory::from_saved(items, positive),
            ))
        })
        .collect()
}

use create_engine::sequencer::{self as seq, Instruction, Kind, Speed};
const INSTRUCTIONS: &str = "Instructions";
pub fn to_tag(list: &[Instruction]) -> Tag {
    Tag::List(
        list.iter()
            .map(|i| {
                Tag::Compound(Compound::from([
                    ("Type".into(), Tag::String(i.kind.name().into())),
                    ("Modifier".into(), Tag::String(i.speed.name().into())),
                    ("Value".into(), Tag::Int(i.value)),
                ]))
            })
            .collect(),
    )
}

/// `Instruction.deserializeAll`: none read gives none (the default list then).
pub fn of_tag(data: &Compound) -> Vec<Instruction> {
    let Some(Tag::List(l)) = data.get(INSTRUCTIONS) else {
        return Vec::new();
    };
    l.iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else {
                return None;
            };
            let text = |k: &str| match c.get(k) {
                Some(Tag::String(s)) => s.to_string(),
                _ => String::new(),
            };
            let value = match c.get("Value") {
                Some(Tag::Int(v)) => *v,
                _ => 0,
            };
            Some(Instruction {
                kind: Kind::of_name(&text("Type")),
                speed: Speed::of_name(&text("Modifier")),
                value,
            })
        })
        .take(seq::ROWS)
        .collect()
}

const RUN_KEYS: [&str; 5] = [
    "InstructionIndex",
    "InstructionDuration",
    "InstructionProgress",
    "Timer",
    "PrevPowered",
];

pub fn run_of_tag(data: &Compound) -> Option<seq::Run> {
    let int = |k: &str| match data.get(k) {
        Some(Tag::Int(v)) => Some(*v),
        _ => None,
    };
    let progress = match data.get(RUN_KEYS[2]) {
        Some(Tag::Float(bits)) => f32::from_bits(*bits),
        _ => 0.0,
    };
    Some(seq::Run {
        current: int(RUN_KEYS[0])?,
        duration: int(RUN_KEYS[1]).unwrap_or(-1),
        progress,
        timer: int(RUN_KEYS[3]).unwrap_or(0),
        powered_before: matches!(data.get(RUN_KEYS[4]), Some(Tag::Byte(b)) if *b != 0),
    })
}

pub fn sequencer_to_nbt(s: &seq::Sequencer) -> Compound {
    Compound::from([
        ("Instructions".into(), to_tag(&s.instructions)),
        ("InstructionIndex".into(), Tag::Int(s.current)),
        ("InstructionDuration".into(), Tag::Int(s.duration)),
        (
            "InstructionProgress".into(),
            Tag::Float(s.progress.to_bits()),
        ),
        ("Timer".into(), Tag::Int(s.timer)),
        ("PrevPowered".into(), Tag::Byte(i8::from(s.powered_before))),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kinetic_round_trip_preserves_ports_sequences_and_float_bits() {
        let saved = SavedKineticState {
            speed: -32.,
            needs_speed_update: true,
            source: Some(PartLocation {
                pos: [-2, 8, 4],
                sub: 1,
            }),
            network: Some(SavedNetwork {
                id: 123,
                stress: 24.,
                capacity: 64.,
                size: 3,
                added_stress: 2.,
                added_capacity: 8.,
            }),
            sequence: Some((
                Context {
                    distance: true,
                    value: 12.,
                    run: 42,
                },
                0.5,
            )),
        };
        let bytes = serde_json::to_vec(&saved_to_nbt(saved)).unwrap();
        let data: Compound = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved_from_nbt(&data), Some(saved));
    }
    #[test]
    fn belt_reload_preserves_insertion_context() {
        let mut inv = create_engine::belt::Inventory::default();
        let mut item = create_engine::belt::Item::new("minecraft:iron_ingot", 4, 180, false);
        item.position = 1.25;
        item.locked = true;
        item.inserted_at = 2;
        item.inserted_from = Some(create_engine::kinetics::Dir::Up);
        inv.add(item);
        let tag = Compound::from([("belts".into(), belts_to_nbt(&[([0, 8, 0], inv.to_save())]))]);
        let saved = belts_from_nbt(&tag);
        assert_eq!(saved[0].1.items[0].position, 1.25);
        assert!(saved[0].1.items[0].locked);
        assert_eq!(saved[0].1.items[0].count, 4);
        assert_eq!(saved[0].1.items[0].inserted_at, 2);
        assert_eq!(
            saved[0].1.items[0].inserted_from,
            Some(create_engine::kinetics::Dir::Up)
        );
    }
}

#[cfg(test)]
mod handle_tests {
    use super::*;
    #[test]
    fn handle_round_trip_preserves_six_fields_without_cooldown() {
        let saved = create_engine::valve::SavedHandle {
            in_use: 6,
            backwards: true,
            angle: -45,
            total_use_ticks: 7,
            start_angle: 13,
            target_angle: -90,
        };
        let tag = handle_to_nbt(saved);
        assert_eq!(tag.len(), 6);
        assert!(!tag.contains_key("Cooldown"));
        let bytes = serde_json::to_vec(&tag).unwrap();
        assert_eq!(
            handle_from_nbt(&serde_json::from_slice(&bytes).unwrap()),
            saved
        );
    }
}

#[cfg(test)]
mod depot_tests {
    use super::*;
    #[test]
    fn depot_record_preserves_transport_and_fan_state_and_clears_empty() {
        use create_engine::{
            depot::{Held, SavedDepot, Side},
            fan::{Kind, Progress},
        };
        for by in [
            None,
            Some(Kind::Splashing),
            Some(Kind::Haunting),
            Some(Kind::Smoking),
            Some(Kind::Blasting),
        ] {
            for side in [None, Some(Side::West)] {
                let mut h = Held::put_by_player(73, false, Side::West);
                h.position = 0.48592162;
                h.side_offset = -0.0;
                h.from = side;
                h.locked = true;
                h.fan = Progress {
                    by,
                    time: if by.is_some() { 37 } else { 0 },
                };
                let saved = SavedDepot { held: Some(h) };
                let bytes = serde_json::to_vec(&depot_to_nbt(saved)).unwrap();
                let restored = depot_from_nbt(&serde_json::from_slice(&bytes).unwrap());
                assert_eq!(restored, saved);
                assert_eq!(
                    restored.held.unwrap().side_offset.to_bits(),
                    h.side_offset.to_bits()
                );
            }
        }
        let empty = depot_to_nbt(SavedDepot { held: None });
        assert!(empty.is_empty());
        assert_eq!(depot_from_nbt(&empty).held, None);
    }
}

#[cfg(test)]
mod belt_membership_tests {
    use super::*;
    #[test]
    fn segment_membership_round_trips_and_requires_typed_fields() {
        let saved = create_engine::belt::SavedSegment {
            controller: [-2, 8, 4],
            index: 2,
            length: 5,
        };
        let bytes = serde_json::to_vec(&belt_segment_to_nbt(saved)).unwrap();
        assert_eq!(
            belt_segment_from_nbt(&serde_json::from_slice(&bytes).unwrap()),
            Some(saved)
        );
        assert_eq!(belt_segment_from_nbt(&Compound::new()), None);
    }
    #[test]
    fn optional_insertion_pairs_preserve_all_directions_and_legacy_is_readable() {
        for direction in create_engine::kinetics::Dir::ALL
            .into_iter()
            .map(Some)
            .chain(std::iter::once(None))
        {
            let mut item = create_engine::belt::Item::new("minecraft:iron_ingot", 1, 180, false);
            item.inserted_at = 3;
            item.inserted_from = direction;
            let inv = create_engine::belt::Inventory::from_saved(vec![item], true);
            let mut data = Compound::from([("belts".into(), belts_to_nbt(&[([0, 8, 0], inv)]))]);
            let bytes = serde_json::to_vec(&data).unwrap();
            let saved = belts_from_nbt(&serde_json::from_slice(&bytes).unwrap());
            assert_eq!(saved[0].1.items[0].inserted_from, direction);
            assert_eq!(saved[0].1.items[0].inserted_at, 3);
            if let Some(Tag::List(list)) = data.get_mut("belts")
                && let Tag::Compound(c) = &mut list[0]
            {
                c.remove("insertion");
            }
            assert_eq!(belts_from_nbt(&data)[0].1.items[0].inserted_at, 0);
        }
    }
}

/// glue/logistics/mod.rs `brass_to_nbt`: brass tunnels' saved state (`SavedBrass`).
pub fn brass_to_nbt(brass: &[([i32; 3], create_engine::tunnel::SavedBrass)]) -> Tag {
    Tag::List(
        brass
            .iter()
            .map(|(p, b)| {
                let mut c = Compound::from([
                    ("Pos".into(), Tag::IntArray(p.to_vec())),
                    ("SelectionMode".into(), Tag::Int(b.mode.index() as i32)),
                    ("DistributionProgress".into(), Tag::Int(b.progress)),
                    ("PreviousIndex".into(), Tag::Int(b.previous_output as i32)),
                    ("SyncedOutput".into(), Tag::Byte(i8::from(b.synced))),
                    (
                        "ConnectedLeft".into(),
                        Tag::Byte(i8::from(b.connected_left)),
                    ),
                    (
                        "ConnectedRight".into(),
                        Tag::Byte(i8::from(b.connected_right)),
                    ),
                ]);
                if let Some(d) = b.entered_from {
                    c.insert("StackEnteredFrom".into(), Tag::Int(d as i32));
                }
                if let Some((item, n)) = &b.stack {
                    c.insert("Item".into(), Tag::String(item.clone()));
                    c.insert("Count".into(), Tag::Int(i32::from(*n)));
                }
                Tag::Compound(c)
            })
            .collect(),
    )
}

/// What `brass_to_nbt` wrote (`brass_from_nbt`), plus the legacy packed-int format.
pub fn brass_from_nbt(data: &Compound) -> Vec<([i32; 3], create_engine::tunnel::SavedBrass)> {
    match data.get("brass") {
        Some(Tag::List(list)) => list
            .iter()
            .filter_map(|tag| {
                let Tag::Compound(c) = tag else { return None };
                let Some(Tag::IntArray(pos)) = c.get("Pos") else {
                    return None;
                };
                let p = [*pos.first()?, *pos.get(1)?, *pos.get(2)?];
                let int = |k: &str, default: i32| match c.get(k) {
                    Some(Tag::Int(v)) => *v,
                    _ => default,
                };
                let flag = |k: &str| matches!(c.get(k), Some(Tag::Byte(v)) if *v != 0);
                let item = match c.get("Item") {
                    Some(Tag::String(s)) => s.clone(),
                    _ => String::new(),
                };
                let n = int("Count", 0).clamp(0, 64) as u8;
                let entered_from = match c.get("StackEnteredFrom") {
                    Some(Tag::Int(v)) => usize::try_from(*v)
                        .ok()
                        .and_then(|i| create_engine::kinetics::Dir::ALL.get(i))
                        .copied(),
                    _ => None,
                };
                let mode_idx = int("SelectionMode", 0).clamp(0, 6) as usize;
                Some((
                    p,
                    create_engine::tunnel::SavedBrass {
                        stack: (!item.is_empty() && n > 0).then_some((item, n)),
                        entered_from,
                        progress: int("DistributionProgress", -1),
                        previous_output: int("PreviousIndex", 0).max(0) as usize,
                        mode: create_engine::tunnel::Mode::ALL[mode_idx],
                        synced: flag("SyncedOutput"),
                        connected_left: flag("ConnectedLeft"),
                        connected_right: flag("ConnectedRight"),
                    },
                ))
            })
            .collect(),
        Some(Tag::Compound(c)) => {
            let (Some(Tag::IntArray(nums)), Some(Tag::List(items))) =
                (c.get("nums"), c.get("items"))
            else {
                return Vec::new();
            };
            nums.as_chunks::<6>()
                .0
                .iter()
                .zip(items)
                .map(|(v, item)| {
                    let item = match item {
                        Tag::String(s) => s.to_string(),
                        _ => String::new(),
                    };
                    let n = v[5].clamp(0, 64) as u8;
                    (
                        [v[0], v[1], v[2]],
                        create_engine::tunnel::SavedBrass {
                            stack: (!item.is_empty() && n > 0).then_some((item, n)),
                            entered_from: create_engine::kinetics::Dir::ALL
                                .get(v[4].clamp(0, 6) as usize)
                                .copied(),
                            mode: create_engine::tunnel::Mode::ALL[v[3].clamp(0, 6) as usize],
                            ..Default::default()
                        },
                    )
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod brass_tests {
    use super::*;

    #[test]
    fn brass_record_preserves_index_progress_and_synced() {
        let saved = create_engine::tunnel::SavedBrass {
            stack: Some(("minecraft:gold_ingot".into(), 2)),
            entered_from: Some(create_engine::kinetics::Dir::South),
            progress: 7,
            previous_output: 1,
            mode: create_engine::tunnel::Mode::RoundRobin,
            synced: true,
            connected_left: true,
            connected_right: false,
        };
        let data = Compound::from([("brass".into(), brass_to_nbt(&[([0, 9, 2], saved.clone())]))]);
        assert_eq!(brass_from_nbt(&data), vec![([0, 9, 2], saved)]);
    }

    #[test]
    fn emptied_belt_stack_round_trips_empty() {
        let mut item = create_engine::belt::Item::new("minecraft:iron_ingot", 0, 180, false);
        item.position = 1.9708341;
        let inv = create_engine::belt::Inventory::from_saved(vec![item], true);
        let data = Compound::from([("belts".into(), belts_to_nbt(&[([0, 8, 0], inv)]))]);
        let saved = belts_from_nbt(&data);
        assert_eq!(saved[0].1.items[0].item, "minecraft:air");
        assert_eq!(saved[0].1.items[0].count, 0);
    }
}
