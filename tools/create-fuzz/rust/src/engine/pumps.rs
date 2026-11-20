//! Engine transport host: world reads and capability callbacks only.
use super::*;
use create_engine::fluids::{
    runtime::{Face, Host},
    transport::Source,
};
use create_engine::{
    Block,
    fluids::{self, Cell},
    kinetics::Dir,
    tank::Fluid,
};
impl Run {
    pub(super) fn pipe_cell_toward(&self, p: Pos, toward: Dir) -> Cell {
        if self.w.table.get(self.at(p)).is_some() {
            return self.pipe_cell(p);
        }
        let state = self.name(self.at(p));
        let (name, props) = parse_state(&state);
        if fluids::vanilla_target(&name, props.iter().any(|(k, _)| k == "honey_level")) {
            return Cell::VanillaTarget;
        }
        if self.sturdy(p, toward.opposite()) {
            return Cell::Solid;
        }
        if self.replaceable(p) || self.waterloggable(p) {
            Cell::Open
        } else {
            Cell::Solid
        }
    }
    pub(super) fn tick_pumps(&mut self) {
        self.tick_pipe_shapes();
        let positions = self.w.fluid_transport_positions(0);
        let mut transport = std::mem::take(&mut self.w.dims[0].fluid_transport);
        for &p in &positions {
            transport.register(p, self.pipe_cell(p), self.w.speed(0, p).unwrap_or(0.));
            self.mark_column(p);
        }
        transport.prepare(self);
        self.put_transport(transport);
    }
    pub(super) fn tick_transport_pipe(&mut self, pos: Pos) {
        if !self.w.dims[0].fluid_transport.pipes.contains_key(&pos) {
            return;
        }
        self.w.fluid_transport_base_tick(0, pos);
        let mut transport = std::mem::take(&mut self.w.dims[0].fluid_transport);
        if let Some((a, b)) = transport.tick_pipe(self, pos) {
            self.set(pos, AIR);
            transport.remove(pos);
            if let Some(name) = create_engine::open_end::flow_collision(a, b) {
                self.set(pos, self.id(name));
            }
        }
        self.put_transport(transport);
        self.mark_column(pos);
    }
    fn put_transport(&mut self, mut transport: create_engine::fluids::runtime::Transport) {
        transport
            .changed
            .append(&mut self.w.dims[0].fluid_transport.changed);
        self.w.dims[0].fluid_transport = transport;
    }
    pub(super) fn fluid_offer(&mut self, from: Pos, amount: u32) -> Option<(Fluid, u32)> {
        if let Some(fluid) = self.w.dims[0].plumbing.creative_fluid(from) {
            return fluid.map(|f| (f, amount));
        }
        if self.w.is_boiler(0, from) {
            return None;
        }
        if let Some(d) = self.w.dims[0].drains.get(&from) {
            return d.fluid.map(|(f, n)| (f, n.min(amount)));
        }
        self.w
            .tank(0, from)
            .and_then(|(_, m)| m.tank.fluid.map(|f| (f, m.tank.amount.min(amount))))
    }
    pub(super) fn fluid_take(&mut self, from: Pos, _fluid: Fluid, amount: u32) -> u32 {
        if self.w.dims[0].plumbing.creative_fluid(from).is_some() {
            return amount;
        }
        if let Some(d) = self.w.dims[0].drains.get_mut(&from) {
            return create_engine::drain::drain(d, amount).map_or(0, |(_, n)| n);
        }
        self.w.drain_tank(0, from, amount).map_or(0, |(_, n)| n)
    }
    pub(super) fn fluid_fill(&mut self, to: Pos, fluid: Fluid, amount: u32) -> u32 {
        if self.w.dims[0].plumbing.creative.contains_key(&to) {
            return amount;
        }
        if let Some(took) = self.w.fill_boiler(0, to, fluid, amount) {
            return took;
        }
        if let Some(s) = self.w.dims[0].spouts.get_mut(&to) {
            return create_engine::spout::fill(s, fluid, amount);
        }
        self.w.fill_tank(0, to, fluid, amount)
    }
    pub(super) fn fluid_network(&self, p: Pos) -> Option<crate::fluid_network::Observation> {
        use crate::fluid_network::{Buffer, Connection, Flight, Observation};
        let block = self.w.block_at(0, p)?;
        if !matches!(
            block,
            Block::FluidPipe
                | Block::EncasedFluidPipe
                | Block::GlassFluidPipe
                | Block::SmartFluidPipe
                | Block::FluidValve
                | Block::MechanicalPump
        ) {
            return None;
        }
        let world = &self.w.dims[0].fluid_transport;
        let connections = Dir::ALL
            .into_iter()
            .map(|side| {
                let c = world.connection(Face { pos: p, side });
                let mut buffer = BTreeMap::new();
                if let Some(c) = c {
                    for end in c.open_ends() {
                        if let Some((fluid, n)) = end.tank {
                            *buffer.entry(fluid.key()).or_insert(0) += n;
                        }
                    }
                }
                (
                    format!("{side:?}").to_lowercase(),
                    Connection {
                        pressure: c.map_or([0.; 2], |c| c.pressure()),
                        flow: c.and_then(|c| c.flow).map(|f| Flight {
                            fluid: f.fluid.key(),
                            inbound: f.inbound,
                            progress: f.progress,
                            complete: f.complete,
                        }),
                        buffer: Buffer { fluids: buffer },
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let pump_pressure = (block == Block::MechanicalPump).then(|| {
            connections
                .values()
                .flat_map(|c| c.pressure)
                .fold(0., f32::max)
        });
        Some(Observation {
            pressure_model: "stored_connections".into(),
            connections,
            pump_pressure,
        })
    }
}
impl Host for Run {
    fn time(&self) -> i64 {
        self.persisted.clock as i64
    }
    fn cell(&self, p: Pos, d: Dir) -> Cell {
        self.pipe_cell_toward(p, d)
    }
    fn speed(&self, p: Pos) -> f32 {
        self.w.speed(0, p).unwrap_or(0.)
    }
    fn notifies_pipe_neighbours(&self, pos: Pos) -> bool {
        create_engine::fluids::notifies_pipe_neighbours(
            self.w.block_at(0, pos),
            Fluid::of_id(&parse_state(&self.name(self.at(pos))).0).is_some_and(|f| f.placeable()),
        )
    }
    fn tank_fluids(&self, face: Face) -> Vec<Fluid> {
        let p = face.connected();
        if self.w.is_boiler(0, p) {
            return Vec::new();
        }
        let fluid = if let Some(fluid) = self.w.dims[0].plumbing.creative_fluid(p) {
            fluid
        } else if let Some(d) = self.w.dims[0].drains.get(&p) {
            d.fluid.filter(|(_, n)| *n > 0).map(|(f, _)| f)
        } else if let Some(s) = self.w.dims[0].spouts.get(&p) {
            s.fluid.filter(|(_, n)| *n > 0).map(|(f, _)| f)
        } else {
            self.w
                .tank(0, p)
                .filter(|(_, m)| m.tank.amount > 0)
                .and_then(|(_, m)| m.tank.fluid)
        };
        fluid.into_iter().collect()
    }
    fn offer(
        &mut self,
        face: Face,
        source: &mut Source,
        n: u32,
        fluid: Option<Fluid>,
    ) -> Option<(Fluid, u32)> {
        match source {
            Source::Open(open) => open.drain(
                &mut super::fluid_world::Front {
                    run: self,
                    pos: face.connected(),
                },
                n,
                fluid,
                false,
            ),
            Source::Handler => self
                .fluid_offer(face.connected(), n)
                .filter(|(f, _)| fluid.is_none_or(|want| want == *f)),
            _ => None,
        }
    }
    fn drain(&mut self, face: Face, source: &mut Source, fluid: Fluid, n: u32) -> u32 {
        match source {
            Source::Open(open) => open
                .drain(
                    &mut super::fluid_world::Front {
                        run: self,
                        pos: face.connected(),
                    },
                    n,
                    Some(fluid),
                    true,
                )
                .map_or(0, |(_, n)| n),
            Source::Handler => self.fluid_take(face.connected(), fluid, n),
            _ => 0,
        }
    }
    fn fill(
        &mut self,
        face: Face,
        source: &mut Source,
        fluid: Fluid,
        n: u32,
        execute: bool,
    ) -> u32 {
        if let Source::Open(open) = source {
            return open.fill(
                &mut super::fluid_world::Front {
                    run: self,
                    pos: face.connected(),
                },
                fluid,
                n,
                execute,
            );
        }
        if execute {
            return self.fluid_fill(face.connected(), fluid, n);
        }
        let p = face.connected();
        if self.w.dims[0].plumbing.creative.contains_key(&p) {
            return n;
        }
        if let Some(took) = self.w.simulate_fill_boiler(0, p, fluid, n) {
            return took;
        }
        if let Some(s) = self.w.dims[0].spouts.get(&p) {
            return create_engine::spout::fill(&mut s.clone(), fluid, n);
        }
        self.w.tank(0, p).map_or(0, |(_, m)| {
            m.tank.clone().fill_amount(fluid, n, m.capacity())
        })
    }
    fn identity(&self, face: Face) -> Pos {
        self.w
            .tank(0, face.connected())
            .map_or(face.connected(), |(p, _)| p)
    }
    fn keep_alive(&mut self, face: Face) {
        if self.w.block_at(0, face.connected()) == Some(Block::PortableFluidInterface)
            && let Some(interface) = self.w.dims[0].psis.get_mut(&face.connected())
        {
            interface.on_content_transferred();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    // The native bucket integration is tested separately. Seed its observed vanilla
    // post-input state here so the fluid/glue proof remains independent of the host.
    pub(super) fn seed_native_cauldron_fill(sc: &mut crate::scenario::Scenario) {
        for event in &mut sc.events {
            if matches!(&event.action, Some(crate::actions::Action::Use { item, .. }) if item == "minecraft:water_bucket")
            {
                event.action = None;
                event.state = Some("minecraft:water_cauldron[level=3]".into());
            }
        }
    }
    #[test]
    fn a_placed_branch_splits_pressure_on_the_next_scheduled_turn() {
        // Java's 32 RPM pump: adding a second tank at turn 36 gives
        // both branches pressure 16 at turn 37, after synchronous updateShape.
        let placements = [
            (20, [1, 3, 0], "create:fluid_pipe"),
            (24, [2, 3, 0], "create:mechanical_pump"),
            (28, [3, 3, 0], "create:fluid_pipe"),
            (32, [4, 3, 0], "create:fluid_tank"),
            (36, [3, 3, 1], "create:fluid_tank"),
        ];
        let events: Vec<_> = placements
            .into_iter()
            .map(|(tick, pos, item)| {
                serde_json::json!({
                    "tick": tick, "pos": pos, "action": {"kind":"use", "item":item,
                    "face":"up", "hit":[0.5,1.0,0.5], "yaw":90.0, "pitch":0.0, "sneaking":false}
                })
            })
            .collect();
        let sc: crate::scenario::Scenario = serde_json::from_value(serde_json::json!({
            "version":3, "seed":0, "ticks":40, "checkpoints":[36,37,38],
            "blocks":[
                {"pos":[1,4,-1], "state":"create:creative_motor[facing=east]", "motor":32},
                {"pos":[2,4,-1], "state":"create:cogwheel[axis=x,waterlogged=false]"},
                {"pos":[1,3,0], "state":"minecraft:stone"},
                {"pos":[2,3,0], "state":"minecraft:stone"},
                {"pos":[3,3,0], "state":"minecraft:stone"},
                {"pos":[4,3,0], "state":"minecraft:stone"},
                {"pos":[3,3,1], "state":"minecraft:stone"}
            ], "events":events
        }))
        .unwrap();
        let mut run = Run::new(&sc);
        run.fluid_host_enabled = false;
        let snapshot = super::super::execute(&sc, run);
        let branch = snapshot.checkpoints[&37].blocks["3,4,0"]
            .fluid_network
            .as_ref()
            .unwrap();
        assert_eq!(branch.connections["east"].pressure, [0., 16.]);
        assert_eq!(branch.connections["south"].pressure, [0., 16.]);
        assert_eq!(branch.connections["west"].pressure, [16., 0.]);
    }

    #[test]
    fn cauldron_transfer_conserves_fluid_without_native_peer() {
        for reload in [false, true] {
            let mut sc = crate::fluid_pumps::generate(18);
            seed_native_cauldron_fill(&mut sc);
            if reload {
                sc.events.push(crate::scenario::Change {
                    tick: 99,
                    pos: [2, 4, 0],
                    state: None,
                    motor: None,
                    turn: None,
                    action: Some(crate::actions::Action::Reload),
                });
            }
            let mut run = Run::new(&sc);
            run.fluid_host_enabled = false;
            let snapshot = super::super::execute(&sc, run);
            assert!(snapshot.notes.is_empty());
            let mut first = None;
            for (&tick, snap) in &snapshot.checkpoints {
                assert!(
                    snap.blocks.values().all(|b| b.world_fluid.is_none()),
                    "this fixture has no native fluid dynamics"
                );
                if tick < 61 {
                    continue;
                }
                let source = if snap.blocks["0,4,0"].state == "minecraft:water_cauldron[level=3]" {
                    1000
                } else {
                    0
                };
                let held = snap.blocks["4,4,0"].fluid_tank.as_ref().unwrap().amount;
                let buffer = snap.blocks["1,4,0"]
                    .fluid_network
                    .as_ref()
                    .unwrap()
                    .connections["west"]
                    .buffer
                    .amount();
                assert_eq!(
                    source + held + buffer,
                    1000,
                    "conservation at {tick}, reload={reload}"
                );
                if held > 0 && first.is_none() {
                    first = Some(tick);
                }
            }
            assert_eq!(
                snapshot.blocks["4,4,0"].fluid_tank.as_ref().unwrap().amount,
                1000
            );
            if !reload {
                assert_eq!(first, Some(85));
            }
            println!(
                "Rust-only cauldron replay: reload={reload}; first fill {first:?}; final 1000 mB; conserved at every checkpoint; no native peer"
            );
        }
    }
}

#[cfg(test)]
mod conversion_fidelity {
    use super::*;
    #[test]
    fn live_conversion_preserves_connection_owned_buffer() {
        let mut sc = crate::fluid_pumps::generate(1554);
        let fill=sc.events.iter().find(|e|matches!(&e.action,Some(crate::actions::Action::Use{item,..})if item=="minecraft:water_bucket")).unwrap().tick;
        super::tests::seed_native_cauldron_fill(&mut sc);
        let mut run = Run::new(&sc);
        run.fluid_host_enabled = false;
        let snapshot = super::super::execute(&sc, run);
        let before = &snapshot.checkpoints[&(fill + 40)].blocks["1,4,0"]
            .fluid_network
            .as_ref()
            .unwrap()
            .connections["west"]
            .buffer;
        let after = &snapshot.checkpoints[&(fill + 41)].blocks["1,4,0"]
            .fluid_network
            .as_ref()
            .unwrap()
            .connections["west"]
            .buffer;
        assert!(before.amount() > 0);
        let mut raw = Run::new(&sc);
        let regular=raw.id("create:fluid_pipe[down=false,east=true,north=false,south=false,up=false,waterlogged=false,west=true]");
        let glass = raw.id("create:glass_fluid_pipe[alt=false,axis=x,waterlogged=false]");
        raw.w.apply_block_change(0, [1, 4, 0], AIR, regular, false);
        raw.w.dims[0].fluid_transport.register(
            [1, 4, 0],
            Cell::Pipe(Dir::East.bit() | Dir::West.bit()),
            0.,
        );
        raw.w.dims[0]
            .fluid_transport
            .connection_mut(Face {
                pos: [1, 4, 0],
                side: Dir::West,
            })
            .unwrap()
            .set_source(Source::open(create_engine::open_end::OpenEnd {
                tank: Some((Fluid::Water, before.amount())),
                pulling: true,
            }));
        raw.replace_pipe_state([1, 4, 0], glass);
        let expected = raw.w.dims[0]
            .fluid_transport
            .connection(Face {
                pos: [1, 4, 0],
                side: Dir::West,
            })
            .and_then(|c| c.open_end())
            .and_then(|e| e.tank)
            .map_or(0, |(_, n)| n);
        let tank_before = snapshot.checkpoints[&(fill + 40)].blocks["4,4,0"]
            .fluid_tank
            .as_ref()
            .unwrap()
            .amount;
        let tank_after = snapshot.checkpoints[&(fill + 41)].blocks["4,4,0"]
            .fluid_tank
            .as_ref()
            .unwrap()
            .amount;
        assert_eq!(after.amount() + tank_after - tank_before, expected);
        println!(
            "Rust-only live conversion: buffer before {}, after {} (direct engine block-change API {}); final tank {} mB",
            before.amount(),
            after.amount(),
            expected,
            snapshot.blocks["4,4,0"].fluid_tank.as_ref().unwrap().amount
        );
    }
}
