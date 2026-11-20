//! Tunnel histories over belts: andesite and brass tunnels placed by the player on a belt
//! (casing it), their shapes from the depots and belts beside the segment (`getTunnelState`)
//! and from tunnels on both ends (windows), the wrench's window toggle, andesite junctions
//! handing single items to their side outputs, and the tunnel breaking with its casing or belt.
use crate::{
    actions::{Action, Face},
    generate::Rng,
    scenario::{Change, Placed, Pos, Scenario},
    transport::{Input, Stack},
};

fn placed(pos: Pos, state: &str, motor: Option<i32>) -> Placed {
    Placed {
        pos,
        state: state.into(),
        motor,
    }
}
fn act(tick: u32, pos: Pos, action: Action) -> Change {
    Change {
        tick,
        pos,
        action: Some(action),
        state: None,
        motor: None,
        turn: None,
    }
}
fn state(tick: u32, pos: Pos, s: &str) -> Change {
    Change {
        tick,
        pos,
        action: None,
        state: Some(s.into()),
        motor: None,
        turn: None,
    }
}
fn use_on(item: &str, face: Face, sneaking: bool) -> Action {
    Action::Use {
        item: item.into(),
        face,
        hit: face.point(0.5, 0.5),
        yaw: 0.,
        pitch: 0.,
        sneaking,
    }
}
fn drop(item: &str, count: u8) -> Action {
    Action::Item {
        input: Input::DropOn {
            stack: Stack {
                item: item.into(),
                count,
            },
        },
    }
}

/// A belt along z from z=0 to z=length-1 over x-axis shafts, driven from z=0; tunnels over
/// segments `at` and beyond, side outputs beside them.
pub fn generate(seed: u64) -> Scenario {
    let mut r = Rng::new(seed);
    let length = 5 + r.below(5) as i32;
    let speed = if r.chance(0.5) { 32 } else { -32 };
    let mut sc = Scenario {
        version: 3,
        seed,
        ticks: 260,
        day_time: 0,
        checkpoints: vec![],
        blocks: vec![
            placed([0, 8, 0], "create:shaft[axis=x,waterlogged=false]", None),
            placed(
                [0, 8, length - 1],
                "create:shaft[axis=x,waterlogged=false]",
                None,
            ),
            placed(
                [-1, 8, 0],
                "create:creative_motor[facing=east]",
                Some(speed),
            ),
        ],
        events: vec![],
    };
    sc.events.push(act(
        10,
        [0, 8, 0],
        use_on("create:belt_connector", Face::Up, false),
    ));
    sc.events.push(act(
        20,
        [0, 8, length - 1],
        use_on("create:belt_connector", Face::Up, false),
    ));
    let at = 2 + r.below((length - 3) as u32) as i32;
    // Outputs beside the tunnel's segment: depots, or a belt across it with its own drive.
    let brass = r.chance(0.2);
    let mut outputs = Vec::new();
    if !brass {
        for x in [1, -1] {
            match r.below(3) {
                0 => {}
                1 => {
                    sc.blocks
                        .push(placed([x, 8, at], "create:depot[waterlogged=false]", None));
                    outputs.push([x, 8, at]);
                }
                _ => {
                    // A belt across, running away from the main belt or into it.
                    let far = [x * 4, 8, at];
                    sc.blocks.push(placed(
                        [x, 8, at],
                        "create:shaft[axis=z,waterlogged=false]",
                        None,
                    ));
                    sc.blocks
                        .push(placed(far, "create:shaft[axis=z,waterlogged=false]", None));
                    let side = if r.chance(0.7) { 32 } else { -32 };
                    sc.blocks.push(placed(
                        [x * 4, 8, at - 1],
                        "create:creative_motor[facing=south]",
                        Some(side),
                    ));
                    sc.events.push(act(
                        22,
                        [x, 8, at],
                        use_on("create:belt_connector", Face::Up, false),
                    ));
                    sc.events.push(act(
                        28,
                        far,
                        use_on("create:belt_connector", Face::Up, false),
                    ));
                    outputs.push([x, 8, at]);
                }
            }
        }
    }
    let kind = if brass {
        "create:brass_tunnel"
    } else {
        "create:andesite_tunnel"
    };
    // Optionally cased first (the tunnel keeps a casing it finds), then the tunnel.
    if r.chance(0.3) {
        sc.events.push(act(
            30,
            [0, 8, at],
            use_on(
                if r.chance(0.5) {
                    "create:andesite_casing"
                } else {
                    "create:brass_casing"
                },
                Face::Up,
                false,
            ),
        ));
    }
    sc.events
        .push(act(32, [0, 8, at], use_on(kind, Face::Up, false)));
    // A second tunnel on a neighbouring segment: windows on both ends need tunnels there.
    let neighbours = r.below(3) as i32;
    for k in 1..=neighbours {
        let z = at + if k == 1 { 1 } else { -1 };
        if (1..length - 1).contains(&z) {
            sc.events.push(act(
                32 + 2 * k as u32,
                [0, 8, z],
                use_on(kind, Face::Up, false),
            ));
        }
    }
    sc.checkpoints.extend([31, 33, 34, 37, 40]);
    // Stacks behind the tunnel in the way the belt moves: a junction splits them.
    let behind = if speed > 0 { 0 } else { length - 1 };
    let items = [
        "minecraft:iron_ingot",
        "minecraft:gold_ingot",
        "minecraft:diamond",
    ];
    for k in 0..1 + r.below(3) {
        sc.events.push(act(
            42 + 12 * k,
            [0, 8, behind],
            drop(r.pick(&items), 1 + r.below(16) as u8),
        ));
    }
    sc.checkpoints.extend(60..=110);
    // The wrench toggles a straight tunnel's window; a sneak wrench removes it.
    let mut tick = 120;
    if r.chance(0.6) {
        sc.events.push(act(
            tick,
            [0, 9, at],
            use_on("create:wrench", Face::Up, false),
        ));
        sc.checkpoints.extend([tick, tick + 1]);
        tick += 6;
    }
    // An output goes away (the junction narrows) or comes back.
    if let Some(&o) = outputs.first()
        && r.chance(0.6)
    {
        sc.events.push(state(tick, o, "minecraft:air"));
        sc.checkpoints.extend([tick, tick + 1, tick + 2]);
        tick += 8;
        if r.chance(0.5) {
            sc.events
                .push(state(tick, o, "create:depot[waterlogged=false]"));
            sc.checkpoints.extend([tick, tick + 1, tick + 2]);
            tick += 8;
        }
    }
    // More stacks through the changed tunnel.
    sc.events.push(act(
        tick,
        [0, 8, behind],
        drop(r.pick(&items), 2 + r.below(14) as u8),
    ));
    sc.checkpoints.extend((tick..tick + 40).step_by(2));
    tick += 45;
    // The tunnel loses its hold: the casing comes off (a side wrench), its belt segment
    // breaks, or a sneak wrench takes it.
    match r.below(4) {
        0 => {
            sc.events.push(act(
                tick,
                [0, 8, at],
                use_on("create:wrench", Face::East, false),
            ));
        }
        1 => sc.events.push(act(tick, [0, 8, at], Action::Remove)),
        2 => sc.events.push(act(
            tick,
            [0, 9, at],
            use_on("create:wrench", Face::Up, true),
        )),
        _ => {}
    }
    sc.checkpoints.extend([tick, tick + 1, tick + 2, tick + 5]);
    sc.ticks = sc.ticks.max(tick + 10);
    // Settle past the last input (`generate::SETTLE`), as mutation keeps it.
    let last = sc.events.iter().map(|e| e.tick).max().unwrap_or(0);
    sc.ticks = sc.ticks.max(last + crate::generate::SETTLE);
    sc.events.sort_by_key(|e| e.tick);
    sc.checkpoints.retain(|t| *t <= sc.ticks);
    sc.checkpoints.sort_unstable();
    sc.checkpoints.dedup();
    sc
}
