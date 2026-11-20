//! Brass tunnels over belts, mirroring glue/logistics/tunnel.rs: their connection with brass
//! tunnels beside them (`isConnected`), `hasDistributionBehaviour`, taking a whole stack off the
//! belt or from a belt's end (`setStackToDistribute`), handing it out among the outputs of their
//! group as their mode says (`tick_brass`, `BrassTunnelBlockEntity.tick`/`distribute`), their
//! sided filters (a click on a filter slot), their mode (a value board pick, `set_brass_mode`),
//! an empty hand taking the group's stacks (`grab_stacks`) and their item handler
//! (`handlers::TunnelItems`).
use super::*;
use create_engine::{
    Block, belt,
    kinetics::{Axis, Dir},
    tunnel,
};

impl Run {
    /// The brass tunnel's axis at `p`, if a brass tunnel is there.
    fn brass_axis(&self, p: Pos) -> Option<Axis> {
        let (block, info) = self.w.table.get(self.at(p))?;
        (block == Block::BrassTunnel).then_some(info.axis?)
    }
    fn has_flaps(&self, p: Pos) -> bool {
        self.tunnel_state(p)
            .is_some_and(|(a, s)| !self.tunnel_flaps(p, a, s).is_empty())
    }
    /// `brass_adjacent` (`getAdjacent`): the brass tunnel of the same axis beside `p` across
    /// its axis (`left`: counterclockwise from the axis' positive way).
    fn brass_adjacent(&self, p: Pos, axis: Axis, left: bool) -> Option<Pos> {
        let next = tunnel::adjacent_pos(p, axis, left);
        (self.brass_axis(next) == Some(axis)).then_some(next)
    }
    /// Collects `(axis, has_flaps)` for every brass tunnel reachable from `roots` across its axis.
    pub(super) fn collect_brass_info(&self, roots: &[Pos]) -> HashMap<Pos, (Axis, bool)> {
        let mut brass_info = HashMap::new();
        let mut stack = roots.to_vec();
        while let Some(p) = stack.pop() {
            if brass_info.contains_key(&p) {
                continue;
            }
            let Some(axis) = self.brass_axis(p) else {
                continue;
            };
            brass_info.insert(p, (axis, self.has_flaps(p)));
            for left in [true, false] {
                stack.push(tunnel::adjacent_pos(p, axis, left));
            }
        }
        brass_info
    }
    /// `has_distribution` (`hasDistributionBehaviour`): flaps, and connected or a flap across
    /// its axis.
    pub(super) fn has_distribution(&self, p: Pos) -> bool {
        let Some(axis) = self.brass_axis(p) else {
            return false;
        };
        let Some((_, shape)) = self.tunnel_state(p) else {
            return false;
        };
        let f = self.tunnel_flaps(p, axis, shape);
        let connected = self.w.dims[0]
            .brass
            .get(&p)
            .is_some_and(|b| b.connected_left || b.connected_right);
        !f.is_empty() && (connected || f.iter().any(|d| d.axis() != axis))
    }
    /// `BrassTunnelBlockEntity.canInsert`: whether an item entering from `side` is accepted by
    /// the tunnel's own filter and (when distributing) its held slot.
    pub(super) fn brass_can_insert(&self, p: Pos, side: Dir, item: &str) -> bool {
        let distributing = self.has_distribution(p);
        let filter_passes = self.brass_filter_passes(p, side, item);
        self.w.dims[0]
            .brass
            .get(&p)
            .is_some_and(|b| b.can_insert(distributing, filter_passes))
    }
    /// `can_take` (`canTakeItems`): no stack held, not handing out with its synchronized group.
    pub(super) fn brass_can_take(&self, p: Pos) -> bool {
        self.w.dims[0]
            .brass
            .get(&p)
            .is_some_and(tunnel::Brass::can_take)
    }
    /// `set_stack` (`setStackToDistribute`).
    pub(super) fn brass_set_stack(&mut self, p: Pos, item: &belt::Item, from: Option<Dir>) {
        let d = &mut self.w.dims[0];
        let Some(b) = d.brass.get_mut(&p) else {
            return;
        };
        b.stack = (item.count > 0).then(|| (item.item.clone(), item.count));
        b.stack_progress = item.progress.clone();
        b.entered_from = from;
        b.progress = -1;
        b.new_item = item.count > 0;
        self.w.mark_unsaved(0, p);
    }
    /// `flap_filter_passes` (`testFlapFilter`): its own filter on `side`, else anything (the
    /// driver places no brass funnel or smart chute beside a tunnel).
    pub(super) fn brass_filter_passes(&self, p: Pos, side: Dir, item: &str) -> bool {
        self.tunnel_filters
            .get(&(p, side))
            .is_none_or(|f| f == item)
    }
    fn brass_filter_empty(&self, p: Pos, side: Dir) -> bool {
        !self.tunnel_filters.contains_key(&(p, side))
    }
    /// `brass_group`: the brass tunnels beside `p` across its axis, itself first.
    fn brass_group(&self, p: Pos, axis: Axis) -> Vec<Pos> {
        let mut group = vec![p];
        for left in [true, false] {
            let mut at = p;
            while let Some(next) = self.brass_adjacent(at, axis, left) {
                at = next;
                group.push(at);
            }
        }
        group
    }
    /// `valid_outputs_of` (`addValidOutputsOf`).
    fn brass_outputs_of(&self, t: Pos, own: bool) -> Vec<(Pos, Dir)> {
        let below = [t[0], t[1] - 1, t[2]];
        let Some((c, _, seg)) = self.belt_location(below) else {
            return Vec::new();
        };
        let Some((axis, shape)) = self.tunnel_state(t) else {
            return Vec::new();
        };
        let speed = self.w.speed(0, c).unwrap_or(0.);
        let movement = belt::movement_facing(speed, seg.facing);
        let open = tunnel::sides(axis, shape);
        let mut out = Vec::new();
        for side_pass in [true, false] {
            if !own && side_pass {
                continue;
            }
            for d in [Dir::South, Dir::West, Dir::North, Dir::East] {
                if d == movement && speed == 0. {
                    continue;
                }
                if own && side_pass == (d.axis() == movement.axis()) {
                    continue;
                }
                if d == movement.opposite() || !open.contains(&d) {
                    continue;
                }
                let offset = add(below, d.offset());
                if self
                    .belt_funnel_at(add(offset, [0, 1, 0]))
                    .is_some_and(|(f, s)| s == create_engine::funnel::BeltShape::Pulling && f == d)
                {
                    continue;
                }
                if self.direct_input_at(offset) {
                    if self.direct_input_accepts(offset, d) {
                        out.push((t, d));
                    }
                    continue;
                }
                if d == movement && !self.sturdy(offset, d.opposite()) {
                    out.push((t, d));
                }
            }
        }
        out
    }
    /// `insert_into_tunnel` (`insertIntoTunnel`): None when side `d` of `t` takes none of it,
    /// else what is left.
    fn brass_insert(&mut self, t: Pos, d: Dir, item: &belt::Item, simulate: bool) -> Option<u8> {
        if !self.brass_filter_passes(t, d, &item.item) {
            return None;
        }
        let below = [t[0], t[1] - 1, t[2]];
        let (c, _, seg) = self.belt_location(below)?;
        let p = add(below, d.offset());
        if self.direct_input_at(p) {
            if !self.direct_input_accepts(p, d) {
                return None;
            }
            return Some(
                self.handle_insertion(p, d, item, false, simulate)
                    .unwrap_or(item.count),
            );
        }
        let speed = self.w.speed(0, c).unwrap_or(0.);
        if d != belt::movement_facing(speed, seg.facing) || self.sturdy(p, d.opposite()) {
            return None;
        }
        if !simulate {
            // Thrown off ahead of the segment: native item flight.
            self.notes.push(format!(
                "adapter gap: brass tunnel at {t:?} throws a stack off the belt"
            ));
        }
        Some(0)
    }
    /// `tick_brass`: a brass tunnel's turn in the transport walk; two ticks after a stack
    /// arrives (then every `brassTunnelTimer` while some is left), it shares it out among its
    /// group's outputs.
    pub(super) fn tick_brass(&mut self, t: Pos) {
        let Some(mut b) = self.w.dims[0].brass.get(&t).cloned() else {
            return;
        };
        let below = [t[0], t[1] - 1, t[2]];
        let rpm = self
            .belt_location(below)
            .and_then(|(c, ..)| self.w.speed(0, c))
            .unwrap_or(0.);
        let phase = b.begin_tick(rpm != 0.);
        if matches!(phase, tunnel::BrassPhase::Idle | tunnel::BrassPhase::Wait) {
            self.brass_save(t, &b);
            return;
        }
        let Some((axis, _)) = self.tunnel_state(t) else {
            return;
        };
        if phase == tunnel::BrassPhase::Gather {
            let group = self.brass_group(t, axis);
            let held: Vec<bool> = group
                .iter()
                .map(|p| {
                    self.w.dims[0]
                        .brass
                        .get(p)
                        .is_some_and(|x| x.stack.is_some())
                })
                .collect();
            let (sync_update, can_gather) = b.gather_sync(&held);
            if let Some(synced) = sync_update {
                let mut changed = Vec::new();
                let d = &mut self.w.dims[0];
                for p in &group {
                    if let Some(x) = d.brass.get_mut(p)
                        && x.synced != synced
                    {
                        x.synced = synced;
                        changed.push(*p);
                    }
                }
                for p in changed {
                    self.w.mark_unsaved(0, p);
                }
            }
            if !can_gather {
                self.brass_save(t, &b);
                return;
            }
            let Some((item, count)) = b.stack.clone() else {
                self.brass_save(t, &b);
                return;
            };
            let mut one = belt::Item::new(&item, count, 180, false);
            one.progress = b.stack_progress.clone();
            let outputs: Vec<(Pos, Dir)> = group
                .iter()
                .enumerate()
                .flat_map(|(i, g)| self.brass_outputs_of(*g, i == 0))
                .collect();
            let any = outputs
                .into_iter()
                .any(|(g, d)| self.brass_insert(g, d, &one, true).is_some());
            if !any {
                self.brass_save(t, &b);
                return;
            }
            b.schedule_distribution();
            self.brass_save(t, &b);
            return;
        }
        let Some((item, count)) = b.stack.clone() else {
            b.progress = -1;
            self.brass_save(t, &b);
            return;
        };
        let group = self.brass_group(t, axis);
        let mut one = belt::Item::new(&item, count, 180, false);
        one.progress = b.stack_progress.clone();
        let mut filtered = Vec::new();
        let mut plain = Vec::new();
        for (i, g) in group.iter().enumerate() {
            for (g2, d) in self.brass_outputs_of(*g, i == 0) {
                if g2 == t && Some(d) == b.entered_from {
                    continue;
                }
                if self.brass_insert(g2, d, &one, true).is_none() {
                    continue;
                }
                if self.brass_filter_empty(g2, d) {
                    plain.push((g2, d));
                } else {
                    filtered.push((g2, d));
                }
            }
        }
        let mut left = count;
        for outputs in [filtered, plain] {
            if left == 0 || outputs.is_empty() {
                continue;
            }
            let random = self.brass_random(t);
            let dealt = tunnel::distribute(
                left,
                outputs.len(),
                b.previous_output,
                b.mode,
                random,
                |i, n, simulate| {
                    let (g, d) = outputs[i];
                    let mut part = one.clone();
                    part.count = n;
                    self.brass_insert(g, d, &part, simulate)
                },
            );
            if let Some(l) = dealt {
                left = l;
                b.previous_output = (b.previous_output + 1) % outputs.len().max(1);
            }
        }
        b.stack = (left > 0).then(|| (item.clone(), left));
        if b.stack.is_none() {
            b.entered_from = None;
            b.stack_progress = None;
        }
        b.progress = -1;
        self.brass_save(t, &b);
        // `set_slot`: the block entity's slot changes with what is left (the host saves
        // its column again).
        if left != count {
            self.w.mark_unsaved(0, t);
        }
    }
    /// The glue's `save` closure in `tick_brass`.
    fn brass_save(&mut self, t: Pos, b: &tunnel::Brass) {
        let d = &mut self.w.dims[0];
        let changed = d.brass.get(&t).is_some_and(|x| x != b);
        if let Some(x) = d.brass.get_mut(&t) {
            *x = b.clone();
        }
        if changed {
            self.w.mark_unsaved(0, t);
        }
    }
    /// `items::random_angle` at `t`: Java's `level.random` is not reproduced; the generator
    /// never randomizes over more than one output.
    fn brass_random(&self, t: Pos) -> usize {
        let mut x = u64::from(self.now)
            ^ (t[0] as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (t[2] as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
            ^ (t[1] as u64) << 40;
        x ^= x >> 31;
        x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x ^= x >> 29;
        (x % 360) as usize
    }
    /// `set_brass_mode` (`selectionMode.withCallback`): the tunnel at `p` and, when connected,
    /// its group take mode `mode`.
    fn set_brass_mode(&mut self, p: Pos, mode: tunnel::Mode) {
        let brass_info = self.collect_brass_info(&[p]);
        let mut changed = Vec::new();
        tunnel::set_group_mode(
            p,
            mode,
            &mut self.w.dims[0].brass,
            &|q| brass_info.get(&q).copied(),
            &mut changed,
        );
        for q in changed {
            self.w.mark_unsaved(0, q);
        }
    }
    /// A value board pick on a brass tunnel (interaction/values.rs): `option_value` against its
    /// current mode.
    pub(super) fn brass_option(&mut self, p: Pos, pick: create_engine::board::Settings) {
        let Some(now) = self.w.dims[0].brass.get(&p).map(|b| b.mode.index() as i32) else {
            return;
        };
        if let Some(mode) =
            create_engine::board::option_value(pick, now, tunnel::OPTIONS.len() as i32)
                .and_then(tunnel::Mode::from_option)
        {
            self.set_brass_mode(p, mode);
        }
    }
    /// A use on a brass tunnel the glue handles before the block's: a click on a side filter
    /// slot (`use_tunnel_filter`), not sneaking, with neither a wrench nor an arm, sets that
    /// side's filter to the held item (an empty hand clears it). True when it was the slot's.
    pub(super) fn brass_filter_use(
        &mut self,
        p: Pos,
        item: &str,
        face: Dir,
        hit: [f64; 3],
        sneaking: bool,
    ) -> bool {
        let Some((axis, shape)) = self.tunnel_state(p) else {
            return false;
        };
        if self.brass_axis(p).is_none() || sneaking {
            return false;
        }
        if matches!(item, "create:wrench" | "create:mechanical_arm") {
            return false;
        }
        let Some(slot) = tunnel::filter_slot(axis, shape, face) else {
            return false;
        };
        if !create_engine::board::hits_box(slot, hit) {
            return false;
        }
        if item == "minecraft:air" {
            self.tunnel_filters.remove(&(p, face));
        } else {
            self.tunnel_filters.insert((p, face), item.to_string());
        }
        self.w.mark_unsaved(0, p);
        true
    }
    /// `grab_stacks` (`BrassTunnelBlock.useWithoutItem`): an empty hand takes the stacks of the
    /// tunnel at `p` and its group (into the player's inventory). True when there were any.
    pub(super) fn brass_grab(&mut self, p: Pos) -> bool {
        let Some(axis) = self.brass_axis(p) else {
            return false;
        };
        let mut any = false;
        for t in self.brass_group(p, axis) {
            if self.w.dims[0]
                .brass
                .get(&t)
                .is_none_or(|b| b.stack.is_none())
            {
                continue;
            }
            // Into the player's inventory: a `use` action measures no outcome.
            any = true;
            let empty = belt::Item::new("minecraft:air", 0, 0, false);
            self.brass_set_stack(t, &empty, None);
        }
        any
    }
    /// `TunnelItems` (`handlers::provide`): one slot, inserting into its stack while
    /// distributing (`hasDistributionBehaviour`, `canTakeItems`), else delegating to the belt
    /// below; extraction always delegates to the belt below.
    pub(super) fn brass_capability(
        &mut self,
        p: Pos,
        slot: u8,
        insert: Option<&crate::transport::Stack>,
        extract: Option<u8>,
        simulate: bool,
        outcome: &mut crate::transport::Outcome,
    ) {
        outcome.handler = Some(true);
        let below = [p[0], p[1] - 1, p[2]];
        if let Some(s) = insert {
            let distributing = self.has_distribution(p);
            let target = self.w.dims[0]
                .brass
                .get(&p)
                .map(|b| b.handler_target(distributing));
            match target {
                Some(tunnel::HandlerTarget::BeltBelow) => {
                    self.belt_capability(below, slot, Some(s), None, simulate, outcome);
                }
                Some(tunnel::HandlerTarget::Distribute) if slot == 0 => {
                    let n = s.count.min(s.limit());
                    if !simulate {
                        let item = belt::Item::new(&s.item, n, 180, false);
                        // `slot_changed`: a stack put by another block comes in by no side.
                        self.brass_set_stack(p, &item, None);
                    }
                    outcome.remainder = (s.count > n).then(|| crate::transport::Stack {
                        item: s.item.clone(),
                        count: s.count - n,
                    });
                }
                _ => {}
            }
        } else if extract.is_some() {
            self.belt_capability(below, slot, None, extract, simulate, outcome);
        }
    }
    pub(super) fn brass_observation(&self, p: Pos) -> Option<crate::logistics::Brass> {
        self.brass_axis(p)?;
        let b = self.w.dims[0].brass.get(&p)?;
        let filters = Dir::ALL
            .iter()
            .filter_map(|d| {
                let f = self.tunnel_filters.get(&(p, *d))?;
                Some((format!("{d:?}").to_lowercase(), f.clone()))
            })
            .collect();
        Some(crate::logistics::Brass {
            stack: b
                .stack
                .clone()
                .map(|(item, count)| crate::transport::Stack { item, count }),
            entered_from: b.entered_from.map(|d| format!("{d:?}").to_lowercase()),
            progress: (b.stack.is_some() || b.synced).then_some(b.progress as f32),
            previous: b.previous_output as i32,
            synced: b.synced,
            connected_left: b.connected_left,
            connected_right: b.connected_right,
            mode: match b.mode {
                tunnel::Mode::Split => "split",
                tunnel::Mode::ForcedSplit => "forced_split",
                tunnel::Mode::RoundRobin => "round_robin",
                tunnel::Mode::ForcedRoundRobin => "forced_round_robin",
                tunnel::Mode::PreferNearest => "prefer_nearest",
                tunnel::Mode::Randomize => "randomize",
                tunnel::Mode::Synchronize => "synchronize",
            }
            .into(),
            filters,
        })
    }
}
