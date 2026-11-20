//! Tank state updates and the valve reconciliation phase from glue/fluids.
use super::*;
impl Run {
    /// fluids::Tanks::send_tank_changes, after logistics/tools/arms and before Solve.
    /// Drawing NBT is retained by the host, but is not a fluid load/restore input.
    pub(super) fn save_fluid_entities(&mut self) {
        let mut changed = std::mem::take(&mut self.w.dims[0].tank_changes);
        changed.sort();
        changed.dedup();
        for p in changed {
            self.w.dims[0].analog_checks.push(p);
            if self.w.block_at(0, p).is_some() {
                let m = self.w.dims[0].tanks.multis.get(&p).copied();
                self.persisted
                    .fluid_entities
                    .insert(p, crate::persistence::fluids::shown_tank(m));
            }
        }
    }
    pub(super) fn tick_fluids(&mut self) {
        self.tick_pipe_shapes();
    }
    pub(super) fn tick_fluid_entity(&mut self, p: Pos) {
        let enabled = self.w.table.get(self.at(p)).map(|(_, i)| i.enabled);
        let change = self.w.tick_fluid_entity(0, p, enabled);
        if let Some(enabled) = change {
            let next = with_prop(
                &self.name(self.at(p)),
                "enabled",
                if enabled { "true" } else { "false" },
            );
            self.switch(p, self.id(&next));
        }
        for p in self.w.take_tank_states(0) {
            if let Some(shape) = self.w.tank_shape(0, p) {
                let mut next = self.name(self.at(p));
                for (k, v) in [
                    ("top", if shape.top { "true" } else { "false" }),
                    ("bottom", if shape.bottom { "true" } else { "false" }),
                    ("shape", shape.shape.name()),
                ] {
                    next = with_prop(&next, k, v);
                }
                self.set(p, self.id(&next));
            }
        }
    }
    pub(super) fn fluid_use(
        &mut self,
        p: Pos,
        item: &str,
        _face: create_engine::kinetics::Dir,
        hit: [f64; 3],
        sneaking: bool,
    ) -> bool {
        use create_engine::{Block, fluids::SmartFilter};
        let Some((block, info)) = self.w.table.get(self.at(p)).map(|(b, i)| (b, *i)) else {
            return false;
        };
        // Native secondary use skips nonempty block-use callbacks; the item/wrench
        // path still runs. Verified with the ordinary creative tank bucket raycast.
        if sneaking && item != "minecraft:air" && item != "create:wrench" {
            return false;
        }
        if block == Block::SmartFluidPipe
            && !sneaking
            && item != "create:wrench"
            && let (Some(side), Some(facing)) = (info.face, info.facing)
            && create_engine::fluids::smart_pipe_filter_hit(side, facing, hit)
        {
            let filter = (item != "minecraft:air").then(|| SmartFilter {
                item: item.into(),
                fluid: crate::fluid_recipes::contents(item).map(|e| e.fluid),
            });
            let changed = if let Some(filter) = filter {
                self.w.dims[0].plumbing.filters.insert(p, filter.clone()) != Some(filter)
            } else {
                self.w.dims[0].plumbing.filters.remove(&p).is_some()
            };
            if changed {
                self.w.dims[0]
                    .fluid_transport
                    .changed
                    .push(create_engine::fluids::runtime::Change::Pipe(p));
                self.w.dims[0].plumbing.filter_changes.push(p);
                self.mark_column(p);
            }
            return true;
        }
        if matches!(block, Block::FluidTank | Block::CreativeFluidTank) {
            if item == "create:wrench" {
                if sneaking {
                    self.set(p, AIR);
                } else {
                    self.w.toggle_tank_windows(0, p);
                }
                return true;
            }
            if (self.creative || block == Block::CreativeFluidTank) && item != "minecraft:air" {
                if let Some(empty) = crate::fluid_recipes::contents(item) {
                    if block == Block::CreativeFluidTank {
                        self.w.set_creative_fluid(0, p, Some(empty.fluid));
                        return true;
                    }
                    if self
                        .w
                        .fill_boiler(0, p, empty.fluid, empty.amount)
                        .is_some()
                    {
                        return true;
                    }
                    if let Some((_, m)) = self.w.tank(0, p) {
                        let mut t = m.tank;
                        if t.fill_amount(empty.fluid, empty.amount, m.capacity()) == empty.amount {
                            self.w.set_tank(0, p, t);
                            return true;
                        }
                    }
                }
                let content = if block == Block::CreativeFluidTank {
                    self.w.dims[0]
                        .plumbing
                        .creative_fluid(p)
                        .flatten()
                        .map(|f| (f, u32::MAX))
                } else if self.w.is_boiler(0, p) {
                    None
                } else {
                    self.w
                        .tank(0, p)
                        .and_then(|(_, m)| m.tank.fluid.map(|f| (f, m.tank.amount)))
                };
                if let Some((fluid, have)) = content
                    && let Some((need, _)) =
                        create_engine::spout::filling(item, fluid, &crate::fluid_recipes::filling())
                    && need <= have
                {
                    if block == Block::CreativeFluidTank {
                        self.w.set_creative_fluid(0, p, None);
                    } else {
                        self.w.drain_tank(0, p, need);
                    }
                    return true;
                }
                return crate::fluid_recipes::contents(item).is_some()
                    || create_engine::spout::fillable(item, &crate::fluid_recipes::filling());
            }
        }
        if block == Block::ItemDrain {
            if let Some(empty) = crate::fluid_recipes::contents(item) {
                let d = self.w.dims[0].drains.get_mut(&p).unwrap();
                if d.fluid.is_none_or(|(f, n)| {
                    f == empty.fluid && n + empty.amount <= create_engine::drain::CAPACITY
                }) {
                    d.fluid = Some((empty.fluid, d.fluid.map_or(0, |(_, n)| n) + empty.amount));
                    self.w.dims[0].analog_checks.push(p);
                    self.mark_column(p);
                }
                return true;
            }
            if crate::actions::BLOCKS.contains(&item)
                || ["create:andesite_casing", "create:brass_casing"].contains(&item)
            {
                return false;
            }
            if self.w.dims[0]
                .drains
                .get_mut(&p)
                .unwrap()
                .held
                .take()
                .is_some()
            {
                self.mark_column(p);
            }
            return true;
        }
        false
    }
    pub(super) fn fluid_tank(&self, p: Pos) -> Option<crate::fluids::Tank> {
        let creative = self.w.block_at(0, p) == Some(create_engine::Block::CreativeFluidTank);
        let (controller, m) = if creative {
            self.w.creative_tank(0, p)?
        } else {
            self.w.tank(0, p)?
        };
        Some(crate::fluids::Tank {
            controller,
            width: m.width,
            height: m.height,
            capacity: m.capacity(),
            fluid: if creative {
                self.w.dims[0].plumbing.creative_fluid(p).flatten()
            } else {
                m.tank.fluid
            }
            .map(|f| f.key()),
            amount: if creative {
                if self.w.dims[0]
                    .plumbing
                    .creative_fluid(p)
                    .flatten()
                    .is_some()
                {
                    m.capacity()
                } else {
                    0
                }
            } else {
                m.tank.amount
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{codec::Tag, fluids as saved};
    use create_engine::tank::{Fluid, Tank};

    fn tank_case() -> Scenario {
        serde_json::from_value(serde_json::json!({"version":3,"ticks":20,"blocks":[
            {"pos":[15,8,0],"state":"create:fluid_tank"},
            {"pos":[15,8,1],"state":"create:fluid_tank"},
            {"pos":[16,8,0],"state":"create:fluid_tank"},
            {"pos":[16,8,1],"state":"create:fluid_tank"}
        ]}))
        .unwrap()
    }

    #[test]
    fn accepted_tank_placement_survives_an_immediate_reload() {
        let sc: Scenario =
            serde_json::from_value(serde_json::json!({"version":3,"ticks":20,"blocks":[
                {"pos":[0,3,0],"state":"minecraft:stone"}
            ]}))
            .unwrap();
        let mut r = Run::new(&sc);
        r.place_player(
            [0, 4, 0],
            "create:fluid_tank",
            0.0,
            0.0,
            create_engine::kinetics::Dir::Up,
            true,
        );
        r.tick();
        r.place_player(
            [0, 5, 0],
            "create:fluid_tank",
            0.0,
            0.0,
            create_engine::kinetics::Dir::Up,
            true,
        );
        r.reload();
        r.tick();
        let (c, m) = r.w.tank(0, [0, 5, 0]).unwrap();
        assert_eq!(c, [0, 4, 0]);
        assert_eq!((m.height, m.capacity()), (2, 16000));
    }

    #[test]
    fn filled_multiblock_survives_repeated_cross_column_reloads() {
        let sc = tank_case();
        let mut r = Run::new(&sc);
        for b in &sc.blocks {
            r.place(b);
        }
        let p = [15, 8, 0];
        assert_eq!(r.w.fill_tank(0, p, Fluid::Water, 17000), 17000);
        assert!(r.w.toggle_tank_windows(0, p));
        r.tick();
        for _ in 0..2 {
            r.reload();
            for b in &sc.blocks {
                let (c, m) = r.w.tank(0, b.pos).unwrap();
                assert_eq!(c, p);
                assert_eq!(
                    (m.width, m.height, m.window, m.tank),
                    (
                        2,
                        1,
                        false,
                        Tank {
                            fluid: Some(Fluid::Water),
                            amount: 17000
                        }
                    )
                );
            }
            r.tick();
            assert!(r.name(r.at([16, 8, 1])).contains("shape=plain"));
        }
        // A drained single tank keeps its member record; stale multiblock data must go.
        for p in [[15, 8, 1], [16, 8, 0], [16, 8, 1]] {
            r.set(p, AIR);
        }
        r.w.set_tank(0, p, Tank::default());
        r.w.toggle_tank_windows(0, p);
        r.tick();
        assert!(r.persisted.fluid_columns.contains_key(&(0, 0)));
        r.reload();
        let (_, m) = r.w.tank(0, p).unwrap();
        assert_eq!(m, create_engine::tank::Multi::default());
    }

    #[test]
    fn reload_uses_published_column_not_live_or_drawing_data() {
        let sc = tank_case();
        let mut r = Run::new(&sc);
        for b in &sc.blocks {
            r.place(b);
        }
        let p = [15, 8, 0];
        r.w.fill_tank(0, p, Fluid::Water, 1000);
        r.tick();
        let shown = serde_json::to_vec(&r.persisted.fluid_entities[&p]).unwrap();
        r.w.fill_tank(0, p, Fluid::Water, 2000);
        // A reload is not an extra serialization operation; even drawing NBT can be newer
        // than the attachment, and KineticEntity::block_entity_on_load never reads Tank.
        r.save_fluid_entities();
        assert_ne!(
            serde_json::to_vec(&r.persisted.fluid_entities[&p]).unwrap(),
            shown
        );
        r.flush_persistence(); // no marked columns: the unsaved journal awaits Solve
        r.reload();
        assert_eq!(r.w.tank(0, p).unwrap().1.tank.amount, 1000);
        r.w.fill_tank(0, p, Fluid::Water, 2000);
        r.tick();
        r.reload();
        assert_eq!(r.w.tank(0, p).unwrap().1.tank.amount, 3000);
    }

    #[test]
    fn fluid_columns_keep_glue_machine_and_plumbing_save_semantics() {
        let mut r = Run::new(&tank_case());
        let d = &mut r.w.dims[0];
        d.plumbing.hose_saved.insert(
            [1, 8, 0],
            (
                2.1259,
                Tank {
                    fluid: Some(Fluid::Water),
                    amount: 1200,
                },
            ),
        );
        d.plumbing.filters.insert(
            [2, 8, 0],
            create_engine::fluids::SmartFilter {
                item: "minecraft:water_bucket".into(),
                fluid: Some(Fluid::Water),
            },
        );
        d.plumbing
            .creative_saved
            .insert([3, 8, 0], Some(Fluid::Lava));
        d.spouts.insert(
            [4, 8, 0],
            create_engine::spout::Spout {
                fluid: Some((Fluid::Water, 750)),
                processing: 8,
            },
        );
        d.drains.insert(
            [5, 8, 0],
            create_engine::drain::Drain {
                fluid: Some((Fluid::Water, 500)),
                processing: 7,
                held: Some(create_engine::drain::Held {
                    item: "minecraft:water_bucket".into(),
                    count: 1,
                    position: 0.375,
                    prev: 0.25,
                    from: create_engine::kinetics::Dir::East,
                }),
            },
        );
        let mut pipe = create_engine::fluids::transport::FluidTransportBehaviour::new(
            create_engine::kinetics::Dir::North.bit(),
        );
        pipe.connection(create_engine::kinetics::Dir::North)
            .unwrap()
            .set_source(create_engine::fluids::transport::Source::open(
                create_engine::open_end::OpenEnd {
                    tank: Some((Fluid::Water, 900)),
                    pulling: true,
                },
            ));
        d.fluid_transport.pipes.insert([6, 8, 0], pipe);
        let data = saved::save_column(&r.w, (0, 0));
        let encoded = serde_json::to_vec(&data).unwrap();
        r.w.dims[0] = Default::default();
        saved::load_column(&mut r.w, &serde_json::from_slice(&encoded).unwrap());
        let d = &r.w.dims[0];
        assert_eq!(d.plumbing.hose_saved[&[1, 8, 0]].0, 2.125);
        assert_eq!(
            d.plumbing.filters[&[2, 8, 0]].item,
            "minecraft:water_bucket"
        );
        assert_eq!(d.plumbing.creative_saved[&[3, 8, 0]], Some(Fluid::Lava));
        assert_eq!(d.spouts[&[4, 8, 0]].fluid, Some((Fluid::Water, 750)));
        assert_eq!(d.spouts[&[4, 8, 0]].processing, -1); // glue persists no spout cycle
        let dr = &d.drains[&[5, 8, 0]];
        assert_eq!(dr.processing, 7);
        assert_eq!(dr.held.as_ref().unwrap().prev, 0.375);
        assert_eq!(
            d.fluid_transport
                .connection(create_engine::fluids::runtime::Face {
                    pos: [6, 8, 0],
                    side: create_engine::kinetics::Dir::North
                })
                .unwrap()
                .open_end()
                .unwrap()
                .tank,
            Some((Fluid::Water, 900))
        );
        assert!(matches!(data.get("create_fluids"), Some(Tag::Compound(_))));
    }
    #[test]
    fn secondary_use_bypasses_the_tank_container_handler() {
        let mut sc = crate::fluid_inputs::generate(19);
        sc.blocks.clear();
        sc.events.clear();
        let mut run = Run::new(&sc);
        let p = [0, 4, 0];
        run.set(
            p,
            run.id("create:fluid_tank[bottom=true,shape=window,top=true]"),
        );
        run.creative = true;
        run.w
            .fill_tank(0, p, create_engine::tank::Fluid::Water, 2000);
        run.fluid_use(
            p,
            "minecraft:bucket",
            create_engine::kinetics::Dir::Up,
            [0.5, 1., 0.5],
            true,
        );
        assert_eq!(run.w.tank(0, p).unwrap().1.tank.amount, 2000);
        run.fluid_use(
            p,
            "minecraft:bucket",
            create_engine::kinetics::Dir::Up,
            [0.5, 1., 0.5],
            false,
        );
        assert_eq!(run.w.tank(0, p).unwrap().1.tank.amount, 1000);
    }

    #[test]
    fn fluid_click_waits_for_the_glues_column_publication() {
        let mut sc = crate::fluid_inputs::generate(19);
        sc.blocks.clear();
        sc.events.clear();
        let mut run = Run::new(&sc);
        let p = [0, 4, 0];
        run.set(
            p,
            run.id("create:creative_fluid_tank[bottom=true,shape=window,top=true]"),
        );
        run.publish_interaction(); // Previously completed frame's attachment.
        run.use_item(
            p,
            "minecraft:water_bucket",
            create_engine::kinetics::Dir::Up,
            [0.5, 1., 0.5],
            0.,
            0.,
            false,
        );
        assert_eq!(
            run.w.creative_tank(0, p).unwrap().1.tank.fluid,
            Some(create_engine::tank::Fluid::Water)
        );
        run.reload(); // No fluid write_entities/flush occurred between the click and save.
        assert_eq!(run.w.creative_tank(0, p).unwrap().1.tank.fluid, None);
    }

    #[test]
    fn moving_valve_commits_a_registered_enabled_state() {
        let mut sc = crate::fluids::generate(8);
        sc.blocks.clear();
        sc.events.clear();
        let mut run = Run::new(&sc);
        let p = [0, 4, 0];
        let id = run.id("create:fluid_valve[axis_along_first=false,enabled=false,facing=north,waterlogged=false]");
        run.set(p, id);
        run.w.dims[0].plumbing.valve_speed(p, 64.);
        for _ in 0..6 {
            run.tick_fluids();
            run.tick_transport();
        }
        assert!(run.name(run.at(p)).contains("enabled=true"));
        assert_eq!(run.w.block_at(0, p), Some(create_engine::Block::FluidValve));
    }
}
