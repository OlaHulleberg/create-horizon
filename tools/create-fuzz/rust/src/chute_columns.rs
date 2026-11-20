//! Columns of chutes at a belt's end: vertical and diagonal chutes (`ChuteBlock`, the shapes
//! `updateChuteState` gives them), a smart chute with a filter and redstone (`SmartChuteBlock`,
//! `canAcceptItem`, `canActivate`), and an encased fan under the column blowing its stacks up
//! (`getItemMotion`, `handleUpwardOutput`) into a depot over the top. The belt hands its stacks
//! into the top chute (`canDirectlyInsert`); they fall into a depot, onto stone, or out of the
//! bottom.
use crate::{
    actions::{Action, Face},
    generate::Rng,
    scenario::{Change, Placed, Pos, Scenario},
    transport::{Input, Stack},
};
use create_engine::{
    chute::{Shape, target, update_state},
    kinetics::Dir,
};
use std::collections::BTreeMap;

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
fn name(d: Dir) -> &'static str {
    match d {
        Dir::North => "north",
        Dir::South => "south",
        Dir::West => "west",
        Dir::East => "east",
        Dir::Up => "up",
        Dir::Down => "down",
    }
}
fn below(p: Pos) -> Pos {
    [p[0], p[1] - 1, p[2]]
}

const ITEMS: [&str; 3] = [
    "minecraft:iron_ingot",
    "minecraft:gold_ingot",
    "minecraft:diamond",
];

/// The chutes' settled states (`updateChuteState` until nothing changes), `smart` the smart
/// chute's position.
fn settle(chutes: &BTreeMap<Pos, Dir>, smart: Option<Pos>) -> BTreeMap<Pos, (Dir, Shape)> {
    let mut now: BTreeMap<Pos, (Dir, Shape)> = chutes
        .iter()
        .map(|(p, f)| (*p, (*f, Shape::Normal)))
        .collect();
    for _ in 0..8 {
        let facing = |q: Pos| now.get(&q).map(|s| s.0);
        let next: BTreeMap<Pos, (Dir, Shape)> = now
            .iter()
            .map(|(p, (f, s))| {
                if Some(*p) == smart {
                    return (*p, (Dir::Down, Shape::Normal));
                }
                (*p, update_state(*p, *f, *s, facing, false))
            })
            .collect();
        if next == now {
            break;
        }
        now = next;
    }
    now
}

pub fn generate(seed: u64) -> Scenario {
    let mut r = Rng::new(seed);
    let length = 3 + r.below(3) as i32;
    let speed = if r.chance(0.5) { 32 } else { -32 };
    let mut sc = Scenario {
        version: 3,
        seed,
        ticks: 300,
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
    let movement = create_engine::belt::movement_facing(speed as f32, Dir::South);
    let (top, start) = if movement == Dir::South {
        ([0, 8, length], 0)
    } else {
        ([0, 8, -1], length - 1)
    };
    // The column from the belt's end down: vertical chutes and diagonal ones across the belt's
    // line, the last one vertical.
    let mut chutes: BTreeMap<Pos, Dir> = BTreeMap::new();
    let mut at = top;
    let depth = 1 + r.below(3);
    for k in 0..=depth {
        let facing = if k < depth && r.chance(0.5) {
            *r.pick(&[Dir::East, Dir::West])
        } else {
            Dir::Down
        };
        chutes.insert(at, facing);
        at = target(at, facing);
    }
    let bottom = *chutes.keys().min_by_key(|p| p[1]).unwrap();
    // A diagonal chute over the top one leading into it makes the top a closed diagonal chute
    // (`isOpen`: down or an intersection), if the top leads across.
    let top_facing = chutes[&top];
    if top_facing != Dir::Down && r.chance(0.5) {
        let o = top_facing.offset();
        chutes.insert([top[0] + o[0], top[1] + 1, top[2] + o[2]], top_facing);
    }
    // A smart chute in a vertical place of the column.
    let vertical: Vec<Pos> = chutes
        .iter()
        .filter(|(p, f)| **f == Dir::Down && p[1] <= top[1])
        .map(|(p, _)| *p)
        .collect();
    let smart = (r.chance(0.6) && !vertical.is_empty()).then(|| *r.pick(&vertical));
    let settled = settle(&chutes, smart);
    for (p, (f, s)) in &settled {
        let st = if Some(*p) == smart {
            "create:smart_chute[powered=false]".to_string()
        } else {
            format!(
                "create:chute[facing={},shape={},waterlogged=false]",
                name(*f),
                s.name()
            )
        };
        // Placed before the belt, so Java ticks them before it as the port's logistics phase
        // does (the order itself is F5, #154).
        sc.blocks.push(placed(*p, &st, None));
    }
    // Under the bottom: a depot, stone, nothing (the stack drops out), or a fan blowing up with
    // a depot over the top to take what rises.
    let under = below(bottom);
    let fan = r.chance(0.3);
    if fan {
        sc.events
            .push(state(20, under, "create:encased_fan[facing=up]"));
        sc.blocks.push(placed(
            below(under),
            "create:creative_motor[facing=up]",
            Some(*r.pick(&[16, 32, 64, -32])),
        ));
        let over = [top[0], top[1] + 1, top[2]];
        if !chutes.contains_key(&over) {
            sc.blocks
                .push(placed(over, "create:depot[waterlogged=false]", None));
        }
    } else {
        match r.below(3) {
            0 => {
                sc.blocks
                    .push(placed(under, "create:depot[waterlogged=false]", None));
                sc.blocks
                    .push(placed(below(under), "minecraft:stone", None));
            }
            1 => sc.blocks.push(placed(under, "minecraft:stone", None)),
            _ => {}
        }
    }
    sc.checkpoints.extend([19, 21, 22, 25]);
    let mut tick = 30;
    let mut filter = None;
    if let Some(p) = smart {
        let item = *r.pick(&ITEMS);
        let side = *r.pick(&[Dir::North, Dir::South, Dir::East, Dir::West]);
        let hit = create_engine::chute::filter_slot(side).expect("a side slot");
        sc.events.push(act(
            tick,
            p,
            Action::Use {
                item: item.into(),
                face: match side {
                    Dir::North => Face::North,
                    Dir::South => Face::South,
                    Dir::East => Face::East,
                    _ => Face::West,
                },
                hit,
                yaw: 0.,
                pitch: 0.,
                sneaking: false,
            },
        ));
        filter = Some(item);
        tick += 2;
    }
    for _ in 0..2 + r.below(4) {
        let item = match filter {
            Some(f) if r.chance(0.6) => f,
            _ => *r.pick(&ITEMS),
        };
        sc.events.push(act(
            tick,
            [0, 8, start],
            Action::Item {
                input: Input::DropOn {
                    stack: Stack {
                        item: item.into(),
                        count: 1 + r.below(64) as u8,
                    },
                },
            },
        ));
        sc.checkpoints.extend(tick..tick + 40);
        tick += 15 + r.below(30);
        if let Some(p) = smart
            && r.chance(0.3)
        {
            // Redstone beside the smart chute holds its stack in the middle.
            let side = [p[0] + 1, p[1], p[2]];
            if !chutes.contains_key(&side) {
                sc.events
                    .push(state(tick, side, "minecraft:redstone_block"));
                let off = tick + 5 + r.below(30);
                sc.events.push(state(off, side, "minecraft:air"));
                sc.checkpoints.extend(off..off + 10);
            }
        }
    }
    let last = sc.events.iter().map(|e| e.tick).max().unwrap_or(0);
    sc.ticks = sc.ticks.max(tick + 60).max(last + crate::generate::SETTLE);
    sc.events.sort_by_key(|e| e.tick);
    sc.checkpoints.retain(|t| *t <= sc.ticks);
    sc.checkpoints.sort_unstable();
    sc.checkpoints.dedup();
    sc
}

#[cfg(test)]
mod tests {
    #[test]
    fn columns_lead_down_from_the_belt_end() {
        for seed in (35..35 + 128 * 60).step_by(128) {
            let sc = super::generate(seed);
            crate::transport::validate_scenario(&sc);
            for e in &sc.events {
                if let Some(a) = &e.action {
                    a.validate();
                }
            }
        }
    }
}
