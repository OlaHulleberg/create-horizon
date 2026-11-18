//! PipeConnection.serializeNBT / deserializeNBT, using engine-owned side records.
use create_engine::{
    fluids::transport::{FluidTransportBehaviour, SavedConnection, SavedFlow},
    kinetics::Dir,
    open_end::OpenEnd,
    tank::Codes,
};
use mod_api::{Compound, Tag};
pub(super) fn encode(pipe: &FluidTransportBehaviour) -> Compound {
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
        connections.insert(
            format!("{side:?}").to_lowercase().as_str().into(),
            Tag::Compound(data),
        );
    }
    Compound::from([
        ("Connections".into(), Tag::Compound(connections)),
        (
            "ConnectionFluids".into(),
            Tag::List(
                codes
                    .keys
                    .into_iter()
                    .map(|s| Tag::String(s.as_str().into()))
                    .collect(),
            ),
        ),
    ])
}
pub(super) fn decode(data: &Compound) -> Option<FluidTransportBehaviour> {
    let Tag::Compound(connections) = data.get(&"Connections".into())? else {
        return None;
    };
    let keys = match data.get(&"ConnectionFluids".into()) {
        Some(Tag::List(k)) => k
            .iter()
            .map(|v| match v {
                Tag::String(s) => s.to_string().unwrap_or_default(),
                _ => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    };
    let fluid = |d: &Compound| match d.get(&"Fluid".into()) {
        Some(Tag::Int(c)) => Codes::fluid(&keys, *c),
        _ => None,
    };
    let mut saved = Vec::new();
    for side in Dir::ALL {
        let Some(Tag::Compound(d)) =
            connections.get(&format!("{side:?}").to_lowercase().as_str().into())
        else {
            continue;
        };
        let pressure = match d.get(&"Pressure".into()) {
            Some(Tag::List(p)) => match p.as_slice() {
                [Tag::Float(a), Tag::Float(b)] => Some([f32::from_bits(*a), f32::from_bits(*b)]),
                _ => None,
            },
            _ => None,
        };
        let open_end = match d.get(&"OpenEnd".into()) {
            Some(Tag::Compound(o)) => {
                let amount = match o.get(&"Amount".into()) {
                    Some(Tag::Int(n)) => (*n).max(0) as u32,
                    _ => 0,
                };
                Some(OpenEnd {
                    tank: fluid(o).filter(|_| amount > 0).map(|f| (f, amount)),
                    pulling: matches!(o.get(&"Pulling".into()), Some(Tag::Byte(1))),
                })
            }
            _ => None,
        };
        let flow = match d.get(&"Flow".into()) {
            Some(Tag::Compound(f)) => fluid(f).map(|fluid| SavedFlow {
                fluid,
                inbound: matches!(f.get(&"In".into()), Some(Tag::Byte(1))),
                progress: match f.get(&"Progress".into()) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use create_engine::{fluids::transport::Source, tank::Fluid};
    #[test]
    fn connections_round_trip_pressure_partial_flow_and_open_end() {
        let mut pipe = FluidTransportBehaviour::new(Dir::West.bit());
        let c = pipe.connection(Dir::West).unwrap();
        c.add_pressure(true, 32.);
        c.add_pressure(false, 16.);
        c.set_source(Source::open(OpenEnd {
            tank: Some((Fluid::Water, 984)),
            pulling: true,
        }));
        c.manage_flows(None, Some(Fluid::Water));
        c.tick_flow_progress();
        let data = encode(&pipe);
        assert_eq!(decode(&data).unwrap().write(), pipe.write());
        let Some(Tag::Compound(connections)) = data.get(&"Connections".into()) else {
            panic!()
        };
        let Some(Tag::Compound(west)) = connections.get(&"west".into()) else {
            panic!()
        };
        assert!(matches!(west.get(&"Pressure".into()),Some(Tag::List(p)) if p.len()==2));
        assert!(
            matches!(west.get(&"Flow".into()),Some(Tag::Compound(f)) if f.contains_key(&"Progress".into()))
        );
    }
}
