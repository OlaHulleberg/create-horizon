//! A depot's output buffer (`DepotBehaviour.processingOutputBuffer`) as a spout fills it: a
//! stack of glass bottles (honey) or sweet berries (chocolate) lands on the depot under a spout,
//! and each filled item past the first goes into the buffer (`applyToAllItems`). Item handlers
//! then reach the buffer's slots (`DepotItemHandler`, slots 1..8) and the held slot, simulated
//! or not; inserts are refused while anything is there (`isOutputEmpty`); a player's empty
//! hand takes everything (`SharedDepotBlockMethods.onUse`); and a pulling belt funnel on the
//! depot exports the outputs, then the held stack, into the depot behind it
//! (`handleBeltFunnelOutput`). The world is the fluid items family's spout over a depot.
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
fn stack(item: &str, count: u8) -> Stack {
    Stack {
        item: item.into(),
        count,
    }
}
fn access(slot: u8, insert: Option<Stack>, extract: Option<u8>, simulate: bool) -> Action {
    Action::Capability {
        side: None,
        slot,
        insert,
        extract,
        simulate,
    }
}
fn drop(s: Stack) -> Action {
    Action::Item {
        input: Input::DropOn { stack: s },
    }
}

/// The depot under the spout in the fluid items family's spout worlds.
pub const DEPOT: Pos = [4, 4, 0];
/// Seeds of `fluid_inputs` whose world is a spout over a depot, filled with honey (mode 3,
/// fluid 2) or chocolate (mode 3, fluid 3).
const HONEY: u64 = 1395;
const CHOCOLATE: u64 = 627;

pub fn generate(seed: u64) -> Scenario {
    let mut r = Rng::new(seed);
    let honey = r.chance(0.5);
    let mut sc = crate::fluid_inputs::generate(if honey { HONEY } else { CHOCOLATE });
    sc.seed = seed;
    // The fluid family's single landing is replaced by this family's history.
    sc.events
        .retain(|e| !matches!(e.action, Some(Action::Item { .. })));
    let (item, max): (&str, u8) = if honey {
        ("minecraft:glass_bottle", 16)
    } else {
        ("minecraft:sweet_berries", 64)
    };
    let filled = if honey {
        "minecraft:honey_bottle"
    } else {
        "create:chocolate_glazed_berries"
    };
    // A pulling belt funnel on the depot (between it and the spout: funnels do not block the
    // spout, `BeltProcessingBehaviour.isBlocked`) with a depot behind it. Its back faces away
    // from the pipe along x = 0..3.
    let funnel = [DEPOT[0], DEPOT[1] + 1, DEPOT[2]];
    let mut behind = None;
    if r.chance(0.5) {
        let facing = *r.pick(&[Dir::North, Dir::South, Dir::West]);
        let back = create_engine::funnel::behind(funnel, facing);
        sc.blocks.push(Placed {
            pos: [back[0], back[1] - 1, back[2]],
            state: "minecraft:stone".into(),
            motor: None,
        });
        // Placed after the depot under the funnel, so Java ticks it after that depot hands it a
        // stack, as the port's funnel phase does (the order itself is F5, #154).
        let support = [back[0], back[1] - 1, back[2]];
        sc.events.push(act(
            45,
            support,
            Action::Use {
                item: "create:depot".into(),
                face: Face::Up,
                hit: [0.5, 1., 0.5],
                yaw: 90.,
                pitch: 0.,
                sneaking: false,
            },
        ));
        let name = match facing {
            Dir::North => "north",
            Dir::South => "south",
            _ => "west",
        };
        sc.events.push(Change {
            tick: 50,
            pos: funnel,
            action: None,
            state: Some(format!(
                "create:andesite_belt_funnel[facing={name},powered=false,shape=pulling,waterlogged=false]"
            )),
            motor: None,
            turn: None,
        });
        behind = Some(back);
    }
    let mut tick = 90 + r.below(20);
    sc.events.push(act(
        tick,
        DEPOT,
        drop(stack(item, 2 + r.below(u32::from(max) - 1) as u8)),
    ));
    sc.checkpoints = (tick..tick + 6).collect();
    tick += 30 + r.below(40);
    for _ in 0..6 + r.below(8) {
        let sim = r.chance(0.3);
        let (pos, a) = match r.below(12) {
            0..=3 => (
                DEPOT,
                access(1 + r.below(3) as u8, None, Some(1 + r.below(16) as u8), sim),
            ),
            4 => (DEPOT, access(0, None, Some(1 + r.below(64) as u8), sim)),
            5 => (
                DEPOT,
                access(0, Some(stack(item, 1 + r.below(8) as u8)), None, sim),
            ),
            6 => (
                DEPOT,
                access(
                    1 + r.below(8) as u8,
                    Some(stack(filled, 1 + r.below(4) as u8)),
                    None,
                    sim,
                ),
            ),
            7 => (
                DEPOT,
                Action::Item {
                    input: Input::Use {
                        stack: None,
                        face: Face::Up,
                        yaw: 0.,
                        sneaking: false,
                    },
                },
            ),
            8 | 9 => (DEPOT, drop(stack(item, 1 + r.below(u32::from(max)) as u8))),
            _ => match behind {
                // Room behind the funnel for the next export.
                Some(back) => (back, access(0, None, Some(64), false)),
                None => (DEPOT, access(1 + r.below(8) as u8, None, Some(64), sim)),
            },
        };
        sc.events.push(act(tick, pos, a));
        sc.checkpoints.extend(tick..tick + 4);
        tick += 15 + r.below(40);
    }
    sc.events.sort_by_key(|e| e.tick);
    let last = sc.events.iter().map(|e| e.tick).max().unwrap_or(0);
    sc.ticks = (last + crate::generate::SETTLE).max(sc.ticks);
    sc.checkpoints.extend((60..sc.ticks).step_by(5));
    sc.checkpoints.push(sc.ticks);
    sc.checkpoints.retain(|t| *t <= sc.ticks);
    sc.checkpoints.sort_unstable();
    sc.checkpoints.dedup();
    sc
}

#[cfg(test)]
mod tests {
    use crate::actions::Action;
    #[test]
    fn histories_reach_the_buffer_slots_after_the_landing() {
        // The fluid world runs on the native fluid host, so the scenario itself is checked here.
        for seed in (55..55 + 128 * 40).step_by(128) {
            let sc = super::generate(seed);
            let landing = sc
                .events
                .iter()
                .find(|e| matches!(e.action, Some(Action::Item { .. })))
                .expect("a landing")
                .tick;
            assert!(sc.events.iter().any(|e| matches!(
                e.action,
                Some(Action::Capability { slot: 1.., .. })
            ) || e.tick < landing));
            assert!(sc.ticks >= landing + crate::generate::SETTLE);
            assert_eq!(crate::generate::generate(seed).seed, seed);
        }
    }
}
