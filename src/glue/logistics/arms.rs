//! Mechanical arms and weighted ejectors on the server: the points an arm works on and what they
//! hold (`ArmBlockEntity.tick`), the ejector's launches and landings (`EjectorBlockEntity.tick`),
//! their redstone, their saved data, and what clients draw (`create:arm`, `create:ejector`).
//!
//! An arm's points are chosen with the arm item in Create (`ArmInteractionPointHandler`); the
//! legacy glue stood in for that with a debug command, which the commands port keeps.

use super::super::net;
use super::super::server::ServerLevel;
use super::super::subsystem::Subsystem;
use super::super::world::with_world;
use super::belt::{belt_segment, put_on_from_above};
use super::chute::{chute_accepts, chute_at, chute_stack, set_chute_stack};
use super::depot::depot_items;
use super::funnel::funnel_at;
use super::items::{self, face, random_angle, stack_limit, upright};
use super::{has_collision, info_at};
use create_engine::Block;
use create_engine::arm::{self, Kind, Point, Stack};
use create_engine::ejector;
use create_engine::kinetics::Dir;
use mod_api::{Compound, PlayerRef, ServerHost, Tag};
use std::collections::HashMap;

/// Mechanical arms and weighted ejectors.
pub struct Arms;

impl Subsystem for Arms {
    fn chunk_load(&self, lv: &mut ServerLevel<'_>, _chunk: [i32; 2], data: &Compound) {
        arms_from_nbt(lv.dim, data);
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        arms_to_nbt(lv.dim, chunk, data);
    }

    fn speed_changed(&self, lv: &mut ServerLevel<'_>, pos: [i32; 3], _speed: f32) {
        wake_on_speed(lv.dim, pos);
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        tick_arms(lv);
        tick_ejectors(lv);
    }

    fn player_joined(&self, host: &mut dyn ServerHost, player: PlayerRef) {
        let Some(level) = player.level(&*host) else {
            return;
        };
        let Some(dimension) = level.dimension().id(&*host) else {
            return;
        };
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        sync_arms(&mut lv, player);
    }

    fn server_message(
        &self,
        host: &mut dyn mod_api::ClientHost,
        channel: &str,
        data: &[u8],
    ) -> bool {
        super::client::arms_message(host, channel, data)
    }

    fn client_tick(&self, host: &mut dyn mod_api::ClientHost) {
        super::client::tick_client_arms(host);
    }

    fn client_reset(&self) {
        super::client::reset_arms();
    }
}

/// The block at `pos` as an arm interaction point (`ArmInteractionPointType.getPrimaryType`, in
/// Create's registry order): the types whose blocks this port has.
pub fn arm_point(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Point> {
    let (block, info) = info_at(lv, pos)?;
    let kind = match block {
        Block::Basin => Kind::Basin,
        Block::Belt => Kind::Belt,
        Block::Chute | Block::SmartChute => Kind::Chute,
        Block::MechanicalCrafter => Kind::Crafter,
        Block::Deployer => Kind::Deployer,
        Block::Depot | Block::WeightedEjector => Kind::Depot,
        Block::AndesiteFunnel
        | Block::BrassFunnel
        | Block::AndesiteBeltFunnel
        | Block::BrassBeltFunnel => Kind::Funnel,
        Block::Millstone => Kind::Millstone,
        _ => return None,
    };
    Some(Point {
        pos,
        kind,
        facing: info.facing,
    })
}

/// Whether the depot at `pos` takes items: always, unless it is an ejector that is not charged
/// (`canAcceptItems`).
pub fn ejector_accepts(dim: usize, pos: [i32; 3]) -> bool {
    with_world(|w| {
        w.dims[dim]
            .ejectors
            .get(&pos)
            .is_none_or(|e| e.state == ejector::State::Charged)
    })
    .unwrap_or(true)
}

/// A block entity's slot, as (item, count).
fn slot_at(lv: &ServerLevel<'_>, pos: [i32; 3], slot: usize) -> Option<Stack> {
    let s = items::slot(lv, pos, slot as u32)?;
    Some((items::id_of(&*lv.host, &s), s.count().min(255) as u8))
}

/// Takes up to `amount` from a block entity's slot (or only looks, `simulate`).
fn take_slot(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    slot: usize,
    amount: u8,
    simulate: bool,
) -> Option<Stack> {
    let held = items::slot(lv, pos, slot as u32)?;
    let item = items::id_of(&*lv.host, &held);
    let n = (held.count().min(255) as u8).min(amount);
    if n == 0 {
        return None;
    }
    if !simulate {
        let rest = held.shrink(u32::from(n));
        items::set_slot(lv, pos, slot as u32, rest);
    }
    Some((item, n))
}

/// Puts what fits of a stack into a block entity's slots `slots` (onto the same items, then
/// into empty slots: `insertItemStacked`); the count left.
fn put_slots(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    slots: std::ops::Range<usize>,
    item: &str,
    count: u8,
    simulate: bool,
) -> u8 {
    let max = stack_limit(&*lv.host, item);
    let mut current: Vec<create_engine::processing::Slot> =
        slots.clone().map(|i| slot_at(lv, pos, i)).collect();
    let before = current.clone();
    let left = create_engine::processing::insert_stacked(&mut current, item, count, max);
    if !simulate && left != count {
        let set: Vec<(u32, Option<mod_api::ItemStack>)> = slots
            .zip(current.into_iter().zip(before))
            .filter(|(_, (now, was))| now != was)
            .map(|(i, (s, _))| {
                let stack = s.and_then(|(id, n)| items::stack_of(&*lv.host, &id, u32::from(n)));
                (i as u32, stack)
            })
            .collect();
        items::set_slots(lv, pos, set);
    }
    left
}

/// An arm's points as the game holds them, for its tick.
struct ArmWorld<'a, 'l> {
    lv: &'a mut ServerLevel<'l>,
}

impl ArmWorld<'_, '_> {
    /// The first item on belt segment `index` (`getStackAtOffset`: past the segment's start, not
    /// past its end).
    fn belt_item(&self, c: [i32; 3], index: i32) -> Option<usize> {
        let dim = self.lv.dim;
        with_world(|w| {
            let b = w.dims[dim].belts.get(&c)?;
            b.items
                .items
                .iter()
                .position(|s| s.position > index as f32 && s.position <= index as f32 + 1.0)
        })
        .flatten()
    }
}

impl arm::World for ArmWorld<'_, '_> {
    /// `isValid`: still a block of its type (`canCreatePoint`).
    fn valid(&mut self, p: &Point) -> bool {
        let Some(now) = arm_point(self.lv, p.pos) else {
            return false;
        };
        if now.kind != p.kind {
            return false;
        }
        match p.kind {
            // BeltType: a belt that carries items, with no tunnel over it.
            Kind::Belt => {
                let above = [p.pos[0], p.pos[1] + 1, p.pos[2]];
                let tunnel = matches!(
                    info_at(self.lv, above),
                    Some((Block::AndesiteTunnel | Block::BrassTunnel, _))
                );
                !tunnel && belt_segment(self.lv, p.pos).is_some_and(|(_, _, seg)| seg.transports())
            }
            // FunnelType: not extracting, not a belt funnel pushing onto its belt.
            Kind::Funnel => funnel_at(self.lv, p.pos).is_some_and(|f| {
                !f.extracting && f.belt != Some(create_engine::funnel::BeltShape::Pushing)
            }),
            _ => true,
        }
    }

    fn slots(&mut self, p: &Point) -> usize {
        match p.kind {
            // DepotItemHandler: the held stack and eight outputs.
            Kind::Depot => create_engine::depot::SLOTS,
            Kind::Belt | Kind::Chute | Kind::Deployer | Kind::Crafter => 1,
            // The millstone's input and its nine outputs; the basin's nine and nine.
            Kind::Millstone => 10,
            Kind::Basin => create_engine::basin::SLOTS,
            _ => 0,
        }
    }

    fn extract(&mut self, p: &Point, slot: usize, amount: u8, simulate: bool) -> Option<Stack> {
        let lv = &mut *self.lv;
        let (dim, pos) = (lv.dim, p.pos);
        match p.kind {
            Kind::Depot => {
                let out = take_slot(lv, pos, slot, amount, simulate)?;
                if !simulate {
                    if slot == 0 && slot_at(lv, pos, 0).is_none() {
                        with_world(|w| w.depot_put(dim, pos, None));
                    } else {
                        with_world(|w| w.depot_changed(dim, pos));
                    }
                }
                Some(out)
            }
            Kind::Belt => {
                let (c, index, _) = belt_segment(lv, pos)?;
                let i = self.belt_item(c, index)?;
                with_world(|w| {
                    let d = &mut w.dims[dim];
                    let b = d.belts.get_mut(&c)?;
                    let s = b.items.items.get_mut(i)?;
                    let n = s.count.min(amount);
                    let out = (s.item.clone(), n);
                    if !simulate {
                        s.count -= n;
                        if s.count == 0 {
                            b.items.items.remove(i);
                        }
                        d.belt_changes.push(c);
                    }
                    Some(out)
                })
                .flatten()
            }
            Kind::Chute => {
                let s = chute_stack(lv, pos)?;
                let n = (s.count().min(255) as u8).min(amount);
                let out = (items::id_of(&*lv.host, &s), n);
                if !simulate {
                    set_chute_stack(lv, pos, s.shrink(u32::from(n)));
                    with_world(|w| w.dims[dim].chute_pulls.wake(pos));
                }
                Some(out)
            }
            // Only the outputs give (`MillstoneInventoryHandler`, `BasinInventory`).
            Kind::Millstone if slot >= 1 => take_slot(lv, pos, slot, amount, simulate),
            Kind::Basin if create_engine::basin::OUTPUTS.contains(&slot) => {
                let out = take_slot(lv, pos, slot, amount, simulate)?;
                if !simulate {
                    with_world(|w| w.dims[dim].mixer_checks.push([pos[0], pos[1] + 2, pos[2]]));
                }
                Some(out)
            }
            // CrafterPoint lets the arm take a crafter's item.
            Kind::Crafter => take_slot(lv, pos, 0, amount, simulate),
            // DeployerItemHandler: its held stack.
            Kind::Deployer => {
                let mut d = with_world(|w| w.dims[dim].deployers.get(&pos).cloned()).flatten()?;
                let (item, count) = d.held.clone()?;
                let n = count.min(amount);
                if !simulate {
                    d.held = (count > n).then(|| (item.clone(), count - n));
                    with_world(|w| w.set_deployer(dim, pos, d));
                }
                Some((item, n))
            }
            _ => None,
        }
    }

    fn insert(&mut self, p: &Point, item: &str, count: u8, simulate: bool) -> u8 {
        let lv = &mut *self.lv;
        let (dim, pos) = (lv.dim, p.pos);
        let max = stack_limit(&*lv.host, item);
        match p.kind {
            Kind::Depot => depot_insert(lv, pos, item, count, simulate),
            // ItemHandlerBeltSegment.insertItem: onto the segment from above while nothing just
            // put there is in the way.
            Kind::Belt => put_on_from_above(lv, pos, item, count, simulate).unwrap_or(count),
            // ChuteItemHandler: while it is empty and takes the item, a stack from the top.
            Kind::Chute => {
                let Some((block, powered)) = chute_at(lv, pos) else {
                    return count;
                };
                let Some(one) = items::stack_of(&*lv.host, item, 1) else {
                    return count;
                };
                if chute_stack(lv, pos).is_some() || !chute_accepts(lv, pos, block, powered, &one) {
                    return count;
                }
                let n = count.min(max);
                if !simulate {
                    set_chute_stack(lv, pos, items::stack_of(&*lv.host, item, u32::from(n)));
                    with_world(|w| w.dims[dim].chutes.insert(pos, 1.0));
                }
                count - n
            }
            // FunnelPoint: not while powered; into the inventory behind it.
            Kind::Funnel => {
                let Some(f) = funnel_at(lv, pos) else {
                    return count;
                };
                if f.powered {
                    return count;
                }
                let behind = create_engine::funnel::behind(pos, f.facing);
                let Some(stack) = items::stack_of(&*lv.host, item, u32::from(count)) else {
                    return count;
                };
                if simulate {
                    let room = items::insert_room(lv, behind, face(f.facing), &stack).unwrap_or(0);
                    return count - room.min(u32::from(count)) as u8;
                }
                items::insert_item(lv, behind, face(f.facing), stack)
                    .map_or(0, |l| l.count().min(255) as u8)
            }
            Kind::Millstone => put_slots(lv, pos, 0..1, item, count, simulate),
            Kind::Basin => {
                let left = put_slots(lv, pos, create_engine::basin::INPUTS, item, count, simulate);
                if !simulate && left != count {
                    with_world(|w| w.dims[dim].mixer_checks.push([pos[0], pos[1] + 2, pos[2]]));
                }
                left
            }
            // A crafter takes one item while its slot is empty.
            Kind::Crafter => {
                if slot_at(lv, pos, 0).is_some() {
                    return count;
                }
                if !simulate {
                    items::set_slot(lv, pos, 0, items::stack_of(&*lv.host, item, 1));
                    with_world(|w| w.dims[dim].crafter_checks.push(pos));
                }
                count - 1
            }
            // DeployerItemHandler.insertItem: into an empty hand, or onto the same item.
            Kind::Deployer => {
                let Some(mut d) =
                    with_world(|w| w.dims[dim].deployers.get(&pos).cloned()).flatten()
                else {
                    return count;
                };
                let (now, left) = match d.held.clone() {
                    None => {
                        let n = count.min(max);
                        ((item.to_owned(), n), count - n)
                    }
                    Some((held, n)) if held == item => {
                        let room = max.saturating_sub(n);
                        if room == 0 {
                            return count;
                        }
                        let put = count.min(room);
                        ((held, n + put), count - put)
                    }
                    Some(_) => return count,
                };
                if !simulate {
                    d.held = Some(now);
                    with_world(|w| w.set_deployer(dim, pos, d));
                }
                left
            }
            _ => count,
        }
    }

    /// BeltPoint.keepAlive: the first item on the segment stays still this tick.
    fn keep_alive(&mut self, p: &Point) {
        if p.kind != Kind::Belt {
            return;
        }
        let Some((c, index, _)) = belt_segment(self.lv, p.pos) else {
            return;
        };
        let dim = self.lv.dim;
        let centre = index as f32 + 0.5;
        with_world(|w| {
            if let Some(b) = w.dims[dim].belts.get_mut(&c)
                && let Some(s) = b
                    .items
                    .items
                    .iter_mut()
                    .find(|s| (s.position - centre).abs() < 0.51)
            {
                s.held_externally = true;
            }
        });
    }
}

/// `DepotItemHandler.insertItem` (slot 0) for the depot or ejector at `pos`: a depot takes a
/// stack while it holds nothing (`canMergeItems` false); an ejector only while charged, and
/// merges onto what it holds up to its stack size (`enableMerging`, `getRemainingSpace`).
fn depot_insert(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    item: &str,
    count: u8,
    simulate: bool,
) -> u8 {
    let dim = lv.dim;
    let stacks = depot_items(lv, pos);
    let held = stacks.iter().find(|s| s.0 == 0).cloned();
    let max = stack_limit(&*lv.host, item);
    let ejector = with_world(|w| w.dims[dim].ejectors.get(&pos).cloned()).flatten();
    let room = match &ejector {
        None => {
            if !stacks.is_empty() {
                return count;
            }
            max
        }
        Some(e) => {
            if e.state != ejector::State::Charged || held.as_ref().is_some_and(|h| h.1 != item) {
                return count;
            }
            let limit = if e.max_stack == 0 { 64 } else { e.max_stack }.min(max);
            let present: u32 = stacks.iter().map(|s| u32::from(s.2)).sum();
            u32::from(limit).saturating_sub(present).min(255) as u8
        }
    };
    let n = count.min(room);
    if n == 0 {
        return count;
    }
    if !simulate {
        let had = held.as_ref().map_or(0, |h| h.2);
        let stack = items::stack_of(&*lv.host, item, u32::from(had + n));
        if !items::set_slot(lv, pos, 0, stack) {
            return count;
        }
        if held.is_none() {
            let h =
                create_engine::depot::Held::new(random_angle(lv, pos), upright(&*lv.host, item));
            with_world(|w| w.depot_put(dim, pos, Some(h)));
        } else {
            with_world(|w| w.depot_changed(dim, pos));
        }
    }
    count - n
}

/// Arms and ejectors whose redstone input may have changed read it (`redstoneUpdate`: a newly
/// freed arm looks for items at once; `updateSignal`).
fn tick_arm_signals(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let checks = with_world(|w| std::mem::take(&mut w.dims[dim].arm_signals)).unwrap_or_default();
    for pos in checks {
        let powered = lv.has_neighbor_signal(pos);
        let arm = with_world(|w| w.dims[dim].arms.arms.remove(&pos)).flatten();
        if let Some(mut a) = arm {
            let changed = a.set_powered(pos, powered, &mut ArmWorld { lv: &mut *lv });
            with_world(|w| {
                let arms = &mut w.dims[dim].arms;
                arms.arms.insert(pos, a);
                arms.wake(pos);
                if changed {
                    arms.changes.push(pos);
                }
            });
            continue;
        }
        with_world(|w| {
            let d = &mut w.dims[dim];
            if let Some(e) = d.ejectors.get_mut(&pos)
                && e.powered != powered
            {
                e.powered = powered;
                if !d.ejecting.contains(&pos) {
                    d.ejecting.push(pos);
                }
            }
        });
    }
}

/// Mechanical arms: their redstone, the lazy tick of sleeping arms that poll, then the awake
/// ones (`ArmBlockEntity.tick`); what removed arms held drops; clients see the arms whose target
/// or phase changed. Nothing runs without an awake arm.
fn tick_arms(lv: &mut ServerLevel<'_>) {
    tick_arm_signals(lv);
    let dim = lv.dim;
    let now = lv.game_time().max(0) as u64;
    let running = with_world(|w| {
        let arms = &mut w.dims[dim].arms;
        arms.poll(now);
        !arms.running.is_empty()
    })
    .unwrap_or(false);
    if running {
        // The arms leave the world while they tick: their points reach into it.
        let mut arms = with_world(|w| std::mem::take(&mut w.dims[dim].arms)).unwrap_or_default();
        let speeds: HashMap<[i32; 3], f32> = arms
            .running
            .iter()
            .map(|p| {
                (
                    *p,
                    with_world(|w| w.speed(dim, *p)).flatten().unwrap_or(0.0),
                )
            })
            .collect();
        arms.tick(
            |p| speeds.get(&p).copied().unwrap_or(0.0),
            &mut ArmWorld { lv: &mut *lv },
        );
        with_world(|w| {
            let back = std::mem::replace(&mut w.dims[dim].arms, arms);
            // Changes reported while they ticked.
            let a = &mut w.dims[dim].arms;
            a.changes.extend(back.changes);
            for (p, arm) in back.arms {
                a.arms.entry(p).or_insert(arm);
            }
        });
    }
    let (drops, changes) = with_world(|w| {
        let d = &mut w.dims[dim];
        let mut changes = std::mem::take(&mut d.arms.changes);
        changes.sort();
        changes.dedup();
        for c in &changes {
            if d.arms.arms.contains_key(c) {
                d.unsaved.push((c[0] >> 4, c[2] >> 4));
            }
        }
        (std::mem::take(&mut d.arm_drops), changes)
    })
    .unwrap_or_default();
    for (pos, item, n) in drops {
        let at = pos.map(|v| f64::from(v) + 0.5);
        items::drop_item(lv, at, [0.0; 3], &item, u32::from(n), 0);
    }
    if !changes.is_empty() {
        let players = lv.players();
        send_arms(lv, &players, &changes);
    }
}

/// What changes at a point wakes the arms using it: depots (and ejectors), belts (any of their
/// segments) and chutes report it.
pub fn arms_touched(dim: usize, depots: &[[i32; 3]], belts: &[[i32; 3]]) {
    with_world(|w| {
        if w.dims[dim].arms.watch.is_empty() {
            return;
        }
        let d = &mut w.dims[dim];
        for p in depots {
            d.arms.touched(*p);
        }
        for c in belts {
            if let Some(b) = d.belts.get(c) {
                for p in b.chain.clone() {
                    d.arms.touched(p);
                }
            }
        }
    });
}

/// An arm or ejector whose speed changed ticks again.
fn wake_on_speed(dim: usize, pos: [i32; 3]) {
    with_world(|w| {
        let d = &mut w.dims[dim];
        if d.arms.arms.contains_key(&pos) {
            d.arms.wake(pos);
            d.arms.changes.push(pos);
        }
        if d.ejectors.contains_key(&pos) {
            if !d.ejecting.contains(&pos) {
                d.ejecting.push(pos);
            }
            d.ejector_changes.push(pos);
        }
    });
}

/// Arms to `players`.
fn send_arms(lv: &mut ServerLevel<'_>, players: &[PlayerRef], list: &[[i32; 3]]) {
    let dim = lv.dim;
    let shown: Vec<arm::Shown> = list
        .iter()
        .map(|&pos| {
            with_world(|w| {
                w.dims[dim]
                    .arms
                    .arms
                    .get(&pos)
                    .map(|a| arm::Shown::of(pos, a))
            })
            .flatten()
            .unwrap_or(arm::Shown {
                pos,
                gone: true,
                ceiling: false,
                phase: arm::Phase::SearchInputs,
                progress: 1.0,
                from: None,
                to: None,
                from_base: 0.0,
                held: None,
            })
        })
        .collect();
    for m in arm::encode(&shown) {
        net::send_to_players(&mut *lv.host, players, arm::CHANNEL, &m);
    }
}

/// `ArmBlock.useItemOn` with something held: it goes to the player, the arm looks for inputs.
/// True when the click was the arm's.
pub fn use_arm(lv: &mut ServerLevel<'_>, pos: [i32; 3], player: u128) -> bool {
    let dim = lv.dim;
    let held = with_world(|w| {
        let arms = &mut w.dims[dim].arms;
        let held = arms.arms.get_mut(&pos)?.take_held()?;
        arms.wake(pos);
        arms.changes.push(pos);
        Some(held)
    })
    .flatten();
    let Some((item, n)) = held else {
        return false;
    };
    if let Some(stack) = items::stack_of(&*lv.host, &item, u32::from(n)) {
        items::give(&mut *lv.host, player, stack);
    }
    true
}

/// A weighted ejector's surroundings as its tick reaches them.
struct EjectorWorld<'a, 'l> {
    lv: &'a mut ServerLevel<'l>,
    pos: [i32; 3],
}

/// `DirectBeltInputBehaviour.handleInsertion(stack, UP)` at `target`: a depot or ejector (empty,
/// charged), else a belt segment that carries items. The count left, None when nothing there
/// takes items.
fn land(
    lv: &mut ServerLevel<'_>,
    target: [i32; 3],
    item: &str,
    count: u8,
    simulate: bool,
) -> Option<u8> {
    let dim = lv.dim;
    let is_depot = with_world(|w| w.dims[dim].depots.contains_key(&target)) == Some(true);
    if is_depot {
        if !depot_items(lv, target).is_empty() || !ejector_accepts(dim, target) {
            return Some(count);
        }
        let n = count.min(stack_limit(&*lv.host, item));
        if !simulate {
            let stack = items::stack_of(&*lv.host, item, u32::from(n));
            if !items::set_slot(lv, target, 0, stack) {
                return None;
            }
            let h =
                create_engine::depot::Held::new(random_angle(lv, target), upright(&*lv.host, item));
            with_world(|w| w.depot_put(dim, target, Some(h)));
        }
        return Some(count - n);
    }
    put_on_from_above(lv, target, item, count, simulate)
}

impl ejector::World for EjectorWorld<'_, '_> {
    fn present(&mut self) -> ejector::Present {
        let dim = self.lv.dim;
        let stacks = depot_items(self.lv, self.pos);
        let held = stacks.iter().any(|s| s.0 == 0);
        let position = with_world(|w| w.dims[dim].depots.get(&self.pos).copied().flatten())
            .flatten()
            .map_or(0.5, |h| h.position);
        ejector::Present {
            count: stacks.iter().map(|s| u32::from(s.2)).sum(),
            held_position: held.then_some(position),
        }
    }

    fn target_refuses(&mut self, target: [i32; 3]) -> bool {
        let Some((_, item, count)) = depot_items(self.lv, self.pos)
            .into_iter()
            .find(|s| s.0 == 0)
        else {
            return false;
        };
        land(self.lv, target, &item, count, true) == Some(count)
    }

    fn take_all(&mut self) -> Vec<(String, u8)> {
        let Some(slots) = items::slots(self.lv, self.pos, create_engine::depot::SLOTS as u32)
        else {
            return Vec::new();
        };
        let taken: Vec<(String, u8)> = slots
            .iter()
            .flatten()
            .map(|s| (items::id_of(&*self.lv.host, s), s.count().min(255) as u8))
            .collect();
        if !taken.is_empty() {
            let clear = (0..create_engine::depot::SLOTS as u32).map(|i| (i, None));
            items::set_slots(self.lv, self.pos, clear);
            let dim = self.lv.dim;
            with_world(|w| w.depot_put(dim, self.pos, None));
        }
        taken
    }

    fn insert(&mut self, target: [i32; 3], item: &str, count: u8) -> Option<u8> {
        land(self.lv, target, item, count, false)
    }

    fn drop_item(&mut self, at: [f64; 3], motion: [f64; 3], item: &str, count: u8) {
        items::drop_item(self.lv, at, motion, item, u32::from(count), 10);
    }

    fn clip(&mut self, from: [f64; 3], to: [f64; 3]) -> Option<([f64; 3], [i32; 3], Dir)> {
        let own = self.pos;
        let lv = &*self.lv;
        ejector::clip_blocks(from, to, |p| p != own && has_collision(lv, p))
    }

    fn extracting_funnel(&mut self, pos: [i32; 3]) -> bool {
        funnel_at(self.lv, pos).is_some_and(|f| f.belt.is_none() && f.extracting)
    }
}

/// Weighted ejectors that have something to do (`EjectorBlockEntity.tick`): winding up,
/// launching, stacks in flight, or charged with something on them; the rest cost nothing.
/// Stacks of removed ejectors drop; clients see the ejectors whose plate or flight changed.
fn tick_ejectors(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let list = with_world(|w| w.dims[dim].ejecting.clone()).unwrap_or_default();
    for pos in list {
        let Some(facing) = info_at(lv, pos).and_then(|(_, i)| i.facing) else {
            continue;
        };
        let Some(mut e) = with_world(|w| w.dims[dim].ejectors.remove(&pos)).flatten() else {
            continue;
        };
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let flying = e.flying.len();
        let changed = e.tick(pos, facing, speed, &mut EjectorWorld { lv: &mut *lv, pos });
        let has_items = !depot_items(lv, pos).is_empty();
        let busy = e.busy(speed, has_items);
        with_world(|w| {
            let d = &mut w.dims[dim];
            d.ejectors.insert(pos, e);
            if changed {
                d.ejector_changes.push(pos);
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            } else if flying > 0 {
                // Stacks still in flight are saved with it.
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if !busy {
                d.ejecting.retain(|p| *p != pos);
            }
        });
    }
    let (drops, changes) = with_world(|w| {
        let d = &mut w.dims[dim];
        let mut c = std::mem::take(&mut d.ejector_changes);
        c.sort();
        c.dedup();
        (std::mem::take(&mut d.ejector_drops), c)
    })
    .unwrap_or_default();
    for (at, v, item, n) in drops {
        items::drop_item(lv, at, v, &item, u32::from(n), 0);
    }
    if !changes.is_empty() {
        let players = lv.players();
        send_ejectors(lv, &players, &changes);
    }
}

/// An ejector starts ticking: its depot changed, it started turning.
pub fn wake_ejectors(dim: usize, list: &[[i32; 3]]) {
    with_world(|w| {
        let d = &mut w.dims[dim];
        if d.ejectors.is_empty() {
            return;
        }
        for p in list {
            if d.ejectors.contains_key(p) && !d.ejecting.contains(p) {
                d.ejecting.push(*p);
            }
        }
    });
}

/// Ejectors to `players`.
fn send_ejectors(lv: &mut ServerLevel<'_>, players: &[PlayerRef], list: &[[i32; 3]]) {
    let dim = lv.dim;
    let shown: Vec<ejector::Shown> = list
        .iter()
        .map(|&pos| {
            let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
            with_world(|w| {
                w.dims[dim]
                    .ejectors
                    .get(&pos)
                    .map(|e| ejector::Shown::of(pos, e, speed))
            })
            .flatten()
            .unwrap_or(ejector::Shown {
                pos,
                gone: true,
                state: ejector::State::Charged,
                lid: 0.0,
                wind_up: 0.0,
                h: 1,
                v: 0,
                early: None,
                flying: Vec::new(),
            })
        })
        .collect();
    for m in ejector::encode(&shown) {
        net::send_to_players(&mut *lv.host, players, ejector::CHANNEL, &m);
    }
}

/// A joining player sees every arm and ejector of its level.
fn sync_arms(lv: &mut ServerLevel<'_>, player: PlayerRef) {
    let dim = lv.dim;
    let arms: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].arms.arms.keys().copied().collect()).unwrap_or_default();
    send_arms(lv, &[player], &arms);
    let ejectors: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].ejectors.keys().copied().collect()).unwrap_or_default();
    send_ejectors(lv, &[player], &ejectors);
}

/// Arms and ejectors saved with their column (`ArmBlockEntity.write`: `InteractionPoints`,
/// `Phase`, `Powered`, `HeldItem`, `TargetPointIndex`, `MovementProgress`, the selection mode;
/// `EjectorBlockEntity.write`: `HorizontalDistance`, `VerticalDistance`, `Powered`, `State`,
/// `Lid`, `LaunchedItems`, the stack size): `arms` and `ejectors`, absent when the column has
/// none.
fn arms_to_nbt(dim: usize, chunk: [i32; 2], own: &mut Compound) {
    let arms = with_world(|w| w.dims[dim].arms.column(chunk[0], chunk[1])).unwrap_or_default();
    let mut ejectors: Vec<([i32; 3], ejector::Ejector)> = with_world(|w| {
        w.dims[dim]
            .ejectors
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]))
            .map(|(p, e)| (*p, e.clone()))
            .collect()
    })
    .unwrap_or_default();
    ejectors.sort_by_key(|e| e.0);
    if !arms.is_empty() {
        let list = arms
            .iter()
            .map(|(p, a)| {
                let mut v = vec![
                    p[0],
                    p[1],
                    p[2],
                    i32::from(a.phase.index()),
                    a.index,
                    a.progress.to_bits() as i32,
                    i32::from(a.powered),
                    a.selection as i32,
                    i32::from(a.held.as_ref().map_or(0, |h| h.1)),
                    a.inputs.len() as i32,
                ];
                for pt in a.points() {
                    v.extend([
                        pt.pos[0] - p[0],
                        pt.pos[1] - p[1],
                        pt.pos[2] - p[2],
                        i32::from(pt.kind.index()),
                        pt.facing.map_or(-1, |f| f as i32),
                    ]);
                }
                let mut c = Compound::new();
                c.insert("v".into(), Tag::IntArray(v));
                let held = a.held.as_ref().map_or("", |h| h.0.as_str());
                c.insert("held".into(), Tag::String(held.into()));
                Tag::Compound(c)
            })
            .collect();
        own.insert("arms".into(), Tag::List(list));
    }
    if !ejectors.is_empty() {
        let list = ejectors
            .iter()
            .map(|(p, e)| {
                let mut v = vec![
                    p[0],
                    p[1],
                    p[2],
                    e.launcher.h,
                    e.launcher.v,
                    i32::from(e.state.index()),
                    e.lid.value.to_bits() as i32,
                    i32::from(e.powered),
                    i32::from(e.max_stack),
                ];
                for f in &e.flying {
                    v.extend([f.time, i32::from(f.count)]);
                }
                let mut c = Compound::new();
                c.insert("v".into(), Tag::IntArray(v));
                let items = e
                    .flying
                    .iter()
                    .map(|f| Tag::String(f.item.as_str().into()))
                    .collect();
                c.insert("flying".into(), Tag::List(items));
                Tag::Compound(c)
            })
            .collect();
        own.insert("ejectors".into(), Tag::List(list));
    }
}

/// What `arms_to_nbt` wrote: arms and ejectors waiting for their blocks.
fn arms_from_nbt(dim: usize, data: &Compound) {
    let text = |t: Option<&Tag>| match t {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    let mut arms = Vec::new();
    if let Some(Tag::List(list)) = data.get(&"arms".into()) {
        for t in list {
            let Tag::Compound(c) = t else { continue };
            let Some(Tag::IntArray(v)) = c.get(&"v".into()) else {
                continue;
            };
            if v.len() < 10 {
                continue;
            }
            let pos = [v[0], v[1], v[2]];
            let mut a = arm::Arm::new(false);
            a.phase = arm::Phase::from_index(v[3].clamp(0, 4) as u8);
            a.index = v[4];
            a.progress = f32::from_bits(v[5] as u32).clamp(0.0, 1.0);
            a.powered = v[6] != 0;
            a.selection = match v[7] {
                1 => arm::Selection::ForcedRoundRobin,
                2 => arm::Selection::PreferFirst,
                _ => arm::Selection::RoundRobin,
            };
            let held = text(c.get(&"held".into()));
            let n = v[8].clamp(0, 64) as u8;
            a.held = (!held.is_empty() && n > 0).then_some((held, n));
            let inputs = v[9].max(0) as usize;
            for (i, pt) in v[10..].as_chunks::<5>().0.iter().enumerate() {
                let Some(kind) = Kind::from_index(pt[3].clamp(0, 255) as u8) else {
                    continue;
                };
                let point = Point {
                    pos: [pos[0] + pt[0], pos[1] + pt[1], pos[2] + pt[2]],
                    kind,
                    facing: usize::try_from(pt[4])
                        .ok()
                        .and_then(|i| Dir::ALL.get(i))
                        .copied(),
                };
                if i < inputs {
                    a.inputs.push(point);
                } else {
                    a.outputs.push(point);
                }
            }
            arms.push((pos, a));
        }
    }
    let mut ejectors = Vec::new();
    if let Some(Tag::List(list)) = data.get(&"ejectors".into()) {
        for t in list {
            let Tag::Compound(c) = t else { continue };
            let Some(Tag::IntArray(v)) = c.get(&"v".into()) else {
                continue;
            };
            if v.len() < 9 {
                continue;
            }
            let pos = [v[0], v[1], v[2]];
            let mut e = ejector::Ejector {
                launcher: ejector::Launcher::new(v[3], v[4]),
                ..Default::default()
            };
            e.launcher.clamp(ejector::MAX_DISTANCE);
            e.state = ejector::State::from_index(v[5].clamp(0, 2) as u8);
            let lid = f32::from_bits(v[6] as u32).clamp(0.0, 1.0);
            e.lid.value = lid;
            e.lid.previous = lid;
            e.lid.target = lid;
            e.powered = v[7] != 0;
            e.max_stack = v[8].clamp(0, 64) as u8;
            let items: Vec<String> = match c.get(&"flying".into()) {
                Some(Tag::List(l)) => l.iter().map(|t| text(Some(t))).collect(),
                _ => Vec::new(),
            };
            for (f, item) in v[9..].as_chunks::<2>().0.iter().zip(items) {
                let id = e.next_id;
                e.next_id += 1;
                e.flying.push(ejector::Flying {
                    id,
                    time: f[0],
                    item,
                    count: f[1].clamp(1, 64) as u8,
                });
            }
            ejectors.push((pos, e));
        }
    }
    if arms.is_empty() && ejectors.is_empty() {
        return;
    }
    with_world(|w| {
        let d = &mut w.dims[dim];
        d.arms.saved.extend(arms);
        d.ejector_saved.extend(ejectors);
    });
}
