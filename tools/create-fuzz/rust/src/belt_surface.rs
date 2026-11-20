//! Belt core histories: dyes (`BeltBlockEntity.applyColor`) applied, repeated, changed and
//! washed off with water, carried to segments the connector adds; casings and the covers a
//! block standing on a cased horizontal segment gives (`updateCoverProperty`); pulleys; and the
//! break cascade, on horizontal, sloped, vertical and sideways belts.
use crate::{
    actions::{Action, Face},
    generate::Rng,
    scenario::{Change, Placed, Pos, Scenario},
};

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
fn use_on(item: &str, face: Face, along: [f64; 3]) -> Action {
    let mut hit = face.point(0.5, 0.5);
    for i in 0..3 {
        hit[i] = (hit[i] + along[i]).clamp(0., 1.);
    }
    Action::Use {
        item: item.into(),
        face,
        hit,
        yaw: 0.,
        pitch: 0.,
        sneaking: false,
    }
}

const DYES: [&str; 4] = [
    "minecraft:red_dye",
    "minecraft:blue_dye",
    "minecraft:lime_dye",
    "minecraft:black_dye",
];

pub fn generate(seed: u64) -> Scenario {
    let mut r = Rng::new(seed);
    let length = 3 + r.below(5) as i32;
    // The belt's shape: horizontal along z, sloped up or down along z, vertical, or sideways
    // (vertical shafts side by side along x).
    let kind = r.below(5);
    let (axis, step) = match kind {
        0 | 1 => ("x", [0, 0, 1]),
        2 => ("x", [0, 1, 1]),
        3 => ("x", [0, 1, 0]),
        _ => ("y", [1, 0, 0]),
    };
    let start = if kind == 1 {
        [0, 8 + length - 1, 0]
    } else {
        [0, 8, 0]
    };
    let step = if kind == 1 { [0, -1, 1] } else { step };
    let at = |k: i32| -> Pos {
        [
            start[0] + step[0] * k,
            start[1] + step[1] * k,
            start[2] + step[2] * k,
        ]
    };
    let shaft = format!("create:shaft[axis={axis},waterlogged=false]");
    let motor_at = if axis == "y" {
        [start[0], start[1] - 1, start[2]]
    } else {
        [start[0] - 1, start[1], start[2]]
    };
    let facing = if axis == "y" { "up" } else { "east" };
    let speed = if r.chance(0.5) { 32 } else { -32 };
    let mut sc = Scenario {
        version: 3,
        seed,
        ticks: 200,
        day_time: 0,
        checkpoints: vec![],
        blocks: vec![
            Placed {
                pos: at(0),
                state: shaft.clone(),
                motor: None,
            },
            Placed {
                pos: at(length - 1),
                state: shaft.clone(),
                motor: None,
            },
            Placed {
                pos: motor_at,
                state: format!("create:creative_motor[facing={facing}]"),
                motor: Some(speed),
            },
        ],
        events: vec![],
    };
    // An intermediate shaft becomes a pulley.
    if length > 3 && r.chance(0.4) && kind != 4 {
        sc.blocks.push(Placed {
            pos: at(1),
            state: shaft.clone(),
            motor: None,
        });
    }
    let (first, second) = if r.chance(0.5) {
        (at(0), at(length - 1))
    } else {
        (at(length - 1), at(0))
    };
    for (tick, p) in [(10, first), (20, second)] {
        sc.events.push(act(
            tick,
            p,
            use_on("create:belt_connector", Face::Up, [0.; 3]),
        ));
    }
    let mut tick = 30;
    // The segment the last event dyed: water washes it only straight after (on an undyed belt
    // the bucket would place water instead).
    let mut dyed: Option<Pos> = None;
    for _ in 0..3 + r.below(4) {
        let k = r.below(length as u32) as i32;
        let mut p = at(k);
        let pick = r.below(8);
        let last = dyed.take();
        match pick {
            0..=2 => {
                // A dye, the same again, or another.
                sc.events
                    .push(act(tick, p, use_on(r.pick(&DYES), Face::Up, [0.; 3])));
                dyed = Some(p);
            }
            // Not on vertical or sideways belts: there the driver's native fluid path and
            // Java part on water a wash leaves (findings f2c), outside this family.
            3 if last.is_some() && kind < 3 => {
                p = last.unwrap();
                sc.events.push(act(
                    tick,
                    p,
                    use_on("minecraft:water_bucket", Face::Up, [0.; 3]),
                ));
            }
            4 => sc.events.push(act(
                tick,
                p,
                use_on(
                    if r.chance(0.5) {
                        "create:andesite_casing"
                    } else {
                        "create:brass_casing"
                    },
                    Face::Up,
                    [0.; 3],
                ),
            )),
            5 if kind != 3 => {
                // A block on the segment (covering it when cased), or taken away.
                let above = [p[0], p[1] + 1, p[2]];
                let s = *r.pick(&[
                    "minecraft:stone",
                    "create:depot[waterlogged=false]",
                    "minecraft:air",
                ]);
                sc.events.push(state(tick, above, s));
            }
            6 => sc
                .events
                .push(act(tick, p, use_on("create:wrench", Face::East, [0.; 3]))),
            _ => sc
                .events
                .push(act(tick, p, use_on("create:shaft", Face::Up, [0.; 3]))),
        }
        sc.checkpoints.extend([tick, tick + 1, tick + 2]);
        tick += 8;
    }
    // The connector carries the colour to the segment it adds.
    if kind == 0 && r.chance(0.6) {
        let end = if first == at(0) {
            at(length - 1)
        } else {
            at(0)
        };
        let along = if end == at(length - 1) { 0.45 } else { -0.45 };
        sc.events.push(act(
            tick,
            end,
            use_on("create:belt_connector", Face::Up, [0., 0., along]),
        ));
        sc.checkpoints.extend([tick, tick + 1, tick + 2]);
        tick += 8;
    }
    // The cascade: a segment breaks, the others become shafts or air.
    if r.chance(0.5) {
        sc.events
            .push(act(tick, at(r.below(length as u32) as i32), Action::Remove));
        sc.checkpoints.extend([tick, tick + 1, tick + 2]);
        tick += 4;
    }
    sc.ticks = tick + 10;
    // Settle past the last input (`generate::SETTLE`), as mutation keeps it.
    let last = sc.events.iter().map(|e| e.tick).max().unwrap_or(0);
    sc.ticks = sc.ticks.max(last + crate::generate::SETTLE);
    sc.checkpoints.push(sc.ticks);
    sc.checkpoints.sort_unstable();
    sc.checkpoints.dedup();
    sc
}
