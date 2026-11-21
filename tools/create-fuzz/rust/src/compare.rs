//! Compares two snapshots block by block.

use crate::scenario::{Kinetic, Snapshot, parse_state};
use crate::transport::{DroppedItem, Placed};

/// Float tolerance: Create's figures are f32 sums of small products.
const EPS: f32 = 1e-3;

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= EPS * a.abs().max(b.abs()).max(1.0)
}

pub fn diff(java: &Snapshot, rust: &Snapshot) -> Vec<String> {
    let mut out = Vec::new();
    for (side, snapshot) in [("java", java), ("rust", rust)] {
        if snapshot
            .unavailable
            .iter()
            .any(|s| s == crate::fluid_world::OUTSIDE)
        {
            out.push(format!(
                "{side} adapter unavailable: {}",
                crate::fluid_world::OUTSIDE
            ));
        }
    }
    if java.item_outcomes != rust.item_outcomes {
        out.push(format!(
            "item outcomes: java {:?} rust {:?}",
            java.item_outcomes, rust.item_outcomes
        ));
    }
    if let (Some(j), Some(r)) = (&java.drops, &rust.drops)
        && !drops_match(j, r)
    {
        out.push(format!("drops: java {j:?} rust {r:?}"));
    }
    if let (Some(j), Some(r)) = (&java.networks, &rust.networks) {
        let normalize = |groups: &[Vec<String>]| {
            let mut groups = groups.to_vec();
            for g in &mut groups {
                g.sort();
            }
            groups.sort();
            groups
        };
        if normalize(&j.partitions) != normalize(&r.partitions) {
            out.push("network partitions differ".into());
        }
        if j.sizes != r.sizes {
            out.push("network sizes differ".into());
        }
    }
    let keys: std::collections::BTreeSet<&String> =
        java.blocks.keys().chain(rust.blocks.keys()).collect();
    for k in keys {
        match (java.blocks.get(k), rust.blocks.get(k)) {
            (Some(j), None) => out.push(format!("{k}: java has {}, rust has air", j.state)),
            (None, Some(r)) => out.push(format!("{k}: java has air, rust has {}", r.state)),
            (Some(j), Some(r)) => {
                if j.fluid_storage != r.fluid_storage {
                    out.push(format!(
                        "{k}: fluid storage: java {:?} rust {:?}",
                        j.fluid_storage, r.fluid_storage
                    ));
                }
                if j.fluid_filter != r.fluid_filter {
                    out.push(format!(
                        "{k}: fluid filter: java {:?} rust {:?}",
                        j.fluid_filter, r.fluid_filter
                    ));
                }
                if j.fluid_machine != r.fluid_machine {
                    out.push(format!(
                        "{k}: fluid machine: java {:?} rust {:?}",
                        j.fluid_machine, r.fluid_machine
                    ));
                }
                out.extend(
                    fluid_network_diff(j.fluid_network.as_ref(), r.fluid_network.as_ref())
                        .into_iter()
                        .map(|d| format!("{k}: {d}")),
                );
                if j.world_fluid != r.world_fluid {
                    out.push(format!(
                        "{k}: world fluid: java {:?} rust {:?}",
                        j.world_fluid, r.world_fluid
                    ));
                }
                if j.fluid_tank != r.fluid_tank {
                    out.push(format!(
                        "{k}: fluid tank: java {:?} rust {:?}",
                        j.fluid_tank, r.fluid_tank
                    ));
                }
                if !transport_equal(j.transport.as_ref(), r.transport.as_ref()) {
                    out.push(format!(
                        "{k}: transport: java {:?} rust {:?}",
                        j.transport, r.transport
                    ));
                }
                if j.conveyor_links != r.conveyor_links {
                    out.push(format!(
                        "{k}: conveyor links: java {:?} rust {:?}",
                        j.conveyor_links, r.conveyor_links
                    ));
                }
                if !crate::persistence::payload::equal(&j.payload, &r.payload) {
                    out.push(format!(
                        "{k}: saved subsystem payload: java {:?} rust {:?}",
                        j.payload, r.payload
                    ));
                }
                if !crate::persistence::equal(&j.saved, &r.saved) {
                    out.push(format!(
                        "{k}: saved kinetic data: java {:?} rust {:?}",
                        j.saved, r.saved
                    ));
                }
                if j.belt != r.belt {
                    out.push(format!("{k}: belt: java {:?} rust {:?}", j.belt, r.belt));
                }
                if j.tunnel != r.tunnel {
                    out.push(format!(
                        "{k}: tunnel: java {:?} rust {:?}",
                        j.tunnel, r.tunnel
                    ));
                }
                if j.funnel != r.funnel {
                    out.push(format!(
                        "{k}: funnel: java {:?} rust {:?}",
                        j.funnel, r.funnel
                    ));
                }
                if !crate::logistics::chute_equal(j.chute.as_ref(), r.chute.as_ref()) {
                    out.push(format!("{k}: chute: java {:?} rust {:?}", j.chute, r.chute));
                }
                if !crate::logistics::ejector_equal(j.ejector.as_ref(), r.ejector.as_ref()) {
                    out.push(format!(
                        "{k}: ejector: java {:?} rust {:?}",
                        j.ejector, r.ejector
                    ));
                }
                if j.bracket != r.bracket {
                    out.push(format!(
                        "{k}: bracket: java {:?} rust {:?}",
                        j.bracket, r.bracket
                    ));
                }
                if let Some(d) = state_diff(&j.state, &r.state)
                    && !crate::allow::trig_state(&j.state, &r.state, r.measured_trig.as_ref())
                {
                    out.push(format!("{k}: state {d}"));
                }
                match (&j.kinetic, &r.kinetic) {
                    (Some(a), Some(b)) => {
                        out.extend(kinetic_diff(a, b).into_iter().map(|d| format!("{k}: {d}")))
                    }
                    (Some(_), None) => {
                        out.push(format!("{k}: java kinetic, rust not ({})", j.state))
                    }
                    (None, Some(_)) => {
                        out.push(format!("{k}: rust kinetic, java not ({})", j.state))
                    }
                    (None, None) => {}
                }
            }
            (None, None) => {}
        }
    }
    let ticks: std::collections::BTreeSet<_> = java
        .checkpoints
        .keys()
        .chain(rust.checkpoints.keys())
        .collect();
    for t in ticks {
        match (java.checkpoints.get(t), rust.checkpoints.get(t)) {
            (Some(j), Some(r)) => {
                out.extend(diff(j, r).into_iter().map(|d| format!("tick {t}: {d}")))
            }
            _ => out.push(format!("tick {t}: missing checkpoint")),
        }
    }
    for n in &rust.notes {
        out.push(format!("rust note: {n}"));
    }
    out
}

/// The same item entities, in any order. A drop placed exactly on one side has one at the same
/// place and motion within the tolerance on the other. The rest agree in how many of each item
/// joined in each tick inside each block, but for drops that flew a tick before they were
/// observed, which only agree in how many of each item joined in each tick.
fn drops_match(a: &[DroppedItem], b: &[DroppedItem]) -> bool {
    use std::collections::BTreeMap;
    let mut a: Vec<&DroppedItem> = a.iter().collect();
    let mut b: Vec<&DroppedItem> = b.iter().collect();
    if !take_exact(&mut a, &mut b) || !take_exact(&mut b, &mut a) {
        return false;
    }
    type Key = (u32, String, Option<[i32; 3]>);
    let add = |m: &mut BTreeMap<Key, u32>, d: &DroppedItem, block: bool| {
        let at = block.then(|| d.pos.map(|v| v.floor() as i32));
        *m.entry((d.tick, d.stack.item.clone(), at)).or_default() += u32::from(d.stack.count);
    };
    let flown = |d: &DroppedItem| d.placed == Some(Placed::Flown);
    let (mut ab, mut bb) = (BTreeMap::new(), BTreeMap::new());
    for d in a.iter().filter(|d| !flown(d)) {
        add(&mut ab, d, true);
    }
    for d in b.iter().filter(|d| !flown(d)) {
        add(&mut bb, d, true);
    }
    let (mut a_left, mut b_left) = (BTreeMap::new(), BTreeMap::new());
    for (k, n) in &ab {
        let m = bb.get(k).copied().unwrap_or(0);
        if *n > m {
            *a_left.entry((k.0, k.1.clone(), None)).or_default() += n - m;
        }
    }
    for (k, n) in &bb {
        let m = ab.get(k).copied().unwrap_or(0);
        if *n > m {
            *b_left.entry((k.0, k.1.clone(), None)).or_default() += n - m;
        }
    }
    let (mut a_flown, mut b_flown) = (BTreeMap::new(), BTreeMap::new());
    for d in a.iter().filter(|d| flown(d)) {
        add(&mut a_flown, d, false);
    }
    for d in b.iter().filter(|d| flown(d)) {
        add(&mut b_flown, d, false);
    }
    a_left == b_flown && b_left == a_flown
}

/// Pairs each of `known`'s exactly placed drops with one of `other`'s at the same tick, stack,
/// place and motion, taking both out. False when one has none.
fn take_exact(known: &mut Vec<&DroppedItem>, other: &mut Vec<&DroppedItem>) -> bool {
    let near =
        |a: &[f64; 3], b: &[f64; 3]| a.iter().zip(b).all(|(x, y)| close(*x as f32, *y as f32));
    let mut i = 0;
    while i < known.len() {
        let k = known[i];
        if k.placed != Some(Placed::Exact) {
            i += 1;
            continue;
        }
        let Some(j) = other.iter().position(|o| {
            o.tick == k.tick
                && o.stack == k.stack
                && near(&o.pos, &k.pos)
                && near(&o.motion, &k.motion)
        }) else {
            return false;
        };
        other.remove(j);
        known.remove(i);
    }
    true
}

/// The block names differ, or a property both sides name has another value.
fn state_diff(j: &str, r: &str) -> Option<String> {
    let (jn, jp) = parse_state(j);
    let (rn, rp) = parse_state(r);
    if jn != rn {
        return Some(format!("java {j} rust {r}"));
    }
    let differing: Vec<String> = rp
        .iter()
        .filter(|(k, _)| {
            !crate::catalogue::entries()
                .iter()
                .find(|e| e.id == jn)
                .is_some_and(|e| e.observation_scope.contains_key(k))
        })
        .filter_map(|(k, v)| {
            let jv = &jp.iter().find(|(jk, _)| jk == k)?.1;
            (jv != v).then(|| format!("{k}: java {jv} rust {v}"))
        })
        .collect();
    (!differing.is_empty()).then(|| differing.join(", "))
}

fn kinetic_diff(j: &Kinetic, r: &Kinetic) -> Vec<String> {
    let mut out = Vec::new();
    let mut f = |name: &str, a: f32, b: f32| {
        if !close(a, b) {
            out.push(format!("{name}: java {a} rust {b}"));
        }
    };
    f("speed", j.speed, r.speed);
    f("theoretical", j.theoretical, r.theoretical);
    f("stress", j.stress, r.stress);
    f("capacity", j.capacity, r.capacity);
    f("impact", j.impact, r.impact);
    f("added_capacity", j.added_capacity, r.added_capacity);
    f("generated", j.generated, r.generated);
    if let (Some(jf), Some(rf)) = (&j.faces, &r.faces) {
        for (face, a) in jf {
            let b = rf.get(face).copied().unwrap_or(f32::NAN);
            if !close(*a, b) {
                out.push(format!("{face} face: java {a} rust {b}"));
            }
        }
    }
    match (&j.valve, &r.valve) {
        (Some(a), Some(b))
            if a.angle_unavailable == b.angle_unavailable
                && a.selected == b.selected
                && a.in_use == b.in_use
                && match (a.angle, b.angle) {
                    (Some(a), Some(b)) => close(a, b),
                    (None, None) => true,
                    _ => false,
                } => {}
        (None, None) => {}
        _ => out.push(format!("valve: java {:?} rust {:?}", j.valve, r.valve)),
    }
    match (&j.sequencer, &r.sequencer) {
        (Some(a), Some(b)) => {
            if (a.current, a.duration, a.timer, a.powered_before)
                != (b.current, b.duration, b.timer, b.powered_before)
                || !close(a.progress, b.progress)
            {
                out.push(format!("sequencer: java {a:?} rust {b:?}"));
            }
        }
        (None, None) => {}
        _ => out.push(format!(
            "sequencer: java {:?} rust {:?}",
            j.sequencer, r.sequencer
        )),
    }
    if j.has_source != r.has_source {
        out.push(format!(
            "has_source: java {} rust {}",
            j.has_source, r.has_source
        ));
    }
    if j.comparator != r.comparator {
        out.push(format!(
            "comparator: java {:?} rust {:?}",
            j.comparator, r.comparator
        ));
    }
    for (name, a, b) in [
        ("speed_setting", j.speed_setting, r.speed_setting),
        ("chain_modifier", j.chain_modifier, r.chain_modifier),
        ("sequence_angle", j.sequence_angle, r.sequence_angle),
        (
            "sequence_distance",
            j.sequence_distance,
            r.sequence_distance,
        ),
    ] {
        match (a, b) {
            (Some(a), Some(b)) if close(a, b) => {}
            (None, None) => {}
            _ => out.push(format!("{name}: java {a:?} rust {b:?}")),
        }
    }
    if j.overstressed != r.overstressed {
        out.push(format!(
            "overstressed: java {} rust {}",
            j.overstressed, r.overstressed
        ));
    }
    out
}

fn transport_equal(
    a: Option<&crate::transport::Contents>,
    b: Option<&crate::transport::Contents>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.kind == b.kind
                && a.outputs == b.outputs
                && a.items.len() == b.items.len()
                && a.items.iter().zip(&b.items).all(|(a, b)| {
                    a.stack == b.stack
                        && a.locked == b.locked
                        && a.pending == b.pending
                        && a.stalled == b.stalled
                        && close(a.position, b.position)
                })
        }
        _ => false,
    }
}

fn fluid_network_diff(
    j: Option<&crate::fluid_network::Observation>,
    r: Option<&crate::fluid_network::Observation>,
) -> Vec<String> {
    let (j, r) = match (j, r) {
        (Some(j), Some(r)) => (j, r),
        (None, None) => return vec![],
        _ => return vec!["fluid network presence differs".into()],
    };
    let mut out = Vec::new();
    match (j.pump_pressure, r.pump_pressure) {
        (Some(a), Some(b)) if close(a, b) => {}
        (None, None) => {}
        (a, b) => out.push(format!("pump pressure: java {a:?} rust {b:?}")),
    }
    let keys: std::collections::BTreeSet<_> =
        j.connections.keys().chain(r.connections.keys()).collect();
    for side in keys {
        match (j.connections.get(side), r.connections.get(side)) {
            (Some(a), Some(b)) => {
                for (i, name) in ["inbound", "outward"].iter().enumerate() {
                    if !close(a.pressure[i], b.pressure[i]) {
                        out.push(format!(
                            "{side} {name} pressure: java {} rust {}",
                            a.pressure[i], b.pressure[i]
                        ));
                    }
                }
                if a.buffer != b.buffer {
                    out.push(format!(
                        "{side} open-end buffer: java {:?} rust {:?}",
                        a.buffer, b.buffer
                    ));
                }
                match (&a.flow, &b.flow) {
                    (Some(a), Some(b))
                        if a.fluid == b.fluid
                            && a.inbound == b.inbound
                            && a.complete == b.complete
                            && close(a.progress, b.progress) => {}
                    (None, None) => {}
                    (a, b) => out.push(format!("{side} flow: java {a:?} rust {b:?}")),
                }
            }
            _ => out.push(format!("{side} fluid connection presence differs")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::Networks;
    #[test]
    fn scope_excludes_only_declared_outputs() {
        assert!(
            state_diff(
                "create:fluid_valve[enabled=true,axis=x]",
                "create:fluid_valve[enabled=false,axis=x]"
            )
            .is_some()
        );
        assert!(
            state_diff(
                "create:fluid_valve[enabled=true,axis=x]",
                "create:fluid_valve[enabled=false,axis=y]"
            )
            .is_some()
        );
        assert!(
            state_diff(
                "create:display_board[up=true,facing=north]",
                "create:display_board[up=false,facing=south]"
            )
            .is_some()
        );
        assert!(crate::catalogue::validate("create:shaft[waterlogged=true]").is_ok());
        assert!(
            state_diff(
                "create:shaft[waterlogged=true]",
                "create:shaft[waterlogged=false]"
            )
            .is_some()
        );
        for e in crate::catalogue::entries() {
            for (property, reason) in &e.observation_scope {
                assert!(e.properties.contains_key(property));
                assert!(!reason.is_empty());
            }
        }
    }
    #[test]
    fn partitions_compare_membership_independently_of_order() {
        let mut j = Snapshot {
            networks: Some(Networks {
                partitions: vec![vec!["a".into(), "b".into()], vec!["c".into()]],
                ..Networks::default()
            }),
            ..Snapshot::default()
        };
        let r = Snapshot {
            networks: Some(Networks {
                partitions: vec![vec!["c".into()], vec!["b".into(), "a".into()]],
                ..Networks::default()
            }),
            ..Snapshot::default()
        };
        assert!(diff(&j, &r).is_empty());
        j.networks.as_mut().unwrap().partitions =
            vec![vec!["a".into(), "c".into()], vec!["b".into()]];
        assert_eq!(diff(&j, &r), ["network partitions differ"]);
    }
    #[test]
    fn drops_compare_by_how_they_were_placed() {
        use crate::transport::{DroppedItem, Placed, Stack};
        let drop = |tick, item: &str, count, x, placed| DroppedItem {
            tick,
            stack: Stack {
                item: item.into(),
                count,
            },
            pos: [x, 9.0625, 0.5],
            motion: [0.1, -0.2, 0.0],
            placed,
        };
        let java = |v| Snapshot {
            drops: Some(v),
            ..Snapshot::default()
        };
        let j = java(vec![
            drop(4, "a", 1, 1.5, None),
            drop(3, "b", 2, 2.3, None),
            drop(3, "b", 1, 2.9, None),
            drop(5, "c", 1, 7.2, None),
        ]);
        let mut r = java(vec![
            drop(3, "b", 3, 2.5, Some(Placed::Spread)),
            drop(4, "a", 1, 1.500_000_1, Some(Placed::Exact)),
            drop(5, "c", 1, 6.9, Some(Placed::Flown)),
        ]);
        assert!(diff(&j, &r).is_empty());
        assert!(diff(&r, &j).is_empty());
        assert!(diff(&j, &j).is_empty());
        r.drops.as_mut().unwrap()[1].pos[0] = 1.6;
        assert!(diff(&j, &r)[0].starts_with("drops:"));
        r.drops.as_mut().unwrap()[1].pos[0] = 1.5;
        r.drops.as_mut().unwrap()[0].pos[0] = 3.5;
        assert!(diff(&j, &r)[0].starts_with("drops:"));
        r.drops.as_mut().unwrap()[0].pos[0] = 2.5;
        r.drops.as_mut().unwrap()[2].stack.count = 2;
        assert!(diff(&j, &r)[0].starts_with("drops:"));
        r.drops = None;
        assert!(diff(&j, &r).is_empty());
    }
    #[test]
    fn checkpoints_and_air_positions_are_compared() {
        let mut j = Snapshot::default();
        let mut r = Snapshot::default();
        j.checkpoints.insert(1, Snapshot::default());
        r.checkpoints.insert(
            1,
            Snapshot {
                blocks: [(
                    "2,8,0".into(),
                    crate::scenario::BlockSnap {
                        state: "create:shaft[axis=x]".into(),
                        ..Default::default()
                    },
                )]
                .into(),
                ..Default::default()
            },
        );
        assert!(diff(&j, &r)[0].starts_with("tick 1: 2,8,0:"));
    }
}
