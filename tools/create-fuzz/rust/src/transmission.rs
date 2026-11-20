//! Player kinetic-board traces, including controller input from either shaft or cog.
use crate::scenario::Scenario;

pub fn crank(seed: u64) -> Scenario {
    let events: Vec<_> = [(20, false), (29, false), (39, true), (50, false)]
        .into_iter()
        .map(|(tick, sneaking)| {
            serde_json::json!({
            "tick":tick,"pos":[0,8,0],"action":{"kind":"use","item":"minecraft:air",
            "face":"up","hit":[0.5,1,0.5],"yaw":0,"pitch":0,"sneaking":sneaking}})
        })
        .collect();
    serde_json::from_value(serde_json::json!({"version":3,"seed":seed,"ticks":160,
        "checkpoints":(20..53).collect::<Vec<_>>(),"blocks":[
        {"pos":[0,7,0],"state":"create:shaft[axis=y,waterlogged=false]"},
        {"pos":[0,8,0],"state":"create:hand_crank[facing=up,waterlogged=false]"},
        {"pos":[0,6,0],"state":"create:shaft[axis=y,waterlogged=false]"}],"events":events}))
    .unwrap()
}

pub fn generate(seed: u64) -> Scenario {
    let reverse = seed & 16 != 0;
    let mut blocks = serde_json::json!([
        {"pos":[0,7,0],"state":"minecraft:stone"},
        {"pos":[0,9,0],"state":"create:large_cogwheel[axis=z,waterlogged=false]"},
        {"pos":[1,8,0],"state":"create:shaft[axis=x,waterlogged=false]"},
        {"pos":[2,8,0],"state":"create:mechanical_press[facing=east]"}
    ]);
    blocks.as_array_mut().unwrap().push(if reverse {
        serde_json::json!({"pos":[0,9,-1],"state":"create:creative_motor[facing=south]","motor":32})
    } else {
        serde_json::json!({"pos":[-1,8,0],"state":"create:creative_motor[facing=east]","motor":32})
    });
    let motor = if reverse { [0, 9, -1] } else { [-1, 8, 0] };
    let mut events = vec![use_block(
        10,
        [0, 7, 0],
        "create:rotation_speed_controller",
        0.,
        0.,
        false,
    )];
    for (i, value) in [0, 1, 16, 128, 256].into_iter().enumerate() {
        for row in 0..2 {
            events.push(serde_json::json!({"tick":20+i*16+row*8,"pos":[0,8,0],
                "action":{"kind":"speed_setting","row":row,"value":value}}));
        }
    }
    events.push(serde_json::json!({"tick":110,"pos":motor,"action":{"kind":"speed_setting","row":0,"value":64}}));
    let mut value = serde_json::json!({"version":3,"seed":seed,"ticks":190,
        "checkpoints":[19,28,44,60,76,92,120],"blocks":blocks,"events":events});
    if seed & 32 != 0 {
        for block in value["blocks"].as_array_mut().unwrap() {
            let pos = block["pos"].as_array_mut().unwrap();
            pos.swap(0, 2);
            let state = block["state"]
                .as_str()
                .unwrap()
                .replace("axis=x", "axis=TEMP")
                .replace("axis=z", "axis=x")
                .replace("axis=TEMP", "axis=z")
                .replace("facing=east", "facing=TEMP")
                .replace("facing=south", "facing=east")
                .replace("facing=TEMP", "facing=south");
            block["state"] = serde_json::json!(state);
        }
        for event in value["events"].as_array_mut().unwrap() {
            event["pos"].as_array_mut().unwrap().swap(0, 2);
        }
    }
    serde_json::from_value(value).unwrap()
}

fn use_block(
    tick: u32,
    pos: [i32; 3],
    item: &str,
    yaw: f32,
    pitch: f32,
    sneaking: bool,
) -> serde_json::Value {
    serde_json::json!({"tick":tick,"pos":pos,"action":{"kind":"use","item":item,"face":"up","hit":[0.5,1,0.5],"yaw":yaw,"pitch":pitch,"sneaking":sneaking}})
}

pub fn chain(seed: u64) -> Scenario {
    let mut blocks = Vec::new();
    let mut events = Vec::new();
    for z in 0..5 {
        blocks.push(serde_json::json!({"pos":[0,7,z],"state":"minecraft:stone"}));
        events.push(use_block(
            10 + z as u32 * 2,
            [0, 7, z],
            if (z + ((seed >> 5) & 1) as i32) % 2 == 0 {
                "create:encased_chain_drive"
            } else {
                "create:adjustable_chain_gearshift"
            },
            90.,
            0.,
            true,
        ));
    }
    blocks.push(serde_json::json!({"pos":[-1,8,0],"state":"create:creative_motor[facing=east]","motor":if seed&16==0 {32} else {-32}}));
    let (left, right) = if seed & 32 == 0 { (1, 3) } else { (0, 4) };
    for (i, strength) in (0..16).chain([0, 1, 14, 15, 14, 1, 0]).enumerate() {
        events.push(serde_json::json!({"tick":30+i*4,"pos":[0,9,left],"action":{"kind":"signal","strength":strength}}));
        events.push(serde_json::json!({"tick":30+i*4,"pos":[0,9,right],"action":{"kind":"signal","strength":15-strength}}));
    }
    events.push(serde_json::json!({"tick":130,"pos":[0,8,2],"action":{"kind":"remove"}}));
    events.push(use_block(
        140,
        [0, 7, 2],
        "create:encased_chain_drive",
        90.,
        0.,
        true,
    ));
    let mut value = serde_json::json!({"version":3,"seed":seed,"ticks":220,"checkpoints":[25,125,135,150],"blocks":blocks,"events":events});
    let axis = (seed / 64) % 3;
    if axis == 1 {
        for b in value["blocks"].as_array_mut().unwrap() {
            b["pos"].as_array_mut().unwrap().swap(0, 2);
            if b["state"].as_str().unwrap().contains("creative_motor") {
                b["state"] = serde_json::json!("create:creative_motor[facing=south]");
            }
        }
        for e in value["events"].as_array_mut().unwrap() {
            e["pos"].as_array_mut().unwrap().swap(0, 2);
            if e["action"]["kind"] == "use" {
                e["action"]["yaw"] = serde_json::json!(0);
            }
        }
    } else if axis == 2 {
        for b in value["blocks"].as_array_mut().unwrap() {
            if b["state"].as_str().unwrap().contains("creative_motor") {
                b["pos"] = serde_json::json!([0, 7, 0]);
                b["state"] = serde_json::json!("create:creative_motor[facing=up]");
            }
        }
        for e in value["events"].as_array_mut().unwrap() {
            if e["action"]["kind"] == "use" {
                e["action"]["pitch"] = serde_json::json!(90);
            }
        }
    }
    serde_json::from_value(value).unwrap()
}

pub fn sequence(seed: u64) -> Scenario {
    let modifier = if seed & 16 == 0 {
        "FORWARD_FAST"
    } else {
        "BACK_FAST"
    };
    let program = serde_json::json!({"kind":"program","instructions":[
        {"instruction":"TURN_ANGLE","modifier":modifier,"value":90},
        {"instruction":"DELAY","modifier":"FORWARD","value":3},
        {"instruction":"AWAIT","modifier":"FORWARD","value":-1},
        {"instruction":"TURN_DISTANCE","modifier":"BACK","value":1},
        {"instruction":"END","modifier":"FORWARD","value":-1}]});
    let mut events = vec![serde_json::json!({"tick":10,"pos":[0,8,0],"action":program})];
    for (tick, strength) in [
        (20, 15),
        (23, 0),
        (25, 15),
        (27, 0),
        (45, 15),
        (47, 0),
        (80, 15),
        (82, 0),
        (110, 15),
        (112, 0),
    ] {
        events.push(serde_json::json!({"tick":tick,"pos":[0,9,0],"action":{"kind":"signal","strength":strength}}));
    }
    events.push(serde_json::json!({"tick":100,"pos":[0,8,0],"action":program}));
    serde_json::from_value(serde_json::json!({"version":3,"seed":seed,"ticks":190,
        "checkpoints":(19..140).collect::<Vec<_>>(),"blocks":[
        {"pos":[-1,8,0],"state":"create:creative_motor[facing=east]","motor":32},
        {"pos":[0,8,0],"state":"create:sequenced_gearshift[axis=x,vertical=false,state=0]"},
        {"pos":[1,8,0],"state":"create:shaft[axis=x,waterlogged=false]"},
        {"pos":[2,8,0],"state":"create:mechanical_press[facing=east]"}],"events":events}))
    .unwrap()
}

pub fn valve(seed: u64) -> Scenario {
    let mut events = Vec::new();
    for (i, value) in [0, 1, 45, 90, 135, 136, 180].into_iter().enumerate() {
        let t = 10 + i as u32 * 35;
        events.push(serde_json::json!({"tick":t,"pos":[0,8,0],"action":{"kind":"valve_setting","row":(seed+i as u64)%2,"value":value}}));
        for offset in [1, 2] {
            let mut event = use_block(
                t + offset,
                [0, 8, 0],
                "minecraft:air",
                0.,
                0.,
                seed & 16 != 0,
            );
            event["action"]["hit"] = serde_json::json!([0.1, 1, 0.1]);
            events.push(event);
        }
    }
    events.push(use_block(
        222,
        [0, 8, 0],
        "minecraft:red_dye",
        0.,
        0.,
        seed & 16 != 0,
    ));
    for (i, color) in create_engine::dye::DyeColor::ALL.into_iter().enumerate() {
        let mut event = use_block(
            260 + i as u32 * 2,
            [0, 8, 0],
            &format!("minecraft:{}_dye", color.name()),
            0.,
            0.,
            true,
        );
        event["action"]["hit"] = serde_json::json!([0.1, 1, 0.1]);
        events.push(event);
    }
    // activate rejects external power and the four-tick cooldown after use.
    // A short angle plus consecutive clicks crosses both rejection boundaries.
    events.push(serde_json::json!({"tick":300,"pos":[0,8,0],"action":{"kind":"valve_setting","row":1,"value":1}}));
    for tick in 301..345 {
        events.push(use_block(tick, [0, 8, 0], "minecraft:air", 0., 0., false));
    }
    events.push(serde_json::json!({"tick":320,"pos":[0,5,0],"state":"create:creative_motor[facing=up]","motor":32}));
    events.push(serde_json::json!({"tick":335,"pos":[0,5,0],"action":{"kind":"remove"}}));
    // The up-facing handle's gameplay hit shape is half-height (Create AllShapes).
    // A surface normal is not the boundary plane of its containing unit cell.
    for event in &mut events {
        if event["action"]["kind"] == "use" {
            event["action"]["hit"][1] = serde_json::json!(0.5);
        }
    }
    serde_json::from_value(serde_json::json!({"version":3,"seed":seed,"ticks":420,"checkpoints":(10..350).collect::<Vec<_>>(),"blocks":[
        {"pos":[0,7,0],"state":"create:shaft[axis=y,waterlogged=false]"},
        {"pos":[0,8,0],"state":"create:copper_valve_handle[facing=up,waterlogged=false]"},
        {"pos":[0,6,0],"state":"create:shaft[axis=y,waterlogged=false]"}],"events":events})).unwrap()
}

/// A redstone clock reverses a driven gearshift faster than its flicker tally
/// decays (KineticBlockEntity.onSpeedChanged adds five; tick removes one).
pub fn flicker(seed: u64) -> Scenario {
    let events: Vec<_> = (20..100).map(|tick| serde_json::json!({
        "tick":tick,"pos":[1,9,0],"action":{"kind":"signal","strength":if tick%2==0 {15} else {0}}
    })).collect();
    serde_json::from_value(serde_json::json!({"version":3,"seed":seed,"ticks":180,
        "checkpoints":(19..105).collect::<Vec<_>>(),"blocks":[
        {"pos":[0,8,0],"state":"create:creative_motor[facing=east]","motor":32},
        {"pos":[1,8,0],"state":"create:gearshift[axis=x,powered=false]"},
        {"pos":[2,8,0],"state":"create:shaft[axis=x,waterlogged=false]"}],"events":events}))
    .unwrap()
}

#[cfg(test)]
mod tests {
    #[test]
    fn controller_board_zero_column_selects_one_and_reverses() {
        let mut sc = super::generate(14);
        sc.checkpoints = vec![21, 29];
        let snap = crate::engine::run(&sc);
        assert_eq!(
            snap.checkpoints[&21].blocks["0,8,0"]
                .kinetic
                .as_ref()
                .unwrap()
                .speed_setting,
            Some(-1.)
        );
        assert_eq!(
            snap.checkpoints[&29].blocks["0,8,0"]
                .kinetic
                .as_ref()
                .unwrap()
                .speed_setting,
            Some(1.)
        );
    }
}

#[cfg(test)]
mod chain_tests {
    #[test]
    fn every_signal_targets_a_placed_adjustable_gearshift() {
        use crate::actions::Action;
        for seed in (12..204).step_by(16) {
            let sc = super::chain(seed);
            for event in &sc.events {
                if matches!(event.action, Some(Action::Signal { .. })) {
                    assert!(sc.events.iter().any(|placed| matches!(&placed.action,Some(Action::Use {item,..}) if item=="create:adjustable_chain_gearshift")
                        && event.pos==[placed.pos[0],placed.pos[1]+2,placed.pos[2]]));
                }
            }
        }
    }
}
