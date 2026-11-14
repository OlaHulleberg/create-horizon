//! Belts on the server: their items moving (`BeltInventory.tick`) with what lies past their
//! end, the funnels and tunnels over them and the machines above (`BeltSide`); items put on
//! from beside, above or another belt (`DirectBeltInputBehaviour.handleInsertion`); what is
//! thrown off the end; the items clients draw (`create:belt`).

use super::super::net;
use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::super::world::with_world;
use super::arms::{arms_touched, ejector_accepts};
use super::depot::depot_items;
use super::funnel::funnel_at;
use super::items::{self, face};
use super::tunnel::tunnel_state;
use create_engine::belt;
use create_engine::kinetics::{Axis, Dir};
use mod_api::{BlockFace, HostBlockStates, ItemStack, PlayerRef, StateConstraint, SupportType};

/// The belt segment at `pos`: its controller, its index along the chain, and its state.
pub fn belt_segment(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<([i32; 3], i32, belt::Segment)> {
    let (_, info) = super::info_at(lv, pos)?;
    let seg = info.belt?;
    let dim = lv.dim;
    let (c, index) = with_world(|w| {
        let d = &w.dims[dim];
        let c = *d.belt_of.get(&pos)?;
        let index = d.belts.get(&c)?.chain.iter().position(|p| *p == pos)? as i32;
        Some((c, index))
    })
    .flatten()?;
    Some((c, index, seg))
}

/// `BeltBlock.useItemOn` with an empty main hand: every item within reach of the segment's
/// centre goes to the player. True when the click was the belt's.
pub fn use_belt(lv: &mut ServerLevel<'_>, pos: [i32; 3], player: u128) -> bool {
    let Some((c, index, _)) = belt_segment(lv, pos) else {
        return false;
    };
    let dim = lv.dim;
    let taken = with_world(|w| {
        let b = w.dims[dim].belts.get_mut(&c)?;
        let taken = b.items.take_within(index as f32 + 0.5, 0.55);
        if !taken.is_empty() {
            w.dims[dim].belt_changes.push(c);
        }
        Some(taken)
    })
    .flatten()
    .unwrap_or_default();
    for i in taken {
        if let Some(s) = belt_stack(lv, &i) {
            items::give(&mut *lv.host, player, s);
        }
    }
    true
}

/// A belt item as a stack.
pub fn belt_stack(lv: &ServerLevel<'_>, item: &belt::Item) -> Option<ItemStack> {
    items::stack_of(&*lv.host, &item.item, u32::from(item.count))
}

/// Puts `count` of item `id` (an angle drawn for it) onto belt segment `pos` from above
/// (`ItemHandlerBeltSegment.insertItem`: `canInsertAt(segment)`). The count left; None when no
/// belt that carries items is there. `simulate`: only how much it would take.
pub fn put_on_from_above(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    id: &str,
    count: u8,
    simulate: bool,
) -> Option<u8> {
    let (c, index, seg) = belt_segment(lv, pos)?;
    if !seg.transports() {
        return None;
    }
    let dim = lv.dim;
    let n = count.min(items::stack_limit(&*lv.host, id));
    let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
    let angle = items::random_angle(lv, pos);
    let up = items::upright(&*lv.host, id);
    let added = with_world(|w| {
        let d = &mut w.dims[dim];
        let b = d.belts.get_mut(&c)?;
        let movement = belt::movement_facing(rpm, b.segment.facing);
        if !b.items.can_insert_at_from_side(index, Dir::Up, movement) {
            return None;
        }
        if simulate {
            return Some(());
        }
        let mut i = belt::Item::new(id, n, angle, up);
        i.inserted_at = index;
        i.inserted_from = Some(Dir::Up);
        i.position = index as f32 + 0.5 + if b.items.positive { -1.0 } else { 1.0 } / 16.0;
        b.items.add(i);
        d.belt_changes.push(c);
        Some(())
    })
    .flatten();
    Some(if added.is_some() { count - n } else { count })
}

/// `DirectBeltInputBehaviour.handleInsertion` at `next`, the item moving toward `side`: a
/// depot or a belt there. What is left of it, None when nothing there takes items.
pub fn insert_into(
    lv: &mut ServerLevel<'_>,
    next: [i32; 3],
    side: Dir,
    item: &belt::Item,
) -> Option<u8> {
    insert_into_from(lv, next, side, item, true)
}

/// `insert_into` for a stack that comes from a belt behind (`from_belt`: Create's smoother
/// hand-over starts it a quarter block back) or from elsewhere (a funnel).
pub fn insert_into_from(
    lv: &mut ServerLevel<'_>,
    next: [i32; 3],
    side: Dir,
    item: &belt::Item,
    from_belt: bool,
) -> Option<u8> {
    let dim = lv.dim;
    let is_depot = with_world(|w| w.dims[dim].depots.contains_key(&next)) == Some(true);
    if is_depot {
        // DepotBehaviour.tryInsertingFromSide: only into an empty depot (an ejector's only
        // while charged, `canAcceptItems`).
        if !depot_items(lv, next).is_empty() || !ejector_accepts(dim, next) {
            return Some(item.count);
        }
        let put = belt_stack(lv, item)?;
        if !items::set_slot(lv, next, 0, Some(put)) {
            return None;
        }
        let held = create_engine::depot::Held {
            position: 0.0,
            from: create_engine::depot::Side::from_dir(side),
            angle: item.angle,
            side_offset: item.side_offset,
            locked: false,
            fan: item.fan,
        };
        with_world(|w| w.depot_put(dim, next, Some(held)));
        return Some(0);
    }
    // BeltBlockEntity.tryInsertingFromSide on the next belt.
    let (c2, index, seg2) = belt_segment(lv, next)?;
    if !seg2.transports() {
        return None;
    }
    let rpm2 = with_world(|w| w.speed(dim, c2)).flatten().unwrap_or(0.0);
    let speed2 = belt::direction_aware_speed(rpm2, seg2.facing);
    with_world(|w| {
        let b = w.dims[dim].belts.get_mut(&c2)?;
        let movement2 = belt::movement_facing(rpm2, b.segment.facing);
        if rpm2 == 0.0
            || movement2 == side.opposite()
            || !b.items.can_insert_at_from_side(index, side, movement2)
        {
            return Some(item.count);
        }
        let mut moved = item.clone();
        moved.locked = false;
        moved.position = index as f32 + 0.5 - speed2.signum() / 16.0;
        // From above or below: in the segment's middle.
        if side.axis() == Axis::Y {
        } else if movement2 != side {
            moved.side_offset = if side.positive() { 0.675 } else { -0.675 };
            if side.axis() == Axis::X {
                moved.side_offset = -moved.side_offset;
            }
        } else {
            // A smoother hand-over from the belt behind it.
            let extra = if from_belt { 0.26 } else { 0.0 };
            moved.position = if speed2 > 0.0 {
                index as f32 - extra
            } else {
                index as f32 + 1.0 + extra
            };
        }
        moved.inserted_at = index;
        moved.inserted_from = Some(side);
        b.items.add(moved);
        w.dims[dim].belt_changes.push(c2);
        Some(0)
    })
    .flatten()
}

/// `BeltInventory.eject`: an item thrown off the end, forward and a little up.
fn eject_from_belt(
    lv: &mut ServerLevel<'_>,
    controller: [i32; 3],
    seg: &belt::Segment,
    length: i32,
    rpm: f32,
    item: &belt::Item,
) {
    let at = belt::vector_for_offset(controller, seg, length, item.position);
    let speed = belt::movement_speed(rpm).abs().max(1.0 / 8.0);
    let d = belt::movement_facing(rpm, seg.facing).offset();
    let motion = [
        f64::from(d[0]) * f64::from(speed),
        1.0 / 8.0,
        f64::from(d[2]) * f64::from(speed),
    ];
    if let Some(stack) = belt_stack(lv, item) {
        items::drop_stack(lv, [at[0], at[1] + 6.0 / 16.0, at[2]], motion, stack, 10);
    }
}

/// Belts: segments broken next to a broken one become shafts or air, a broken belt's items
/// drop, then belts holding items move them (`BeltInventory.tick`); belts at rest or empty cost
/// nothing.
pub fn tick_belts(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let (breaks, drops) = with_world(|w| {
        let d = &mut w.dims[dim];
        (
            std::mem::take(&mut d.belt_breaks),
            std::mem::take(&mut d.belt_drops),
        )
    })
    .unwrap_or_default();
    for b in breaks {
        let state = {
            let facts = lv.host.block_states();
            match b.shaft {
                Some(a) => mod_api::BlockState::of(
                    &facts,
                    states::key(create_engine::Block::Shaft),
                    &[StateConstraint::new("axis", axis_name(a))],
                )
                .ok(),
                None => lv
                    .host
                    .block_key("minecraft:air")
                    .and_then(|air| mod_api::BlockState::of(&facts, air, &[]).ok()),
            }
        };
        if let Some(state) = state {
            lv.set_state(b.pos, state, update_all());
        }
    }
    for (c, seg, length, item) in drops {
        eject_from_belt(lv, c, &seg, length, 0.0, &item);
    }
    let busy = with_world(|w| w.busy_belts(dim)).unwrap_or_default();
    for c in busy {
        let Some((seg, length, mut belt_items)) = with_world(|w| {
            let b = w.dims[dim].belts.get_mut(&c)?;
            Some((
                b.segment,
                b.chain.len() as i32,
                std::mem::take(&mut b.items),
            ))
        })
        .flatten() else {
            continue;
        };
        let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
        let speed = belt::direction_aware_speed(rpm, seg.facing);
        let mut side = BeltSide {
            lv: &mut *lv,
            controller: c,
            segment: seg,
            length,
            rpm,
            positive: speed > 0.0,
        };
        let horizontal = seg.slope == belt::Slope::Horizontal;
        let ticked = belt_items.tick(length, speed, horizontal, &mut side);
        with_world(|w| {
            if let Some(b) = w.dims[dim].belts.get_mut(&c) {
                // Items another belt put here meanwhile go after ours.
                let added = std::mem::replace(&mut b.items, belt_items);
                for i in added.items {
                    b.items.add(i);
                }
            }
            if ticked.changed {
                w.dims[dim].belt_changes.push(c);
            }
        });
        for item in &ticked.ejected {
            eject_from_belt(lv, c, &seg, length, rpm, item);
        }
    }
    let changed = with_world(|w| {
        let mut v = std::mem::take(&mut w.dims[dim].belt_changes);
        v.sort();
        v.dedup();
        for c in &v {
            w.mark_unsaved(dim, *c);
        }
        v
    })
    .unwrap_or_default();
    if !changed.is_empty() {
        arms_touched(dim, &[], &changed);
        let players = lv.players();
        send_belts(lv, &players, &changed);
    }
}

fn axis_name(a: Axis) -> &'static str {
    match a {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

/// The belts `belts` (controllers) to `players`.
pub fn send_belts(lv: &mut ServerLevel<'_>, players: &[PlayerRef], belts: &[[i32; 3]]) {
    let dim = lv.dim;
    let all: Vec<belt::Contents> = belts
        .iter()
        .map(|&c| {
            with_world(|w| w.dims[dim].belts.get(&c).cloned())
                .flatten()
                .map_or(
                    belt::Contents {
                        controller: c,
                        segment: belt::Segment {
                            facing: Dir::North,
                            slope: belt::Slope::Horizontal,
                            part: belt::Part::Start,
                        },
                        length: 0,
                        ending: belt::Ending::Eject,
                        items: belt::Inventory::default(),
                    },
                    |b| {
                        let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
                        let speed = belt::direction_aware_speed(rpm, b.segment.facing);
                        let mut side = BeltSide {
                            lv: &mut *lv,
                            controller: c,
                            segment: b.segment,
                            length: b.chain.len() as i32,
                            rpm,
                            positive: speed > 0.0,
                        };
                        let ending = belt::Surroundings::ending(&mut side);
                        belt::Contents {
                            controller: c,
                            segment: b.segment,
                            length: b.chain.len() as u16,
                            ending,
                            items: b.items,
                        }
                    },
                )
        })
        .collect();
    for m in belt::encode(&all) {
        net::send_to_players(&mut *lv.host, players, belt::CHANNEL, &m);
    }
}

/// A belt's surroundings on the server: what lies past its end, the funnels and tunnels over
/// its segments, and the machines above them.
pub struct BeltSide<'a, 'l> {
    pub lv: &'a mut ServerLevel<'l>,
    pub controller: [i32; 3],
    pub segment: belt::Segment,
    pub length: i32,
    pub rpm: f32,
    pub positive: bool,
}

impl BeltSide<'_, '_> {
    fn past_end(&self) -> [i32; 3] {
        belt::position_for_offset(
            self.controller,
            &self.segment,
            self.length,
            if self.positive { self.length } else { -1 },
        )
    }

    fn movement(&self) -> Dir {
        belt::movement_facing(self.rpm, self.segment.facing)
    }

    fn at(&self, segment: i32) -> [i32; 3] {
        belt::position_for_offset(self.controller, &self.segment, self.length, segment)
    }
}

/// The game's face `d`.
fn block_face(d: Dir) -> BlockFace {
    match d {
        Dir::Down => BlockFace::Down,
        Dir::Up => BlockFace::Up,
        Dir::North => BlockFace::North,
        Dir::South => BlockFace::South,
        Dir::West => BlockFace::West,
        Dir::East => BlockFace::East,
    }
}

impl belt::Surroundings for BeltSide<'_, '_> {
    fn ending(&mut self) -> belt::Ending {
        let next = self.past_end();
        let dim = self.lv.dim;
        let has = with_world(|w| {
            let d = &w.dims[dim];
            d.depots.contains_key(&next) || d.belt_of.contains_key(&next)
        });
        if has == Some(true) {
            return belt::Ending::Insert;
        }
        // BlockHelper.hasBlockSolidSide toward the belt.
        let face = block_face(self.movement().opposite());
        let sturdy = self
            .lv
            .level
            .block_face_sturdy(&*self.lv.host, next, face, SupportType::Full);
        if sturdy == Some(true) {
            return belt::Ending::Blocked;
        }
        belt::Ending::Eject
    }

    fn insert(&mut self, item: &belt::Item) -> Option<u8> {
        let (next, side) = (self.past_end(), self.movement());
        insert_into(self.lv, next, side, item)
    }

    fn funnel(&mut self, segment: i32) -> Option<(create_engine::funnel::OnBelt, Dir)> {
        let at = self.at(segment);
        let f = funnel_at(self.lv, [at[0], at[1] + 1, at[2]])?;
        let on = create_engine::funnel::OnBelt {
            facing: f.facing,
            shape: f.belt?,
            powered: f.powered,
        };
        Some((on, self.movement()))
    }

    fn funnel_insert(&mut self, segment: i32, item: &belt::Item) -> Option<u8> {
        let at = self.at(segment);
        let pos = [at[0], at[1] + 1, at[2]];
        let f = funnel_at(self.lv, pos)?;
        let behind = create_engine::funnel::behind(pos, f.facing);
        let stack = belt_stack(self.lv, item)?;
        let left = items::insert_item(self.lv, behind, face(f.facing), stack);
        Some(left.map_or(0, |l| l.count().min(255) as u8))
    }

    fn tunnel(&mut self, segment: i32, item: &mut belt::Item) -> bool {
        if self.segment.slope != belt::Slope::Horizontal {
            return false;
        }
        let at = self.at(segment);
        let t = [at[0], at[1] + 1, at[2]];
        let Some((axis, shape)) = tunnel_state(self.lv, t) else {
            return false;
        };
        let movement = self.movement();
        let dim = self.lv.dim;
        // A brass tunnel that distributes (`hasDistributionBehaviour`: an open side across its
        // axis) takes the whole stack while it holds none, and holds back the next.
        if with_world(|w| w.dims[dim].brass.contains_key(&t)) == Some(true) {
            if !shape.junction() {
                return false;
            }
            let took = with_world(|w| {
                let d = &mut w.dims[dim];
                let b = d.brass.get_mut(&t)?;
                if b.stack.is_some() {
                    return Some(false);
                }
                // `setStackToDistribute`.
                b.stack = Some((item.item.clone(), item.count));
                b.stack_progress = item.progress.clone();
                b.entered_from = Some(movement.opposite());
                b.progress = -1;
                b.new_item = true;
                if !d.brass_running.contains(&t) {
                    d.brass_running.push(t);
                }
                w.mark_unsaved(dim, t);
                Some(true)
            })
            .flatten();
            if took == Some(true) {
                item.count = 0;
            }
            return true;
        }
        if !create_engine::tunnel::splits(axis, shape, movement, item.count) {
            return false;
        }
        for d in create_engine::tunnel::sides(axis, shape) {
            if d.axis() == axis {
                continue;
            }
            // A flap: not where another tunnel continues it.
            let o = d.offset();
            if tunnel_state(self.lv, [t[0] + o[0], t[1], t[2] + o[2]]).is_some() {
                continue;
            }
            let out = [at[0] + o[0], at[1], at[2] + o[2]];
            let one = belt::Item {
                count: 1,
                ..item.clone()
            };
            match insert_into(self.lv, out, d, &one) {
                None => continue,
                Some(0) => {}
                Some(_) => return true,
            }
            item.count -= 1;
            if item.count <= 1 {
                break;
            }
        }
        false
    }

    /// The machines over segment `segment` (presses, deployers, spouts) are the processing
    /// subsystems': the first that answers acts.
    fn process(
        &mut self,
        segment: i32,
        item: &mut belt::Item,
        locked: bool,
    ) -> Option<belt::Process> {
        let at = self.at(segment);
        super::super::SUBSYSTEMS
            .iter()
            .find_map(|s| s.belt_process(self.lv, at, item, locked))
    }
}
