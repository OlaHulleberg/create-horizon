//! Fluid block entity codecs from MIT glue/fluids/saved.rs.
//! Called only by the marked-column flush and entity-load phases, never at unload.
use super::codec::{Compound, Tag};
use crate::scenario::Pos;
use create_engine::kinetics::Dir;
use create_engine::spout;
use create_engine::tank::{Codes, Fluid, MAX_HEIGHT, MAX_WIDTH, Multi, Tank};
use create_engine::world::CreateWorld;
use std::collections::BTreeMap;

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
const TANK: &str = "FluidTank";
const BOILER: &str = "BoilerState";
const SMART_PIPE: &str = "SmartPipe";
const HOSE: &str = "HosePulley";
const CREATIVE: &str = "CreativeTank";
const SPOUT: &str = "SpoutTank";
const DRAIN: &str = "DrainTank";

fn put_fluid(c: &mut Compound, fluid: Option<Fluid>) {
    if let Some(f) = fluid {
        c.insert("FluidName".into(), Tag::String(f.key()));
    }
}
fn fluid_of(c: &Compound) -> Option<Fluid> {
    match c.get("FluidName") {
        Some(Tag::String(s)) => Fluid::of_key(s),
        _ => None,
    }
}
fn int(c: &Compound, k: &str) -> i32 {
    match c.get(k) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    }
}
fn float(c: &Compound, k: &str) -> f32 {
    match c.get(k) {
        Some(Tag::Float(v)) => f32::from_bits(*v),
        _ => 0.,
    }
}
fn flag(c: &Compound, k: &str) -> bool {
    matches!(c.get(k), Some(Tag::Byte(b)) if *b != 0)
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

/// glue/fluids/saved.rs `save`: the fluid blocks' fields, by block entity.
pub fn save_entities(w: &CreateWorld, column: (i32, i32), out: &mut BTreeMap<Pos, Compound>) {
    let here = |p: &Pos| (p[0] >> 4, p[2] >> 4) == column;
    for (p, saved) in w.column_tanks(0, column.0, column.1) {
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
    for (p, b) in w.column_boilers(0, column.0, column.1) {
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
    let plumbing = &w.dims[0].plumbing;
    for (p, f) in plumbing.filters.iter().filter(|(p, _)| here(p)) {
        let mut c = Compound::from([("Item".into(), Tag::String(f.item.clone()))]);
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
    for (p, m) in w.column_creative_tanks(0, column.0, column.1) {
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
    let d = &w.dims[0];
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
                    ("Item".into(), Tag::String(h.item.clone())),
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
        out.entry(*p)
            .or_default()
            .extend(super::connections::encode(b));
    }
}

/// glue/fluids/saved.rs `load`.
pub fn load(w: &mut CreateWorld, pos: Pos, block: create_engine::Block, tag: &Compound) {
    let compound = |k: &str| match tag.get(k) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    };
    let transport = super::connections::decode(tag);
    if let Some(c) = compound(TANK) {
        let part = create_engine::tank::Part {
            controller: match c.get("Controller") {
                Some(Tag::IntArray(v)) if v.len() == 3 => [v[0], v[1], v[2]],
                _ => pos,
            },
            window: flag(c, "Window"),
            update_connectivity: flag(c, "Uninitialized"),
        };
        let multi = match c.get("Contents") {
            Some(Tag::Compound(m)) => Some(multi_of(m)),
            _ => None,
        };
        let saved = create_engine::tank::SavedTank { part, multi };
        w.read_tank(0, pos, saved);
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
        w.read_boiler(0, pos, b);
    }
    if block == create_engine::Block::CreativeFluidTank
        && let Some(c) = compound(CREATIVE)
    {
        let multi = c.contains_key("Size").then(|| Multi {
            tank: Tank::default(),
            ..multi_of(c)
        });
        w.read_creative_tank(0, pos, fluid_of(c), multi);
    }
    if let Some(c) = compound(SMART_PIPE) {
        let p = &mut w.dims[0].plumbing;
        let item = match c.get("Item") {
            Some(Tag::String(s)) => s.clone(),
            _ => String::new(),
        };
        p.filters.insert(
            pos,
            create_engine::fluids::SmartFilter {
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
        w.read_hose(0, pos, float(c, "Offset").max(0.), tank);
    }
    let d = &mut w.dims[0];
    if let Some(c) = compound(SPOUT) {
        d.spouts.insert(
            pos,
            spout::Spout {
                fluid: fluid_tank_of(c),
                processing: int(c, "ProcessingTicks"),
            },
        );
    }
    if let Some(c) = compound(DRAIN) {
        let dr = d.drains.entry(pos).or_default();
        dr.fluid = fluid_tank_of(c);
        dr.processing = int(c, "ProcessingTicks");
        dr.held = match c.get("HeldItem") {
            Some(Tag::Compound(h)) => {
                let item = match h.get("Item") {
                    Some(Tag::String(s)) => s.clone(),
                    _ => String::new(),
                };
                let position = float(h, "BeltPosition");
                (!item.is_empty()).then(|| create_engine::drain::Held {
                    item,
                    count: int(h, "Count").clamp(1, 64) as u8,
                    position,
                    prev: position,
                    from: Dir::ALL[int(h, "From").clamp(0, 5) as usize],
                })
            }
            _ => None,
        };
    }
    if let Some(b) = transport {
        d.fluid_transport.pipes.insert(pos, b);
    }
}
