//! Create's chute (`ChuteBlockEntity`): one stack falls through each chute at gravity's 4 chutes
//! a second, into the inventory below, the chute below, or out of the bottom; an empty chute
//! takes from the inventory above it (`handleInputFromAbove`). Encased fans at the ends of a
//! column of chutes blow its stacks up or speed them down (`getItemMotion`); rising, an empty
//! chute takes from the inventory below it. The smart chute (`SmartChuteBlockEntity`) adds a
//! filter and stops while powered. A diagonal chute (facing sideways) leads down into the chute
//! below and beside it, away from its facing.

use crate::kinetics::Dir;

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
    step(position, MOTION, can_output)
}

/// `getItemMotion`: chutes a tick for a column of chutes pushed by a fan below it (`push`, the
/// fan's speed) and pulled by one above it (`pull`): up while the fans blow up, else down
/// faster than gravity alone while they blow down.
pub fn item_motion(push: f32, pull: f32) -> f32 {
    let motion = (push + pull) / 64.0;
    (motion.clamp(-20.0, 20.0) + if motion <= 0.0 { -4.0 } else { 0.0 }) / 20.0
}

/// `tick` for a stack moving at `motion` (`item_motion`): past the middle it only goes on when
/// `can_output` (the place it goes next takes it); past the bottom (or the top, moving up) it
/// leaves.
pub fn step(position: f32, motion: f32, can_output: bool) -> Fall {
    let next = position + motion;
    if motion < 0.0 && next < 0.5 || motion > 0.0 && next > 0.5 {
        if !can_output {
            return Fall::At(0.5);
        }
        if next < 0.0 || next > 1.0 {
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
    /// The filter it held before is returned.
    pub fn set_chute_filter(
        &mut self,
        dim: usize,
        pos: [i32; 3],
        filter: Option<crate::filter::Filter>,
    ) -> Option<crate::filter::Filter> {
        let d = &mut self.dims[dim];
        let old = match filter {
            Some(f) => d.chute_filters.insert(pos, f),
            None => d.chute_filters.remove(&pos),
        };
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        // `withCallback(invVersionTracker.reset)`: it looks at its inventory again.
        d.chute_pulls.wake(pos);
        old
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

/// `ChuteBlock.SHAPE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Intersection,
    Window,
    Normal,
    Encased,
}

impl Shape {
    /// Its state value.
    pub fn name(self) -> &'static str {
        match self {
            Shape::Intersection => "intersection",
            Shape::Window => "window",
            Shape::Normal => "normal",
            Shape::Encased => "encased",
        }
    }

    /// The shape a state value names.
    pub fn read(s: &str) -> Option<Shape> {
        Some(match s {
            "intersection" => Shape::Intersection,
            "window" => Shape::Window,
            "normal" => Shape::Normal,
            "encased" => Shape::Encased,
            _ => return None,
        })
    }
}

/// Where a chute at `pos` facing `facing` hands its stack on (`getTargetChute`): the block below,
/// a diagonal one's (facing sideways) beside that, away from its facing.
pub fn target(pos: [i32; 3], facing: Dir) -> [i32; 3] {
    let below = [pos[0], pos[1] - 1, pos[2]];
    if facing.axis() == crate::kinetics::Axis::Y {
        return below;
    }
    let o = facing.opposite().offset();
    [below[0] + o[0], below[1], below[2] + o[2]]
}

/// `ChuteBlock.updateChuteState` for a chute facing `facing` with shape `shape`: a diagonal one
/// with no chute at its target turns down again; a vertical one is an intersection while
/// diagonal chutes above it lead into it (`facing_at(p)`: the facing of the chute at `p`, None
/// when there is none, a smart chute's down), else as it was; a diagonal one too, but for one
/// chute leading on in its own line while no chute or down-facing funnel (`funnel_down_above`)
/// is over it.
pub fn update_state(
    pos: [i32; 3],
    facing: Dir,
    shape: Shape,
    facing_at: impl Fn([i32; 3]) -> Option<Dir>,
    funnel_down_above: bool,
) -> (Dir, Shape) {
    let vertical = facing == Dir::Down;
    if !vertical && facing_at(target(pos, facing)).is_none() {
        return (Dir::Down, Shape::Normal);
    }
    let above = [pos[0], pos[1] + 1, pos[2]];
    let connected = |d: Dir| {
        let o = d.offset();
        facing_at([above[0] + o[0], above[1], above[2] + o[2]]) == Some(d)
    };
    let horizontal = [Dir::South, Dir::West, Dir::North, Dir::East];
    let count = horizontal.iter().filter(|d| connected(**d)).count();
    if vertical {
        let shape = if count == 0 {
            if shape == Shape::Intersection {
                Shape::Normal
            } else {
                shape
            }
        } else {
            Shape::Intersection
        };
        return (facing, shape);
    }
    if count == 0
        || connected(Dir::North) && connected(Dir::South)
        || connected(Dir::East) && connected(Dir::West)
    {
        return (facing, Shape::Intersection);
    }
    if count == 1 && connected(facing) && facing_at(above) != Some(Dir::Down) && !funnel_down_above
    {
        let s = if shape == Shape::Encased {
            Shape::Encased
        } else {
            Shape::Normal
        };
        return (facing, s);
    }
    (facing, Shape::Intersection)
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
    fn diagonal_chutes_lead_down_beside_and_join_as_create_s_do() {
        // A chute at the origin; a diagonal one facing east sits up and east of it.
        let chutes = |list: Vec<([i32; 3], Dir)>| {
            move |p: [i32; 3]| list.iter().find(|(q, _)| *q == p).map(|(_, f)| *f)
        };
        assert_eq!(target([1, 1, 0], Dir::East), [0, 0, 0]);
        let world = chutes(vec![([0, 0, 0], Dir::Down), ([1, 1, 0], Dir::East)]);
        // The diagonal one, with nothing leading into it: an intersection (its start).
        assert_eq!(
            update_state([1, 1, 0], Dir::East, Shape::Normal, &world, false),
            (Dir::East, Shape::Intersection)
        );
        // The vertical one it leads into: an intersection.
        assert_eq!(
            update_state([0, 0, 0], Dir::Down, Shape::Window, &world, false),
            (Dir::Down, Shape::Intersection)
        );
        // With nothing below to lead into, a diagonal chute faces down again.
        assert_eq!(
            update_state([5, 5, 5], Dir::East, Shape::Normal, &world, false),
            (Dir::Down, Shape::Normal)
        );
        // A diagonal chute fed by one more in its line is a plain run.
        let world = chutes(vec![
            ([0, 0, 0], Dir::Down),
            ([1, 1, 0], Dir::East),
            ([2, 2, 0], Dir::East),
        ]);
        assert_eq!(
            update_state([1, 1, 0], Dir::East, Shape::Normal, &world, false),
            (Dir::East, Shape::Normal)
        );
    }

    #[test]
    fn a_fan_blowing_up_a_column_carries_its_stacks_up() {
        // No fans: gravity's 4 chutes a second down.
        assert_eq!(item_motion(0.0, 0.0), MOTION);
        // A fan at 64 RPM under the column: one chute a second up (64 / 64 / 20 a tick).
        assert!((item_motion(64.0, 0.0) - 0.05).abs() < 1e-6);
        // Up past the middle only when the way above takes it; out past the top.
        assert_eq!(step(0.45, 0.1, false), Fall::At(0.5));
        assert_eq!(step(0.95, 0.1, true), Fall::Out);
    }

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
