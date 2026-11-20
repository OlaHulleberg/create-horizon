use super::*;

impl Run {
    /// One level tick: inputs, then the solve and what it breaks.
    pub(super) fn tick(&mut self) {
        self.now += 1;
        self.persisted.clock += 1;
        // Shape notifications from committed inputs precede the level tick.
        self.tick_pipe_shapes();
        self.tick_structures();
        self.tick_large_wheels();
        self.w.dims[0].fluid_transport.begin_tick();
        self.tick_world_fluids();
        if self.native_failed {
            return;
        }
        self.apply_wheel_flow();
        let mut events = Vec::new();
        // Clutches and gearshifts follow redstone (`ClutchBlock.neighborChanged`): the
        // callback runs with the block change, before any block entity of this tick.
        for p in self.w.take_redstone(0) {
            let state = self.at(p);
            let Some(was) = self.w.powered(state) else {
                continue;
            };
            let now = self.signal(p) > 0;
            if now != was {
                let next = with_prop(
                    &self.name(state),
                    "powered",
                    if now { "true" } else { "false" },
                );
                let id = self.id(&next);
                // The glue's `tick_inputs`: GearshiftBlock.neighborChanged detaches first.
                self.w.transmission_power_changing(0, p);
                // ChainGearshiftBlock.neighborChanged sets with UPDATE_KNOWN_SHAPE.
                if self.w.reads_signal(state) {
                    self.commit_known_shape(p, id);
                } else {
                    self.commit(p, id, false);
                }
            }
            if self.w.reads_signal(state) {
                let signal = i32::from(self.signal(p));
                self.w.set_chain_signal(0, p, signal);
            }
        }
        // The scheduled block ticks the detach owes (`GearshiftBlock.tick`): the level runs
        // them before block entities tick, so the transmission re-attaches before any base
        // tick of this tick.
        self.w.reattach_scheduled_transmissions(0, &mut events);
        self.publish_after_effects(events);
        self.w.tick_cranks(0);
        for h in self.handles.values_mut() {
            h.remaining = h.remaining.saturating_sub(1);
        }
        self.apply_chain_shapes();
        let mut steps = Vec::new();
        for (p, running) in self.w.take_sequencer_checks(0) {
            let powered = self.signal(p) > 0;
            if let Some(step) = self.w.sequencer_redstone(0, p, powered, running) {
                steps.push((p, step));
            }
        }
        for (p, step) in steps {
            self.sequencer_step(p, step);
        }
        let mut events = Vec::new();
        self.tick_fluids();
        self.tick_boilers();
        self.tick_burners();
        // Ejector redstone before the walk ticks them (Java's `neighborChanged` runs at the
        // block change).
        self.ejector_signals();
        self.tick_transport();
        // `processing::Tools`: presses on what the depots and belts did this tick.
        self.tick_presses();
        self.ejectors_ticked();
        self.save_fluid_entities();
        self.w.settle_belts(0);
        // CreateWorld::tick: the solve, then sequencer instructions (committed after it).
        let (_, steps) = self.w.tick(0, &mut events);
        self.persisted.dirty_columns.extend(self.w.take_unsaved(0));
        for (p, step) in steps {
            self.sequencer_step(p, step);
        }
        self.w.dims[0].kinetics.take_removed();
        self.mark_package_columns();
        self.publish_after_effects(events);
    }

    /// Glue applies neighbour shapes in Inputs and immediately after each kinetic break.
    pub(super) fn apply_chain_shapes(&mut self) {
        for (p, drive) in self.w.take_chain_shapes(0) {
            let next = with_prop(
                &self.name(self.at(p)),
                "axis_along_first",
                if drive.along_first { "true" } else { "false" },
            );
            self.set(p, self.id(&next));
            let next = with_prop(&self.name(self.at(p)), "part", drive.part.name());
            self.set(p, self.id(&next));
        }
    }

    /// The glue's commit of a sequencer step: its `state` (kept while powered at the end).
    pub(super) fn sequencer_step(&mut self, p: [i32; 3], step: create_engine::sequencer::Step) {
        self.save_sequence(p);
        use create_engine::sequencer::Step;
        let n = match step {
            Step::Run(n) => n,
            Step::Idle => 0,
            Step::Ended if self.signal(p) > 0 => return,
            Step::Ended => 0,
        };
        let next = with_prop(&self.name(self.at(p)), "state", &n.to_string());
        self.set(p, self.id(&next));
    }
}
