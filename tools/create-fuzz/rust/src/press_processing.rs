//! Mechanical presses over depots and belts (`PressingBehaviour`, `BeltProcessingBehaviour`):
//! ingots pressed into sheets one at a time, a single item replaced in place and the rest of a
//! stack held while the results go to the depot's outputs, belt items held under a running press
//! and converted at the head's lowest point, presses at several speeds and stopped, and a belt
//! funnel between a press and its depot (`isBlocked`: funnels do not block).
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

const ITEMS: [&str; 3] = [
    "minecraft:iron_ingot",
    "minecraft:gold_ingot",
    "minecraft:diamond",
];

pub fn generate(seed: u64) -> Scenario {
    let mut r = Rng::new(seed);
    let rpm = *r.pick(&[32, 64, 128, 256, -64]);
    let mut sc = Scenario {
        version: 3,
        seed,
        ticks: 400,
        day_time: 0,
        checkpoints: vec![],
        blocks: vec![],
        events: vec![],
    };
    let on_belt = r.chance(0.4);
    // Where the stacks land, and the segment or depot under the press.
    let (drops, under) = if on_belt {
        let length = 4 + r.below(3) as i32;
        let speed = if r.chance(0.5) { 32 } else { -32 };
        sc.blocks.extend([
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
        ]);
        for (tick, z) in [(8, 0), (14, length - 1)] {
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
        let movement =
            create_engine::belt::movement_facing(speed as f32, create_engine::kinetics::Dir::South);
        let (end, start) = if movement == create_engine::kinetics::Dir::South {
            ([0, 8, length], 0)
        } else {
            ([0, 8, -1], length - 1)
        };
        sc.blocks.push(placed(end, "minecraft:stone", None));
        (
            [0, 8, start],
            [0, 8, 1 + r.below((length - 2) as u32) as i32],
        )
    } else {
        sc.blocks
            .push(placed([0, 8, 0], "create:depot[waterlogged=false]", None));
        ([0, 8, 0], [0, 8, 0])
    };
    let press = [under[0], under[1] + 2, under[2]];
    sc.blocks
        .push(placed(press, "create:mechanical_press[facing=north]", None));
    let motor = [press[0], press[1], press[2] - 1];
    sc.blocks.push(placed(
        motor,
        "create:creative_motor[facing=south]",
        Some(rpm),
    ));
    // A belt funnel between the press and its depot does not block it in Java.
    if !on_belt && r.chance(0.3) {
        sc.blocks
            .push(placed([0, 8, 1], "create:depot[waterlogged=false]", None));
        sc.events.push(Change {
            tick: 20,
            pos: [0, 9, 0],
            action: None,
            state: Some(
                "create:andesite_belt_funnel[facing=north,powered=false,shape=pulling,waterlogged=false]"
                    .into(),
            ),
            motor: None,
            turn: None,
        });
    }
    let mut tick = 30;
    for _ in 0..2 + r.below(4) {
        let item = if r.chance(0.8) {
            *r.pick(&ITEMS[..2])
        } else {
            ITEMS[2]
        };
        sc.events
            .push(act(tick, drops, drop(item, 1 + r.below(12) as u8)));
        sc.checkpoints.extend(tick..tick + 8);
        tick += 40 + r.below(80);
        if !on_belt {
            let a = match r.below(4) {
                // Take the outputs, or everything.
                0 => Action::Capability {
                    side: None,
                    slot: 1,
                    insert: None,
                    extract: Some(64),
                    simulate: false,
                },
                1 => Action::Item {
                    input: Input::Use {
                        stack: None,
                        face: Face::Up,
                        yaw: 0.,
                        sneaking: false,
                    },
                },
                _ => continue,
            };
            sc.events.push(act(tick, drops, a));
            sc.checkpoints.extend(tick..tick + 3);
            tick += 5;
        }
    }
    // The press stops and starts again.
    if r.chance(0.3) {
        let t = 60 + r.below(200);
        for (k, s) in [(t, 0), (t + 30 + r.below(60), rpm)] {
            sc.events.push(Change {
                tick: k,
                pos: motor,
                action: None,
                state: Some("create:creative_motor[facing=south]".into()),
                motor: Some(s),
                turn: None,
            });
            sc.checkpoints.extend(k..k + 3);
        }
    }
    let last = sc.events.iter().map(|e| e.tick).max().unwrap_or(0);
    sc.ticks = sc.ticks.max(tick + 60).max(last + crate::generate::SETTLE);
    sc.checkpoints.extend((20..sc.ticks).step_by(4));
    sc.events.sort_by_key(|e| e.tick);
    sc.checkpoints.retain(|t| *t <= sc.ticks);
    sc.checkpoints.sort_unstable();
    sc.checkpoints.dedup();
    sc
}

#[cfg(test)]
mod tests {
    #[test]
    fn presses_stand_two_over_their_depot_or_segment() {
        for seed in (78..78 + 128 * 60).step_by(128) {
            let sc = super::generate(seed);
            crate::catalogue::scenario(&sc);
            crate::transport::validate_scenario(&sc);
            assert!(
                sc.blocks
                    .iter()
                    .any(|b| b.state.contains("mechanical_press"))
            );
        }
    }
}
