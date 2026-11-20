//! Chutes and smart chutes at belts, mirroring glue/logistics/chute.rs: a belt's end hands a
//! stack into an empty chute (`ChuteBlockEntity`'s `DirectBeltInputBehaviour`, refused under a
//! chute and while fans blow up a chute with none below it); it falls (`tick`) at the column's
//! motion (`chute_motion`: gravity, and encased fans under the lowest chute blowing up or over
//! the highest blowing down) and leaves at the bottom into the belt segment below
//! (`ItemHandlerBeltSegment`), a depot, or the chute it leads into (a diagonal chute's beside
//! the one below), or out of the bottom (`handleDownwardOutput`, native item flight); blown up,
//! it rises into the chute or depot above, or out of the top (native item flight). A smart chute
//! takes only what its filter passes and holds its stack in the middle while powered
//! (`canActivate`). Diagonal chutes turn down or become intersections as the chutes around them
//! change (`tick_chute_shapes`). Chutes taking from an inventory above or lifting from below
//! (`handleInputFromAbove`, `findEntities`) are outside this adapter.
use super::*;
use crate::transport::Stack;
use create_engine::{
    Block, belt,
    chute::{self, Fall, Shape},
    kinetics::Dir,
};

impl Run {
    /// `chute_facing`: the way the chute at `p` leads (down, or a diagonal chute's facing; a
    /// smart chute's down); None when no chute is there.
    pub(super) fn chute_at(&self, p: Pos) -> Option<Dir> {
        let (block, info) = self.w.table.get(self.at(p))?;
        match block {
            Block::Chute => Some(info.facing.unwrap_or(Dir::Down)),
            Block::SmartChute => Some(Dir::Down),
            _ => None,
        }
    }
    /// `chute_at` of the glue: the chute or smart chute at `p`, with whether it is powered.
    fn chute_block(&self, p: Pos) -> Option<(Block, bool)> {
        let (block, info) = self.w.table.get(self.at(p))?;
        matches!(block, Block::Chute | Block::SmartChute).then_some((block, info.powered))
    }
    /// `chute_accepts` (`canAcceptItem`): a smart chute not while powered, and only what its
    /// filter passes (the slot shared with brass funnels: `chute_filters`).
    fn chute_accepts(&self, p: Pos, block: Block, powered: bool, item: &str) -> bool {
        match block {
            Block::Chute => true,
            Block::SmartChute => !powered && self.funnel_passes(p, item),
            _ => false,
        }
    }
    /// The speed of an encased fan at `p` blowing `way`, if one is there.
    fn fan_blowing(&self, p: Pos, way: Dir) -> Option<f32> {
        let (block, info) = self.w.table.get(self.at(p))?;
        if block != Block::EncasedFan || info.facing != Some(way) {
            return None;
        }
        self.w.speed(0, p)
    }
    /// `getInputChutes`: the chute above facing down, and the diagonal chutes above beside it
    /// facing toward it.
    fn input_chutes(&self, p: Pos) -> Vec<Pos> {
        [Dir::Down, Dir::North, Dir::South, Dir::West, Dir::East]
            .into_iter()
            .filter_map(|f| {
                let o = if f == Dir::Down { [0; 3] } else { f.offset() };
                let q = [p[0] + o[0], p[1] + 1, p[2] + o[2]];
                (self.chute_at(q) == Some(f)).then_some(q)
            })
            .collect()
    }
    /// `calculatePull`: a fan over the chute blowing down, else what its inputs pull.
    fn chute_pull(&self, p: Pos, depth: u32) -> f32 {
        if let Some(speed) = self.fan_blowing([p[0], p[1] + 1, p[2]], Dir::Down) {
            return speed;
        }
        if depth > 256 {
            return 0.;
        }
        self.input_chutes(p)
            .into_iter()
            .map(|q| self.chute_pull(q, depth + 1))
            .sum()
    }
    /// `calculatePush`: a fan under the chute blowing up, else the push of the chute it leads
    /// into, shared among that chute's inputs.
    fn chute_push(&self, p: Pos, depth: u32) -> f32 {
        if let Some(speed) = self.fan_blowing([p[0], p[1] - 1, p[2]], Dir::Up) {
            return speed;
        }
        let Some(facing) = self.chute_at(p) else {
            return 0.;
        };
        let target = chute::target(p, facing);
        if depth > 256 || self.chute_at(target).is_none() {
            return 0.;
        }
        let branches = self.input_chutes(target).len().max(1) as f32;
        self.chute_push(target, depth + 1) / branches
    }
    /// `getItemMotion`, in chutes a tick.
    fn chute_motion(&self, p: Pos) -> f32 {
        chute::item_motion(self.chute_push(p, 0), self.chute_pull(p, 0))
    }
    /// `canDirectlyInsert` as the glue reads it (`direct_input_accepts`): no chute above, and not
    /// blown up without a chute below.
    pub(super) fn chute_takes_from_side(&self, p: Pos) -> bool {
        self.chute_at([p[0], p[1] + 1, p[2]]).is_none()
            && !(self.chute_motion(p) > 0. && self.chute_at([p[0], p[1] - 1, p[2]]).is_none())
    }
    /// `chute_direct_input` and `chute_take`: the whole stack, at the top, while it is empty and
    /// takes it.
    pub(super) fn chute_direct_input(&mut self, p: Pos, item: &belt::Item) -> u8 {
        let Some((block, powered)) = self.chute_block(p) else {
            return item.count;
        };
        if !self.chute_takes_from_side(p)
            || !self.chute_accepts(p, block, powered, &item.item)
            || self.chute_stacks.contains_key(&p)
        {
            return item.count;
        }
        self.chute_stacks.insert(
            p,
            Stack {
                item: item.item.clone(),
                count: item.count,
            },
        );
        self.w.dims[0].chutes.insert(p, 1.);
        0
    }
    /// `tick_chute_shapes` (`updateChuteState`): diagonal chutes with nothing to lead into turn
    /// down; intersections come and go with the diagonal chutes leading in.
    pub(super) fn tick_chute_shapes(&mut self) {
        let mut checks = std::mem::take(&mut self.w.dims[0].chute_shapes);
        checks.sort();
        checks.dedup();
        for pos in checks {
            let Some((Block::Chute, info)) = self.w.table.get(self.at(pos)).map(|(b, i)| (b, *i))
            else {
                continue;
            };
            let state = self.name(self.at(pos));
            let (_, props) = crate::scenario::parse_state(&state);
            let shape = props
                .iter()
                .find(|(k, _)| k == "shape")
                .and_then(|(_, v)| Shape::read(v))
                .unwrap_or(Shape::Normal);
            let facing = info.facing.unwrap_or(Dir::Down);
            let funnel_down = self
                .funnel_at([pos[0], pos[1] + 1, pos[2]])
                .is_some_and(|(b, i)| {
                    matches!(b, Block::AndesiteFunnel | Block::BrassFunnel)
                        && i.facing == Some(Dir::Down)
                });
            let (f2, s2) =
                chute::update_state(pos, facing, shape, |q| self.chute_at(q), funnel_down);
            if (f2, s2) == (facing, shape) {
                continue;
            }
            let next = crate::scenario::with_prop(
                &crate::scenario::with_prop(&state, "facing", &format!("{f2:?}").to_lowercase()),
                "shape",
                s2.name(),
            );
            self.set(pos, self.id(&next));
        }
    }
    /// `Level.blockEntityTickers`, each chute at its own turn
    /// (`ChuteBlockEntity.tick`): a stack a chute hands down during its turn moves on in the
    /// same level tick when the chute below is registered after it.
    fn chute_turns(&self) -> create_engine::world::ChuteTicks {
        self.w.chute_ticks(0)
    }
    /// The next chute in that order that holds a moving stack, with its position.
    fn next_chute_turn(&self, turns: &mut create_engine::world::ChuteTicks) -> Option<(Pos, f32)> {
        loop {
            let pos = turns.next(&self.w)?;
            if let Some(position) = self.w.dims[0].chutes.get(&pos) {
                return Some((pos, *position));
            }
        }
    }
    /// `tick_chutes`: each chute holding a stack moves it at its column's motion, falling or
    /// rising, and hands it on at the end of its way.
    pub(super) fn tick_chutes(&mut self) {
        self.tick_chute_shapes();
        let mut turns = self.chute_turns();
        while let Some((pos, position)) = self.next_chute_turn(&mut turns) {
            let Some(stack) = self.chute_stacks.get(&pos).cloned() else {
                self.w.dims[0].chutes.remove(&pos);
                continue;
            };
            let active = !matches!(self.chute_block(pos), Some((_, true)));
            if !active && position <= 0.5 {
                // A powered smart chute holds its stack in the middle; it ticks again once its
                // `powered` changes.
                self.w.dims[0].chutes.remove(&pos);
                continue;
            }
            let motion = self.chute_motion(pos);
            let next = if motion > 0. {
                self.chute_rise(pos, position, motion, stack, active)
            } else {
                self.chute_fall(pos, position, motion, stack, active)
            };
            match next {
                Some(p) => {
                    self.w.dims[0].chutes.insert(pos, p);
                }
                None => {
                    self.w.dims[0].chutes.remove(&pos);
                }
            }
        }
        // Entries whose chute block is gone (removed while it held a stack) leave with their
        // pass.
        let stale: Vec<Pos> = self.w.dims[0]
            .chutes
            .keys()
            .copied()
            .filter(|p| self.chute_at(*p).is_none())
            .collect();
        for p in stale {
            self.w.dims[0].chutes.remove(&p);
        }
    }
    /// `handleDownwardOutput`: the inventory below first; then the chute it leads into (a
    /// diagonal chute's beside that); a diagonal chute never drops what it holds.
    fn chute_fall(
        &mut self,
        pos: Pos,
        position: f32,
        motion: f32,
        stack: Stack,
        active: bool,
    ) -> Option<f32> {
        let facing = self.chute_at(pos).unwrap_or(Dir::Down);
        let diagonal = facing != Dir::Down;
        let below = [pos[0], pos[1] - 1, pos[2]];
        let target = chute::target(pos, facing);
        let target_chute = self.chute_block(target);
        let chute_below = self.chute_at(below).is_some();
        let belt_below = !chute_below
            && self
                .belt_location(below)
                .is_some_and(|(_, _, s)| s.transports());
        let block_below = self.w.table.get(self.at(below)).map(|(b, _)| b);
        let depot_below = !chute_below && block_below == Some(Block::Depot);
        if !chute_below && block_below == Some(Block::WeightedEjector) {
            self.notes
                .push(format!("adapter gap: chute into the ejector at {below:?}"));
        }
        let container = belt_below || depot_below;
        let target_takes = |run: &Run| {
            target_chute.is_some_and(|(b, powered)| {
                !run.chute_stacks.contains_key(&target)
                    && run.chute_accepts(target, b, powered, &stack.item)
            })
        };
        let drops = |run: &Run| {
            let funnel_down = run.funnel_at(below).is_some_and(|(b, i)| {
                matches!(b, Block::AndesiteFunnel | Block::BrassFunnel)
                    && i.facing == Some(Dir::Down)
            });
            // `falls_through`: unless the block below can hold a rigid block up.
            !funnel_down && !run.sturdy(below, Dir::Up)
        };
        // `handleDownwardOutput(true)` past the middle asks, by a simulated
        // `insertItemStacked` into the inventory below, whether it takes any of the stack; a
        // straight chute over one that refuses waits at 0.5, a diagonal one asks its target
        // (`chute::can_output_down`).
        let can = active && {
            let inventory = container.then(|| {
                if belt_below {
                    let (c, index, _) = self.belt_location(below).unwrap();
                    let rpm = self.w.speed(0, c).unwrap_or(0.);
                    self.w.dims[0].belts.get(&c).is_some_and(|b| {
                        b.items.can_insert_at_from_side(
                            index,
                            Dir::Up,
                            belt::movement_facing(rpm, b.segment.facing),
                        )
                    })
                } else {
                    self.depot_insert(below, &stack, true)
                        .is_none_or(|left| left.count != stack.count)
                }
            });
            let target = target_chute.map(|_| target_takes(self));
            create_engine::chute::can_output_down(diagonal, inventory, target, || drops(self))
        };
        let into_target = |run: &mut Run, stack: Stack| {
            run.chute_stacks.remove(&pos);
            run.chute_stacks.insert(target, stack);
            let at = if diagonal { 0.51 } else { 1. };
            run.w.dims[0].chutes.insert(target, at);
        };
        match chute::step(position, motion, can) {
            Fall::At(p) => Some(p),
            Fall::Out if container => {
                let left = if belt_below {
                    self.put_on_from_above(below, &stack)
                        .map_or(Some(stack.clone()), |l| {
                            (l > 0).then(|| Stack {
                                item: stack.item.clone(),
                                count: l,
                            })
                        })
                } else {
                    self.depot_insert(below, &stack, false)
                };
                match left {
                    Some(l) if l.count == stack.count => {
                        if diagonal && target_takes(self) {
                            into_target(self, stack);
                            None
                        } else {
                            Some(0.5)
                        }
                    }
                    Some(l) => {
                        self.chute_stacks.insert(pos, l);
                        Some(0.5)
                    }
                    None => {
                        self.chute_stacks.remove(&pos);
                        None
                    }
                }
            }
            Fall::Out if target_chute.is_some() => {
                into_target(self, stack);
                None
            }
            Fall::Out => {
                self.chute_stacks.remove(&pos);
                None
            }
        }
    }
    /// `rise` (`handleUpwardOutput`): into the chute above if it is empty, takes the stack and
    /// does not fall, else the depot above, else out of the top when nothing solid is over it
    /// (native item flight); it waits in the middle while the way is shut.
    fn chute_rise(
        &mut self,
        pos: Pos,
        position: f32,
        motion: f32,
        stack: Stack,
        active: bool,
    ) -> Option<f32> {
        let above = [pos[0], pos[1] + 1, pos[2]];
        let above_chute = self.chute_block(above);
        let block_above = self.w.table.get(self.at(above)).map(|(b, _)| b);
        let container = above_chute.is_none() && block_above == Some(Block::Depot);
        if above_chute.is_none() && block_above == Some(Block::WeightedEjector) {
            self.notes
                .push(format!("adapter gap: chute into the ejector at {above:?}"));
        }
        let can = if !active {
            false
        } else if let Some((b, powered)) = above_chute {
            !self.chute_stacks.contains_key(&above)
                && self.chute_accepts(above, b, powered, &stack.item)
                && self.chute_motion(above) >= 0.
        } else if container {
            true
        } else {
            let funnel_up = self
                .funnel_at(above)
                .is_some_and(|(_, i)| i.facing == Some(Dir::Up));
            // `falls_through` upward: unless its bottom is a full solid side.
            !funnel_up && !self.sturdy(above, Dir::Down)
        };
        match chute::step(position, motion, can) {
            Fall::At(p) => Some(p),
            Fall::Out if above_chute.is_some() => {
                self.chute_stacks.remove(&pos);
                self.chute_stacks.insert(above, stack);
                self.w.dims[0].chutes.insert(above, 0.);
                None
            }
            Fall::Out if container => match self.depot_insert(above, &stack, false) {
                Some(l) if l.count == stack.count => Some(0.5),
                Some(l) => {
                    self.chute_stacks.insert(pos, l);
                    Some(0.5)
                }
                None => {
                    self.chute_stacks.remove(&pos);
                    None
                }
            },
            Fall::Out => {
                self.notes.push(format!(
                    "adapter gap: chute at {pos:?} blows its stack out of the top (native item flight)"
                ));
                self.chute_stacks.remove(&pos);
                None
            }
        }
    }
    /// `use_smart_chute`: a click on a smart chute's filter slot (a horizontal side), not
    /// sneaking, with neither a wrench nor an arm, sets its filter from the held item unless its
    /// amount board shows (it extracts: the block its stacks come from is neither a chute nor
    /// replaceable; the client then waits for the release). True when it was the slot's.
    pub(super) fn smart_chute_filter_use(
        &mut self,
        p: Pos,
        item: &str,
        face: Dir,
        hit: [f64; 3],
        sneaking: bool,
    ) -> bool {
        if self.chute_block(p).map(|(b, _)| b) != Some(Block::SmartChute) || sneaking {
            return false;
        }
        let Some(slot) = chute::filter_slot(face) else {
            return false;
        };
        if !create_engine::board::hits_box(slot, hit)
            || matches!(item, "create:wrench" | "create:mechanical_arm")
        {
            return false;
        }
        let up = self.chute_motion(p) < 0.;
        let from = [p[0], p[1] + if up { 1 } else { -1 }, p[2]];
        let extracting = self.chute_at(from).is_none() && !self.replaceable(from);
        let max = self.funnel_filter_max_of(item);
        if extracting && max > 1 {
            return true;
        }
        self.set_funnel_filter(p, item);
        true
    }
    pub(super) fn chute_observation(&self, p: Pos) -> Option<crate::logistics::Chute> {
        self.chute_at(p)?;
        let item = self.chute_stacks.get(&p).cloned();
        // A chute's position is only observed while it holds a stack.
        let position = if item.is_some() {
            self.w.dims[0].chutes.get(&p).copied().unwrap_or(0.5)
        } else {
            0.
        };
        Some(crate::logistics::Chute { item, position })
    }
}
