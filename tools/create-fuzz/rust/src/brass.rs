//! Brass tunnel distribution histories: one to three parallel belts along z with brass tunnels
//! over the same row (a connected group, `isConnected`), depots or a belt across beside the
//! group's outer tunnels, stacks fed by the belts, by a belt ending into the row from the side
//! and by item handler inserts (`BrassTunnelItemHandler`), the selection mode picked on a value
//! board (every mode but `RANDOMIZE`, whose start is `level.random`), sided filters set by a
//! click on their slot, outputs taken away while a stack waits, the group's stacks taken by an
//! empty hand, reloads while a stack waits, and a tunnel of the row placed after the mode was
//! picked (`updateTunnelConnections` hands it on).
use crate::{
    actions::{Action, Face},
    generate::Rng,
    scenario::{Change, Placed, Pos, Scenario},
    transport::{Input, Stack},
};
use create_engine::kinetics::Dir;

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
fn use_at(item: &str, face: Face, hit: [f64; 3], sneaking: bool) -> Action {
    Action::Use {
        item: item.into(),
        face,
        hit,
        yaw: 0.,
        pitch: 0.,
        sneaking,
    }
}
fn use_on(item: &str, face: Face) -> Action {
    use_at(item, face, face.point(0.5, 0.5), false)
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
fn face_of(d: Dir) -> Face {
    match d {
        Dir::Down => Face::Down,
        Dir::Up => Face::Up,
        Dir::North => Face::North,
        Dir::South => Face::South,
        Dir::West => Face::West,
        Dir::East => Face::East,
    }
}
/// Where side `d`'s filter slot is (`BrassTunnelFilterSlot`, 8, 13, 15.5 px on the south side
/// turned to `d`), in the block.
fn filter_hit(d: Dir) -> [f64; 3] {
    create_engine::board::sided_offset([0.5, 13.0 / 16.0, 15.5 / 16.0], d)
}

const ITEMS: [&str; 3] = [
    "minecraft:iron_ingot",
    "minecraft:gold_ingot",
    "minecraft:diamond",
];
/// `SelectionMode` ordinals but `RANDOMIZE` (5).
const MODES: [i32; 6] = [0, 1, 2, 3, 4, 6];

pub fn generate(seed: u64) -> Scenario {
    let mut r = Rng::new(seed);
    let length = 6 + r.below(4) as i32;
    let speed = if r.chance(0.5) { 32 } else { -32 };
    let belts = 1 + r.below(3) as i32;
    let mut sc = Scenario {
        version: 3,
        seed,
        ticks: 300,
        day_time: 0,
        checkpoints: vec![],
        blocks: vec![placed(
            [-1, 8, 0],
            "create:creative_motor[facing=east]",
            Some(speed),
        )],
        events: vec![],
    };
    // Parallel belts at x = 0..belts, their pulleys on one line of x-axis shafts.
    for x in 0..belts {
        for z in [0, length - 1] {
            sc.blocks.push(placed(
                [x, 8, z],
                "create:shaft[axis=x,waterlogged=false]",
                None,
            ));
        }
        sc.events.push(act(
            8 + 12 * x as u32,
            [x, 8, 0],
            use_on("create:belt_connector", Face::Up),
        ));
        sc.events.push(act(
            14 + 12 * x as u32,
            [x, 8, length - 1],
            use_on("create:belt_connector", Face::Up),
        ));
    }
    // The belts run toward +z with a positive speed; a stone past their ends holds what
    // the tunnels let through.
    let (behind, past) = if speed > 0 {
        (0, length)
    } else {
        (length - 1, -1)
    };
    for x in 0..belts {
        sc.blocks
            .push(placed([x, 8, past], "minecraft:stone", None));
    }
    let at = 2 + r.below((length - 4) as u32) as i32;
    // Beside the row: a depot, a belt across (its own motor), or nothing.
    let mut outputs = Vec::new();
    let mut feeder = None;
    let mut feed_click = 8 + 12 * belts as u32;
    for x in [-1, belts] {
        match r.below(4) {
            0 => {}
            1 | 2 => {
                sc.blocks
                    .push(placed([x, 8, at], "create:depot[waterlogged=false]", None));
                outputs.push([x, 8, at]);
            }
            _ => {
                let dir = if x < 0 { -1 } else { 1 };
                let far = [x + dir * 3, 8, at];
                sc.blocks.push(placed(
                    [x, 8, at],
                    "create:shaft[axis=z,waterlogged=false]",
                    None,
                ));
                sc.blocks
                    .push(placed(far, "create:shaft[axis=z,waterlogged=false]", None));
                sc.blocks
                    .push(placed([far[0] + dir, 8, at], "minecraft:stone", None));
                let side = if r.chance(0.5) { 32 } else { -32 };
                sc.blocks.push(placed(
                    [far[0], 8, at - 1],
                    "create:creative_motor[facing=south]",
                    Some(side),
                ));
                sc.events.push(act(
                    feed_click,
                    [x, 8, at],
                    use_on("create:belt_connector", Face::Up),
                ));
                sc.events.push(act(
                    feed_click + 6,
                    far,
                    use_on("create:belt_connector", Face::Up),
                ));
                feed_click += 12;
                // A belt into the row is fed from its far end, one away from the row
                // takes what the tunnels hand out.
                feeder = Some((far, [x, 8, at]));
                outputs.push([x, 8, at]);
            }
        }
    }
    // The tunnels: the row, or all but one placed later (after the mode is picked).
    let late = belts > 1 && r.chance(0.4);
    let base = feed_click + 4;
    let first = if late { 1 } else { belts };
    for x in 0..first {
        sc.events.push(act(
            base + x as u32,
            [x, 8, at],
            use_on("create:brass_tunnel", Face::Up),
        ));
    }
    sc.checkpoints
        .extend([base + 3, base + 4, base + 5, base + 10]);
    // The mode, picked on the first tunnel's board.
    let mode = *r.pick(&MODES);
    sc.events.push(act(
        base + 8,
        [0, 9, at],
        Action::ScrollSetting {
            row: 0,
            value: mode,
        },
    ));
    if late {
        for x in first..belts {
            sc.events.push(act(
                base + 11 + x as u32,
                [x, 8, at],
                use_on("create:brass_tunnel", Face::Up),
            ));
        }
        sc.checkpoints.extend(base + 11..base + 16);
    }
    // Filters on some of the tunnels' sides: a held item clicked on the slot.
    if r.chance(0.5) {
        let x = r.below(belts as u32) as i32;
        let side = *r.pick(&[Dir::East, Dir::West, Dir::North, Dir::South]);
        sc.events.push(act(
            base + 16,
            [x, 9, at],
            use_at(r.pick(&ITEMS), face_of(side), filter_hit(side), false),
        ));
        sc.checkpoints.push(base + 17);
    }
    // Stacks down the belts toward the row.
    let mut tick = base + 20;
    let feeds = 2 + r.below(4);
    for _ in 0..feeds {
        let x = r.below(belts as u32) as i32;
        let count = if r.chance(0.3) {
            1
        } else {
            2 + r.below(15) as u8
        };
        sc.events
            .push(act(tick, [x, 8, behind], drop(r.pick(&ITEMS), count)));
        tick += 3 + r.below(20);
    }
    // A stack fed into the row from the side by the belt across (when it runs toward it).
    if let Some((far, _)) = feeder {
        sc.events
            .push(act(tick, far, drop(r.pick(&ITEMS), 1 + r.below(16) as u8)));
        tick += 10;
    }
    // A stack straight into a tunnel's item handler.
    if r.chance(0.5) {
        let x = r.below(belts as u32) as i32;
        sc.events.push(act(
            tick,
            [x, 9, at],
            Action::Capability {
                side: None,
                slot: 0,
                insert: Some(Stack {
                    item: r.pick(&ITEMS).to_string(),
                    count: 1 + r.below(64) as u8,
                }),
                extract: None,
                simulate: r.chance(0.2),
            },
        ));
        tick += 2;
    }
    let travel = 15 * length as u32;
    sc.checkpoints
        .extend(base + 20..(tick + travel).min(base + 170));
    tick += travel;
    // The mode changes while stacks flow.
    if r.chance(0.5) {
        sc.events.push(act(
            tick,
            [r.below(belts as u32) as i32, 9, at],
            Action::ScrollSetting {
                row: 0,
                value: *r.pick(&MODES),
            },
        ));
        sc.events
            .push(act(tick + 2, [0, 8, behind], drop(r.pick(&ITEMS), 16)));
        sc.checkpoints.extend(tick..tick + 40);
        tick += 40;
    }
    // An output goes away while a stack waits, or comes back.
    if let Some(&o) = outputs.first()
        && r.chance(0.5)
    {
        sc.events
            .push(act(tick, [0, 8, behind], drop(r.pick(&ITEMS), 12)));
        let gone = tick + 12 + r.below(8 * length as u32);
        sc.events.push(state(gone, o, "minecraft:air"));
        sc.checkpoints.extend(tick..gone + 20);
        tick = gone + 20;
        if r.chance(0.5) {
            sc.events
                .push(state(tick, o, "create:depot[waterlogged=false]"));
            sc.checkpoints.extend(tick..tick + 5);
            tick += 6;
        }
    }
    // A reload while a stack waits in a tunnel.
    if r.chance(0.4) {
        sc.events
            .push(act(tick, [0, 8, behind], drop(r.pick(&ITEMS), 9)));
        let at_reload = tick + 15 * (at - behind).unsigned_abs() + r.below(14);
        sc.events.push(act(at_reload, [-1, 8, 0], Action::Reload));
        sc.checkpoints.extend(tick..at_reload + 20);
        tick = at_reload + 20;
    }
    // An empty hand takes the group's stacks (off the mode slot on top).
    if r.chance(0.4) {
        sc.events
            .push(act(tick, [0, 8, behind], drop(r.pick(&ITEMS), 5)));
        let grab = tick + 15 * (at - behind).unsigned_abs() + 1 + r.below(4);
        sc.events.push(act(
            grab,
            [0, 9, at],
            use_at("minecraft:air", Face::Up, [0.1, 1.0, 0.1], false),
        ));
        sc.checkpoints.extend(grab - 2..grab + 4);
        tick = grab + 5;
    }
    // The row loses a tunnel: a sneak wrench takes one.
    if r.chance(0.3) {
        sc.events.push(act(
            tick,
            [r.below(belts as u32) as i32, 9, at],
            use_at("create:wrench", Face::Up, [0.1, 1.0, 0.1], true),
        ));
        sc.events
            .push(act(tick + 1, [0, 8, behind], drop(r.pick(&ITEMS), 7)));
        sc.checkpoints.extend(tick..tick + 30);
        tick += 30;
    }
    let last = sc.events.iter().map(|e| e.tick).max().unwrap_or(0);
    sc.ticks = sc.ticks.max(tick + 10).max(last + crate::generate::SETTLE);
    sc.events.sort_by_key(|e| e.tick);
    sc.checkpoints.retain(|t| *t <= sc.ticks);
    sc.checkpoints.sort_unstable();
    sc.checkpoints.dedup();
    sc
}
