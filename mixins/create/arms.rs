// Create's mechanical arm and weighted ejector in the game: the points an arm works on and what
// they hold, the ejector's launches and landings, their redstone, saves and debug commands, and
// how clients draw them as GPU parts. A sibling of the `create` module (`runtime.rs`).
use super::Game;
use super::create::*;
use super::create_logistics::*;
use create_engine::Block;
use create_engine::arm::{self, Kind, Point, Stack};
use create_engine::ejector;
use create_engine::kinetics::Dir;
use create_engine::states;
use std::cell::RefCell;
use std::collections::HashMap;

fn stack_of(item: &str, count: u8) -> horizon_client::items::ItemStack {
    horizon_client::items::ItemStack {
        item: item.into(),
        count: u32::from(count),
        damage: 0,
        extra: None,
    }
}

/// The block at `pos` as an arm interaction point (`ArmInteractionPointType.getPrimaryType`, in
/// Create's registry order): the types whose blocks this port has.
pub(super) fn arm_point(game: &Game<'_>, pos: [i32; 3]) -> Option<Point> {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    states::index(state)?;
    let (block, facing) =
        with_world(|w| w.table.get(state).map(|(b, i)| (b, i.facing))).flatten()?;
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
    Some(Point { pos, kind, facing })
}

fn is_ejector(game: &Game<'_>, pos: [i32; 3]) -> bool {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    states::index(state).is_some()
        && with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
            == Some(Block::WeightedEjector)
}

/// Whether the depot at `pos` takes items: always, unless it is an ejector that is not charged
/// (`canAcceptItems`).
pub(super) fn ejector_accepts(dim: usize, pos: [i32; 3]) -> bool {
    with_world(|w| {
        w.dims[dim]
            .ejectors
            .get(&pos)
            .is_none_or(|e| e.state == ejector::State::Charged)
    })
    .unwrap_or(true)
}

/// A block entity's slot, as (item, count).
fn slot_at(game: &Game<'_>, pos: [i32; 3], slot: usize) -> Option<Stack> {
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(horizon_client::containers::BlockEntity::Mod(m)) => m
            .items
            .get(slot)?
            .as_ref()
            .map(|s| (s.item.to_string(), s.count.min(255) as u8)),
        _ => None,
    }
}

/// Takes up to `amount` from a block entity's slot (or only looks, `simulate`).
fn take_slot(
    game: &mut Game<'_>,
    pos: [i32; 3],
    slot: usize,
    amount: u8,
    simulate: bool,
) -> Option<Stack> {
    let (item, count) = slot_at(game, pos, slot)?;
    let n = count.min(amount);
    if n == 0 {
        return None;
    }
    if !simulate {
        game.native_with_mod_block_entity(pos, |be| {
            if let Some(Some(s)) = be.items.get_mut(slot) {
                s.count -= u32::from(n);
                if s.count == 0 {
                    be.items[slot] = None;
                }
            }
        });
    }
    Some((item, n))
}

/// Puts what fits of a stack into a block entity's slots `slots` (onto the same items, then
/// into empty slots: `insertItemStacked`); the count left.
fn put_slots(
    game: &mut Game<'_>,
    pos: [i32; 3],
    slots: std::ops::Range<usize>,
    item: &str,
    count: u8,
    simulate: bool,
) -> u8 {
    let max = stack_limit(game, item);
    let end = slots.end;
    let mut current: Vec<create_engine::processing::Slot> =
        slots.clone().map(|i| slot_at(game, pos, i)).collect();
    let left = create_engine::processing::insert_stacked(&mut current, item, count, max);
    if !simulate && left != count {
        game.native_with_mod_block_entity(pos, |be| {
            be.items.resize(be.items.len().max(end), None);
            for (i, s) in slots.zip(current) {
                be.items[i] = s.map(|(i, n)| stack_of(&i, n));
            }
        });
    }
    left
}

/// An arm's points as the game holds them, for its tick.
struct ArmWorld<'a, 'g> {
    game: &'a mut Game<'g>,
    dim: usize,
}

impl ArmWorld<'_, '_> {
    /// The first item on belt segment `index` (`getStackAtOffset`: past the segment's start, not
    /// past its end).
    fn belt_item(&self, c: [i32; 3], index: i32) -> Option<usize> {
        with_world(|w| {
            let b = w.dims[self.dim].belts.get(&c)?;
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
        let Some(now) = arm_point(self.game, p.pos) else {
            return false;
        };
        if now.kind != p.kind {
            return false;
        }
        match p.kind {
            // BeltType: a belt that carries items, with no tunnel over it.
            Kind::Belt => {
                let above = [p.pos[0], p.pos[1] + 1, p.pos[2]];
                let state = self.game.world.block(above[0], above[1], above[2]).0;
                let tunnel = states::index(state).is_some()
                    && matches!(
                        with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten(),
                        Some(Block::AndesiteTunnel | Block::BrassTunnel)
                    );
                !tunnel
                    && belt_segment(self.game, self.dim, p.pos)
                        .is_some_and(|(_, _, seg)| seg.transports())
            }
            // FunnelType: not extracting, not a belt funnel pushing onto its belt.
            Kind::Funnel => funnel_at(self.game, p.pos).is_some_and(|f| {
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
        let (game, dim, pos) = (&mut *self.game, self.dim, p.pos);
        match p.kind {
            Kind::Depot => {
                let out = take_slot(game, pos, slot, amount, simulate)?;
                if !simulate {
                    if slot == 0 && slot_at(game, pos, 0).is_none() {
                        with_world(|w| w.depot_put(dim, pos, None));
                    } else {
                        with_world(|w| w.depot_changed(dim, pos));
                    }
                }
                Some(out)
            }
            Kind::Belt => {
                let (c, index, _) = belt_segment(game, dim, pos)?;
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
                let mut s = chute_stack(game, pos)?;
                let n = (s.count.min(255) as u8).min(amount);
                let out = (s.item.to_string(), n);
                if !simulate {
                    s.count -= u32::from(n);
                    set_chute_stack(game, pos, (s.count > 0).then_some(s));
                    with_world(|w| w.dims[dim].chute_pulls.wake(pos));
                }
                Some(out)
            }
            // Only the outputs give (`MillstoneInventoryHandler`, `BasinInventory`).
            Kind::Millstone if slot >= 1 => take_slot(game, pos, slot, amount, simulate),
            Kind::Basin if create_engine::basin::OUTPUTS.contains(&slot) => {
                let out = take_slot(game, pos, slot, amount, simulate)?;
                if !simulate {
                    with_world(|w| w.dims[dim].mixer_checks.push([pos[0], pos[1] + 2, pos[2]]));
                }
                Some(out)
            }
            // CrafterPoint lets the arm take a crafter's item.
            Kind::Crafter => take_slot(game, pos, 0, amount, simulate),
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
        let (game, dim, pos) = (&mut *self.game, self.dim, p.pos);
        let max = stack_limit(game, item);
        match p.kind {
            Kind::Depot => depot_insert(game, dim, pos, item, count, simulate),
            // ItemHandlerBeltSegment.insertItem: onto the segment from above while nothing just
            // put there is in the way.
            Kind::Belt => {
                let Some((c, index, seg)) = belt_segment(game, dim, pos) else {
                    return count;
                };
                if !seg.transports() {
                    return count;
                }
                let n = count.min(max);
                let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
                let angle = random_angle(game, pos);
                let up = upright(game, item);
                let added = with_world(|w| {
                    let d = &mut w.dims[dim];
                    let b = d.belts.get_mut(&c)?;
                    let movement = create_engine::belt::movement_facing(rpm, b.segment.facing);
                    if !b.items.can_insert_at_from_side(index, Dir::Up, movement) {
                        return None;
                    }
                    if !simulate {
                        let mut i = create_engine::belt::Item::new(item, n, angle, up);
                        i.inserted_at = index;
                        i.inserted_from = Some(Dir::Up);
                        i.position =
                            index as f32 + 0.5 + if b.items.positive { -1.0 } else { 1.0 } / 16.0;
                        b.items.add(i);
                        d.belt_changes.push(c);
                    }
                    Some(())
                })
                .flatten();
                if added.is_some() { count - n } else { count }
            }
            // ChuteItemHandler: while it is empty and takes the item, a stack from the top.
            Kind::Chute => {
                let Some((block, powered)) = chute_at(game, pos) else {
                    return count;
                };
                if chute_stack(game, pos).is_some()
                    || !chute_accepts(dim, pos, block, powered, item)
                {
                    return count;
                }
                let n = count.min(max);
                if !simulate {
                    set_chute_stack(game, pos, Some(stack_of(item, n)));
                    with_world(|w| w.dims[dim].chutes.insert(pos, 1.0));
                }
                count - n
            }
            // FunnelPoint: not while powered; into the inventory behind it.
            Kind::Funnel => {
                let Some(f) = funnel_at(game, pos) else {
                    return count;
                };
                if f.powered {
                    return count;
                }
                let behind = create_engine::funnel::behind(pos, f.facing);
                let face = horizon_core::geometry::Direction::ALL[f.facing as usize];
                let stack = stack_of(item, count);
                if simulate {
                    let room = insert_room(game, dim, behind, face, &stack).unwrap_or(0);
                    return count - room.min(u32::from(count)) as u8;
                }
                insert_item(game, dim, behind, face, stack).map_or(0, |l| l.count.min(255) as u8)
            }
            Kind::Millstone => put_slots(game, pos, 0..1, item, count, simulate),
            Kind::Basin => {
                let left = put_slots(
                    game,
                    pos,
                    create_engine::basin::INPUTS,
                    item,
                    count,
                    simulate,
                );
                if !simulate && left != count {
                    with_world(|w| w.dims[dim].mixer_checks.push([pos[0], pos[1] + 2, pos[2]]));
                }
                left
            }
            // A crafter takes one item while its slot is empty.
            Kind::Crafter => {
                if slot_at(game, pos, 0).is_some() {
                    return count;
                }
                if !simulate {
                    game.native_with_mod_block_entity(pos, |be| {
                        be.items.resize(be.items.len().max(1), None);
                        be.items[0] = Some(stack_of(item, 1));
                    });
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
        let Some((c, index, _)) = belt_segment(self.game, self.dim, p.pos) else {
            return;
        };
        let centre = index as f32 + 0.5;
        with_world(|w| {
            if let Some(b) = w.dims[self.dim].belts.get_mut(&c)
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
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    item: &str,
    count: u8,
    simulate: bool,
) -> u8 {
    let items = depot_items(game, pos);
    let held = items.iter().find(|s| s.0 == 0).cloned();
    let max = stack_limit(game, item);
    let ejector = with_world(|w| w.dims[dim].ejectors.get(&pos).cloned()).flatten();
    let room = match &ejector {
        None => {
            if !items.is_empty() {
                return count;
            }
            max
        }
        Some(e) => {
            if e.state != ejector::State::Charged || held.as_ref().is_some_and(|h| h.1 != item) {
                return count;
            }
            let limit = if e.max_stack == 0 { 64 } else { e.max_stack }.min(max);
            let present: u32 = items.iter().map(|s| u32::from(s.2)).sum();
            u32::from(limit).saturating_sub(present).min(255) as u8
        }
    };
    let n = count.min(room);
    if n == 0 {
        return count;
    }
    if !simulate {
        let had = held.as_ref().map_or(0, |h| h.2);
        game.native_with_mod_block_entity(pos, |be| {
            be.items
                .resize(be.items.len().max(create_engine::depot::SLOTS), None);
            be.items[0] = Some(stack_of(item, had + n));
        });
        if held.is_none() {
            let h = create_engine::depot::Held::new(random_angle(game, pos), upright(game, item));
            with_world(|w| w.depot_put(dim, pos, Some(h)));
        } else {
            with_world(|w| w.depot_changed(dim, pos));
        }
    }
    count - n
}

/// Arms and ejectors whose redstone input may have changed read it (`redstoneUpdate`: a newly
/// freed arm looks for items at once; `updateSignal`).
fn tick_arm_signals(game: &mut Game<'_>, dim: usize) {
    let checks = with_world(|w| std::mem::take(&mut w.dims[dim].arm_signals)).unwrap_or_default();
    for pos in checks {
        let at = horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]);
        let powered = game.with_level(|b, level, ticks| b.has_neighbor_signal(level, ticks, at));
        let arm = with_world(|w| w.dims[dim].arms.arms.remove(&pos)).flatten();
        if let Some(mut a) = arm {
            let changed = a.set_powered(pos, powered, &mut ArmWorld { game, dim });
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
pub(super) fn tick_arms(game: &mut Game<'_>, dimension: &str, dim: usize) {
    tick_arm_signals(game, dim);
    let now = game.world.game_time.max(0) as u64;
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
            &mut ArmWorld { game, dim },
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
        game.native_run_command(&format!(
            "execute in {dimension} run summon minecraft:item {} {} {} {{Item:{{id:'{item}',count:{n}}}}}",
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]) + 0.5,
            f64::from(pos[2]) + 0.5
        ));
    }
    if !changes.is_empty() {
        let players = players_in(game, dimension);
        send_arms(game, &players, dim, &changes);
    }
}

/// What changes at a point wakes the arms using it: depots (and ejectors), belts (any of their
/// segments) and chutes report it.
pub(super) fn arms_touched(dim: usize, depots: &[[i32; 3]], belts: &[[i32; 3]]) {
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
pub(super) fn wake_on_speed(dim: usize, pos: [i32; 3]) {
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
pub(super) fn send_arms(game: &mut Game<'_>, players: &[u128], dim: usize, list: &[[i32; 3]]) {
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
        for &p in players {
            let _ = game.native_net_send_to_player(p, arm::CHANNEL, &m);
        }
    }
}

/// `ArmBlock.useItemOn` with something held: it goes to the player, the arm looks for inputs.
pub(super) fn use_arm(game: &mut Game<'_>, dim: usize, pos: [i32; 3], canceled: &mut bool) {
    let held = with_world(|w| {
        let arms = &mut w.dims[dim].arms;
        let held = arms.arms.get_mut(&pos)?.take_held()?;
        arms.wake(pos);
        arms.changes.push(pos);
        Some(held)
    })
    .flatten();
    if let Some((item, n)) = held {
        *canceled = true;
        game.native_give_items(&item, u32::from(n));
    }
}

/// `/create_debug arm <pos> <input|output> <x y z>...`: the arm's points, standing in for the
/// arm item's selection (`ArmInteractionPointHandler`): each block must be a point type, inputs
/// not deposit-only, all closer than 5 blocks (`flushSettings`).
pub(super) fn debug_arm(game: &mut Game<'_>, dim: usize, pos: [i32; 3], rest: &[&str]) -> String {
    let have = with_world(|w| w.dims[dim].arms.arms.contains_key(&pos)).unwrap_or(false);
    if !have {
        return format!("no mechanical arm at {} {} {}", pos[0], pos[1], pos[2]);
    }
    let (mut inputs, mut outputs) = (Vec::new(), Vec::new());
    let mut take = None;
    let mut nums = Vec::new();
    let mut skipped = 0;
    for t in rest {
        match *t {
            "input" => take = Some(true),
            "output" => take = Some(false),
            n => {
                let Ok(v) = n.parse::<i32>() else {
                    return format!("not a coordinate: {n}");
                };
                nums.push(v);
                if nums.len() == 3 {
                    let at = [nums[0], nums[1], nums[2]];
                    nums.clear();
                    let Some(p) = arm_point(game, at) else {
                        skipped += 1;
                        continue;
                    };
                    match take {
                        Some(true) => inputs.push(p),
                        Some(false) => outputs.push(p),
                        None => return "say input or output before the points".into(),
                    }
                }
            }
        }
    }
    let asked = inputs.len() + outputs.len();
    with_world(|w| w.dims[dim].arms.set_points(pos, inputs, outputs));
    let (i, o) = with_world(|w| {
        let a = &w.dims[dim].arms.arms[&pos];
        (a.inputs.len(), a.outputs.len())
    })
    .unwrap_or_default();
    with_world(|w| w.mark_unsaved(dim, pos));
    let dropped = asked - i - o;
    format!(
        "mechanical arm at {} {} {}: {i} inputs, {o} outputs ({skipped} not points, {dropped} out of range or deposit-only)",
        pos[0], pos[1], pos[2]
    )
}

/// `/create_debug arm_state <pos>`: what the arm does, for tests.
pub(super) fn debug_arm_state(dim: usize, pos: [i32; 3]) -> String {
    with_world(|w| {
        w.dims[dim].arms.arms.get(&pos).map(|a| {
            format!(
                "mechanical arm at {} {} {}: {:?} target {} progress {:.3} held {} powered {}",
                pos[0],
                pos[1],
                pos[2],
                a.phase,
                a.index,
                a.progress,
                a.held
                    .as_ref()
                    .map_or("nothing".to_owned(), |h| format!("{} {}", h.1, h.0)),
                a.powered
            )
        })
    })
    .flatten()
    .unwrap_or_else(|| format!("no mechanical arm at {} {} {}", pos[0], pos[1], pos[2]))
}

/// `/create_debug ejector <pos> <x y z>`: the ejector's target (the ejector item's selection,
/// `EjectorTargetHandler`, `EjectorPlacementPacket`): on a straight line from it, within 32
/// blocks; it turns to face it.
pub(super) fn debug_ejector(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    target: [i32; 3],
) -> String {
    if !is_ejector(game, pos) {
        return format!("no weighted ejector at {} {} {}", pos[0], pos[1], pos[2]);
    }
    let Some((facing, h, v)) = ejector::target_for(pos, target) else {
        return "target not valid: not on a line from the ejector, or too far".into();
    };
    with_world(|w| {
        let d = &mut w.dims[dim];
        if let Some(e) = d.ejectors.get_mut(&pos) {
            e.set_target(h, v);
            d.ejector_changes.push(pos);
            if !d.ejecting.contains(&pos) {
                d.ejecting.push(pos);
            }
        }
        w.mark_unsaved(dim, pos);
    });
    let state = game.world.block(pos[0], pos[1], pos[2]);
    if with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten() != Some(facing)
        && let Some(mut s) = game.pack.assets.registry.state(state).cloned()
    {
        s.properties
            .insert("facing".to_owned(), dir_name(facing).to_owned());
        if let Some(id) = game.pack.assets.registry.id(&s) {
            game.native_set_block(dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
        }
    }
    format!(
        "weighted ejector at {} {} {}: facing {}, {h} ahead, {v} up",
        pos[0],
        pos[1],
        pos[2],
        dir_name(facing)
    )
}

/// `/create_debug ejector_state <pos>`: what the ejector does, for tests.
pub(super) fn debug_ejector_state(dim: usize, pos: [i32; 3]) -> String {
    with_world(|w| {
        w.dims[dim].ejectors.get(&pos).map(|e| {
            format!(
                "weighted ejector at {} {} {}: {:?} lid {:.3} target {} ahead {} up, {} in flight, powered {}",
                pos[0],
                pos[1],
                pos[2],
                e.state,
                e.lid.value,
                e.launcher.h,
                e.launcher.v,
                e.flying.len(),
                e.powered
            )
        })
    })
    .flatten()
    .unwrap_or_else(|| format!("no weighted ejector at {} {} {}", pos[0], pos[1], pos[2]))
}

/// A weighted ejector's surroundings as its tick reaches them.
struct EjectorWorld<'a, 'g> {
    game: &'a mut Game<'g>,
    dimension: &'a str,
    dim: usize,
    pos: [i32; 3],
}

/// `DirectBeltInputBehaviour.handleInsertion(stack, UP)` at `target`: a depot or ejector (empty,
/// charged), else a belt segment that carries items. The count left, None when nothing there
/// takes items.
fn land(
    game: &mut Game<'_>,
    dim: usize,
    target: [i32; 3],
    item: &str,
    count: u8,
    simulate: bool,
) -> Option<u8> {
    let is_depot = with_world(|w| w.dims[dim].depots.contains_key(&target)) == Some(true);
    if is_depot {
        if !depot_items(game, target).is_empty() || !ejector_accepts(dim, target) {
            return Some(count);
        }
        let n = count.min(stack_limit(game, item));
        if !simulate {
            game.native_with_mod_block_entity(target, |be| {
                be.items
                    .resize(be.items.len().max(create_engine::depot::SLOTS), None);
                be.items[0] = Some(stack_of(item, n));
            })?;
            let h =
                create_engine::depot::Held::new(random_angle(game, target), upright(game, item));
            with_world(|w| w.depot_put(dim, target, Some(h)));
        }
        return Some(count - n);
    }
    let (_, _, seg) = belt_segment(game, dim, target)?;
    if !seg.transports() {
        return None;
    }
    let p = Point {
        pos: target,
        kind: Kind::Belt,
        facing: None,
    };
    Some(arm::World::insert(
        &mut ArmWorld { game, dim },
        &p,
        item,
        count,
        simulate,
    ))
}

impl ejector::World for EjectorWorld<'_, '_> {
    fn present(&mut self) -> ejector::Present {
        let items = depot_items(self.game, self.pos);
        let held = items.iter().any(|s| s.0 == 0);
        let position = with_world(|w| w.dims[self.dim].depots.get(&self.pos).copied().flatten())
            .flatten()
            .map_or(0.5, |h| h.position);
        ejector::Present {
            count: items.iter().map(|s| u32::from(s.2)).sum(),
            held_position: held.then_some(position),
        }
    }

    fn target_refuses(&mut self, target: [i32; 3]) -> bool {
        let Some((_, item, count)) = depot_items(self.game, self.pos)
            .into_iter()
            .find(|s| s.0 == 0)
        else {
            return false;
        };
        land(self.game, self.dim, target, &item, count, true) == Some(count)
    }

    fn take_all(&mut self) -> Vec<(String, u8)> {
        let taken = self
            .game
            .native_with_mod_block_entity(self.pos, |be| {
                be.items
                    .iter_mut()
                    .take(create_engine::depot::SLOTS)
                    .filter_map(Option::take)
                    .map(|s| (s.item.to_string(), s.count.min(255) as u8))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !taken.is_empty() {
            with_world(|w| w.depot_put(self.dim, self.pos, None));
        }
        taken
    }

    fn insert(&mut self, target: [i32; 3], item: &str, count: u8) -> Option<u8> {
        land(self.game, self.dim, target, item, count, false)
    }

    fn drop_item(&mut self, at: [f64; 3], motion: [f64; 3], item: &str, count: u8) {
        self.game.native_run_command(&format!(
            "execute in {} run summon minecraft:item {:.4} {:.4} {:.4} {{Item:{{id:'{item}',count:{count}}},Motion:[{:.4}d,{:.4}d,{:.4}d],PickupDelay:10s}}",
            self.dimension, at[0], at[1], at[2], motion[0], motion[1], motion[2]
        ));
    }

    fn clip(&mut self, from: [f64; 3], to: [f64; 3]) -> Option<([f64; 3], [i32; 3], Dir)> {
        let own = self.pos;
        let game = &*self.game;
        ejector::clip_blocks(from, to, |p| p != own && blocked(game, p))
    }

    fn extracting_funnel(&mut self, pos: [i32; 3]) -> bool {
        funnel_at(self.game, pos).is_some_and(|f| f.belt.is_none() && f.extracting)
    }
}

/// Weighted ejectors that have something to do (`EjectorBlockEntity.tick`): winding up,
/// launching, stacks in flight, or charged with something on them; the rest cost nothing.
/// Stacks of removed ejectors drop; clients see the ejectors whose plate or flight changed.
pub(super) fn tick_ejectors(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let list = with_world(|w| {
        let d = &w.dims[dim];
        if d.ejecting.is_empty() {
            Vec::new()
        } else {
            d.ejecting.clone()
        }
    })
    .unwrap_or_default();
    for pos in list {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let Some(facing) = with_world(|w| w.table.get(state).and_then(|(_, i)| i.facing)).flatten()
        else {
            continue;
        };
        let Some(mut e) = with_world(|w| w.dims[dim].ejectors.remove(&pos)).flatten() else {
            continue;
        };
        let speed = with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0);
        let flying = e.flying.len();
        let changed = e.tick(
            pos,
            facing,
            speed,
            &mut EjectorWorld {
                game,
                dimension,
                dim,
                pos,
            },
        );
        let has_items = !depot_items(game, pos).is_empty();
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
        game.native_run_command(&format!(
            "execute in {dimension} run summon minecraft:item {:.4} {:.4} {:.4} {{Item:{{id:'{item}',count:{n}}},Motion:[{:.4}d,{:.4}d,{:.4}d]}}",
            at[0], at[1], at[2], v[0], v[1], v[2]
        ));
    }
    if !changes.is_empty() {
        let players = players_in(game, dimension);
        send_ejectors(game, &players, dim, &changes);
    }
}

/// An ejector starts ticking: its depot changed, it started turning.
pub(super) fn wake_ejectors(dim: usize, list: &[[i32; 3]]) {
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
pub(super) fn send_ejectors(game: &mut Game<'_>, players: &[u128], dim: usize, list: &[[i32; 3]]) {
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
        for &p in players {
            let _ = game.native_net_send_to_player(p, ejector::CHANNEL, &m);
        }
    }
}

/// A joining player sees every arm and ejector of its level.
pub(super) fn sync_arms(game: &mut Game<'_>, player: u128, dim: usize) {
    let arms: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].arms.arms.keys().copied().collect()).unwrap_or_default();
    send_arms(game, &[player], dim, &arms);
    let ejectors: Vec<[i32; 3]> =
        with_world(|w| w.dims[dim].ejectors.keys().copied().collect()).unwrap_or_default();
    send_ejectors(game, &[player], dim, &ejectors);
}

/// Arms and ejectors saved with their column (`ArmBlockEntity.write`: `InteractionPoints`,
/// `Phase`, `Powered`, `HeldItem`, `TargetPointIndex`, `MovementProgress`, the selection mode;
/// `EjectorBlockEntity.write`: `HorizontalDistance`, `VerticalDistance`, `Powered`, `State`,
/// `Lid`, `LaunchedItems`, the stack size). None when the column has neither.
pub(super) fn arms_to_nbt(dim: usize, chunk: [i32; 2]) -> Option<horizon_world::nbt::Compound> {
    use horizon_world::nbt::{Compound, Tag};
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
    if arms.is_empty() && ejectors.is_empty() {
        return None;
    }
    ejectors.sort_by_key(|e| e.0);
    let mut own = Compound::new();
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
    Some(own)
}

/// What `arms_to_nbt` wrote: arms and ejectors waiting for their blocks.
pub(super) fn arms_from_nbt(dim: usize, data: Option<&horizon_world::nbt::Compound>) {
    use horizon_world::nbt::Tag;
    let Some(data) = data else {
        return;
    };
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
            for (i, pt) in v[10..].chunks_exact(5).enumerate() {
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
            let mut e = ejector::Ejector::default();
            e.launcher = ejector::Launcher::new(v[3], v[4]);
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
            for (f, item) in v[9..].chunks_exact(2).zip(items) {
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
    with_world(|w| {
        let d = &mut w.dims[dim];
        d.arms.saved.extend(arms);
        d.ejector_saved.extend(ejectors);
    });
}

// The client's side.

/// An arm a client draws: the server's last word, how far its move has come here, and the
/// step of the move its joints were last posed at.
#[derive(Clone, Debug)]
pub(super) struct ClientArm {
    shown: arm::Shown,
    progress: f32,
    step: i32,
    block_item: Option<bool>,
}

/// An ejector a client draws: the server's last word, ticks since, and when its plate was last
/// posed.
#[derive(Clone, Debug)]
pub(super) struct ClientEjector {
    shown: ejector::Shown,
    ticks: i32,
}

#[derive(Default)]
pub(super) struct ClientArms {
    arms: HashMap<[i32; 3], ClientArm>,
    ejectors: HashMap<[i32; 3], ClientEjector>,
}

thread_local! {
    pub(super) static ARMS: RefCell<ClientArms> = RefCell::new(ClientArms::default());
}

/// Joint poses per move: the joints are posed again at each eighth of a move (Create lerps
/// them every frame); the base turns smoothly on the GPU.
const STEPS: f32 = 8.0;

fn arm_key(pos: [i32; 3], slot: u8) -> u64 {
    depot_key(pos, 220 + slot, 0)
}

fn ejector_key(pos: [i32; 3], slot: u8) -> u64 {
    depot_key(pos, 240 + slot, 0)
}

fn hide_arm(game: &mut Game<'_>, pos: [i32; 3]) {
    for slot in 0..7 {
        game.native_part_clear(arm_key(pos, slot));
    }
}

fn hide_ejector(game: &mut Game<'_>, pos: [i32; 3]) {
    for slot in 0..17 {
        game.native_part_clear(ejector_key(pos, slot));
    }
}

/// A partial's `up` state, as the parts are posed from it.
fn partial(game: &Game<'_>, name: &str) -> Option<horizon_world::registry::BlockStateId> {
    state_with(game.pack.assets.registry, name, &[("facing", "up")])
}

fn client_speed(pos: [i32; 3]) -> f32 {
    CLIENT.with_borrow(|c| {
        c.as_ref()
            .and_then(|c| c.speeds.get(&pos))
            .map_or(0.0, |s| s[0])
    })
}

/// Draws an arm as `ArmRenderer` poses it at its progress: its joints at the current step of the
/// move, every part turned about the block's vertical axis by the base's yaw, which glides on the
/// GPU while it moves.
fn draw_arm(game: &mut Game<'_>, a: &mut ClientArm) {
    let s = &a.shown;
    let pos = s.pos;
    let step = (a.progress * STEPS).floor() as i32;
    a.step = step;
    let posed = (step as f32 / STEPS).min(1.0);
    let joints = arm::at_progress(s.from, s.from_base, s.to, posed);
    let now = arm::at_progress(s.from, s.from_base, s.to, a.progress);
    let speed = client_speed(pos);
    let moving = a.progress < 1.0 && speed != 0.0;
    // The base's yaw, degrees a second, as the GPU turns the parts (rpm × 6).
    let diff =
        s.to.map_or(0.0, |t| arm::shortest_angle_diff(s.from_base, t.base));
    let per_second = if moving {
        diff * arm::step(speed) * 20.0
    } else {
        0.0
    };
    let sign = if s.ceiling { -1.0 } else { 1.0 };
    let rpm = sign * per_second / 6.0;
    let seconds = game.native_parts_seconds();
    let phase = sign * now.base - rpm * 6.0 * seconds;
    if a.block_item.is_none()
        && let Some(item) = &s.held
    {
        a.block_item = Some(is_block_item(game, item));
    }
    let held = s.held.as_ref().map(|_| a.block_item.unwrap_or(false));
    let p = arm::poses(&joints, s.ceiling, held);
    let parts = [
        ("create:part_arm_base", p.base),
        ("create:part_arm_lower_body", p.lower),
        ("create:part_arm_upper_body", p.upper),
        ("create:part_arm_claw_base", p.claw),
        ("create:part_arm_upper_claw_grip", p.upper_grip),
        ("create:part_arm_lower_claw_grip", p.lower_grip),
    ];
    for (slot, (name, pose)) in parts.into_iter().enumerate() {
        let Some(state) = partial(game, name) else {
            continue;
        };
        if let Some(model) = game.native_part_model_posed(state, pose) {
            game.native_part_set(arm_key(pos, slot as u8), model, pos, 1, rpm, phase);
        }
    }
    let item = s.held.clone();
    match item.and_then(|i| game.native_part_item_model(&i, p.item)) {
        Some(model) => game.native_part_set(arm_key(pos, 6), model, pos, 1, rpm, phase),
        None => game.native_part_clear(arm_key(pos, 6)),
    }
}

/// Arms from the server.
pub(super) fn arms_from_server(game: &mut Game<'_>, data: &[u8]) {
    for s in arm::decode(data) {
        let pos = s.pos;
        if s.gone {
            hide_arm(game, pos);
            ARMS.with_borrow_mut(|a| a.arms.remove(&pos));
            continue;
        }
        let mut a = ClientArm {
            progress: s.progress,
            shown: s,
            step: -1,
            block_item: None,
        };
        draw_arm(game, &mut a);
        ARMS.with_borrow_mut(|c| c.arms.insert(pos, a));
    }
}

/// Ejectors from the server.
pub(super) fn ejectors_from_server(game: &mut Game<'_>, data: &[u8]) {
    for s in ejector::decode(data) {
        let pos = s.pos;
        if s.gone {
            hide_ejector(game, pos);
            ARMS.with_borrow_mut(|a| a.ejectors.remove(&pos));
            continue;
        }
        let mut e = ClientEjector { shown: s, ticks: 0 };
        draw_ejector(game, &mut e, true);
        ARMS.with_borrow_mut(|c| c.ejectors.insert(pos, e));
    }
}

/// Draws an ejector's plate (`EjectorRenderer.applyLidAngle`: turned to its facing, swung about
/// its hinge by `lid × 70°`; winding back it turns on the GPU) and, when `plate` is false, only
/// its stacks in flight: each glides from where it is to where it will be a tick from now along
/// the arc, tumbling (`rotateXDegrees(time × 40)`) about the axis across its facing.
fn draw_ejector(game: &mut Game<'_>, e: &mut ClientEjector, plate: bool) {
    let s = &e.shown;
    let pos = s.pos;
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    let Some(facing) = CLIENT.with_borrow(|c| {
        c.as_ref()
            .and_then(|c| c.table.get(state).and_then(|(_, i)| i.facing))
    }) else {
        return;
    };
    let seconds = game.native_parts_seconds();
    let centre = glam::Vec3::splat(0.5);
    let turn = ejector::horizontal_angle(facing);
    if plate && let Some(top) = partial(game, "create:part_ejector_top") {
        let (axis, sign, hinge) = ejector::lid_hinge(facing);
        let hinge = glam::Vec3::from(hinge);
        // Turned to its facing about the centre, then moved so the hinge sits at the centre:
        // the GPU turns it there, and the offset puts the hinge back.
        let pose = glam::Mat4::from_translation(centre - hinge)
            * glam::Mat4::from_translation(centre)
            * glam::Mat4::from_rotation_y((180.0 + turn).to_radians())
            * glam::Mat4::from_translation(-centre);
        let angle = s.lid * 70.0;
        let rpm = match s.state {
            // Winding back toward closed at `wind_up` a tick.
            ejector::State::Retracting if s.lid > 0.0 => -sign * s.wind_up * 70.0 * 20.0 / 6.0,
            _ => 0.0,
        };
        let phase = sign * angle - rpm * 6.0 * seconds;
        let off = hinge - centre;
        if let Some(model) = game.native_part_model_posed(top, pose) {
            game.native_part_glide(
                ejector_key(pos, 0),
                model,
                pos,
                axis,
                rpm,
                phase,
                [off.x, off.y, off.z, seconds],
                [0.0; 3],
            );
        }
    }
    // Stacks in flight.
    let launcher = ejector::Launcher::new(s.h, s.v);
    let max = s.max_time();
    let item_offset = glam::Vec3::new(0.0, 2.0 / 16.0, -1.0 / 16.0);
    // The tumble's axis: x turned to the facing (`rotateYDegrees(horizontalAngle)`).
    let x = ejector::rotate_y([1.0, 0.0, 0.0], f64::from(turn));
    let (axis, sign) = if x[0].abs() > 0.5 {
        (0u8, x[0].signum() as f32)
    } else {
        (2u8, x[2].signum() as f32)
    };
    let away = facing.opposite();
    for slot in 0..16u8 {
        let flight = s.flying.get(usize::from(slot));
        let Some((t0, item)) = flight else {
            game.native_part_clear(ejector_key(pos, 1 + slot));
            continue;
        };
        let time = (*t0 + e.ticks) as f32;
        if time > max {
            game.native_part_clear(ejector_key(pos, 1 + slot));
            continue;
        }
        let here = launcher.global_pos(f64::from(time), away, pos);
        let next = launcher.global_pos(f64::from((time + 1.0).min(max)), away, pos);
        let rel = glam::Vec3::new(
            (here[0] - f64::from(pos[0])) as f32,
            (here[1] - f64::from(pos[1])) as f32,
            (here[2] - f64::from(pos[2])) as f32,
        );
        let velocity = [0, 1, 2].map(|i| ((next[i] - here[i]) * 20.0) as f32);
        let pose = glam::Mat4::from_translation(centre)
            * glam::Mat4::from_scale(glam::Vec3::splat(0.5))
            * glam::Mat4::from_rotation_y(turn.to_radians())
            * glam::Mat4::from_translation(-item_offset);
        let Some(model) = game.native_part_item_model(item, pose) else {
            continue;
        };
        let rpm = sign * 40.0 * 20.0 / 6.0;
        let phase = sign * time * 40.0 - rpm * 6.0 * seconds;
        let off = rel + item_offset - centre;
        game.native_part_glide(
            ejector_key(pos, 1 + slot),
            model,
            pos,
            axis,
            rpm,
            phase,
            [off.x, off.y, off.z, seconds],
            velocity,
        );
    }
}

/// Each client tick: moving arms move on (their joints posed again at each step, the base
/// already turning on the GPU); ejectors with stacks in flight move them on, and a plate winding
/// back stops when closed.
pub(super) fn tick_client_arms(game: &mut Game<'_>) {
    if !game.native_playing() {
        ARMS.with_borrow_mut(|a| *a = ClientArms::default());
        return;
    }
    let moving: Vec<[i32; 3]> = ARMS.with_borrow(|a| {
        a.arms
            .iter()
            .filter(|(_, x)| x.progress < 1.0)
            .map(|(p, _)| *p)
            .collect()
    });
    for pos in moving {
        let Some(mut a) = ARMS.with_borrow_mut(|c| c.arms.remove(&pos)) else {
            continue;
        };
        let speed = client_speed(pos);
        a.progress = (a.progress + arm::step(speed)).min(1.0);
        let step = (a.progress * STEPS).floor() as i32;
        if step != a.step || a.progress >= 1.0 {
            draw_arm(game, &mut a);
        }
        ARMS.with_borrow_mut(|c| c.arms.insert(pos, a));
    }
    let busy: Vec<[i32; 3]> = ARMS.with_borrow(|a| {
        a.ejectors
            .iter()
            .filter(|(_, e)| {
                !e.shown.flying.is_empty()
                    || (e.shown.state == ejector::State::Retracting
                        && e.shown.lid > 0.0
                        && e.shown.wind_up > 0.0)
            })
            .map(|(p, _)| *p)
            .collect()
    });
    for pos in busy {
        let Some(mut e) = ARMS.with_borrow_mut(|c| c.ejectors.remove(&pos)) else {
            continue;
        };
        e.ticks += 1;
        let mut plate = false;
        if e.shown.state == ejector::State::Retracting && e.shown.lid > 0.0 {
            let lid = e.shown.lid - e.shown.wind_up;
            if lid <= 0.0 {
                e.shown.lid = 0.0;
                plate = true;
            } else {
                e.shown.lid = lid;
            }
        }
        let max = e.shown.max_time();
        let landed = e
            .shown
            .flying
            .iter()
            .all(|(t, _)| (*t + e.ticks) as f32 > max + 1.0);
        draw_ejector(game, &mut e, plate);
        if landed {
            e.shown.flying.clear();
        }
        ARMS.with_borrow_mut(|c| c.ejectors.insert(pos, e));
    }
}
