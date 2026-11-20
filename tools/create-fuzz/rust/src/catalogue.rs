//! Explicit Create 6.0.11 registry and Cartesian property domains.
use crate::scenario::{Scenario, parse_state};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Deserialize)]
pub struct Entry {
    pub id: String,
    pub properties: BTreeMap<String, Vec<String>>,
    pub source: String,
    pub observation_scope: BTreeMap<String, String>,
    pub state_phase: Option<String>,
    pub connection: crate::geometry::Connection,
    pub excluded: Option<String>,
}
pub fn entries() -> &'static [Entry] {
    static ENTRIES: std::sync::OnceLock<Vec<Entry>> = std::sync::OnceLock::new();
    ENTRIES.get_or_init(|| {
        serde_json::from_str(include_str!("kinetics.json")).expect("kinetic manifest")
    })
}
pub fn validate(state: &str) -> Result<(), String> {
    let (id, props) = parse_state(state);
    if crate::fluid_world::id(state).is_some() {
        return Ok(());
    }
    if !id.starts_with("create:") {
        return if [
            "minecraft:air",
            "minecraft:stone",
            "minecraft:redstone_block",
        ]
        .contains(&state)
        {
            Ok(())
        } else {
            Err(format!("unrepresented external state {state}"))
        };
    }
    if state.contains('[') && !state.ends_with(']') {
        return Err(format!("malformed state {state}"));
    }
    if let Some(result) = crate::transport::validate_state(&id, &props) {
        return result;
    }
    if let Some(result) = crate::fluids::validate_state(&id, &props) {
        return result;
    }
    let entry = entries()
        .iter()
        .find(|e| e.id == id)
        .ok_or_else(|| format!("unknown kinetic id {id}"))?;
    let mut seen = BTreeSet::new();
    for (k, v) in props {
        if !seen.insert(k.clone()) || !entry.properties.get(&k).is_some_and(|vs| vs.contains(&v)) {
            return Err(format!("unrepresented state {state}: {k}={v}"));
        }
    }
    if state
        .split_once('[')
        .is_some_and(|(_, p)| p.trim_end_matches(']').split(',').any(|p| !p.contains('=')))
    {
        return Err(format!("malformed state {state}"));
    }
    Ok(())
}
pub fn scenario(sc: &Scenario) {
    crate::transport::validate_scenario(sc);
    sc.region();
    assert!([1, 3].contains(&sc.version), "unsupported scenario version");
    for e in &sc.events {
        if let Some(a) = &e.action {
            assert_eq!(sc.version, 3);
            a.validate();
        }
    }
    assert!(
        sc.checkpoints.iter().all(|t| *t <= sc.ticks),
        "checkpoint after final tick"
    );
    assert!(
        sc.events.iter().all(|e| e.tick < sc.ticks
            && usize::from(e.state.is_some())
                + usize::from(e.turn.is_some())
                + usize::from(e.action.is_some())
                == 1),
        "event must contain exactly one action before final tick"
    );
    for b in sc
        .blocks
        .iter()
        .cloned()
        .chain(sc.events.iter().filter_map(|e| e.placed()))
    {
        assert!(
            b.motor.is_none() || parse_state(&b.state).0 == "create:creative_motor",
            "motor action only supported for creative motors"
        );
    }
    let mut world = BTreeMap::new();
    for t in 0..sc.ticks {
        if let Some(b) = sc.blocks.get(t as usize) {
            world.insert(b.pos, parse_state(&b.state).0);
        }
        for event in sc.events.iter().filter(|e| e.tick == t) {
            if let Some(s) = &event.state {
                world.insert(event.pos, parse_state(s).0);
            }
            if event.turn.is_some() {
                assert!(
                    world
                        .get(&event.pos)
                        .is_none_or(|id| id == "create:hand_crank" || id == "minecraft:air"),
                    "turn adapter supports hand cranks only; valve activation is excluded"
                );
            }
        }
    }
    for state in sc
        .blocks
        .iter()
        .map(|b| &b.state)
        .chain(sc.events.iter().filter_map(|e| e.state.as_ref()))
    {
        validate(state).unwrap_or_else(|e| panic!("{e}"));
    }
}
/// Counts submitted states separately from crank interaction targets; never claims full domains.
pub fn report(manual: &[Scenario], generated: &[Scenario]) {
    crate::fluids::report(manual, generated);
    let collect = |cases: &[Scenario]| -> BTreeMap<String, BTreeSet<String>> {
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for sc in cases {
            scenario(sc);
            for s in sc
                .blocks
                .iter()
                .map(|b| &b.state)
                .chain(sc.events.iter().filter_map(|e| e.state.as_ref()))
            {
                out.entry(parse_state(s).0).or_default().insert(s.clone());
            }
        }
        out
    };
    let interactions = |cases: &[Scenario]| -> (usize, usize) {
        (
            cases
                .iter()
                .map(|sc| sc.events.iter().filter(|e| e.turn.is_some()).count())
                .sum(),
            cases
                .iter()
                .map(|sc| {
                    sc.blocks.iter().filter(|b| b.motor.is_some()).count()
                        + sc.events.iter().filter(|e| e.motor.is_some()).count()
                })
                .sum(),
        )
    };
    println!(
        "interaction submissions (turn, motor): generated {:?}; manual {:?}",
        interactions(generated),
        interactions(manual)
    );
    for (label, cases) in [("manual", manual), ("generated", generated)] {
        let mut counts = BTreeMap::<String, usize>::new();
        for e in cases.iter().flat_map(|sc| &sc.events) {
            if let Some(a) = &e.action {
                let v = serde_json::to_value(a).unwrap();
                *counts
                    .entry(v["kind"].as_str().unwrap().to_owned())
                    .or_default() += 1;
            }
        }
        println!("{label} version 3 action submissions: {counts:?}");
    }
    println!(
        "use limits: shaft casing/bracket; belts through persistent connector use, casing, pulley and slicer endpoints; conveyor chain_link screen packet; motor value-box geometry via board::motor_slot/hits_box; motor/controller speed boards, sequencer programs, valve angle/use/dye and chain topology enabled; fluid placement, world callbacks, bucket handlers and smart filters enabled; other block-use overrides and arbitrary inventories require separate contracts; ordinary ServerPlayer uses geometric value-box hit tests; no allowance for these gaps"
    );
    for block in &crate::transport::contract().blocks {
        println!(
            "transport block: {} {:?}; {}",
            block.id, block.properties, block.source
        );
    }
    for item in &crate::transport::contract().items {
        println!("transport item: {} max_stack={}", item.id, item.max_stack);
    }
    println!(
        "item transport: controlled player-drop landing, empty-hand belt pickup, depot use/swap, belt inventory tick and belt/depot end insertion; longitudinal position/stack/lock/pending/stall compared; fluid spout/drain processing and depot outputs enabled; no native item flight, components, other machine processing, funnels or tunnels"
    );
    let m = collect(manual);
    let g = collect(generated);
    println!(
        "id\tdomain states\tgenerated specifications\tmanual specifications\tinteraction support\tstatic scope"
    );
    for e in entries() {
        let domain: usize = e.properties.values().map(Vec::len).product();
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            e.id,
            domain,
            g.get(&e.id).map_or(0, BTreeSet::len),
            m.get(&e.id).map_or(0, BTreeSet::len),
            if e.id == "create:hand_crank" {
                "turn"
            } else if e.id == "create:creative_motor" {
                "kinetic value board (legacy motor and v3 speed_setting)"
            } else if e.id == "create:rotation_speed_controller" {
                "v3 speed_setting; static controller/cog fixtures"
            } else if e.id == "create:sequenced_gearshift" {
                "v3 program, signal; per-tick execution and context"
            } else if e.id.ends_with("_valve_handle") {
                "v3 valve_setting, empty-hand/sneak/dye use; per-tick angle and context"
            } else if e.id == "create:belt" {
                "v3 connector use; casing/pulley; empty endpoint slicing/reform; break cascade"
            } else if e.id == "create:chain_conveyor" {
                "v3 chain_link server packet; relative offsets and loops"
            } else if crate::actions::BLOCKS.contains(&e.id.as_str()) {
                "v3 item use/break; isolated adjacent signal"
            } else {
                "excluded: no interaction adapter"
            },
            e.excluded
                .as_deref()
                .unwrap_or("static node; machine work and player interactions excluded")
        );
        println!(
            "observation scope: {:?}; state phase: {:?}",
            e.observation_scope, e.state_phase
        );
        assert!(!e.source.is_empty());
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contract() {
        let e = entries();
        assert_eq!(e.len(), 73);
        assert_eq!(e.iter().map(|e| &e.id).collect::<BTreeSet<_>>().len(), 73);
        assert!(validate("create:shaft[axis=q]").is_err());
        assert!(validate("create:shaft[axis=x,axis=y]").is_err());
        assert!(validate("create:missing").is_err());
        assert!(validate("create:shaft[wat=true]").is_err());
        assert!(validate("create:shaft[axis=x]").is_ok());
    }
}
