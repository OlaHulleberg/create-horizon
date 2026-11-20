//! Funnels moving stacks out of the depot behind them: belt funnels pushing onto their belt (one
//! facing with the belt in any shape, or a pushing one across it), belt funnels on a depot
//! pushing onto it or taking its stack (`handleBeltFunnelOutput`), their cooldown
//! (`startCooldown`) and redstone pausing them. Stacks reach the depots behind by landing on
//! them; the belt carries what the funnels put on it to a stone at its end.
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
fn funnel(facing: Dir, shape: &str) -> String {
    format!(
        "create:andesite_belt_funnel[facing={},powered=false,shape={shape},waterlogged=false]",
        name(facing)
    )
}

const ITEMS: [&str; 3] = [
    "minecraft:iron_ingot",
    "minecraft:gold_ingot",
    "minecraft:diamond",
];

pub fn generate(seed: u64) -> Scenario {
    let mut r = Rng::new(seed);
    let length = 4 + r.below(4) as i32;
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
    // The belt's first click is the z=0 shaft: it faces south.
    let movement = create_engine::belt::movement_facing(speed as f32, Dir::South);
    let end = if movement == Dir::South {
        [0, 8, length]
    } else {
        [0, 8, -1]
    };
    sc.blocks.push(placed(end, "minecraft:stone", None));
    // A funnel over a segment pushing onto the belt from the depot behind it.
    let z = 1 + r.below((length - 2) as u32) as i32;
    let (facing, shape) = if r.chance(0.6) {
        // Along the belt only retracted and extended are reachable (`getShapeForPosition`,
        // `onWrenched`); `updateShape` turns a pushing one back to retracted.
        (movement, *r.pick(&["retracted", "extended"]))
    } else {
        (*r.pick(&[Dir::East, Dir::West]), "pushing")
    };
    let pos = [0, 9, z];
    let behind = create_engine::funnel::behind(pos, facing);
    sc.blocks
        .push(placed(behind, "create:depot[waterlogged=false]", None));
    sc.events.push(state(24, pos, &funnel(facing, shape)));
    let mut depots = vec![behind];
    // A belt funnel on a depot beside the belt, pushing onto it or taking its stack into the
    // depot behind it.
    if r.chance(0.6) {
        let at = [3, 8, 1];
        let push = r.chance(0.5);
        let f = *r.pick(&[Dir::North, Dir::South, Dir::East]);
        let top = [3, 9, 1];
        let back = create_engine::funnel::behind(top, f);
        sc.blocks
            .push(placed(at, "create:depot[waterlogged=false]", None));
        sc.blocks
            .push(placed(back, "create:depot[waterlogged=false]", None));
        sc.events.push(state(
            26,
            top,
            &funnel(f, if push { "pushing" } else { "pulling" }),
        ));
        depots.push(if push { back } else { at });
    }
    sc.checkpoints.extend([25, 27, 30]);
    let mut tick = 32;
    for _ in 0..2 + r.below(3) {
        let d = *r.pick(&depots);
        sc.events
            .push(act(tick, d, drop(r.pick(&ITEMS), 1 + r.below(12) as u8)));
        sc.checkpoints.extend(tick..tick + 30);
        tick += 12 + r.below(30);
    }
    // Redstone beside the funnel pauses it (and resets its cooldown), then lets it go.
    if r.chance(0.4) {
        let side = [0, 10, z];
        sc.events
            .push(state(tick, side, "minecraft:redstone_block"));
        sc.events.push(act(tick + 1, behind, drop(ITEMS[0], 5)));
        sc.checkpoints.extend(tick..tick + 6);
        let off = tick + 8 + r.below(20);
        sc.events.push(state(off, side, "minecraft:air"));
        sc.checkpoints.extend(off..off + 14);
        tick = off + 15;
    }
    let last = sc.events.iter().map(|e| e.tick).max().unwrap_or(0);
    sc.ticks = sc.ticks.max(tick + 40).max(last + crate::generate::SETTLE);
    sc.events.sort_by_key(|e| e.tick);
    sc.checkpoints.retain(|t| *t <= sc.ticks);
    sc.checkpoints.sort_unstable();
    sc.checkpoints.dedup();
    sc
}
