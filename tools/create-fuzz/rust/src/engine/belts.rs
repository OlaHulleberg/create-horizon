//! Belt item interactions and committed break effects, mirroring logistics/belt and items/belt_connector.
use super::*;
use create_engine::{
    Block, belt, belt_connector as bc,
    kinetics::{Axis, Dir},
};

impl bc::Ground for Run {
    fn loaded(&self, _: Pos) -> bool {
        true
    }
    fn shaft(&self, p: Pos) -> Option<Axis> {
        self.w
            .table
            .get(self.at(p))
            .and_then(|(b, i)| (b == Block::Shaft).then_some(i.axis).flatten())
    }
    fn speed(&self, p: Pos) -> f32 {
        self.w.speed(0, p).unwrap_or(0.)
    }
    fn replaceable(&self, p: Pos) -> bool {
        self.at(p) == AIR
    }
}
impl Run {
    pub(super) fn belt_use(
        &mut self,
        p: Pos,
        item: &str,
        face: Dir,
        hit: [f64; 3],
        sneaking: bool,
    ) -> bool {
        let seg = self.w.table.get(self.at(p)).and_then(|(_, i)| i.belt);
        if let Some(seg) = seg {
            if sneaking {
                return false;
            }
            if (item == bc::ITEM || item == "create:wrench")
                && self.slice_belt(p, item, face, hit.map(|v| v - 0.5), seg)
            {
                return true;
            }
            // `applyColor`: a dye colours the whole belt, a water bucket washes it; neither is
            // used up. Unchanged, the click passes on.
            let dye = create_engine::dye::DyeColor::of_item(item);
            if (dye.is_some() || item == "minecraft:water_bucket")
                && self.apply_belt_color(p, dye.map(|d| d.name()))
            {
                return true;
            }
            if item == "create:shaft" && seg.part == belt::Part::Middle {
                let s = with_prop(&self.name(self.at(p)), "part", "pulley");
                self.switch(p, self.id(&s));
                return true;
            }
            if item == "create:andesite_casing" || item == "create:brass_casing" {
                self.set_belt_casing(
                    p,
                    Some(if item.contains("brass") {
                        "brass"
                    } else {
                        "andesite"
                    }),
                );
                return true;
            }
            if item == "create:wrench" && face != Dir::Up && self.belt_cased(p) {
                self.set_belt_casing(p, None);
                return true;
            }
            if item == "create:wrench" && seg.part == belt::Part::Pulley && face.axis() != Axis::Y {
                let s = with_prop(&self.name(self.at(p)), "part", "middle");
                self.switch(p, self.id(&s));
                return true;
            }
        }
        if item != bc::ITEM {
            return false;
        }
        if self.now < self.connector_ready {
            return true;
        }
        match bc::click(self, sneaking, self.connector, p, true) {
            bc::Click::Forget | bc::Click::Fail { forget: true } => self.connector = None,
            bc::Click::Remember => {
                self.connector = Some(p);
                self.connector_ready = self.now + 5;
            }
            bc::Click::Connect(start) => {
                let axis = bc::Ground::shaft(self, start).unwrap();
                let layout = bc::layout(start, p, axis, |p| bc::Ground::shaft(self, p));
                for (p, seg) in layout {
                    if self.at(p) != AIR {
                        self.set(p, AIR);
                    }
                    let s = format!(
                        "create:belt[facing={},slope={},part={},casing=false,waterlogged=false]",
                        format!("{:?}", seg.facing).to_lowercase(),
                        bc::slope_name(seg.slope),
                        bc::part_name(seg.part)
                    );
                    self.switch(p, self.id(&s));
                }
                self.connector = None;
                self.connector_ready = self.now + 5;
            }
            _ => {}
        }
        true
    }
    pub(super) fn belt_breaks(&mut self) {
        belt::break_cascade(self);
    }
}
impl Run {
    /// The segment at `p` is cased (its `casing` property).
    pub(super) fn belt_cased(&self, p: Pos) -> bool {
        self.w
            .table
            .get(self.at(p))
            .is_some_and(|(b, _)| b == Block::Belt)
            && parse_state(&self.name(self.at(p)))
                .1
                .iter()
                .any(|(k, v)| k == "casing" && v == "true")
    }
    /// `set_casing`: the segment at `p` takes casing `kind` (None: its casing comes off), then
    /// its cover is looked at (`updateCoverProperty`).
    pub(super) fn set_belt_casing(&mut self, p: Pos, kind: Option<&str>) {
        match kind {
            Some(k) => {
                self.belt_casing.insert(p, k.into());
            }
            None => {
                self.belt_casing.remove(&p);
            }
        }
        let s = with_prop(
            &self.name(self.at(p)),
            "casing",
            if kind.is_some() { "true" } else { "false" },
        );
        self.switch(p, self.id(&s));
        self.update_cover(p);
    }
    /// `update_cover` (`BeltBlock.updateCoverProperty`): a cased horizontal segment is covered
    /// while a block whose collision box is at least half a block wide each way and starts at
    /// its bottom stands on it, but for funnels not facing up and tunnels.
    pub(super) fn update_cover(&mut self, p: Pos) {
        let Some(seg) = self.segment(p) else {
            return;
        };
        if !self.belt_cased(p) || seg.slope != belt::Slope::Horizontal {
            return;
        }
        let above = [p[0], p[1] + 1, p[2]];
        let (name, _) = parse_state(&self.name(self.at(above)));
        let excluded = match self.w.table.get(self.at(above)) {
            Some((b, i)) => {
                matches!(b, Block::AndesiteTunnel | Block::BrassTunnel)
                    || matches!(
                        b,
                        Block::AndesiteFunnel
                            | Block::BrassFunnel
                            | Block::AndesiteBeltFunnel
                            | Block::BrassBeltFunnel
                    ) && i.facing != Some(Dir::Up)
            }
            None => false,
        };
        // The covering blocks the grammar places: full cubes and depots cover; air, shafts and
        // fluids do not.
        let covers = match name.as_str() {
            "minecraft:stone"
            | "minecraft:redstone_block"
            | "create:andesite_casing"
            | "create:brass_casing"
            | "create:depot" => true,
            _ => self
                .w
                .table
                .get(self.at(above))
                .is_some_and(|(b, _)| matches!(b, Block::AndesiteFunnel | Block::BrassFunnel)),
        };
        let covered = !excluded && covers;
        if covered {
            self.belt_covered.insert(p);
        } else {
            self.belt_covered.remove(&p);
        }
    }
    /// `apply_color`: every segment of the belt at `p` takes `color` (None: washed). False
    /// when the clicked segment already has it.
    pub(super) fn apply_belt_color(&mut self, p: Pos, color: Option<&str>) -> bool {
        if self.belt_color.get(&p).map(String::as_str) == color {
            return false;
        }
        let Some(chain) = self.w.dims[0]
            .belt_of
            .get(&p)
            .and_then(|c| self.w.dims[0].belts.get(c))
            .map(|b| b.chain.clone())
        else {
            return false;
        };
        for q in chain {
            self.set_segment_color(q, color);
        }
        true
    }
    pub(super) fn set_segment_color(&mut self, p: Pos, color: Option<&str>) {
        match color {
            Some(c) => {
                self.belt_color.insert(p, c.into());
            }
            None => {
                self.belt_color.remove(&p);
            }
        }
    }
    pub(super) fn belt_observation(&self, p: Pos) -> Option<crate::belts::Observation> {
        let seg = self.w.table.get(self.at(p))?.1.belt?;
        let _ = seg;
        let c = self.w.dims[0].belt_of.get(&p).copied().unwrap_or(p);
        let b = self.w.dims[0].belts.get(&c);
        Some(crate::belts::Observation {
            controller: c,
            index: b
                .and_then(|b| b.chain.iter().position(|q| *q == p))
                .unwrap_or(0) as i32,
            length: b.map_or(0, |b| b.chain.len() as i32),
            casing: self.belt_casing.get(&p).cloned().unwrap_or("none".into()),
            covered: self.belt_covered.contains(&p),
            color: self.belt_color.get(&p).cloned(),
        })
    }
}
impl Run {
    fn segment(&self, p: Pos) -> Option<belt::Segment> {
        self.w.table.get(self.at(p))?.1.belt
    }
    fn segment_state(&self, seg: belt::Segment, cased: bool) -> u32 {
        let s = format!(
            "create:belt[facing={},slope={},part={},casing={cased},waterlogged=false]",
            format!("{:?}", seg.facing).to_lowercase(),
            seg.slope.name(),
            seg.part.name()
        );
        self.id(&s)
    }
    fn set_segment(&mut self, p: Pos, seg: belt::Segment, cased: bool) {
        let id = self.segment_state(seg, cased);
        self.switch(p, id);
    }
    fn set_segment_direct(&mut self, p: Pos, seg: belt::Segment, cased: bool) {
        let id = self.segment_state(seg, cased);
        self.set(p, id);
    }
    /// `BeltSlicer.useWrench`/`useConnector` as the glue's `logistics/slicer.rs` does them:
    /// true when the slicer answered the click.
    fn slice_belt(
        &mut self,
        p: Pos,
        item: &str,
        face: Dir,
        hit: [f64; 3],
        seg: belt::Segment,
    ) -> bool {
        let Some(c) = self.w.dims[0].belt_of.get(&p).copied() else {
            return false;
        };
        let length = self.w.dims[0].belts[&c].chain.len() as i32;
        if item == "create:wrench" {
            if self.belt_casing.contains_key(&p) && face != Dir::Up
                || seg.part == belt::Part::Pulley && face.axis() != Axis::Y
            {
                return false;
            }
            if length != 2 {
                self.slice_wrench(p, seg, length, hit);
            }
            return true;
        }
        if length == bc::MAX_LENGTH {
            return true;
        }
        if !seg.hovering_end(hit) {
            return false;
        }
        self.slice_connector(p, seg, length)
    }
    /// `use_wrench` past its pass-through checks: shorten from the hovered end, or split.
    fn slice_wrench(&mut self, p: Pos, seg: belt::Segment, length: i32, hit: [f64; 3]) {
        let v = seg.vector();
        let index = self.w.dims[0].belts[&self.w.dims[0].belt_of[&p]]
            .chain
            .iter()
            .position(|q| *q == p)
            .unwrap() as i32;
        if seg.hovering_end(hit) {
            // `BeltSlicer.java:86-93`: `detachKinetics`, `invalidateItemHandler` and
            // `beltLength = 0` on every segment (`take_belt`), as the glue's `use_wrench`.
            let (_, mut taken) = self.w.take_belt(0, p).unwrap();
            let next = add(
                p,
                if seg.part == belt::Part::End {
                    v.map(|x| -x)
                } else {
                    v
                },
            );
            self.set_segment(next, seg, self.belt_casing.contains_key(&next));
            self.w.remove_belt_quietly(0, p);
            // `BeltSlicer.java:97`'s air write (`UPDATE_MOVE_BY_PISTON`) still runs
            // `BeltBlockEntity.destroy` (`:194-198`): the clicked START is the controller, so
            // `getInventory().ejectAll()` empties it before the transfer.
            if seg.part == belt::Part::Start {
                self.w.destroy_belt(0, &mut taken);
            }
            self.set(p, AIR);
            let chain: Vec<_> = taken.chain.iter().copied().filter(|q| *q != p).collect();
            if seg.part == belt::Part::End {
                // `BeltSlicer.java:106-113`: items past the new end are ejected (native item
                // flight is outside the comparison); the controller keeps its inventory.
                taken
                    .items
                    .items
                    .retain(|i| i.position <= (length - 1) as f32);
                self.w.reform_belt(0, chain[0], &chain, taken.items);
            } else {
                // `BeltSlicer.java:116-132`: the next segment is the controller now, with a
                // fresh `BeltInventory` (`beltMovementPositive` false).
                self.w
                    .reform_belt(0, chain[0], &chain, belt::Inventory::fresh(Vec::new()));
            }
            return;
        }
        let toward_positive = (0..3).map(|i| hit[i] * f64::from(v[i])).sum::<f64>() > 0.;
        let next = add(p, if toward_positive { v } else { v.map(|x| -x) });
        if index == 0 || index == 1 && !toward_positive {
            return;
        }
        if index == length - 1 || index == length - 2 && toward_positive {
            return;
        }
        let Some(next_seg) = self.segment(next) else {
            return;
        };
        // A survival split takes a belt connector and shafts from the inventory; the actor's
        // inventory holds neither (`use` equips only the held wrench), so it is refused.
        if !self.creative {
            return;
        }
        let (controller, mut taken) = self.w.take_belt(0, p).unwrap();
        let (here, there) = if toward_positive {
            (belt::Part::End, belt::Part::Start)
        } else {
            (belt::Part::Start, belt::Part::End)
        };
        self.set_segment(
            p,
            belt::Segment { part: here, ..seg },
            self.belt_casing.contains_key(&p),
        );
        self.set_segment(
            next,
            belt::Segment {
                part: there,
                ..next_seg
            },
            self.belt_casing.contains_key(&next),
        );
        let new_controller = if toward_positive { next } else { p };
        let shift = index as f32 + if toward_positive { 1. } else { 0. };
        let cut = ((index + i32::from(toward_positive)).max(0) as usize).min(taken.chain.len());
        let (first, second) = taken.chain.split_at(cut);
        // `BeltSlicer.java:222-236`: the first half keeps the old inventory and its flag; the
        // new controller gets a fresh `BeltInventory` (`beltMovementPositive` false) that the
        // items past the cut are added to.
        let (moved, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut taken.items.items)
            .into_iter()
            .partition(|i| i.position - shift > 0.);
        taken.items.items = rest;
        let moved = moved.into_iter().map(|mut i| {
            i.position -= shift;
            // `toRemove` belongs to the inventory that marked it; the new controller's
            // `addItem` takes the stack unmarked (BeltSlicer.java:222-238 walks
            // `getTransportedItems()`), so it survives there while the belt is stopped.
            i.removing = false;
            i
        });
        let mut fresh = belt::Inventory::fresh(Vec::new());
        fresh.add_all(moved);
        self.w.reform_belt(0, controller, first, taken.items);
        self.w.reform_belt(0, new_controller, second, fresh);
    }
    /// `use_connector` at a hovered end: join the belt there, or lengthen this one. False when
    /// the glue passes the click on to the connector item (a block there that is no belt).
    fn slice_connector(&mut self, p: Pos, seg: belt::Segment, length: i32) -> bool {
        let clicked_controller = self.w.dims[0].belt_of[&p];
        let v = seg.vector();
        let color = self.belt_color.get(&p).cloned();
        let next = add(
            p,
            if seg.part == belt::Part::Start {
                v.map(|x| -x)
            } else {
                v
            },
        );
        if self.at(next) != AIR {
            let Some(other) = self.segment(next) else {
                return false;
            };
            if !seg.compatible(&other) {
                return true;
            }
            let Some(oc) = self.w.dims[0].belt_of.get(&next).copied() else {
                return true;
            };
            let other_length = self.w.dims[0].belts[&oc].chain.len() as i32;
            if other_length + length > bc::MAX_LENGTH {
                return true;
            }
            // `BeltSlicer.java:290-306`: per segment of the other belt in chain order,
            // `detachKinetics`, `beltLength = 0`, then the flip. The clicked belt's segments
            // keep their length (`:308` reads the merged chain before the zeroing loop).
            let mut their_chain = self.w.belt_chain(0, next).unwrap();
            let their_controller = self.w.dims[0].belt_of[&next];
            let flip = seg.facing != other.facing;
            if flip && let Some(items) = self.w.belt_items_mut(0, their_controller) {
                for i in items {
                    i.position = other_length as f32 - i.position;
                }
            }
            for q in &their_chain {
                self.w.detach_belt_segment(0, *q);
                if flip && let Some(s) = self.segment(*q) {
                    self.set_segment_direct(*q, s.flipped(), self.belt_casing.contains_key(q));
                }
            }
            if flip {
                their_chain.reverse();
            }
            for q in [p, next] {
                self.set_segment(
                    q,
                    belt::Segment {
                        part: belt::Part::Middle,
                        ..seg
                    },
                    self.belt_casing.contains_key(&q),
                );
            }
            for q in &their_chain {
                self.set_segment_color(*q, color.as_deref());
            }
            if seg.part == belt::Part::End {
                // `BeltSlicer.java:322-333`: the clicked belt starts the merged belt: its
                // inventory stays the merged one, and the other belt's items move on by its
                // length into it (`mergedController.inventory = null`).
                let their_items = self
                    .w
                    .replace_belt_inventory(0, their_controller)
                    .into_iter()
                    .map(|mut i| {
                        i.position += length as f32;
                        i
                    });
                self.w.belt_add_items(0, clicked_controller, their_items);
            } else {
                // The other belt starts the merged belt: its controller keeps its inventory
                // and flag; the clicked controller's inventory is emptied (`= null`).
                let mut mine = self.w.replace_belt_inventory(0, clicked_controller);
                for i in &mut mine {
                    i.position += other_length as f32;
                }
                self.w.belt_add_items(0, their_controller, mine);
            }
            return true;
        }
        let (_, mut taken) = self.w.take_belt(0, p).unwrap();
        self.set_segment(
            p,
            belt::Segment {
                part: belt::Part::Middle,
                ..seg
            },
            self.belt_casing.contains_key(&p),
        );
        self.set_segment_direct(next, seg, false);
        self.set_segment_color(next, color.as_deref());
        let mut chain = taken.chain;
        if seg.part == belt::Part::Start {
            // `BeltSlicer.java:352-361`: the placed segment is the controller now, with a
            // fresh `BeltInventory`; the items move on by one segment.
            chain.insert(0, next);
            let mut fresh = belt::Inventory::fresh(Vec::new());
            fresh.add_all(
                std::mem::take(&mut taken.items.items)
                    .into_iter()
                    .map(|mut i| {
                        i.position += 1.;
                        i
                    }),
            );
            self.w.reform_belt(0, next, &chain, fresh);
        } else {
            chain.push(next);
            self.w.reform_belt(0, chain[0], &chain, taken.items);
        }
        true
    }
}

impl belt::BreakWorld for Run {
    fn take_breaks(&mut self) -> Vec<create_engine::world::BeltBreak> {
        std::mem::take(&mut self.w.dims[0].belt_breaks)
    }
    fn replace(&mut self, b: create_engine::world::BeltBreak) {
        let s = b.shaft.map(|axis| {
            format!(
                "create:shaft[axis={},waterlogged=false]",
                format!("{axis:?}").to_lowercase()
            )
        });
        self.set(b.pos, s.map_or(AIR, |s| self.id(&s)));
        self.belt_casing.remove(&b.pos);
        self.belt_covered.remove(&b.pos);
        self.belt_color.remove(&b.pos);
    }
}
