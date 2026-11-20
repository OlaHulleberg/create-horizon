//! Player histories for fluid handlers, filters and item machines.
use crate::{
    actions::{Action, Face},
    scenario::{Change, Placed, Scenario},
    transport::{Input, Stack},
};
fn use_at(tick: u32, pos: [i32; 3], item: &str) -> Change {
    Change {
        tick,
        pos,
        state: None,
        motor: None,
        turn: None,
        action: Some(Action::Use {
            item: item.into(),
            face: Face::Up,
            hit: [0.5, 1., 0.5],
            yaw: 90.,
            pitch: 0.,
            sneaking: false,
        }),
    }
}
fn game(tick: u32, pos: [i32; 3], creative: bool) -> Change {
    Change {
        tick,
        pos,
        state: None,
        motor: None,
        turn: None,
        action: Some(Action::GameMode { creative }),
    }
}
fn drop_on(tick: u32, pos: [i32; 3], item: &str, count: u8) -> Change {
    Change {
        tick,
        pos,
        state: None,
        motor: None,
        turn: None,
        action: Some(Action::Item {
            input: Input::DropOn {
                stack: Stack {
                    item: item.into(),
                    count,
                },
            },
        }),
    }
}
fn support(pos: [i32; 3]) -> Placed {
    Placed {
        pos,
        state: "minecraft:stone".into(),
        motor: None,
    }
}
pub fn generate(seed: u64) -> Scenario {
    let mode = (seed / 32) % 8;
    let fluid = *[
        "minecraft:water_bucket",
        "minecraft:lava_bucket",
        "create:honey_bucket",
        "create:chocolate_bucket",
        "minecraft:milk_bucket",
    ]
    .get((seed / 192 % 5) as usize)
    .unwrap();
    if mode < 2 {
        let mut sc = Scenario {
            version: 3,
            seed,
            ticks: 200,
            day_time: 0,
            checkpoints: (10..=100).collect(),
            blocks: vec![support([0, 3, 0])],
            events: vec![],
        };
        sc.events.push(use_at(
            10,
            [0, 3, 0],
            if mode == 0 {
                "create:fluid_tank"
            } else {
                "create:creative_fluid_tank"
            },
        ));
        if mode == 0 {
            sc.events.push(game(20, [0, 4, 0], true));
        }
        for (tick, item) in [(25, fluid), (30, fluid), (45, "minecraft:bucket")] {
            sc.events.push(use_at(tick, [0, 4, 0], item));
        }
        sc.events.push(game(60, [0, 4, 0], false));
        sc.events.push(use_at(65, [0, 4, 0], "minecraft:bucket"));
        return sc;
    }
    let mut sc = crate::fluid_pumps::generate(18);
    sc.seed = seed;
    if mode == 2 {
        for e in &mut sc.events {
            if let Some(Action::Use { item, pitch, .. }) = &mut e.action
                && item == "create:fluid_pipe"
                && e.pos == [3, 3, 0]
            {
                *item = "create:smart_fluid_pipe".into();
                *pitch = 90.;
            }
        }
        let hit = create_engine::fluids::smart_pipe_filter_slot(
            create_engine::steam::Face::Floor,
            create_engine::kinetics::Dir::West,
        )
        .0;
        for (tick, item) in [
            (50, "minecraft:water_bucket"),
            (90, "minecraft:lava_bucket"),
            (130, "minecraft:air"),
            (150, "create:andesite_casing"),
            (170, "minecraft:air"),
        ] {
            let mut e = use_at(tick, [3, 4, 0], item);
            if let Some(Action::Use { hit: point, .. }) = &mut e.action {
                *point = hit;
            }
            sc.events.push(e);
        }
    } else if mode == 6 {
        for e in &mut sc.events {
            if let Some(Action::Use { item, yaw, .. }) = &mut e.action
                && item == "create:fluid_pipe"
                && e.pos == [3, 3, 0]
            {
                *item = "create:fluid_valve".into();
                *yaw = 180.;
            }
        }
        sc.blocks.push(Placed {
            pos: [3, 5, 0],
            state: "create:gearshift[axis=y,powered=false]".into(),
            motor: None,
        });
        sc.blocks.push(Placed {
            pos: [3, 6, 0],
            state: "create:creative_motor[facing=down]".into(),
            motor: Some(-64),
        });
        for (tick, strength) in [(50, 15), (140, 0), (200, 15)] {
            sc.events.push(Change {
                tick,
                pos: [4, 5, 0],
                state: None,
                motor: None,
                turn: None,
                action: Some(Action::Signal { strength }),
            });
        }
    } else if mode == 3 || mode == 5 || mode == 7 {
        for b in &mut sc.blocks {
            b.pos[1] += 2;
            if mode == 5 {
                b.pos[2] += 2;
            }
        }
        sc.blocks.retain(|b| !b.state.contains("cauldron"));
        for e in &mut sc.events {
            e.pos[1] += 2;
            if mode == 5 {
                e.pos[2] += 2;
            }
            if let Some(Action::Use { item, .. }) = &mut e.action
                && item == "create:fluid_tank"
            {
                *item = "create:spout".into();
            }
        }
        let z = if mode == 5 { 2 } else { 0 };
        if mode == 7 {
            sc.events.retain(|e| !matches!(&e.action, Some(Action::Use { item, .. }) if item == "minecraft:water_bucket"));
            sc.events.push(use_at(16, [0, 5, z], "create:item_drain"));
            sc.blocks.push(support([0, 5, -1]));
            sc.events.push(use_at(17, [0, 5, -1], "create:depot"));
            let bottle = if (seed / 256).is_multiple_of(2) {
                "minecraft:honey_bottle"
            } else {
                "create:builders_tea"
            };
            for tick in [60, 90, 120, 150] {
                sc.events.push(drop_on(tick, [0, 6, z], bottle, 1));
            }
        } else {
            sc.events
                .push(use_at(16, [0, 5, z], "create:creative_fluid_tank"));
            for e in &mut sc.events {
                if let Some(Action::Use { item, .. }) = &mut e.action
                    && item == "minecraft:water_bucket"
                {
                    *item = fluid.into();
                }
            }
        }
        sc.events.push(Change {
            tick: 40,
            pos: [4, 5, z],
            action: Some(Action::Remove),
            state: None,
            motor: None,
            turn: None,
        });
        if mode == 3 || mode == 7 {
            sc.blocks.push(support([4, 3, 0]));
            sc.events.push(use_at(17, [4, 3, 0], "create:depot"));
            sc.events.push(drop_on(
                190,
                [4, 4, 0],
                if mode == 7 {
                    "minecraft:glass_bottle"
                } else if fluid == "create:chocolate_bucket" {
                    "minecraft:sweet_berries"
                } else {
                    "minecraft:bucket"
                },
                if mode == 7 { 4 } else { 1 },
            ));
        } else {
            for z in [0, 4] {
                sc.blocks.push(Placed {
                    pos: [4, 4, z],
                    state: "create:shaft[axis=x,waterlogged=false]".into(),
                    motor: None,
                });
            }
            sc.blocks.push(Placed {
                pos: [3, 4, 0],
                state: "create:creative_motor[facing=east]".into(),
                motor: Some(32),
            });
            sc.events
                .push(use_at(18, [4, 4, 0], "create:belt_connector"));
            sc.events
                .push(use_at(24, [4, 4, 4], "create:belt_connector"));
            for z in [-1, 5] {
                sc.blocks.push(support([4, 3, z]));
                sc.events.push(use_at(17, [4, 3, z], "create:depot"));
            }
            sc.events
                .push(drop_on(190, [4, 4, 2], "minecraft:bucket", 1));
        }
    } else {
        sc.blocks.retain(|b| !b.state.contains("cauldron"));
        sc.blocks.push(support([0, 3, -1]));
        sc.events.retain(
            |e| !matches!(&e.action,Some(Action::Use{item,..})if item=="minecraft:water_bucket"),
        );
        sc.events.push(use_at(16, [0, 3, 0], "create:item_drain"));
        sc.events.push(use_at(17, [0, 3, -1], "create:depot"));
        for t in [60, 110] {
            sc.events.push(drop_on(t, [0, 4, 0], fluid, 1));
        }
    }
    sc.events.sort_by_key(|e| e.tick);
    sc.ticks = 360;
    sc.checkpoints = (15..=300).collect();
    sc
}

#[cfg(test)]
mod tests {
    #[test]
    fn redstone_reverses_a_connected_fluid_valve() {
        let mut sc = super::generate(211);
        sc.blocks.retain(|b| !b.state.contains("cauldron"));
        sc.events.retain(|e| !matches!(&e.action, Some(crate::actions::Action::Use { item, .. }) if item.ends_with("bucket")));
        assert!(!crate::fluid_world::enabled(&sc));
        let trace = crate::engine::run(&sc);
        for (tick, enabled, sign) in [
            (40, true, 1.),
            (70, false, -1.),
            (160, true, 1.),
            (220, false, -1.),
        ] {
            let valve = &trace.checkpoints[&tick].blocks["3,4,0"];
            assert!(
                valve.state.contains(if enabled {
                    "enabled=true"
                } else {
                    "enabled=false"
                }),
                "tick {tick}: {}",
                valve.state
            );
            let speed = valve.kinetic.as_ref().unwrap().speed;
            assert!(speed * sign > 0., "tick {tick}: {speed}");
        }
    }
}
