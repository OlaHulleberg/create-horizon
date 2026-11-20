//! Belt scenario family and semantic segment observations.
use crate::{
    actions::{Action, Face},
    scenario::{Change, Placed, Pos, Scenario},
};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub controller: Pos,
    pub index: i32,
    pub length: i32,
    pub casing: String,
    /// `BeltBlockEntity.covered`: a block stands on this cased horizontal segment.
    #[serde(default)]
    pub covered: bool,
    /// `BeltBlockEntity.color`, by dye name.
    #[serde(default)]
    pub color: Option<String>,
}

pub fn generate(seed: u64) -> Scenario {
    if seed / 16 % 8 == 7 {
        return conveyor(seed);
    }
    let n = seed / 16;
    let length = 2 + (n % 5) as i32;
    let variant = (n / 5) % 7;
    let (axis, delta) = match variant {
        0 => ("x", [0, 0, length]),
        1 => ("z", [length, 0, 0]),
        2 => ("x", [0, length, length]),
        3 => ("z", [length, -length, 0]),
        4 => ("x", [0, length, 0]),
        5 => ("y", [length, 0, 0]),
        _ => ("x", [length, 0, 0]),
    };
    let a = [0, 8, 0];
    let b = [delta[0], 8 + delta[1], delta[2]];
    let (a, b) = if n.is_multiple_of(2) { (a, b) } else { (b, a) };
    let mut sc = Scenario {
        version: 3,
        seed,
        ticks: 180,
        day_time: 0,
        checkpoints: vec![40, 73, 75, 77, 79, 101],
        blocks: vec![
            Placed {
                pos: a,
                state: format!("create:shaft[axis={axis},waterlogged=false]"),
                motor: None,
            },
            Placed {
                pos: b,
                state: format!("create:shaft[axis={axis},waterlogged=false]"),
                motor: None,
            },
        ],
        events: vec![],
    };
    let mut motor = a;
    let (coordinate, facing) = match axis {
        "x" => (0, "east"),
        "y" => (1, "up"),
        _ => (2, "south"),
    };
    motor[coordinate] -= 1;
    sc.blocks.push(Placed {
        pos: motor,
        state: format!("create:creative_motor[facing={facing}]"),
        motor: Some(if n.is_multiple_of(2) { 32 } else { -32 }),
    });
    // Cancel a selected first shaft, then start the normal two-click connection.
    // Sneaking with the connector clears BELT_FIRST_SHAFT in Create's useOn.
    if n.is_multiple_of(3) {
        for (tick, sneaking) in [(3, false), (9, true)] {
            sc.events.push(Change {
                tick,
                pos: a,
                action: Some(Action::Use {
                    item: "create:belt_connector".into(),
                    face: Face::Up,
                    hit: [0.5, 1., 0.5],
                    yaw: 0.,
                    pitch: 0.,
                    sneaking,
                }),
                state: None,
                motor: None,
                turn: None,
            });
        }
    }
    let use_item = |item: &str| Action::Use {
        item: item.into(),
        face: Face::Up,
        hit: [0.5, 1., 0.5],
        yaw: 0.,
        pitch: 0.,
        sneaking: false,
    };
    for (tick, pos, item) in [
        (10, a, "create:belt_connector"),
        (20, b, "create:belt_connector"),
        (72, a, "create:andesite_casing"),
        (74, a, "create:brass_casing"),
    ] {
        sc.events.push(Change {
            tick,
            pos,
            action: Some(use_item(item)),
            state: None,
            motor: None,
            turn: None,
        });
    }
    sc.events.push(Change {
        tick: 100,
        pos: a,
        action: Some(Action::Remove),
        state: None,
        motor: None,
        turn: None,
    });
    sc
}

fn conveyor(seed: u64) -> Scenario {
    let positions = [[0, 8, 0], [5, 8, 0], [5, 9, 5]];
    let mut sc = Scenario {
        version: 3,
        seed,
        ticks: 180,
        day_time: 0,
        checkpoints: vec![40, 60, 80, 101],
        blocks: positions
            .into_iter()
            .map(|pos| Placed {
                pos,
                state: "create:chain_conveyor".into(),
                motor: None,
            })
            .collect(),
        events: vec![],
    };
    for (tick, a, b, connect) in [
        (10, 0, 1, true),
        (20, 1, 2, true),
        (30, 2, 0, true),
        (70, 0, 1, false),
        (90, 0, 1, true),
    ] {
        sc.events.push(Change {
            tick,
            pos: positions[a],
            action: Some(Action::ChainLink {
                target: positions[b],
                connect,
            }),
            state: None,
            motor: None,
            turn: None,
        });
    }
    sc
}
