//! glue/processing/fan.rs for what the transport families can put in a current: the currents
//! (`tick_fans`, `fan_current`, `catalyst_at`) and a depot's held stack in them
//! (`fan_on_depot`). The admitted items have no splashing, smoking, smelting or blasting
//! recipe and resist no fire, so the recipe books are empty.
use super::*;
use create_engine::fan::{self, Books, Catalyst, Current, Heat, Kind, Way};
use create_engine::kinetics::Dir;
use create_engine::processing::Book;

fn step(p: Pos, d: Dir, n: i32) -> Pos {
    let o = d.offset();
    [p[0] + o[0] * n, p[1] + o[1] * n, p[2] + o[2] * n]
}

/// Full cubes among the states the families place.
fn full_cube(name: &str) -> bool {
    matches!(
        name,
        "minecraft:stone"
            | "minecraft:cobblestone"
            | "minecraft:obsidian"
            | "minecraft:magma_block"
            | "minecraft:redstone_block"
            | "create:limestone"
            | "create:scoria"
    )
}

/// `#create:fan_transparent` among the states the families place.
fn fan_transparent(name: &str) -> bool {
    matches!(
        name,
        "minecraft:campfire" | "minecraft:soul_campfire" | "create:blaze_burner"
    )
}

impl Run {
    /// `tick_fans`, without nozzles and entities (none in the admitted worlds).
    pub(super) fn tick_fans(&mut self) {
        for pos in self.w.fans_to_tick(0) {
            let Some(f) = self.w.dims[0].fans.get(&pos) else {
                continue;
            };
            let (rebuild, speed, facing) = (f.rebuild, f.speed, f.facing);
            if rebuild {
                let (current, handlers) = match facing {
                    Some(facing) if speed != 0.0 => self.fan_current(pos, facing, speed),
                    _ => Default::default(),
                };
                if let Some(f) = self.w.dims[0].fans.get_mut(&pos) {
                    f.rebuild = false;
                    f.changed |= f.current != current;
                    f.current = current;
                    f.handlers = handlers;
                }
            }
            if speed == 0.0 {
                continue;
            }
            let handlers = self.w.dims[0].fans[&pos].handlers.clone();
            for (at, kind) in handlers {
                let Some(kind) = kind else {
                    continue;
                };
                self.fan_on_depot(at, kind);
                if self.w.dims[0].belt_of.contains_key(&at) {
                    self.notes
                        .push(format!("adapter gap: fan processing on the belt at {at:?}"));
                }
            }
        }
    }

    fn fan_current(
        &mut self,
        pos: Pos,
        facing: Dir,
        speed: f32,
    ) -> (Current, Vec<(Pos, Option<Kind>)>) {
        let unknown = std::cell::Cell::new(None);
        let max = fan::flow_limit(fan::max_distance(speed), |i| {
            let p = step(pos, facing, i);
            let name = self.name(self.at(p));
            let (block, _) = parse_state(&name);
            if fan_transparent(&block) || !self.host_collision(p) {
                Way::Open
            } else {
                if !full_cube(&block) {
                    unknown.set(unknown.get().or(Some(p)));
                }
                Way::Full
            }
        });
        let current = Current::build(max, fan::pushing(speed, facing), |i| {
            self.catalyst_at(step(pos, facing, i))
        });
        let mut handlers = Vec::new();
        for i in 1..=current.limit() {
            let segment = current.type_at((i - 1) as f32);
            for below in 0..2 {
                let p = step(pos, facing, i);
                let p = [p[0], p[1] - below, p[2]];
                let d = &self.w.dims[0];
                if d.depots.contains_key(&p) || d.belt_of.contains_key(&p) {
                    handlers.push((p, self.catalyst_at(p).or(segment)));
                }
                if facing.axis() == create_engine::kinetics::Axis::Y {
                    break;
                }
            }
        }
        if let Some(p) = unknown.get()
            && current.segments.iter().any(|s| s.2.is_some())
        {
            self.notes.push(format!(
                "adapter gap: collision shape at {p:?} in a fan's current"
            ));
        }
        (current, handlers)
    }

    fn catalyst_at(&self, p: Pos) -> Option<Kind> {
        let fluid = self
            .world_fluid_observation(p)
            .filter(|f| f.amount > 0)
            .map(|f| f.fluid);
        let (block, props) = parse_state(&self.name(self.at(p)));
        let prop = |k: &str| props.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let mut c = Catalyst {
            water: matches!(
                fluid.as_deref(),
                Some("minecraft:water" | "minecraft:flowing_water")
            ),
            lava: matches!(
                fluid.as_deref(),
                Some("minecraft:lava" | "minecraft:flowing_lava")
            ),
            haunting: matches!(
                block.as_str(),
                "create:lit_blaze_burner" | "minecraft:soul_fire" | "minecraft:soul_campfire"
            ),
            smoking: matches!(
                block.as_str(),
                "create:blaze_burner"
                    | "create:lit_blaze_burner"
                    | "minecraft:fire"
                    | "minecraft:campfire"
            ),
            blasting: block == "create:blaze_burner",
            ..Default::default()
        };
        if !(c.haunting || c.smoking || c.blasting) {
            return fan::kind_at(&c);
        }
        if block.ends_with("campfire") {
            c.campfire_lit = prop("lit").map(|v| v == "true");
        }
        c.soul_flame = prop("flame_type").map(|v| v == "soul");
        c.heat = prop("blaze").and_then(|v| {
            Some(match v.as_str() {
                "none" => Heat::None,
                "smouldering" => Heat::Smouldering,
                "fading" => Heat::Fading,
                "kindled" => Heat::Kindled,
                "seething" => Heat::Seething,
                _ => return None,
            })
        });
        fan::kind_at(&c)
    }

    fn fan_on_depot(&mut self, at: Pos, kind: Kind) {
        let Some(Some(mut held)) = self.w.dims[0].depots.get(&at).copied() else {
            return;
        };
        let Some(stack) = self.depot_stacks.get(&at).cloned() else {
            return;
        };
        if kind == Kind::Haunting && stack.item == "minecraft:sweet_berries" {
            self.notes.push(format!(
                "adapter gap: haunting recipes on the depot at {at:?}"
            ));
            return;
        }
        let empty = Book::default();
        let books = Books {
            splashing: &empty,
            haunting: &empty,
            smoking: &empty,
            smelting: &empty,
            blasting: &empty,
        };
        let can = fan::can_process(kind, &stack.item, false, books);
        if !fan::advance(&mut held.fan, kind, u32::from(stack.count), can) {
            self.w.dims[0].depots.insert(at, Some(held));
            return;
        }
        let mut rng = create_engine::processing::Rng::new(0);
        let Some(results) = fan::process(kind, &stack.item, stack.count, books, |_| 64, &mut rng)
        else {
            self.w.dims[0].depots.insert(at, Some(held));
            return;
        };
        if !results.is_empty() {
            self.notes
                .push(format!("adapter gap: fan results on the depot at {at:?}"));
        }
        self.depot_stacks.remove(&at);
        self.w.dims[0].depots.insert(at, None);
        self.w.depot_changed(0, at);
    }
}
