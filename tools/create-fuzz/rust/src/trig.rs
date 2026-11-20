//! Counterfactual *state* trace for the measured nearest-direction tie. The ordinary run
//! remains the sole source of kinetic, network, item and other observations.
use crate::{
    allow::MeasuredTrigState,
    scenario::{Pos, Scenario, Snapshot, parse_state, with_prop},
};
use create_engine::{Block, kinetics::Dir, place};
use std::collections::BTreeSet;

/// Only this exact input was measured. Java's magnitudes tie bit-for-bit; the port's
/// cosine is one table entry lower. Near angles and other quadrants are not evidence.
pub fn exact_tie(yaw: f32, pitch: f32) -> bool {
    yaw.to_bits() == 45f32.to_bits()
        && pitch.to_bits() == 0f32.to_bits()
        && f32::from_bits(3207922931).abs().to_bits() == 1060439283
        && f32::from_bits(1060438146).to_bits() != 1060439283
        && place::ordered_by_nearest(yaw, pitch)[0] == Dir::West
}

/// Placement API provenance, rather than block identity in the comparison allowance.
/// These families mirror place.rs and Create's getStateForPlacement implementations.
/// Horizontal-direction, clicked-face and neighbour-selected choices are excluded.
pub fn nearest_property(
    b: Block,
    ctx: place::Context,
    shaft: impl Fn(Dir) -> bool,
    near: place::Nearby,
) -> Option<&'static str> {
    let preferred = place::preferred(false, &shaft);
    match b {
        Block::Shaft
        | Block::Flywheel
        | Block::CrushingWheel
        | Block::LargeWaterWheel
        | Block::AndesiteEncasedShaft
        | Block::BrassEncasedShaft
        | Block::Clutch
        | Block::Gearshift
            if preferred.is_none() =>
        {
            Some("axis")
        }
        Block::AndesiteEncasedCogwheel
        | Block::BrassEncasedCogwheel
        | Block::AndesiteEncasedLargeCogwheel
        | Block::BrassEncasedLargeCogwheel
            if preferred.is_none() && near.small_cog_clicked.is_none() =>
        {
            Some("axis")
        }
        Block::EncasedChainDrive | Block::AdjustableChainGearshift | Block::SequencedGearshift
            if preferred.is_none() || ctx.sneaking =>
        {
            Some("axis")
        }
        Block::CreativeMotor
        | Block::WindmillBearing
        | Block::WaterWheel
        | Block::MechanicalBearing
        | Block::ClockworkBearing
        | Block::MechanicalDrill
            if preferred.is_none() || ctx.sneaking =>
        {
            Some("facing")
        }
        Block::EncasedFan if preferred.is_none() => Some("facing"),
        Block::Deployer
        | Block::MechanicalSaw
        | Block::ElevatorContact
        | Block::Sticker
        | Block::PortableStorageInterface
        | Block::PortableFluidInterface => Some("facing"),
        b if b == Block::SailFrame || b.name().ends_with("_sail") => Some("facing"),
        _ => None,
    }
}

pub fn java_choice(state: &str, property: &str) -> String {
    let (_, props) = parse_state(state);
    let old = props
        .iter()
        .find(|(k, _)| k == property)
        .map(|(_, v)| v.as_str());
    let value = match (property, old) {
        ("axis", Some("x")) => "z",
        ("facing", Some("west")) => "south",
        ("facing", Some("east")) => "north",
        _ => return state.into(),
    };
    with_prop(state, property, value)
}

fn is_chain(s: &str) -> bool {
    matches!(
        parse_state(s).0.as_str(),
        "create:encased_chain_drive" | "create:adjustable_chain_gearshift"
    )
}
fn pos(key: &str) -> Pos {
    let v: Vec<i32> = key.split(',').map(|s| s.parse().unwrap()).collect();
    [v[0], v[1], v[2]]
}

/// Limit derived-property allowances to the neighbouring chain component of an actual
/// tie placement. A counterfactual elsewhere cannot excuse an unrelated chain output.
pub fn attach(actual: &mut Snapshot, alternative: &Snapshot) {
    let mut connected = BTreeSet::new();
    let mut queue = Vec::new();
    for (k, b) in &actual.blocks {
        if b.measured_trig.is_some() && is_chain(&b.state) {
            queue.push(pos(k));
        }
    }
    while let Some(p) = queue.pop() {
        if !connected.insert(p) {
            continue;
        }
        for d in Dir::ALL {
            let o = d.offset();
            let q = [p[0] + o[0], p[1] + o[1], p[2] + o[2]];
            let key = crate::scenario::key(q);
            if actual.blocks.get(&key).is_some_and(|b| is_chain(&b.state))
                && !connected.contains(&q)
            {
                queue.push(q);
            }
        }
    }
    for (k, b) in &mut actual.blocks {
        let root = b.measured_trig.take();
        let mut properties = root.map(|m| m.properties).unwrap_or_default();
        if connected.contains(&pos(k)) {
            properties.extend(["part".into(), "axis_along_first".into()]);
        }
        let Some(other) = alternative.blocks.get(k) else {
            continue;
        };
        if parse_state(&b.state).0 != parse_state(&other.state).0 {
            continue;
        }
        let mut java = b.state.clone();
        for (property, value) in parse_state(&other.state).1 {
            if properties.contains(&property) {
                java = with_prop(&java, &property, &value);
            }
        }
        if java != b.state {
            b.measured_trig = Some(MeasuredTrigState {
                tie: Some(crate::allow::NearestTie::Yaw45Pitch0),
                java,
                port: b.state.clone(),
                properties,
            });
        }
    }
    for (t, s) in &mut actual.checkpoints {
        if let Some(a) = alternative.checkpoints.get(t) {
            attach(s, a);
        }
    }
}

pub fn run(sc: &Scenario, phase: u8, origin: crate::scenario::Pos) -> Snapshot {
    let mut actual = crate::engine::run_trace_at(sc, false, phase, origin);
    if has_roots(&actual) {
        let alternative = crate::engine::run_trace_at(sc, true, phase, origin);
        attach(&mut actual, &alternative);
    }
    actual
}
fn has_roots(s: &Snapshot) -> bool {
    s.blocks.values().any(|b| b.measured_trig.is_some()) || s.checkpoints.values().any(has_roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scenario(item: &str, yaw: f32) -> Scenario {
        serde_json::from_value(serde_json::json!({"version":3,"ticks":80,"blocks":[{"pos":[0,7,0],"state":"minecraft:stone"}],"events":[{"tick":4,"pos":[0,7,0],"action":{"kind":"use","item":item,"face":"up","hit":[0.5,1,0.5],"yaw":yaw,"pitch":0,"sneaking":true}}]})).unwrap()
    }
    #[test]
    fn non_ties_other_properties_and_figures_still_report() {
        let sc = scenario("create:adjustable_chain_gearshift", 45.);
        let r = crate::engine::run(&sc);
        let mut j = crate::engine::run_trace(&sc, true);
        assert!(crate::compare::diff(&j, &r).is_empty());
        j.blocks
            .get_mut("0,8,0")
            .unwrap()
            .kinetic
            .as_mut()
            .unwrap()
            .speed = 17.;
        assert!(
            crate::compare::diff(&j, &r)
                .iter()
                .any(|d| d.contains("speed: java 17"))
        );
        j = crate::engine::run_trace(&sc, true);
        j.blocks
            .get_mut("0,8,0")
            .unwrap()
            .kinetic
            .as_mut()
            .unwrap()
            .capacity = 123.;
        assert!(
            crate::compare::diff(&j, &r)
                .iter()
                .any(|d| d.contains("capacity: java 123"))
        );
        j = crate::engine::run_trace(&sc, true);
        j.networks
            .as_mut()
            .unwrap()
            .sizes
            .insert("0,8,0".into(), 99);
        assert!(
            crate::compare::diff(&j, &r)
                .iter()
                .any(|d| d == "network sizes differ")
        );
        j = crate::engine::run_trace(&sc, true);
        let block = j.blocks.get_mut("0,8,0").unwrap();
        block.state = with_prop(&block.state, "powered", "true");
        assert!(
            crate::compare::diff(&j, &r)
                .iter()
                .any(|d| d.contains("powered"))
        );
        for yaw in [
            f32::from_bits(45f32.to_bits() - 1),
            f32::from_bits(45f32.to_bits() + 1),
        ] {
            assert!(!exact_tie(yaw, 0.));
            let sc = scenario("create:encased_chain_drive", yaw);
            let r = crate::engine::run(&sc);
            let mut j = r.clone();
            assert!(r.blocks["0,8,0"].measured_trig.is_none());
            let state = &r.blocks["0,8,0"].state;
            let axis = if parse_state(state)
                .1
                .iter()
                .any(|(k, v)| k == "axis" && v == "x")
            {
                "z"
            } else {
                "x"
            };
            j.blocks.get_mut("0,8,0").unwrap().state = with_prop(state, "axis", axis);
            assert!(
                crate::compare::diff(&j, &r)
                    .iter()
                    .any(|d| d.contains("axis:"))
            );
        }
    }
    #[test]
    fn provenance_excludes_preferred_clicked_face_and_horizontal_choices() {
        let ctx = place::Context {
            yaw: 45.,
            pitch: 0.,
            sneaking: false,
            clicked_face: Dir::Up,
        };
        assert_eq!(
            nearest_property(
                Block::Shaft,
                ctx,
                |d| d == Dir::East,
                place::Nearby::default()
            ),
            None
        );
        assert_eq!(
            nearest_property(
                Block::Shaft,
                place::Context {
                    sneaking: true,
                    ..ctx
                },
                |d| d == Dir::East,
                place::Nearby::default()
            ),
            None
        );
        assert_eq!(
            nearest_property(
                Block::RotationSpeedController,
                ctx,
                |_| false,
                place::Nearby::default()
            ),
            None
        );
        assert_eq!(
            nearest_property(Block::Gearbox, ctx, |_| false, place::Nearby::default()),
            None
        );
        assert_eq!(
            nearest_property(
                Block::EncasedChainDrive,
                place::Context {
                    sneaking: true,
                    ..ctx
                },
                |d| d == Dir::East,
                place::Nearby::default()
            ),
            Some("axis")
        );
        assert_eq!(
            nearest_property(
                Block::CreativeMotor,
                ctx,
                |_| false,
                place::Nearby::default()
            ),
            Some("facing")
        );
        let sc = scenario("create:creative_motor", 45.);
        let r = crate::engine::run(&sc);
        let mut j = r.clone();
        j.blocks.get_mut("0,8,0").unwrap().state = r.blocks["0,8,0"]
            .measured_trig
            .as_ref()
            .unwrap()
            .java
            .clone();
        assert!(
            r.blocks["0,8,0"]
                .measured_trig
                .as_ref()
                .unwrap()
                .properties
                .contains(&"facing".into())
        );
        assert!(crate::compare::diff(&j, &r).is_empty());
    }
}

#[cfg(test)]
mod derived_tests {
    #[test]
    fn chain_component_shapes_are_derived_but_unrelated_states_are_not() {
        let sc:crate::scenario::Scenario=serde_json::from_value(serde_json::json!({"version":3,"ticks":80,"blocks":[
            {"pos":[0,7,2],"state":"minecraft:stone"},{"pos":[0,7,3],"state":"minecraft:stone"},{"pos":[0,7,4],"state":"minecraft:stone"},
            {"pos":[6,8,0],"state":"create:encased_chain_drive[axis=x,axis_along_first=false,part=none]"}],"events":[
            {"tick":14,"pos":[0,7,2],"action":{"kind":"use","item":"create:adjustable_chain_gearshift","face":"up","hit":[0.1,1,0.9],"yaw":131,"pitch":0,"sneaking":true}},
            {"tick":16,"pos":[0,7,3],"action":{"kind":"use","item":"create:encased_chain_drive","face":"up","hit":[0.1,1,0.9],"yaw":287,"pitch":0,"sneaking":true}},
            {"tick":18,"pos":[0,7,4],"action":{"kind":"use","item":"create:adjustable_chain_gearshift","face":"up","hit":[0.5,1,0.9],"yaw":45,"pitch":0,"sneaking":true}}]})).unwrap();
        let r = crate::engine::run(&sc);
        let mut j = crate::engine::run_trace(&sc, true);
        assert!(
            r.blocks["0,8,3"]
                .measured_trig
                .as_ref()
                .unwrap()
                .properties
                .contains(&"part".into())
        );
        assert!(crate::compare::diff(&j, &r).is_empty());
        let b = j.blocks.get_mut("6,8,0").unwrap();
        b.state = crate::scenario::with_prop(&b.state, "part", "end");
        assert!(
            crate::compare::diff(&j, &r)
                .iter()
                .any(|d| d.contains("6,8,0: state part"))
        );
    }
}
