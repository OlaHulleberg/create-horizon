//! The item drain/spout fluid phase and player boundaries from glue/fluids/machines.rs.
use super::*;
use crate::transport::{Input, Outcome, Stack};
use create_engine::{Block, belt, drain, kinetics::Dir, spout};
impl Run {
    pub(super) fn fluid_item_input(&mut self, p: Pos, input: &Input) -> Option<Outcome> {
        if self.w.block_at(0, p) != Some(Block::ItemDrain) {
            return None;
        }
        let offered = match input {
            Input::DropOn { stack } => Some(stack.clone()),
            Input::Use { stack, .. } => stack.clone(),
        };
        if matches!(
            input,
            Input::Use {
                stack: Some(_),
                sneaking: true,
                ..
            }
        ) {
            return Some(Outcome {
                pos: p,
                offered: offered.clone(),
                remainder: offered,
                taken: Vec::new(),
                handler: None,
            });
        }
        let mut remainder = offered.clone();
        let mut taken = Vec::new();
        match input {
            Input::DropOn { stack } => {
                let got = drain::insert(
                    self.w.dims[0].drains.get_mut(&p).unwrap(),
                    &stack.item,
                    stack.count,
                    Dir::North,
                    &crate::fluid_recipes::emptying(),
                );
                remainder = (got < stack.count).then(|| Stack {
                    item: stack.item.clone(),
                    count: stack.count - got,
                });
                if got > 0 {
                    self.mark_column(p);
                }
            }
            Input::Use { stack, .. } => {
                if let Some(stack) = stack
                    && let Some(empty) = crate::fluid_recipes::contents(&stack.item)
                {
                    let d = self.w.dims[0].drains.get_mut(&p).unwrap();
                    if d.fluid.is_none_or(|(f, n)| {
                        f == empty.fluid && n + empty.amount <= drain::CAPACITY
                    }) {
                        d.fluid = Some((empty.fluid, d.fluid.map_or(0, |(_, n)| n) + empty.amount));
                        self.mark_column(p);
                        self.w.dims[0].analog_checks.push(p);
                        if !self.creative {
                            if stack.count == 1 {
                                remainder = empty.leftover.map(|item| Stack { item, count: 1 });
                            } else {
                                remainder = Some(Stack {
                                    item: stack.item.clone(),
                                    count: stack.count - 1,
                                });
                                if let Some(item) = empty.leftover {
                                    taken.push(Stack { item, count: 1 });
                                }
                            }
                        }
                    }
                } else if let Some(held) = self.w.dims[0].drains.get_mut(&p).unwrap().held.take() {
                    taken.push(Stack {
                        item: held.item,
                        count: held.count,
                    });
                    self.mark_column(p);
                }
            }
        }
        Some(Outcome {
            pos: p,
            offered,
            remainder,
            taken,
            handler: None,
        })
    }
    pub(super) fn spout_on_depot(&mut self, depot: Pos) {
        let p = add(depot, [0, 2, 0]);
        let Some(mut s) = self.w.dims[0].spouts.get(&p).copied() else {
            return;
        };
        let recipes = crate::fluid_recipes::filling();
        let Some(mut h) = self.w.dims[0].depots.get(&depot).copied().flatten() else {
            return;
        };
        // `lv.blocked`: anything with a collision shape over the depot (a funnel too) stops an
        // unlocked stack.
        if !h.locked && self.host_collision(add(depot, [0, 1, 0])) {
            return;
        }
        {
            let depot = add(p, [0, -2, 0]);
            let held = self.depot_stacks.get(&depot).cloned();
            let before = s.fluid;
            let result = held
                .as_ref()
                .map(|stack| spout::on_depot(&mut s, &mut h, &stack.item, &recipes));
            if let Some(Some(h)) = self.w.dims[0].depots.get_mut(&depot) {
                h.locked = !matches!(result, Some(spout::OnBelt::Pass));
            }
            let filled = match result {
                Some(spout::OnBelt::Fill(out)) => Some(out),
                _ => None,
            };
            if let (Some(item), Some(mut stack)) = (filled, held) {
                if stack.count == 1 {
                    stack.item = item;
                    self.depot_stacks.insert(depot, stack);
                } else {
                    stack.count -= 1;
                    self.depot_stacks.insert(depot, stack);
                    let filled = Stack { item, count: 1 };
                    // glue/fluids/machines.rs: onto a stack of it with room, else an empty slot.
                    let outputs = self.depot_outputs.entry(depot).or_default();
                    if outputs.insert_stacked(&filled) > 0 {
                        self.notes.push(format!(
                            "adapter gap: native spout overflow item flight at {depot:?}"
                        ));
                    }
                }
                self.w.depot_changed(0, depot);
            }
            self.w.dims[0].spouts.insert(p, s);
            if s.fluid != before {
                self.w.dims[0].analog_checks.push(p);
                self.mark_column(p);
            }
        }
    }
    pub(super) fn tick_drain(&mut self, p: Pos) {
        for p in [p] {
            let result = drain::tick(
                self.w.dims[0].drains.get_mut(&p).unwrap(),
                &crate::fluid_recipes::emptying(),
            );
            if result == drain::Tick::Emptied {
                self.w.dims[0].analog_checks.push(p);
                self.mark_column(p);
            }
            let drain::Tick::Out(side) = result else {
                continue;
            };
            let held = self.w.dims[0].drains[&p].held.clone().unwrap();
            let next = add(p, side.offset());
            let left = if let Some(d) = self.w.dims[0].drains.get_mut(&next) {
                let got = drain::insert(
                    d,
                    &held.item,
                    held.count,
                    side,
                    &crate::fluid_recipes::emptying(),
                );
                if got > 0 {
                    self.mark_column(next);
                }
                Some(held.count - got)
            } else {
                self.insert_transport(
                    next,
                    side,
                    &belt::Item::new(&held.item, held.count, 0, false),
                )
            };
            let left = match left {
                Some(left) => left,
                None if !self.host_collision(next) => 0,
                None => continue,
            };
            if left != held.count {
                let d = self.w.dims[0].drains.get_mut(&p).unwrap();
                if left == 0 {
                    d.held = None;
                } else {
                    d.held.as_mut().unwrap().count = left;
                }
                self.mark_column(p);
            }
        }
    }
    pub(super) fn spout_on_belt(
        &mut self,
        p: Pos,
        item: &mut belt::Item,
        locked: bool,
    ) -> Option<belt::Process> {
        let above = add(p, [0, 2, 0]);
        let s = self.w.dims[0].spouts.get_mut(&above)?;
        let before = s.fluid;
        let result = spout::on_belt(s, &item.item, &crate::fluid_recipes::filling(), locked);
        if s.fluid != before {
            self.w.dims[0].analog_checks.push(above);
            self.mark_column(above);
        }
        Some(match result {
            spout::OnBelt::Pass => belt::Process::Pass,
            spout::OnBelt::Hold => belt::Process::Hold,
            spout::OnBelt::Fill(out) => {
                let mut filled = item.processing_copy();
                filled.item = out;
                filled.count = 1;
                filled.progress = None;
                let held = if item.count > 1 {
                    let mut rest = item.processing_copy();
                    rest.count -= 1;
                    Some(rest)
                } else {
                    None
                };
                belt::Process::Convert {
                    held,
                    outputs: vec![filled],
                }
            }
        })
    }
    pub(super) fn fluid_storage(&self, p: Pos) -> Option<crate::fluid_machines::Storage> {
        let (fluid, capacity) = if let Some(s) = self.w.dims[0].spouts.get(&p) {
            (s.fluid, spout::CAPACITY)
        } else {
            let d = self.w.dims[0].drains.get(&p)?;
            (d.fluid, drain::CAPACITY)
        };
        Some(crate::fluid_machines::Storage {
            capacity,
            fluid: fluid.map(|(f, _)| f.key()),
            amount: fluid.map_or(0, |(_, n)| n),
        })
    }
    pub(super) fn fluid_machine(&self, p: Pos) -> Option<crate::fluid_machines::Machine> {
        if let Some(s) = self.w.dims[0].spouts.get(&p) {
            return Some(crate::fluid_machines::Machine {
                processing: s.processing,
                held: None,
            });
        }
        let d = self.w.dims[0].drains.get(&p)?;
        Some(crate::fluid_machines::Machine {
            processing: d.processing,
            held: d.held.as_ref().map(|h| crate::fluid_machines::Held {
                item: h.item.clone(),
                count: h.count,
                position: h.position,
                from: format!("{:?}", h.from).to_lowercase(),
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use create_engine::tank::Fluid;

    #[test]
    fn depot_filling_matches_glue_step_and_preserves_all_outputs() {
        let mut sc = crate::fluid_inputs::generate(19);
        sc.blocks.clear();
        sc.events.clear();
        let mut run = Run::new(&sc);
        let p = [0, 6, 0];
        let depot = [0, 4, 0];
        run.set(p, run.id("create:spout"));
        run.set(depot, run.id("create:depot[waterlogged=false]"));
        run.depot_stacks.insert(
            depot,
            Stack {
                item: "minecraft:glass_bottle".into(),
                count: 4,
            },
        );
        let mut direct = spout::Spout::default();
        spout::fill(&mut direct, Fluid::Tea, 1000);
        run.w.dims[0].spouts.insert(p, direct);
        let mut held = create_engine::depot::Held::new(180, false);
        run.w.depot_put(0, depot, Some(held));
        let mut left = 4;
        for _ in 0..100 {
            let item = if left > 0 {
                "minecraft:glass_bottle"
            } else {
                "create:builders_tea"
            };
            spout::tick(&mut direct);
            if matches!(
                spout::on_depot(
                    &mut direct,
                    &mut held,
                    item,
                    &crate::fluid_recipes::filling()
                ),
                spout::OnBelt::Fill(_)
            ) {
                left -= 1;
            }
            run.tick_transport();
            assert_eq!(run.w.dims[0].spouts[&p], direct);
            assert_eq!(run.depot_stacks[&depot].count, left.max(1));
        }
        assert_eq!(direct.fluid, None);
        assert_eq!(run.depot_stacks[&depot].item, "create:builders_tea");
        assert_eq!(
            run.depot_outputs[&depot]
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec![Stack {
                item: "create:builders_tea".into(),
                count: 3
            }]
        );
    }

    #[test]
    fn landed_bucket_matches_glue_drain_cycle_without_native_peer() {
        let mut sc = crate::fluid_inputs::generate(19);
        sc.blocks.clear();
        sc.events.clear();
        let mut run = Run::new(&sc);
        let p = [0, 4, 0];
        run.set(p, run.id("create:item_drain"));
        let input = Input::DropOn {
            stack: Stack {
                item: "minecraft:water_bucket".into(),
                count: 1,
            },
        };
        assert!(run.fluid_item_input(p, &input).unwrap().remainder.is_none());
        let mut direct = drain::Drain::default();
        drain::insert(
            &mut direct,
            "minecraft:water_bucket",
            1,
            Dir::North,
            &crate::fluid_recipes::emptying(),
        );
        for _ in 0..23 {
            drain::tick(&mut direct, &crate::fluid_recipes::emptying());
            run.tick_drain(p);
            assert_eq!(run.w.dims[0].drains[&p], direct);
        }
        assert_eq!(direct.fluid, Some((Fluid::Water, 1000)));
        assert_eq!(direct.held.unwrap().item, "minecraft:bucket");
    }
}
