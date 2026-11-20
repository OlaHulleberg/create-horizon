//! Mechanical presses over depots and belts, mirroring glue/processing/press.rs: a depot whose
//! stack came to rest asks the press two above it (`tick_presses`, `press::depot_step`), which
//! starts its cycle when a pressing recipe takes the stack and presses one of it at the head's
//! lowest point (`results_on_depot`); a belt item under a press is held while the press runs and
//! converted at that point (`press_on_belt`, the belt's `process`). Presses tick after the
//! logistics phase (`processing::Tools`). Recipes are Create's plain pressing recipes of the
//! transport contract's items.
use super::*;
use crate::transport::Stack;
use create_engine::{belt, depot, press};

/// Create's pressing recipes (`data/create/recipe/pressing`) for the contract's items: an ingot
/// (`c:ingots/*`) to its sheet.
fn pressing(item: &str) -> Option<&'static str> {
    match item {
        "minecraft:iron_ingot" => Some("create:iron_sheet"),
        "minecraft:gold_ingot" => Some("create:golden_sheet"),
        _ => None,
    }
}

impl Run {
    /// `ServerLevel::blocked`: a collision shape at `p` (a funnel's too).
    fn press_blocked(&self, p: Pos) -> bool {
        self.host_collision(p)
    }
    /// `tick_presses`: the depots whose stack arrived or changed ask the press above; running
    /// presses move their head.
    pub(super) fn tick_presses(&mut self) {
        use press::{Step, Tick};
        let d = &mut self.w.dims[0];
        let running: Vec<Pos> = d
            .presses
            .iter()
            .filter(|(_, p)| p.running)
            .map(|(k, _)| *k)
            .collect();
        let work = std::mem::take(&mut d.depot_work);
        if work.is_empty() && running.is_empty() {
            return;
        }
        let mut keep = Vec::new();
        for at in work {
            let above = [at[0], at[1] + 2, at[2]];
            let Some(mut p) = self.w.dims[0].presses.get(&above).copied() else {
                continue;
            };
            let Some(Some(mut held)) = self.w.dims[0].depots.get(&at).copied() else {
                continue;
            };
            if held.sliding() && held.position < 0.5 - 1. / 16. {
                continue;
            }
            let Some(stack) = self.depot_stacks.get(&at).cloned() else {
                continue;
            };
            let speed = self.w.speed(0, above).unwrap_or(0.);
            let in_way = self.press_blocked([at[0], at[1] + 1, at[2]]);
            let fits = pressing(&stack.item).is_some();
            match press::depot_step(&mut held, &p, speed, in_way, fits) {
                Step::Idle => {}
                Step::Wait => keep.push(at),
                Step::Start => {
                    p.start(press::Mode::Belt);
                    self.w.dims[0].presses.insert(above, p);
                    keep.push(at);
                }
                Step::Process => {
                    held = self.press_on_depot(at, &stack, held);
                    keep.push(at);
                }
            }
            if let Some(h) = self.w.dims[0].depots.get_mut(&at) {
                *h = h.map(|_| held);
            }
        }
        for at in running {
            let speed = self.w.speed(0, at).unwrap_or(0.);
            let d = &mut self.w.dims[0];
            let Some(p) = d.presses.get_mut(&at) else {
                continue;
            };
            if p.tick(speed) == Tick::Done {
                let below = [at[0], at[1] - 2, at[2]];
                if d.depots.contains_key(&below) && !keep.contains(&below) {
                    keep.push(below);
                }
            }
        }
        let d = &mut self.w.dims[0];
        for p in keep {
            if !d.depot_work.contains(&p) {
                d.depot_work.push(p);
            }
        }
    }
    /// `press_on_depot` and `results_on_depot`: one of the held stack becomes the recipe's
    /// result: a single item is replaced by it (centred, locked), otherwise the rest stays held
    /// and the result goes to the outputs. The held stack's motion after.
    fn press_on_depot(&mut self, at: Pos, stack: &Stack, held: depot::Held) -> depot::Held {
        let Some(result) = pressing(&stack.item) else {
            return held;
        };
        let out = Stack {
            item: result.into(),
            count: 1,
        };
        let mut next = held;
        if stack.count == 1 {
            next = depot::Held {
                position: 0.5,
                locked: true,
                ..held
            };
            self.depot_stacks.insert(at, out);
        } else {
            if let Some(h) = self.depot_stacks.get_mut(&at) {
                h.count -= 1;
            }
            if self
                .depot_outputs
                .entry(at)
                .or_default()
                .insert_stacked(&out)
                > 0
            {
                self.notes.push(format!(
                    "adapter gap: press output at {at:?} drops (native item flight)"
                ));
            }
        }
        self.w.depot_changed(0, at);
        next
    }
    /// `press_on_belt` for the belt item at segment `at`: None without a press two above.
    pub(super) fn press_on_belt(
        &mut self,
        at: Pos,
        item: &mut belt::Item,
        locked: bool,
    ) -> Option<belt::Process> {
        use belt::Process;
        let above = [at[0], at[1] + 2, at[2]];
        let mut p = self.w.dims[0].presses.get(&above).copied()?;
        if !locked && self.press_blocked([at[0], at[1] + 1, at[2]]) {
            return None;
        }
        if self.w.speed(0, above).unwrap_or(0.) == 0. {
            return Some(Process::Pass);
        }
        let result = pressing(&item.item);
        if !locked {
            if p.running {
                return Some(Process::Hold);
            }
            if result.is_none() {
                return Some(Process::Pass);
            }
            p.start(press::Mode::Belt);
            self.w.dims[0].presses.insert(above, p);
            return Some(Process::Hold);
        }
        if !p.running {
            return Some(Process::Pass);
        }
        if p.ticks != press::CYCLE / 2 {
            return Some(Process::Hold);
        }
        let result = result?;
        let mut out = item.clone();
        out.item = result.into();
        out.count = 1;
        out.progress = None;
        out.locked = true;
        let held = (item.count > 1).then(|| {
            let mut left = item.clone();
            left.count -= 1;
            left
        });
        Some(Process::Convert {
            held,
            outputs: vec![out],
        })
    }
}
