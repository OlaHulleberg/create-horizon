//! Create's fluid tank (`FluidTankBlock`, `FluidTankBlockEntity`, `FluidTankRenderer`) and its
//! multiblocks (`ConnectivityHandler`): tanks placed together form tanks up to 3 wide and 32 high
//! with the lowest corner as controller; 8 buckets (`fluidTankCapacity`) of one fluid a block,
//! buckets in and out for creative players (Create 6 leaves survival to spouts and drains), drawn
//! as a box of the fluid at its level over the whole tank.

use std::collections::{HashMap, HashSet};

/// The fluids a tank can hold without fluids of its own yet (gap 5): vanilla's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fluid {
    Water,
    Lava,
    /// Create's own (`AllFluids`): kept in its machines; no fluid blocks in the world yet.
    Honey,
    Chocolate,
    /// NeoForge's milk (`NeoForgeMod.MILK`), which Create enables.
    Milk,
}

impl Fluid {
    pub const ALL: [Fluid; 5] = [
        Fluid::Water,
        Fluid::Lava,
        Fluid::Honey,
        Fluid::Chocolate,
        Fluid::Milk,
    ];

    pub fn index(self) -> u8 {
        self as u8
    }
    pub fn from_index(i: u8) -> Option<Fluid> {
        Fluid::ALL.get(usize::from(i)).copied()
    }
    /// Its registry id.
    pub fn id(self) -> &'static str {
        match self {
            Fluid::Water => "minecraft:water",
            Fluid::Lava => "minecraft:lava",
            Fluid::Honey => "create:honey",
            Fluid::Chocolate => "create:chocolate",
            Fluid::Milk => "minecraft:milk",
        }
    }
    /// A recipe's fluid by id or by its tag (`c:honey`, `c:milk`, `minecraft:water`...).
    pub fn of_id(id: &str) -> Option<Fluid> {
        Some(match id {
            "minecraft:water" | "c:water" => Fluid::Water,
            "minecraft:lava" | "c:lava" => Fluid::Lava,
            "create:honey" | "c:honey" => Fluid::Honey,
            "create:chocolate" | "c:chocolate" => Fluid::Chocolate,
            "minecraft:milk" | "c:milk" => Fluid::Milk,
            _ => return None,
        })
    }
    /// The bucket that holds it.
    pub fn bucket(self) -> &'static str {
        match self {
            Fluid::Water => "minecraft:water_bucket",
            Fluid::Lava => "minecraft:lava_bucket",
            Fluid::Honey => "create:honey_bucket",
            Fluid::Chocolate => "create:chocolate_bucket",
            Fluid::Milk => "minecraft:milk_bucket",
        }
    }
    pub fn of_bucket(item: &str) -> Option<Fluid> {
        Fluid::ALL.into_iter().find(|f| f.bucket() == item)
    }
    /// Whether it can be let out into the world as a source block (vanilla's fluids only here).
    pub fn placeable(self) -> bool {
        matches!(self, Fluid::Water | Fluid::Lava)
    }
}

/// `FluidType.BUCKET_VOLUME`.
pub const BUCKET: u32 = 1000;
/// `getCapacityMultiplier`: 8 buckets a block (`fluidTankCapacity`).
pub const CAPACITY: u32 = 8 * BUCKET;
/// `FluidTankBlockEntity.MAX_SIZE`: the widest tank.
pub const MAX_WIDTH: i32 = 3;
/// `getMaxHeight` (`fluidTankMaxHeight`): the tallest tank.
pub const MAX_HEIGHT: i32 = 32;

/// A tank's contents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tank {
    pub fluid: Option<Fluid>,
    pub amount: u32,
}

impl Tank {
    /// `FluidHelper.tryEmptyItemIntoBE` for a bucket: all of it or nothing.
    pub fn fill(&mut self, fluid: Fluid, capacity: u32) -> bool {
        if self.fluid.is_some_and(|f| f != fluid) || self.amount + BUCKET > capacity {
            return false;
        }
        self.fluid = Some(fluid);
        self.amount += BUCKET;
        true
    }

    /// `FluidTank.fill`: as much of `amount` as fits; what it took.
    pub fn fill_amount(&mut self, fluid: Fluid, amount: u32, capacity: u32) -> u32 {
        if self.fluid.is_some_and(|f| f != fluid) {
            return 0;
        }
        let took = amount.min(capacity.saturating_sub(self.amount));
        if took > 0 {
            self.fluid = Some(fluid);
            self.amount += took;
        }
        took
    }

    /// `FluidTank.drain`: up to `amount`; what came out.
    pub fn drain_amount(&mut self, amount: u32) -> Option<(Fluid, u32)> {
        let fluid = self.fluid?;
        let out = amount.min(self.amount);
        self.amount -= out;
        if self.amount == 0 {
            self.fluid = None;
        }
        (out > 0).then_some((fluid, out))
    }

    /// `FluidHelper.tryFillItemFromBE` for an empty bucket: a whole bucket's worth or nothing.
    pub fn drain(&mut self) -> Option<Fluid> {
        let fluid = self.fluid?;
        if self.amount < BUCKET {
            return None;
        }
        self.amount -= BUCKET;
        if self.amount == 0 {
            self.fluid = None;
        }
        Some(fluid)
    }

    /// `applyFluidTankSize`: what is over `capacity` is lost.
    fn truncate(&mut self, capacity: u32) {
        if self.amount > capacity {
            self.drain_amount(self.amount - capacity);
        }
    }
}

/// `FluidTankBlock.Shape` (`shape`): where a tank block shows a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Plain,
    Window,
    WindowNw,
    WindowSw,
    WindowNe,
    WindowSe,
}

impl Shape {
    pub fn name(self) -> &'static str {
        match self {
            Shape::Plain => "plain",
            Shape::Window => "window",
            Shape::WindowNw => "window_nw",
            Shape::WindowSw => "window_sw",
            Shape::WindowNe => "window_ne",
            Shape::WindowSe => "window_se",
        }
    }

    /// `setWindows` for the block `x`, `z` from the controller: one block wide every block has a
    /// window, two wide each its corner window, three wide the middles of the sides; none
    /// without windows.
    pub fn of(window: bool, width: u8, x: i32, z: i32) -> Shape {
        if !window {
            return Shape::Plain;
        }
        match width {
            1 => Shape::Window,
            2 => match (x == 0, z == 0) {
                (true, true) => Shape::WindowNw,
                (true, false) => Shape::WindowSw,
                (false, true) => Shape::WindowNe,
                (false, false) => Shape::WindowSe,
            },
            3 if (x.abs() - z.abs()).abs() == 1 => Shape::Window,
            _ => Shape::Plain,
        }
    }
}

/// A tank's controller (`FluidTankBlockEntity` with `isController`): its size, whether it has
/// windows, and the contents of the whole tank.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Multi {
    pub width: u8,
    pub height: u8,
    pub window: bool,
    pub tank: Tank,
}

impl Default for Multi {
    /// A tank as placed: one block, windows, empty.
    fn default() -> Self {
        Multi {
            width: 1,
            height: 1,
            window: true,
            tank: Tank::default(),
        }
    }
}

impl Multi {
    /// `getTotalTankSize`.
    pub fn blocks(&self) -> i32 {
        i32::from(self.width) * i32::from(self.width) * i32::from(self.height)
    }
    pub fn capacity(&self) -> u32 {
        self.blocks() as u32 * CAPACITY
    }
}

/// The block states of a tank block (`notifyMultiUpdated`, `setWindows`, `removeController`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockShape {
    pub top: bool,
    pub bottom: bool,
    pub shape: Shape,
}

/// Every tank of a dimension (`ConnectivityHandler` over `FluidTankBlockEntity`s): each loaded
/// tank block and its controller, each controller's tank. Tanks form and split only when a tank
/// block is placed or removed.
#[derive(Default)]
pub struct Tanks {
    /// Each loaded tank block and its controller (itself for a controller).
    pub parts: HashMap<[i32; 3], [i32; 3]>,
    /// Controllers and their tank.
    pub multis: HashMap<[i32; 3], Multi>,
    /// Blocks whose part of a tank changed: their block state, drawing and boiler follow.
    pub touched: Vec<[i32; 3]>,
    /// Blocks that stopped being part of a bigger tank (`removeController`: `boiler.clear()`).
    pub cleared: Vec<[i32; 3]>,
    /// Tanks that look for a tank to form again (`updateConnectivity`).
    pending: Vec<[i32; 3]>,
}

fn add(p: [i32; 3], x: i32, y: i32, z: i32) -> [i32; 3] {
    [p[0] + x, p[1] + y, p[2] + z]
}

/// The blocks of a tank of `width` and `height` from `origin`, in Create's order (y, x, z).
pub fn blocks(origin: [i32; 3], width: u8, height: u8) -> impl Iterator<Item = [i32; 3]> {
    let (w, h) = (i32::from(width), i32::from(height));
    (0..h).flat_map(move |y| (0..w).flat_map(move |x| (0..w).map(move |z| add(origin, x, y, z))))
}

impl Tanks {
    /// The controller of the tank block at `p`, when both are loaded.
    #[inline]
    pub fn controller(&self, p: [i32; 3]) -> Option<[i32; 3]> {
        let c = *self.parts.get(&p)?;
        self.multis.contains_key(&c).then_some(c)
    }

    /// The tank the block at `p` is part of, and its controller.
    pub fn get(&self, p: [i32; 3]) -> Option<([i32; 3], &Multi)> {
        let c = *self.parts.get(&p)?;
        Some((c, self.multis.get(&c)?))
    }

    pub fn get_mut(&mut self, p: [i32; 3]) -> Option<([i32; 3], &mut Multi)> {
        let c = *self.parts.get(&p)?;
        Some((c, self.multis.get_mut(&c)?))
    }

    /// The block states the tank block at `p` should have, when its tank is loaded.
    pub fn block_shape(&self, p: [i32; 3]) -> Option<BlockShape> {
        let (c, m) = self.get(p)?;
        Some(BlockShape {
            bottom: c[1] == p[1],
            top: c[1] + i32::from(m.height) - 1 == p[1],
            shape: Shape::of(m.window, m.width, p[0] - c[0], p[2] - c[2]),
        })
    }

    /// A tank block found in a loaded column: the controller of a saved tank (`saved`) takes
    /// its loaded blocks back; a block inside a loaded tank joins it; anything else is a tank
    /// of its own. Nothing forms (`read` without `Uninitialized`).
    pub fn load(&mut self, p: [i32; 3], saved: Option<Multi>) {
        if self.parts.contains_key(&p) {
            return;
        }
        if let Some(m) = saved {
            self.parts.insert(p, p);
            self.multis.insert(p, m);
            for q in blocks(p, m.width, m.height) {
                if q != p && self.parts.contains_key(&q) {
                    self.multis.remove(&q);
                    self.parts.insert(q, p);
                }
                self.touched.push(q);
            }
            return;
        }
        // The controller of a loaded tank holding `p`: at most 2 west and north, 31 down, with
        // a tank block under `p` all the way down (its column loads from the bottom).
        for dy in 0..MAX_HEIGHT {
            if dy > 0 && !self.parts.contains_key(&add(p, 0, -dy, 0)) {
                break;
            }
            for dx in 0..MAX_WIDTH {
                for dz in 0..MAX_WIDTH {
                    let c = add(p, -dx, -dy, -dz);
                    if let Some(m) = self.multis.get(&c)
                        && dx < i32::from(m.width)
                        && dz < i32::from(m.width)
                        && dy < i32::from(m.height)
                        && self.parts.get(&c) == Some(&c)
                    {
                        self.parts.insert(p, c);
                        return;
                    }
                }
            }
        }
        self.parts.insert(p, p);
        self.multis.insert(p, Multi::default());
    }

    /// `FluidTankBlock.onPlace` for a placed tank block: it looks for a tank to form
    /// (`updateConnectivity`, `ConnectivityHandler.formMulti`).
    pub fn place(&mut self, p: [i32; 3]) {
        if self.parts.contains_key(&p) {
            return;
        }
        self.parts.insert(p, p);
        self.multis.insert(p, Multi::default());
        self.touched.push(p);
        self.form(p);
        self.settle();
    }

    /// `FluidTankBlock.onRemove`: the tank it was part of splits (`splitMulti`), the fluid shared
    /// among the blocks left, and those blocks form tanks again.
    pub fn remove(&mut self, p: [i32; 3]) {
        let Some(c) = self.parts.remove(&p) else {
            return;
        };
        self.touched.push(p);
        if c == p {
            if let Some(m) = self.multis.remove(&p) {
                self.cleared.push(p);
                self.split_at(p, m, true);
            }
        } else if let Some(m) = self.multis.get(&c).copied() {
            self.split_at(c, m, false);
        }
        self.settle();
    }

    /// A column unloaded: its blocks and controllers go; blocks elsewhere of a tank whose
    /// controller went wait for it (`getControllerBE` is null).
    pub fn unload_column(&mut self, cx: i32, cz: i32) {
        let here = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (cx, cz);
        self.multis.retain(|c, _| !here(c));
        self.parts.retain(|p, _| !here(p));
    }

    /// Tanks whose blocks were split form again (`updateConnectivity` on their next tick, here
    /// at once, in the order they were split).
    fn settle(&mut self) {
        let mut i = 0;
        while i < self.pending.len() {
            let p = self.pending[i];
            i += 1;
            if self.parts.get(&p) == Some(&p) && self.multis.contains_key(&p) {
                self.form(p);
            }
        }
        self.pending.clear();
    }

    /// `preventConnectivityUpdate`.
    fn prevent(&mut self, p: [i32; 3]) {
        self.pending.retain(|q| *q != p);
    }

    /// `ConnectivityHandler.formMulti` from the tank at `start`: every tank block reachable
    /// westward, downward and northward (at most 3 west and north) is looked at as the corner of
    /// a new tank; the biggest are formed first.
    fn form(&mut self, start: [i32; 3]) {
        let (min_x, min_z) = (start[0] - MAX_WIDTH, start[2] - MAX_WIDTH);
        let mut frontier = std::collections::VecDeque::from([start]);
        let mut visited = HashSet::new();
        let mut queue: Vec<(i32, [i32; 3])> = Vec::new();
        while let Some(part) = frontier.pop_front() {
            if !visited.insert(part) {
                continue;
            }
            let amount = self.try_form(part, true);
            if amount > 1 {
                queue.push((amount, part));
            }
            for next in [
                add(part, -1, 0, 0),
                add(part, 0, -1, 0),
                add(part, 0, 0, -1),
            ] {
                if next[0] <= min_x || next[2] <= min_z || visited.contains(&next) {
                    continue;
                }
                if self.parts.contains_key(&next) {
                    frontier.push_back(next);
                }
            }
        }
        visited.clear();
        // `makeCreationQueue`: the biggest first (ties in the order found).
        queue.sort_by_key(|(amount, _)| std::cmp::Reverse(*amount));
        for (_, p) in queue {
            if visited.insert(p) {
                self.try_form(p, false);
            }
        }
    }

    /// `tryToFormNewMulti`: the width (1 to 3) that makes the biggest tank from the controller
    /// `be`, the wider on a tie; formed unless `simulate`. Its size in blocks.
    fn try_form(&mut self, be: [i32; 3], simulate: bool) -> i32 {
        let Some(m) = self.multis.get(&be).copied() else {
            return 0;
        };
        if self.parts.get(&be) != Some(&be) {
            return 0;
        }
        let (mut best_width, mut best_amount) = (1u8, -1);
        for w in 1..=MAX_WIDTH as u8 {
            let amount = self.of_width(be, w, None);
            if amount < best_amount {
                continue;
            }
            best_width = w;
            best_amount = amount;
        }
        if simulate {
            return best_amount;
        }
        if m.width == best_width && m.blocks() == best_amount {
            return best_amount;
        }
        // Create would make a tank of no height here (a full-height tank that can't grow).
        if best_amount < 1 {
            return best_amount;
        }
        self.split(be);
        let capacity = best_amount as u32 * CAPACITY;
        if let Some(m) = self.multis.get_mut(&be) {
            m.tank.truncate(capacity);
        }
        self.of_width(be, best_width, Some(capacity));
        self.prevent(be);
        if let Some(m) = self.multis.get_mut(&be) {
            m.width = best_width;
            m.height = (best_amount / i32::from(best_width) / i32::from(best_width)) as u8;
            // `notifyMultiUpdated`, `setWindows` over the whole tank.
            let (w, h) = (m.width, m.height);
            self.touched.extend(blocks(be, w, h));
        }
        best_amount
    }

    /// `tryToFormNewMultiOfWidth`: how many blocks a tank `width` wide from `origin` would hold,
    /// layer by layer while every block is a tank inside it whose fluid matches; with
    /// `capacity` (not simulating) the blocks join it, their fluid poured into it.
    fn of_width(&mut self, origin: [i32; 3], width: u8, capacity: Option<u32>) -> i32 {
        let Some(own) = self.multis.get(&origin).copied() else {
            return 0;
        };
        let fluid = own.tank.fluid;
        let w = i32::from(width);
        let (mut amount, mut height) = (0, 0u8);
        'search: for y in 0..MAX_HEIGHT {
            for x in 0..w {
                for z in 0..w {
                    let pos = add(origin, x, y, z);
                    let Some((c, other)) = self.get(pos) else {
                        break 'search;
                    };
                    if other.width > width
                        || (other.width == width && i32::from(other.height) == MAX_HEIGHT)
                    {
                        break 'search;
                    }
                    let ow = i32::from(other.width);
                    if c != origin
                        && (c[0] < origin[0]
                            || c[2] < origin[2]
                            || c[0] + ow > origin[0] + w
                            || c[2] + ow > origin[2] + w)
                    {
                        break 'search;
                    }
                    if let (Some(f), Some(o)) = (fluid, other.tank.fluid)
                        && f != o
                    {
                        break 'search;
                    }
                }
            }
            amount += w * w;
            height += 1;
        }
        let Some(capacity) = capacity else {
            return amount;
        };
        for pos in blocks(origin, width, height) {
            if pos == origin || !self.parts.contains_key(&pos) {
                continue;
            }
            // A controller's fluid is poured into the new tank (what doesn't fit is lost).
            if self.parts.get(&pos) == Some(&pos)
                && let Some(t) = self
                    .multis
                    .get_mut(&pos)
                    .map(|m| std::mem::take(&mut m.tank))
                && let Some(f) = t.fluid
                && let Some(m) = self.multis.get_mut(&origin)
            {
                m.tank.fill_amount(f, t.amount, capacity);
            }
            self.split(pos);
            // `setController`: it is part of the new tank.
            self.multis.remove(&pos);
            self.parts.insert(pos, origin);
            self.prevent(pos);
            self.touched.push(pos);
        }
        amount
    }

    /// `splitMultiAndInvalidate` for the tank the block at `part` is in.
    fn split(&mut self, part: [i32; 3]) {
        let Some((c, m)) = self.get(part).map(|(c, m)| (c, *m)) else {
            return;
        };
        self.split_at(c, m, false);
    }

    /// `splitMultiAndInvalidate` for the controller `c` (gone when `removed`), unless it is one
    /// block: each of its blocks becomes a tank of its own (`removeController`) with the tank's
    /// windows; the controller keeps a block's worth of fluid, the rest is shared out a block's
    /// worth each in order (a removed controller's share is lost); the blocks form again later.
    fn split_at(&mut self, c: [i32; 3], m: Multi, removed: bool) {
        if m.width == 1 && m.height == 1 {
            return;
        }
        let mut share = m.tank;
        if !removed {
            share.amount = share.amount.saturating_sub(CAPACITY);
            if share.amount == 0 {
                share.fluid = None;
            }
            if let Some(own) = self.multis.get_mut(&c) {
                own.tank.truncate(CAPACITY);
            }
        }
        for pos in blocks(c, m.width, m.height) {
            if self.parts.get(&pos) != Some(&c) {
                continue;
            }
            let mut tank = if pos == c {
                self.multis.get(&c).map(|m| m.tank).unwrap_or_default()
            } else {
                Tank::default()
            };
            if let Some(f) = share.fluid
                && pos != c
            {
                let split = share.amount.min(CAPACITY);
                tank.fill_amount(f, split, CAPACITY);
                share.drain_amount(split);
            }
            self.parts.insert(pos, pos);
            self.multis.insert(
                pos,
                Multi {
                    tank,
                    window: m.window,
                    ..Multi::default()
                },
            );
            self.cleared.push(pos);
            self.touched.push(pos);
            self.pending.push(pos);
        }
    }
}

/// `FluidTankRenderer.renderSafe` for a controller: the fluid's box over the whole tank, from its
/// floor (cap and puddle above the bottom) up to its level, inside the hull; None when it is too
/// little to show or the tank has no windows (a boiler).
pub fn fluid_box(m: &Multi) -> Option<([f32; 3], [f32; 3])> {
    m.tank.fluid?;
    if !m.window {
        return None;
    }
    let (height, width) = (f32::from(m.height), f32::from(m.width));
    let cap = 1.0 / 4.0;
    let hull = 1.0 / 16.0 + 1.0 / 128.0;
    let puddle = 1.0 / 16.0;
    let total = height - 2.0 * cap - puddle;
    let level = m.tank.amount as f32 / m.capacity() as f32;
    if level < 1.0 / (512.0 * total) {
        return None;
    }
    let clamped = (level * total).clamp(0.0, total);
    // yMin = total + cap + puddle - clamped, then translated down by total - clamped.
    let y0 = cap + puddle;
    Some(([hull, y0, hull], [width - hull, y0 + clamped, width - hull]))
}

/// Tanks as clients draw them: position, width (0: no controller there), height, windows, fluid
/// (255: none), amount; 20 bytes each.
pub const CHANNEL: &str = "create:tank";
const RECORD: usize = 20;

pub fn encode(all: &[([i32; 3], Option<Multi>)]) -> Vec<Vec<u8>> {
    all.chunks(30_000 / RECORD)
        .map(|chunk| {
            let mut out = Vec::with_capacity(chunk.len() * RECORD);
            for (p, m) in chunk {
                for v in p {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                let m = m.unwrap_or(Multi {
                    width: 0,
                    ..Multi::default()
                });
                out.extend_from_slice(&[m.width, m.height, u8::from(m.window)]);
                out.push(m.tank.fluid.map_or(255, Fluid::index));
                out.extend_from_slice(&m.tank.amount.to_le_bytes());
            }
            out
        })
        .collect()
}

pub fn decode(data: &[u8]) -> Vec<([i32; 3], Option<Multi>)> {
    let i = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    data.chunks_exact(RECORD)
        .map(|r| {
            let m = Multi {
                width: r[12],
                height: r[13],
                window: r[14] != 0,
                tank: Tank {
                    fluid: Fluid::from_index(r[15]),
                    amount: u32::from_le_bytes(r[16..20].try_into().unwrap()),
                },
            };
            (
                [i(&r[0..4]), i(&r[4..8]), i(&r[8..12])],
                (m.width > 0).then_some(m),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn water(amount: u32) -> Tank {
        Tank {
            fluid: Some(Fluid::Water),
            amount,
        }
    }

    fn placed(blocks: &[[i32; 3]]) -> Tanks {
        let mut t = Tanks::default();
        for &p in blocks {
            t.place(p);
        }
        t
    }

    /// The controller and size of the tank the block at `p` is in.
    fn size(t: &Tanks, p: [i32; 3]) -> ([i32; 3], u8, u8) {
        let (c, m) = t.get(p).unwrap();
        (c, m.width, m.height)
    }

    #[test]
    fn buckets_go_in_and_out_whole() {
        let mut t = Tank::default();
        for _ in 0..8 {
            assert!(t.fill(Fluid::Water, CAPACITY));
        }
        assert!(!t.fill(Fluid::Water, CAPACITY), "eight buckets fill it");
        assert!(
            t.fill(Fluid::Water, 2 * CAPACITY),
            "a bigger tank takes more"
        );
        assert!(!water(1000).fill(Fluid::Lava, CAPACITY));
        assert_eq!(t.drain(), Some(Fluid::Water));
        assert_eq!(t.amount, 8000);
        let mut one = Tank {
            fluid: Some(Fluid::Lava),
            amount: BUCKET,
        };
        assert_eq!(one.drain(), Some(Fluid::Lava));
        assert_eq!(one, Tank::default());
        assert_eq!(one.drain(), None);
        let mut most = water(7000);
        assert_eq!(most.fill_amount(Fluid::Water, 5000, CAPACITY), 1000);
        assert_eq!(most.fill_amount(Fluid::Water, 5000, CAPACITY), 0);
    }

    #[test]
    fn the_fluid_rises_from_the_tank_floor() {
        let half = Multi {
            tank: water(CAPACITY / 2),
            ..Multi::default()
        };
        let (min, max) = fluid_box(&half).unwrap();
        assert_eq!(min[1], 0.3125);
        // Half of the 7/16 between the caps and the puddle.
        assert!((max[1] - (0.3125 + 0.4375 / 2.0)).abs() < 1e-6);
        assert!((min[0] - (1.0 / 16.0 + 1.0 / 128.0)).abs() < 1e-6);
        assert_eq!(fluid_box(&Multi::default()), None);
        // A 2x2x3 tank half full: 2 wide, half of the 2.4375 between its caps.
        let big = Multi {
            width: 2,
            height: 3,
            window: true,
            tank: water(6 * CAPACITY),
        };
        let (min, max) = fluid_box(&big).unwrap();
        assert!((max[0] - (2.0 - 1.0 / 16.0 - 1.0 / 128.0)).abs() < 1e-6);
        assert!((max[1] - (0.3125 + 2.4375 / 2.0)).abs() < 1e-6);
        assert_eq!(min[1], 0.3125);
        let boiler = Multi {
            window: false,
            ..big
        };
        assert_eq!(fluid_box(&boiler), None, "no windows, no fluid drawn");
        let all = vec![([1, 2, 3], Some(big)), ([-4, 5, 6], None)];
        assert_eq!(decode(&encode(&all)[0]), all);
    }

    #[test]
    fn four_tanks_form_a_two_by_two_tank_from_its_lowest_corner() {
        let t = placed(&[[0, 0, 0], [1, 0, 0], [0, 0, 1]]);
        assert_eq!(t.multis.len(), 3, "three are no square");
        let mut t = placed(&[[1, 0, 1], [0, 0, 1], [1, 0, 0], [0, 0, 0]]);
        assert_eq!(t.multis.len(), 1);
        assert_eq!(size(&t, [1, 0, 1]), ([0, 0, 0], 2, 1));
        assert_eq!(t.multis[&[0, 0, 0]].capacity(), 32_000);
        let shape = |t: &Tanks, p| t.block_shape(p).unwrap().shape;
        assert_eq!(shape(&t, [0, 0, 0]), Shape::WindowNw);
        assert_eq!(shape(&t, [0, 0, 1]), Shape::WindowSw);
        assert_eq!(shape(&t, [1, 0, 0]), Shape::WindowNe);
        assert_eq!(shape(&t, [1, 0, 1]), Shape::WindowSe);
        // A second layer: one 2x2x2 tank, the bottom layer bottom, the top top.
        for p in [[0, 1, 0], [1, 1, 0], [0, 1, 1], [1, 1, 1]] {
            t.place(p);
        }
        assert_eq!(t.multis.len(), 1);
        assert_eq!(size(&t, [1, 1, 1]), ([0, 0, 0], 2, 2));
        let low = t.block_shape([1, 0, 1]).unwrap();
        let high = t.block_shape([1, 1, 1]).unwrap();
        assert!(low.bottom && !low.top && !high.bottom && high.top);
        assert_eq!(high.shape, Shape::WindowSe);
    }

    #[test]
    fn columns_grow_up_to_the_highest_tank() {
        let t = placed(&[[0, 0, 0], [0, 1, 0], [0, 2, 0]]);
        assert_eq!(size(&t, [0, 2, 0]), ([0, 0, 0], 1, 3));
        let s = |p| t.block_shape(p).unwrap();
        assert!(s([0, 0, 0]).bottom && !s([0, 0, 0]).top);
        assert!(!s([0, 1, 0]).bottom && !s([0, 1, 0]).top);
        assert!(s([0, 2, 0]).top && s([0, 2, 0]).shape == Shape::Window);
        let column: Vec<[i32; 3]> = (0..=MAX_HEIGHT).map(|y| [0, y, 0]).collect();
        let t = placed(&column);
        assert_eq!(size(&t, [0, 31, 0]), ([0, 0, 0], 1, 32));
        assert_eq!(size(&t, [0, 32, 0]), ([0, 32, 0], 1, 1), "33 high is two");
    }

    #[test]
    fn three_wide_tanks_window_the_middles_of_their_sides() {
        let mut layer = Vec::new();
        for x in 0..3 {
            for z in 0..3 {
                layer.push([x, 0, z]);
            }
        }
        let t = placed(&layer);
        assert_eq!(size(&t, [2, 0, 2]), ([0, 0, 0], 3, 1));
        let shape = |p| t.block_shape(p).unwrap().shape;
        for p in [[1, 0, 0], [0, 0, 1], [2, 0, 1], [1, 0, 2]] {
            assert_eq!(shape(p), Shape::Window);
        }
        for p in [[0, 0, 0], [1, 0, 1], [2, 0, 2]] {
            assert_eq!(shape(p), Shape::Plain);
        }
        // A fourth row makes no 4-wide tank.
        let mut t = placed(&layer);
        for z in 0..3 {
            t.place([3, 0, z]);
        }
        assert_eq!(size(&t, [2, 0, 2]), ([0, 0, 0], 3, 1));
        assert_eq!(size(&t, [3, 0, 2]), ([3, 0, 2], 1, 1));
    }

    #[test]
    fn tanks_pour_their_fluid_together_unless_it_differs() {
        let mut t = Tanks::default();
        t.place([0, 0, 0]);
        t.multis.get_mut(&[0, 0, 0]).unwrap().tank = water(3000);
        t.place([1, 0, 0]);
        t.multis.get_mut(&[1, 0, 0]).unwrap().tank = water(5000);
        t.place([0, 0, 1]);
        t.place([1, 0, 1]);
        assert_eq!(t.multis.len(), 1);
        assert_eq!(t.multis[&[0, 0, 0]].tank, water(8000));
        // Lava beside water forms nothing.
        let mut t = Tanks::default();
        t.place([0, 0, 0]);
        t.multis.get_mut(&[0, 0, 0]).unwrap().tank = water(3000);
        t.place([1, 0, 0]);
        t.multis.get_mut(&[1, 0, 0]).unwrap().tank = Tank {
            fluid: Some(Fluid::Lava),
            amount: 1000,
        };
        t.place([0, 0, 1]);
        t.place([1, 0, 1]);
        assert_eq!(t.multis.len(), 4);
    }

    #[test]
    fn a_broken_block_splits_its_tank_and_shares_the_fluid() {
        let square = [[0, 0, 0], [1, 0, 0], [0, 0, 1], [1, 0, 1]];
        // A side block broken: the controller keeps 8 buckets, the others get 8 each in order,
        // the broken block's share is lost.
        let mut t = placed(&square);
        t.multis.get_mut(&[0, 0, 0]).unwrap().tank = water(32_000);
        t.touched.clear();
        t.remove([1, 0, 1]);
        assert_eq!(t.multis.len(), 3);
        for p in [[0, 0, 0], [0, 0, 1], [1, 0, 0]] {
            assert_eq!(size(&t, p), (p, 1, 1));
            assert_eq!(t.multis[&p].tank, water(8000));
            assert_eq!(t.block_shape(p).unwrap().shape, Shape::Window);
        }
        assert!(t.touched.contains(&[1, 0, 1]) && t.touched.contains(&[0, 0, 1]));
        // The controller broken: what it held is shared among the rest, in order.
        let mut t = placed(&square);
        t.multis.get_mut(&[0, 0, 0]).unwrap().tank = water(20_000);
        t.remove([0, 0, 0]);
        assert_eq!(t.multis[&[0, 0, 1]].tank, water(8000));
        assert_eq!(t.multis[&[1, 0, 0]].tank, water(8000));
        assert_eq!(t.multis[&[1, 0, 1]].tank, water(4000));
        // A 2x2x2 tank losing a top block: the blocks left form again (a 2x2 layer below,
        // three tanks above), keeping their fluid but the broken block's share.
        let mut cube = square.to_vec();
        cube.extend(square.iter().map(|p| [p[0], 1, p[2]]));
        let mut t = placed(&cube);
        assert_eq!(size(&t, [1, 1, 1]), ([0, 0, 0], 2, 2));
        t.multis.get_mut(&[0, 0, 0]).unwrap().tank = water(60_000);
        t.remove([1, 1, 1]);
        assert_eq!(size(&t, [1, 0, 1]), ([0, 0, 0], 2, 1));
        assert_eq!(t.multis.len(), 4);
        let total: u32 = t.multis.values().map(|m| m.tank.amount).sum();
        assert_eq!(total, 56_000);
        assert_eq!(t.multis[&[0, 0, 0]].tank, water(32_000));
    }

    #[test]
    fn loaded_tanks_take_their_blocks_back() {
        let saved = Multi {
            width: 2,
            height: 1,
            window: false,
            tank: water(9000),
        };
        // A block of it found before its controller is a tank of its own until then.
        let mut t = Tanks::default();
        t.load([1, 0, 1], None);
        t.load([0, 0, 0], Some(saved));
        t.load([0, 0, 1], None);
        t.load([1, 0, 0], None);
        assert_eq!(t.multis.len(), 1);
        assert_eq!(size(&t, [1, 0, 1]), ([0, 0, 0], 2, 1));
        assert_eq!(size(&t, [1, 0, 0]), ([0, 0, 0], 2, 1));
        assert_eq!(t.block_shape([1, 0, 0]).unwrap().shape, Shape::Plain);
        t.unload_column(0, 0);
        assert!(t.multis.is_empty() && t.parts.is_empty());
        // Its controller's column unloads: the rest wait for it.
        let mut t = Tanks::default();
        t.load([15, 0, 0], Some(saved));
        t.load([16, 0, 0], None);
        t.load([14, 0, 0], None);
        t.unload_column(0, 0);
        assert_eq!(t.controller([16, 0, 0]), None);
        assert!(t.parts.contains_key(&[16, 0, 0]));
        t.load([15, 0, 0], Some(saved));
        assert_eq!(t.controller([16, 0, 0]), Some([15, 0, 0]));
        // A tall tank's upper blocks find their controller below them.
        let tall = Multi { height: 3, ..saved };
        let mut t = Tanks::default();
        t.load([0, 0, 0], Some(tall));
        for y in 0..4 {
            t.load([1, y, 1], None);
        }
        assert_eq!(size(&t, [1, 2, 1]), ([0, 0, 0], 2, 3));
        assert_eq!(size(&t, [1, 3, 1]), ([1, 3, 1], 1, 1), "above it");
        t.load([5, 1, 5], None);
        assert_eq!(size(&t, [5, 1, 5]), ([5, 1, 5], 1, 1));
    }
}
