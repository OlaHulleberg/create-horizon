//! Player histories for kinetic network construction and reconfiguration.

use crate::actions::{Action, Face};
use crate::scenario::{Change, Placed, Pos, Scenario};

fn action(tick: u32, pos: Pos, action: Action) -> Change {
    Change {
        tick,
        pos,
        action: Some(action),
        state: None,
        motor: None,
        turn: None,
    }
}

fn use_item(tick: u32, pos: Pos, item: &str, face: Face, hit: [f64; 3], sneaking: bool) -> Change {
    action(
        tick,
        pos,
        Action::Use {
            item: item.into(),
            face,
            hit,
            yaw: 0.,
            pitch: 0.,
            sneaking,
        },
    )
}

/// Exercise both cog sizes as the clicked gear and the held item. After placement,
/// wrench both shaft ends, turn the casing across its axis, then take it off.
pub fn relays(seed: u64) -> Scenario {
    let click = [0, 8, 0];
    let floor = [0, 7, 0];
    let shaft = [0, 9, 0];
    let items = ["create:cogwheel", "create:large_cogwheel"];
    let clicked = items[((seed >> 6) & 1) as usize];
    let held = items[((seed >> 7) & 1) as usize];
    let casing = ["create:andesite_casing", "create:brass_casing"][((seed >> 8) & 1) as usize];
    let hit = if seed & 512 == 0 {
        [0.5, 1., 0.5]
    } else {
        [0.93, 0.625, 0.7]
    };
    let mut events = vec![
        use_item(5, floor, clicked, Face::Up, Face::Up.point(0.5, 0.5), true),
        use_item(10, click, held, Face::Up, hit, false),
        action(20, shaft, Action::Remove),
        use_item(25, click, casing, Face::Up, Face::Up.point(0.5, 0.5), false),
        use_item(
            30,
            click,
            "create:wrench",
            Face::Down,
            Face::Down.point(0.5, 0.5),
            false,
        ),
        use_item(
            35,
            click,
            "create:wrench",
            Face::Up,
            Face::Up.point(0.5, 0.5),
            false,
        ),
        use_item(
            40,
            click,
            "create:wrench",
            Face::East,
            Face::East.point(0.5, 0.5),
            false,
        ),
        use_item(
            45,
            click,
            "create:wrench",
            Face::Up,
            Face::Up.point(0.5, 0.5),
            true,
        ),
    ];
    events.extend([
        use_item(
            55,
            click,
            "create:wooden_bracket",
            Face::Up,
            Face::Up.point(0.5, 0.5),
            false,
        ),
        use_item(
            60,
            click,
            "create:wrench",
            Face::Up,
            Face::Up.point(0.5, 0.5),
            false,
        ),
        action(65, click, Action::Remove),
    ]);
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 140,
        checkpoints: vec![9, 15, 26, 31, 36, 41, 46, 56, 61, 66],
        blocks: vec![crate::scenario::Placed {
            pos: floor,
            state: "minecraft:stone".into(),
            motor: None,
        }],
        events,
    }
}

/// Place gauges or a vertical gearbox with one, two competing, or no eligible shafts.
pub fn instruments(seed: u64) -> Scenario {
    let face = [Face::Up, Face::East, Face::South][(seed / 64 % 3) as usize];
    let pos = [0, 8, 0];
    let offset = face.dir().offset();
    let cell = [offset[0], pos[1] + offset[1], offset[2]];
    let shaft = match face {
        Face::Up => "y",
        Face::East => "x",
        Face::South => "z",
        _ => unreachable!(),
    };
    let item = [
        "create:speedometer",
        "create:stressometer",
        "create:vertical_gearbox",
    ][(seed / 192 % 3) as usize];
    let on_shaft = (seed / 576).is_multiple_of(3);
    let state = if on_shaft {
        format!("create:shaft[axis={shaft}]")
    } else {
        "minecraft:stone".into()
    };
    let mut blocks = vec![crate::scenario::Placed {
        pos,
        state,
        motor: None,
    }];
    if seed / 576 % 3 == 2 {
        for (axis, [dx, dz]) in [("x", [1, 0]), ("z", [0, 1])] {
            blocks.push(crate::scenario::Placed {
                pos: [cell[0] + dx, cell[1], cell[2] + dz],
                state: format!("create:shaft[axis={axis}]"),
                motor: None,
            });
        }
    }
    let hit = face.point(0.5, 0.5);
    let mut placement = use_item(8, pos, item, face, hit, false);
    if let Some(Action::Use { yaw, .. }) = &mut placement.action {
        *yaw = [0., 90., 180., 270.][(seed / 1728 % 4) as usize];
    }
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 90,
        checkpoints: vec![9, 21],
        blocks,
        events: vec![
            placement,
            use_item(
                20,
                cell,
                "create:wrench",
                Face::North,
                Face::North.point(0.5, 0.5),
                false,
            ),
        ],
    }
}

/// Close a four-cog mesh with the sources already turning, separate it again,
/// and restore it after a source has expired. A source at the opposite corner of
/// the square conveys the same sign, while one next to it conveys the reverse.
pub fn competing_sources(seed: u64) -> Scenario {
    let left = [0, 8, 0];
    let opposite = [1, 8, 1];
    let right = [1, 8, 0];
    let join = [0, 8, 1];
    let second = if seed & 64 == 0 { opposite } else { right };
    let above = |[x, y, z]: Pos| [x, y + 1, z];
    let below = |[x, y, z]: Pos| [x, y - 1, z];
    let up = |tick, pos, item, sneaking| {
        use_item(
            tick,
            pos,
            item,
            Face::Up,
            Face::Up.point(0.5, 0.5),
            sneaking,
        )
    };
    let blocks = [left, opposite, right, join]
        .map(|p| crate::scenario::Placed {
            pos: below(p),
            state: "minecraft:stone".into(),
            motor: None,
        })
        .to_vec();
    let mut events = [left, opposite, right]
        .into_iter()
        .enumerate()
        .map(|(i, p)| up(5 + i as u32, below(p), "create:cogwheel", true))
        .collect::<Vec<_>>();
    events.extend([
        up(10, left, "create:hand_crank", false),
        up(11, second, "create:hand_crank", false),
        up(20, above(left), "minecraft:air", false),
        up(23, above(second), "minecraft:air", seed & 128 != 0),
        up(25, below(join), "create:cogwheel", true),
        action(29, right, Action::Remove),
        up(31, above(second), "minecraft:air", seed & 256 != 0),
        up(34, below(right), "create:cogwheel", true),
        action(38, above(left), Action::Remove),
        up(45, above(second), "minecraft:air", false),
    ]);
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 120,
        checkpoints: vec![19, 21, 24, 26, 30, 32, 35, 39, 46, 70],
        blocks,
        events,
    }
}

/// Place a relay while powered, then reconnect it with either side as the
/// input. Both crank supports are also live shaft neighbours.
pub fn switched_shaft(seed: u64) -> Scenario {
    let relay = [0, 8, 0];
    let floor = [0, 7, 0];
    let signal = [0, 9, 0];
    let floor_use = |tick, item, sneaking| {
        use_item(
            tick,
            floor,
            item,
            Face::Up,
            Face::Up.point(0.5, 0.5),
            sneaking,
        )
    };
    let item = if seed & 64 == 0 {
        "create:clutch"
    } else {
        "create:gearshift"
    };
    let mut blocks = vec![Placed {
        pos: floor,
        state: "minecraft:stone".into(),
        motor: None,
    }];
    let mut events = Vec::new();
    for (side, face) in [(-1, Face::West), (1, Face::East)] {
        let shaft = [side, 8, 0];
        blocks.push(Placed {
            pos: shaft,
            state: "create:shaft[axis=x]".into(),
            motor: None,
        });
        events.push(use_item(
            5,
            shaft,
            "create:hand_crank",
            face,
            face.point(0.5, 0.5),
            false,
        ));
    }
    let turn = |tick, x, face, backwards| {
        use_item(
            tick,
            [x, 8, 0],
            "minecraft:air",
            face,
            face.point(0.5, 0.5),
            backwards,
        )
    };
    events.extend([
        action(8, signal, Action::Signal { strength: 15 }),
        floor_use(10, item, false),
        turn(12, -2, Face::West, false),
        action(17, signal, Action::Remove),
        turn(22, -2, Face::West, false),
        action(23, signal, Action::Signal { strength: 15 }),
        action(29, [-2, 8, 0], Action::Remove),
        turn(32, 2, Face::East, seed & 128 != 0),
        action(37, signal, Action::Remove),
        action(43, relay, Action::Remove),
        floor_use(48, item, true),
        action(52, relay, Action::Remove),
        floor_use(56, item, false),
        turn(60, 2, Face::East, false),
    ]);
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 135,
        checkpoints: vec![11, 14, 18, 24, 30, 34, 38, 44, 49, 57, 62, 90],
        blocks,
        events,
    }
}

/// Attempt assisted large-cog placement with the slot blocked, then drive a
/// diagonal small cog from the controller across the rotation-speed limit.
pub fn controller_cog(seed: u64) -> Scenario {
    let controller = [0, 8, 0];
    let gear = [0, 9, 0];
    let support = [-1, 8, 0];
    let crank = [-2, 8, 0];
    let floor = [0, 7, 0];
    let gear_support = [1, 10, -1];
    let blocker = if seed & 64 == 0 {
        (gear, "minecraft:stone")
    } else {
        ([1, 9, 0], "create:cogwheel[axis=z]")
    };
    let up =
        |tick, p, item, sneak| use_item(tick, p, item, Face::Up, Face::Up.point(0.5, 0.5), sneak);
    let fit_gear = |tick| {
        use_item(
            tick,
            controller,
            "create:large_cogwheel",
            Face::North,
            Face::North.point(0.82, 0.35),
            false,
        )
    };
    let turn = |tick| {
        use_item(
            tick,
            crank,
            "minecraft:air",
            Face::West,
            Face::West.point(0.5, 0.5),
            false,
        )
    };
    let blocks = [
        (support, "create:shaft[axis=x]"),
        (floor, "minecraft:stone"),
        (gear_support, "minecraft:stone"),
        blocker,
    ]
    .map(|(pos, state)| Placed {
        pos,
        state: state.into(),
        motor: None,
    })
    .to_vec();
    let events = vec![
        up(6, floor, "create:rotation_speed_controller", false),
        fit_gear(9),
        action(12, blocker.0, Action::Remove),
        fit_gear(14),
        use_item(
            17,
            gear_support,
            "create:cogwheel",
            Face::South,
            Face::South.point(0.5, 0.5),
            true,
        ),
        use_item(
            19,
            support,
            "create:hand_crank",
            Face::West,
            Face::West.point(0.5, 0.5),
            false,
        ),
        turn(22),
        action(28, controller, Action::SpeedSetting { row: 1, value: 128 }),
        turn(33),
        action(38, controller, Action::SpeedSetting { row: 1, value: 129 }),
    ];
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 115,
        checkpoints: vec![10, 15, 21, 25, 30, 40],
        blocks,
        events,
    }
}

/// Creative-mode fixture for the motor board and the speed gauge's dial
/// breakpoints. The gauge and the relay placements use ordinary block items.
pub fn gauge_thresholds(seed: u64) -> Scenario {
    let motor = [-1, 8, 0];
    let shaft = [0, 8, 0];
    let gauge = [1, 8, 0];
    let side_use = |tick, p, item, face: Face, sneak| {
        use_item(tick, p, item, face, face.point(0.5, 0.5), sneak)
    };
    let item = if seed & 64 == 0 {
        "create:speedometer"
    } else {
        "create:stressometer"
    };
    let mut events = vec![
        action(3, motor, Action::GameMode { creative: true }),
        side_use(5, shaft, "create:creative_motor", Face::West, false),
        side_use(8, shaft, item, Face::East, false),
    ];
    let mut checkpoints = vec![9];
    for (index, value) in [1, 29, 30, 31, 99, 100, 101, 256].into_iter().enumerate() {
        let tick = 12 + index as u32 * 5;
        events.push(action(
            tick,
            motor,
            Action::SpeedSetting {
                row: ((seed >> 7) & 1) as i32,
                value,
            },
        ));
        checkpoints.push(tick + 3);
    }
    events.extend([
        action(55, motor, Action::Remove),
        side_use(60, shaft, "create:creative_motor", Face::West, true),
        action(68, gauge, Action::Remove),
    ]);
    checkpoints.extend([57, 63, 69]);
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 145,
        checkpoints,
        blocks: vec![Placed {
            pos: shaft,
            state: "create:shaft[axis=x]".into(),
            motor: None,
        }],
        events,
    }
}

/// Try to branch a running chain across two connection axes, then remove
/// both ends of the first run and rebuild the connection along the other axis.
pub fn chain_junction(seed: u64) -> Scenario {
    let center = [0, 8, 0];
    let crank = [-2, 8, 0];
    let shaft = [-1, 8, 0];
    let side = [0, 8, 1];
    let rear = [0, 8, -1];
    let top = [0, 9, 0];
    let hanger = [0, 9, -1];
    let place = |tick, pos, item, face: Face, sneaking| {
        let mut change = use_item(tick, pos, item, face, face.point(0.5, 0.5), sneaking);
        if let Some(Action::Use { yaw, .. }) = &mut change.action {
            *yaw = 90.;
        }
        change
    };
    let gearshift = "create:adjustable_chain_gearshift";
    let chain = "create:encased_chain_drive";
    let (first, second) = if seed & 64 == 0 {
        (chain, gearshift)
    } else {
        (gearshift, chain)
    };
    let base = [0, 7, 0];
    let mut blocks = [base, [0, 7, 1], [0, 7, -1], hanger]
        .map(|pos| Placed {
            pos,
            state: "minecraft:stone".into(),
            motor: None,
        })
        .to_vec();
    blocks.push(Placed {
        pos: shaft,
        state: "create:shaft[axis=x]".into(),
        motor: None,
    });
    let events = vec![
        place(6, base, first, Face::Up, false),
        place(8, [0, 7, 1], second, Face::Up, true),
        place(10, [0, 7, -1], second, Face::Up, true),
        place(12, shaft, "create:hand_crank", Face::West, false),
        place(17, crank, "minecraft:air", Face::West, false),
        place(20, hanger, second, Face::South, true),
        place(23, crank, "minecraft:air", Face::West, false),
        action(24, [0, 9, 1], Action::Signal { strength: 15 }),
        action(29, side, Action::Remove),
        action(31, rear, Action::Remove),
        place(33, crank, "minecraft:air", Face::West, false),
        action(36, top, Action::Remove),
        place(40, hanger, second, Face::South, true),
        action(44, [0, 9, 1], Action::Remove),
        action(46, center, Action::Remove),
        place(50, base, first, Face::Up, true),
    ];
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 125,
        checkpoints: vec![16, 18, 22, 27, 30, 32, 35, 42, 47, 52],
        blocks,
        events,
    }
}

/// Extend a pole past several shafts, then block that end and try the other
/// side. Case the resulting shaft, remove the casing, then fit brackets.
pub fn shaft_extension(seed: u64) -> Scenario {
    let floor = [0, 7, 0];
    let shaft = [0, 8, 0];
    let tip = [0, 10, 0];
    let blocker = [0, 11, 0];
    let up = |tick, pos, item, sneak| {
        use_item(tick, pos, item, Face::Up, Face::Up.point(0.5, 0.5), sneak)
    };
    let first = if seed & 64 == 0 {
        "create:andesite_casing"
    } else {
        "create:brass_casing"
    };
    let gear = if seed & 128 == 0 {
        "create:cogwheel"
    } else {
        "create:large_cogwheel"
    };
    let mut place_shaft = up(5, floor, "create:shaft", false);
    if let Some(Action::Use { pitch, .. }) = &mut place_shaft.action {
        *pitch = 80.;
    }
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 125,
        checkpoints: vec![6, 11, 16, 21, 26, 31, 36, 41, 47, 52],
        blocks: [floor, blocker]
            .map(|pos| Placed {
                pos,
                state: "minecraft:stone".into(),
                motor: None,
            })
            .to_vec(),
        events: vec![
            place_shaft,
            up(10, shaft, "create:shaft", false),
            up(15, shaft, "create:shaft", false),
            up(20, shaft, gear, false),
            up(25, tip, first, false),
            up(30, tip, "create:wrench", true),
            up(35, tip, "create:metal_bracket", false),
            up(40, tip, "create:wrench", false),
            up(46, tip, "create:wooden_bracket", false),
            action(51, tip, Action::Remove),
        ],
    }
}

/// Install a sequencer over a live vertical input, add a shaft from its face,
/// and remove drive during a program. Turn it flat using the side wrench.
pub fn vertical_sequencer(seed: u64) -> Scenario {
    use crate::program::{Instruction, Kind, Modifier};
    let support = [0, 7, 0];
    let sequencer = [0, 8, 0];
    let arm = [0, 6, 0];
    let crank = [0, 5, 0];
    let signal = [1, 8, 0];
    let up = |tick, p, item| use_item(tick, p, item, Face::Up, Face::Up.point(0.5, 0.5), false);
    let down =
        |tick, p, item| use_item(tick, p, item, Face::Down, Face::Down.point(0.5, 0.5), false);
    let mut shaft = up(14, sequencer, "create:shaft");
    if let Some(Action::Use { pitch, .. }) = &mut shaft.action {
        *pitch = 80.;
    }
    let wrench = |tick, face: Face| {
        use_item(
            tick,
            sequencer,
            "create:wrench",
            face,
            face.point(0.5, 0.5),
            false,
        )
    };
    let instructions = vec![
        Instruction {
            instruction: Kind::TurnAngle,
            modifier: if seed & 64 == 0 {
                Modifier::Forward
            } else {
                Modifier::BackFast
            },
            value: 360,
        },
        Instruction {
            instruction: Kind::End,
            modifier: Modifier::Forward,
            value: -1,
        },
    ];
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 125,
        checkpoints: vec![13, 17, 24, 27, 33, 39, 45, 49],
        blocks: [support, arm]
            .map(|pos| Placed {
                pos,
                state: "create:shaft[axis=y]".into(),
                motor: None,
            })
            .to_vec(),
        events: vec![
            down(6, arm, "create:hand_crank"),
            down(10, crank, "minecraft:air"),
            up(12, support, "create:sequenced_gearshift"),
            shaft,
            down(18, crank, "minecraft:air"),
            action(19, sequencer, Action::Program { instructions }),
            action(21, signal, Action::Signal { strength: 15 }),
            action(25, crank, Action::Remove),
            action(28, signal, Action::Remove),
            wrench(32, Face::North),
            wrench(38, Face::East),
            wrench(44, Face::Up),
            wrench(48, Face::South),
        ],
    }
}

/// Wrench drives of a running chain: the middle turns about the chain's axis and
/// back, the end, then a powered one. A drive beside the end but off its axis is
/// wrenched about each clicked face until it can join, and the middle is finally
/// picked up with a sneaking wrench.
pub fn chain_wrench(seed: u64) -> Scenario {
    let crank = [-2, 8, 0];
    let shaft = [-1, 8, 0];
    let middle = [0, 8, 1];
    let end = [0, 8, 2];
    let lone = [0, 8, 3];
    let signal = [1, 8, 1];
    let drives = [
        "create:encased_chain_drive",
        "create:adjustable_chain_gearshift",
    ];
    let kind = |z: i32| drives[((z as u64 + (seed >> 6)) & 1) as usize];
    let place = |tick, pos: Pos, item, sneaking, yaw| {
        let mut change = use_item(
            tick,
            pos,
            item,
            Face::Up,
            Face::Up.point(0.5, 0.5),
            sneaking,
        );
        if let Some(Action::Use { yaw: y, .. }) = &mut change.action {
            *y = yaw;
        }
        change
    };
    let wrench = |tick, pos, face: Face, sneaking| {
        use_item(
            tick,
            pos,
            "create:wrench",
            face,
            face.point(0.5, 0.5),
            sneaking,
        )
    };
    let turn = |tick| {
        use_item(
            tick,
            crank,
            "minecraft:air",
            Face::West,
            Face::West.point(0.5, 0.5),
            false,
        )
    };
    let faces = [Face::Up, Face::North, Face::East];
    let first = faces[(seed / 128 % 3) as usize];
    let mut blocks: Vec<Placed> = (0..4)
        .map(|z| Placed {
            pos: [0, 7, z],
            state: "minecraft:stone".into(),
            motor: None,
        })
        .collect();
    blocks.push(Placed {
        pos: shaft,
        state: "create:shaft[axis=x]".into(),
        motor: None,
    });
    let mut events: Vec<Change> = (0..3)
        .map(|z| place(4 + 2 * z as u32, [0, 7, z], kind(z), true, 90.))
        .collect();
    events.extend([
        use_item(
            10,
            shaft,
            "create:hand_crank",
            Face::West,
            Face::West.point(0.5, 0.5),
            false,
        ),
        turn(12),
        wrench(14, middle, first, false),
        turn(17),
        wrench(19, middle, first, false),
        turn(22),
        wrench(24, end, Face::Up, false),
        turn(27),
        wrench(29, end, Face::Up, false),
        action(31, signal, Action::Signal { strength: 15 }),
        turn(32),
        wrench(34, middle, Face::West, false),
        turn(37),
        wrench(39, middle, Face::West, false),
        action(41, signal, Action::Remove),
        place(43, [0, 7, 3], kind(3), seed & 1024 != 0, 0.),
        turn(45),
        wrench(47, lone, Face::Up, false),
        turn(49),
        wrench(51, lone, Face::North, false),
        turn(53),
        wrench(55, lone, Face::East, false),
        turn(57),
        wrench(60, middle, Face::Up, true),
        turn(62),
    ]);
    let checkpoints = events
        .iter()
        .map(|e| e.tick + 1)
        .filter(|t| *t > 10)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 135,
        checkpoints,
        blocks,
        events,
    }
}

/// Every block item a player places into a kinetic network.
const PLACEABLE: [&str; 15] = [
    "create:shaft",
    "create:cogwheel",
    "create:large_cogwheel",
    "create:gearbox",
    "create:vertical_gearbox",
    "create:clutch",
    "create:gearshift",
    "create:encased_chain_drive",
    "create:adjustable_chain_gearshift",
    "create:sequenced_gearshift",
    "create:rotation_speed_controller",
    "create:creative_motor",
    "create:hand_crank",
    "create:speedometer",
    "create:stressometer",
];

/// Place one block item as a player into seven neighbourhoods: no shaft, one or two
/// shafts on one axis, competing horizontal or vertical shafts, a shaft facing away
/// beside one facing in, and a small cog. The clicked face, sneaking and the look
/// direction vary, so the preferred-axis and preferred-facing searches take every outcome.
pub fn contested_placement(seed: u64) -> Scenario {
    let n = seed / 64;
    let item = PLACEABLE[(n % 15) as usize];
    let sneaking = n / 15 % 2 == 1;
    let faces = [
        Face::Up,
        Face::North,
        Face::East,
        Face::Down,
        Face::South,
        Face::West,
    ];
    let yaws = [10., 100., 190., 280.];
    let pitches = [0., 70., -70.];
    let patterns: [&[(Pos, &str)]; 7] = [
        &[],
        &[([1, 0, 0], "create:shaft[axis=x]")],
        &[
            ([1, 0, 0], "create:shaft[axis=x]"),
            ([-1, 0, 0], "create:shaft[axis=x]"),
        ],
        &[
            ([1, 0, 0], "create:shaft[axis=x]"),
            ([0, 0, 1], "create:shaft[axis=z]"),
        ],
        &[
            ([0, 1, 0], "create:shaft[axis=y]"),
            ([1, 0, 0], "create:shaft[axis=x]"),
        ],
        &[
            ([1, 0, 0], "create:shaft[axis=z]"),
            ([0, 0, -1], "create:shaft[axis=z]"),
        ],
        &[([0, -1, 0], "create:cogwheel[axis=x]")],
    ];
    let mut blocks = Vec::new();
    let mut events = Vec::new();
    if item == "create:creative_motor" {
        events.push(action(2, [0, 8, 0], Action::GameMode { creative: true }));
    }
    for (i, pattern) in patterns.iter().enumerate() {
        let i = i as u64;
        let cell = [i as i32 * 4 - 12, 8, 0];
        let at = |[dx, dy, dz]: Pos| [cell[0] + dx, cell[1] + dy, cell[2] + dz];
        for (offset, state) in pattern.iter() {
            blocks.push(Placed {
                pos: at(*offset),
                state: (*state).into(),
                motor: None,
            });
        }
        let face = faces[((n / 30 + i) % 6) as usize];
        let back = face.dir().offset().map(|v| -v);
        let clicked = at(back);
        if !pattern.iter().any(|(offset, _)| *offset == back) {
            blocks.push(Placed {
                pos: clicked,
                state: "minecraft:stone".into(),
                motor: None,
            });
        }
        let mut place = use_item(
            5 + 2 * i as u32,
            clicked,
            item,
            face,
            face.point(0.5, 0.5),
            sneaking,
        );
        if let Some(Action::Use { yaw, pitch, .. }) = &mut place.action {
            *yaw = yaws[((n / 180 + i) % 4) as usize];
            *pitch = pitches[((n / 720 + i) % 3) as usize];
        }
        events.push(place);
    }
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 90,
        checkpoints: vec![20],
        blocks,
        events,
    }
}

/// Turn a player-placed crank holding other items: the crank's use takes the click
/// with a stick, an extendo grip, a wrench or a block item, while a sneaking player
/// holding an item uses the item instead and an empty sneaking hand turns it back.
pub fn held_crank(seed: u64) -> Scenario {
    let shaft = [0, 8, 0];
    let crank = [-1, 8, 0];
    let gauge = [1, 8, 0];
    let west = |tick, pos, item, sneaking| {
        use_item(
            tick,
            pos,
            item,
            Face::West,
            Face::West.point(0.5, 0.5),
            sneaking,
        )
    };
    let held = [
        "minecraft:stick",
        "create:extendo_grip",
        "create:wrench",
        "create:cogwheel",
        "create:shaft",
    ];
    let mut events = vec![
        west(4, shaft, "create:hand_crank", false),
        use_item(
            5,
            shaft,
            "create:speedometer",
            Face::East,
            Face::East.point(0.5, 0.5),
            false,
        ),
    ];
    let mut checkpoints = vec![6];
    let mut tick = 8;
    for i in 0..held.len() {
        let item = held[(i + (seed / 64) as usize) % held.len()];
        events.push(west(tick, crank, item, false));
        checkpoints.extend([tick + 1, tick + 4]);
        tick += 13;
    }
    for (item, sneaking) in [
        ("create:extendo_grip", true),
        ("minecraft:air", true),
        ("create:shaft", true),
    ] {
        events.push(west(tick, crank, item, sneaking));
        checkpoints.extend([tick + 1, tick + 4]);
        tick += 13;
    }
    events.push(action(tick, gauge, Action::Remove));
    checkpoints.push(tick + 1);
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: tick + 75,
        checkpoints,
        blocks: vec![Placed {
            pos: shaft,
            state: "create:shaft[axis=x]".into(),
            motor: None,
        }],
        events,
    }
}

/// Two sequencers in one driven line, powered and unpowered in the same tick: the
/// first one's state change reaches the second while its own scheduled tick is due.
pub fn sequencer_pair(seed: u64) -> Scenario {
    use crate::program::{Instruction, Kind, Modifier};
    let shaft = [-1, 8, 0];
    let crank = [-2, 8, 0];
    let pair = [[0, 8, 0], [1, 8, 0]];
    let above = |[x, y, z]: Pos| [x, y + 1, z];
    let turn = |tick| {
        use_item(
            tick,
            crank,
            "minecraft:air",
            Face::West,
            Face::West.point(0.5, 0.5),
            false,
        )
    };
    let angle = [90, 360][((seed >> 6) & 1) as usize];
    let program = || Action::Program {
        instructions: vec![
            Instruction {
                instruction: Kind::TurnAngle,
                modifier: Modifier::Forward,
                value: angle,
            },
            Instruction {
                instruction: Kind::End,
                modifier: Modifier::Forward,
                value: -1,
            },
        ],
    };
    let mut blocks = vec![Placed {
        pos: shaft,
        state: "create:shaft[axis=x]".into(),
        motor: None,
    }];
    let mut events = vec![use_item(
        4,
        shaft,
        "create:hand_crank",
        Face::West,
        Face::West.point(0.5, 0.5),
        false,
    )];
    for p in pair {
        blocks.push(Placed {
            pos: p,
            state: "create:sequenced_gearshift[axis=x,state=0,vertical=false]".into(),
            motor: None,
        });
        events.push(action(7, p, program()));
    }
    let mut checkpoints = Vec::new();
    for (tick, on) in [(10, true), (21, false), (24, true), (36, false), (37, true)] {
        events.push(turn(tick - 2));
        for p in pair {
            events.push(if on {
                action(tick, above(p), Action::Signal { strength: 15 })
            } else {
                action(tick, above(p), Action::Remove)
            });
        }
        checkpoints.extend([tick, tick + 1, tick + 2, tick + 4]);
    }
    events.push(turn(44));
    // Off-shaft clicks with a plain item, a kinetic block item and another block item
    // open the program screen (SequencedGearshiftBlock.useItemOn): nothing is placed.
    for (tick, item) in [
        (46, "minecraft:stick"),
        (47, "create:cogwheel"),
        (48, "create:andesite_casing"),
    ] {
        events.push(use_item(
            tick,
            pair[0],
            item,
            Face::Up,
            Face::Up.point(0.5, 0.5),
            false,
        ));
        checkpoints.push(tick + 1);
    }
    checkpoints.sort();
    checkpoints.dedup();
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 120,
        checkpoints,
        blocks,
        events,
    }
}

/// Close metal girders around turning shafts on both horizontal axes, then extend
/// another shaft while sneaking, which places it beside rather than along the line.
pub fn girder_shaft(seed: u64) -> Scenario {
    let crank = [-2, 8, 0];
    let line = [-1, 8, 0];
    let girdered = [0, 8, 0];
    let across = [0, 8, 2];
    let pole = [2, 8, 0];
    let west = |tick, pos, item| {
        use_item(
            tick,
            pos,
            item,
            Face::West,
            Face::West.point(0.5, 0.5),
            false,
        )
    };
    let face = [Face::Up, Face::North, Face::East][(seed / 64 % 3) as usize];
    let girder = |tick, pos| {
        use_item(
            tick,
            pos,
            "create:metal_girder",
            face,
            face.point(0.5, 0.5),
            false,
        )
    };
    let blocks = [
        (line, "create:shaft[axis=x]"),
        (girdered, "create:shaft[axis=x]"),
        (across, "create:shaft[axis=z]"),
        (pole, "create:shaft[axis=x]"),
        ([2, 7, 0], "minecraft:stone"),
    ]
    .map(|(pos, state)| Placed {
        pos,
        state: state.into(),
        motor: None,
    })
    .to_vec();
    let events = vec![
        west(4, line, "create:hand_crank"),
        west(6, crank, "minecraft:air"),
        girder(8, girdered),
        girder(10, across),
        west(12, crank, "minecraft:air"),
        use_item(
            14,
            pole,
            "create:shaft",
            Face::East,
            Face::East.point(0.5, 0.5),
            true,
        ),
        west(18, crank, "minecraft:air"),
        action(24, girdered, Action::Remove),
    ];
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 100,
        checkpoints: vec![7, 9, 11, 13, 15, 19, 25],
        blocks,
        events,
    }
}

/// Click a valve handle's rim through its turn and the cooldown after it, with a
/// wrench, with the dye it already has, sneaking, and finally pick it up with a
/// sneaking wrench. Rim hits miss the value box, so each click reaches `activate`.
pub fn valve_rim(seed: u64) -> Scenario {
    let valve = [0, 8, 0];
    let rim =
        |tick, item, sneaking| use_item(tick, valve, item, Face::Up, [0.1, 0.5, 0.1], sneaking);
    let dye = ["minecraft:white_dye", "minecraft:blue_dye"][((seed >> 6) & 1) as usize];
    let mut events = vec![action(
        4,
        valve,
        Action::ValveSetting {
            row: ((seed >> 7) & 1) as i32,
            value: 1,
        },
    )];
    events.extend((6..20).map(|tick| rim(tick, "minecraft:air", false)));
    events.extend([
        rim(24, dye, false),
        rim(30, dye, false),
        rim(32, dye, true),
        rim(40, "create:wrench", false),
        rim(42, "create:wrench", false),
        rim(50, "minecraft:air", true),
        rim(60, "create:wrench", true),
    ]);
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 135,
        checkpoints: (5..62).collect(),
        blocks: [
            ([0, 7, 0], "create:shaft[axis=y,waterlogged=false]"),
            (
                valve,
                "create:copper_valve_handle[facing=up,waterlogged=false]",
            ),
            ([0, 6, 0], "create:shaft[axis=y,waterlogged=false]"),
        ]
        .map(|(pos, state)| Placed {
            pos,
            state: state.into(),
            motor: None,
        })
        .to_vec(),
        events,
    }
}

/// Pick gearboxes on each axis and a bracketed shaft up with a sneaking wrench,
/// which takes their drops (a horizontal gearbox drops a vertical gearbox item).
pub fn wrench_pickup(seed: u64) -> Scenario {
    let face = [Face::Up, Face::North, Face::East][(seed / 64 % 3) as usize];
    let cells = [
        ([0, 8, 0], "create:gearbox[axis=y]"),
        ([2, 8, 0], "create:gearbox[axis=x]"),
        ([4, 8, 0], "create:gearbox[axis=z]"),
        ([6, 8, 0], "create:shaft[axis=x]"),
    ];
    let mut events = vec![use_item(
        4,
        [6, 8, 0],
        "create:metal_bracket",
        Face::Up,
        Face::Up.point(0.5, 0.5),
        false,
    )];
    for (i, (pos, _)) in cells.iter().enumerate() {
        events.push(use_item(
            8 + 3 * i as u32,
            *pos,
            "create:wrench",
            face,
            face.point(0.5, 0.5),
            true,
        ));
    }
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 95,
        checkpoints: vec![5, 9, 12, 15, 18, 21],
        blocks: cells
            .map(|(pos, state)| Placed {
                pos,
                state: state.into(),
                motor: None,
            })
            .to_vec(),
        events,
    }
}

/// Rotation speed controllers on both axes: the target set before any input, the
/// helper's large cog fitted on either axis, a crank input, a zero target, a shaft
/// item used on the controller, and controllers placed under a vertical large cog
/// and under a small cog, whose targets are then changed.
pub fn controller_axes(seed: u64) -> Scenario {
    let up = |tick, pos, item, yaw: f32| {
        let mut change = use_item(tick, pos, item, Face::Up, Face::Up.point(0.5, 0.5), false);
        if let Some(Action::Use { yaw: y, .. }) = &mut change.action {
            *y = yaw;
        }
        change
    };
    let yaw = [0., 90.][((seed >> 6) & 1) as usize];
    let setting = |tick, pos, value| action(tick, pos, Action::SpeedSetting { row: 0, value });
    let controller = [0, 8, 0];
    let vertical = [4, 8, 0];
    let small = [8, 8, 0];
    let (shaft, crank, face) = if yaw == 0. {
        ([-1, 8, 0], [-2, 8, 0], Face::West)
    } else {
        ([0, 8, -1], [0, 8, -2], Face::North)
    };
    let blocks = [
        ([0, 7, 0], "minecraft:stone".to_string()),
        ([4, 7, 0], "minecraft:stone".into()),
        ([8, 7, 0], "minecraft:stone".into()),
        ([4, 9, 0], "create:large_cogwheel[axis=y]".into()),
        ([8, 9, 0], "create:cogwheel[axis=x]".into()),
    ]
    .map(|(pos, state)| Placed {
        pos,
        state,
        motor: None,
    })
    .to_vec();
    let turn = |tick| {
        use_item(
            tick,
            crank,
            "minecraft:air",
            face,
            face.point(0.5, 0.5),
            false,
        )
    };
    let along = if yaw == 0. { Face::East } else { Face::South };
    let events = vec![
        up(4, [0, 7, 0], "create:rotation_speed_controller", yaw),
        setting(6, controller, 64),
        use_item(
            8,
            controller,
            "create:large_cogwheel",
            Face::North,
            Face::North.point(0.8, 0.4),
            false,
        ),
        up(10, shaft, "create:shaft", yaw),
        use_item(
            12,
            shaft,
            "create:hand_crank",
            face,
            face.point(0.5, 0.5),
            false,
        ),
        turn(14),
        setting(16, controller, 96),
        turn(20),
        setting(22, controller, 0),
        turn(26),
        // The face along the controller's axis has no value box, so the click reaches
        // SpeedControllerBlock.useItemOn with an item its helper does not take.
        use_item(
            28,
            controller,
            "create:shaft",
            along,
            along.point(0.5, 0.5),
            false,
        ),
        use_item(
            29,
            controller,
            "create:shaft",
            Face::South,
            Face::South.point(0.5, 0.5),
            false,
        ),
        // A wrench there is not a block item: the helper's large-cog predicate refuses it
        // and the wrench turns the controller.
        use_item(
            30,
            controller,
            "create:wrench",
            along,
            along.point(0.5, 0.5),
            false,
        ),
        up(31, [4, 7, 0], "create:rotation_speed_controller", yaw),
        setting(32, vertical, 32),
        up(34, [8, 7, 0], "create:rotation_speed_controller", yaw),
        setting(36, small, 32),
    ];
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 110,
        checkpoints: vec![5, 7, 9, 13, 15, 17, 21, 23, 27, 29, 31, 32, 33, 35, 37],
        blocks,
        events,
    }
}

/// Join two running creative motors on one shaft line by placing the shaft between
/// them, with the second at the same, a slower, a faster or the opposite speed. Then
/// retarget the second through slower, faster, stopped and reversed settings, and
/// remove the first so the second takes the line over.
pub fn opposed_motors(seed: u64) -> Scenario {
    let first = [-1, 8, 0];
    let second = [3, 8, 0];
    let speeds = [32, -32, 16, -16, 64, -64];
    let start = speeds[(seed / 64 % 6) as usize];
    let blocks = vec![
        Placed {
            pos: first,
            state: "create:creative_motor[facing=east]".into(),
            motor: Some(32),
        },
        Placed {
            pos: [0, 8, 0],
            state: "create:shaft[axis=x]".into(),
            motor: None,
        },
        Placed {
            pos: [2, 8, 0],
            state: "create:shaft[axis=x]".into(),
            motor: None,
        },
        Placed {
            pos: second,
            state: "create:creative_motor[facing=west]".into(),
            motor: Some(start),
        },
    ];
    let setting = |tick, row, value| action(tick, second, Action::SpeedSetting { row, value });
    let events = vec![
        action(5, first, Action::GameMode { creative: true }),
        use_item(
            8,
            [0, 8, 0],
            "create:shaft",
            Face::East,
            Face::East.point(0.5, 0.5),
            false,
        ),
        setting(14, 0, 16),
        setting(20, 0, 64),
        setting(26, 0, 0),
        setting(32, 1, 32),
        action(38, first, Action::Remove),
        setting(44, 0, 32),
    ];
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 120,
        checkpoints: vec![7, 9, 10, 15, 21, 27, 33, 39, 45],
        blocks,
        events,
    }
}

/// Wrench encased cogs on every axis twice from each face across it, so each turn
/// swaps or keeps their shaft caps (EncasedCogwheelBlock.swapShaftsForRotation for
/// every clockwise turn the wrench makes).
pub fn encased_wrench(seed: u64) -> Scenario {
    let size = ["cogwheel", "large_cogwheel"][((seed >> 6) & 1) as usize];
    let casing = ["andesite", "brass"][((seed >> 7) & 1) as usize];
    let axes = [
        ("x", Face::Up, Face::North),
        ("y", Face::East, Face::North),
        ("z", Face::Up, Face::East),
    ];
    let mut blocks = Vec::new();
    let mut events = Vec::new();
    let mut checkpoints = Vec::new();
    for (i, (axis, first, second)) in axes.into_iter().enumerate() {
        let pos = [i as i32 * 5 - 5, 8, 0];
        blocks.push(Placed {
            pos,
            state: format!(
                "create:{casing}_encased_{size}[axis={axis},bottom_shaft=false,top_shaft=true]"
            ),
            motor: None,
        });
        let wrench = |tick, face: Face| {
            use_item(
                tick,
                pos,
                "create:wrench",
                face,
                face.point(0.5, 0.5),
                false,
            )
        };
        let base = 6 + 12 * i as u32;
        events.extend([
            wrench(base, first),
            wrench(base + 2, first),
            wrench(base + 4, second),
            wrench(base + 6, second),
        ]);
        checkpoints.extend([base + 1, base + 3, base + 5, base + 7]);
    }
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 115,
        checkpoints,
        blocks,
        events,
    }
}

/// Build a second chain on top of a driven one, so the drives touch across their
/// connection axis without joining (ChainDriveBlock.areBlocksConnected), then break a
/// lower drive under a running chain.
pub fn stacked_chains(seed: u64) -> Scenario {
    let drives = [
        "create:encased_chain_drive",
        "create:adjustable_chain_gearshift",
    ];
    let kind = |z: i32, level: u64| drives[((z as u64 + level + (seed >> 6)) & 1) as usize];
    let place = |tick, pos: Pos, item, yaw: f32| {
        let mut change = use_item(tick, pos, item, Face::Up, Face::Up.point(0.5, 0.5), true);
        if let Some(Action::Use { yaw: y, .. }) = &mut change.action {
            *y = yaw;
        }
        change
    };
    let crank = [-2, 8, 0];
    let shaft = [-1, 8, 0];
    let mut blocks: Vec<Placed> = (0..3)
        .map(|z| Placed {
            pos: [0, 7, z],
            state: "minecraft:stone".into(),
            motor: None,
        })
        .collect();
    blocks.push(Placed {
        pos: shaft,
        state: "create:shaft[axis=x]".into(),
        motor: None,
    });
    let mut events: Vec<Change> = (0..3)
        .map(|z| place(4 + z as u32, [0, 7, z], kind(z, 0), 90.))
        .collect();
    events.push(use_item(
        8,
        shaft,
        "create:hand_crank",
        Face::West,
        Face::West.point(0.5, 0.5),
        false,
    ));
    let turn = |tick| {
        use_item(
            tick,
            crank,
            "minecraft:air",
            Face::West,
            Face::West.point(0.5, 0.5),
            false,
        )
    };
    events.push(turn(10));
    events.extend((0..3).map(|z| place(12 + z as u32, [0, 8, z], kind(z, 1), 90.)));
    events.extend([
        turn(18),
        turn(26),
        action(28, [0, 8, 1], Action::Remove),
        turn(32),
    ]);
    Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 110,
        checkpoints: vec![9, 11, 13, 14, 15, 16, 19, 27, 29, 33],
        blocks,
        events,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn player_casing_registers_both_cog_sizes() {
        for (seed, name) in [(5, "cogwheel"), (69, "large_cogwheel")] {
            let case = super::relays(seed);
            let snapshot = crate::engine::run(&case);
            assert_eq!(
                snapshot.checkpoints[&26].blocks["0,8,0"].state,
                format!(
                    "create:andesite_encased_{name}[axis=y,bottom_shaft=false,top_shaft=false]"
                )
            );
        }
    }

    #[test]
    fn controller_uses_a_player_crank_and_breaks_the_overspeed_gear() {
        let snapshot = crate::engine::run(&super::controller_cog(6));
        let loaded = &snapshot.checkpoints[&30].blocks;
        assert_eq!(
            loaded["-2,8,0"].state,
            "create:hand_crank[facing=west,waterlogged=false]"
        );
        assert_eq!(loaded["1,10,0"].kinetic.as_ref().unwrap().speed, -256.);
        assert_eq!(
            snapshot.checkpoints[&40].blocks["0,9,0"].state,
            "minecraft:air"
        );
    }
}
