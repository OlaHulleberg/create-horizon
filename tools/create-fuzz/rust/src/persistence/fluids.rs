//! Fluid column codecs from MIT glue/fluids/{mod,plumbing,machines,pipes}.rs.
//! Called only by the marked-column flush and chunk-load phases, never at unload.
use super::codec::{Compound, Tag};
use crate::scenario::Pos;
use create_engine::kinetics::Dir;
use create_engine::tank::{Codes, Fluid, MAX_HEIGHT, MAX_WIDTH, Multi, Tank};
use create_engine::world::CreateWorld;

fn ints<'a>(data: &'a Compound, key: &str) -> &'a [i32] {
    match data.get(key) {
        Some(Tag::IntArray(v)) => v,
        _ => &[],
    }
}
fn keys(data: &Compound, key: &str) -> Vec<String> {
    match data.get(&format!("{key}_fluids")) {
        Some(Tag::List(v)) => v
            .iter()
            .map(|t| match t {
                Tag::String(s) => s.clone(),
                _ => String::new(),
            })
            .collect(),
        _ => vec![],
    }
}
fn put_keys(data: &mut Compound, key: &str, codes: Codes) {
    if !codes.keys.is_empty() {
        data.insert(
            format!("{key}_fluids"),
            Tag::List(codes.keys.into_iter().map(Tag::String).collect()),
        );
    }
}
pub fn shown_tank(m: Option<Multi>) -> Compound {
    let mut codes = Codes::default();
    let v = m.map_or_else(Vec::new, |m| {
        vec![
            codes.code(m.tank.fluid),
            m.tank.amount as i32,
            i32::from(m.width),
            i32::from(m.height),
            i32::from(m.window),
        ]
    });
    Compound::from([
        ("Tank".into(), Tag::IntArray(v)),
        (
            "Tank_fluids".into(),
            Tag::List(codes.keys.into_iter().map(Tag::String).collect()),
        ),
    ])
}
pub fn save_column(w: &CreateWorld, column: (i32, i32)) -> Compound {
    let here = |p: &Pos| (p[0] >> 4, p[2] >> 4) == column;
    let mut data = Compound::new();
    let tanks = w.column_tanks(0, column.0, column.1);
    let mut codes = Codes::default();
    let v: Vec<_> = tanks
        .iter()
        .flat_map(|(p, saved)| {
            let m = saved.multi.unwrap_or_default();
            [
                p[0],
                p[1],
                p[2],
                codes.code(m.tank.fluid),
                m.tank.amount as i32,
                i32::from(m.width),
                i32::from(m.height),
                i32::from(m.window),
                saved.part.controller[0],
                saved.part.controller[1],
                saved.part.controller[2],
                i32::from(saved.part.window),
                i32::from(saved.part.update_connectivity),
                i32::from(saved.multi.is_some()),
            ]
        })
        .collect();
    if !v.is_empty() {
        data.insert("tank_entities".into(), Tag::IntArray(v));
        put_keys(&mut data, "tank_entities", codes);
    }
    let d = &w.dims[0];
    let p = &d.plumbing;
    let mut filters: Vec<_> = p.filters.iter().filter(|(p, _)| here(p)).collect();
    filters.sort_by_key(|(p, _)| **p);
    let mut hoses: Vec<_> = p
        .hoses
        .iter()
        .filter(|(p, _)| here(p))
        .map(|(p, h)| (*p, h.offset, h.tank))
        .chain(
            p.hose_saved
                .iter()
                .filter(|(p, _)| here(p))
                .map(|(p, (o, t))| (*p, *o, *t)),
        )
        .collect();
    hoses.sort_by_key(|v| v.0);
    let mut creative: Vec<_> = p
        .creative
        .iter()
        .chain(p.creative_saved.iter())
        .filter(|(p, _)| here(p))
        .collect();
    creative.sort_by_key(|(p, _)| **p);
    // Match the glue's guard, including its treatment of a column with only pending multis.
    if !filters.is_empty() || !hoses.is_empty() || !creative.is_empty() {
        let mut own = Compound::new();
        let mut codes = Codes::default();
        own.insert(
            "filters".into(),
            Tag::IntArray(
                filters
                    .iter()
                    .flat_map(|(p, f)| [p[0], p[1], p[2], codes.code(f.fluid)])
                    .collect(),
            ),
        );
        own.insert(
            "filter_items".into(),
            Tag::List(
                filters
                    .iter()
                    .map(|(_, f)| Tag::String(f.item.clone()))
                    .collect(),
            ),
        );
        own.insert(
            "hoses".into(),
            Tag::IntArray(
                hoses
                    .iter()
                    .flat_map(|(p, o, t)| {
                        [
                            p[0],
                            p[1],
                            p[2],
                            (o * 1000.) as i32,
                            codes.code(t.fluid),
                            t.amount as i32,
                        ]
                    })
                    .collect(),
            ),
        );
        own.insert(
            "creative_tanks".into(),
            Tag::IntArray(
                creative
                    .iter()
                    .flat_map(|(p, f)| [p[0], p[1], p[2], codes.code(**f)])
                    .collect(),
            ),
        );
        own.insert(
            "creative_multis".into(),
            Tag::IntArray(
                w.column_creative_tanks(0, column.0, column.1)
                    .iter()
                    .flat_map(|(p, m)| {
                        [
                            p[0],
                            p[1],
                            p[2],
                            i32::from(m.width),
                            i32::from(m.height),
                            i32::from(m.window),
                        ]
                    })
                    .collect(),
            ),
        );
        put_keys(&mut own, "create_fluids", codes);
        data.insert("create_fluids".into(), Tag::Compound(own));
    }
    let mut spouts: Vec<_> = d.spouts.iter().filter(|(p, _)| here(p)).collect();
    spouts.sort_by_key(|(p, _)| **p);
    let mut drains: Vec<_> = d.drains.iter().filter(|(p, _)| here(p)).collect();
    drains.sort_by_key(|(p, _)| **p);
    if !spouts.is_empty() || !drains.is_empty() {
        let mut codes = Codes::default();
        let mut tank = |p: &Pos, f: Option<(Fluid, u32)>| {
            [
                p[0],
                p[1],
                p[2],
                codes.code(f.map(|v| v.0)),
                f.map_or(0, |v| v.1 as i32),
            ]
        };
        let spouts = spouts.iter().flat_map(|(p, s)| tank(p, s.fluid)).collect();
        let fluids = drains.iter().flat_map(|(p, s)| tank(p, s.fluid)).collect();
        let items = drains
            .iter()
            .filter_map(|(p, d)| {
                let h = d.held.as_ref()?;
                Some(Tag::Compound(Compound::from([
                    ("Pos".into(), Tag::IntArray(p.to_vec())),
                    ("Item".into(), Tag::String(h.item.clone())),
                    ("Count".into(), Tag::Int(i32::from(h.count))),
                    ("BeltPosition".into(), Tag::Float(h.position.to_bits())),
                    ("From".into(), Tag::Int(h.from as i32)),
                    ("ProcessingTicks".into(), Tag::Int(d.processing)),
                ])))
            })
            .collect();
        let mut own = Compound::from([
            ("spouts".into(), Tag::IntArray(spouts)),
            ("drains".into(), Tag::IntArray(fluids)),
            ("drain_items".into(), Tag::List(items)),
        ]);
        put_keys(&mut own, "create_machines", codes);
        data.insert("create_machines".into(), Tag::Compound(own));
    }
    let records = d
        .fluid_transport
        .pipes
        .iter()
        .filter(|(p, _)| here(p))
        .map(|(p, b)| {
            let mut data = super::connections::encode(b);
            data.insert("Pos".into(), Tag::IntArray(p.to_vec()));
            Tag::Compound(data)
        })
        .collect();
    data.insert("Connections".into(), Tag::List(records));
    data
}

pub fn load_column(w: &mut CreateWorld, data: &Compound) {
    let k = keys(data, "tank_multis");
    let tank = |c: &[i32]| Tank {
        fluid: Codes::fluid(&k, c[3]),
        amount: c[4].max(0) as u32,
    };
    let saved: Vec<_> = ints(data, "tank_multis")
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| {
            (
                [c[0], c[1], c[2]],
                Multi {
                    tank: tank(c),
                    width: c[5].clamp(1, MAX_WIDTH) as u8,
                    height: c[6].clamp(1, MAX_HEIGHT) as u8,
                    window: c[7] != 0,
                },
            )
        })
        .chain(ints(data, "tanks").as_chunks::<5>().0.iter().map(|c| {
            (
                [c[0], c[1], c[2]],
                Multi {
                    tank: tank(c),
                    ..Default::default()
                },
            )
        }))
        .collect();
    let saved = if data.contains_key("tank_entities") {
        let keys = keys(data, "tank_entities");
        ints(data, "tank_entities")
            .as_chunks::<14>()
            .0
            .iter()
            .map(|c| {
                (
                    [c[0], c[1], c[2]],
                    create_engine::tank::SavedTank {
                        part: create_engine::tank::Part {
                            controller: [c[8], c[9], c[10]],
                            window: c[11] != 0,
                            update_connectivity: c[12] != 0,
                        },
                        multi: (c[13] != 0).then_some(Multi {
                            tank: Tank {
                                fluid: Codes::fluid(&keys, c[3]),
                                amount: c[4].max(0) as u32,
                            },
                            width: c[5] as u8,
                            height: c[6] as u8,
                            window: c[7] != 0,
                        }),
                    },
                )
            })
            .collect::<Vec<_>>()
    } else {
        saved
            .into_iter()
            .map(|(p, m)| (p, create_engine::tank::SavedTank::controller(p, m)))
            .collect()
    };
    w.load_tanks(0, saved.into_iter());
    if let Some(Tag::Compound(own)) = data.get("create_fluids") {
        let k = keys(own, "create_fluids");
        let p = &mut w.dims[0].plumbing;
        let items = match own.get("filter_items") {
            Some(Tag::List(v)) => v.as_slice(),
            _ => &[],
        };
        for (i, c) in ints(own, "filters").as_chunks::<4>().0.iter().enumerate() {
            let fluid = Codes::fluid(&k, c[3]);
            let item = match items.get(i) {
                Some(Tag::String(s)) => s.clone(),
                Some(_) => String::new(),
                None => fluid.and_then(Fluid::bucket).unwrap_or_default().into(),
            };
            let pos = [c[0], c[1], c[2]];
            p.filters
                .insert(pos, create_engine::fluids::SmartFilter { item, fluid });
            p.filter_changes.push(pos);
        }
        for c in ints(own, "hoses").as_chunks::<6>().0 {
            p.hose_saved.insert(
                [c[0], c[1], c[2]],
                (
                    c[3].max(0) as f32 / 1000.,
                    Tank {
                        fluid: Codes::fluid(&k, c[4]),
                        amount: c[5].clamp(0, create_engine::hose::CAPACITY as i32) as u32,
                    },
                ),
            );
        }
        for c in ints(own, "creative_tanks").as_chunks::<4>().0 {
            p.creative_saved
                .insert([c[0], c[1], c[2]], Codes::fluid(&k, c[3]));
        }
        w.load_creative_tanks(
            0,
            ints(own, "creative_multis")
                .as_chunks::<6>()
                .0
                .iter()
                .map(|c| {
                    (
                        [c[0], c[1], c[2]],
                        Multi {
                            width: c[3].clamp(1, MAX_WIDTH) as u8,
                            height: c[4].clamp(1, MAX_HEIGHT) as u8,
                            window: c[5] != 0,
                            tank: Tank::default(),
                        },
                    )
                }),
        );
    }
    if let Some(Tag::Compound(own)) = data.get("create_machines") {
        let k = keys(own, "create_machines");
        let fluid = |c: &[i32]| Codes::fluid(&k, c[3]).map(|f| (f, c[4].max(0) as u32));
        let d = &mut w.dims[0];
        for c in ints(own, "spouts").as_chunks::<5>().0 {
            d.spouts.entry([c[0], c[1], c[2]]).or_default().fluid = fluid(c);
        }
        for c in ints(own, "drains").as_chunks::<5>().0 {
            d.drains.entry([c[0], c[1], c[2]]).or_default().fluid = fluid(c);
        }
        if let Some(Tag::List(items)) = own.get("drain_items") {
            for t in items {
                let Tag::Compound(c) = t else { continue };
                let (p, Some(Tag::String(item))) = (ints(c, "Pos"), c.get("Item")) else {
                    continue;
                };
                let &[x, y, z] = p else { continue };
                let int = |key: &str| match c.get(key) {
                    Some(Tag::Int(i)) => *i,
                    _ => 0,
                };
                let position = match c.get("BeltPosition") {
                    Some(Tag::Float(v)) => f32::from_bits(*v),
                    _ => 0.,
                };
                let dr = d.drains.entry([x, y, z]).or_default();
                dr.processing = int("ProcessingTicks");
                dr.held = Some(create_engine::drain::Held {
                    item: item.clone(),
                    count: int("Count").clamp(1, 64) as u8,
                    position,
                    prev: position,
                    from: Dir::ALL[int("From").clamp(0, 5) as usize],
                });
            }
        }
    }
    if let Some(Tag::List(records)) = data.get("Connections") {
        for record in records {
            let Tag::Compound(d) = record else {
                continue;
            };
            let Some(Tag::IntArray(p)) = d.get("Pos") else {
                continue;
            };
            let [x, y, z] = p.as_slice() else {
                continue;
            };
            if let Some(b) = super::connections::decode(d) {
                w.dims[0].fluid_transport.pipes.insert([*x, *y, *z], b);
            }
        }
    }
    if data.contains_key("Connections") {
        return;
    }
    let k = keys(data, "open_ends");
    for c in ints(data, "open_ends").as_chunks::<7>().0 {
        let tank = Codes::fluid(&k, c[4])
            .map(|f| (f, c[5].max(0) as u32))
            .filter(|t| t.1 > 0);
        let pos = [c[0], c[1], c[2]];
        let side = Dir::ALL[c[3].clamp(0, 5) as usize];
        let pipe = w.dims[0]
            .fluid_transport
            .pipes
            .entry(pos)
            .or_insert_with(|| {
                create_engine::fluids::transport::FluidTransportBehaviour::new(side.bit())
            });
        if pipe.connection(side).is_none() {
            pipe.interfaces.push((side, Default::default()));
        }
        if let Some(connection) = pipe.connection(side) {
            connection.set_source(create_engine::fluids::transport::Source::open(
                create_engine::open_end::OpenEnd {
                    tank,
                    pulling: c[6] != 0,
                },
            ));
        }
        w.dims[0]
            .fluid_transport
            .changed
            .push(create_engine::fluids::runtime::Change::Pipe(pos));
    }
}
