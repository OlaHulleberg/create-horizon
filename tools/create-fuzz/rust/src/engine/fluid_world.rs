//! The host-facing callbacks in glue/fluids/open_end::Front. World-fluid updates are
//! scheduled separately from Create's pressure/transfer phase.
use super::*;
use create_engine::{kinetics::Dir, open_end::Space, tank::Fluid};
pub(super) struct Front<'a> {
    pub run: &'a mut Run,
    pub pos: Pos,
}
impl Run {
    pub(super) fn world_fluid(&self, p: Pos) -> Option<(Fluid, u8)> {
        crate::fluid_world::fluid(&self.name(self.at(p)))
    }
    pub(super) fn waterloggable(&self, p: Pos) -> bool {
        parse_state(&self.name(self.at(p)))
            .1
            .iter()
            .any(|(k, _)| k == "waterlogged")
    }
    fn facts(&self, p: Pos) -> Option<&crate::fluid_host::Facts> {
        let (state, facts) = self.fluid_facts.get(&p)?;
        let current = self.name(self.at(p));
        let (old, mut op) = parse_state(state);
        let (new, mut np) = parse_state(&current);
        op.retain(|(k, _)| k != "waterlogged");
        np.retain(|(k, _)| k != "waterlogged");
        op.sort();
        np.sort();
        (old == new && op == np).then_some(facts)
    }
    pub(super) fn world_fluid_observation(
        &self,
        p: Pos,
    ) -> Option<crate::fluid_world::Observation> {
        if let Some((state, facts)) = self.fluid_facts.get(&p)
            && *state == self.name(self.at(p))
        {
            return facts.fluid.clone();
        }
        crate::fluid_world::observe(&self.name(self.at(p)))
    }
    pub(super) fn sturdy(&self, p: Pos, face: Dir) -> bool {
        self.facts(p).map_or_else(
            || !self.replaceable(p) && !self.waterloggable(p),
            |f| f.sturdy & face.bit() != 0,
        )
    }
    pub(super) fn replaceable(&self, p: Pos) -> bool {
        if let Some(facts) = self.facts(p) {
            return facts.replaceable;
        }
        self.at(p) == AIR
            || crate::fluid_world::fluid(&self.name(self.at(p))).is_some() && !self.waterloggable(p)
    }
    pub(super) fn host_collision(&self, p: Pos) -> bool {
        self.solid(p)
    }
    fn solid(&self, p: Pos) -> bool {
        self.facts(p)
            .map_or_else(|| !self.replaceable(p), |f| f.collision)
    }
    fn set_world_state(&mut self, p: Pos, state: &str) {
        let id = self.id(state);
        assert!(
            id != OTHER || state == "minecraft:stone",
            "unregistered world state {state}"
        );
        self.set(p, id);
    }
    pub(super) fn fluid_world_changed(&mut self, p: Pos) {
        if self.fluid_host_enabled {
            self.fluid_host_changes.push(crate::fluid_host::State {
                pos: p,
                state: self.name(self.at(p)),
            });
        }
    }
    pub(super) fn world_bucket(
        &mut self,
        p: Pos,
        item: &str,
        face: Dir,
        hit: [f64; 3],
        sneaking: bool,
    ) -> bool {
        let contained = Fluid::of_bucket(item);
        if item != "minecraft:bucket" && contained.is_none() {
            return false;
        }
        if contained.is_some_and(|f| !f.placeable()) {
            return true;
        }
        assert!(
            self.fluid_host_enabled,
            "world bucket input requires the private native host"
        );
        let mut host = self
            .fluid_host
            .take()
            .unwrap_or_else(crate::fluid_host::Host::new);
        let results = host.bucket(
            self.persisted.clock,
            std::mem::take(&mut self.fluid_host_changes),
            self.fluid_domain.iter().copied().collect(),
            crate::fluid_host::Bucket {
                pos: p,
                item: item.into(),
                face: format!("{face:?}").to_lowercase(),
                hit,
                creative: self.creative,
                sneaking,
            },
        );
        self.fluid_host = Some(host);
        match results {
            Ok(frame) => self.apply_native_frame(frame),
            Err(error) => {
                self.notes.push(error);
                self.native_failed = true;
            }
        }
        true
    }
    pub(super) fn tick_world_fluids(&mut self) {
        if !self.fluid_host_enabled {
            return;
        }
        let now = self.persisted.clock;
        let due: Vec<_> = self
            .fluid_water_due
            .keys()
            .copied()
            .filter(|t| *t <= now)
            .collect();
        let mut manual = Vec::new();
        for time in due {
            manual.extend(self.fluid_water_due.remove(&time).unwrap());
        }
        let changes = std::mem::take(&mut self.fluid_host_changes);
        let native = std::mem::take(&mut self.fluid_native_schedule);
        let flows = self.wheel_samples();
        let mut host = self
            .fluid_host
            .take()
            .unwrap_or_else(crate::fluid_host::Host::new);
        let results = host.tick(
            now,
            changes,
            manual,
            native,
            self.fluid_domain.iter().copied().collect(),
            flows.clone(),
        );
        let results = match results {
            Ok(frame) => frame,
            Err(error) => {
                self.notes.push(format!(
                    "adapter gap: fluid world escaped the observed region at tick {now}: {error}"
                ));
                self.native_failed = true;
                return;
            }
        };
        self.fluid_host = Some(host);
        let crate::fluid_host::Frame { flows: answers, .. } = &results;
        self.wheel_fluid_flow.clear();
        for (p, flow) in flows.into_iter().zip(answers.iter().copied()) {
            self.wheel_fluid_flow.insert(p, flow);
        }
        self.apply_native_frame(results);
    }
    fn apply_native_frame(&mut self, results: crate::fluid_host::Frame) {
        self.fluid_rules = results.rules;
        for state in results.states {
            self.fluid_facts
                .insert(state.pos, (state.state.clone(), state.facts));
            let current = self.name(self.at(state.pos));
            let (old_name, _) = parse_state(&current);
            let (new_name, new_props) = parse_state(&state.state);
            // Native environment may waterlog Create blocks. Their topology and block-entity
            // behavior remain owned by the Rust engine, never imported from Java Create.
            let next = if self.w.table.get(self.at(state.pos)).is_some() && old_name == new_name {
                if let Some((_, water)) = new_props.iter().find(|(k, _)| k == "waterlogged") {
                    with_prop(&current, "waterlogged", water)
                } else {
                    continue;
                }
            } else {
                state.state
            };
            if next != current {
                self.set_world_state(state.pos, &next);
            }
        }
    }
}
impl Space for Front<'_> {
    fn drain_block(&mut self, simulate: bool) -> Option<(Fluid, u32)> {
        let state = self.run.name(self.run.at(self.pos));
        let (name, props) = parse_state(&state);
        let honey = props
            .iter()
            .find(|(k, _)| k == "honey_level")
            .and_then(|(_, v)| v.parse::<u8>().ok());
        let (out, left) = if honey.is_some_and(|h| h >= 5) {
            ((Fluid::Honey, 250), with_prop(&state, "honey_level", "0"))
        } else if name == "minecraft:lava_cauldron" {
            ((Fluid::Lava, 1000), "minecraft:cauldron".into())
        } else if name == "minecraft:water_cauldron"
            && props.iter().any(|(k, v)| k == "level" && v == "3")
        {
            ((Fluid::Water, 1000), "minecraft:cauldron".into())
        } else {
            return None;
        };
        if !simulate {
            self.run.set_world_state(self.pos, &left);
        }
        Some(out)
    }
    fn waterloggable(&self) -> bool {
        self.run.waterloggable(self.pos)
    }
    fn replaceable(&self) -> bool {
        self.run.replaceable(self.pos)
    }
    fn fluid(&self) -> Option<(Option<Fluid>, bool)> {
        self.run
            .world_fluid_observation(self.pos)
            .map(|f| (Fluid::of_id(&f.fluid), f.source))
    }
    fn evaporates(&self) -> bool {
        self.run.fluid_rules.evaporates
    }
    fn spill(&mut self, pipe: Fluid, world: Option<Fluid>, source: bool) {
        if let Some(state) = world.and_then(|w| create_engine::open_end::spill(pipe, w, source)) {
            self.run.set_world_state(self.pos, state);
        }
    }
    fn set_waterlogged(&mut self, on: bool) {
        let next = with_prop(
            &self.run.name(self.run.at(self.pos)),
            "waterlogged",
            if on { "true" } else { "false" },
        );
        self.run.set_world_state(self.pos, &next);
        self.run
            .fluid_water_due
            .entry(self.run.persisted.clock + 1)
            .or_default()
            .push(self.pos);
    }
    fn place_source(&mut self, fluid: Fluid) {
        self.run
            .set_world_state(self.pos, &format!("{}[level=0]", fluid.id()));
    }
    fn take_source(&mut self) {
        let Some((fluid, _)) = self.run.world_fluid(self.pos) else {
            return;
        };
        let sources = [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]]
            .into_iter()
            .filter(|d| self.run.world_fluid(add(self.pos, *d)) == Some((fluid, 0)))
            .count();
        let below = add(self.pos, [0, -1, 0]);
        let converts = match fluid {
            Fluid::Water => self.run.fluid_rules.water_source_conversion,
            Fluid::Lava => self.run.fluid_rules.lava_source_conversion,
            _ => false,
        };
        if converts
            && sources >= 2
            && (self.run.solid(below) || self.run.world_fluid(below) == Some((fluid, 0)))
        {
            return;
        }
        self.run
            .set_world_state(self.pos, &format!("{}[level=14]", fluid.id()));
    }
    fn fizz(&mut self) {}
    fn effect(&mut self, _fluid: Fluid) {} // No entity/fire/candle workload admitted by this catalogue.
}

#[cfg(test)]
mod tests {
    use super::*;
    use create_engine::open_end::OpenEnd;
    fn run() -> Run {
        Run::new(&crate::fluid_world::generate(17))
    }
    #[test]
    fn source_drain_keeps_in_flight_remainder_and_renewable_source() {
        let mut r = run();
        let p = [0, 4, 0];
        r.set_world_state(p, "minecraft:water[level=0]");
        let mut e = OpenEnd::default();
        assert_eq!(
            e.drain(
                &mut Front {
                    run: &mut r,
                    pos: p
                },
                32,
                None,
                false
            ),
            Some((Fluid::Water, 32))
        );
        assert_eq!(r.world_fluid(p), Some((Fluid::Water, 0)));
        assert_eq!(
            e.drain(
                &mut Front {
                    run: &mut r,
                    pos: p
                },
                32,
                None,
                true
            ),
            Some((Fluid::Water, 32))
        );
        assert_eq!(e.tank, Some((Fluid::Water, 968)));
        assert_eq!(r.world_fluid(p), Some((Fluid::Water, 14)));
        r.set_world_state(p, "minecraft:water[level=0]");
        r.set_world_state([0, 3, 0], "minecraft:stone");
        for x in [-1, 1] {
            r.set_world_state([x, 4, 0], "minecraft:water[level=0]");
        }
        Front {
            run: &mut r,
            pos: p,
        }
        .take_source();
        assert_eq!(r.world_fluid(p), Some((Fluid::Water, 0)));
    }
    #[test]
    fn waterlogging_cauldron_and_spill_callbacks_use_real_states() {
        let mut r = run();
        let p = [0, 4, 0];
        r.set_world_state(
            p,
            "create:glass_fluid_pipe[alt=false,axis=x,waterlogged=true]",
        );
        let mut e = OpenEnd::default();
        assert_eq!(
            e.drain(
                &mut Front {
                    run: &mut r,
                    pos: p
                },
                1000,
                None,
                true
            ),
            Some((Fluid::Water, 1000))
        );
        assert!(r.world_fluid(p).is_none());
        r.set_world_state(p, "minecraft:water_cauldron[level=3]");
        assert_eq!(
            Front {
                run: &mut r,
                pos: p
            }
            .drain_block(true),
            Some((Fluid::Water, 1000))
        );
        assert_eq!(r.name(r.at(p)), "minecraft:water_cauldron[level=3]");
        assert_eq!(
            Front {
                run: &mut r,
                pos: p
            }
            .drain_block(false),
            Some((Fluid::Water, 1000))
        );
        assert_eq!(r.name(r.at(p)), "minecraft:cauldron");
        r.set_world_state(p, "minecraft:lava[level=0]");
        Front {
            run: &mut r,
            pos: p,
        }
        .spill(Fluid::Water, Some(Fluid::Lava), true);
        assert_eq!(r.name(r.at(p)), "minecraft:obsidian");
    }
}
