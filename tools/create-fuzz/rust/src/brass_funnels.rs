//! Brass funnels at belts: their filters (a click on the filter slot, or the release of a short
//! click the client sends while the amount board shows), their amount boards (up to N or
//! exactly N, `FilteringBehaviour.setValueSettings`) and redstone. A brass belt funnel takes
//! stacks off the belt into the depot behind it (`BeltFunnelInteractionHandler`: the filter,
//! an exact amount only from a stack that large and only when it all fits), pushes from that
//! depot onto the belt (`activateExtractingBeltFunnel`: what the filter passes, up to or exactly
//! the amount), or sits on a depot beside the belt pulling its stacks into the depot behind
//! (`handleBeltFunnelOutput`) or pushing onto it; a brass funnel facing up at the belt's end
//! takes what its filter passes (`handleDirectBeltInput`).
use crate::{
    actions::{Action, Face, FilterInteract},
    generate::Rng,
    scenario::{Change, Placed, Pos, Scenario},
    transport::{Input, Stack},
};
use create_engine::{funnel::BeltShape, kinetics::Dir};

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
        Dir::East => "east",
        Dir::Up => "up",
        Dir::Down => "down",
    }
}
fn face(d: Dir) -> Face {
    match d {
        Dir::North => Face::North,
        Dir::South => Face::South,
        Dir::West => Face::West,
        Dir::East => Face::East,
        Dir::Up => Face::Up,
        Dir::Down => Face::Down,
    }
}

const ITEMS: [&str; 3] = [
    "minecraft:iron_ingot",
    "minecraft:gold_ingot",
    "minecraft:diamond",
];

/// One brass funnel of the history: where it is, how it faces, its belt funnel shape and
/// whether its amount board shows (the filter is then set by the client's release).
struct Funnel {
    pos: Pos,
    facing: Dir,
    shape: Option<BeltShape>,
    board: bool,
}

impl Funnel {
    /// A click on its filter slot (`FunnelFilterSlotPositioning`) holding `item`.
    fn click(&self, item: &str) -> Action {
        let side = match self.shape {
            Some(BeltShape::Extended) => Dir::Up,
            None => Dir::North,
            _ => self.facing,
        };
        let hit = create_engine::funnel::filter_slot(self.facing, self.shape, side)
            .expect("the funnel's filter side");
        Action::Use {
            item: item.into(),
            face: face(side),
            hit,
            yaw: 0.,
            pitch: 0.,
            sneaking: false,
        }
    }
    /// Its filter set to `item` as a player does it.
    fn set_filter(&self, item: &str) -> Vec<Action> {
        let mut out = vec![self.click(item)];
        if self.board {
            out.push(Action::FilterSetting {
                row: 0,
                value: 0,
                interact: Some(FilterInteract {
                    item: item.into(),
                    face: Face::Up,
                }),
            });
        }
        out
    }
}

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
    let (end, start) = if movement == Dir::South {
        ([0, 8, length], 0)
    } else {
        ([0, 8, -1], length - 1)
    };
    let z = 1 + r.below((length - 2) as u32) as i32;
    // Stacks land on the belt's first segment, or on the depot a pushing funnel takes from.
    let mut drops: Pos = [0, 8, start];
    let funnel = match r.below(4) {
        // Taking off the belt: facing against it (retracted or extended), or pulling across.
        0 => {
            let (facing, shape) = if r.chance(0.5) {
                let s = if r.chance(0.5) {
                    BeltShape::Retracted
                } else {
                    BeltShape::Extended
                };
                (movement.opposite(), s)
            } else {
                (*r.pick(&[Dir::East, Dir::West]), BeltShape::Pulling)
            };
            Funnel {
                pos: [0, 9, z],
                facing,
                shape: Some(shape),
                board: true,
            }
        }
        // Pushing onto the belt from the depot behind: along it, or across it.
        1 => {
            let (facing, shape) = if r.chance(0.6) {
                let s = if r.chance(0.5) {
                    BeltShape::Retracted
                } else {
                    BeltShape::Extended
                };
                (movement, s)
            } else {
                (*r.pick(&[Dir::East, Dir::West]), BeltShape::Pushing)
            };
            let f = Funnel {
                pos: [0, 9, z],
                facing,
                shape: Some(shape),
                board: true,
            };
            drops = create_engine::funnel::behind(f.pos, facing);
            f
        }
        // On a depot beside the belt: pulling its stack into the depot behind, or pushing
        // onto it (only a pushing one has a board there).
        2 => {
            let push = r.chance(0.5);
            let facing = *r.pick(&[Dir::North, Dir::South, Dir::East]);
            let f = Funnel {
                pos: [3, 9, 1],
                facing,
                shape: Some(if push {
                    BeltShape::Pushing
                } else {
                    BeltShape::Pulling
                }),
                board: push,
            };
            sc.blocks
                .push(placed([3, 8, 1], "create:depot[waterlogged=false]", None));
            drops = if push {
                create_engine::funnel::behind(f.pos, facing)
            } else {
                [3, 8, 1]
            };
            f
        }
        // Facing up at the belt's end, over a depot.
        _ => Funnel {
            pos: end,
            facing: Dir::Up,
            shape: None,
            board: false,
        },
    };
    let behind = create_engine::funnel::behind(funnel.pos, funnel.facing);
    sc.blocks
        .push(placed(behind, "create:depot[waterlogged=false]", None));
    if funnel.shape.is_some() {
        sc.events.push(state(
            24,
            funnel.pos,
            &format!(
                "create:brass_belt_funnel[facing={},powered=false,shape={},waterlogged=false]",
                name(funnel.facing),
                match funnel.shape {
                    Some(BeltShape::Retracted) => "retracted",
                    Some(BeltShape::Extended) => "extended",
                    Some(BeltShape::Pushing) => "pushing",
                    _ => "pulling",
                }
            ),
        ));
        // The belt's end stops what is left on it.
        sc.blocks.push(placed(end, "minecraft:stone", None));
    } else {
        sc.events.push(state(
            24,
            funnel.pos,
            "create:brass_funnel[extracting=false,facing=up,powered=false,waterlogged=false]",
        ));
    }
    sc.checkpoints.extend([25, 27, 30]);
    let mut tick = 30;
    // A filter, and an amount where the board shows; sometimes changed later.
    let filtered = r.chance(0.75);
    let filter = *r.pick(&ITEMS);
    if filtered {
        for a in funnel.set_filter(filter) {
            sc.events.push(act(tick, funnel.pos, a));
        }
        sc.checkpoints.extend([tick, tick + 1]);
        tick += 2;
    }
    if r.chance(0.7) {
        sc.events.push(act(
            tick,
            funnel.pos,
            Action::FilterSetting {
                row: r.below(2) as i32,
                value: 1 + r.below(64) as i32,
                interact: None,
            },
        ));
        sc.checkpoints.extend([tick, tick + 1]);
        tick += 2;
    }
    for _ in 0..3 + r.below(4) {
        let pick = r.below(10);
        if pick < 6 {
            // Mostly the filter's item, so that it passes.
            let item = if filtered && r.chance(0.5) {
                filter
            } else {
                *r.pick(&ITEMS)
            };
            sc.events
                .push(act(tick, drops, drop(item, 1 + r.below(64) as u8)));
        } else if pick < 7 {
            // Empty the depot behind: a full one refuses further stacks.
            sc.events.push(act(
                tick,
                behind,
                Action::Item {
                    input: Input::Use {
                        stack: None,
                        face: Face::Up,
                        yaw: 0.,
                        sneaking: false,
                    },
                },
            ));
        } else if pick < 8 {
            let item = if r.chance(0.2) {
                "minecraft:air"
            } else {
                *r.pick(&ITEMS)
            };
            for a in funnel.set_filter(item) {
                sc.events.push(act(tick, funnel.pos, a));
            }
        } else if pick < 9 {
            sc.events.push(act(
                tick,
                funnel.pos,
                Action::FilterSetting {
                    row: r.below(2) as i32,
                    value: 1 + r.below(64) as i32,
                    interact: None,
                },
            ));
        } else {
            // Redstone beside the funnel pauses it (a blocking funnel holds the items).
            let side = [funnel.pos[0] - 1, funnel.pos[1], funnel.pos[2]];
            sc.events
                .push(state(tick, side, "minecraft:redstone_block"));
            let off = tick + 5 + r.below(25);
            sc.events.push(state(off, side, "minecraft:air"));
            sc.checkpoints.extend(off..off + 4);
        }
        sc.checkpoints.extend(tick..tick + 30);
        tick += 12 + r.below(30);
    }
    let last = sc.events.iter().map(|e| e.tick).max().unwrap_or(0);
    sc.ticks = sc.ticks.max(tick + 40).max(last + crate::generate::SETTLE);
    sc.events.sort_by_key(|e| e.tick);
    sc.checkpoints.retain(|t| *t <= sc.ticks);
    sc.checkpoints.sort_unstable();
    sc.checkpoints.dedup();
    sc
}

#[cfg(test)]
mod tests {
    #[test]
    fn filter_clicks_hit_the_slot() {
        for seed in (39..39 + 128 * 60).step_by(128) {
            let sc = super::generate(seed);
            for e in &sc.events {
                if let Some(a) = &e.action {
                    a.validate();
                }
            }
        }
    }
}
