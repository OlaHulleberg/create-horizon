//! Logistics phase and player item boundaries, mirrored from glue/logistics/{belt,depot}.rs.
use super::*;
use crate::transport::{Contents, Input, Item as ObservedItem, Outcome, Stack};
use create_engine::{
    Block, belt, depot,
    kinetics::{Axis, Dir},
};
impl Run {
    pub(super) fn belt_location(&self, p: Pos) -> Option<(Pos, i32, belt::Segment)> {
        let seg = self.w.table.get(self.at(p))?.1.belt?;
        let c = *self.w.dims[0].belt_of.get(&p)?;
        let index = self.w.dims[0]
            .belts
            .get(&c)?
            .chain
            .iter()
            .position(|q| *q == p)? as i32;
        Some((c, index, seg))
    }
    pub(super) fn transport_empty_use(&mut self, p: Pos, face: Dir, sneaking: bool) -> bool {
        if self.belt_location(p).is_some() || self.w.block_at(0, p) == Some(Block::Depot) {
            self.item_input(
                p,
                &Input::Use {
                    stack: None,
                    face: match face {
                        Dir::Up => crate::actions::Face::Up,
                        Dir::Down => crate::actions::Face::Down,
                        Dir::East => crate::actions::Face::East,
                        Dir::West => crate::actions::Face::West,
                        Dir::North => crate::actions::Face::North,
                        Dir::South => crate::actions::Face::South,
                    },
                    yaw: 0.,
                    sneaking,
                },
            );
            true
        } else {
            false
        }
    }
    pub(super) fn item_input(&mut self, p: Pos, input: &Input) {
        if let Some(outcome) = self.fluid_item_input(p, input) {
            self.item_outcomes.push(outcome);
            return;
        }
        let mut taken = Vec::new();
        let offered = match input {
            Input::DropOn { stack } => Some(stack.clone()),
            Input::Use { stack, .. } => stack.clone(),
        };
        let mut remainder = offered.clone();
        match input {
            Input::DropOn { stack } => {
                if self.belt_location(p).is_some() {
                    if self.put_on_from_above(p, stack) == Some(0) {
                        remainder = None;
                    }
                } else if self.is_ejector(p) {
                    // `item_landed`: an ejector merges it onto what it holds.
                    let left = self.ejector_insert(p, stack, false);
                    remainder = (left > 0).then(|| Stack {
                        item: stack.item.clone(),
                        count: left,
                    });
                } else if self.w.dims[0].depots.contains_key(&p)
                    && !self.depot_stacks.contains_key(&p)
                    // `item_landed`: only into a depot whose slots are all empty.
                    && self.depot_outputs.get(&p).is_none_or(|o| o.is_empty())
                {
                    self.depot_stacks.insert(p, stack.clone());
                    self.w.depot_put(0, p, Some(depot::Held::new(180, false)));
                    remainder = None;
                }
            }
            Input::Use {
                stack,
                face,
                yaw,
                sneaking,
            } => {
                if let Some((c, index, _)) = self.belt_location(p) {
                    if !sneaking && stack.is_none() {
                        taken = self.w.dims[0]
                            .belts
                            .get_mut(&c)
                            .unwrap()
                            .items
                            .take_within(index as f32 + 0.5, 0.55)
                            .into_iter()
                            .map(|i| Stack {
                                item: i.item,
                                count: i.count,
                            })
                            .collect();
                        if !taken.is_empty() {
                            self.w.belt_inventory_changed(0, c);
                        }
                    }
                } else if self.w.dims[0].depots.contains_key(&p) {
                    if face.dir() != Dir::Up {
                        self.item_outcomes.push(Outcome {
                            pos: p,
                            offered: offered.clone(),
                            remainder: offered,
                            taken: vec![],
                            handler: None,
                        });
                        return;
                    }
                    let mut inventory = DepotUse {
                        run: self,
                        pos: p,
                        hand: stack.clone(),
                        taken: Vec::new(),
                        facing: depot::Side::of_yaw(*yaw),
                    };
                    depot::use_top(
                        &mut inventory,
                        stack.is_none(),
                        stack
                            .as_ref()
                            .is_some_and(|s| s.item == "create:mechanical_arm"),
                    );
                    remainder = inventory.hand;
                    taken = inventory.taken;
                }
                // The remaining blocks in the bounded contract have no stack-transfer
                // handler for these plain items. A refused use leaves the offered stack.
            }
        }
        // Mirror the depot interaction's publication of HeldItem, including motion.
        self.publish_interaction();
        let mut merged = BTreeMap::<String, u8>::new();
        for s in taken {
            *merged.entry(s.item).or_default() += s.count;
        }
        self.item_outcomes.push(Outcome {
            pos: p,
            offered,
            remainder,
            taken: merged
                .into_iter()
                .map(|(item, count)| Stack { item, count })
                .collect(),
            handler: None,
        });
    }
    /// `funnel_direct_input`: only an inserting, unpowered funnel facing up takes the stack, into
    /// the inventory under it (`handleDirectBeltInput`); the count left.
    fn funnel_direct_input(
        &mut self,
        p: Pos,
        block: Block,
        info: create_engine::parts::StateInfo,
        item: &belt::Item,
    ) -> u8 {
        if !matches!(block, Block::AndesiteFunnel | Block::BrassFunnel)
            || info.extracting
            || info.facing != Some(Dir::Up)
            || info.powered
        {
            return item.count;
        }
        // `filtering.test`: a brass funnel takes only what its filter passes.
        if !self.funnel_passes(p, &item.item) {
            return item.count;
        }
        let behind = create_engine::funnel::behind(p, Dir::Up);
        let stack = Stack {
            item: item.item.clone(),
            count: item.count,
        };
        match self.w.table.get(self.at(behind)).map(|(b, _)| b) {
            Some(Block::Depot) => self
                .depot_insert(behind, &stack, false)
                .map_or(0, |s| s.count),
            Some(b) => {
                self.notes.push(format!(
                    "adapter gap: funnel insertion into create:{} at {behind:?}",
                    b.name()
                ));
                item.count
            }
            // `InvManipulationBehaviour` without a target inserts nothing.
            None => item.count,
        }
    }
    /// `DepotItems.insert` into the held slot of the depot at `p`: only while every slot is
    /// empty, up to the stack limit; the held stack arrives at the edge
    /// (`depot_contents_changed`). What is left of `s`.
    pub(super) fn depot_insert(&mut self, p: Pos, s: &Stack, simulate: bool) -> Option<Stack> {
        let empty = !self.depot_stacks.contains_key(&p)
            && self.depot_outputs.get(&p).is_none_or(|o| o.is_empty());
        if !empty {
            return Some(s.clone());
        }
        let n = s.count.min(s.limit());
        if !simulate {
            self.depot_stacks.insert(
                p,
                Stack {
                    item: s.item.clone(),
                    count: n,
                },
            );
            self.w.depot_put(
                0,
                p,
                Some(depot::Held {
                    position: 0.,
                    ..depot::Held::new(180, false)
                }),
            );
        }
        (s.count > n).then(|| Stack {
            item: s.item.clone(),
            count: s.count - n,
        })
    }
    /// `handlers::provide` for an item access at `p`, then one insert or extract: a depot's
    /// `DepotItems` takes into its held slot only while it holds nothing, gives from any slot.
    /// The port provides no handler for belts or andesite tunnels.
    pub(super) fn capability(
        &mut self,
        p: Pos,
        slot: u8,
        insert: Option<&Stack>,
        extract: Option<u8>,
        simulate: bool,
    ) {
        let mut outcome = Outcome {
            pos: p,
            offered: insert.cloned(),
            remainder: insert.cloned(),
            taken: Vec::new(),
            handler: Some(false),
        };
        let block = self.w.table.get(self.at(p)).map(|(b, _)| b);
        match block {
            Some(Block::Depot | Block::WeightedEjector) => {
                outcome.handler = Some(true);
                if let Some(s) = insert {
                    if slot == 0 && self.is_ejector(p) {
                        let left = self.ejector_handler_insert(p, s, simulate);
                        outcome.remainder = (left > 0).then(|| Stack {
                            item: s.item.clone(),
                            count: left,
                        });
                    } else if slot == 0 {
                        outcome.remainder = self.depot_insert(p, s, simulate);
                    }
                } else if let Some(n) = extract {
                    if slot == 0 {
                        if let Some(held) = self.depot_stacks.get(&p).cloned() {
                            let k = n.min(held.count);
                            outcome.taken.push(Stack {
                                item: held.item.clone(),
                                count: k,
                            });
                            if !simulate {
                                if k == held.count {
                                    self.depot_stacks.remove(&p);
                                    self.w.depot_put(0, p, None);
                                } else if let Some(h) = self.depot_stacks.get_mut(&p) {
                                    h.count -= k;
                                }
                            }
                        }
                    } else if let Some(out) = self.depot_outputs.get_mut(&p) {
                        // `take` out of the block entity's slot: the buffer's slot `slot - 1`.
                        if let Some(s) = out.take(usize::from(slot) - 1, n, simulate) {
                            outcome.taken.push(s);
                        }
                        if !simulate {
                            self.w.depot_changed(0, p);
                        }
                    }
                }
            }
            Some(Block::BrassTunnel) => {
                self.brass_capability(p, slot, insert, extract, simulate, &mut outcome)
            }
            Some(Block::Belt) => {
                self.belt_capability(p, slot, insert, extract, simulate, &mut outcome)
            }
            Some(Block::AndesiteTunnel) | None => {}
            Some(b) => self.notes.push(format!(
                "adapter gap: item handler of create:{} at {p:?}",
                b.name()
            )),
        }
        self.item_outcomes.push(outcome);
    }
    /// The glue's `handlers::BeltItems`, `ItemHandlerBeltSegment` (registered by
    /// `BeltBlockEntity.registerCapabilities`, none where the belt cannot transport): one slot at
    /// the segment. In: onto the segment while `canInsertAt` allows, from the next tick
    /// (`addItem`); out: `getStackAtOffset`, split in place, a stack it empties only marked
    /// `toRemove`.
    pub(super) fn belt_capability(
        &mut self,
        p: Pos,
        slot: u8,
        insert: Option<&Stack>,
        extract: Option<u8>,
        simulate: bool,
        outcome: &mut Outcome,
    ) {
        let Some((c, index, seg)) = self.belt_location(p) else {
            return;
        };
        if !seg.transports() {
            return;
        }
        outcome.handler = Some(true);
        if slot != 0 {
            return;
        }
        if let Some(s) = insert {
            let left = if simulate {
                // `insertItem` simulated: `canInsertAt`, and the count limited to one stack.
                let rpm = self.w.speed(0, c).unwrap_or(0.);
                let room = self.w.dims[0].belts.get(&c).is_some_and(|b| {
                    b.items.can_insert_at_from_side(
                        index,
                        Dir::Up,
                        belt::movement_facing(rpm, b.segment.facing),
                    )
                });
                if room {
                    s.count.saturating_sub(s.limit())
                } else {
                    s.count
                }
            } else {
                self.put_on_from_above(p, s).unwrap_or(s.count)
            };
            outcome.remainder = (left > 0).then(|| Stack {
                item: s.item.clone(),
                count: left,
            });
        } else if let Some(n) = extract {
            let d = &mut self.w.dims[0];
            let taken = d
                .belts
                .get_mut(&c)
                .and_then(|b| b.items.extract_at(index, n, simulate));
            if let Some((item, k)) = taken.filter(|(_, k)| *k > 0) {
                if !simulate {
                    d.belt_changes.push(c);
                }
                outcome.taken.push(Stack { item, count: k });
            }
        }
    }
    pub(super) fn tick_transport(&mut self) {
        // The glue's logistics phase: chutes, funnels, tunnels, then covers, then transport.
        // The glue's `break_pass` leads it: `BeltBlock.onRemove`'s break cascade runs at
        // block-change time in Java, before every block entity tick, so a tunnel over a removed
        // segment breaks in the same tick (`BeltTunnelBlock.neighborChanged`).
        self.belt_breaks();
        self.tick_chutes();
        self.tick_tunnels();
        for p in std::mem::take(&mut self.cover_checks) {
            self.update_cover(p);
        }
        let changed = self.w.take_depot_changes(0);
        for p in changed {
            if self.w.dims[0].depots.get(&p) == Some(&None) && self.depot_stacks.contains_key(&p) {
                if let Some(h) = self.w.dims[0].depots.get_mut(&p) {
                    *h = Some(depot::Held::new(180, false));
                }
                self.w.wake_depot(0, p);
            }
        }
        // The glue's shared level registration walk (BeltBlockEntity.tick and
        // DepotBehaviour.tick): apply transfers before the next entity's turn.
        self.tick_pumps();
        let mut phase = self.w.transport_ticks(0, u64::from(self.now));
        while let Some(turn) = phase.next(&mut self.w) {
            let c = match turn {
                create_engine::world::TransportTick::Fluid(p) => {
                    self.tick_transport_pipe(p);
                    self.tick_fluid_entity(p);
                    continue;
                }
                create_engine::world::TransportTick::Spout(p) => {
                    create_engine::spout::tick(self.w.dims[0].spouts.get_mut(&p).unwrap());
                    continue;
                }
                create_engine::world::TransportTick::Drain(p) => {
                    self.tick_drain(p);
                    continue;
                }
                create_engine::world::TransportTick::Pump(p)
                | create_engine::world::TransportTick::Pipe(p) => {
                    self.tick_transport_pipe(p);
                    continue;
                }
                create_engine::world::TransportTick::Depot(p) => {
                    let arrived = self.w.tick_depot(0, p, false);
                    // The glue's `tick_depot`: `handleBeltFunnelOutput` before the processing.
                    if !self.depot_output(p) && arrived {
                        self.spout_on_depot(p);
                    }
                    continue;
                }
                // The glue's `tick_brass` and `tick_funnel` turns (`BrassTunnelBlockEntity.tick`,
                // `FunnelBlockEntity.tick` in the level's block-entity order).
                create_engine::world::TransportTick::Brass(p) => {
                    self.tick_brass(p);
                    continue;
                }
                create_engine::world::TransportTick::Funnel(p) => {
                    self.tick_funnel(p);
                    continue;
                }
                // A weighted ejector is the depot behaviour it extends, ticked with its
                // block entity by this walk.
                create_engine::world::TransportTick::Ejector(p) => {
                    self.tick_ejector(p);
                    continue;
                }
                create_engine::world::TransportTick::Belt(c) => c,
            };
            let b = self.w.dims[0].belts.get_mut(&c).unwrap();
            let (seg, length, mut items) = (
                b.segment,
                b.chain.len() as i32,
                std::mem::take(&mut b.items),
            );
            let rpm = self.w.speed(0, c).unwrap_or(0.);
            let speed = belt::direction_aware_speed(rpm, seg.facing);
            let past =
                belt::position_for_offset(c, &seg, length, if speed > 0. { length } else { -1 });
            // Funnel-held items stay at their funnel's entry; the engine marks them `at_funnel`
            // rather than `stalled`: they are still where an item was before the tick.
            let held: Vec<f32> = items.items.iter().map(|i| i.position).collect();
            let ticked = items.tick(
                length,
                speed,
                seg.slope == belt::Slope::Horizontal,
                &mut Side {
                    run: self,
                    past,
                    movement: belt::movement_facing(rpm, seg.facing),
                    controller: c,
                    segment: seg,
                    length,
                },
            );
            let still: std::collections::HashSet<u32> = items
                .items
                .iter()
                .filter(|i| i.at_funnel && held.contains(&i.position))
                .map(|i| i.position.to_bits())
                .collect();
            self.funnel_held.insert(c, still);
            if let Some(b) = self.w.dims[0].belts.get_mut(&c) {
                let added = std::mem::replace(&mut b.items, items);
                for i in added.items {
                    b.items.add(i);
                }
            }
            if ticked.saved_changed {
                self.w.belt_inventory_changed(0, c);
            }
            if ticked.changed {
                self.w.dims[0].belt_changes.push(c);
            }
            // Native dropped-entity flight after an open end is outside this adapter.
            let _ = ticked;
        }
        let mut changed = std::mem::take(&mut self.w.dims[0].belt_changes);
        changed.sort_unstable();
        changed.dedup();
        self.save_belt_records(&changed);
        for p in changed {
            self.w.mark_unsaved(0, p);
        }
    }
    /// `put_on_from_above`: the whole stack onto the belt segment at `p` from above, at the
    /// segment's middle, when the segment has room; the count left, None without a belt that
    /// carries items there.
    pub(super) fn put_on_from_above(&mut self, p: Pos, stack: &Stack) -> Option<u8> {
        let (c, index, seg) = self.belt_location(p)?;
        if !seg.transports() {
            return None;
        }
        let rpm = self.w.speed(0, c).unwrap_or(0.);
        let b = self.w.dims[0].belts.get_mut(&c).unwrap();
        if !b.items.can_insert_at_from_side(
            index,
            Dir::Up,
            belt::movement_facing(rpm, b.segment.facing),
        ) {
            return Some(stack.count);
        }
        let mut i = belt::Item::new(&stack.item, stack.count, 180, false);
        i.inserted_at = index;
        i.inserted_from = Some(Dir::Up);
        i.position = index as f32 + 0.5 + if b.items.positive { -1. } else { 1. } / 16.;
        b.items.add(i);
        self.w.dims[0].belt_changes.push(c);
        self.w.belt_inventory_changed(0, c);
        Some(0)
    }
    /// `direct_input_at`: something at `p` takes items straight from a belt's end
    /// (`DirectBeltInputBehaviour`): a depot, a belt or an item drain.
    pub(super) fn direct_input_at(&self, p: Pos) -> bool {
        let d = &self.w.dims[0];
        // A belt segment is a `DirectBeltInputBehaviour` before its chain forms, too.
        d.depots.contains_key(&p)
            || self.w.block_at(0, p) == Some(Block::Belt)
            || d.drains.contains_key(&p)
            || self.funnel_at(p).is_some()
            || self.chute_at(p).is_some()
    }
    /// `funnel_at`: a funnel (belt funnel or not) at `p`: its block, facing and state facts.
    pub(super) fn funnel_at(&self, p: Pos) -> Option<(Block, create_engine::parts::StateInfo)> {
        let (block, info) = self.w.table.get(self.at(p))?;
        matches!(
            block,
            Block::AndesiteFunnel
                | Block::BrassFunnel
                | Block::AndesiteBeltFunnel
                | Block::BrassBeltFunnel
        )
        .then_some((block, *info))
    }
    /// `direct_input_accepts` (`canInsertFromSide`) for an item moving toward `side`: a running
    /// belt that is not sideways or vertical and does not run toward the side; the others always.
    pub(super) fn direct_input_accepts(&self, p: Pos, side: Dir) -> bool {
        if let Some((block, info)) = self.funnel_at(p) {
            return matches!(block, Block::AndesiteFunnel | Block::BrassFunnel)
                && !info.extracting
                && info.facing == Some(Dir::Up);
        }
        if self.chute_at(p).is_some() {
            return self.chute_takes_from_side(p);
        }
        if let Some((c, _, seg)) = self.belt_location(p) {
            let rpm = self.w.speed(0, c).unwrap_or(0.);
            return rpm != 0.
                && !matches!(seg.slope, belt::Slope::Sideways | belt::Slope::Vertical)
                && belt::movement_facing(rpm, seg.facing) != side.opposite();
        }
        if self.w.block_at(0, p) == Some(Block::Belt) {
            // A segment whose chain has not formed: `getControllerBE` finds nothing and
            // `tryInsertingFromSide` refuses.
            return false;
        }
        true
    }
    pub(super) fn insert_transport(&mut self, p: Pos, side: Dir, item: &belt::Item) -> Option<u8> {
        self.handle_insertion(p, side, item, true, false)
    }
    /// `handle_insertion`: what is left of `item` (moving toward `side`) after what takes items
    /// at `p` takes it (or would, `simulate`); None when nothing there takes items. `from_belt`:
    /// the smoother hand-over from a belt behind (`insert_into_from`).
    pub(super) fn handle_insertion(
        &mut self,
        p: Pos,
        side: Dir,
        item: &belt::Item,
        from_belt: bool,
        simulate: bool,
    ) -> Option<u8> {
        if simulate
            && (self.w.dims[0].drains.contains_key(&p)
                || self.funnel_at(p).is_some()
                || self.chute_at(p).is_some())
        {
            self.notes.push(format!(
                "adapter gap: simulated belt input into the block at {p:?}"
            ));
            return Some(item.count);
        }
        if let Some(d) = self.w.dims[0].drains.get_mut(&p) {
            let got = create_engine::drain::insert(
                d,
                &item.item,
                item.count,
                side,
                &crate::fluid_recipes::emptying(),
            );
            if got > 0 {
                self.mark_column(p);
            }
            return Some(item.count - got);
        }
        if let Some((block, info)) = self.funnel_at(p) {
            return Some(self.funnel_direct_input(p, block, info, item));
        }
        if self.chute_at(p).is_some() {
            return Some(self.chute_direct_input(p, item));
        }
        if self.w.dims[0].depots.contains_key(&p) {
            // An ejector merges onto what it holds, up to its stack size (`enableMerging`).
            let mut item = item.clone();
            let mut rest = 0;
            if self.is_ejector(p) {
                let s = Stack {
                    item: item.item.clone(),
                    count: item.count,
                };
                let left = self.ejector_insert(p, &s, true);
                if left == item.count || simulate {
                    return Some(left);
                }
                if self.depot_stacks.contains_key(&p) {
                    return Some(self.ejector_insert(p, &s, false));
                }
                item.count -= left;
                rest = left;
            }
            // `DepotBehaviour.tryInsertingFromSide`: only into an empty depot (an ejector's
            // only while charged).
            let charged = self.w.dims[0]
                .ejectors
                .get(&p)
                .is_none_or(|e| e.state == create_engine::ejector::State::Charged);
            if self.depot_stacks.contains_key(&p) || !charged {
                return Some(item.count);
            }
            if simulate {
                return Some(0);
            }
            self.depot_stacks.insert(
                p,
                Stack {
                    item: item.item.clone(),
                    count: item.count,
                },
            );
            self.w.depot_put(
                0,
                p,
                Some(depot::Held {
                    position: if side.axis() == Axis::Y { 0.5 } else { 0. },
                    from: depot::Side::from_dir(side),
                    angle: item.angle,
                    side_offset: item.side_offset,
                    locked: false,
                    fan: item.fan,
                }),
            );
            return Some(rest);
        }
        let Some((c, index, seg)) = self.belt_location(p) else {
            // `BeltBlockEntity.tryInsertingFromSide`: a segment whose chain has not formed
            // (a slicer's segment before its tick) has no controller; the stack comes back.
            if self.w.block_at(0, p) == Some(Block::Belt) {
                return Some(item.count);
            }
            return None;
        };
        if !seg.transports() {
            return Some(item.count);
        }
        // A brass tunnel over it that distributes takes the whole stack while it holds none and
        // its flap filter on that side passes it.
        let above = [p[0], p[1] + 1, p[2]];
        if self.has_distribution(above) {
            let from = side.opposite();
            if !self.brass_can_take(above) || !self.brass_filter_passes(above, from, &item.item) {
                return Some(item.count);
            }
            if !simulate {
                self.brass_set_stack(above, item, Some(from));
            }
            return Some(0);
        }
        let rpm = self.w.speed(0, c).unwrap_or(0.);
        let speed = belt::direction_aware_speed(rpm, seg.facing);
        let movement = belt::movement_facing(rpm, seg.facing);
        let b = self.w.dims[0].belts.get_mut(&c)?;
        if rpm == 0.
            || movement == side.opposite()
            || !b.items.can_insert_at_from_side(index, side, movement)
        {
            return Some(item.count);
        }
        if simulate {
            return Some(0);
        }
        let mut moved = item.clone();
        moved.locked = false;
        moved.position = index as f32 + 0.5 - speed.signum() / 16.;
        if side.axis() != Axis::Y {
            if movement != side {
                moved.side_offset = if side.positive() { 0.675 } else { -0.675 };
                if side.axis() == Axis::X {
                    moved.side_offset = -moved.side_offset;
                }
            } else {
                let extra = if from_belt { 0.26 } else { 0. };
                moved.position = if speed > 0. {
                    index as f32 - extra
                } else {
                    index as f32 + 1. + extra
                };
            }
        }
        moved.inserted_at = index;
        moved.inserted_from = Some(side);
        b.items.add(moved);
        self.w.belt_inventory_changed(0, c);
        Some(0)
    }
    pub(super) fn transport_observation(&self, p: Pos) -> Option<Contents> {
        if matches!(
            self.w.block_at(0, p),
            Some(Block::Depot | Block::WeightedEjector)
        ) {
            let items = self
                .depot_stacks
                .get(&p)
                .map(|s| {
                    let h = self.w.dims[0]
                        .depots
                        .get(&p)
                        .copied()
                        .flatten()
                        .expect("held depot motion");
                    ObservedItem {
                        stack: s.clone(),
                        position: h.position,
                        locked: h.locked,
                        pending: false,
                        stalled: None,
                    }
                })
                .into_iter()
                .collect();
            let mut outputs: Vec<Stack> = self
                .depot_outputs
                .get(&p)
                .map(|o| o.iter().cloned().collect())
                .unwrap_or_default();
            outputs.sort_by(|a, b| (&a.item, a.count).cmp(&(&b.item, b.count)));
            // glue/redstone/analog.rs: a depot's items over its held stack's max stack size (64
            // when it holds none), an ejector's over its stack size.
            let present: u32 = self.depot_stacks.get(&p).map_or(0, |s| u32::from(s.count))
                + outputs.iter().map(|s| u32::from(s.count)).sum::<u32>();
            let max = match self.w.dims[0].ejectors.get(&p) {
                Some(e) => u32::from(e.max_stack),
                None => self
                    .depot_stacks
                    .get(&p)
                    .map_or(64, |s| u32::from(s.limit())),
            };
            return Some(Contents {
                outputs,
                kind: "depot".into(),
                items,
                comparator: Some(create_engine::comparator::depot(present, max)),
            });
        }
        let b = self.w.dims[0].belts.get(&p)?;
        let mut live = b.items.items.clone();
        let mut items = Vec::new();
        for i in b.items.to_save().items {
            let pending = if let Some(index) = live.iter().position(|x| *x == i) {
                live.remove(index);
                false
            } else {
                true
            };
            items.push(ObservedItem {
                // An emptied stack is `ItemStack.EMPTY`, air.
                stack: Stack {
                    item: if i.count == 0 {
                        "minecraft:air".into()
                    } else {
                        i.item
                    },
                    count: i.count,
                },
                position: i.position,
                locked: i.locked,
                pending,
                stalled: if pending {
                    None
                } else {
                    // Java: `beltPosition == prevBeltPosition`, a funnel-held item included
                    // from its second tick at the entry.
                    Some(
                        self.w.speed(0, p).unwrap_or(0.) == 0.
                            || i.stalled
                            || self
                                .funnel_held
                                .get(&p)
                                .is_some_and(|s| s.contains(&i.position.to_bits())),
                    )
                },
            });
        }
        items.sort_by(|a, b| {
            a.position
                .total_cmp(&b.position)
                .then(a.stack.item.cmp(&b.stack.item))
                .then(a.stack.count.cmp(&b.stack.count))
        });
        Some(Contents {
            outputs: Vec::new(),
            kind: "belt".into(),
            items,
            comparator: None,
        })
    }
}
/// `depot::use_depot`'s inventory boundary. Transport inputs are explicitly
/// component-free (`Stack` rejects component fields), so equal item ids here
/// also have equal components. Stack limits come from the audited contract.
struct DepotUse<'a> {
    run: &'a mut Run,
    pos: Pos,
    hand: Option<Stack>,
    taken: Vec<Stack>,
    facing: depot::Side,
}
impl depot::PlayerUse for DepotUse<'_> {
    type Stack = Stack;

    fn take_contents(&mut self) -> Option<Vec<Stack>> {
        let mut contents: Vec<_> = self
            .run
            .depot_stacks
            .remove(&self.pos)
            .into_iter()
            .collect();
        contents.extend(self.run.depot_outputs.remove(&self.pos).unwrap_or_default());
        self.run.w.depot_put(0, self.pos, None);
        Some(contents)
    }

    fn give_back(&mut self, mut stack: Stack) {
        if let Some(hand) = &mut self.hand
            && hand.item == stack.item
        {
            let count = stack.count.min(hand.limit() - hand.count);
            hand.count += count;
            stack.count -= count;
        }
        // An initially empty hand only collects. Its refunds are reported as
        // taken rather than as a remainder, as ItemInputs measures on Java.
        if stack.count > 0 {
            self.taken.push(stack);
        }
    }

    fn place_hand(&mut self) {
        if let Some(stack) = self.hand.take() {
            self.run.depot_stacks.insert(self.pos, stack);
            self.run.w.depot_put(
                0,
                self.pos,
                Some(depot::Held::put_by_player(180, false, self.facing)),
            );
        }
    }
}

struct Side<'a> {
    run: &'a mut Run,
    past: Pos,
    movement: Dir,
    controller: Pos,
    segment: belt::Segment,
    length: i32,
}
impl belt::Surroundings for Side<'_> {
    fn ending(&mut self) -> belt::Ending {
        if self.run.direct_input_at(self.past) || self.run.belt_location(self.past).is_some() {
            belt::Ending::Insert
        } else if self.run.sturdy(self.past, self.movement.opposite()) {
            belt::Ending::Blocked
        } else {
            belt::Ending::Eject
        }
    }
    fn insert(&mut self, i: &belt::Item) -> Option<u8> {
        self.run.insert_transport(self.past, self.movement, i)
    }
    fn process(
        &mut self,
        index: i32,
        item: &mut belt::Item,
        locked: bool,
    ) -> Option<belt::Process> {
        let p = belt::position_for_offset(self.controller, &self.segment, self.length, index);
        // `belt_process` in subsystem order: processing (presses) before fluids (spouts).
        if let Some(r) = self.run.press_on_belt(p, item, locked) {
            return Some(r);
        }
        self.run.spout_on_belt(p, item, locked)
    }
    /// The glue's `BeltSide::funnel`: the belt funnel over `segment`, with the belt's way.
    fn funnel(&mut self, segment: i32) -> Option<(create_engine::funnel::OnBelt, Dir)> {
        let at = belt::position_for_offset(self.controller, &self.segment, self.length, segment);
        let pos = [at[0], at[1] + 1, at[2]];
        let (facing, shape) = self.run.belt_funnel_at(pos)?;
        let powered = self
            .run
            .w
            .table
            .get(self.run.at(pos))
            .is_some_and(|(_, i)| i.powered);
        Some((
            create_engine::funnel::OnBelt {
                facing,
                shape,
                powered,
            },
            self.movement,
        ))
    }
    /// The glue's `BeltSide::funnel_insert`: an andesite belt funnel puts the whole stack into
    /// the inventory behind it (`InvManipulationBehaviour.insert`); the count left.
    fn funnel_insert(&mut self, segment: i32, item: &belt::Item) -> Option<u8> {
        let at = belt::position_for_offset(self.controller, &self.segment, self.length, segment);
        let pos = [at[0], at[1] + 1, at[2]];
        let (facing, _) = self.run.belt_funnel_at(pos)?;
        // `filtering.test`: a brass funnel lets only what its filter passes in.
        if !self.run.funnel_passes(pos, &item.item) {
            return Some(item.count);
        }
        let behind = create_engine::funnel::behind(pos, facing);
        let mut stack = Stack {
            item: item.item.clone(),
            count: item.count,
        };
        match self.run.w.table.get(self.run.at(behind)).map(|(b, _)| b) {
            Some(Block::Depot) => {
                // A brass funnel set to exactly N: only a stack of at least N, and only when
                // all N fit; then N of it.
                let (amount, exact) = self.run.funnel_extraction(pos);
                let mut kept = 0;
                if exact {
                    let n = amount.min(255) as u8;
                    if n > item.count {
                        return Some(item.count);
                    }
                    stack.count = n;
                    let left = self
                        .run
                        .depot_insert(behind, &stack, true)
                        .map_or(0, |s| s.count);
                    if left > 0 {
                        return Some(item.count);
                    }
                    kept = item.count - n;
                }
                Some(
                    self.run
                        .depot_insert(behind, &stack, false)
                        .map_or(0, |s| s.count)
                        + kept,
                )
            }
            Some(b) => {
                self.run.notes.push(format!(
                    "adapter gap: funnel insertion into create:{} at {behind:?}",
                    b.name()
                ));
                None
            }
            None => None,
        }
    }
    /// The glue's `BeltSide::tunnel`: the tunnel over `segment` may take some of `item`; the
    /// flaps it sets are client animation and are not observed.
    fn tunnel(&mut self, segment: i32, item: &mut belt::Item) -> belt::TunnelStep {
        self.run.tunnel_takes(
            self.controller,
            self.segment,
            self.length,
            self.movement,
            segment,
            item,
        )
    }
}
