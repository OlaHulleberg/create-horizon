//! Create's smart observer (`SmartObserverBlock`, `SmartObserverBlockEntity`) and threshold
//! switch (`ThresholdSwitchBlock`, `ThresholdSwitchBlockEntity`): blocks that look at the block
//! they face (`DirectedDirectionalBlock.getTargetDirection`) and give a signal from what it
//! holds.
//!
//! Create ticks both every tick (the switch reads its target every 10, `lazyTick`). Here they
//! look again only when their target's contents change (the notifications the glue already
//! has for depots, vaults, chutes and tanks), when a neighbour changes, while a belt they watch
//! carries items, or every `lazyTick` interval for a vanilla container (no notification).
//! Between looks their result is the same as Create's: an inventory whose version did not
//! change keeps the observer on (`sustainSignal`) and the switch's level as it was.

use crate::filter::{Filter, Passes};
use crate::kinetics::Dir;
use crate::steam::Face;
use crate::tank::Fluid;
use std::collections::{BTreeSet, HashMap, HashSet};

/// `SmartObserverBlockEntity.DEFAULT_DELAY`: ticks it stays on after its last sighting.
pub const DEFAULT_DELAY: u8 = 6;
/// `activate(3)`: the target block itself passes the filter.
pub const BLOCK_DELAY: u8 = 3;
/// `SmartObserverBlockEntity`'s `lazyTickRate` (20) and the switch's (10): how often a vanilla
/// container, which sends no notification, is looked at.
pub const OBSERVER_LAZY: i64 = 20;
pub const SWITCH_LAZY: i64 = 10;
/// `ThresholdSwitchBlockEntity.scheduleBlockTick`: its output follows two ticks later.
pub const SWITCH_DELAY: i64 = 2;
/// `handleCenteredProcessingOnAllItems(.45f, ...)`: how far from a belt segment's (or a
/// depot's) centre an item is seen.
pub const CENTRE_REACH: f32 = 0.45;

/// `DirectedDirectionalBlock.getTargetDirection`: up from a ceiling `target`, down from a
/// floor one, else its facing.
pub fn target_direction(target: Face, facing: Dir) -> Dir {
    match target {
        Face::Ceiling => Dir::Up,
        Face::Floor => Dir::Down,
        Face::Wall => facing,
    }
}

/// The block a detector at `pos` looks at.
pub fn target_pos(pos: [i32; 3], target: Face, facing: Dir) -> [i32; 3] {
    let o = target_direction(target, facing).offset();
    [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]
}

/// `SmartObserverBlock.getStateForPlacement` and `ThresholdSwitchBlock`'s: the first of the
/// player's nearest looking directions with something to look at there (`detects`), else away
/// from the player's view (toward it when sneaking); a vertical one is a floor or ceiling
/// `target` with the player's horizontal facing.
pub fn placement(
    nearest: [Dir; 6],
    horizontal: Dir,
    sneaking: bool,
    detects: impl Fn(Dir) -> bool,
) -> (Face, Dir) {
    let preferred = nearest
        .iter()
        .copied()
        .find(|&d| detects(d))
        .unwrap_or(if sneaking {
            nearest[0]
        } else {
            nearest[0].opposite()
        });
    match preferred {
        Dir::Up => (Face::Ceiling, horizontal),
        Dir::Down => (Face::Floor, horizontal),
        d => (Face::Wall, d),
    }
}

/// `FilteredDetectorFilterSlot.isSideActive`: whether a detector has its filter slot on `side`
/// (`at_bottom`: the threshold switch's).
pub fn filter_side(target: Face, facing: Dir, side: Dir, at_bottom: bool) -> bool {
    let t = target_direction(target, facing);
    if side == t {
        return false;
    }
    if t.opposite() == side {
        return true;
    }
    if t.axis() != crate::kinetics::Axis::Y {
        return side == Dir::Up || side == Dir::Down && at_bottom;
    }
    let side = if t == Dir::Up { side.opposite() } else { side };
    if !at_bottom {
        return side == facing;
    }
    side.axis() == crate::display::clockwise(facing).axis()
}

/// `FilteredDetectorFilterSlot.getSouthLocation` (8, 8, 15.5) px, turned to `side`.
pub fn filter_slot(side: Dir) -> [f64; 3] {
    crate::board::sided_offset([0.5, 0.5, 15.5 / 16.0], side)
}

/// `FilteringBehaviour.test` of an item against a filter item (none: anything).
pub fn passes(filter: Option<&str>, item: &str) -> bool {
    crate::chute::passes(filter, item)
}

/// `FilterItemStack.test(level, FluidStack)`: the fluid the filter item empties into (a
/// bucket's); a filter that holds none passes no fluid.
pub fn passes_fluid(filter: Option<&str>, fluid: Fluid) -> bool {
    filter.is_none_or(|f| Fluid::of_bucket(f) == Some(fluid))
}

/// One inventory slot as a detector sees it: its item (none: empty), count and the item's
/// largest stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub item: Option<String>,
    pub count: u32,
    pub max_stack: u32,
}

/// What a detector finds at its target.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seen {
    /// The target block's item id (`block.asItem()`), for the observer's own filter test.
    pub block_item: Option<String>,
    /// Items a belt or depot holds near the centre (`TransportedItemStackHandlerBehaviour`);
    /// Some even when empty: such a target is looked at no further.
    pub transported: Option<Vec<String>>,
    /// An item handler's slots and their limit (`getSlotLimit`), if it has one.
    pub inventory: Option<(Vec<Slot>, u32)>,
    /// A fluid handler's tank: its fluid, amount and capacity.
    pub tank: Option<(Option<Fluid>, u32, u32)>,
}

/// `SmartObserverBlockEntity.tick`'s finding: activate for `n` ticks, or nothing.
pub fn observe(filter: impl Passes, seen: &Seen) -> Option<u8> {
    if let Some(item) = seen.block_item.as_deref()
        && filter.filtered()
        && filter.item(item)
    {
        return Some(BLOCK_DELAY);
    }
    if let Some(items) = &seen.transported {
        return items
            .iter()
            .any(|i| filter.item(i))
            .then_some(DEFAULT_DELAY);
    }
    if let Some((slots, _)) = &seen.inventory
        && slots
            .iter()
            .any(|s| s.count > 0 && s.item.as_deref().is_some_and(|i| filter.item(i)))
    {
        return Some(DEFAULT_DELAY);
    }
    if let Some((Some(fluid), amount, _)) = seen.tank
        && amount > 0
        && filter.fluid(fluid)
    {
        return Some(DEFAULT_DELAY);
    }
    None
}

/// `ThresholdSwitchBlockEntity.updateCurrentLevel`'s reading of an item and a fluid handler:
/// (min, level, max), None when the target has neither.
pub fn fill_level(filter: impl Passes, seen: &Seen) -> Option<(i32, i32, i32)> {
    if seen.inventory.is_none() && seen.tank.is_none() {
        return None;
    }
    let (mut level, mut max) = (0i64, 0i64);
    if let Some((slots, limit)) = &seen.inventory {
        for s in slots {
            // `min(MAX_STACK_SIZE (64 for an empty slot), getSlotLimit)`.
            let space = match s.item {
                Some(_) => s.max_stack.min(*limit),
                None => 64.min(*limit),
            };
            if space == 0 {
                continue;
            }
            max += i64::from(space);
            if s.item.as_deref().is_none_or(|i| filter.item(i)) {
                level += i64::from(s.count);
            }
        }
    }
    if let Some((fluid, amount, capacity)) = seen.tank
        && capacity > 0
    {
        max += i64::from(capacity);
        // `filtering.test(FluidStack.EMPTY)` passes only without a filter, and adds nothing.
        if let Some(f) = fluid
            && filter.fluid(f)
        {
            level += i64::from(amount);
        }
    }
    let clamp = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    Some((0, clamp(level), clamp(max)))
}

/// A smart observer's block entity: its filter, what its last look found and when it turns
/// off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Observer {
    pub filter: Option<Filter>,
    /// Its last look found something (with its `activate(n)`): it stays on until a look finds
    /// nothing (`sustainSignal`, or the same finding each tick), with no timer meanwhile.
    pub sustained: Option<u8>,
    /// The game time its `powered` is cleared (`turnOffTicks` ran out, then the block tick).
    pub off_at: Option<i64>,
}

/// What a detector's look asks of its block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Effect {
    /// `setBlockAndUpdate(powered = true)` and `updateNeighborsAt`.
    pub power_on: bool,
}

impl Observer {
    /// A look at game time `now` found `found` (from `observe`). Create activates each tick it
    /// finds something (`turnOffTicks = n`) and turns off `n + 1` ticks after the last one; a
    /// target that stops holding it at `now` was last seen the tick before.
    pub fn look(&mut self, found: Option<u8>, now: i64, powered: bool) -> Effect {
        match found {
            Some(n) => {
                self.sustained = Some(n);
                self.off_at = None;
                Effect { power_on: !powered }
            }
            None => {
                if let Some(n) = self.sustained.take() {
                    self.off_at = Some(now + i64::from(n));
                }
                Effect::default()
            }
        }
    }
}

/// A threshold switch's block entity (`onWhenAbove` 128, `offWhenBelow` 64 by default).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Switch {
    pub on_above: i32,
    pub off_below: i32,
    pub inverted: bool,
    pub in_stacks: bool,
    pub filter: Option<Filter>,
    pub min: i32,
    pub level: i32,
    pub max: i32,
    /// `redstoneState`, and `poweredAfterDelay` (the signal it gives).
    pub state: bool,
    pub powered: bool,
}

impl Default for Switch {
    fn default() -> Self {
        Switch {
            on_above: 128,
            off_below: 64,
            inverted: false,
            in_stacks: false,
            filter: None,
            min: 0,
            level: -1,
            max: 0,
            state: false,
            powered: false,
        }
    }
}

/// What `updateCurrentLevel` asks of the switch's block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LevelUpdate {
    /// Its `level` (0 to 5), set with UPDATE_ALL when `update`, else UPDATE_CLIENTS.
    pub display: Option<u8>,
    pub update: bool,
    /// `scheduleBlockTick`: its output follows two ticks later.
    pub schedule: bool,
}

impl Switch {
    /// `updateCurrentLevel` with the target's reading (`fill_level`).
    pub fn update_level(&mut self, reading: Option<(i32, i32, i32)>) -> LevelUpdate {
        let Some((min, level, max)) = reading else {
            // No compatible inventory.
            self.min = -1;
            self.max = -1;
            if self.level == -1 {
                return LevelUpdate::default();
            }
            self.level = -1;
            self.state = false;
            return LevelUpdate {
                display: Some(0),
                update: true,
                schedule: true,
            };
        };
        self.min = min;
        self.max = max;
        // `Mth.clamp(level, min, max)`: the max wins when they cross.
        self.level = level.max(min).min(max);
        let was = self.state;
        if self.state && self.level <= self.off_below {
            self.state = false;
        } else if !self.state && self.level >= self.on_above {
            self.state = true;
        }
        let update = was != self.state;
        let normed = (self.level - self.min) as f32 / (self.max - self.min) as f32;
        let display = if self.level > 0 {
            (1.0 + normed * 4.0) as i32
        } else {
            0
        };
        LevelUpdate {
            display: Some(display.clamp(0, 5) as u8),
            update,
            schedule: update,
        }
    }

    /// `updatePowerAfterDelay`: the signal follows `inverted != redstoneState`; whether it
    /// changed.
    pub fn power_after_delay(&mut self) -> bool {
        let now = self.inverted != self.state;
        std::mem::replace(&mut self.powered, now) != now
    }

    /// `getSignal`: 15 while powered, none into its target. `side` is vanilla's direction.
    pub fn signal(&self, target: Dir, side: Dir) -> i32 {
        if side == target.opposite() || !self.powered {
            0
        } else {
            15
        }
    }
}

/// `SmartObserverBlock.getSignal`: 15 while `powered`, none into its target.
pub fn observer_signal(powered: bool, target: Dir, side: Dir) -> i32 {
    if powered && side != target.opposite() {
        15
    } else {
        0
    }
}

/// Which kind of target a detector last found, for how it is looked at again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Watch {
    /// A target that tells of its changes (a Create block), or none at all.
    Notified,
    /// A belt carrying items: looked at each tick while it has any.
    Belt,
    /// A container with no notification: looked at every lazy interval.
    Lazy,
}

/// The smart observers and threshold switches of one dimension.
#[derive(Default)]
pub struct Detectors {
    pub observers: HashMap<[i32; 3], Observer>,
    pub switches: HashMap<[i32; 3], Switch>,
    /// Each detector's target, and the detectors looking at each block.
    pub target_of: HashMap<[i32; 3], [i32; 3]>,
    pub watching: HashMap<[i32; 3], Vec<[i32; 3]>>,
    /// Detectors to look again after the tick.
    pub checks: Vec<[i32; 3]>,
    /// Observers watching a belt: looked at each tick its belt has items.
    pub belt_watch: HashSet<[i32; 3]>,
    /// Detectors looking at a container with no notification, by the game time they look again.
    pub lazy: BTreeSet<(i64, [i32; 3])>,
    lazy_at: HashMap<[i32; 3], i64>,
    /// Observers' turn-off times, and switches' output ticks (by game time).
    pub offs: BTreeSet<(i64, [i32; 3])>,
    pub switch_ticks: BTreeSet<(i64, [i32; 3])>,
    saved_observers: HashMap<[i32; 3], Observer>,
    saved_switches: HashMap<[i32; 3], Switch>,
    /// Filter items of detectors that were broken, for the glue to drop.
    pub dropped: Vec<([i32; 3], Filter)>,
}

impl Detectors {
    pub fn is_empty(&self) -> bool {
        self.target_of.is_empty()
    }

    /// A detector placed, loaded or changed state (`switch`: a threshold switch): its block
    /// entity kept (or read back), its target followed, and a look after the tick (`onPlace`:
    /// `updateCurrentLevel`).
    pub fn placed(&mut self, pos: [i32; 3], switch: bool, target: [i32; 3]) {
        if switch {
            if !self.switches.contains_key(&pos) {
                let s = self.saved_switches.remove(&pos).unwrap_or_default();
                self.switches.insert(pos, s);
                self.checks.push(pos);
            }
        } else if !self.observers.contains_key(&pos) {
            let o = self.saved_observers.remove(&pos).unwrap_or_default();
            if let Some(t) = o.off_at {
                self.offs.insert((t, pos));
            }
            self.observers.insert(pos, o);
            self.checks.push(pos);
        }
        if self.target_of.get(&pos) != Some(&target) {
            self.unwatch(pos);
            self.target_of.insert(pos, target);
            self.watching.entry(target).or_default().push(pos);
            self.checks.push(pos);
        }
    }

    fn unwatch(&mut self, pos: [i32; 3]) {
        if let Some(t) = self.target_of.remove(&pos)
            && let Some(list) = self.watching.get_mut(&t)
        {
            list.retain(|p| *p != pos);
            if list.is_empty() {
                self.watching.remove(&t);
            }
        }
        self.belt_watch.remove(&pos);
        if let Some(t) = self.lazy_at.remove(&pos) {
            self.lazy.remove(&(t, pos));
        }
    }

    /// A detector gone: true when it had state worth saving. A filter item it held is in
    /// `dropped` (`FilteringBehaviour.destroy`).
    pub fn removed(&mut self, pos: [i32; 3]) -> bool {
        self.unwatch(pos);
        let observer = self.observers.remove(&pos);
        if let Some(t) = observer.as_ref().and_then(|o| o.off_at) {
            self.offs.remove(&(t, pos));
        }
        let switch = self.switches.remove(&pos);
        self.switch_ticks.retain(|(_, p)| *p != pos);
        let saved = observer.as_ref().is_some_and(|o| o.filter.is_some()) || switch.is_some();
        let filter = observer
            .and_then(|o| o.filter)
            .or_else(|| switch.and_then(|s| s.filter));
        if let Some(f) = filter.filter(Filter::is_filter_item) {
            self.dropped.push((pos, f));
        }
        saved
    }

    /// The contents of the blocks `at` changed: the detectors looking at them look again.
    pub fn contents_changed(&mut self, at: impl IntoIterator<Item = [i32; 3]>) {
        if self.watching.is_empty() {
            return;
        }
        for p in at {
            if let Some(list) = self.watching.get(&p) {
                self.checks.extend(list.iter().copied());
            }
        }
    }

    /// A detector's neighbour changed: it looks again (its target may be a new block).
    pub fn neighbor_changed(&mut self, pos: [i32; 3]) {
        if self.target_of.contains_key(&pos) {
            self.checks.push(pos);
        }
    }

    /// How a detector's target is looked at again after this look, at game time `now`.
    pub fn watch(&mut self, pos: [i32; 3], watch: Watch, now: i64) {
        if watch == Watch::Belt {
            self.belt_watch.insert(pos);
        } else {
            self.belt_watch.remove(&pos);
        }
        if let Some(t) = self.lazy_at.remove(&pos) {
            self.lazy.remove(&(t, pos));
        }
        if watch == Watch::Lazy {
            let every = if self.switches.contains_key(&pos) {
                SWITCH_LAZY
            } else {
                OBSERVER_LAZY
            };
            self.lazy.insert((now + every, pos));
            self.lazy_at.insert(pos, now + every);
        }
    }

    /// The detectors to look at now: those asked to, and those whose lazy interval came
    /// round; each once.
    pub fn take_checks(&mut self, now: i64) -> Vec<[i32; 3]> {
        let mut out = std::mem::take(&mut self.checks);
        while let Some(&(t, pos)) = self.lazy.first() {
            if t > now {
                break;
            }
            self.lazy.pop_first();
            self.lazy_at.remove(&pos);
            out.push(pos);
        }
        out.sort();
        out.dedup();
        out
    }

    /// An observer's look: its new turn-off time is kept in order.
    pub fn observer_look(
        &mut self,
        pos: [i32; 3],
        found: Option<u8>,
        now: i64,
        powered: bool,
    ) -> Effect {
        let Some(o) = self.observers.get_mut(&pos) else {
            return Effect::default();
        };
        let before = o.off_at;
        let e = o.look(found, now, powered);
        if before != o.off_at {
            if let Some(t) = before {
                self.offs.remove(&(t, pos));
            }
            if let Some(t) = o.off_at {
                self.offs.insert((t, pos));
            }
        }
        e
    }

    /// Observers whose time is up (`tick`: `powered` false).
    pub fn take_offs(&mut self, now: i64) -> Vec<[i32; 3]> {
        let mut out = Vec::new();
        while let Some(&(t, pos)) = self.offs.first() {
            if t > now {
                break;
            }
            self.offs.pop_first();
            if let Some(o) = self.observers.get_mut(&pos)
                && o.off_at == Some(t)
            {
                o.off_at = None;
                out.push(pos);
            }
        }
        out
    }

    /// `scheduleBlockTick`: a switch's output tick two ticks on, unless one is coming.
    pub fn schedule_switch(&mut self, pos: [i32; 3], now: i64) {
        if !self.switch_ticks.iter().any(|(_, p)| *p == pos) {
            self.switch_ticks.insert((now + SWITCH_DELAY, pos));
        }
    }

    pub fn take_switch_ticks(&mut self, now: i64) -> Vec<[i32; 3]> {
        let mut out = Vec::new();
        while let Some(&(t, pos)) = self.switch_ticks.first() {
            if t > now {
                break;
            }
            self.switch_ticks.pop_first();
            out.push(pos);
        }
        out
    }

    /// A filter set by a click (none: cleared): the detector looks again
    /// (`withCallback`: `invVersionTracker.reset`, `updateCurrentLevel`).
    /// The filter it held before is in the `Ok`; `Err` gives the filter back when there is no
    /// detector at `pos`.
    pub fn set_filter(
        &mut self,
        pos: [i32; 3],
        filter: Option<Filter>,
    ) -> Result<Option<Filter>, Option<Filter>> {
        let old = if let Some(o) = self.observers.get_mut(&pos) {
            std::mem::replace(&mut o.filter, filter)
        } else if let Some(s) = self.switches.get_mut(&pos) {
            std::mem::replace(&mut s.filter, filter)
        } else {
            return Err(filter);
        };
        self.checks.push(pos);
        Ok(old)
    }

    pub fn filter(&self, pos: [i32; 3]) -> Option<&Filter> {
        self.observers
            .get(&pos)
            .and_then(|o| o.filter.as_ref())
            .or_else(|| self.switches.get(&pos).and_then(|s| s.filter.as_ref()))
    }

    /// Saved block entities, for the detectors found later.
    pub fn load(
        &mut self,
        observers: impl Iterator<Item = ([i32; 3], Observer)>,
        switches: impl Iterator<Item = ([i32; 3], Switch)>,
    ) {
        self.saved_observers.extend(observers);
        self.saved_switches.extend(switches);
    }

    /// A column's observers with a filter or a pending turn-off, and its switches, to save.
    #[allow(clippy::type_complexity)]
    pub fn column(&self, cx: i32, cz: i32) -> (Vec<([i32; 3], Observer)>, Vec<([i32; 3], Switch)>) {
        let inside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (cx, cz);
        let mut o: Vec<([i32; 3], Observer)> = self
            .observers
            .iter()
            .filter(|(p, o)| inside(p) && (o.filter.is_some() || o.off_at.is_some()))
            .map(|(p, o)| (*p, o.clone()))
            .collect();
        o.sort_by_key(|e| e.0);
        let mut s: Vec<([i32; 3], Switch)> = self
            .switches
            .iter()
            .filter(|(p, _)| inside(p))
            .map(|(p, s)| (*p, s.clone()))
            .collect();
        s.sort_by_key(|e| e.0);
        (o, s)
    }

    pub fn column_unloaded(&mut self, cx: i32, cz: i32) {
        let inside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (cx, cz);
        let gone: Vec<[i32; 3]> = self.target_of.keys().copied().filter(inside).collect();
        for p in gone {
            self.unwatch(p);
        }
        self.observers.retain(|p, _| !inside(p));
        self.switches.retain(|p, _| !inside(p));
        self.saved_observers.retain(|p, _| !inside(p));
        self.saved_switches.retain(|p, _| !inside(p));
        self.checks.retain(|p| !inside(p));
        self.offs.retain(|(_, p)| !inside(p));
        self.switch_ticks.retain(|(_, p)| !inside(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(item: &str, count: u32) -> Slot {
        Slot {
            item: Some(item.into()),
            count,
            max_stack: 64,
        }
    }

    fn empty() -> Slot {
        Slot {
            item: None,
            count: 0,
            max_stack: 64,
        }
    }

    #[test]
    fn targets_placement_and_filter_slots_follow_directed_directional_block() {
        assert_eq!(target_direction(Face::Wall, Dir::East), Dir::East);
        assert_eq!(target_direction(Face::Floor, Dir::East), Dir::Down);
        assert_eq!(
            target_pos([0, 64, 0], Face::Ceiling, Dir::North),
            [0, 65, 0]
        );
        let nearest = [
            Dir::North,
            Dir::Down,
            Dir::East,
            Dir::West,
            Dir::Up,
            Dir::South,
        ];
        // Nothing to look at: away from the view, toward it when sneaking.
        assert_eq!(
            placement(nearest, Dir::North, false, |_| false),
            (Face::Wall, Dir::South)
        );
        assert_eq!(
            placement(nearest, Dir::North, true, |_| false),
            (Face::Wall, Dir::North)
        );
        // The nearest direction with an inventory wins; below is a floor target.
        assert_eq!(
            placement(nearest, Dir::North, false, |d| d == Dir::Down
                || d == Dir::East),
            (Face::Floor, Dir::North)
        );
        // Filter slots: never on the target's face, always on the back; a wall detector's on
        // top (and the switch's at the bottom too).
        assert!(!filter_side(Face::Wall, Dir::North, Dir::North, true));
        assert!(filter_side(Face::Wall, Dir::North, Dir::South, false));
        assert!(filter_side(Face::Wall, Dir::North, Dir::Up, false));
        assert!(!filter_side(Face::Wall, Dir::North, Dir::Down, false));
        assert!(filter_side(Face::Wall, Dir::North, Dir::Down, true));
        assert!(!filter_side(Face::Wall, Dir::North, Dir::East, true));
        // A floor observer's on its facing; a floor switch's across it.
        assert!(filter_side(Face::Floor, Dir::North, Dir::North, false));
        assert!(!filter_side(Face::Floor, Dir::North, Dir::East, false));
        assert!(filter_side(Face::Floor, Dir::North, Dir::East, true));
        assert!(filter_side(Face::Floor, Dir::North, Dir::West, true));
        let s = filter_slot(Dir::Up);
        assert!((s[1] - 15.5 / 16.0).abs() < 1e-9);
    }

    #[test]
    fn the_observer_sees_its_filter_in_blocks_belts_inventories_and_tanks() {
        let iron = Some("minecraft:iron_ingot");
        let block = Seen {
            block_item: Some("minecraft:iron_ingot".into()),
            ..Seen::default()
        };
        assert_eq!(observe(iron, &block), Some(BLOCK_DELAY));
        // No filter: the block alone is nothing.
        assert_eq!(observe(None::<&str>, &block), None);
        let belt = Seen {
            transported: Some(vec!["minecraft:dirt".into()]),
            inventory: Some((vec![slot("minecraft:iron_ingot", 1)], 64)),
            ..Seen::default()
        };
        // A belt or depot is looked at no further than its items.
        assert_eq!(observe(iron, &belt), None);
        assert_eq!(observe(None::<&str>, &belt), Some(DEFAULT_DELAY));
        let chest = Seen {
            inventory: Some((vec![empty(), slot("minecraft:iron_ingot", 3)], 64)),
            ..Seen::default()
        };
        assert_eq!(observe(iron, &chest), Some(DEFAULT_DELAY));
        assert_eq!(observe(Some("minecraft:dirt"), &chest), None);
        let tank = Seen {
            tank: Some((Some(Fluid::Water), 500, 8000)),
            ..Seen::default()
        };
        assert_eq!(observe(None::<&str>, &tank), Some(DEFAULT_DELAY));
        assert_eq!(
            observe(Some("minecraft:water_bucket"), &tank),
            Some(DEFAULT_DELAY)
        );
        assert_eq!(observe(Some("minecraft:lava_bucket"), &tank), None);
        assert_eq!(observe(iron, &tank), None);
        assert_eq!(observe(None::<&str>, &Seen::default()), None);
    }

    #[test]
    fn an_observer_stays_on_while_it_finds_and_turns_off_seven_ticks_after() {
        let mut d = Detectors::default();
        let p = [0, 64, 0];
        d.placed(p, false, [1, 64, 0]);
        assert_eq!(d.take_checks(0), [p]);
        assert!(d.observer_look(p, Some(DEFAULT_DELAY), 10, false).power_on);
        // Found again while on: nothing to do; still found when the time comes: kept on.
        assert!(!d.observer_look(p, Some(DEFAULT_DELAY), 11, true).power_on);
        // No timer while it keeps finding it.
        assert!(d.take_offs(18).is_empty());
        assert!(d.offs.is_empty());
        // Gone at 20: the last finding was at 19 in Create (the same each tick), so off at 26.
        assert_eq!(d.observer_look(p, None, 20, true), Effect::default());
        assert!(d.take_offs(25).is_empty());
        assert_eq!(d.take_offs(26), [p]);
        // A block match (`activate(3)`) found once: off four ticks later.
        d.observer_look(p, Some(BLOCK_DELAY), 30, false);
        d.observer_look(p, None, 31, true);
        assert_eq!(d.take_offs(34), [p]);
    }

    #[test]
    fn detectors_look_again_when_their_target_changes_or_lazily() {
        let mut d = Detectors::default();
        let (a, b) = ([0, 64, 0], [5, 64, 0]);
        d.placed(a, false, [1, 64, 0]);
        d.placed(b, true, [6, 64, 0]);
        d.take_checks(0);
        d.contents_changed([[1, 64, 0], [9, 9, 9]]);
        assert_eq!(d.take_checks(1), [a]);
        // A chest beside the switch: every 10 ticks; the observer's every 20.
        d.watch(b, Watch::Lazy, 1);
        d.watch(a, Watch::Lazy, 1);
        assert!(d.take_checks(10).is_empty());
        assert_eq!(d.take_checks(11), [b]);
        assert_eq!(d.take_checks(21), [a]);
        // Turned to another target: watched there instead.
        d.placed(a, false, [0, 63, 0]);
        d.take_checks(22);
        d.contents_changed([[1, 64, 0]]);
        assert!(d.take_checks(23).is_empty());
        d.watch(a, Watch::Belt, 23);
        assert!(d.belt_watch.contains(&a));
        assert_eq!(
            d.set_filter(a, Some(Filter::item("minecraft:dirt"))),
            Ok(None)
        );
        assert_eq!(d.filter(a), Some(&Filter::item("minecraft:dirt")));
        assert!(d.removed(a));
        // A plain item is a ghost copy: nothing to drop.
        assert!(d.dropped.is_empty());
        d.placed(a, false, [0, 63, 0]);
        let _ = d.set_filter(a, Some(Filter::item(crate::filter::FILTER)));
        assert!(d.removed(a));
        assert_eq!(d.dropped.len(), 1);
        assert!(d.belt_watch.is_empty());
        let (o, s) = d.column(0, 0);
        assert!(o.is_empty());
        assert_eq!(s.len(), 1);
        d.column_unloaded(0, 0);
        assert!(d.is_empty());
    }

    #[test]
    fn the_switch_fills_turns_on_above_and_off_below_with_its_level() {
        // A chest of 27 slots, 2 stacks of cobblestone: 128 of 1728.
        let mut slots = vec![empty(); 27];
        slots[0] = slot("minecraft:cobblestone", 64);
        slots[5] = slot("minecraft:cobblestone", 64);
        let chest = Seen {
            inventory: Some((slots.clone(), 64)),
            ..Seen::default()
        };
        assert_eq!(fill_level(None::<&str>, &chest), Some((0, 128, 1728)));
        assert_eq!(
            fill_level(Some("minecraft:dirt"), &chest),
            Some((0, 0, 1728))
        );
        let mut s = Switch::default();
        let u = s.update_level(fill_level(None::<&str>, &chest));
        // At 128 (onWhenAbove 128): on; level 1 + 128/1728 * 4 = 1.
        assert_eq!(
            u,
            LevelUpdate {
                display: Some(1),
                update: true,
                schedule: true
            }
        );
        assert!(s.power_after_delay());
        assert_eq!(s.signal(Dir::North, Dir::North), 15);
        // Into its target, nothing.
        assert_eq!(s.signal(Dir::North, Dir::South), 0);
        // 100 left: still on (off at or below 64).
        slots[5] = slot("minecraft:cobblestone", 36);
        let u = s.update_level(fill_level(
            None::<&str>,
            &Seen {
                inventory: Some((slots.clone(), 64)),
                ..Seen::default()
            },
        ));
        assert!(!u.update && s.state);
        slots[0] = empty();
        let u = s.update_level(fill_level(
            None::<&str>,
            &Seen {
                inventory: Some((slots.clone(), 64)),
                ..Seen::default()
            },
        ));
        assert!(u.update && !s.state);
        assert!(s.power_after_delay() && !s.powered);
        // Inverted: on while off.
        s.inverted = true;
        assert!(s.power_after_delay());
        assert!(s.powered);
        // A full tank: level 5; a tank of lava read through a water filter: 0.
        let tank = Seen {
            tank: Some((Some(Fluid::Lava), 8000, 8000)),
            ..Seen::default()
        };
        let mut t = Switch::default();
        assert_eq!(
            t.update_level(fill_level(None::<&str>, &tank)).display,
            Some(5)
        );
        assert_eq!(
            fill_level(Some("minecraft:water_bucket"), &tank),
            Some((0, 0, 8000))
        );
        // The tank gone: level 0, off, a tick scheduled; then nothing more.
        assert_eq!(
            t.update_level(None),
            LevelUpdate {
                display: Some(0),
                update: true,
                schedule: true
            }
        );
        assert_eq!(t.update_level(None), LevelUpdate::default());
        assert_eq!(fill_level(None::<&str>, &Seen::default()), None);
        assert_eq!(observer_signal(true, Dir::Down, Dir::Up), 0);
        assert_eq!(observer_signal(true, Dir::Down, Dir::North), 15);
    }
}
