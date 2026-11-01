//! Create's chute (`ChuteBlockEntity`), without fans: one stack falls through each chute at
//! gravity's 4 chutes a second, into the inventory below, the chute below, or out of the bottom;
//! an empty chute takes from the inventory above it (`handleInputFromAbove`). The smart chute
//! (`SmartChuteBlockEntity`) adds a filter and stops while powered.

/// `getItemMotion` with no fan: `-gravity / 20` chutes a tick.
pub const MOTION: f32 = -4.0 / 20.0;

/// What a tick of a falling stack does at `position` (1 at the top, 0 at the bottom).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fall {
    /// Still falling, now here.
    At(f32),
    /// At the bottom: hand it on now (`handleDownwardOutput`).
    Out,
}

/// The downward part of `tick`: past the middle it only goes on when `can_output` (the place
/// below takes it); past the bottom it leaves.
pub fn fall(position: f32, can_output: bool) -> Fall {
    let next = position + MOTION;
    if next < 0.5 {
        if !can_output {
            return Fall::At(0.5);
        }
        if next < 0.0 {
            return Fall::Out;
        }
    }
    Fall::At(next)
}

/// `ChuteBlockEntity.getExtractionAmount`: a chute takes up to 16 at once.
pub const CHUTE_AMOUNT: u32 = 16;
/// `SmartChuteBlockEntity.getExtractionAmount` with the filter's default count (64, "up to"):
/// the filter's count is set on a value board, not ported, so always up to 64.
pub const SMART_AMOUNT: u32 = 64;
/// How often an empty chute under a container that does not tell when it changes (a vanilla
/// chest) looks into it again, in ticks. Create looks every tick, waiting on an inventory version
/// a vanilla container lacks; a funnel's `defaultExtractionTimer` here. Containers of Create's
/// own blocks (vaults, depots) wake the chute when their contents change instead.
pub const RETRY: i64 = 8;

/// Empty chutes that may take from the inventory above them (`handleInputFromAbove`): those to
/// look now (placed, emptied, a neighbour or the inventory changed), those to look again later
/// (under a container that does not tell when it changes; all wait `RETRY`, so the queue stays in
/// time order and only its front is looked at), and those asleep until their inventory changes.
/// Chutes with nothing above them are in none: they cost nothing.
#[derive(Default)]
pub struct Pulls {
    now: Vec<[i32; 3]>,
    later: std::collections::VecDeque<([i32; 3], i64)>,
    asleep: Vec<[i32; 3]>,
}

impl Pulls {
    /// The chute looks at the inventory above it in this tick's pass.
    pub fn wake(&mut self, chute: [i32; 3]) {
        self.now.push(chute);
    }

    /// Nothing to take now from a container that will say when it changes.
    pub fn sleep(&mut self, chute: [i32; 3]) {
        if !self.asleep.contains(&chute) {
            self.asleep.push(chute);
        }
    }

    /// Nothing to take now from a container that won't: look again `RETRY` ticks after `now`.
    pub fn retry(&mut self, chute: [i32; 3], now: i64) {
        self.later.push_back((chute, now + RETRY));
    }

    /// The chutes to look at in the tick at `time`, each once.
    pub fn take_due(&mut self, time: i64) -> Vec<[i32; 3]> {
        while self.later.front().is_some_and(|l| l.1 <= time) {
            let (c, _) = self.later.pop_front().expect("a front");
            self.now.push(c);
        }
        if self.now.is_empty() {
            return Vec::new();
        }
        let mut v = std::mem::take(&mut self.now);
        v.sort_unstable();
        v.dedup();
        self.asleep.retain(|a| v.binary_search(a).is_err());
        v
    }

    /// The contents of the container at `pos` changed: the chute asleep under it wakes, and those
    /// asleep under the same vault (`same(source)`).
    pub fn source_changed(&mut self, pos: [i32; 3], same: impl Fn([i32; 3]) -> bool) {
        let now = &mut self.now;
        self.asleep.retain(|c| {
            let source = [c[0], c[1] + 1, c[2]];
            let wake = source == pos || same(source);
            if wake {
                now.push(*c);
            }
            !wake
        });
    }

    /// The chute is gone.
    pub fn forget(&mut self, chute: [i32; 3]) {
        self.asleep.retain(|c| *c != chute);
        self.later.retain(|c| c.0 != chute);
    }

    /// A column left the level.
    pub fn column_unloaded(&mut self, cx: i32, cz: i32) {
        let out = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (cx, cz);
        self.now.retain(|p| !out(p));
        self.asleep.retain(|p| !out(p));
        self.later.retain(|p| !out(&p.0));
    }

    /// Whether no chute waits for anything (for tests).
    pub fn is_idle(&self) -> bool {
        self.now.is_empty() && self.later.is_empty()
    }
}

impl crate::world::CreateWorld {
    /// `FilteringBehaviour.setFilter` on the smart chute at `pos` (None clears it); its column is
    /// saved.
    pub fn set_chute_filter(&mut self, dim: usize, pos: [i32; 3], item: Option<String>) {
        let d = &mut self.dims[dim];
        match item {
            Some(i) => d.chute_filters.insert(pos, i),
            None => d.chute_filters.remove(&pos),
        };
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        // `withCallback(invVersionTracker.reset)`: it looks at its inventory again.
        d.chute_pulls.wake(pos);
    }

    /// The contents of the container at `pos` changed: chutes asleep under it (or under another
    /// block of its vault) wake.
    pub fn container_changed(&mut self, dim: usize, pos: [i32; 3]) {
        let d = &mut self.dims[dim];
        let vaults = &d.vaults;
        let vault = vaults.controller_of(pos);
        d.chute_pulls
            .source_changed(pos, |s| vault.is_some() && vaults.controller_of(s) == vault);
    }
}

/// `FilteringBehaviour.test` with an item filter (`FilterItemStack.test`, not matching
/// components): any item without one.
pub fn passes(filter: Option<&str>, item: &str) -> bool {
    filter.is_none_or(|f| f == item)
}

/// `SmartChuteFilterSlotPositioning`: the filter slot on a horizontal side, 11 px up, half a
/// pixel in from the face.
pub fn filter_slot(side: crate::kinetics::Dir) -> Option<[f64; 3]> {
    (side.axis() != crate::kinetics::Axis::Y)
        .then(|| crate::board::sided_offset([0.5, 11.0 / 16.0, 15.5 / 16.0], side))
}

/// `ItemHelper.extract` over slots (each empty, or `(item, count, max stack)` with `item`
/// compared as stacks are by `isSameItemSameComponents`) of the items `test` accepts: in `exact`
/// mode exactly `amount` of one item or nothing, else up to `amount` of the first item found,
/// gathered from the slots in order. The count to take from each slot.
pub fn extract<K: PartialEq>(
    slots: &[Option<(K, u32, u32)>],
    test: impl Fn(&K) -> bool,
    exact: bool,
    amount: u32,
) -> Vec<(usize, u32)> {
    // Items given up on (`blackListed`), by a slot holding them.
    let mut black: Vec<usize> = Vec::new();
    let mut other_match = false;
    loop {
        let mut taking: Vec<(usize, u32)> = Vec::new();
        let mut first: Option<usize> = None;
        let mut total = 0;
        for (slot, s) in slots.iter().enumerate() {
            let Some((key, count, max)) = s else {
                continue;
            };
            let n = (*count).min(amount - total).min(*max);
            if n == 0
                || !test(key)
                || black
                    .iter()
                    .any(|&b| slots[b].as_ref().is_some_and(|x| x.0 == *key))
            {
                continue;
            }
            if let Some(f) = first {
                let (fk, _, fmax) = slots[f].as_ref().expect("a slot taken from");
                // canItemStackAmountsStack
                if fk != key || n + total > *fmax {
                    other_match = true;
                    continue;
                }
            }
            first.get_or_insert(slot);
            total += n;
            taking.push((slot, n));
            if total >= amount {
                break;
            }
        }
        // Up to: what one pass found. Exactly: the pass that found enough of one item, after
        // giving up on items there were too few of while others matched.
        if !exact || total >= amount {
            return taking;
        }
        match first {
            Some(f) if other_match => black.push(f),
            _ => return Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stack_falls_through_in_five_ticks_or_waits_in_the_middle() {
        let mut p = 1.0;
        let mut ticks = 0;
        while let Fall::At(next) = fall(p, true) {
            p = next;
            ticks += 1;
        }
        assert_eq!(ticks, 5);
        assert_eq!(fall(0.6, false), Fall::At(0.5));
        assert_eq!(fall(0.5, false), Fall::At(0.5));
    }

    #[test]
    fn extraction_gathers_one_item_from_the_slots_in_order() {
        let slots = [
            None,
            Some(("dirt", 10, 64)),
            Some(("iron", 20, 64)),
            Some(("dirt", 30, 64)),
            Some(("pearl", 16, 16)),
        ];
        // A chute: up to 16 of the first item.
        assert_eq!(extract(&slots, |_| true, false, 16), [(1, 10), (3, 6)]);
        // A smart chute filtering iron: up to 64.
        assert_eq!(extract(&slots, |k| *k == "iron", false, 64), [(2, 20)]);
        // Nothing passes the filter: nothing.
        assert!(extract(&slots, |k| *k == "gold", false, 64).is_empty());
        // Exactly 25: dirt, the first item, has 40.
        assert_eq!(extract(&slots, |_| true, true, 25), [(1, 10), (3, 15)]);
        // Exactly 30: dirt has 40, taken from both of its slots.
        assert_eq!(extract(&slots, |_| true, true, 30), [(1, 10), (3, 20)]);
        // Exactly 45: no item has that many; nothing.
        assert!(extract(&slots, |_| true, true, 45).is_empty());
        // Exactly 20 of anything but dirt: iron (pearls stack to 16).
        assert_eq!(extract(&slots, |k| *k != "dirt", true, 20), [(2, 20)]);
        // Up to 64 never mixes items, and never more than the first item's stack size.
        assert_eq!(extract(&slots[4..], |_| true, false, 64), [(0, 16)]);
    }

    #[test]
    fn chutes_wait_for_their_inventory_without_ticking() {
        let mut p = Pulls::default();
        assert!(p.take_due(0).is_empty());
        p.wake([0, 64, 0]);
        p.wake([0, 64, 0]);
        p.wake([5, 64, 0]);
        assert_eq!(p.take_due(0), [[0, 64, 0], [5, 64, 0]]);
        // One under a vault sleeps; one under a chest looks again 8 ticks later.
        p.sleep([0, 64, 0]);
        p.retry([5, 64, 0], 0);
        assert!(p.take_due(7).is_empty());
        assert_eq!(p.take_due(8), [[5, 64, 0]]);
        assert!(p.is_idle());
        // Another vault block changing leaves it asleep; its own vault's wakes it.
        p.source_changed([3, 65, 0], |_| false);
        assert!(p.take_due(9).is_empty());
        p.source_changed([1, 65, 0], |s| s == [0, 65, 0]);
        assert_eq!(p.take_due(10), [[0, 64, 0]]);
        p.retry([5, 64, 0], 10);
        p.forget([5, 64, 0]);
        assert!(p.is_idle());
    }

    #[test]
    fn filters_pass_their_item_and_empty_ones_pass_all() {
        assert!(passes(None, "minecraft:dirt"));
        assert!(passes(Some("minecraft:iron_ingot"), "minecraft:iron_ingot"));
        assert!(!passes(Some("minecraft:iron_ingot"), "minecraft:dirt"));
        use crate::kinetics::Dir;
        assert!(filter_slot(Dir::Up).is_none());
        let south = filter_slot(Dir::South).unwrap();
        assert!((south[2] - 15.5 / 16.0).abs() < 1e-9 && (south[1] - 11.0 / 16.0).abs() < 1e-9);
        let north = filter_slot(Dir::North).unwrap();
        assert!((north[2] - 0.5 / 16.0).abs() < 1e-9);
    }
}
