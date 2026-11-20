//! Generating kinetic sources' environment: water wheels' flow and the boilers that drive
//! steam engines. The native fluid host supplies the fluid states and their flow; boilers read
//! the heat under their tank from the world's block states.
use super::*;
use create_engine::kinetics::{Axis, Dir};

impl Run {
    /// The cells every existing wheel samples for its flow, sorted and deduplicated, so the
    /// native host answers for them in the same frame as this tick's fluid tick.
    pub(super) fn wheel_samples(&self) -> Vec<Pos> {
        let mut out = Vec::new();
        for (&p, &id) in &self.world {
            let Some((block, info)) = self.w.table.get(id) else {
                continue;
            };
            let (axis, large) = match block {
                create_engine::Block::WaterWheel => match info.facing {
                    Some(f) => (f.axis(), false),
                    None => continue,
                },
                create_engine::Block::LargeWaterWheel => match info.axis {
                    Some(a) => (a, true),
                    None => continue,
                },
                _ => continue,
            };
            for o in offsets(axis, large) {
                out.push(add(p, o));
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// `WaterWheelBlockEntity.determineAndApplyFlowScore` when a wheel's surroundings changed or
    /// its lazy tick came.
    pub(super) fn apply_wheel_flow(&mut self) {
        for (p, _, _) in self.w.take_wheel_checks(0, self.persisted.clock as u64) {
            self.apply_flow(p);
        }
    }

    /// One wheel's flow score, from its axis and the flow at its sample cells.
    fn apply_flow(&mut self, p: Pos) {
        let Some((block, info)) = self.w.table.get(self.at(p)) else {
            return;
        };
        let (axis, large) = match block {
            create_engine::Block::WaterWheel => match info.facing {
                Some(f) => (f.axis(), false),
                None => return,
            },
            create_engine::Block::LargeWaterWheel => match info.axis {
                Some(a) => (a, true),
                None => return,
            },
            _ => return,
        };
        let samples: Vec<([i32; 3], [f64; 3])> = offsets(axis, large)
            .into_iter()
            .map(|o| (o, self.flow_at(add(p, o))))
            .collect();
        let score = create_engine::waterwheel::flow_score(axis, &samples);
        self.w.set_wheel_flow(0, p, score);
        self.wheel_scores.insert(p, score.clamp(-1, 1) as i8);
    }

    /// `WaterWheelBlockEntity.getFlowVectorAtPosition`: a bubble column reads its drag, anything
    /// else the native flow the host reported for this tick.
    fn flow_at(&self, p: Pos) -> [f64; 3] {
        let name = self.name(self.at(p));
        if name.starts_with("minecraft:bubble_column") {
            let down = name.contains("drag=true");
            return [0.0, if down { -1.0 } else { 1.0 }, 0.0];
        }
        if let Some(flow) = self.wheel_fluid_flow.get(&p) {
            return *flow;
        }
        // No native peer: every cell the scenario placed is air or a non-fluid host state.
        [0.0; 3]
    }

    /// `LargeWaterWheelBlock.tick`: every structural cell around the wheel, and the flow after.
    /// A cell that cannot be replaced takes the wheel with it (`destroyBlock(pos, false)`).
    pub(super) fn tick_large_wheels(&mut self) {
        let checks = std::mem::take(&mut self.w.dims[0].large_wheel_checks);
        for p in checks {
            let Some((create_engine::Block::LargeWaterWheel, info)) = self.w.table.get(self.at(p))
            else {
                continue;
            };
            let Some(axis) = info.axis else { continue };
            let mut destroyed = false;
            for (o, facing) in create_engine::waterwheel::structure(axis) {
                let q = add(p, o);
                let required = structure_state(facing);
                if self.name(self.at(q)) == required {
                    continue;
                }
                if !self.replaceable(q) {
                    self.set(p, AIR);
                    destroyed = true;
                    break;
                }
                self.set(q, self.id(&required));
            }
            if !destroyed {
                self.apply_flow(p);
            }
        }
    }

    /// `WaterWheelStructuralBlock.tick`: a cell whose scheduled tick came and whose frame no
    /// longer holds it goes. The shape update decided that a tick earlier (`updateShape`; the
    /// engine keeps the cells a frame holds again).
    pub(super) fn tick_structures(&mut self) {
        for p in self.w.take_frame_ticks(0) {
            self.set(p, AIR);
        }
    }

    /// `WaterWheelStructuralBlock.onRemove`: a cell removed while its frame holds removes the
    /// wheel too (`destroyBlock(master, true)`); the engine recorded the wheels it broke.
    pub(super) fn break_wheels(&mut self) {
        for master in self.w.take_broken_wheels(0) {
            self.set(master, AIR);
        }
    }

    /// `BlazeBurnerBlock.useItemOn` on a lit burner: a creative blaze cake sets the next heat,
    /// a blaze cake burns special fuel for 3200 ticks (`BoilerHeaters.blazeBurner` reads the
    /// resulting `blaze` property). Other fuels are outside this adapter.
    pub(super) fn use_burner(&mut self, p: Pos, item: &str) -> bool {
        use create_engine::burner::{self, Fuel};
        let Some((block, info)) = self.w.table.get(self.at(p)) else {
            return false;
        };
        if block != create_engine::Block::BlazeBurner
            || info
                .heat
                .is_none_or(|h| h == create_engine::fan::Heat::None)
            || !self.w.dims[0].burners.contains_key(&p)
        {
            return false;
        }
        let heat = if item == "create:creative_blaze_cake" {
            let shown = info.heat.unwrap_or(create_engine::fan::Heat::Smouldering);
            let Some(next) = self.w.dims[0]
                .burners
                .get_mut(&p)
                .map(|b| b.creative_fuel(shown))
            else {
                return false;
            };
            next
        } else if item == "create:blaze_cake" {
            if !self.w.fuel_burner(0, p, Fuel::Special, 3200, true) {
                return false;
            }
            self.w.dims[0].burners[&p].heat()
        } else {
            return false;
        };
        let next = with_prop(&self.name(self.at(p)), "blaze", burner::heat_name(heat));
        self.set(p, self.id(&next));
        self.mark_column(p);
        true
    }

    /// `BlazeBurnerBlockEntity.tick` on the server: each burning burner's heat sets its block.
    pub(super) fn tick_burners(&mut self) {
        for (p, heat) in self.w.tick_burners(0) {
            let next = with_prop(
                &self.name(self.at(p)),
                "blaze",
                create_engine::burner::heat_name(heat),
            );
            self.set(p, self.id(&next));
        }
    }

    /// The glue's `tick_boilers`: engines counted, shafts in front of them powered or plain, heat
    /// read under the boiler, water supply sampled.
    pub(super) fn tick_boilers(&mut self) {
        self.w.evaluate_boilers(0);
        self.apply_shaft_changes();
        for (tank, width) in self.w.take_heat_checks(0) {
            // `updateTemperature`: every block under the tank's bottom layer.
            let w = i32::from(width);
            let heats: Vec<i32> = (0..w)
                .flat_map(|x| (0..w).map(move |z| [tank[0] + x, tank[1] - 1, tank[2] + z]))
                .map(|p| self.boiler_heater(p))
                .collect();
            self.w.set_boiler_heat(0, tank, &heats);
        }
        self.w.tick_boilers(0);
    }

    /// Shafts in front of engines that changed: a powered shaft (`SteamEngineBlock.onPlace`,
    /// `PoweredShaftBlock.getEquivalent`) or a plain one again (`PoweredShaftBlock.tick`).
    pub(super) fn apply_shaft_changes(&mut self) {
        for (p, powered) in self.w.shaft_changes(0) {
            let (_, props) = parse_state(&self.name(self.at(p)));
            let get = |k: &str| {
                props
                    .iter()
                    .find(|(key, _)| key == k)
                    .map(|(_, v)| v.as_str())
            };
            let shaft = format!(
                "create:{}[axis={},waterlogged={}]",
                if powered { "powered_shaft" } else { "shaft" },
                get("axis").unwrap_or("x"),
                get("waterlogged").unwrap_or("false"),
            );
            self.set(p, self.id(&shaft));
        }
    }

    /// `BoilerHeater.findHeat` under a boiler: a blaze burner by its heat level, another block in
    /// `create:passive_boiler_heaters` passive unless it is out (an unlit campfire), anything else
    /// none. Lava and fire are not representable as scenario host states.
    fn boiler_heater(&self, p: Pos) -> i32 {
        use create_engine::fan::Heat;
        use create_engine::steam::{NO_HEAT, PASSIVE_HEAT, blaze_burner_heat};
        let (name, props) = parse_state(&self.name(self.at(p)));
        let get = |k: &str| {
            props
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        match name.as_str() {
            "create:blaze_burner" => {
                let heat = match get("blaze") {
                    Some("smouldering") => Heat::Smouldering,
                    Some("fading") => Heat::Fading,
                    Some("kindled") => Heat::Kindled,
                    Some("seething") => Heat::Seething,
                    _ => Heat::None,
                };
                blaze_burner_heat(heat)
            }
            "create:lit_blaze_burner" | "minecraft:magma_block" => PASSIVE_HEAT,
            "minecraft:campfire" | "minecraft:soul_campfire" => {
                if get("lit") == Some("true") {
                    PASSIVE_HEAT
                } else {
                    NO_HEAT
                }
            }
            _ => NO_HEAT,
        }
    }

    /// `WaterWheelBlock.getStateForPlacement` (the positive facing of the chosen axis) and
    /// `canSurvive` (no adjacent wheel on another axis or off that axis).
    pub(super) fn place_water_wheel(
        &mut self,
        p: Pos,
        yaw: f32,
        pitch: f32,
        sneaking: bool,
        face: Dir,
    ) -> bool {
        use create_engine::{Block, place};
        let ctx = place::Context {
            yaw,
            pitch,
            sneaking,
            clicked_face: face,
        };
        let shaft = |d: Dir| {
            self.w
                .has_shaft_towards(0, add(p, d.offset()), d.opposite())
        };
        let Some(chosen) = place::facing(Block::WaterWheel, ctx, shaft) else {
            return false;
        };
        let axis = chosen.axis();
        for d in Dir::ALL {
            let q = add(p, d.offset());
            let Some((Block::WaterWheel, info)) = self.w.table.get(self.at(q)) else {
                continue;
            };
            let Some(other) = info.facing else { continue };
            if other.axis() != axis || axis != d.axis() {
                return false;
            }
        }
        // `getStateForPlacement`'s `setValue` discards its result: the facing stays as chosen.
        self.set(
            p,
            self.id(&format!(
                "create:water_wheel[facing={}]",
                format!("{chosen:?}").to_lowercase()
            )),
        );
        true
    }

    /// `LargeWaterWheelBlock.getStateForPlacement` (the wheel's room in its plane and its
    /// `extension`) and `LargeWaterWheelBlockItem.place`'s retry at the clicked face's
    /// neighbour when the first position has no room.
    pub(super) fn place_large_wheel(
        &mut self,
        p: Pos,
        yaw: f32,
        pitch: f32,
        sneaking: bool,
        face: Dir,
    ) -> bool {
        let axis = self.large_wheel_axis(p, yaw, pitch, sneaking, face);
        if self.large_wheel_room(p, axis) {
            let ext = self.large_wheel_extension(p, axis);
            self.set(p, self.id(&large_wheel_state(axis, ext)));
            return true;
        }
        if face.axis() == axis {
            return false;
        }
        let q = add(p, face.offset());
        if !self.replaceable(q) {
            return false;
        }
        let axis = self.large_wheel_axis(q, yaw, pitch, sneaking, face);
        if !self.large_wheel_room(q, axis) {
            return false;
        }
        let ext = self.large_wheel_extension(q, axis);
        self.set(q, self.id(&large_wheel_state(axis, ext)));
        true
    }

    fn large_wheel_axis(&self, p: Pos, yaw: f32, pitch: f32, sneaking: bool, face: Dir) -> Axis {
        use create_engine::{Block, place};
        let ctx = place::Context {
            yaw,
            pitch,
            sneaking,
            clicked_face: face,
        };
        let shaft = |d: Dir| {
            self.w
                .has_shaft_towards(0, add(p, d.offset()), d.opposite())
        };
        place::axis(
            Block::LargeWaterWheel,
            ctx,
            shaft,
            place::Nearby {
                large_cog_above: self
                    .w
                    .cog_at(0, add(p, [0, 1, 0]))
                    .filter(|(large, _)| *large)
                    .map(|(_, axis)| axis),
                ..Default::default()
            },
        )
        .expect("large wheel placement axis")
        .0
    }

    /// The wheel's plane must be replaceable (`getStateForPlacement`'s 3x3 check).
    fn large_wheel_room(&self, p: Pos, axis: Axis) -> bool {
        create_engine::waterwheel::plane(axis)
            .into_iter()
            .all(|o| self.replaceable(add(p, o)))
    }

    /// `extension`: the wheel extends one behind it on its negative axis.
    fn large_wheel_extension(&self, p: Pos, axis: Axis) -> bool {
        let negative = match axis {
            Axis::X => Dir::West,
            Axis::Y => Dir::Down,
            Axis::Z => Dir::North,
        };
        self.w.block_at(0, add(p, negative.offset())) == Some(create_engine::Block::LargeWaterWheel)
    }

    /// `SteamEngineBlock.getStateForPlacement` on the clicked face (`FACE` and `FACING`), its
    /// waterlogging, `canSurvive` (a fluid tank behind it) and `onPlace` (a shaft in front
    /// becomes a powered shaft).
    pub(super) fn place_steam_engine(&mut self, p: Pos, yaw: f32, face: Dir) -> bool {
        use create_engine::steam::{self, Face};
        use create_engine::{Block, place};
        let (attach, facing) = match face {
            Dir::Down => (Face::Ceiling, place::horizontal_direction(yaw)),
            Dir::Up => (Face::Floor, place::horizontal_direction(yaw)),
            _ => (Face::Wall, face),
        };
        let connected = steam::connected_direction(attach, facing);
        if self.w.block_at(0, add(p, connected.opposite().offset())) != Some(Block::FluidTank) {
            return false;
        }
        let waterlogged = self
            .world_fluid(p)
            .is_some_and(|(fluid, _)| fluid == create_engine::tank::Fluid::Water);
        let state = format!(
            "create:steam_engine[face={},facing={},waterlogged={}]",
            attach.name(),
            format!("{facing:?}").to_lowercase(),
            waterlogged,
        );
        self.set(p, self.id(&state));
        // `SteamEngineBlock.onPlace` converts the shaft at once.
        self.apply_shaft_changes();
        true
    }

    /// `WaterWheelBlock.canSurvive` for a state at `p`: no adjacent wheel on another axis, nor off
    /// the wheel's own axis.
    pub(super) fn water_wheel_survives(&self, p: Pos, state: &str) -> bool {
        use create_engine::{Block, kinetics::Axis};
        let (_, props) = parse_state(state);
        let Some(axis) = props
            .iter()
            .find(|(k, _)| k == "facing")
            .map(|(_, v)| match v.as_str() {
                "east" | "west" => Axis::X,
                "up" | "down" => Axis::Y,
                _ => Axis::Z,
            })
        else {
            return false;
        };
        for d in Dir::ALL {
            let Some((Block::WaterWheel, other)) = self.w.table.get(self.at(add(p, d.offset())))
            else {
                continue;
            };
            let Some(other) = other.facing else { continue };
            if other.axis() != axis || axis != d.axis() {
                return false;
            }
        }
        true
    }

    /// `PoweredShaftBlock.canSurvive` for a turned state at `p`: an engine two blocks away across
    /// its axis points at it.
    pub(super) fn powered_shaft_survives(&self, p: Pos, state: &str) -> bool {
        use create_engine::kinetics::Axis;
        let (_, props) = parse_state(state);
        let Some(axis) =
            props
                .iter()
                .find(|(k, _)| k == "axis")
                .and_then(|(_, v)| match v.as_str() {
                    "x" => Some(Axis::X),
                    "y" => Some(Axis::Y),
                    "z" => Some(Axis::Z),
                    _ => None,
                })
        else {
            return false;
        };
        create_engine::steam::still_valid(p, axis, |q| self.w.engine_at(0, q))
    }

    /// `SteamEngineBlock.canSurvive` for the turned state at `p`: a fluid tank behind the face
    /// it attaches by.
    pub(super) fn steam_engine_survives(&self, p: Pos, state: &str) -> bool {
        use create_engine::steam::{self, Face};
        let (_, props) = parse_state(state);
        let get = |k: &str| {
            props
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        let Some(face) = get("face").and_then(|v| match v {
            "wall" => Some(Face::Wall),
            "ceiling" => Some(Face::Ceiling),
            _ => None,
        }) else {
            return false;
        };
        let Some(facing) = get("facing").and_then(|v| match v {
            "north" => Some(Dir::North),
            "south" => Some(Dir::South),
            "west" => Some(Dir::West),
            "east" => Some(Dir::East),
            "up" => Some(Dir::Up),
            "down" => Some(Dir::Down),
            _ => None,
        }) else {
            return false;
        };
        let connected = steam::connected_direction(face, facing);
        self.w.block_at(0, add(p, connected.opposite().offset()))
            == Some(create_engine::Block::FluidTank)
    }
}

/// `LargeWaterWheelBlock`'s state with its axis and `extension`.
fn large_wheel_state(axis: Axis, extension: bool) -> String {
    format!(
        "create:large_water_wheel[axis={},extension={}]",
        format!("{axis:?}").to_lowercase(),
        extension
    )
}

/// The state of a frame cell facing `facing` (`WaterWheelStructuralBlock` with FACING).
fn structure_state(facing: Dir) -> String {
    format!(
        "create:water_wheel_structure[facing={}]",
        format!("{facing:?}").to_lowercase()
    )
}

/// The offsets a wheel of this axis reads (`WaterWheelBlockEntity.getOffsetsToCheck`).
fn offsets(axis: Axis, large: bool) -> Vec<[i32; 3]> {
    if large {
        create_engine::waterwheel::large_offsets(axis)
    } else {
        create_engine::waterwheel::small_offsets(axis).to_vec()
    }
}
