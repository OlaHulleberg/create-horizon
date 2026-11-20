//! Depot item access as other blocks make it (`DepotItemHandler` through the block's item
//! capability): simulated and real inserts into the held slot, refusals while it holds a stack
//! or into an output slot, partial and whole extracts, mixed with belt handoffs into the depot
//! and player pickups; and the item access a belt segment gives (`ItemHandlerBeltSegment`).
use crate::{
    actions::{Action, Face},
    generate::Rng,
    scenario::{Change, Placed, Pos, Scenario},
    transport::{Input, Stack},
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
fn stack(item: &str, count: u8) -> Stack {
    Stack {
        item: item.into(),
        count,
    }
}
fn access(
    side: Option<Face>,
    slot: u8,
    insert: Option<Stack>,
    extract: Option<u8>,
    sim: bool,
) -> Action {
    Action::Capability {
        side,
        slot,
        insert,
        extract,
        simulate: sim,
    }
}

pub fn generate(seed: u64) -> Scenario {
    let mut r = Rng::new(seed);
    let depot = [0, 8, 0];
    let mut sc = Scenario {
        version: 3,
        seed,
        ticks: 200,
        day_time: 0,
        checkpoints: vec![],
        blocks: vec![Placed {
            pos: depot,
            state: "create:depot[waterlogged=false]".into(),
            motor: None,
        }],
        events: vec![],
    };
    // A belt ending at the depot hands stacks into it.
    let fed = r.chance(0.5);
    if fed {
        for z in [-4, -1] {
            sc.blocks.push(Placed {
                pos: [0, 8, z],
                state: "create:shaft[axis=x,waterlogged=false]".into(),
                motor: None,
            });
        }
        sc.blocks.push(Placed {
            pos: [-1, 8, -4],
            state: "create:creative_motor[facing=east]".into(),
            motor: Some(32),
        });
        for (tick, z) in [(10, -4), (20, -1)] {
            sc.events.push(act(
                tick,
                [0, 8, z],
                Action::Use {
                    item: "create:belt_connector".into(),
                    face: Face::Up,
                    hit: [0.5, 1., 0.5],
                    yaw: 0.,
                    pitch: 0.,
                    sneaking: false,
                },
            ));
        }
    }
    let sides = [
        None,
        Some(Face::Up),
        Some(Face::Down),
        Some(Face::North),
        Some(Face::East),
    ];
    let items = [
        ("minecraft:iron_ingot", 64),
        ("minecraft:gold_ingot", 64),
        ("minecraft:ender_pearl", 16),
    ];
    let mut tick = 30;
    for _ in 0..6 + r.below(6) {
        let side = *r.pick(&sides);
        let sim = r.chance(0.3);
        let a = match r.below(7) {
            0..=2 => {
                let (item, max) = *r.pick(&items);
                access(
                    side,
                    0,
                    Some(stack(item, 1 + r.below(max) as u8)),
                    None,
                    sim,
                )
            }
            3 | 4 => access(side, 0, None, Some(1 + r.below(64) as u8), sim),
            5 => access(
                side,
                1 + r.below(8) as u8,
                None,
                Some(1 + r.below(64) as u8),
                sim,
            ),
            _ => access(
                side,
                1 + r.below(8) as u8,
                Some(stack("minecraft:diamond", 3)),
                None,
                sim,
            ),
        };
        sc.events.push(act(tick, depot, a));
        sc.checkpoints.extend([tick, tick + 1, tick + 3, tick + 8]);
        tick += 10;
        if fed && r.chance(0.25) {
            sc.events.push(act(
                tick,
                [0, 8, -3],
                Action::Item {
                    input: Input::DropOn {
                        stack: stack("minecraft:iron_ingot", 1 + r.below(64) as u8),
                    },
                },
            ));
            tick += 40;
            sc.checkpoints.extend((tick - 30..tick).step_by(3));
        }
        if r.chance(0.15) {
            sc.events.push(act(
                tick,
                depot,
                Action::Item {
                    input: Input::Use {
                        stack: None,
                        face: Face::Up,
                        yaw: 0.,
                        sneaking: false,
                    },
                },
            ));
            sc.checkpoints.extend([tick, tick + 1]);
            tick += 5;
        }
    }
    // A belt segment's own item access, from the side and from above.
    if fed && r.chance(0.3) {
        sc.events.push(act(
            tick,
            [0, 8, -2],
            access(
                *r.pick(&sides),
                0,
                Some(stack("minecraft:gold_ingot", 1 + r.below(64) as u8)),
                None,
                false,
            ),
        ));
        sc.checkpoints.extend([tick, tick + 1, tick + 5]);
        tick += 10;
    }
    sc.ticks = tick + 10;
    // Settle past the last input (`generate::SETTLE`), as mutation keeps it.
    let last = sc.events.iter().map(|e| e.tick).max().unwrap_or(0);
    sc.ticks = sc.ticks.max(last + crate::generate::SETTLE);
    sc.checkpoints.push(sc.ticks);
    sc.checkpoints.retain(|t| *t <= sc.ticks);
    sc.checkpoints.sort_unstable();
    sc.checkpoints.dedup();
    sc
}
