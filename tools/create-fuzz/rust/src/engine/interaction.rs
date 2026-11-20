use super::*;
use create_engine::{
    Block,
    kinetics::{Axis, Dir},
    place,
};

fn axis_name(axis: Axis) -> &'static str {
    match axis {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

fn cog_item(item: &str) -> Option<bool> {
    match item {
        "create:cogwheel" => Some(false),
        "create:large_cogwheel" => Some(true),
        _ => None,
    }
}

impl Run {
    pub(super) fn action(&mut self, p: Pos, action: &crate::actions::Action) {
        use crate::actions::Action;
        match action {
            Action::GameMode { creative } => self.creative = *creative,
            Action::Reload => self.reload(),
            Action::Item { input } => self.item_input(p, input),
            Action::Capability {
                slot,
                insert,
                extract,
                simulate,
                ..
            } => self.capability(p, *slot, insert.as_ref(), *extract, *simulate),
            Action::SpeedSetting { row, value } => {
                self.w.board_motor(
                    0,
                    p,
                    create_engine::board::Settings {
                        row: *row,
                        value: *value,
                    },
                );
                self.publish_interaction();
            }
            Action::ScrollSetting { row, value } => {
                let pick = create_engine::board::Settings {
                    row: *row,
                    value: *value,
                };
                match self.w.block_at(0, p) {
                    Some(create_engine::Block::BrassTunnel) => self.brass_option(p, pick),
                    Some(create_engine::Block::WeightedEjector) => self.ejector_stack_size(p, pick),
                    _ => {}
                }
                self.publish_interaction();
            }
            Action::ValveSetting { row, value } => {
                if let Some(h) = self.handles.get_mut(&p) {
                    h.selected = create_engine::board::kinetic_value(
                        create_engine::board::Settings {
                            row: *row,
                            value: *value,
                        },
                        180,
                    );
                }
                self.publish_interaction();
            }
            Action::Program { instructions } => {
                // sequencer::client_message rejects late screen packets after replacement.
                if self.w.block_at(0, p) != Some(create_engine::Block::SequencedGearshift) {
                    return;
                }
                if let Some(step) = self.w.set_sequencer_instructions(
                    0,
                    p,
                    instructions.iter().map(|i| i.engine()).collect(),
                    true,
                ) {
                    self.sequencer_step(p, step);
                    let list = &self.w.dims[0].sequencers[&p].instructions;
                    self.persisted.sequences.entry(p).or_default().insert(
                        "Instructions".into(),
                        crate::persistence::codec::to_tag(list),
                    );
                    self.mark_column(p);
                }
            }
            Action::FilterSetting {
                row,
                value,
                interact,
            } => self.funnel_value_settings(
                p,
                *row,
                *value,
                interact.as_ref().map(|i| i.item.as_str()),
            ),
            Action::ChainLink { target, connect } => self.chain_link(p, *target, *connect),
            Action::Remove => self.set(p, AIR),
            Action::Signal { strength } => self.set(p, 3 + u32::from(*strength)),
            Action::Use {
                item,
                face,
                hit,
                yaw,
                pitch,
                sneaking,
            } => {
                // The native host constructs an absolute hit, then the glue subtracts
                // the clicked block. Preserve that rounding in every batch slot.
                let point = [0, 1, 2].map(|i| {
                    let base = f64::from(self.native_origin[i] + p[i]);
                    (base + hit[i]) - base
                });
                self.use_item(p, item, face.dir(), point, *yaw, *pitch, *sneaking);
            }
        }
    }

    /// Glue order: RightClickBlock, block use_item_on, then the item's use_on.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn use_item(
        &mut self,
        p: Pos,
        item: &str,
        face: create_engine::kinetics::Dir,
        hit: [f64; 3],
        yaw: f32,
        pitch: f32,
        sneaking: bool,
    ) {
        use create_engine::board;
        if item == "minecraft:air" && self.transport_empty_use(p, face, sneaking) {
            return;
        }
        if self.belt_use(p, item, face, hit, sneaking) {
            return;
        }
        if self.tunnel_use(p, item, face, hit, sneaking) {
            return;
        }
        if self.funnel_filter_use(p, item, face, hit, sneaking)
            || self.smart_chute_filter_use(p, item, face, hit, sneaking)
        {
            return;
        }
        if self.fluid_use(p, item, face, hit, sneaking) {
            return;
        }
        if self.world_bucket(p, item, face, hit, sneaking) {
            return;
        }
        if self.pipe_use(p, item, face, hit, sneaking) {
            return;
        }
        let state = self.at(p);
        let own = self.w.table.get(state).map(|(b, i)| (b, *i));
        if let Some((block, info)) = own.filter(|(b, _)| create_engine::dye::is_valve_handle(*b)) {
            // ItemUseOverrides (glue/interaction/item_use_overrides), with the value box of
            // interaction/values.rs taking a click first, only while its behaviour is active.
            if !sneaking
                && self.w.value_box_active(0, p)
                && let Some(slot) = info.facing.and_then(|f| board::valve_slot(f, face))
                && board::hits_valve_box(slot, hit)
            {
                return;
            }
            if !(sneaking && item == "create:wrench") {
                if let Some(to) = create_engine::dye::DyeColor::of_item(item)
                    .and_then(|c| create_engine::dye::recoloured(block, c))
                {
                    let (_, props) = parse_state(&self.name(state));
                    let next =
                        crate::scenario::format_state(&format!("create:{}", to.name()), &props);
                    self.set(p, self.id(&next));
                    self.publish_interaction();
                } else {
                    let selected = self.handles.get(&p).map_or(45, |h| h.selected);
                    if self.w.use_valve(0, p, sneaking, selected) {
                        self.handles.entry(p).or_default().started(sneaking);
                        self.publish_interaction();
                    }
                }
                return;
            }
        }
        // ItemUseOverrides (glue/interaction/item_use_overrides), then the game's own use of
        // the block (glue/interaction/crank.rs): the crank's use takes the click whatever the
        // hand holds but the wrench; a sneaking player holding the wrench uses it instead.
        if !(sneaking && item == "create:wrench") && own.is_some_and(|(b, _)| b == Block::HandCrank)
        {
            self.w.turn_crank(0, p, sneaking);
            self.publish_interaction();
            return;
        }
        // WrenchItem.useOn dispatches a sneaking belt use to
        // IWrenchable.onSneakWrenched (destroyBlock), handled below like the glue.
        if let Some((block, info)) = own {
            let name = format!("create:{}", block.name());
            if block == Block::BlazeBurner && self.use_burner(p, item) {
                return;
            }
            let _ = info;
            if !crate::fluids::contract()
                .blocks
                .iter()
                .any(|entry| entry.id == name)
                && !crate::actions::BLOCKS.contains(&name.as_str())
                && name != "create:andesite_encased_shaft"
                && name != "create:brass_encased_shaft"
                && name != "create:powered_shaft"
                && !name.ends_with("_encased_cogwheel")
                && !name.ends_with("_encased_large_cogwheel")
                // A sneaking wrench picks a valve handle up as any wrenchable block.
                && !(create_engine::dye::is_valve_handle(block)
                    && sneaking
                    && item == "create:wrench")
                && name != "create:brass_casing"
                && name != "create:andesite_casing"
                && !(name == "create:belt"
                    && sneaking
                    && [
                        "create:andesite_casing",
                        "create:brass_casing",
                        "create:wrench",
                    ]
                    .contains(&item))
                // `use_belt_with` passed: a dye it did not change, or a shaft on a segment that
                // is no middle one, goes on to the item (`DyeItem` does nothing to a block, the
                // shaft is placed against the clicked face).
                && !(name == "create:belt"
                    && (item == "create:shaft"
                        || create_engine::dye::DyeColor::of_item(item).is_some()))
            {
                self.notes.push(format!("adapter gap: RightClickBlock/value-box and block-use handlers for {name} at {p:?} are outside the supported subset; input retained"));
                return;
            }
        }
        // interaction/values.rs: the motor's or controller's active value box consumes the
        // click first, whatever is held.
        if !sneaking
            && let Some((block, info)) = own
            && self.w.value_box_active(0, p)
            && let Some(slot) = match block {
                Block::CreativeMotor => info.facing.and_then(|f| board::motor_slot(f, face)),
                Block::RotationSpeedController => {
                    info.axis.and_then(|a| board::controller_slot(a, face))
                }
                _ => None,
            }
            && board::hits_box(slot, hit)
        {
            return;
        }
        if !sneaking {
            // An off-shaft click opens the program screen; shaft-facing clicks
            // carrying a kinetic block pass through to ordinary item placement.
            if own.is_some_and(|(block, _)| block == Block::SequencedGearshift)
                && item != "create:wrench"
                && !(crate::catalogue::entries()
                    .iter()
                    .any(|entry| entry.id == item)
                    && self.w.has_shaft_towards(0, p, face))
            {
                return;
            }
            if let Some(large) = cog_item(item)
                && let Some((clicked_large, axis)) = self.w.cog_at(0, p)
                && let Some((target, axis)) = place::cog_helper(
                    large,
                    p,
                    place::ClickedCog {
                        large: clicked_large,
                        axis,
                    },
                    hit,
                    |q, large, axis| self.w.valid_cog_position(0, q, large, axis),
                    |q| self.replaceable(q),
                )
            {
                self.place_cog(target, item, large, axis);
                return;
            }
            // SpeedControllerBlock's placement helper sends the held large cog above it.
            if item == "create:large_cogwheel"
                && let Some((Block::RotationSpeedController, info)) = own
            {
                let target = add(p, Dir::Up.offset());
                let axis = if info.axis == Some(Axis::X) {
                    Axis::Z
                } else {
                    Axis::X
                };
                if self.replaceable(target) && self.w.valid_cog_position(0, target, true, axis) {
                    self.place_cog(target, item, true, axis);
                    return;
                }
            }
            // decor/brackets.rs runs before the wrench item's use_on.
            if item == "create:wrench" && {
                let removed = self.brackets.remove(&p).is_some();
                if removed {
                    self.mark_column(p);
                }
                removed
            } {
                return;
            }
            // `SteamEngineBlock.useItemOn`: a shaft item goes through the engine's placement
            // helper, at the shaft two blocks in front, on an axis it can turn.
            if item == "create:shaft"
                && let Some((Block::SteamEngine, info)) = own
                && let (Some(f), Some(facing)) = (info.face, info.facing)
            {
                let connected = create_engine::steam::connected_direction(f, facing);
                let shaft = create_engine::steam::shaft_pos(p, connected);
                let mut axis = None;
                for d in place::ordered_by_nearest(yaw, pitch) {
                    if create_engine::steam::is_shaft_valid(connected, d.axis()) {
                        axis = Some(d.axis());
                        break;
                    }
                }
                if let Some(axis) = axis
                    && self.replaceable(shaft)
                {
                    let water = parse_state(&self.name(self.at(shaft)))
                        .1
                        .iter()
                        .any(|(k, v)| k == "waterlogged" && v == "true");
                    let next = format!(
                        "create:powered_shaft[axis={},waterlogged={water}]",
                        format!("{axis:?}").to_lowercase()
                    );
                    self.set(shaft, self.id(&next));
                }
                return;
            }
            // transmission.rs Clicked: casing, then the shaft placement helper.
            // PoweredShaftBlock.useItemOn has only the shaft's placement helper.
            if let Some((block, info)) = own
                && matches!(
                    block,
                    Block::Shaft | Block::PoweredShaft | Block::Cogwheel | Block::LargeCogwheel
                )
            {
                let axis = info.axis.expect("relay axis");
                if block != Block::PoweredShaft
                    && let Some(casing) = match item {
                        "create:brass_casing" => Some("brass"),
                        "create:andesite_casing" => Some("andesite"),
                        _ => None,
                    }
                {
                    let mut next = format!(
                        "create:{casing}_encased_{}[axis={}]",
                        block.name(),
                        axis_name(axis)
                    );
                    if block != Block::Shaft {
                        for (positive, key) in [(true, "top_shaft"), (false, "bottom_shaft")] {
                            let d = Dir::from_axis(axis, positive);
                            let connected =
                                self.w
                                    .has_shaft_towards(0, add(p, d.offset()), d.opposite());
                            next = with_prop(&next, key, if connected { "true" } else { "false" });
                        }
                    }
                    self.switch(p, self.id(&next));
                    return;
                }
                // ShaftBlock.useItemOn: a metal girder around a horizontal shaft.
                if block == Block::Shaft && item == "create:metal_girder" && axis != Axis::Y {
                    let mut next = format!(
                        "create:metal_girder_encased_shaft[axis={},waterlogged={}]",
                        axis_name(axis),
                        info.waterlogged
                    );
                    for (k, v) in crate::defaults::declared("create:metal_girder_encased_shaft") {
                        if !parse_state(&next).1.iter().any(|(key, _)| *key == k) {
                            next = with_prop(&next, &k, &v);
                        }
                    }
                    self.switch(p, self.id(&next));
                    return;
                }
                if matches!(block, Block::Shaft | Block::PoweredShaft)
                    && (item == "create:shaft" || cog_item(item).is_some())
                {
                    let at = place::pole_helper(
                        p,
                        axis,
                        hit.map(|v| v - 0.5),
                        |q| {
                            self.w.table.get(self.at(q)).is_some_and(|(b, i)| {
                                matches!(b, Block::Shaft | Block::PoweredShaft)
                                    && i.axis == Some(axis)
                            })
                        },
                        |q| self.at(q) == AIR,
                    );
                    if let Some(q) = at {
                        if let Some(large) = cog_item(item) {
                            self.place_cog(q, item, large, axis);
                        } else {
                            let next =
                                format!("create:shaft[axis={},waterlogged=false]", axis_name(axis));
                            self.set(q, self.id(&next));
                        }
                        return;
                    }
                }
            }
        }
        // Item use_on: bracket, wrench (including subsystem casing removal), BlockItem.
        if item == "create:metal_bracket" || item == "create:wooden_bracket" {
            if let Some((block, info)) = own
                && matches!(block, Block::Shaft | Block::Cogwheel | Block::LargeCogwheel)
            {
                let kind = if block == Block::Shaft {
                    create_engine::decor::BracketKind::Shaft
                } else {
                    create_engine::decor::BracketKind::Cog
                };
                let toward = place::ordered_by_nearest(yaw, pitch)[0].opposite();
                if let Some(b) = create_engine::decor::bracket_for_click(
                    item == "create:wooden_bracket",
                    info.axis,
                    kind,
                    face,
                    toward,
                ) {
                    self.brackets.insert(p, b);
                    self.mark_column(p);
                }
            } else if own.is_some_and(|(block, _)| block == Block::FluidPipe) {
                self.notes.push(format!(
                    "adapter gap: bracket target outside relay subset at {p:?}"
                ));
            }
            // glue/decor/brackets.rs use_bracket: a block without bracket support takes
            // nothing (BracketBlockItem.useOn returns FAIL without a bracket behaviour).
            return;
        }
        if item == "create:wrench" {
            let (name, props) = parse_state(&self.name(state));
            if name == "minecraft:air" || name == "minecraft:stone" {
                return;
            }
            if let Some((block, info)) = own
                && matches!(
                    block,
                    Block::AndesiteEncasedCogwheel
                        | Block::BrassEncasedCogwheel
                        | Block::AndesiteEncasedLargeCogwheel
                        | Block::BrassEncasedLargeCogwheel
                )
            {
                let axis = info.axis.expect("encased cog axis");
                let large = name.ends_with("_large_cogwheel");
                let next = if sneaking {
                    let bare = if large { "large_cogwheel" } else { "cogwheel" };
                    format!("create:{bare}[axis={},waterlogged=false]", axis_name(axis))
                } else if face.axis() == axis {
                    let (key, on) = if face == Dir::from_axis(axis, true) {
                        ("top_shaft", info.top_shaft)
                    } else {
                        ("bottom_shaft", info.bottom_shaft)
                    };
                    with_prop(&self.name(state), key, if on { "false" } else { "true" })
                } else {
                    let turned =
                        create_engine::wrench::clockwise(Dir::from_axis(axis, true), face.axis());
                    let swap = matches!(
                        (face.axis(), axis),
                        (Axis::X, Axis::Y) | (Axis::Y, Axis::Z) | (Axis::Z, Axis::X)
                    );
                    let (top, bottom) = if swap {
                        (info.bottom_shaft, info.top_shaft)
                    } else {
                        (info.top_shaft, info.bottom_shaft)
                    };
                    if !self.w.valid_cog_position(0, p, large, turned.axis()) {
                        return;
                    }
                    format!(
                        "{name}[axis={},top_shaft={top},bottom_shaft={bottom}]",
                        axis_name(turned.axis())
                    )
                };
                self.switch(p, self.id(&next));
                return;
            }
            // glue/transmission.rs wrench_used: a drive in a chain turns about the chain's
            // axis (ChainDriveBlock.getRotatedBlockState); its neighbours reshape it after.
            // A drive alone takes the ordinary axis turn below.
            if !sneaking
                && let Some((Block::EncasedChainDrive | Block::AdjustableChainGearshift, info)) =
                    own
                && info.chain.unwrap_or_default() != create_engine::chain::Part::None
            {
                let axis = info.axis.expect("chain drive axis");
                let about = create_engine::chain::connection_axis(axis, info.along_first);
                let turned = create_engine::wrench::clockwise(Dir::from_axis(axis, true), about);
                if turned.axis() != axis {
                    let next = with_prop(&self.name(state), "axis", axis_name(turned.axis()));
                    let id = self.id(&next);
                    self.switch(p, id);
                    self.w.neighbor_changed(0, p, id);
                }
                return;
            }
            if !sneaking
                && let Some((Block::SequencedGearshift, info)) = own
                && face.axis() != Axis::Y
            {
                if info.axis != Some(face.axis()) {
                    let next = with_prop(
                        &self.name(state),
                        "vertical",
                        if info.vertical { "false" } else { "true" },
                    );
                    self.switch(p, self.id(&next));
                }
                return;
            }
            if sneaking {
                if name == "create:andesite_encased_shaft" || name == "create:brass_encased_shaft" {
                    let axis = props.iter().find(|(k, _)| k == "axis").unwrap().1.clone();
                    self.switch(
                        p,
                        self.id(&format!("create:shaft[axis={axis},waterlogged=false]")),
                    );
                } else {
                    // `WaterWheelStructuralBlock.onSneakWrenched`: a frame cell hands the
                    // wrench to the wheel it holds.
                    let target = self.w.sneak_wrenched(0, p, state);
                    self.set(target, AIR);
                }
                return;
            }
            // `IWrenchable.onWrenched` answering PASS (the port's `wrench::turns`): the large
            // water wheel and its frame cells are neither turned nor taken.
            if !create_engine::wrench::turns(name.trim_start_matches("create:")) {
                return;
            }
            if !crate::actions::BLOCKS.contains(&name.as_str())
                && !name.ends_with("_encased_shaft")
                && !name.ends_with("_casing")
            {
                self.notes.push(format!(
                    "adapter gap: unsupported wrench override at {p:?}: {name}"
                ));
                return;
            }
            let rotated = create_engine::wrench::rotated(&props.into_iter().collect(), false, face);
            let next =
                crate::scenario::format_state(&name, &rotated.into_iter().collect::<Vec<_>>());
            // `IWrenchable.onWrenched` keeps the old state when the turned one cannot stand.
            if name == "create:steam_engine" && !self.steam_engine_survives(p, &next) {
                return;
            }
            if name == "create:water_wheel" && !self.water_wheel_survives(p, &next) {
                return;
            }
            if name == "create:powered_shaft" && !self.powered_shaft_survives(p, &next) {
                return;
            }
            self.switch(p, self.id(&next));
            // `updateAfterWrenched`: `Block.updateFromNeighbourShapes` runs the block's own
            // shape update, which is how a wheel looks at its flow again.
            self.w.neighbor_changed(0, p, self.at(p));
            return;
        }
        if item == "minecraft:air"
            || create_engine::dye::DyeColor::of_item(item).is_some()
            || (!crate::actions::BLOCKS.contains(&item)
                && !["create:andesite_casing", "create:brass_casing"].contains(&item))
        {
            return;
        }
        // No override of the clicked position: air replaces itself; solid clicks place adjacent.
        let p = if self.replaceable(p) {
            p
        } else {
            add(p, face.offset())
        };
        if !self.replaceable(p) {
            return;
        }
        self.place_player(p, item, yaw, pitch, face, sneaking);
        if item == "create:shaft" {
            // `ShaftBlock.pickCorrectShaftType`: a shaft placed in front of an engine.
            self.apply_shaft_changes();
        }
    }

    fn place_cog(&mut self, p: Pos, item: &str, large: bool, axis: Axis) {
        if self.w.valid_cog_position(0, p, large, axis) {
            let state = format!("{item}[axis={},waterlogged=false]", axis_name(axis));
            self.set(p, self.id(&state));
        }
    }

    pub(super) fn switch(&mut self, p: Pos, id: u32) {
        if self.at(p) == id {
            return;
        }
        let inherited = self.measured_trig.get(&p).cloned().filter(|property| {
            let old = parse_state(&self.name(self.at(p))).1;
            let new = parse_state(&self.name(id)).1;
            old.iter().find(|(k, _)| k == property) == new.iter().find(|(k, _)| k == property)
                && new.iter().any(|(k, _)| k == property)
        });
        self.w.switching_block_state(0, p, id);
        self.set(p, id);
        if let Some(property) = inherited {
            self.measured_trig.insert(p, property);
        }
    }
    pub(super) fn place_player(
        &mut self,
        p: Pos,
        block: &str,
        yaw: f32,
        pitch: f32,
        face: create_engine::kinetics::Dir,
        sneaking: bool,
    ) {
        if self.place_pipe(p, block, yaw, pitch, sneaking) {
            return;
        }
        if block == "create:water_wheel" {
            self.place_water_wheel(p, yaw, pitch, sneaking, face);
            return;
        }
        if block == "create:large_water_wheel" {
            self.place_large_wheel(p, yaw, pitch, sneaking, face);
            return;
        }
        if block == "create:steam_engine" {
            self.place_steam_engine(p, yaw, face);
            return;
        }
        if block == "create:fluid_tank" {
            self.set(
                p,
                self.id("create:fluid_tank[bottom=true,shape=window,top=true]"),
            );
            // TankItem.on_block_placed_by publishes the accepted placement.
            self.publish_interaction();
            return;
        }
        if [
            "create:brass_casing",
            "create:andesite_casing",
            "create:copper_casing",
        ]
        .contains(&block)
        {
            self.set(p, self.id(block));
            return;
        }
        if block == "create:vertical_gearbox" {
            let shafts: Vec<_> = [Dir::South, Dir::West, Dir::North, Dir::East]
                .into_iter()
                .filter(|d| {
                    self.w
                        .has_shaft_towards(0, add(p, d.offset()), d.opposite())
                })
                .collect();
            let axis = create_engine::decor::vertical_gearbox_axis(
                &shafts,
                place::horizontal_direction(yaw),
            );
            self.set(
                p,
                self.id(&format!("create:gearbox[axis={}]", axis_name(axis))),
            );
            return;
        }
        let b = create_engine::Block::ALL
            .iter()
            .copied()
            .find(|b| format!("create:{}", b.name()) == block)
            .expect("block");
        let ctx = create_engine::place::Context {
            yaw,
            pitch,
            sneaking,
            clicked_face: face,
        };
        let shaft = |d: create_engine::kinetics::Dir| {
            self.w
                .has_shaft_towards(0, add(p, d.offset()), d.opposite())
        };
        if matches!(b, Block::Speedometer | Block::Stressometer) {
            let (facing, along) = create_engine::gauge::placement(
                face,
                place::ordered_by_nearest(yaw, pitch)[0],
                place::horizontal_direction(yaw),
                self.w
                    .has_shaft_towards(0, add(p, face.opposite().offset()), face),
                shaft,
            );
            let state =
                format!("{block}[facing={facing:?},axis_along_first={along}]").to_lowercase();
            self.set(p, self.id(&state));
            return;
        }
        let near = place::Nearby {
            large_cog_above: self
                .w
                .cog_at(0, add(p, [0, 1, 0]))
                .filter(|(large, _)| *large)
                .map(|(_, a)| a),
            small_cog_clicked: self
                .w
                .cog_at(0, add(p, face.opposite().offset()))
                .and_then(|(large, axis)| (!large).then_some(axis)),
            controller_below: self
                .w
                .table
                .get(self.at(add(p, [0, -1, 0])))
                .filter(|(block, _)| *block == Block::RotationSpeedController)
                .and_then(|(_, info)| info.axis),
        };
        let mut next = if let Some(f) = create_engine::place::facing(b, ctx, shaft) {
            format!("{block}[facing={}]", format!("{f:?}").to_lowercase())
        } else {
            let (a, vertical) = place::axis(b, ctx, shaft, near).expect("placement axis");
            if let Some(large) = cog_item(block) {
                self.place_cog(p, block, large, a);
                return;
            }
            let axis = axis_name(a);
            if b == Block::SequencedGearshift {
                format!("{block}[axis={axis},state=0,vertical={vertical}]")
            } else {
                format!("{block}[axis={axis}]")
            }
        };
        // Start from the port's declared default for properties placement does not select.
        let set: Vec<String> = parse_state(&next).1.into_iter().map(|(k, _)| k).collect();
        for (k, v) in crate::defaults::declared(block) {
            if !set.contains(&k) {
                next = with_prop(&next, &k, &v);
            }
        }
        if matches!(b, Block::Shaft | Block::HandCrank) {
            next = with_prop(&next, "waterlogged", "false");
        }
        if block == "create:clutch"
            || block == "create:gearshift"
            || block == "create:adjustable_chain_gearshift"
        {
            next = with_prop(
                &next,
                "powered",
                if self.signal(p) > 0 { "true" } else { "false" },
            );
        }
        let measured = crate::trig::exact_tie(yaw, pitch)
            .then(|| crate::trig::nearest_property(b, ctx, shaft, near))
            .flatten();
        if self.java_ties
            && let Some(property) = measured
        {
            next = crate::trig::java_choice(&next, property);
        }
        if b == Block::HandCrank {
            let facing = place::facing(b, ctx, shaft).expect("crank facing");
            if self.replaceable(add(p, facing.opposite().offset())) {
                return;
            }
        }
        self.set(p, self.id(&next));
        if let Some(property) = measured {
            self.measured_trig.insert(p, property.into());
        }
    }
}
