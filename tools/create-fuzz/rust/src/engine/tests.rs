#[test]
fn busy_valve_box_passes_a_centred_dye_to_the_block() {
    // interaction/values.rs: `ValveHandleBlockEntity.showValue` hides the box while in use.
    let sc: crate::scenario::Scenario =
        serde_json::from_value(serde_json::json!({"ticks":80,"blocks":[
            {"pos":[0,7,0],"state":"minecraft:stone"},
            {"pos":[0,8,0],"state":"create:copper_valve_handle[facing=up,waterlogged=false]"}]}))
        .unwrap();
    let mut r = super::Run::new(&sc);
    for b in &sc.blocks {
        r.place(b);
    }
    let p = [0, 8, 0];
    let face = create_engine::kinetics::Dir::Up;
    r.use_item(p, "minecraft:air", face, [0.1, 0.5, 0.1], 0., 0., false);
    assert!(r.handles[&p].remaining > 0);
    r.use_item(p, "minecraft:red_dye", face, [0.5, 0.5, 0.5], 0., 0., false);
    assert_eq!(
        r.w.block_at(0, p),
        Some(create_engine::Block::RedValveHandle)
    );
}

#[test]
fn late_program_packet_cannot_reconfigure_a_replacement_motor() {
    let sc: crate::scenario::Scenario = serde_json::from_value(serde_json::json!({"ticks":80,
            "blocks":[{"pos":[0,8,0],"state":"create:sequenced_gearshift[axis=x,vertical=false,state=0]"}],
            "events":[{"tick":5,"pos":[0,8,0],"state":"create:creative_motor[facing=east]","motor":32}]})).unwrap();
    let mut r = super::Run::new(&sc);
    let p = [0, 8, 0];
    r.place(&sc.blocks[0]);
    r.place(&sc.events[0].placed().unwrap());
    let previous = r.w.sequencer_instructions(0, p);
    r.action(
        p,
        &crate::actions::Action::Program {
            instructions: vec![crate::program::Instruction {
                instruction: crate::program::Kind::Delay,
                modifier: crate::program::Modifier::Forward,
                value: 5,
            }],
        },
    );
    assert_eq!(r.w.sequencer_instructions(0, p), previous);
    assert_eq!(
        r.w.block_at(0, p),
        Some(create_engine::Block::CreativeMotor)
    );
}

#[test]
fn replaced_sequencer_has_only_the_current_blocks_observations() {
    let sc: crate::scenario::Scenario = serde_json::from_value(serde_json::json!({
            "ticks":80,"blocks":[{"pos":[0,8,0],"state":"create:sequenced_gearshift[axis=x,vertical=false,state=0]"}],
            "events":[{"tick":5,"pos":[0,8,0],"state":"create:creative_motor[facing=east]","motor":32}]})).unwrap();
    let snap = super::run(&sc);
    let k = snap.blocks["0,8,0"].kinetic.as_ref().unwrap();
    assert_eq!(k.speed_setting, Some(32.));
    assert!(k.sequencer.is_none());
}

#[test]
fn client_only_power_commit_does_not_enqueue_neighbour_inputs() {
    let sc: crate::scenario::Scenario = serde_json::from_value(serde_json::json!({
            "ticks":80,"blocks":[
            {"pos":[0,8,0],"state":"create:adjustable_chain_gearshift[axis=x,axis_along_first=false,part=none,powered=false]"},
            {"pos":[0,8,1],"state":"create:adjustable_chain_gearshift[axis=x,axis_along_first=false,part=none,powered=false]"}]})).unwrap();
    let mut r = super::Run::new(&sc);
    for b in &sc.blocks {
        r.place(b);
    }
    // Complete the placement-time initial signal read before changing power.
    for block in &sc.blocks {
        r.w.set_chain_signal(0, block.pos, 0);
    }
    r.w.take_redstone(0);
    let p = [0, 8, 0];
    let original = r.at(p);
    let powered = r.id(&crate::scenario::with_prop(
        &r.name(original),
        "powered",
        "true",
    ));
    r.commit(p, powered, false);
    // The current engine may requeue this block's own initial signal read when zero
    // has no settings entry. UPDATE_CLIENTS must still suppress neighbour inputs.
    assert!(!r.w.take_redstone(0).contains(&[0, 8, 1]));
    r.set(p, original);
    assert!(r.w.take_redstone(0).contains(&[0, 8, 1]));
}

#[test]
fn network_observation_uses_driven_positive_split_half() {
    let sc: crate::scenario::Scenario = serde_json::from_value(serde_json::json!({
        "ticks": 80,
        "blocks": [
            {"pos":[1,8,0],"state":"create:creative_motor[facing=west]","motor":32},
            {"pos":[0,9,0],"state":"minecraft:redstone_block"},
            {"pos":[0,8,0],"state":"create:clutch[axis=x,powered=true]"},
            {"pos":[-1,8,0],"state":"create:shaft[axis=x]"}
        ]
    }))
    .unwrap();
    let snapshot = super::run(&sc);
    let networks = snapshot.networks.unwrap();
    assert_eq!(networks.partitions, vec![vec!["0,8,0", "1,8,0"]]);
    assert_eq!(networks.sizes["0,8,0"], 2);
    assert_eq!(
        snapshot.blocks["0,8,0"].kinetic.as_ref().unwrap().speed,
        -32.0
    );
}
#[test]
fn network_observations_group_identity_and_follow_removal() {
    let mut sc: crate::scenario::Scenario = serde_json::from_value(serde_json::json!({
        "ticks": 80,
        "blocks": [
            {"pos":[0,8,0],"state":"create:creative_motor[facing=east]","motor":32},
            {"pos":[1,8,0],"state":"create:shaft[axis=x]"},
            {"pos":[0,8,4],"state":"create:creative_motor[facing=east]","motor":32},
            {"pos":[1,8,4],"state":"create:shaft[axis=x]"},
            {"pos":[4,8,0],"state":"create:shaft[axis=x]"}
        ]
    }))
    .unwrap();
    let snapshot = super::run(&sc);
    assert!(snapshot.unavailable.is_empty());
    let networks = snapshot.networks.unwrap();
    assert_eq!(
        networks.partitions,
        vec![vec!["0,8,0", "1,8,0"], vec!["0,8,4", "1,8,4"]]
    );
    assert_eq!(networks.sizes.len(), 4);
    assert!(networks.sizes.values().all(|size| *size == 2));
    sc.events
        .push(crate::scenario::Change::set(10, [0, 8, 0], "minecraft:air"));
    let networks = super::run(&sc).networks.unwrap();
    assert_eq!(networks.partitions, vec![vec!["0,8,4", "1,8,4"]]);
    assert_eq!(networks.sizes.len(), 2);
}

#[test]
fn connector_builds_driven_chain_and_keeps_casing_kind() {
    let sc = crate::belts::generate(15);
    let snap = super::run(&sc);
    let at = &snap.checkpoints[&75].blocks["0,8,0"];
    assert_eq!(at.belt.as_ref().unwrap().length, 3);
    assert_eq!(at.belt.as_ref().unwrap().casing, "brass");
    assert_ne!(at.kinetic.as_ref().unwrap().speed, 0.);
    assert!(snap.blocks.values().all(|b| b.belt.is_none()));
}
#[test]
fn remote_conveyor_links_form_loop_and_disconnect_symmetrically() {
    let sc = crate::belts::generate(127);
    let snap = super::run(&sc);
    assert_eq!(
        snap.checkpoints[&40].blocks["0,8,0"]
            .conveyor_links
            .as_ref()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        snap.checkpoints[&80].blocks["0,8,0"]
            .conveyor_links
            .as_ref()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        snap.blocks["0,8,0"].conveyor_links.as_ref().unwrap().len(),
        2
    );
}

#[test]
fn belt_break_effects_finish_the_glue_cascade_in_the_same_tick() {
    let sc = crate::belts::generate(15);
    let snap = super::run(&sc);
    assert!(snap.checkpoints[&101].blocks["0,8,2"].belt.is_none());
    assert!(snap.blocks["0,8,2"].state.starts_with("create:shaft"));
}
#[test]
fn pulley_switch_preserves_engine_network_capacity() {
    let sc:crate::scenario::Scenario=serde_json::from_str(r#"{"version":3,"ticks":80,"checkpoints":[51],"blocks":[{"pos":[-1,8,0],"state":"create:creative_motor[facing=east]","motor":32},{"pos":[0,8,0],"state":"create:shaft[axis=x,waterlogged=false]"},{"pos":[0,8,2],"state":"create:shaft[axis=x,waterlogged=false]"}],"events":[{"tick":10,"pos":[0,8,0],"action":{"kind":"use","item":"create:belt_connector","face":"up","hit":[0.5,1,0.5],"yaw":0,"pitch":0,"sneaking":false}},{"tick":20,"pos":[0,8,2],"action":{"kind":"use","item":"create:belt_connector","face":"up","hit":[0.5,1,0.5],"yaw":0,"pitch":0,"sneaking":false}},{"tick":50,"pos":[0,8,1],"action":{"kind":"use","item":"create:shaft","face":"up","hit":[0.5,1,0.5],"yaw":0,"pitch":0,"sneaking":false}}]}"#).unwrap();
    let snap = super::run(&sc);
    assert_eq!(
        snap.checkpoints[&51].blocks["0,8,1"]
            .kinetic
            .as_ref()
            .unwrap()
            .capacity,
        524288.
    );
    assert_eq!(
        snap.checkpoints[&51].blocks["0,8,1"]
            .kinetic
            .as_ref()
            .unwrap()
            .speed,
        32.
    );
}

#[test]
fn player_chain_placement_keeps_declared_default_properties() {
    let sc: crate::scenario::Scenario = serde_json::from_value(serde_json::json!({
        "ticks": 80, "blocks": []
    }))
    .unwrap();
    let mut r = super::Run::new(&sc);
    for (x, block) in [
        "create:encased_chain_drive",
        "create:adjustable_chain_gearshift",
    ]
    .into_iter()
    .enumerate()
    {
        let p = [x as i32 * 3, 8, 0];
        r.place_player(p, block, 90., 0., create_engine::kinetics::Dir::Up, false);
        let (_, placed) = crate::scenario::parse_state(&r.name(r.at(p)));
        for (property, value) in crate::defaults::declared(block) {
            if property != "axis" {
                assert!(
                    placed.contains(&(property.clone(), value.clone())),
                    "{block}: {property} must retain declared {value}"
                );
            }
        }
    }
}

#[test]
fn an_empty_running_belt_sets_its_direction_before_the_first_landing() {
    let sc = crate::transport::generate(31);
    let snap = super::run(&sc);
    let t = &snap.checkpoints[&41].blocks["0,8,0"]
        .transport
        .as_ref()
        .unwrap()
        .items;
    assert_eq!(t.len(), 1);
    // BeltInventory.tick set beltMovementPositive while empty: -1/16, then the tick moves 32/480.
    assert!((t[0].position - (1.5 - 1. / 16. + 32. / 480.)).abs() < 0.00001);
    assert!(
        snap.item_outcomes[1].remainder.is_some(),
        "immediate second landing is refused"
    );
}

#[test]
fn depot_use_put_swap_and_take_are_glue_owned_motion_and_stacks() {
    use crate::{
        actions::Face,
        transport::{Input, Stack},
    };
    let sc:crate::scenario::Scenario=serde_json::from_value(serde_json::json!({"ticks":80,"blocks":[{"pos":[0,8,0],"state":"create:depot[waterlogged=false]"}]})).unwrap();
    let mut r = super::Run::new(&sc);
    r.place(&sc.blocks[0]);
    let stack = Stack {
        item: "minecraft:iron_ingot".into(),
        count: 16,
    };
    r.item_input(
        [0, 8, 0],
        &Input::Use {
            stack: Some(stack.clone()),
            face: Face::Up,
            yaw: 0.,
            sneaking: false,
        },
    );
    assert_eq!(r.w.dims[0].depots[&[0, 8, 0]].unwrap().position, 0.25);
    r.tick();
    assert_eq!(r.w.dims[0].depots[&[0, 8, 0]].unwrap().position, 0.3125);
    r.item_input(
        [0, 8, 0],
        &Input::Use {
            stack: None,
            face: Face::Up,
            yaw: 0.,
            sneaking: false,
        },
    );
    assert_eq!(r.item_outcomes[1].taken, vec![stack]);
    assert!(r.depot_stacks.is_empty());
}

#[test]
fn an_empty_belt_receives_and_moves_within_the_tick() {
    let snap = super::run(&crate::transport::generate(95));
    let second = snap
        .checkpoints
        .values()
        .filter_map(|s| s.blocks["0,8,5"].transport.as_ref())
        .find(|s| !s.items.is_empty())
        .unwrap();
    assert!(
        !second.items[0].pending,
        "the receiving belt ticks after the sender and takes its insert in the same tick"
    );
    assert!(
        snap.blocks["0,8,5"]
            .transport
            .as_ref()
            .unwrap()
            .items
            .iter()
            .any(|i| i.stalled == Some(true))
    );
}

#[test]
fn observing_pending_items_does_not_flush_the_transport_queue() {
    use crate::transport::{Input, Stack};
    let mut sc = crate::transport::generate(31);
    sc.events.retain(|e| e.tick < 40);
    sc.ticks = 40;
    sc.checkpoints.clear();
    let mut r = super::Run::new(&sc);
    for b in &sc.blocks {
        r.place(b);
    }
    // Make the belt through the player's two connector clicks, not state injection.
    for e in &sc.events {
        if let Some(a) = &e.action {
            r.action(e.pos, a);
        }
        // Past the connector's 5-tick item cooldown before the next click.
        for _ in 0..5 {
            r.tick();
        }
    }
    r.tick();
    r.item_input(
        [0, 8, 1],
        &Input::DropOn {
            stack: Stack {
                item: "minecraft:iron_ingot".into(),
                count: 16,
            },
        },
    );
    let a = r.snapshot(&sc);
    let b = r.snapshot(&sc);
    assert_eq!(a, b);
    assert!(a.blocks["0,8,0"].transport.as_ref().unwrap().items[0].pending);
    assert!(r.w.dims[0].belts[&[0, 8, 0]].items.items.is_empty());
    r.tick();
    assert_eq!(r.w.dims[0].belts[&[0, 8, 0]].items.items.len(), 1);
}

#[test]
fn replacing_a_depot_drops_its_host_slot_state() {
    use crate::{
        actions::Face,
        transport::{Input, Stack},
    };
    let sc:crate::scenario::Scenario=serde_json::from_value(serde_json::json!({"ticks":80,"blocks":[{"pos":[0,8,0],"state":"create:depot[waterlogged=false]"}]})).unwrap();
    let mut r = super::Run::new(&sc);
    r.place(&sc.blocks[0]);
    r.item_input(
        [0, 8, 0],
        &Input::Use {
            stack: Some(Stack {
                item: "minecraft:diamond".into(),
                count: 1,
            }),
            face: Face::Up,
            yaw: 0.,
            sneaking: false,
        },
    );
    r.set([0, 8, 0], super::AIR);
    r.place(&sc.blocks[0]);
    assert!(r.depot_stacks.is_empty());
    assert!(
        r.snapshot(&sc).blocks["0,8,0"]
            .transport
            .as_ref()
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn chain_heading_boundary_uses_the_glues_axis_selection() {
    use create_engine::{
        Block,
        kinetics::{Axis, Dir},
        place,
    };
    let sc: crate::scenario::Scenario =
        serde_json::from_value(serde_json::json!({"ticks":80,"blocks":[]})).unwrap();
    let ctx = place::Context {
        yaw: 45.,
        pitch: 0.,
        sneaking: true,
        clicked_face: Dir::Up,
    };
    let (axis, _) = place::axis(
        Block::AdjustableChainGearshift,
        ctx,
        |_| false,
        place::Nearby::default(),
    )
    .unwrap();
    assert_eq!(axis, Axis::X);
    let mut r = super::Run::new(&sc);
    r.place_player(
        [0, 8, 0],
        "create:adjustable_chain_gearshift",
        45.,
        0.,
        Dir::Up,
        true,
    );
    assert_eq!(r.w.table.get(r.at([0, 8, 0])).unwrap().1.axis, Some(axis));
}

#[test]
fn adding_item_cases_preserves_the_conveyor_generator_lane() {
    assert!(
        crate::generate::generate(127)
            .events
            .iter()
            .any(|e| matches!(e.action, Some(crate::actions::Action::ChainLink { .. })))
    );
    assert!(
        crate::generate::generate(9)
            .events
            .iter()
            .any(|e| matches!(e.action, Some(crate::actions::Action::Item { .. })))
    );
}

#[test]
fn sneaking_casing_on_a_belt_reaches_normal_item_placement() {
    use crate::actions::{Action, Face};
    let mut sc = crate::transport::generate(31);
    sc.events.retain(|e| e.tick < 40);
    sc.checkpoints.clear();
    let mut r = super::Run::new(&sc);
    for b in &sc.blocks {
        r.place(b);
    }
    for e in &sc.events {
        if let Some(a) = &e.action {
            r.action(e.pos, a);
        }
        // Past the connector's 5-tick item cooldown before the next click.
        for _ in 0..5 {
            r.tick();
        }
    }
    r.tick();
    r.action(
        [0, 8, 0],
        &Action::Use {
            item: "create:brass_casing".into(),
            face: Face::Up,
            hit: [0.5, 1., 0.5],
            yaw: 0.,
            pitch: 0.,
            sneaking: true,
        },
    );
    assert_eq!(r.name(r.at([0, 9, 0])), "create:brass_casing");
    assert_eq!(r.belt_observation([0, 8, 0]).unwrap().casing, "none");
    assert!(r.notes.is_empty());
}

#[test]
fn empty_hand_on_an_unformed_shaft_has_no_transfer_or_adapter_gap() {
    use crate::{actions::Face, transport::Input};
    let sc = crate::transport::generate(31);
    let mut r = super::Run::new(&sc);
    for b in &sc.blocks {
        r.place(b);
    }
    r.item_input(
        [0, 8, 4],
        &Input::Use {
            stack: None,
            face: Face::Up,
            yaw: 0.,
            sneaking: false,
        },
    );
    assert!(r.item_outcomes[0].taken.is_empty());
    assert!(r.item_outcomes[0].remainder.is_none());
    assert!(r.notes.is_empty());
}

#[test]
fn depot_refunds_merge_into_the_selected_stack_before_placement() {
    use crate::{
        actions::Face,
        transport::{Input, Stack},
    };
    let sc = serde_json::from_value(serde_json::json!({"ticks":1,
        "blocks":[{"pos":[0,8,0],"state":"create:depot[waterlogged=false]"}]}))
    .unwrap();
    let mut r = super::Run::new(&sc);
    let pos = [0, 8, 0];
    r.place(&sc.blocks[0]);
    let stack = |item: &str, count| Stack {
        item: item.into(),
        count,
    };
    let use_hand = |r: &mut super::Run, hand| {
        r.item_input(
            pos,
            &Input::Use {
                stack: hand,
                face: Face::Up,
                yaw: 0.,
                sneaking: false,
            },
        )
    };
    use_hand(&mut r, Some(stack("minecraft:iron_ingot", 16)));
    use_hand(&mut r, Some(stack("minecraft:iron_ingot", 1)));
    assert_eq!(r.depot_stacks[&pos].count, 17);
    assert!(r.item_outcomes[1].taken.is_empty());
    assert!(r.item_outcomes[1].remainder.is_none());
    assert_eq!(r.w.dims[0].depots[&pos].unwrap().position, 0.25);
    use_hand(&mut r, None);
    assert_eq!(
        r.item_outcomes[2].taken,
        vec![stack("minecraft:iron_ingot", 17)]
    );
    assert!(r.item_outcomes[2].remainder.is_none());
    assert!(!r.depot_stacks.contains_key(&pos));

    // A full selected stack returns overflow to other inventory slots.
    use_hand(&mut r, Some(stack("minecraft:ender_pearl", 12)));
    use_hand(&mut r, Some(stack("minecraft:ender_pearl", 10)));
    assert_eq!(r.depot_stacks[&pos].count, 16);
    assert_eq!(
        r.item_outcomes.last().unwrap().taken,
        vec![stack("minecraft:ender_pearl", 6)]
    );
    use_hand(&mut r, Some(stack("minecraft:diamond", 2)));
    assert_eq!(r.depot_stacks[&pos], stack("minecraft:diamond", 2));
    assert_eq!(
        r.item_outcomes.last().unwrap().taken,
        vec![stack("minecraft:ender_pearl", 16)]
    );

    // Arms are collection-only even when a caller reaches this boundary directly
    // (they remain outside the current public transport item contract).
    use_hand(&mut r, Some(stack("create:mechanical_arm", 1)));
    assert!(!r.depot_stacks.contains_key(&pos));
    assert_eq!(
        r.item_outcomes.last().unwrap().remainder,
        Some(stack("create:mechanical_arm", 1))
    );
    assert_eq!(
        r.item_outcomes.last().unwrap().taken,
        vec![stack("minecraft:diamond", 2)]
    );

    // Component-bearing stacks cannot silently lose identity and merge by id.
    assert!(
        serde_json::from_value::<Stack>(serde_json::json!({
            "item":"minecraft:iron_ingot", "count":1, "components":{"minecraft:custom_name":"named"}
        }))
        .is_err()
    );
}

#[test]
fn sneak_wrench_removes_a_connected_sloped_belt() {
    // Java builds all five segments at tick 20; WrenchItem.useOn ->
    // IWrenchable.onSneakWrenched destroys the middle at tick 30.
    let sc: crate::scenario::Scenario = serde_json::from_value(serde_json::json!({
        "version":3,"ticks":35,"checkpoints":[21,31],"blocks":[
            {"pos":[0,8,0],"state":"create:shaft[axis=x,waterlogged=false]"},
            {"pos":[0,4,4],"state":"create:shaft[axis=x,waterlogged=false]"}
        ],"events":[
            {"tick":10,"pos":[0,8,0],"action":{"kind":"use","item":"create:belt_connector","face":"up","hit":[0.1,1.0,0.5],"yaw":266.0,"pitch":0.0,"sneaking":false}},
            {"tick":20,"pos":[0,4,4],"action":{"kind":"use","item":"create:belt_connector","face":"up","hit":[0.9,1.0,0.1],"yaw":339.0,"pitch":0.0,"sneaking":false}},
            {"tick":30,"pos":[0,6,2],"action":{"kind":"use","item":"create:wrench","face":"east","hit":[0.5,0.9,0.1],"yaw":288.0,"pitch":0.0,"sneaking":true}}
        ]
    })).unwrap();
    let snap = super::run(&sc);
    for (y, z) in [(8, 0), (7, 1), (6, 2), (5, 3), (4, 4)] {
        let p = format!("0,{y},{z}");
        assert!(
            snap.checkpoints[&21].blocks[&p]
                .state
                .starts_with("create:belt[")
        );
        let expected = if z == 0 || z == 4 {
            "create:shaft[axis=x,waterlogged=false]"
        } else {
            "minecraft:air"
        };
        assert_eq!(snap.checkpoints[&31].blocks[&p].state, expected);
    }
}
