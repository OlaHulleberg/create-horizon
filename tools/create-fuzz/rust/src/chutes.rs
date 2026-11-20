//! Chutes at a belt's end (`ChuteBlockEntity`'s `DirectBeltInputBehaviour`): a belt hands its
//! stacks into an empty vertical chute, which drops them into a depot, a belt or another chute
//! below, out of the bottom, or holds them over a solid block; a chute under another one
//! leading into it takes nothing from the side.
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
fn connector(tick: u32, pos: Pos) -> Change {
    act(
        tick,
        pos,
        Action::Use {
            item: "create:belt_connector".into(),
            face: Face::Up,
            hit: [0.5, 1., 0.5],
            yaw: 0.,
            pitch: 0.,
            sneaking: false,
        },
    )
}

pub fn generate(seed: u64) -> Scenario {
    let mut r = Rng::new(seed);
    let length = 3 + r.below(4) as i32;
    let speed = if r.chance(0.5) { 32 } else { -32 };
    let mut sc = Scenario {
        version: 3,
        seed,
        ticks: 240,
        day_time: 0,
        checkpoints: vec![],
        blocks: vec![
            placed([0, 9, 0], "create:shaft[axis=x,waterlogged=false]", None),
            placed(
                [0, 9, length - 1],
                "create:shaft[axis=x,waterlogged=false]",
                None,
            ),
            placed(
                [-1, 9, 0],
                "create:creative_motor[facing=east]",
                Some(speed),
            ),
        ],
        events: vec![connector(10, [0, 9, 0]), connector(20, [0, 9, length - 1])],
    };
    let movement = create_engine::belt::movement_facing(speed as f32, Dir::South);
    let end = if movement == Dir::South {
        [0, 9, length]
    } else {
        [0, 9, -1]
    };
    let shape = if r.chance(0.5) { "normal" } else { "window" };
    let chute = format!("create:chute[facing=down,shape={shape},waterlogged=false]");
    // The chute at the belt's end; under it a depot, a belt across, another chute (and a depot
    // under that), stone, or nothing; or a chute above it leading into it.
    let mut below = [end[0], end[1] - 1, end[2]];
    sc.blocks.push(placed(end, &chute, None));
    match r.below(6) {
        0 => sc
            .blocks
            .push(placed(below, "create:depot[waterlogged=false]", None)),
        1 => {
            // A belt across under the chute, along x, driven from its own motor.
            for x in [-2, 2] {
                sc.blocks.push(placed(
                    [x, below[1], below[2]],
                    "create:shaft[axis=z,waterlogged=false]",
                    None,
                ));
            }
            sc.blocks.push(placed(
                [-2, below[1], below[2] - 1],
                "create:creative_motor[facing=south]",
                Some(32),
            ));
            sc.events.push(connector(22, [-2, below[1], below[2]]));
            sc.events.push(connector(28, [2, below[1], below[2]]));
        }
        2 => {
            sc.blocks.push(placed(below, &chute, None));
            below[1] -= 1;
            sc.blocks
                .push(placed(below, "create:depot[waterlogged=false]", None));
        }
        3 => sc.blocks.push(placed(below, "minecraft:stone", None)),
        4 => sc
            .blocks
            .push(placed([end[0], end[1] + 1, end[2]], &chute, None)),
        _ => {}
    }
    let items = [
        "minecraft:iron_ingot",
        "minecraft:gold_ingot",
        "minecraft:diamond",
    ];
    let behind = if movement == Dir::South {
        0
    } else {
        length - 1
    };
    let mut tick = 40;
    for _ in 0..2 + r.below(3) {
        sc.events.push(act(
            tick,
            [0, 9, behind],
            Action::Item {
                input: Input::DropOn {
                    stack: Stack {
                        item: (*r.pick(&items)).into(),
                        count: 1 + r.below(64) as u8,
                    },
                },
            },
        ));
        sc.checkpoints.extend(tick..tick + 50);
        tick += 25 + r.below(30);
    }
    let last = sc.events.iter().map(|e| e.tick).max().unwrap_or(0);
    sc.ticks = sc.ticks.max(last + crate::generate::SETTLE);
    sc.checkpoints.retain(|t| *t <= sc.ticks);
    sc.checkpoints.sort_unstable();
    sc.checkpoints.dedup();
    sc
}
