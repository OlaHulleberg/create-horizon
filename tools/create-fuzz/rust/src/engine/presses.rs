//! Mechanical presses over depots and belts, mirroring glue/processing/press.rs: a depot whose
//! stack came to rest asks the press two above it (`tick_presses`, `press::depot_step`), which
//! starts its cycle when a pressing recipe takes the stack and presses one of it at the head's
//! lowest point (`results_on_depot`); a belt item under a press is held while the press runs and
//! converted at that point (`press_on_belt`, the belt's `process`). Depots ask and presses move
//! at their turns in the transport walk. Recipes are Create's plain pressing recipes of the
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
    /// `ServerLevel::processing_blocked` (`BeltProcessingBehaviour.isBlocked`) for the
    /// processing space `space`: a collision shape over it, but not a funnel's.
    pub(super) fn processing_blocked(&self, space: Pos) -> bool {
        let above = [space[0], space[1] + 1, space[2]];
        belt::processing_blocked(self.w.block_at(0, above), || self.host_collision(above))
    }
    /// `tick_presses`: the deployers the depots asked for (none in the contract, so the asks
    /// are dropped). Presses are asked at their depot's turn (`press_depot_turn`) and move at
    /// their own (`tick_press`), in the transport walk.
    pub(super) fn tick_presses(&mut self) {
        self.w.dims[0].depot_work.clear();
    }
    /// glue/processing/press.rs `depot_turn`: the depot at `at`, its stack arrived and not
    /// blocked, asks the press two above (`press::depot_step`).
    pub(super) fn press_depot_turn(&mut self, at: Pos) {
        use press::Step;
        let above = [at[0], at[1] + 2, at[2]];
        let Some(mut p) = self.w.dims[0].presses.get(&above).copied() else {
            return;
        };
        let Some(Some(mut held)) = self.w.dims[0].depots.get(&at).copied() else {
            return;
        };
        let Some(stack) = self.depot_stacks.get(&at).cloned() else {
            return;
        };
        let speed = self.w.speed(0, above).unwrap_or(0.);
        let fits = pressing(&stack.item).is_some();
        match press::depot_step(&mut held, &p, speed, fits) {
            Step::Wait => {}
            Step::Start => {
                p.start(press::Mode::Belt);
                self.w.dims[0].presses.insert(above, p);
            }
            Step::Process => held = self.press_on_depot(at, &stack, held),
        }
        if let Some(h) = self.w.dims[0].depots.get_mut(&at) {
            *h = h.map(|_| held);
        }
    }
    /// glue/logistics/depot.rs `process_held`: the machine two above an arrived stack, unless
    /// none is there or (unlocked) the block over the depot is in the way (`isBlocked`).
    pub(super) fn depot_process_held(&mut self, at: Pos) {
        let Some(block) = self.w.block_at(0, [at[0], at[1] + 2, at[2]]) else {
            return;
        };
        if !belt::processes_below(block) {
            return;
        }
        let locked = matches!(self.w.dims[0].depots.get(&at), Some(Some(h)) if h.locked);
        if !locked && self.processing_blocked(at) {
            return;
        }
        match block {
            create_engine::Block::Spout => self.spout_on_depot(at),
            create_engine::Block::MechanicalPress => self.press_depot_turn(at),
            _ => {}
        }
    }
    /// `press_on_depot` and `results_on_depot`: one of the held stack becomes the recipe's
    /// result: a single item is replaced by it (centred, locked), otherwise the rest stays held
    /// and the result goes to the outputs. The held stack's motion after.
    fn press_on_depot(&mut self, at: Pos, stack: &Stack, held: depot::Held) -> depot::Held {
        let Some(result) = pressing(&stack.item) else {
            return held;
        };
        let mut held = held;
        held.fan.clear();
        let out = Stack {
            item: result.into(),
            count: 1,
        };
        // `results_on_depot`: what stays held is centred (`setCenteredHeldItem`).
        let mut next = held.centred();
        if stack.count == 1 {
            next = depot::Held {
                locked: true,
                ..held
            }
            .centred();
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
        item.fan.clear();
        let mut out = item.processing_copy();
        out.item = result.into();
        out.count = 1;
        out.progress = None;
        out.locked = true;
        Some(Process::Convert {
            held: item.left_after_one(),
            outputs: vec![out],
        })
    }
}
