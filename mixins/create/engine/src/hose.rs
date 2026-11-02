//! Create's hose pulley (`HosePulleyBlockEntity`, `HosePulleyFluidHandler`,
//! `FluidManipulationBehaviour`, `FluidDrainingBehaviour`, `FluidFillingBehaviour`): turning
//! forwards lowers its hose, backwards raises it; once still, pipes draw fluid blocks from the
//! body the hose ends in (the highest and farthest first, a bucket a block; without end once
//! the body is `hosePulleyBlockThreshold` blocks of a bottomless fluid) or pour fluid into the
//! world (the lowest and nearest free space first, a source a bucket; voided into a body that
//! big). The world is read and changed through [`Level`].
//!
//! Idle costs nothing: a hose ticks only while it moves or a search runs; Create's counters of
//! ticks (`revalidateIn`) are game times here, compared when the hose is used.

use crate::tank::{BUCKET, Fluid, Tank};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet, VecDeque};

/// `hosePulleyRange`: how far from the hose's end blocks are drawn or filled.
pub const RANGE: i32 = 128;
/// `hosePulleyBlockThreshold`: a body this big is bottomless.
pub const THRESHOLD: usize = 10_000;
/// `FluidManipulationBehaviour.searchedPerTick`.
pub const SEARCHED_PER_TICK: usize = 1024;
/// `validationTimerMin`, and `validationTimer()`: `max(160, threshold / 1024 + 1)`.
pub const VALIDATION_TIMER: i64 = {
    let t = (THRESHOLD / SEARCHED_PER_TICK + 1) as i64;
    if t > 160 { t } else { 160 }
};
/// `HosePulleyBlockEntity.internalTank`'s capacity.
pub const CAPACITY: u32 = 1500;
/// `fillInfinite` (false): a bottomless body takes what is poured without growing.
pub const FILL_INFINITE: bool = false;
/// `fluidFillPlaceFluidSourceBlocks` (true).
pub const PLACE_SOURCES: bool = true;

/// `create:bottomless/allow` (`BottomlessFluidMode.ALLOW_BY_TAG`): water and lava.
pub fn bottomless(fluid: Option<Fluid>) -> bool {
    matches!(fluid, Some(Fluid::Water | Fluid::Lava))
}

/// What a block is to the hose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// A fluid block (`LiquidBlock`): its fluid, and whether it is a source (level 0).
    Liquid(Fluid, bool),
    /// A block with `waterlogged`, and whether it is.
    Waterloggable(bool),
    /// Another block holding a fluid, without collision (seagrass): a source of it.
    Holding(Fluid),
    /// A block a fluid washes away (`canBeReplacedByFluid`: air, grass...).
    Replaceable,
    /// Anything else.
    Solid,
}

impl Space {
    /// `getFluidState`: its fluid and whether a source.
    pub fn fluid(self) -> Option<(Fluid, bool)> {
        match self {
            Space::Liquid(f, source) => Some((f, source)),
            Space::Waterloggable(true) => Some((Fluid::Water, true)),
            Space::Holding(f) => Some((f, true)),
            _ => None,
        }
    }
}

/// The world as the hose sees and changes it.
pub trait Level {
    fn loaded(&self, p: [i32; 3]) -> bool;
    fn space(&self, p: [i32; 3]) -> Space;
    /// `canBeReplaced`: the hose may go down through it.
    fn passable(&self, p: [i32; 3]) -> bool;
    /// The fluid drawn out of the block: air, or no longer waterlogged.
    fn empty(&mut self, p: [i32; 3]);
    /// A source of `fluid` poured into the block (waterlogged, for water in a waterloggable).
    fn fill(&mut self, p: [i32; 3], fluid: Fluid);
    /// `playEffect`: the fill (drawn) or empty (poured) sound of `fluid` at `p`; `fizz` for
    /// water poured where it boils away (`FIRE_EXTINGUISH`).
    fn effect(&mut self, p: [i32; 3], fluid: Fluid, fill: bool, fizz: bool);
}

fn below(p: [i32; 3], n: i32) -> [i32; 3] {
    [p[0], p[1] - n, p[2]]
}

const SIDES: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];

fn add(p: [i32; 3], o: [i32; 3]) -> [i32; 3] {
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

fn dist_sq(a: [i32; 3], b: [i32; 3]) -> i64 {
    (0..3).map(|i| i64::from(a[i] - b[i]).pow(2)).sum()
}

/// A queued block (`BlockPosEntry`) in the order its queue takes them (`comparePositions`):
/// draining takes the highest, then the farthest by steps, then by distance from the root;
/// filling the lowest, nearest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    pos: [i32; 3],
    distance: i32,
    key: (i32, i32, i64),
}

impl Entry {
    fn new(pos: [i32; 3], distance: i32, root: [i32; 3], drain: bool) -> Entry {
        let d = dist_sq(pos, root);
        let key = if drain {
            (pos[1], distance, d)
        } else {
            (-pos[1], -distance, -d)
        };
        Entry { pos, distance, key }
    }
}

impl Ord for Entry {
    fn cmp(&self, o: &Self) -> Ordering {
        self.key.cmp(&o.key).then_with(|| o.pos.cmp(&self.pos))
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// `ChunkNotLoadedException`.
struct NotLoaded;

/// `FluidManipulationBehaviour.search`: up to 1024 blocks of the frontier (while the body is
/// under the threshold or not bottomless), each of `fluid` (the first one found sets it) given
/// to `found` and its neighbours of fluid queued within range, not up into flowing fluid off
/// the root's column, nor down unless `downward`.
fn search(
    level: &impl Level,
    root: [i32; 3],
    mut fluid: Option<Fluid>,
    frontier: &mut VecDeque<([i32; 3], i32)>,
    visited: &mut HashSet<[i32; 3]>,
    mut found: impl FnMut([i32; 3], i32),
    downward: bool,
) -> Result<Option<Fluid>, NotLoaded> {
    let range_sq = i64::from(RANGE) * i64::from(RANGE);
    let mut i = 0;
    while i < SEARCHED_PER_TICK
        && !frontier.is_empty()
        && (visited.len() <= THRESHOLD || !bottomless(fluid))
    {
        i += 1;
        let Some((at, distance)) = frontier.pop_front() else {
            break;
        };
        if !visited.insert(at) {
            continue;
        }
        if !level.loaded(at) {
            return Err(NotLoaded);
        }
        let Some((here, _)) = level.space(at).fluid() else {
            continue;
        };
        if fluid.is_none() {
            fluid = Some(here);
        }
        if fluid != Some(here) {
            continue;
        }
        found(at, distance);
        for (side, o) in SIDES.iter().enumerate() {
            if !downward && side == 0 {
                continue;
            }
            let next = add(at, *o);
            if !level.loaded(next) {
                return Err(NotLoaded);
            }
            if visited.contains(&next) || dist_sq(next, root) > range_sq {
                continue;
            }
            let Some((_, source)) = level.space(next).fluid() else {
                continue;
            };
            if !source && side == 1 && (next[0], next[2]) != (root[0], root[2]) {
                continue;
            }
            frontier.push_back((next, distance + 1));
        }
    }
    Ok(fluid)
}

/// `FluidDrainingBehaviour`: the blocks of the body under the hose, found a tick at a time and
/// drawn one per pull.
#[derive(Clone, Debug, Default)]
pub struct Drainer {
    root: Option<[i32; 3]>,
    pub fluid: Option<Fluid>,
    pub infinite: bool,
    counterpart_acted: bool,
    /// The game time `revalidateIn` reaches 0.
    revalidate_at: i64,
    frontier: VecDeque<([i32; 3], i32)>,
    visited: HashSet<[i32; 3]>,
    validation_set: HashSet<[i32; 3]>,
    queue: BinaryHeap<Entry>,
    is_valid: bool,
    validation_frontier: VecDeque<([i32; 3], i32)>,
    validation_visited: HashSet<[i32; 3]>,
    new_validation_set: HashSet<[i32; 3]>,
}

/// `FluidBlockType` of `canPullFluidsFrom`.
fn pull_type(s: Space) -> Option<bool> {
    match s {
        Space::Waterloggable(true) | Space::Holding(_) => Some(true),
        Space::Liquid(_, source) => Some(source),
        _ => None,
    }
}

impl Drainer {
    /// Whether a search or a validation runs (the hose ticks for it).
    pub fn searching(&self) -> bool {
        !self.frontier.is_empty() || !self.validation_frontier.is_empty()
    }

    pub fn counterpart_acted(&mut self) {
        self.counterpart_acted = true;
    }

    /// The blocks found to draw, highest first.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// `reset`.
    pub fn reset(&mut self, now: i64) {
        self.revalidate_at = now + VALIDATION_TIMER;
        self.frontier.clear();
        self.visited.clear();
        self.infinite = false;
        self.fluid = None;
        self.root = None;
        self.queue.clear();
        self.validation_set.clear();
        self.new_validation_set.clear();
        self.validation_frontier.clear();
        self.validation_visited.clear();
    }

    fn soft_reset(&mut self, root: [i32; 3], now: i64) {
        self.queue.clear();
        self.validation_set.clear();
        self.new_validation_set.clear();
        self.validation_frontier.clear();
        self.validation_visited.clear();
        self.visited.clear();
        self.infinite = false;
        self.revalidate_at = now + VALIDATION_TIMER;
        self.frontier.push_back((root, 0));
    }

    fn rebuild_context(&mut self, root: [i32; 3], now: i64) {
        self.reset(now);
        self.root = Some(root);
        if self.is_valid {
            self.frontier.push_back((root, 0));
        }
    }

    /// `checkValid`: the root is a source, or flowing fluid with a source beside it on the way
    /// up.
    fn check_valid(level: &impl Level, hose: [i32; 3], root: [i32; 3]) -> bool {
        let mut at = root;
        let mut timeout = 1000;
        while timeout > 0 && root != hose {
            timeout -= 1;
            match pull_type(level.space(at)) {
                Some(false) => {
                    if SIDES
                        .iter()
                        .any(|o| pull_type(level.space(add(at, *o))) == Some(true))
                    {
                        return true;
                    }
                    at = [at[0], at[1] + 1, at[2]];
                }
                Some(true) => return true,
                None => break,
            }
        }
        false
    }

    /// `tick`: the search or the validation goes on.
    pub fn tick(&mut self, level: &impl Level, now: i64) {
        if !self.frontier.is_empty() {
            self.continue_search(level);
        } else if !self.validation_frontier.is_empty() {
            self.continue_validation(level, now);
        }
    }

    fn continue_search(&mut self, level: &impl Level) {
        let root = self.root.unwrap_or_default();
        let (queue, validation) = (&mut self.queue, &mut self.validation_set);
        match search(
            level,
            root,
            self.fluid,
            &mut self.frontier,
            &mut self.visited,
            |p, d| {
                queue.push(Entry::new(p, d, root, true));
                validation.insert(p);
            },
            false,
        ) {
            Ok(f) => self.fluid = f,
            Err(NotLoaded) => {
                self.frontier.clear();
                self.visited.clear();
            }
        }
        if self.visited.len() >= THRESHOLD && bottomless(self.fluid) && !self.queue.is_empty() {
            self.infinite = true;
            let first = self.queue.peek().map(|e| e.pos).unwrap_or(root);
            self.frontier.clear();
            self.visited.clear();
            self.queue.clear();
            self.queue.push(Entry::new(first, 0, root, true));
            return;
        }
        if self.frontier.is_empty() {
            self.visited.clear();
        }
    }

    fn continue_validation(&mut self, level: &impl Level, now: i64) {
        let root = self.root.unwrap_or_default();
        let set = &mut self.new_validation_set;
        if search(
            level,
            root,
            self.fluid,
            &mut self.validation_frontier,
            &mut self.validation_visited,
            |p, _| {
                set.insert(p);
            },
            false,
        )
        .is_err()
        {
            self.validation_frontier.clear();
            self.validation_visited.clear();
            self.revalidate_at = now + 2 * VALIDATION_TIMER;
            return;
        }
        if self.validation_visited.len() >= THRESHOLD && bottomless(self.fluid) {
            if !self.infinite {
                self.reset(now);
            }
            self.validation_frontier.clear();
            self.revalidate_at = now + 2 * VALIDATION_TIMER;
            return;
        }
        if !self.validation_frontier.is_empty() {
            return;
        }
        if self.infinite {
            self.reset(now);
            return;
        }
        self.validation_set = std::mem::take(&mut self.new_validation_set);
        self.validation_visited.clear();
    }

    /// `pullNext`: whether a bucket's worth comes out of the body at `root` (a block drawn
    /// unless `simulate`; flowing fluid in the way is cleared without counting).
    pub fn pull_next(
        &mut self,
        level: &mut impl Level,
        hose: [i32; 3],
        root: [i32; 3],
        simulate: bool,
        now: i64,
    ) -> bool {
        // `tick`'s `isValid`, read when it is used.
        if let Some(r) = self.root {
            self.is_valid = Self::check_valid(level, hose, r);
        }
        if !self.frontier.is_empty() {
            return false;
        }
        if self.root != Some(root) {
            self.rebuild_context(root, now);
            return false;
        }
        if self.counterpart_acted {
            self.counterpart_acted = false;
            self.soft_reset(root, now);
            return false;
        }
        if !self.queue.is_empty() && !self.is_valid {
            self.rebuild_context(root, now);
            return false;
        }
        if self.validation_frontier.is_empty()
            && !self.queue.is_empty()
            && !simulate
            && now >= self.revalidate_at
        {
            // `revalidate`.
            self.validation_frontier.clear();
            self.validation_visited.clear();
            self.new_validation_set.clear();
            self.validation_frontier.push_back((root, 0));
            self.revalidate_at = now + VALIDATION_TIMER;
        }
        if self.infinite {
            if !simulate && let Some(f) = self.fluid {
                level.effect(root, f, true, false);
            }
            return true;
        }
        while let Some(top) = self.queue.peek().copied() {
            let space = level.space(top.pos);
            let fluid = match space {
                Space::Waterloggable(true) => Some(Fluid::Water),
                Space::Liquid(f, true) | Space::Holding(f) => Some(f),
                Space::Liquid(_, false) => {
                    // Flowing fluid is cleared on the way.
                    level.empty(top.pos);
                    self.queue.pop();
                    if self.queue.is_empty() {
                        self.is_valid = Self::check_valid(level, hose, self.root.unwrap_or(root));
                        self.reset(now);
                    }
                    continue;
                }
                _ => None,
            };
            if self.fluid.is_none() {
                self.fluid = fluid;
            }
            if fluid.is_none() || self.fluid != fluid {
                self.queue.pop();
                if self.queue.is_empty() {
                    self.is_valid = Self::check_valid(level, hose, self.root.unwrap_or(root));
                    self.reset(now);
                }
                continue;
            }
            if simulate {
                return true;
            }
            if let Some(f) = fluid {
                level.effect(top.pos, f, true, false);
            }
            level.empty(top.pos);
            self.queue.pop();
            if self.queue.is_empty() {
                self.is_valid = Self::check_valid(level, hose, self.root.unwrap_or(root));
                self.reset(now);
            } else if !self.validation_set.contains(&top.pos) {
                self.reset(now);
            }
            return true;
        }
        if self.root.is_none() {
            return false;
        }
        if self.is_valid {
            self.rebuild_context(root, now);
        }
        false
    }

    /// `getDrainableFluid`: the fluid a pull would give, if one would.
    pub fn drainable(
        &mut self,
        level: &mut impl Level,
        hose: [i32; 3],
        root: [i32; 3],
        now: i64,
    ) -> Option<Fluid> {
        let f = self.fluid?;
        if !self.frontier.is_empty() || !self.pull_next(level, hose, root, true, now) {
            return None;
        }
        Some(f)
    }
}

/// `FluidFillingBehaviour`: the free spaces around the hose's end, filled one per bucket.
#[derive(Clone, Debug, Default)]
pub struct Filler {
    root: Option<[i32; 3]>,
    pub infinite: bool,
    counterpart_acted: bool,
    /// The game time `revalidateIn` reaches 0 (0 at first: due, as `revalidateIn = 1` is after
    /// a tick).
    revalidate_at: i64,
    queue: BinaryHeap<Entry>,
    visited: HashSet<[i32; 3]>,
    infinity_frontier: VecDeque<([i32; 3], i32)>,
    infinity_visited: HashSet<[i32; 3]>,
}

/// `SpaceType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fill {
    Fillable,
    Filled,
    Blocking,
}

/// `getAtPos`.
fn fill_type(s: Space, fluid: Fluid) -> Fill {
    match s {
        Space::Waterloggable(w) => match (fluid, w) {
            (Fluid::Water, true) => Fill::Filled,
            (Fluid::Water, false) => Fill::Fillable,
            _ => Fill::Blocking,
        },
        Space::Liquid(f, true) | Space::Holding(f) => {
            if f == fluid {
                Fill::Filled
            } else {
                Fill::Blocking
            }
        }
        Space::Liquid(_, false) | Space::Replaceable => Fill::Fillable,
        Space::Solid => Fill::Blocking,
    }
}

impl Filler {
    /// Whether the check for a bottomless body runs (the hose ticks for it).
    pub fn searching(&self) -> bool {
        !self.infinity_frontier.is_empty() && self.root.is_some()
    }

    pub fn counterpart_acted(&mut self) {
        self.counterpart_acted = true;
    }

    /// `reset` (the root stays).
    pub fn reset(&mut self, now: i64) {
        self.revalidate_at = now + VALIDATION_TIMER;
        self.visited.clear();
        self.infinite = false;
        self.queue.clear();
        self.infinity_frontier.clear();
        self.infinity_visited.clear();
    }

    fn soft_reset(&mut self, root: [i32; 3], now: i64) {
        self.visited.clear();
        self.queue.clear();
        self.queue.push(Entry::new(root, 0, root, false));
        self.infinite = false;
        self.revalidate_at = now + VALIDATION_TIMER;
    }

    /// `tick`: the check for a bottomless body goes on while the root holds fluid.
    pub fn tick(&mut self, level: &impl Level, now: i64) {
        let Some(root) = self.root else {
            return;
        };
        if self.infinity_frontier.is_empty() {
            return;
        }
        let Some((fluid, _)) = level.space(root).fluid() else {
            return;
        };
        let mut found = Vec::new();
        if search(
            level,
            root,
            Some(fluid),
            &mut self.infinity_frontier,
            &mut self.infinity_visited,
            |p, d| found.push((p, d)),
            true,
        )
        .is_err()
        {
            self.infinity_frontier.clear();
            self.infinity_visited.clear();
            self.revalidate_at = now + 2 * VALIDATION_TIMER;
            return;
        }
        self.infinity_frontier.extend(found);
        if self.infinity_visited.len() >= THRESHOLD && !FILL_INFINITE {
            if !self.infinite {
                self.reset(now);
                self.infinite = true;
            }
            self.infinity_frontier.clear();
            self.revalidate_at = now + 2 * VALIDATION_TIMER;
            return;
        }
        if !self.infinity_frontier.is_empty() {
            return;
        }
        if self.infinite {
            self.reset(now);
            return;
        }
        self.infinity_visited.clear();
    }

    /// `tryDeposit`: whether a bucket's worth of `fluid` went into the world at `root` (a
    /// source placed unless `simulate`). `ultra_warm`: water boils away (the nether).
    pub fn try_deposit(
        &mut self,
        level: &mut impl Level,
        fluid: Fluid,
        root: [i32; 3],
        simulate: bool,
        ultra_warm: bool,
        now: i64,
    ) -> bool {
        if self.root != Some(root) {
            self.reset(now);
            self.root = Some(root);
            self.queue.push(Entry::new(root, 0, root, false));
            return false;
        }
        if self.counterpart_acted {
            self.counterpart_acted = false;
            self.soft_reset(root, now);
            return false;
        }
        if now >= self.revalidate_at {
            self.visited.clear();
            self.infinity_frontier.clear();
            self.infinity_visited.clear();
            self.infinity_frontier.push_back((root, 0));
            self.soft_reset(root, now);
        }
        let evaporate = ultra_warm && fluid == Fluid::Water;
        if (!FILL_INFINITE && self.infinite) || evaporate || !PLACE_SOURCES {
            let same = level.space(root).fluid().map(|f| f.0) == Some(fluid);
            if !same && !evaporate && PLACE_SOURCES {
                return false;
            }
            if simulate {
                return true;
            }
            level.effect(root, fluid, false, evaporate);
            return true;
        }
        let range_sq = i64::from(RANGE) * i64::from(RANGE);
        let mut success = false;
        let mut i = 0;
        while !success && i < SEARCHED_PER_TICK {
            i += 1;
            let Some(entry) = self.queue.peek().copied() else {
                break;
            };
            if self.visited.contains(&entry.pos) {
                self.queue.pop();
                continue;
            }
            if !simulate {
                self.visited.insert(entry.pos);
            }
            if self.visited.len() >= THRESHOLD {
                self.infinite = true;
                if !FILL_INFINITE {
                    self.visited.clear();
                    self.queue.clear();
                    return false;
                }
            }
            match fill_type(level.space(entry.pos), fluid) {
                Fill::Blocking => continue,
                Fill::Fillable => {
                    success = true;
                    if !simulate {
                        level.effect(entry.pos, fluid, false, false);
                        level.fill(entry.pos, fluid);
                    }
                }
                Fill::Filled => {}
            }
            if simulate && success {
                return true;
            }
            self.visited.insert(entry.pos);
            self.queue.pop();
            for o in &SIDES[..] {
                if *o == [0, 1, 0] {
                    continue;
                }
                let next = add(entry.pos, *o);
                if self.visited.contains(&next) || dist_sq(next, root) > range_sq {
                    continue;
                }
                if fill_type(level.space(next), fluid) != Fill::Blocking {
                    self.queue
                        .push(Entry::new(next, entry.distance + 1, root, false));
                }
            }
        }
        success
    }
}

/// `HosePulleyBlock.getStateForPlacement`: its shaft toward a shaft beside it
/// (`HorizontalKineticBlock.getPreferredHorizontalFacing`), else its pipe side toward something a
/// pipe joins, else facing the player; sides on two axes count as none. `shaft` and `pipe` say
/// what is on each side.
pub fn placement(
    shaft: impl Fn(crate::kinetics::Dir) -> bool,
    pipe: impl Fn(crate::kinetics::Dir) -> bool,
    horizontal: crate::kinetics::Dir,
) -> crate::kinetics::Dir {
    use crate::kinetics::{Axis, Dir};
    let find = |test: &dyn Fn(Dir) -> bool| {
        let mut preferred: Option<Dir> = None;
        // Iterate.horizontalDirections: south, west, north, east.
        for side in [Dir::South, Dir::West, Dir::North, Dir::East] {
            if !test(side) {
                continue;
            }
            if preferred.is_some_and(|p| p.axis() != side.axis()) {
                return None;
            }
            preferred = Some(side);
        }
        preferred
    };
    let preferred = find(&shaft).or_else(|| find(&pipe).map(Dir::opposite));
    match preferred {
        // `getCounterClockWise`.
        Some(p) => crate::wrench::clockwise(p, Axis::Y).opposite(),
        None => horizontal.opposite(),
    }
}

/// `HosePulleyBlock.hasPipeTowards`: the side counterclockwise of its facing.
pub fn pipe_side(facing: crate::kinetics::Dir) -> crate::kinetics::Dir {
    crate::wrench::clockwise(facing, crate::kinetics::Axis::Y).opposite()
}

/// The pieces of a hose drawn down from its pulley (`AbstractPulleyRenderer.renderSafe`, a
/// hose always running), each with how far down it is drawn (blocks).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece {
    /// The magnet at the hose's end, or its half while near the top.
    Magnet(f32),
    HalfMagnet(f32),
    /// The half piece where the hose crosses a block's edge.
    HalfRope(f32),
    /// A whole piece of hose, the `i`th from the bottom.
    Rope(u8, f32),
}

/// The most pieces of rope a client draws for one hose.
pub const MAX_DRAWN: u8 = 255;

/// The pieces for a hose down `offset` blocks (`getInterpolatedOffset`: at least 3/16).
pub fn pieces(offset: f32) -> Vec<Piece> {
    let offset = offset.max(3.0 / 16.0);
    let mut out = vec![if offset > 0.25 {
        Piece::Magnet(offset)
    } else {
        Piece::HalfMagnet(offset)
    }];
    let f = offset % 1.0;
    if offset > 0.75 && !(0.25..=0.75).contains(&f) {
        out.push(Piece::HalfRope(if f > 0.75 { f - 1.0 } else { f }));
    }
    let mut i = 0;
    while (i as f32) < offset - 1.25 && i < u32::from(MAX_DRAWN - 2) {
        out.push(Piece::Rope(i as u8, offset - i as f32 - 1.0));
        i += 1;
    }
    out
}

/// The channel hoses are sent to clients on: each pulley's offset and blocks a tick it moves,
/// or none for a pulley gone.
pub const CHANNEL: &str = "create:hose";
const RECORD: usize = 21;

/// A pulley and its hose (offset, blocks a tick it moves), or none for one gone.
pub type Record = ([i32; 3], Option<(f32, f32)>);

pub fn encode(all: &[Record]) -> Vec<Vec<u8>> {
    all.chunks(30_000 / RECORD)
        .map(|chunk| {
            let mut out = Vec::with_capacity(chunk.len() * RECORD);
            for (p, h) in chunk {
                for v in p {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                out.push(u8::from(h.is_some()));
                let (o, v) = h.unwrap_or_default();
                out.extend_from_slice(&o.to_le_bytes());
                out.extend_from_slice(&v.to_le_bytes());
            }
            out
        })
        .collect()
}

pub fn decode(data: &[u8]) -> Vec<Record> {
    let i = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    let f = |b: &[u8]| f32::from_le_bytes(b.try_into().unwrap());
    data.chunks_exact(RECORD)
        .map(|r| {
            let h = (r[12] != 0).then(|| (f(&r[13..17]), f(&r[17..21])));
            ([i(&r[0..4]), i(&r[4..8]), i(&r[8..12])], h)
        })
        .collect()
}

/// A hose pulley: how far its hose is down (`offset`, blocks), whether it moves, the speed
/// it turns at, its 1500 mB tank (`internalTank`) and its drainer and filler.
#[derive(Clone, Debug, Default)]
pub struct Hose {
    pub offset: f32,
    pub moving: bool,
    pub speed: f32,
    pub tank: Tank,
    pub drainer: Drainer,
    pub filler: Filler,
}

impl Hose {
    /// `getMovementSpeed`: `convertToLinear`, a 512th of the speed a tick.
    pub fn movement(&self) -> f32 {
        self.speed / 512.0
    }

    /// `rootPosGetter`: the block the hose ends in.
    pub fn root(&self, pos: [i32; 3]) -> [i32; 3] {
        below(pos, self.offset.ceil() as i32)
    }

    /// Whether it ticks: moving, or searching.
    pub fn busy(&self) -> bool {
        self.moving || self.drainer.searching() || self.filler.searching()
    }

    /// `onSpeedChanged`: it moves unless still, at the top going up, or blocked below;
    /// starting to move forgets what was found. A still hose ends on a whole block.
    pub fn speed_changed(&mut self, level: &impl Level, pos: [i32; 3], speed: f32, now: i64) {
        self.speed = speed;
        self.moving = true;
        if speed == 0.0 {
            self.offset = self.offset.round();
            self.moving = false;
        }
        if self.moving {
            let next = self.offset + self.movement();
            if next < 0.0 || !level.passable(below(pos, next.ceil() as i32)) {
                self.moving = false;
            }
            if self.moving {
                self.drainer.reset(now);
                self.filler.reset(now);
            }
        }
    }

    /// `tick` while moving: the hose goes down (or up) until the top, or a block it can't pass
    /// (it stops on a whole block). Its searches go on.
    pub fn tick(&mut self, level: &mut impl Level, pos: [i32; 3], now: i64) {
        if self.moving {
            let mut next = self.offset + self.movement();
            if next < 0.0 {
                next = 0.0;
                self.moving = false;
            }
            if !level.passable(below(pos, next.ceil() as i32)) {
                next = next.trunc();
                self.moving = false;
            }
            if self.speed == 0.0 {
                self.moving = false;
            }
            self.offset = next;
        }
        self.drainer.tick(level, now);
        self.filler.tick(level, now);
    }

    /// `lazyTick` of a still hose turning forwards: it moves again once the block below its end
    /// can be passed (the glue asks when that block changes).
    pub fn wake(&mut self, level: &impl Level, pos: [i32; 3], now: i64) -> bool {
        if self.moving || self.movement() <= 0.0 {
            return false;
        }
        let depth = (self.offset + self.movement()).ceil() as i32;
        if !level.passable(below(pos, depth)) {
            return false;
        }
        self.moving = true;
        self.drainer.reset(now);
        self.filler.reset(now);
        true
    }

    /// `HosePulleyFluidHandler.fill` (executed): what of `amount` mB of `fluid` it took. A
    /// still hose pours a bucket's worth into the world once its tank and this make one.
    pub fn fill(
        &mut self,
        level: &mut impl Level,
        pos: [i32; 3],
        fluid: Fluid,
        amount: u32,
        ultra_warm: bool,
        now: i64,
    ) -> u32 {
        if self.tank.fluid.is_some_and(|f| f != fluid) || amount == 0 || !fluid.placeable() {
            return 0;
        }
        let mut diff = i64::from(amount);
        let total = diff + i64::from(self.tank.amount);
        let mut remaining = amount;
        let mut deposited = false;
        if !self.moving && total >= i64::from(BUCKET) {
            let root = self.root(pos);
            if self
                .filler
                .try_deposit(level, fluid, root, false, ultra_warm, now)
            {
                self.drainer.counterpart_acted();
                remaining = remaining.saturating_sub(BUCKET);
                diff -= i64::from(BUCKET);
                deposited = true;
            }
        }
        if diff <= 0 {
            self.tank.drain_amount((-diff) as u32);
            return amount;
        }
        self.tank.fill_amount(fluid, remaining, CAPACITY) + if deposited { BUCKET } else { 0 }
    }

    /// `HosePulleyFluidHandler.drain` (`drainInternal`): up to `max` mB. Under a bucket in its
    /// tank, a still hose draws a block from the world (simulated: only says what it would
    /// give); what is left of it stays in the tank.
    pub fn drain(
        &mut self,
        level: &mut impl Level,
        pos: [i32; 3],
        max: u32,
        simulate: bool,
        now: i64,
    ) -> Option<(Fluid, u32)> {
        let from_tank = |t: &mut Tank, simulate: bool| {
            let f = t.fluid?;
            let n = t.amount.min(max);
            if n == 0 {
                return None;
            }
            if !simulate {
                t.drain_amount(n);
            }
            Some((f, n))
        };
        if self.tank.amount >= BUCKET {
            return from_tank(&mut self.tank, simulate);
        }
        let root = self.root(pos);
        let returned = self.drainer.drainable(level, pos, root, now);
        if self.moving || !self.drainer.pull_next(level, pos, root, simulate, now) {
            return from_tank(&mut self.tank, simulate);
        }
        self.filler.counterpart_acted();
        let Some(fluid) = returned else {
            return from_tank(&mut self.tank, simulate);
        };
        if self.tank.fluid.is_some_and(|f| f != fluid) && self.tank.amount > 0 {
            return from_tank(&mut self.tank, simulate);
        }
        let available = BUCKET + self.tank.amount;
        let drained = max.min(available);
        if !simulate {
            let left = available - drained;
            self.tank = Tank {
                fluid: (left > 0).then_some(fluid),
                amount: left,
            };
        }
        Some((fluid, drained))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A world of blocks by position, solid where unset below y 0, air above; it records the
    /// effects.
    #[derive(Default)]
    struct World {
        blocks: HashMap<[i32; 3], Space>,
        effects: usize,
    }

    impl Level for World {
        fn loaded(&self, _: [i32; 3]) -> bool {
            true
        }
        fn space(&self, p: [i32; 3]) -> Space {
            self.blocks.get(&p).copied().unwrap_or(if p[1] < 0 {
                Space::Solid
            } else {
                Space::Replaceable
            })
        }
        fn passable(&self, p: [i32; 3]) -> bool {
            matches!(self.space(p), Space::Replaceable | Space::Liquid(..))
        }
        fn empty(&mut self, p: [i32; 3]) {
            let s = match self.space(p) {
                Space::Waterloggable(_) => Space::Waterloggable(false),
                _ => Space::Replaceable,
            };
            self.blocks.insert(p, s);
        }
        fn fill(&mut self, p: [i32; 3], fluid: Fluid) {
            let s = match self.space(p) {
                Space::Waterloggable(_) => Space::Waterloggable(true),
                _ => Space::Liquid(fluid, true),
            };
            self.blocks.insert(p, s);
        }
        fn effect(&mut self, _: [i32; 3], _: Fluid, _: bool, _: bool) {
            self.effects += 1;
        }
    }

    /// A square pool of water sources, `r` blocks each way around x 0, z 0, at y -5..-4 (one
    /// deep), solid around and below.
    fn pool(r: i32) -> World {
        let mut w = World::default();
        for x in -r..=r {
            for z in -r..=r {
                w.blocks
                    .insert([x, -5, z], Space::Liquid(Fluid::Water, true));
            }
        }
        // Air from the pulley down to the pool.
        for y in -4..0 {
            w.blocks.insert([0, y, 0], Space::Replaceable);
        }
        w
    }

    /// Lowers the hose of a pulley at y 0 at `rpm` until it stops.
    fn lower(w: &mut World, rpm: f32) -> (Hose, i64) {
        let mut h = Hose::default();
        let mut now = 0;
        h.speed_changed(w, [0, 0, 0], rpm, now);
        while h.moving {
            now += 1;
            h.tick(w, [0, 0, 0], now);
        }
        (h, now)
    }

    #[test]
    fn a_pulley_turns_its_shaft_or_its_pipe_side_to_what_is_beside_it() {
        use crate::kinetics::Dir;
        // A shaft to the east: facing north (shaft side east, pipe side west).
        let f = placement(|d| d == Dir::East, |_| false, Dir::South);
        assert_eq!(f, Dir::North);
        assert_eq!(pipe_side(f), Dir::West);
        // A pipe to the west: the same.
        assert_eq!(
            placement(|_| false, |d| d == Dir::West, Dir::South),
            Dir::North
        );
        // Nothing: facing the player (looking south: north).
        assert_eq!(placement(|_| false, |_| false, Dir::South), Dir::North);
        // Shafts on two axes count as none; a pipe south then decides.
        let f = placement(
            |d| matches!(d, Dir::East | Dir::North),
            |d| d == Dir::South,
            Dir::East,
        );
        assert_eq!(pipe_side(f), Dir::South);
    }

    #[test]
    fn a_hose_is_drawn_in_pieces_and_sent_as_records() {
        assert_eq!(pieces(0.0), vec![Piece::HalfMagnet(3.0 / 16.0)]);
        // 3 blocks down: the magnet at 3, a half piece at the block's edge, whole pieces at 2
        // (and 1).
        assert_eq!(
            pieces(3.0),
            vec![
                Piece::Magnet(3.0),
                Piece::HalfRope(0.0),
                Piece::Rope(0, 2.0),
                Piece::Rope(1, 1.0)
            ]
        );
        let all = [([1, -2, 3], Some((2.5, 0.25))), ([4, 5, 6], None)];
        let m = encode(&all);
        assert_eq!(m.len(), 1);
        assert_eq!(decode(&m[0]), all.to_vec());
    }

    #[test]
    fn the_hose_goes_down_by_rotation_to_the_fluid_body() {
        let mut w = pool(1);
        // 256 RPM: half a block a tick; down through 4 blocks of air into the pool's block,
        // stopped by the solid floor below it.
        let (h, ticks) = lower(&mut w, 256.0);
        assert_eq!(h.offset, 5.0);
        assert_eq!(h.root([0, 0, 0]), [0, -5, 0]);
        assert_eq!(ticks, 11);
        assert!(!h.busy(), "a still hose has nothing to do");
        // Backwards it goes back up to the top.
        let mut h = h;
        let mut now = ticks;
        h.speed_changed(&w, [0, 0, 0], -256.0, now);
        while h.moving {
            now += 1;
            h.tick(&mut w, [0, 0, 0], now);
        }
        assert_eq!(h.offset, 0.0);
        // Stopped mid-way it rounds to a whole block.
        let mut h = Hose::default();
        h.speed_changed(&w, [0, 0, 0], 128.0, 0);
        for t in 1..=9 {
            h.tick(&mut w, [0, 0, 0], t);
        }
        h.speed_changed(&w, [0, 0, 0], 0.0, 9);
        assert_eq!((h.offset, h.moving), (2.0, false));
    }

    /// Drains `n` mB a tick from a still hose (as a pump does: simulate, then take), ticking
    /// its searches; returns what came out.
    fn pump_out(w: &mut World, h: &mut Hose, mut now: i64, ticks: i64, n: u32) -> u32 {
        let mut got = 0;
        for _ in 0..ticks {
            now += 1;
            if h.busy() {
                h.tick(w, [0, 0, 0], now);
            }
            if let Some((_, offer)) = h.drain(w, [0, 0, 0], n, true, now) {
                got += h.drain(w, [0, 0, 0], offer, false, now).map_or(0, |d| d.1);
            }
        }
        got
    }

    #[test]
    fn a_small_body_is_drawn_a_block_a_bucket() {
        // 3x3 sources: 9 buckets, the farthest first.
        let mut w = pool(1);
        let (mut h, now) = lower(&mut w, 256.0);
        let got = pump_out(&mut w, &mut h, now, 2000, 100);
        assert_eq!(got, 9 * BUCKET);
        let left = (-1..=1)
            .flat_map(|x| (-1..=1).map(move |z| [x, -5, z]))
            .filter(|p| w.space(*p) != Space::Replaceable)
            .count();
        assert_eq!(left, 0);
        assert!(!h.drainer.infinite);
        assert!(w.effects >= 9);
    }

    #[test]
    fn a_body_over_the_threshold_is_bottomless() {
        // 101x101 = 10201 sources >= 10,000.
        let mut w = pool(50);
        let (mut h, now) = lower(&mut w, 256.0);
        let got = pump_out(&mut w, &mut h, now, 200, 100);
        assert!(h.drainer.infinite, "the body is bottomless");
        assert!(got >= 15 * BUCKET, "drawn on: {got}");
        let left = (-50..=50)
            .flat_map(|x| (-50..=50).map(move |z| [x, -5, z]))
            .filter(|p| w.space(*p) == Space::Liquid(Fluid::Water, true))
            .count();
        assert_eq!(left, 101 * 101, "no block taken");
    }

    #[test]
    fn a_still_hose_pours_a_source_a_bucket_lowest_and_nearest_first() {
        // A 3x3 basin one deep at y -5 under 4 blocks of air; the hose ends in its middle.
        let mut w = World::default();
        for x in -1..=1 {
            for z in -1..=1 {
                w.blocks.insert([x, -5, z], Space::Replaceable);
            }
        }
        for y in -4..0 {
            w.blocks.insert([0, y, 0], Space::Replaceable);
        }
        let (mut h, mut now) = lower(&mut w, 256.0);
        assert_eq!(h.root([0, 0, 0]), [0, -5, 0]);
        let mut sources = 0;
        for _ in 0..400 {
            now += 1;
            if h.busy() {
                h.tick(&mut w, [0, 0, 0], now);
            }
            h.fill(&mut w, [0, 0, 0], Fluid::Water, 100, false, now);
            sources = w
                .blocks
                .values()
                .filter(|s| **s == Space::Liquid(Fluid::Water, true))
                .count();
        }
        // The basin's 9 blocks (never up from the hose's end); then its tank fills and takes
        // no more.
        assert_eq!(sources, 9);
        assert_eq!(w.space([0, -5, 0]), Space::Liquid(Fluid::Water, true));
        assert_eq!(w.space([0, -4, 0]), Space::Replaceable);
        assert_eq!(h.tank.amount, CAPACITY);
        assert_eq!(h.fill(&mut w, [0, 0, 0], Fluid::Water, 100, false, now), 0);
        // Honey has no block in the world: refused.
        assert_eq!(h.fill(&mut w, [0, 0, 0], Fluid::Honey, 100, false, now), 0);
        // Water poured in the nether boils away, taken all the same.
        let mut hot = Hose {
            tank: Tank {
                fluid: Some(Fluid::Water),
                amount: 900,
            },
            ..h.clone()
        };
        let before = w.blocks.clone();
        for t in 0..3 {
            hot.fill(&mut w, [0, 0, 0], Fluid::Water, 100, true, now + 1 + t);
        }
        assert_eq!(w.blocks, before);
    }
}
