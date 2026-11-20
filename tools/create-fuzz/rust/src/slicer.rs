//! Belt slicer and connector histories (`BeltSlicer.useWrench`/`useConnector`,
//! `BeltConnectorItem.canConnect`): interior splits on both sides of a segment, shortening
//! from either end, lengthening, joins with and without a flipped partner, the maximum length,
//! and the connector's refusals, on belts that carry items while they are cut and joined.
use crate::{
    actions::{Action, Face},
    generate::Rng,
    scenario::{Change, Placed, Pos, Scenario},
    transport::{Input, Stack},
};

fn shaft(pos: Pos) -> Placed {
    Placed {
        pos,
        state: "create:shaft[axis=x,waterlogged=false]".into(),
        motor: None,
    }
}
fn motor(pos: Pos, speed: i32) -> Placed {
    Placed {
        pos,
        state: "create:creative_motor[facing=east]".into(),
        motor: Some(speed),
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
/// A use on the top face at `along` (-0.5..0.5) of the segment along the belt's z axis.
fn use_at(item: &str, along: f64, sneaking: bool) -> Action {
    Action::Use {
        item: item.into(),
        face: Face::Up,
        hit: [0.5, 1., 0.5 + along],
        yaw: 0.,
        pitch: 0.,
        sneaking,
    }
}
fn drop(stack: &str, count: u8) -> Action {
    Action::Item {
        input: Input::DropOn {
            stack: Stack {
                item: stack.into(),
                count,
            },
        },
    }
}

/// Belts run along z over x-axis shafts; the belt's vector is +z when its first click is
/// the lower z shaft.
pub fn generate(seed: u64) -> Scenario {
    let mut r = Rng::new(seed);
    let mut sc = Scenario {
        version: 3,
        seed,
        ticks: 200,
        day_time: 0,
        checkpoints: vec![],
        blocks: vec![],
        events: vec![],
    };
    let speed = if r.chance(0.5) { 32 } else { -32 };
    match crate::generate::index(seed) % 4 {
        0 | 1 => cut(&mut r, &mut sc, speed),
        2 => join(&mut r, &mut sc, speed),
        _ => refusals(&mut r, &mut sc, speed),
    }
    // Settle past the last input (`generate::SETTLE`), as mutation keeps it.
    let last = sc.events.iter().map(|e| e.tick).max().unwrap_or(0);
    sc.ticks = sc.ticks.max(last + crate::generate::SETTLE);
    sc.events.sort_by_key(|e| e.tick);
    sc.checkpoints.sort_unstable();
    sc.checkpoints.dedup();
    sc
}

/// Stacks on a few segments, landing before the cuts so they move while the belt changes.
fn load(r: &mut Rng, sc: &mut Scenario, from: i32, to: i32, tick: u32) {
    let items = [
        "minecraft:iron_ingot",
        "minecraft:gold_ingot",
        "minecraft:diamond",
    ];
    for k in 0..1 + r.below(3) {
        let z = from + r.below((to - from + 1) as u32) as i32;
        sc.events.push(act(
            tick + 2 * k,
            [0, 8, z],
            drop(r.pick(&items), 1 + r.below(16) as u8),
        ));
    }
}

/// One belt: interior splits (creative, or refused in survival), shortening and lengthening.
fn cut(r: &mut Rng, sc: &mut Scenario, speed: i32) {
    let length = 4 + r.below(9) as i32;
    let reversed = r.chance(0.5);
    sc.blocks.push(shaft([0, 8, 0]));
    sc.blocks.push(shaft([0, 8, length - 1]));
    sc.blocks.push(motor([-1, 8, 0], speed));
    let (a, b) = if reversed {
        ([0, 8, length - 1], [0, 8, 0])
    } else {
        ([0, 8, 0], [0, 8, length - 1])
    };
    sc.events
        .push(act(10, a, use_at("create:belt_connector", 0., false)));
    sc.events
        .push(act(20, b, use_at("create:belt_connector", 0., false)));
    let creative = r.chance(0.75);
    if creative {
        sc.events
            .push(act(25, [0, 8, 0], Action::GameMode { creative: true }));
    }
    load(r, sc, 0, length - 1, 30);
    let mut tick = 50;
    for _ in 0..2 + r.below(3) {
        let z = r.below(length as u32) as i32;
        let along = *r.pick(&[-0.3, 0.3, -0.45, 0.45, 0.]);
        let item = if r.chance(0.7) {
            "create:wrench"
        } else {
            "create:belt_connector"
        };
        sc.events
            .push(act(tick, [0, 8, z], use_at(item, along, false)));
        sc.checkpoints.extend([tick, tick + 1, tick + 2, tick + 5]);
        tick += 15 + r.below(10);
    }
    sc.checkpoints.extend([40, 49, tick, 199]);
}

/// Two belts end to end: lengthen one toward the other, then join them (the second flipped
/// when it was clicked the other way), or meet the maximum length.
fn join(r: &mut Rng, sc: &mut Scenario, speed: i32) {
    let first = 2 + r.below(8) as i32;
    let gap = 1 + r.below(2) as i32;
    let span = if r.chance(0.25) { 12 } else { 6 };
    let second = 2 + r.below(span) as i32;
    let start = first + gap;
    for z in [0, first - 1, start, start + second - 1] {
        sc.blocks.push(shaft([0, 8, z]));
    }
    sc.blocks.push(motor([-1, 8, 0], speed));
    sc.blocks.push(motor([-1, 8, start], speed));
    let flip = r.chance(0.5);
    let creative = r.chance(0.5);
    // Clicks 6 ticks apart, past the connector's 5-tick cooldown.
    sc.events.push(act(
        10,
        [0, 8, 0],
        use_at("create:belt_connector", 0., false),
    ));
    sc.events.push(act(
        16,
        [0, 8, first - 1],
        use_at("create:belt_connector", 0., false),
    ));
    let (c, d) = if flip {
        ([0, 8, start + second - 1], [0, 8, start])
    } else {
        ([0, 8, start], [0, 8, start + second - 1])
    };
    sc.events
        .push(act(22, c, use_at("create:belt_connector", 0., false)));
    sc.events
        .push(act(28, d, use_at("create:belt_connector", 0., false)));
    if creative {
        sc.events
            .push(act(30, [0, 8, 0], Action::GameMode { creative: true }));
    }
    load(r, sc, 0, first - 1, 32);
    load(r, sc, start, start + second - 1, 38);
    // Lengthen the first belt's end until it meets the second, then join; or join from the
    // second belt's start toward the first.
    let mut tick = 50;
    let from_second = r.chance(0.3);
    if from_second {
        let z = start;
        // Its START (or, flipped, its END) is at `start`; either way the hovered end is -z.
        let along = -0.45;
        for k in 0..gap {
            sc.events.push(act(
                tick,
                [0, 8, z - k],
                use_at("create:belt_connector", along, false),
            ));
            sc.checkpoints.extend([tick, tick + 1, tick + 3]);
            tick += 12;
        }
        sc.events.push(act(
            tick,
            [0, 8, z - gap],
            use_at("create:belt_connector", along, false),
        ));
    } else {
        for k in 0..gap {
            sc.events.push(act(
                tick,
                [0, 8, first - 1 + k],
                use_at("create:belt_connector", 0.45, false),
            ));
            sc.checkpoints.extend([tick, tick + 1, tick + 3]);
            tick += 12;
        }
        sc.events.push(act(
            tick,
            [0, 8, first - 1 + gap],
            use_at("create:belt_connector", 0.45, false),
        ));
    }
    sc.checkpoints
        .extend([tick, tick + 1, tick + 2, tick + 5, tick + 20]);
    // Shorten the joined belt from one end, then split it.
    tick += 25;
    let total = first + gap + second;
    let end = if r.chance(0.5) { 0 } else { total - 1 };
    let along = if end == 0 { -0.45 } else { 0.45 };
    sc.events.push(act(
        tick,
        [0, 8, end],
        use_at("create:wrench", along, false),
    ));
    sc.checkpoints.extend([tick, tick + 1, tick + 3]);
    tick += 15;
    let z = 1 + r.below(total.max(2) as u32 - 1) as i32;
    sc.events.push(act(
        tick,
        [0, 8, z],
        use_at("create:wrench", *r.pick(&[-0.3, 0.3]), false),
    ));
    sc.checkpoints.extend([40, tick, tick + 1, tick + 3, 199]);
}

/// The connector's refusals: shafts across axes, out of range, an obstacle or a cross
/// shaft between them, opposing driven ends, a forgotten first shaft; and intermediate
/// shafts that become pulleys.
fn refusals(r: &mut Rng, sc: &mut Scenario, speed: i32) {
    let kind = r.below(6);
    let far = match kind {
        1 => 19 + r.below(3) as i32,
        _ => 3 + r.below(8) as i32,
    };
    sc.blocks.push(shaft([0, 8, 0]));
    sc.blocks.push(if kind == 0 {
        Placed {
            pos: [0, 8, far],
            state: "create:shaft[axis=z,waterlogged=false]".into(),
            motor: None,
        }
    } else {
        shaft([0, 8, far])
    });
    sc.blocks.push(motor([-1, 8, 0], speed));
    match kind {
        2 => sc.blocks.push(Placed {
            pos: [0, 8, 1 + r.below(far as u32 - 1) as i32],
            state: "minecraft:stone".into(),
            motor: None,
        }),
        3 => {
            // Opposing driven ends; equal signs are allowed.
            let other = if r.chance(0.7) { -speed } else { speed };
            sc.blocks.push(motor([-1, 8, far], other));
        }
        4 => {
            for z in 1..far {
                if r.chance(0.4) {
                    sc.blocks.push(shaft([0, 8, z]));
                }
            }
        }
        5 => {
            // A shaft of another axis between them is no intermediate shaft.
            sc.blocks.push(Placed {
                pos: [0, 8, far / 2],
                state: "create:shaft[axis=z,waterlogged=false]".into(),
                motor: None,
            });
        }
        _ => {}
    }
    let a = [0, 8, 0];
    let b = [0, 8, far];
    sc.events
        .push(act(10, a, use_at("create:belt_connector", 0., false)));
    if kind == 1 && r.chance(0.5) {
        // Remove the remembered shaft before the second click: it is forgotten and the
        // second shaft is remembered instead.
        sc.events.push(act(14, a, Action::Remove));
    }
    sc.events
        .push(act(20, b, use_at("create:belt_connector", 0., false)));
    // A second attempt from the other shaft, after any refusal.
    sc.events
        .push(act(30, b, use_at("create:belt_connector", 0., false)));
    sc.events
        .push(act(40, a, use_at("create:belt_connector", 0., false)));
    // Within the cooldown of a remembered or connected click: the item does nothing.
    let soon = 41 + r.below(5);
    sc.events
        .push(act(soon, b, use_at("create:belt_connector", 0., false)));
    sc.checkpoints.extend([11, 21, 31, 41, soon + 1, 70, 100]);
    sc.ticks = 120;
}
