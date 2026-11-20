//! Weighted ejector histories: an ejector (its default target, one block ahead) driven at a
//! random speed winds its plate down (`RETRACTING` to `CHARGED`), takes stacks landing on it, from
//! a belt ending into it and through its item handler, merging them up to its stack size (picked
//! on its value board), and throws them when enough lie on it (`ejectIfTriggered`) at what stands
//! ahead: a depot, a belt, or a second ejector throwing on to a depot. Redstone holds it; a full
//! target holds the throw.
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
fn insert(item: &str, count: u8) -> Action {
    Action::Capability {
        side: None,
        slot: 0,
        insert: Some(Stack {
            item: item.into(),
            count,
        }),
        extract: None,
        simulate: false,
    }
}
fn use_on(item: &str) -> Action {
    Action::Use {
        item: item.into(),
        face: Face::Up,
        hit: [0.5, 1.0, 0.5],
        yaw: 0.,
        pitch: 0.,
        sneaking: false,
    }
}

const ITEMS: [&str; 3] = [
    "minecraft:iron_ingot",
    "minecraft:gold_ingot",
    "minecraft:ender_pearl",
];
const EJECTOR: &str = "create:weighted_ejector[facing=east,waterlogged=false]";

pub fn generate(seed: u64) -> Scenario {
    let mut r = Rng::new(seed);
    let speed = *r.pick(&[16, 32, 64, 128, 256]) * if r.chance(0.5) { 1 } else { -1 };
    let mut sc = Scenario {
        version: 3,
        seed,
        ticks: 300,
        day_time: 0,
        checkpoints: vec![],
        blocks: vec![
            // The ejector's shaft runs along z (`getRotationAxis`: its facing turned clockwise).
            placed(
                [0, 8, -1],
                "create:creative_motor[facing=south]",
                Some(speed),
            ),
            placed([0, 8, 0], EJECTOR, None),
        ],
        events: vec![],
    };
    // What stands ahead, one block east (the default target, `EntityLauncher(1, 0)`).
    let target = r.below(4);
    let ahead = [1, 8, 0];
    match target {
        0 => sc
            .blocks
            .push(placed(ahead, "create:depot[waterlogged=false]", None)),
        1 => {
            // A belt across x from the target on, its own drive, ending at a stone.
            sc.blocks.push(placed(
                ahead,
                "create:shaft[axis=z,waterlogged=false]",
                None,
            ));
            sc.blocks.push(placed(
                [4, 8, 0],
                "create:shaft[axis=z,waterlogged=false]",
                None,
            ));
            sc.blocks.push(placed(
                [4, 8, 1],
                "create:creative_motor[facing=north]",
                Some(32),
            ));
            sc.blocks.push(placed([5, 8, 0], "minecraft:stone", None));
            sc.events
                .push(act(8, ahead, use_on("create:belt_connector")));
            sc.events
                .push(act(14, [4, 8, 0], use_on("create:belt_connector")));
        }
        2 => {
            // A second ejector on the same shaft line, throwing on to a depot.
            sc.blocks.push(placed(
                [1, 8, -1],
                "create:creative_motor[facing=south]",
                Some(speed),
            ));
            sc.blocks.push(placed(ahead, EJECTOR, None));
            sc.blocks
                .push(placed([2, 8, 0], "create:depot[waterlogged=false]", None));
        }
        _ => {
            // A depot already holding a stack: the throw waits (the target refuses it).
            sc.blocks
                .push(placed(ahead, "create:depot[waterlogged=false]", None));
            sc.events.push(act(20, ahead, drop("minecraft:diamond", 3)));
        }
    }
    // The stack size: 0 throws whatever lies on it.
    let size = if r.chance(0.5) {
        0
    } else {
        1 + r.below(24) as i32
    };
    if size > 0 {
        sc.events.push(act(
            22,
            [0, 8, 0],
            Action::ScrollSetting {
                row: 0,
                value: size,
            },
        ));
    }
    let mut tick = 30 + r.below(30);
    for _ in 0..2 + r.below(4) {
        let item = if r.chance(0.7) {
            ITEMS[0]
        } else {
            *r.pick(&ITEMS)
        };
        let n = 1 + r.below(16) as u8;
        let a = if r.chance(0.6) {
            drop(item, n)
        } else {
            insert(item, n)
        };
        sc.events.push(act(tick, [0, 8, 0], a));
        tick += 1 + r.below(25);
    }
    // Redstone holds it while it stands beside it.
    if r.chance(0.4) {
        let on = tick;
        sc.events
            .push(state(on, [0, 8, 1], "minecraft:redstone_block"));
        sc.events.push(act(
            on + 2,
            [0, 8, 0],
            drop(ITEMS[0], 1 + r.below(64) as u8),
        ));
        let off = on + 10 + r.below(30);
        sc.events.push(state(off, [0, 8, 1], "minecraft:air"));
        tick = off + 5;
    }
    // The target empties (a depot's stack taken off by hand) and the throw goes on.
    if target == 3 {
        sc.events.push(act(
            tick + 20,
            ahead,
            Action::Item {
                input: Input::Use {
                    stack: None,
                    face: Face::Up,
                    yaw: 0.,
                    sneaking: false,
                },
            },
        ));
        tick += 25;
    }
    sc.checkpoints.extend(20..(tick + 80).min(320));
    let last = sc.events.iter().map(|e| e.tick).max().unwrap_or(0);
    sc.ticks = sc.ticks.max(tick + 100).max(last + crate::generate::SETTLE);
    sc.events.sort_by_key(|e| e.tick);
    sc.checkpoints.retain(|t| *t <= sc.ticks);
    sc.checkpoints.sort_unstable();
    sc.checkpoints.dedup();
    sc
}
