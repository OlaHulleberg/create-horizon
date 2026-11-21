//! What the fluid blocks keep in their block entities: tanks and their boilers, smart pipes'
//! filters, hose pulleys, creative tanks, spouts, drains and every pipe's connections. An older
//! version kept them with the column (`tank_entities`, `boilers`, `create_fluids`,
//! `create_machines`, `Connections`), read once by the subsystems' `chunk_load`.

use super::connections;
use crate::glue::entity_data::Entities;
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::Block;
use create_engine::fluids::SmartFilter;
use create_engine::tank::{Fluid, MAX_HEIGHT, MAX_WIDTH, Multi, Tank};
use mod_api::{Compound, Tag};

const TANK: &str = "FluidTank";
const BOILER: &str = "BoilerState";
const SMART_PIPE: &str = "SmartPipe";
const HOSE: &str = "HosePulley";
const CREATIVE: &str = "CreativeTank";
const SPOUT: &str = "SpoutTank";
const DRAIN: &str = "DrainTank";
const CONNECTIONS: [&str; 2] = ["Connections", "ConnectionFluids"];

pub(super) fn saved_keys(block: Block) -> &'static [&'static str] {
    match block {
        Block::FluidTank => &[TANK, BOILER],
        Block::CreativeFluidTank => &[TANK, CREATIVE],
        Block::Spout => &[SPOUT],
        Block::ItemDrain => &[DRAIN],
        Block::SmartFluidPipe => &[SMART_PIPE, CONNECTIONS[0], CONNECTIONS[1]],
        Block::HosePulley => &[HOSE, CONNECTIONS[0], CONNECTIONS[1]],
        b if create_engine::world::is_fluid_block(b) => &CONNECTIONS,
        _ => &[],
    }
}

fn put_fluid(c: &mut Compound, fluid: Option<Fluid>) {
    if let Some(f) = fluid {
        c.insert("FluidName".into(), Tag::String(f.key().as_str().into()));
    }
}

fn fluid_of(c: &Compound) -> Option<Fluid> {
    match c.get(&"FluidName".into()) {
        Some(Tag::String(s)) => Fluid::of_key(&s.to_string_lossy()),
        _ => None,
    }
}

fn int(c: &Compound, k: &str) -> i32 {
    match c.get(&k.into()) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    }
}

fn float(c: &Compound, k: &str) -> f32 {
    match c.get(&k.into()) {
        Some(Tag::Float(v)) => f32::from_bits(*v),
        _ => 0.0,
    }
}

fn flag(c: &Compound, k: &str) -> bool {
    matches!(c.get(&k.into()), Some(Tag::Byte(b)) if *b != 0)
}

fn byte(b: bool) -> Tag {
    Tag::Byte(i8::from(b))
}

fn multi_to_tag(m: &Multi) -> Compound {
    let mut c = Compound::from([
        ("Amount".into(), Tag::Int(m.tank.amount as i32)),
        ("Size".into(), Tag::Int(i32::from(m.width))),
        ("Height".into(), Tag::Int(i32::from(m.height))),
        ("Window".into(), byte(m.window)),
    ]);
    put_fluid(&mut c, m.tank.fluid);
    c
}

fn multi_of(c: &Compound) -> Multi {
    Multi {
        tank: Tank {
            fluid: fluid_of(c),
            amount: int(c, "Amount").max(0) as u32,
        },
        width: int(c, "Size").clamp(1, MAX_WIDTH) as u8,
        height: int(c, "Height").clamp(1, MAX_HEIGHT) as u8,
        window: flag(c, "Window"),
    }
}

fn fluid_tank(fluid: Option<(Fluid, u32)>, processing: Option<i32>) -> Compound {
    let mut c = Compound::from([("Amount".into(), Tag::Int(fluid.map_or(0, |f| f.1 as i32)))]);
    put_fluid(&mut c, fluid.map(|f| f.0));
    if let Some(ticks) = processing {
        c.insert("ProcessingTicks".into(), Tag::Int(ticks));
    }
    c
}

fn fluid_tank_of(c: &Compound) -> Option<(Fluid, u32)> {
    fluid_of(c).map(|f| (f, int(c, "Amount").max(0) as u32))
}

/// `FluidTankBlockEntity.write` (with its `BoilerData`), `SmartFluidPipeBlockEntity`'s filter,
/// `HosePulleyBlockEntity.write`, `CreativeFluidTankBlockEntity`'s fluid,
/// `SpoutBlockEntity.write`, `ItemDrainBlockEntity.write` and `FluidTransportBehaviour.write` of
/// column `chunk`'s block entities.
pub(super) fn save(lv: &ServerLevel<'_>, chunk: [i32; 2], out: &mut Entities) {
    let dim = lv.dim;
    let (cx, cz) = (chunk[0], chunk[1]);
    let here = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (cx, cz);
    with_world(|w| {
        for (p, saved) in w.column_tanks(dim, cx, cz) {
            let part = saved.part;
            let mut c = Compound::from([
                ("Controller".into(), Tag::IntArray(part.controller.to_vec())),
                ("Window".into(), byte(part.window)),
                ("Uninitialized".into(), byte(part.update_connectivity)),
            ]);
            if let Some(m) = saved.multi {
                c.insert("Contents".into(), Tag::Compound(multi_to_tag(&m)));
            }
            out.entry(p)
                .or_default()
                .insert(TANK.into(), Tag::Compound(c));
        }
        for (p, b) in w.column_boilers(dim, cx, cz) {
            let c = Compound::from([
                ("Supply".into(), Tag::Float(b.water_supply.to_bits())),
                ("ActiveHeat".into(), Tag::Int(b.active_heat)),
                ("PassiveHeat".into(), byte(b.passive_heat)),
                ("Engines".into(), Tag::Int(b.attached_engines)),
                ("Whistles".into(), Tag::Int(b.attached_whistles)),
                ("Update".into(), byte(b.needs_heat_level_update)),
            ]);
            out.entry(p)
                .or_default()
                .insert(BOILER.into(), Tag::Compound(c));
        }
        let plumbing = &w.dims[dim].plumbing;
        for (p, f) in plumbing.filters.iter().filter(|(p, _)| here(p)) {
            let mut c = Compound::from([("Item".into(), Tag::String(f.item.as_str().into()))]);
            put_fluid(&mut c, f.fluid);
            out.entry(*p)
                .or_default()
                .insert(SMART_PIPE.into(), Tag::Compound(c));
        }
        let hoses = plumbing.hoses.iter().map(|(p, h)| (p, h.offset, h.tank));
        let staged = plumbing.hose_saved.iter().map(|(p, s)| (p, s.0, s.1));
        for (p, offset, tank) in hoses.chain(staged).filter(|(p, _, _)| here(p)) {
            let mut c = fluid_tank(tank.fluid.map(|f| (f, tank.amount)), None);
            c.insert("Offset".into(), Tag::Float(offset.to_bits()));
            out.entry(*p)
                .or_default()
                .insert(HOSE.into(), Tag::Compound(c));
        }
        let creative = plumbing.creative.iter().chain(&plumbing.creative_saved);
        for (p, f) in creative.filter(|(p, _)| here(p)) {
            let mut c = Compound::new();
            put_fluid(&mut c, *f);
            out.entry(*p)
                .or_default()
                .insert(CREATIVE.into(), Tag::Compound(c));
        }
        for (p, m) in w.column_creative_tanks(dim, cx, cz) {
            let entry = out.entry(p).or_default();
            let Tag::Compound(c) = entry
                .entry(CREATIVE.into())
                .or_insert_with(|| Tag::Compound(Compound::new()))
            else {
                continue;
            };
            c.extend([
                ("Size".into(), Tag::Int(i32::from(m.width))),
                ("Height".into(), Tag::Int(i32::from(m.height))),
                ("Window".into(), byte(m.window)),
            ]);
        }
        let d = &w.dims[dim];
        for (p, s) in d.spouts.iter().filter(|(p, _)| here(p)) {
            let c = fluid_tank(s.fluid, Some(s.processing));
            out.entry(*p)
                .or_default()
                .insert(SPOUT.into(), Tag::Compound(c));
        }
        for (p, dr) in d.drains.iter().filter(|(p, _)| here(p)) {
            let mut c = fluid_tank(dr.fluid, Some(dr.processing));
            if let Some(h) = &dr.held {
                c.insert(
                    "HeldItem".into(),
                    Tag::Compound(Compound::from([
                        ("Item".into(), Tag::String(h.item.as_str().into())),
                        ("Count".into(), Tag::Int(i32::from(h.count))),
                        ("BeltPosition".into(), Tag::Float(h.position.to_bits())),
                        ("From".into(), Tag::Int(h.from as i32)),
                    ])),
                );
            }
            out.entry(*p)
                .or_default()
                .insert(DRAIN.into(), Tag::Compound(c));
        }
        for (p, b) in d.fluid_transport.pipes.iter().filter(|(p, _)| here(p)) {
            out.entry(*p).or_default().extend(connections::encode(b));
        }
    });
}

/// The `read`s of the fields [`save`] writes.
pub(super) fn load(lv: &mut ServerLevel<'_>, pos: [i32; 3], block: Block, tag: &Compound) {
    let dim = lv.dim;
    let compound = |k: &str| match tag.get(&k.into()) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    };
    let transport = connections::decode(tag);
    with_world(|w| {
        if let Some(c) = compound(TANK) {
            let part = create_engine::tank::Part {
                controller: match c.get(&"Controller".into()) {
                    Some(Tag::IntArray(v)) if v.len() == 3 => [v[0], v[1], v[2]],
                    _ => pos,
                },
                window: flag(c, "Window"),
                update_connectivity: flag(c, "Uninitialized"),
            };
            let multi = match c.get(&"Contents".into()) {
                Some(Tag::Compound(m)) => Some(multi_of(m)),
                _ => None,
            };
            let saved = create_engine::tank::SavedTank { part, multi };
            w.read_tank(dim, pos, saved);
        }
        if let Some(c) = compound(BOILER) {
            let b = create_engine::steam::SavedBoiler {
                water_supply: float(c, "Supply"),
                active_heat: int(c, "ActiveHeat"),
                passive_heat: flag(c, "PassiveHeat"),
                attached_engines: int(c, "Engines"),
                attached_whistles: int(c, "Whistles"),
                needs_heat_level_update: flag(c, "Update"),
            };
            w.read_boiler(dim, pos, b);
        }
        if block == Block::CreativeFluidTank
            && let Some(c) = compound(CREATIVE)
        {
            let multi = c.contains_key(&"Size".into()).then(|| Multi {
                tank: Tank::default(),
                ..multi_of(c)
            });
            w.read_creative_tank(dim, pos, fluid_of(c), multi);
        }
        let p = &mut w.dims[dim].plumbing;
        if let Some(c) = compound(SMART_PIPE) {
            let item = match c.get(&"Item".into()) {
                Some(Tag::String(s)) => s.to_string_lossy(),
                _ => String::new(),
            };
            p.filters.insert(
                pos,
                SmartFilter {
                    item,
                    fluid: fluid_of(c),
                },
            );
            p.filter_changes.push(pos);
        }
        if let Some(c) = compound(HOSE) {
            let tank = Tank {
                fluid: fluid_of(c),
                amount: int(c, "Amount").clamp(0, create_engine::hose::CAPACITY as i32) as u32,
            };
            w.read_hose(dim, pos, float(c, "Offset").max(0.0), tank);
        }
        let d = &mut w.dims[dim];
        if let Some(c) = compound(SPOUT) {
            let s = create_engine::spout::Spout {
                fluid: fluid_tank_of(c),
                processing: int(c, "ProcessingTicks"),
            };
            d.spouts.insert(pos, s);
        }
        if let Some(c) = compound(DRAIN) {
            let dr = d.drains.entry(pos).or_default();
            dr.fluid = fluid_tank_of(c);
            dr.processing = int(c, "ProcessingTicks");
            dr.held = match c.get(&"HeldItem".into()) {
                Some(Tag::Compound(h)) => {
                    let item = match h.get(&"Item".into()) {
                        Some(Tag::String(s)) => s.to_string_lossy(),
                        _ => String::new(),
                    };
                    let position = float(h, "BeltPosition");
                    (!item.is_empty()).then(|| create_engine::drain::Held {
                        item,
                        count: int(h, "Count").clamp(1, 64) as u8,
                        position,
                        prev: position,
                        from: create_engine::kinetics::Dir::ALL
                            [int(h, "From").clamp(0, 5) as usize],
                    })
                }
                _ => None,
            };
        }
        if let Some(b) = transport {
            d.fluid_transport.pipes.insert(pos, b);
        }
    });
}
