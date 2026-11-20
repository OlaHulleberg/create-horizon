//! Belt funnels taking items off belts (`BeltFunnelInteractionHandler.checkForFunnels`): side
//! funnels the items pass under, retracted and extended funnels facing against the belt that
//! hold them at their entry, into a depot behind each funnel, empty, occupied and emptied by a
//! player again, with stacks queued behind one another.
use crate::{
    actions::{Action, Face},
    generate::Rng,
    scenario::{Change, Placed, Pos, Scenario},
    transport::{Input, Stack},
};
use create_engine::kinetics::Dir;

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
fn name(d: Dir) -> &'static str {
    match d {
        Dir::North => "north",
        Dir::South => "south",
        Dir::West => "west",
        _ => "east",
    }
}

pub fn generate(seed: u64) -> Scenario {
    let mut r = Rng::new(seed);
    let length = 5 + r.below(4) as i32;
    let speed = if r.chance(0.5) { 32 } else { -32 };
    let mut sc = Scenario {
        version: 3,
        seed,
        ticks: 260,
        day_time: 0,
        checkpoints: vec![],
        blocks: vec![
            Placed {
                pos: [0, 8, 0],
                state: "create:shaft[axis=x,waterlogged=false]".into(),
                motor: None,
            },
            Placed {
                pos: [0, 8, length - 1],
                state: "create:shaft[axis=x,waterlogged=false]".into(),
                motor: None,
            },
            Placed {
                pos: [-1, 8, 0],
                state: "create:creative_motor[facing=east]".into(),
                motor: Some(speed),
            },
        ],
        events: vec![],
    };
    for (tick, z) in [(10, 0), (20, length - 1)] {
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
    // One or two funnels over interior segments: side funnels pull (`getShapeForPosition`:
    // across the belt), funnels along it are retracted or extended.
    let mut depots = Vec::new();
    // Positions above the belt taken by a funnel or the depot behind one.
    let mut taken: Vec<Pos> = Vec::new();
    // The way the belt moves: its first click is the z=0 shaft, so it faces south; a funnel
    // along it faces against that way (it would push onto the belt facing with it).
    let movement = create_engine::belt::movement_facing(speed as f32, Dir::South);
    for k in 0..1 + r.below(2) {
        let z = 1 + r.below((length - 2) as u32) as i32;
        let along = r.chance(0.5);
        let facing = if along {
            movement.opposite()
        } else {
            *r.pick(&[Dir::East, Dir::West])
        };
        let shape = if along {
            if r.chance(0.5) {
                "retracted"
            } else {
                "extended"
            }
        } else {
            "pulling"
        };
        let pos = [0, 9, z];
        let behind = create_engine::funnel::behind(pos, facing);
        if taken.contains(&pos) || taken.contains(&behind) {
            continue;
        }
        taken.extend([pos, behind]);
        sc.events.push(state(
            30 + 2 * k,
            pos,
            &format!(
                "create:andesite_belt_funnel[facing={},powered=false,shape={shape},waterlogged=false]",
                name(facing)
            ),
        ));
        sc.blocks.push(Placed {
            pos: behind,
            state: "create:depot[waterlogged=false]".into(),
            motor: None,
        });
        depots.push(behind);
    }
    // A funnel at the belt's end: facing up it takes what reaches it into the depot under it
    // (`supportsDirectBeltInput`); facing elsewhere, or extracting, it holds the items there.
    if r.chance(0.5) {
        let end = if movement == Dir::South {
            [0, 8, length]
        } else {
            [0, 8, -1]
        };
        let facing = *r.pick(&["up", "up", "up", "north", "south", "east"]);
        // An extracting funnel would pull from the inventory behind it, the belt's end
        // segment for one facing along the belt: outside this family.
        let extracting = false;
        sc.events.push(state(
            36,
            end,
            &format!(
                "create:andesite_funnel[extracting={extracting},facing={facing},powered=false,waterlogged=false]"
            ),
        ));
        if facing == "up" {
            sc.blocks.push(Placed {
                pos: [end[0], end[1] - 1, end[2]],
                state: "create:depot[waterlogged=false]".into(),
                motor: None,
            });
        }
    }
    sc.checkpoints.extend([31, 33, 35]);
    let behind = if speed > 0 { 0 } else { length - 1 };
    let items = [
        "minecraft:iron_ingot",
        "minecraft:gold_ingot",
        "minecraft:diamond",
    ];
    let mut tick = 40;
    for _ in 0..2 + r.below(3) {
        sc.events.push(act(
            tick,
            [0, 8, behind],
            drop(r.pick(&items), 1 + r.below(64) as u8),
        ));
        sc.checkpoints.extend(tick..tick + 30);
        tick += 20 + r.below(20);
        // The player empties a depot: a funnel blocked by it may take again.
        if r.chance(0.4)
            && let Some(&d) = depots.first()
        {
            sc.events.push(act(
                tick,
                d,
                Action::Item {
                    input: Input::Use {
                        stack: None,
                        face: Face::Up,
                        yaw: 0.,
                        sneaking: false,
                    },
                },
            ));
            sc.checkpoints.extend([tick, tick + 1, tick + 2]);
            tick += 5;
        }
    }
    let last = sc.events.iter().map(|e| e.tick).max().unwrap_or(0);
    sc.ticks = sc.ticks.max(last + crate::generate::SETTLE);
    sc.checkpoints.retain(|t| *t <= sc.ticks);
    sc.checkpoints.sort_unstable();
    sc.checkpoints.dedup();
    sc
}
