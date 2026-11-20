//! Funnels that move stacks out of the inventory behind them, mirroring glue/logistics/funnel.rs
//! `tick_funnels`: a belt funnel pushing onto its belt or onto the depot under it
//! (`activateExtractingBeltFunnel`), a belt funnel over a depot taking its stacks
//! (`handleBeltFunnelOutput`), and redstone pausing them. The inventory behind is a depot
//! (`DepotItems`); others, extracting funnels (whose stacks fly off as item entities), brass
//! funnels' amounts and machines below are adapter gaps with a note.
use super::*;
use crate::transport::Stack;
use create_engine::{
    Block, belt, depot,
    funnel::{EXTRACTION_COOLDOWN, Mode, belt_mode},
    kinetics::Dir,
};

/// What a funnel's turn did (`extract_in_front`'s answers): a stack left, something blocks it,
/// or the inventory gave nothing.
enum Turn {
    Moved,
    Blocked,
    Empty,
}

impl Run {
    fn note_once(&mut self, note: String) {
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
    }
    /// `belt_movement_below`: the way the belt that carries items below `p` moves.
    fn belt_movement_below(&self, p: Pos) -> Option<Dir> {
        let (c, _, seg) = self.belt_location([p[0], p[1] - 1, p[2]])?;
        if !seg.transports() {
            return None;
        }
        let rpm = self.w.speed(0, c).unwrap_or(0.);
        Some(belt::movement_facing(rpm, seg.facing))
    }
    /// The funnel's mode (`determineCurrentMode`).
    fn funnel_mode(&self, p: Pos) -> Option<(Block, create_engine::parts::StateInfo, Mode)> {
        let (block, info) = self.funnel_at(p)?;
        let facing = info.facing?;
        let mode = match block {
            Block::AndesiteBeltFunnel | Block::BrassBeltFunnel => belt_mode(
                info.powered,
                info.belt_funnel.unwrap_or_default(),
                self.belt_movement_below(p),
                facing,
            ),
            _ if info.powered => Mode::Paused,
            _ if info.extracting => Mode::Extract,
            _ => Mode::Collect,
        };
        Some((block, info, mode))
    }
    /// `take_filtered` from the depot behind (`DepotItems`, its held slot then its outputs):
    /// what the funnel at `pos` lets through, up to `amount` of the first item found gathered
    /// over the slots, or exactly `amount` of one item (`extract_matching`, `chute::extract`).
    fn take_from_depot(
        &mut self,
        pos: Pos,
        behind: Pos,
        (amount, exact): (u32, bool),
    ) -> Option<Stack> {
        let outputs = self.depot_outputs.get(&behind).cloned().unwrap_or_default();
        let slots: Vec<Option<Stack>> = std::iter::once(self.depot_stacks.get(&behind).cloned())
            .chain((0..super::outputs::SLOTS).map(|i| outputs.slot(i).cloned()))
            .collect();
        let keys: Vec<Option<(String, u32, u32)>> = slots
            .iter()
            .map(|s| {
                let s = s.as_ref().filter(|s| self.funnel_passes(pos, &s.item))?;
                Some((s.item.clone(), u32::from(s.count), u32::from(s.limit())))
            })
            .collect();
        let plan = create_engine::chute::extract(&keys, |_| true, exact, amount);
        let &(first, _) = plan.first()?;
        let item = keys[first].as_ref()?.0.clone();
        let mut taken = 0u8;
        for (slot, n) in plan {
            let n = n.min(255) as u8;
            taken += n;
            if slot == 0 {
                let held = self.depot_stacks.get_mut(&behind)?;
                held.count -= n;
                if held.count == 0 {
                    self.depot_stacks.remove(&behind);
                    self.w.depot_put(0, behind, None);
                } else {
                    self.w.depot_changed(0, behind);
                }
            } else if let Some(o) = self.depot_outputs.get_mut(&behind) {
                o.take(slot - 1, n, false);
                self.w.depot_changed(0, behind);
            }
        }
        (taken > 0).then_some(Stack { item, count: taken })
    }
    /// The inventory behind the funnel at `p` facing `facing`, when the driver models it.
    fn funnel_inventory(&mut self, p: Pos, facing: Dir) -> Option<Pos> {
        let behind = create_engine::funnel::behind(p, facing);
        match self.w.table.get(self.at(behind)).map(|(b, _)| b) {
            Some(Block::Depot) => Some(behind),
            Some(b) => {
                self.note_once(format!(
                    "adapter gap: funnel inventory create:{} at {behind:?}",
                    b.name()
                ));
                None
            }
            None => None,
        }
    }
    /// `tick_funnel`: a due funnel's turn in the transport walk; it acts as its mode says and
    /// is due again the next tick, after the cooldown once a stack left (`startCooldown`), or
    /// never while dormant.
    pub(super) fn tick_funnel(&mut self, pos: Pos) {
        let now = u64::from(self.now);
        let Some((block, info, mode)) = self.funnel_mode(pos) else {
            self.funnel_schedule(pos, u64::MAX);
            return;
        };
        let facing = info.facing.unwrap_or(Dir::Up);
        let belt_funnel = matches!(block, Block::AndesiteBeltFunnel | Block::BrassBeltFunnel);
        let below = [pos[0], pos[1] - 1, pos[2]];
        let depot_below = matches!(
            self.w.table.get(self.at(below)).map(|(b, _)| b),
            Some(Block::Depot | Block::WeightedEjector)
        );
        let next = match mode {
            Mode::Extract => {
                self.note_once(format!(
                    "adapter gap: extracting funnel at {pos:?} (native item flight)"
                ));
                None
            }
            Mode::PushingToBelt if depot_below => {
                let max = self.funnel_extraction(pos);
                self.push_onto_depot(pos, facing, max)
            }
            Mode::PushingToBelt => {
                let max = self.funnel_extraction(pos);
                self.push_onto_belt(pos, facing, max)
            }
            // The glue's `depot_output` hands a depot's stack over at the depot's turn; only
            // an ejector's (not in the walk) is pulled here.
            Mode::TakingFromBelt if depot_below && self.is_ejector(below) => {
                self.pull_from_depot(pos, facing);
                Some(Turn::Blocked)
            }
            _ => None,
        };
        let at = match next {
            Some(Turn::Moved) => now + u64::from(EXTRACTION_COOLDOWN) + 1,
            Some(_) => now + 1,
            None if belt_funnel => now + 1,
            None => u64::MAX,
        };
        self.funnel_schedule(pos, at);
    }
    fn funnel_schedule(&mut self, pos: Pos, at: u64) {
        let d = &mut self.w.dims[0];
        if let Some(due) = d.funnels.get_mut(&pos) {
            *due = at;
        }
    }
    /// `push_onto_belt` (`activateExtractingBeltFunnel`): one item of the inventory behind onto
    /// the belt below from the funnel's side while the belt takes items there.
    fn push_onto_belt(&mut self, pos: Pos, facing: Dir, max: (u32, bool)) -> Option<Turn> {
        let below = [pos[0], pos[1] - 1, pos[2]];
        let (c, index, _) = self.belt_location(below)?;
        let rpm = self.w.speed(0, c).unwrap_or(0.);
        let b = self.w.dims[0].belts.get(&c)?;
        let movement = belt::movement_facing(rpm, b.segment.facing);
        let free = rpm != 0.
            && movement != facing.opposite()
            && b.items.can_insert_at_from_side(index, facing, movement);
        if !free {
            return Some(Turn::Blocked);
        }
        let behind = self.funnel_inventory(pos, facing)?;
        let Some(stack) = self.take_from_depot(pos, behind, max) else {
            return Some(Turn::Empty);
        };
        let item = belt::Item::new(&stack.item, stack.count, 180, false);
        let left = self
            .handle_insertion(below, facing, &item, false, false)
            .unwrap_or(stack.count);
        if left > 0 {
            self.note_once(format!(
                "adapter gap: funnel at {pos:?} gives {left} back to its inventory"
            ));
        }
        Some(Turn::Moved)
    }
    /// `push_onto_depot`: the inventory behind onto the depot below while it holds nothing,
    /// sliding in from the funnel's side.
    fn push_onto_depot(&mut self, pos: Pos, facing: Dir, max: (u32, bool)) -> Option<Turn> {
        let below = [pos[0], pos[1] - 1, pos[2]];
        if self.is_ejector(below) {
            self.note_once(format!("adapter gap: funnel onto the ejector at {below:?}"));
            return None;
        }
        if self.depot_stacks.contains_key(&below)
            || self
                .depot_outputs
                .get(&below)
                .is_some_and(|o| !o.is_empty())
        {
            return Some(Turn::Blocked);
        }
        let behind = self.funnel_inventory(pos, facing)?;
        let Some(stack) = self.take_from_depot(pos, behind, max) else {
            return Some(Turn::Empty);
        };
        self.depot_stacks.insert(below, stack);
        self.w.depot_put(
            0,
            below,
            Some(depot::Held {
                position: 0.,
                from: depot::Side::from_dir(facing),
                ..depot::Held::new(180, false)
            }),
        );
        Some(Turn::Moved)
    }
    /// The glue's `depot_output` (`DepotBehaviour.tick` → `handleBeltFunnelOutput`): at the
    /// depot's turn, a belt funnel pulling from it above takes what `pull_from_depot` gives.
    /// True when a stack moved.
    pub(super) fn depot_output(&mut self, depot: Pos) -> bool {
        let pos = [depot[0], depot[1] + 1, depot[2]];
        let Some((block, info, mode)) = self.funnel_mode(pos) else {
            return false;
        };
        if !matches!(block, Block::AndesiteBeltFunnel | Block::BrassBeltFunnel)
            || mode != Mode::TakingFromBelt
        {
            return false;
        }
        let facing = info.facing.unwrap_or(Dir::Up);
        matches!(self.pull_from_depot(pos, facing), Some(Turn::Moved))
    }
    /// `pull_from_depot` (`handleBeltFunnelOutput`): the depot's outputs, then its held stack
    /// once at rest, into the inventory behind the funnel as far as they fit.
    fn pull_from_depot(&mut self, pos: Pos, facing: Dir) -> Option<Turn> {
        let below = [pos[0], pos[1] - 1, pos[2]];
        if self.is_ejector(below) {
            self.note_once(format!("adapter gap: funnel over the ejector at {below:?}"));
            return None;
        }
        let resting = self.w.dims[0]
            .depots
            .get(&below)
            .is_some_and(|h| h.is_none_or(|h| !h.sliding()));
        let behind = self.funnel_inventory(pos, facing)?;
        // The output slots in order, then the held one once it rests; the first that moves.
        for slot in 0..super::outputs::SLOTS {
            let Some(out) = self
                .depot_outputs
                .get(&below)
                .and_then(|o| o.slot(slot))
                .cloned()
            else {
                continue;
            };
            if !self.funnel_passes(pos, &out.item) {
                continue;
            }
            let left = self.depot_insert(behind, &out, false);
            if left.as_ref().is_some_and(|l| l.count == out.count) {
                continue;
            }
            if let Some(o) = self.depot_outputs.get_mut(&below) {
                o.set(slot, left);
            }
            self.w.depot_changed(0, below);
            return Some(Turn::Moved);
        }
        if !resting {
            return Some(Turn::Empty);
        }
        let Some(held) = self
            .depot_stacks
            .get(&below)
            .filter(|h| self.funnel_passes(pos, &h.item))
            .cloned()
        else {
            return Some(Turn::Empty);
        };
        let left = self.depot_insert(behind, &held, false);
        if left.as_ref().is_some_and(|l| l.count == held.count) {
            return Some(Turn::Empty);
        }
        match left {
            None => {
                self.depot_stacks.remove(&below);
                self.w.depot_put(0, below, None);
            }
            Some(rest) => {
                self.depot_stacks.insert(below, rest);
                self.w.depot_changed(0, below);
            }
        }
        Some(Turn::Moved)
    }
    /// `FunnelBlockEntity.extractionCooldown`: the ticks before the funnel's next turn, as its
    /// due tick says (0 while it acts each tick or is dormant).
    pub(super) fn funnel_cooldown(&self, p: Pos) -> i32 {
        let Some(&due) = self.w.dims[0].funnels.get(&p) else {
            return 0;
        };
        let now = u64::from(self.now);
        if due == u64::MAX || due <= now + 1 {
            return 0;
        }
        (due - now - 1) as i32
    }
}
