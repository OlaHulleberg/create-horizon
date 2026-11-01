//! Create's item vault (`ItemVaultBlock`, `ItemVaultBlockEntity`) and the multiblock rules it
//! shares with tanks (`ConnectivityHandler`, horizontal case): vault blocks along one horizontal
//! axis join into one vault up to 3 wide and 3 × its width long, whose inventory is the slots of
//! its blocks in order, the controller's first (`initCapability`). Each block's 20 slots
//! (`vaultCapacity`) are its block entity's; this keeps only who belongs to which vault, found
//! again when a vault block is placed or removed, never per tick.

use crate::kinetics::Axis;
use std::collections::HashMap;

/// `vaultCapacity`: the stacks a vault block holds.
pub const SLOTS: usize = 20;
/// `getMaxWidth`.
pub const MAX_WIDTH: i32 = 3;

/// `ItemVaultBlockEntity.getMaxLength(radius)`.
pub fn max_length(width: i32) -> i32 {
    width * 3
}

/// A vault block's place in its vault (`controller`, `radius`, `length`, `updateConnectivity`),
/// with its block's axis and `large`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub axis: Axis,
    /// None: its own controller.
    pub controller: Option<[i32; 3]>,
    pub width: i32,
    pub length: i32,
    /// `updateConnectivity`: it looks for its vault at its next tick.
    pub dirty: bool,
    pub large: bool,
}

impl Part {
    pub fn new(axis: Axis, large: bool) -> Part {
        Part {
            axis,
            controller: None,
            width: 1,
            length: 1,
            dirty: false,
            large,
        }
    }
}

/// The block at `y` along, `x` and `z` across a vault of `axis` from `origin` (the offsets of
/// `tryToFormNewMultiOfWidth` and `initCapability`).
pub fn offset(origin: [i32; 3], axis: Axis, y: i32, x: i32, z: i32) -> [i32; 3] {
    match axis {
        Axis::X => [origin[0] + y, origin[1] + x, origin[2] + z],
        Axis::Y => [origin[0] + x, origin[1] + y, origin[2] + z],
        Axis::Z => [origin[0] + x, origin[1] + z, origin[2] + y],
    }
}

/// `ConnectivityHandler.SearchCache`: each position's controller, as first found.
type Cache = HashMap<[i32; 3], Option<[i32; 3]>>;

/// One dimension's vault blocks.
#[derive(Default)]
pub struct Vaults {
    pub parts: HashMap<[i32; 3], Part>,
    /// What was saved with a column, waiting for its block.
    saved: HashMap<[i32; 3], Part>,
    /// Blocks whose `updateConnectivity` was set, in that order.
    queue: Vec<[i32; 3]>,
    /// `large` to set on blocks (`notifyMultiUpdated`, `removeController`).
    pub large: Vec<([i32; 3], bool)>,
    /// Blocks whose part changed, for their columns to be saved.
    pub changed: Vec<[i32; 3]>,
}

impl Vaults {
    /// A vault block in the world (placed, or its column loaded): what was saved for it, else a
    /// vault of its own that looks for its neighbours (`onPlace` -> `updateConnectivity`; a
    /// block without saved data is `Uninitialized`).
    pub fn placed(&mut self, pos: [i32; 3], axis: Axis, large: bool) {
        if let Some(p) = self.parts.get_mut(&pos) {
            p.large = large;
            return;
        }
        let part = match self.saved.remove(&pos) {
            Some(mut p) => {
                p.axis = axis;
                p.large = large;
                p
            }
            None => Part {
                dirty: true,
                ..Part::new(axis, large)
            },
        };
        if part.dirty {
            self.queue.push(pos);
        }
        self.parts.insert(pos, part);
    }

    /// `ItemVaultBlock.onRemove`: the block is gone, its vault splits; the others look for a
    /// vault again at their next tick.
    pub fn removed(&mut self, pos: [i32; 3]) {
        let Some(part) = self.parts.get(&pos).copied() else {
            return;
        };
        let controller = self.controller_of(pos);
        self.parts.remove(&pos);
        self.changed.push(pos);
        match controller {
            Some(c) if c == pos => self.split_from(pos, part, None),
            Some(c) => self.split(c, None),
            None => {}
        }
    }

    /// A column left the level: its blocks are forgotten (no split: they are not gone).
    pub fn column_unloaded(&mut self, cx: i32, cz: i32) {
        let out = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (cx, cz);
        self.parts.retain(|p, _| !out(p));
        self.saved.retain(|p, _| !out(p));
        self.queue.retain(|p| !out(p));
    }

    /// Parts saved with a column, for when their blocks are found.
    pub fn load(&mut self, saved: impl Iterator<Item = ([i32; 3], Part)>) {
        self.saved.extend(saved);
    }

    /// The parts of a column, to save.
    pub fn column(&self, cx: i32, cz: i32) -> Vec<([i32; 3], Part)> {
        let mut v: Vec<([i32; 3], Part)> = self
            .parts
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
            .map(|(p, x)| (*p, *x))
            .collect();
        v.sort_by_key(|e| e.0);
        v
    }

    /// `isController`.
    pub fn is_controller(&self, pos: [i32; 3]) -> bool {
        self.parts
            .get(&pos)
            .is_some_and(|p| p.controller.is_none_or(|c| c == pos))
    }

    /// `getController`: its controller's position, itself for a controller.
    fn controller_pos(&self, pos: [i32; 3]) -> Option<[i32; 3]> {
        let p = self.parts.get(&pos)?;
        Some(p.controller.unwrap_or(pos))
    }

    /// `getControllerBE`: the controller of the block at `pos`, if it is there.
    pub fn controller_of(&self, pos: [i32; 3]) -> Option<[i32; 3]> {
        let c = self.controller_pos(pos)?;
        self.parts.contains_key(&c).then_some(c)
    }

    /// The blocks whose slots make up the inventory of the vault at `pos`, in order
    /// (`initCapability`: layer by layer along the vault from its controller); None for no vault
    /// there.
    pub fn inventory(&self, pos: [i32; 3]) -> Option<Vec<[i32; 3]>> {
        let c = self.controller_of(pos)?;
        let p = self.parts[&c];
        let mut out = Vec::with_capacity((p.width * p.width * p.length) as usize);
        for y in 0..p.length {
            for x in 0..p.width {
                for z in 0..p.width {
                    let at = offset(c, p.axis, y, x, z);
                    if self.parts.contains_key(&at) {
                        out.push(at);
                    }
                }
            }
        }
        Some(out)
    }

    /// The vault blocks' ticks: those whose `updateConnectivity` is set look for their vault
    /// (`updateConnectivity`: controllers only). Nothing when no vault block changed.
    pub fn tick(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        for pos in std::mem::take(&mut self.queue) {
            let Some(p) = self.parts.get_mut(&pos) else {
                continue;
            };
            if !p.dirty {
                continue;
            }
            p.dirty = false;
            self.changed.push(pos);
            if self.is_controller(pos) {
                self.form(pos);
            }
        }
    }

    /// `ConnectivityHandler.formMulti` from the block at `start`: the vault blocks reached from
    /// it toward negative x, y and z (at most 3 below it) each try to form the largest vault
    /// they can lead, the largest first.
    pub fn form(&mut self, start: [i32; 3]) {
        let mut cache = Cache::new();
        let min_y = start[1] - MAX_WIDTH;
        let mut frontier = std::collections::VecDeque::from([start]);
        let mut visited = std::collections::HashSet::new();
        let mut creation: Vec<(i32, [i32; 3])> = Vec::new();
        while let Some(pos) = frontier.pop_front() {
            if !visited.insert(pos) {
                continue;
            }
            let amount = self.try_form(pos, &mut cache, true);
            if amount > 1 {
                creation.push((amount, pos));
            }
            for d in [[-1, 0, 0], [0, -1, 0], [0, 0, -1]] {
                let next = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
                if next[1] <= min_y || visited.contains(&next) {
                    continue;
                }
                if self.parts.contains_key(&next) {
                    frontier.push_back(next);
                }
            }
        }
        // The creation queue: the largest first (stable among equals).
        creation.sort_by_key(|c| std::cmp::Reverse(c.0));
        visited.clear();
        for (_, pos) in creation {
            if visited.insert(pos) {
                self.try_form(pos, &mut cache, false);
            }
        }
    }

    /// `tryToFormNewMulti`: the width (1 to 3, wider on a tie) that holds the most blocks, and
    /// that many; unless simulating, the vault becomes that one.
    fn try_form(&mut self, pos: [i32; 3], cache: &mut Cache, simulate: bool) -> i32 {
        if !self.is_controller(pos) {
            return 0;
        }
        let (mut best_width, mut best_amount) = (1, -1);
        for w in 1..=MAX_WIDTH {
            let amount = self.try_width(pos, w, cache, true);
            if amount < best_amount {
                continue;
            }
            best_width = w;
            best_amount = amount;
        }
        if !simulate {
            let p = self.parts[&pos];
            if p.width == best_width && p.width * p.width * p.length == best_amount {
                return best_amount;
            }
            self.split(pos, Some(cache));
            self.try_width(pos, best_width, cache, false);
            let p = self.parts.get_mut(&pos).expect("a controller");
            p.dirty = false;
            p.width = best_width;
            p.length = (best_amount / (best_width * best_width)).max(1);
            self.notify(pos);
        }
        best_amount
    }

    /// `getOrCache`: the controller of the vault block at `pos`, if both are there.
    fn controller_cached(&self, pos: [i32; 3], cache: &mut Cache) -> Option<[i32; 3]> {
        *cache.entry(pos).or_insert_with(|| self.controller_of(pos))
    }

    /// `tryToFormNewMultiOfWidth`: how many blocks a vault `width` wide led from `origin` would
    /// hold, layer by layer along its axis while each layer is whole and no other vault crosses
    /// it; unless simulating, those blocks join it.
    fn try_width(
        &mut self,
        origin: [i32; 3],
        width: i32,
        cache: &mut Cache,
        simulate: bool,
    ) -> i32 {
        let axis = self.parts[&origin].axis;
        let max = max_length(width);
        let (mut amount, mut height) = (0, 0);
        'search: for y in 0..max {
            for x in 0..width {
                for z in 0..width {
                    let pos = offset(origin, axis, y, x, z);
                    let Some(c) = self.controller_cached(pos, cache) else {
                        break 'search;
                    };
                    let Some(con) = self.parts.get(&c).copied() else {
                        break 'search;
                    };
                    if con.width > width || con.width == width && con.length == max {
                        break 'search;
                    }
                    if con.axis != axis {
                        break 'search;
                    }
                    if c != origin {
                        let w = con.width;
                        // A horizontal vault (Create's ItemVault case).
                        if axis == Axis::Z && c[0] < origin[0]
                            || c[1] < origin[1]
                            || axis == Axis::X && c[2] < origin[2]
                            || axis == Axis::Z && c[0] + w > origin[0] + width
                            || c[1] + w > origin[1] + width
                            || axis == Axis::X && c[2] + w > origin[2] + width
                        {
                            break 'search;
                        }
                    }
                }
            }
            amount += width * width;
            height += 1;
        }
        if simulate {
            return amount;
        }
        for y in 0..height {
            for x in 0..width {
                for z in 0..width {
                    let pos = offset(origin, axis, y, x, z);
                    if pos == origin || !self.parts.contains_key(&pos) {
                        continue;
                    }
                    self.split(pos, Some(cache));
                    let p = self.parts.get_mut(&pos).expect("a vault block");
                    p.controller = Some(origin);
                    p.dirty = false;
                    p.length = height;
                    p.width = width;
                    cache.insert(pos, Some(origin));
                    self.notify(pos);
                }
            }
        }
        amount
    }

    /// `splitMultiAndInvalidate` (no reconnecting): every block of the vault of `pos` becomes a
    /// vault of its own that looks for its vault again.
    fn split(&mut self, pos: [i32; 3], cache: Option<&mut Cache>) {
        let Some(c) = self.controller_of(pos) else {
            return;
        };
        let part = self.parts[&c];
        self.split_from(c, part, cache);
    }

    fn split_from(&mut self, origin: [i32; 3], part: Part, mut cache: Option<&mut Cache>) {
        if part.width == 1 && part.length == 1 {
            return;
        }
        for y in 0..part.length {
            for x in 0..part.width {
                for z in 0..part.width {
                    let pos = offset(origin, part.axis, y, x, z);
                    if self.controller_pos(pos) != Some(origin) {
                        continue;
                    }
                    self.remove_controller(pos);
                    if let Some(cache) = cache.as_deref_mut() {
                        cache.insert(pos, Some(pos));
                    }
                }
            }
        }
    }

    /// `removeController`: a vault of its own, not large, looking for its vault again.
    fn remove_controller(&mut self, pos: [i32; 3]) {
        let Some(p) = self.parts.get_mut(&pos) else {
            return;
        };
        p.controller = None;
        p.width = 1;
        p.length = 1;
        p.dirty = true;
        let was_large = std::mem::replace(&mut p.large, false);
        self.queue.push(pos);
        self.changed.push(pos);
        if was_large {
            self.large.push((pos, false));
        }
    }

    /// `notifyMultiUpdated`: large when 3 wide.
    fn notify(&mut self, pos: [i32; 3]) {
        let Some(p) = self.parts.get_mut(&pos) else {
            return;
        };
        let large = p.width > 2;
        self.changed.push(pos);
        if p.large != large {
            p.large = large;
            self.large.push((pos, large));
        }
    }
}

impl crate::world::CreateWorld {
    /// The vault blocks' tick in `dim`: blocks that look for their vault do; the columns of blocks
    /// whose vault changed are saved. The `large` to set on blocks.
    pub fn tick_vaults(&mut self, dim: usize) -> Vec<([i32; 3], bool)> {
        let d = &mut self.dims[dim];
        d.vaults.tick();
        if !d.vaults.changed.is_empty() {
            for p in d.vaults.changed.drain(..) {
                d.unsaved.push((p[0] >> 4, p[2] >> 4));
            }
        }
        std::mem::take(&mut d.vaults.large)
    }
}

/// `ItemHandlerHelper.insertItemStacked` into slots (each empty, or holding `(the same item and
/// components as the stack, count)`) of a stack of `count` at most `limit` to a slot
/// (`min(64, max stack size)`): first onto the same items, then into empty slots, in slot order.
/// The count each slot takes, in order.
pub fn insert_stacked(
    slots: &[Option<(bool, u32)>],
    mut count: u32,
    limit: u32,
) -> Vec<(usize, u32)> {
    let mut out = Vec::new();
    if limit > 1 {
        for (i, s) in slots.iter().enumerate() {
            if count == 0 {
                break;
            }
            if let Some((true, n)) = *s
                && n < limit
            {
                let put = count.min(limit - n);
                out.push((i, put));
                count -= put;
            }
        }
    }
    for (i, s) in slots.iter().enumerate() {
        if count == 0 {
            break;
        }
        if s.is_none() {
            let put = count.min(limit);
            out.push((i, put));
            count -= put;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vaults(blocks: &[[i32; 3]], axis: Axis) -> Vaults {
        let mut v = Vaults::default();
        for &p in blocks {
            v.placed(p, axis, false);
            v.tick();
        }
        v
    }

    fn cube(w: i32, len: i32, axis: Axis) -> Vec<[i32; 3]> {
        let mut out = Vec::new();
        for y in 0..len {
            for x in 0..w {
                for z in 0..w {
                    out.push(offset([0, 64, 0], axis, y, x, z));
                }
            }
        }
        out
    }

    #[test]
    fn a_2x2x3_vault_forms_as_its_blocks_are_placed() {
        let blocks = cube(2, 3, Axis::Z);
        let v = vaults(&blocks, Axis::Z);
        let c = v.controller_of([1, 65, 2]).unwrap();
        assert_eq!(c, [0, 64, 0], "the lowest, most negative corner leads");
        let p = v.parts[&c];
        assert_eq!((p.width, p.length), (2, 3));
        assert!(blocks.iter().all(|b| v.controller_of(*b) == Some(c)));
        assert!(v.parts.values().all(|p| !p.large), "only 3 wide is large");
        let inv = v.inventory([1, 64, 1]).unwrap();
        assert_eq!(inv.len(), 12);
        assert_eq!(inv.len() * SLOTS, 240, "12 blocks × 20 stacks");
        // Layer by layer along the vault (z); in each layer x, then y.
        assert_eq!(
            &inv[..5],
            &[[0, 64, 0], [0, 65, 0], [1, 64, 0], [1, 65, 0], [0, 64, 1]]
        );
    }

    #[test]
    fn three_wide_vaults_are_large_and_at_most_nine_long() {
        let blocks = cube(3, 10, Axis::X);
        let v = vaults(&blocks, Axis::X);
        let p = v.parts[&[0, 64, 0]];
        assert_eq!((p.width, p.length), (3, 9), "max length is 3 × width");
        assert!(v.parts[&[8, 66, 2]].large);
        assert_eq!(v.controller_of([8, 66, 2]), Some([0, 64, 0]));
        // The tenth layer leads a vault of its own, 3 × 3 × 1.
        assert_eq!(v.controller_of([9, 66, 2]), Some([9, 64, 0]));
        assert_eq!(
            (v.parts[&[9, 64, 0]].width, v.parts[&[9, 64, 0]].length),
            (3, 1)
        );
        let large: Vec<bool> = v
            .large
            .iter()
            .filter(|l| l.0 == [8, 66, 2])
            .map(|l| l.1)
            .collect();
        assert_eq!(large.last(), Some(&true));
    }

    #[test]
    fn blocks_along_another_axis_stay_apart() {
        let mut v = vaults(&[[0, 64, 0], [0, 64, 1]], Axis::Z);
        assert_eq!(v.parts[&[0, 64, 0]].length, 2);
        v.placed([1, 64, 0], Axis::X, false);
        v.tick();
        assert_eq!(v.controller_of([1, 64, 0]), Some([1, 64, 0]));
        assert_eq!(v.parts[&[0, 64, 0]].length, 2);
    }

    #[test]
    fn removing_a_block_splits_the_vault_and_the_rest_form_again() {
        let blocks = cube(2, 3, Axis::Z);
        let mut v = vaults(&blocks, Axis::Z);
        // A block of the middle layer goes: the vault splits, then (next tick) the blocks form
        // what still fits, in the order they were split: the first layer leads a 2 × 2 × 1 (the
        // middle layer is broken), then the middle layer's blocks each lead a 1 × 1 × 2 with the
        // block behind them; the last block of the far layer is left alone.
        v.removed([1, 65, 1]);
        assert!(v.parts.values().all(|p| p.width == 1 && p.length == 1));
        v.tick();
        let c = v.parts[&[0, 64, 0]];
        assert_eq!((c.width, c.length), (2, 1));
        for front in [[0, 64, 1], [0, 65, 1], [1, 64, 1]] {
            assert_eq!(v.parts[&front].length, 2);
            assert_eq!(v.controller_of([front[0], front[1], 2]), Some(front));
        }
        assert_eq!(v.controller_of([1, 65, 2]), Some([1, 65, 2]));
        // The controller going splits it too.
        v.removed([0, 64, 0]);
        v.tick();
        assert!(v.controller_of([1, 64, 0]).is_some());
        assert_ne!(v.controller_of([1, 64, 0]), Some([0, 64, 0]));
    }

    #[test]
    fn idle_vaults_do_nothing() {
        let mut v = vaults(&cube(2, 2, Axis::Z), Axis::Z);
        v.changed.clear();
        v.large.clear();
        v.tick();
        assert!(v.changed.is_empty() && v.large.is_empty() && v.queue.is_empty());
    }

    #[test]
    fn saved_parts_come_back_without_forming() {
        let v = vaults(&cube(2, 1, Axis::Z), Axis::Z);
        let saved = v.column(0, 0);
        let mut back = Vaults::default();
        back.load(saved.into_iter());
        for p in cube(2, 1, Axis::Z) {
            back.placed(p, Axis::Z, false);
        }
        assert!(back.queue.is_empty());
        assert_eq!(back.controller_of([1, 65, 0]), Some([0, 64, 0]));
    }

    #[test]
    fn the_world_forms_vaults_on_block_changes_and_tracks_smart_chutes() {
        use crate::states;
        use crate::world::{CreateWorld, StateTable};
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:item_vault", vec![("axis", "z"), ("large", "false")]),
            ("create:smart_chute", vec![("powered", "false")]),
            ("create:smart_chute", vec![("powered", "true")]),
        ];
        states::set_range(10, 3);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        for p in cube(2, 3, Axis::Z) {
            w.block_changed(0, p, 0, 10);
            assert!(w.tick_vaults(0).is_empty(), "2 wide: never large");
        }
        assert_eq!(w.dims[0].vaults.inventory([1, 65, 2]).unwrap().len(), 12);
        assert!(!w.take_unsaved(0).is_empty(), "the parts are saved");
        // Idle: nothing to do, nothing saved.
        w.tick_vaults(0);
        assert!(w.tick_vaults(0).is_empty() && w.take_unsaved(0).is_empty());
        // Broken: the vault splits.
        w.block_changed(0, [0, 64, 0], 10, 0);
        w.tick_vaults(0);
        assert_ne!(w.dims[0].vaults.controller_of([1, 65, 2]), Some([0, 64, 0]));
        // A smart chute placed is checked for power and looks at the inventory above it; its
        // filter is saved and wakes it; powered, its neighbours changing still wakes it (the
        // glue then finds it powered and leaves it).
        let chute = [5, 64, 5];
        w.block_changed(0, chute, 0, 12);
        assert_eq!(w.take_redstone(0), [chute]);
        assert_eq!(w.powered(12), Some(true));
        assert_eq!(w.dims[0].chute_pulls.take_due(0), [chute]);
        w.set_chute_filter(0, chute, Some("minecraft:iron_ingot".into()));
        assert_eq!(w.take_unsaved(0), [(0, 0)]);
        assert_eq!(w.dims[0].chute_pulls.take_due(1), [chute]);
        w.neighbor_changed(0, chute, 12);
        assert_eq!(w.take_redstone(0), [chute]);
        // Broken: its filter goes.
        w.block_changed(0, chute, 11, 0);
        assert!(w.dims[0].chute_filters.is_empty());
        states::clear_range();
    }

    #[test]
    fn stacks_go_onto_the_same_items_first_then_into_empty_slots() {
        let slots = [
            None,
            Some((false, 5)),
            Some((true, 60)),
            None,
            Some((true, 64)),
        ];
        assert_eq!(insert_stacked(&slots, 30, 64), [(2, 4), (0, 26)]);
        // Unstackable: one to each empty slot.
        assert_eq!(insert_stacked(&slots, 3, 1), [(0, 1), (3, 1)]);
        // Full: nothing goes in.
        assert!(insert_stacked(&[Some((false, 1))], 5, 64).is_empty());
    }
}
