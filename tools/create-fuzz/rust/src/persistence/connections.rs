//! PipeConnection.serializeNBT / deserializeNBT, using engine-owned side records.
use super::codec::{Compound, Tag};
use create_engine::{
    fluids::transport::{FluidTransportBehaviour, SavedConnection, SavedFlow},
    kinetics::Dir,
    open_end::OpenEnd,
    tank::Codes,
};
pub fn encode(pipe: &FluidTransportBehaviour) -> Compound {
    let mut codes = Codes::default();
    let mut connections = Compound::new();
    for (side, saved) in pipe.write() {
        let mut data = Compound::new();
        if let Some(p) = saved.pressure {
            data.insert(
                "Pressure".into(),
                Tag::List(p.into_iter().map(|f| Tag::Float(f.to_bits())).collect()),
            );
        }
        if let Some(open) = saved.open_end {
            data.insert(
                "OpenEnd".into(),
                Tag::Compound(Compound::from([
                    ("Fluid".into(), Tag::Int(codes.code(open.tank.map(|t| t.0)))),
                    (
                        "Amount".into(),
                        Tag::Int(open.tank.map_or(0, |t| t.1 as i32)),
                    ),
                    ("Pulling".into(), Tag::Byte(i8::from(open.pulling))),
                ])),
            );
        }
        if let Some(flow) = saved.flow {
            let mut f = Compound::from([
                ("Fluid".into(), Tag::Int(codes.code(Some(flow.fluid)))),
                ("In".into(), Tag::Byte(i8::from(flow.inbound))),
            ]);
            if let Some(progress) = flow.progress {
                f.insert("Progress".into(), Tag::Float(progress.to_bits()));
            }
            data.insert("Flow".into(), Tag::Compound(f));
        }
        connections.insert(format!("{side:?}").to_lowercase(), Tag::Compound(data));
    }
    Compound::from([
        ("Connections".into(), Tag::Compound(connections)),
        (
            "ConnectionFluids".into(),
            Tag::List(codes.keys.into_iter().map(Tag::String).collect()),
        ),
    ])
}
pub fn decode(data: &Compound) -> Option<FluidTransportBehaviour> {
    let Tag::Compound(connections) = data.get("Connections")? else {
        return None;
    };
    let keys = match data.get("ConnectionFluids") {
        Some(Tag::List(k)) => k
            .iter()
            .map(|v| match v {
                Tag::String(s) => s.to_string(),
                _ => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    };
    let fluid = |d: &Compound| match d.get("Fluid") {
        Some(Tag::Int(c)) => Codes::fluid(&keys, *c),
        _ => None,
    };
    let mut saved = Vec::new();
    for side in Dir::ALL {
        let Some(Tag::Compound(d)) = connections.get(&format!("{side:?}").to_lowercase()) else {
            continue;
        };
        let pressure = match d.get("Pressure") {
            Some(Tag::List(p)) => match p.as_slice() {
                [Tag::Float(a), Tag::Float(b)] => Some([f32::from_bits(*a), f32::from_bits(*b)]),
                _ => None,
            },
            _ => None,
        };
        let open_end = match d.get("OpenEnd") {
            Some(Tag::Compound(o)) => {
                let amount = match o.get("Amount") {
                    Some(Tag::Int(n)) => (*n).max(0) as u32,
                    _ => 0,
                };
                Some(OpenEnd {
                    tank: fluid(o).filter(|_| amount > 0).map(|f| (f, amount)),
                    pulling: matches!(o.get("Pulling"), Some(Tag::Byte(1))),
                })
            }
            _ => None,
        };
        let flow = match d.get("Flow") {
            Some(Tag::Compound(f)) => fluid(f).map(|fluid| SavedFlow {
                fluid,
                inbound: matches!(f.get("In"), Some(Tag::Byte(1))),
                progress: match f.get("Progress") {
                    Some(Tag::Float(p)) => Some(f32::from_bits(*p)),
                    _ => None,
                },
            }),
            _ => None,
        };
        saved.push((
            side,
            SavedConnection {
                pressure,
                open_end,
                flow,
            },
        ));
    }
    Some(FluidTransportBehaviour::read(&saved, 0))
}
