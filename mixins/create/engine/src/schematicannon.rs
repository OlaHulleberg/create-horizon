//! The schematicannon (`SchematicannonBlockEntity`, `SchematicPrinter`, `LaunchedItem`): a
//! deployed schematic printed block by block, one shot per `schematicannonDelay`, a gunpowder
//! worth `schematicannonShotsPerGunpowder` shots, blocks taken from the inventories beside it.
//!
//! Create ticks every cannon every tick. Here a cannon is ticked only when something can happen:
//! the tick its cooldown ends, a launched block lands, it is woken (its slots changed, a command,
//! a neighbour), or it waits for items, fuel or a loaded target (then at its shot rate). The
//! ticks it sleeps through are the ones where Create only counts its cooldown down (`settle`).

use crate::schematic::{Mirror, Rotation, SchematicData, Structure, transform};
use std::collections::VecDeque;

/// `CSchematics.schematicannonDelay`: ticks between shots.
pub const DELAY: u32 = 10;
/// `CSchematics.schematicannonShotsPerGunpowder`.
pub const SHOTS_PER_GUNPOWDER: i32 = 400;
/// `SchematicannonBlockEntity.MAX_ANCHOR_DISTANCE`.
pub const MAX_ANCHOR_DISTANCE: f64 = 256.0;
/// Skips a tick may go through (`skipsLeft`).
pub const SKIPS_PER_TICK: u32 = 1000;

/// `SchematicannonBlockEntity.State`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Stopped,
    Paused,
    Running,
}

impl State {
    pub const ALL: [State; 3] = [State::Stopped, State::Paused, State::Running];
    pub fn name(self) -> &'static str {
        ["STOPPED", "PAUSED", "RUNNING"][self as usize]
    }
}

/// `statusMsg`, typed: its names are Create's keys (`gui.schematicannon.status.*`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Ready,
    Running,
    Paused,
    Stopped,
    Finished,
    Searching,
    Skipping,
    Placing,
    Clearing,
    MissingBlock,
    NoGunpowder,
    TargetNotLoaded,
    TargetOutsideRange,
    SchematicInvalid,
    SchematicNotPlaced,
    SchematicErrored,
    SchematicExpired,
}

impl Status {
    pub const ALL: [Status; 18] = [
        Status::Idle,
        Status::Ready,
        Status::Running,
        Status::Paused,
        Status::Stopped,
        Status::Finished,
        Status::Searching,
        Status::Skipping,
        Status::Placing,
        Status::Clearing,
        Status::MissingBlock,
        Status::NoGunpowder,
        Status::TargetNotLoaded,
        Status::TargetOutsideRange,
        Status::SchematicInvalid,
        Status::SchematicNotPlaced,
        Status::SchematicErrored,
        Status::SchematicExpired,
    ];
    pub fn name(self) -> &'static str {
        [
            "idle",
            "ready",
            "running",
            "paused",
            "stopped",
            "finished",
            "searching",
            "skipping",
            "placing",
            "clearing",
            "missingBlock",
            "noGunpowder",
            "targetNotLoaded",
            "targetOutsideRange",
            "schematicInvalid",
            "schematicNotPlaced",
            "schematicErrored",
            "schematicExpired",
        ][self as usize]
    }
    pub fn parse(s: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|v| v.name() == s)
    }
}

/// `ConfigureSchematicannonPacket.Option`'s replace modes, by ordinal.
pub const DONT_REPLACE: u8 = 0;
pub const REPLACE_SOLID: u8 = 1;
pub const REPLACE_ANY: u8 = 2;
pub const REPLACE_EMPTY: u8 = 3;

/// `SchematicannonOptions`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub replace_mode: u8,
    pub skip_missing: bool,
    pub replace_block_entities: bool,
}

impl Default for Options {
    /// The block entity's constructor (`replaceMode = 2`).
    fn default() -> Self {
        Self {
            replace_mode: REPLACE_ANY,
            skip_missing: false,
            replace_block_entities: false,
        }
    }
}

impl Options {
    /// `applyImplicitComponents`' default for a cannon placed from an item without options.
    pub fn placed() -> Self {
        Self {
            replace_mode: REPLACE_ANY,
            skip_missing: true,
            replace_block_entities: false,
        }
    }
}

/// What a block of the schematic costs (`ItemRequirement`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Requirement {
    /// `ItemRequirement.NONE` (air).
    Empty,
    /// `ItemRequirement.INVALID`: no item places it.
    Invalid,
    /// Items consumed.
    Items(Vec<(String, u32)>),
}

/// One palette entry of a loaded schematic, resolved against the game once per load: the state
/// it prints (`setZeroAge`) and the one it places, and what the printer asks of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub state: u32,
    pub placed: u32,
    /// `state.isAir()`.
    pub air: bool,
    /// The block is `minecraft:air` (a clearing shot rather than a placing one).
    pub plain_air: bool,
    /// `isRedstoneConductor` (Create's `isNormalCube`).
    pub conductor: bool,
    pub requirement: Requirement,
    /// `shouldIgnoreBlockState`.
    pub ignore: bool,
    /// `shouldDeferBlock`.
    pub defer: bool,
    /// The other block a two-block state replaces, from it.
    pub other: Option<[i32; 3]>,
}

/// `SchematicPrinter.PrintStage`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Blocks,
    DeferredBlocks,
    Entities,
}

const NONE: u32 = u32::MAX;

/// `SchematicPrinter` over a schematic placed in its `SchematicLevel`: the blocks by position
/// relative to the anchor (a dense box), the level's bounds and where printing stands.
#[derive(Clone, Debug, PartialEq)]
pub struct Printer {
    pub anchor: [i32; 3],
    pub min: [i32; 3],
    pub max: [i32; 3],
    cells: Vec<u32>,
    set: usize,
    /// `currentPos`, relative to the anchor.
    pub current: [i32; 3],
    pub stage: Stage,
    pub deferred: VecDeque<[i32; 3]>,
}

impl Printer {
    /// `SchematicPrinter.loadSchematic`: the template placed into a `SchematicLevel` at the
    /// anchor (each block at `transform(pos)`, the level's bounds growing from the origin over
    /// them), the bounds grown over the transformed far corner, printing from before the first
    /// block. `entry` gives each block's palette index the entry the printer reads.
    pub fn load(s: &Structure, anchor: [i32; 3], rotation: Rotation, mirror: Mirror) -> Printer {
        let mut min = [0; 3];
        let mut max = [0; 3];
        let grow = |p: [i32; 3], min: &mut [i32; 3], max: &mut [i32; 3]| {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        };
        let placed: Vec<([i32; 3], u32)> = s
            .blocks
            .iter()
            .map(|b| (transform(b.pos, mirror, rotation, [0; 3]), b.state as u32))
            .collect();
        for (p, _) in &placed {
            grow(*p, &mut min, &mut max);
        }
        let far = [s.size[0] - 1, s.size[1] - 1, s.size[2] - 1];
        grow(transform(far, mirror, rotation, [0; 3]), &mut min, &mut max);
        let span = [0, 1, 2].map(|i| (max[i] - min[i] + 1) as usize);
        let mut p = Printer {
            anchor,
            min,
            max,
            cells: vec![NONE; span[0] * span[1] * span[2]],
            set: 0,
            current: [min[0] - 1, min[1], min[2]],
            stage: Stage::Blocks,
            deferred: VecDeque::new(),
        };
        for (pos, state) in placed {
            let i = p.index(pos).unwrap();
            if p.cells[i] == NONE {
                p.set += 1;
            }
            p.cells[i] = state;
        }
        p
    }

    fn span(&self) -> [i32; 3] {
        [0, 1, 2].map(|i| self.max[i] - self.min[i] + 1)
    }

    fn inside(&self, p: [i32; 3]) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }

    fn index(&self, p: [i32; 3]) -> Option<usize> {
        if !self.inside(p) {
            return None;
        }
        let s = self.span();
        let d = [0, 1, 2].map(|i| (p[i] - self.min[i]) as usize);
        Some((d[1] * s[2] as usize + d[2]) * s[0] as usize + d[0])
    }

    /// The palette index at a position relative to the anchor (`SchematicLevel.getBlockState`:
    /// none outside the bounds or where nothing was placed, which reads as air).
    pub fn at(&self, rel: [i32; 3]) -> Option<u32> {
        let v = self.cells[self.index(rel)?];
        (v != NONE).then_some(v)
    }

    /// `isWorldEmpty`.
    pub fn is_empty(&self) -> bool {
        self.set == 0
    }

    /// Every placed position and its palette index (`getAllPositions`).
    pub fn positions(&self) -> impl Iterator<Item = ([i32; 3], u32)> + '_ {
        let s = self.span();
        self.cells.iter().enumerate().filter_map(move |(i, &v)| {
            if v == NONE {
                return None;
            }
            let x = (i % s[0] as usize) as i32;
            let rest = i / s[0] as usize;
            let z = (rest % s[2] as usize) as i32;
            let y = (rest / s[2] as usize) as i32;
            Some(([self.min[0] + x, self.min[1] + y, self.min[2] + z], v))
        })
    }

    /// `getCurrentTarget`.
    pub fn target(&self) -> [i32; 3] {
        [0, 1, 2].map(|i| self.anchor[i] + self.current[i])
    }

    /// `tryAdvanceCurrentPos`: one step east, wrapping to the next row and layer as Create's
    /// arithmetic does (rows and layers past the bounds are walked too); true when the block
    /// there is one to defer.
    fn try_advance(&mut self, defer: &impl Fn(u32) -> bool) -> bool {
        self.current[0] += 1;
        let span = self.span();
        let in_bounds = [0, 1, 2].map(|i| self.current[i] - self.min[i]);
        if in_bounds[0] > span[0] {
            self.current = [self.min[0] - 1, self.current[1], self.current[2] + 1];
        }
        if in_bounds[2] > span[2] {
            self.current = [self.current[0] - 1, self.current[1] + 1, self.min[2]];
        }
        if self.current[1] > span[1] {
            self.stage = Stage::DeferredBlocks;
            return false;
        }
        self.at(self.current).is_some_and(defer)
    }

    /// `advanceCurrentPos`: the next position to print, deferred blocks after the rest; false
    /// once everything was gone through (entities are not printed here).
    pub fn advance(&mut self, defer: impl Fn(u32) -> bool) -> bool {
        loop {
            if self.stage == Stage::Blocks {
                while self.try_advance(&defer) {
                    self.deferred.push_back(self.current);
                }
            }
            if self.stage == Stage::DeferredBlocks {
                match self.deferred.pop_front() {
                    None => self.stage = Stage::Entities,
                    Some(p) => self.current = p,
                }
            }
            if self.stage == Stage::Entities {
                return false;
            }
            if self.inside(self.current) {
                return true;
            }
        }
    }
}

/// A block in the world as the printer reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spot {
    pub state: u32,
    /// `getDestroySpeed == -1`.
    pub unbreakable: bool,
    pub conductor: bool,
    pub has_block_entity: bool,
}

/// `SchematicannonBlockEntity.shouldPlace` with `SchematicPrinter.shouldPlaceBlock`'s checks
/// before it (the world's state there, the other half's, both breakable): whether the printer
/// shoots `entry` at `pos`.
pub fn should_place(
    cannon: [i32; 3],
    options: &Options,
    pos: [i32; 3],
    entry: &Entry,
    here: Spot,
    other: Option<Spot>,
) -> bool {
    if here.state == entry.state {
        return false;
    }
    if here.unbreakable || other.is_some_and(|o| o.unbreakable) {
        return false;
    }
    let d = [0, 1, 2].map(|i| f64::from(pos[i] - cannon[i]));
    if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < 4.0 {
        return false;
    }
    if !options.replace_block_entities
        && (here.has_block_entity || other.is_some_and(|o| o.has_block_entity))
    {
        return false;
    }
    if entry.ignore {
        return false;
    }
    let open = !here.conductor && other.is_none_or(|o| !o.conductor);
    match options.replace_mode {
        REPLACE_EMPTY => true,
        REPLACE_ANY => !entry.air,
        REPLACE_SOLID => (entry.conductor || open) && !entry.air,
        DONT_REPLACE => open && !entry.air,
        _ => false,
    }
}

/// `LaunchedItem.ticksForDistance`.
pub fn flight_ticks(from: [i32; 3], to: [i32; 3]) -> u32 {
    let d = [0, 1, 2].map(|i| f64::from(to[i] - from[i]));
    let dist2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    (dist2.sqrt().sqrt() * 4.0).max(10.0) as u32
}

/// A block on its way (`LaunchedItem.ForBlockState`): it lands at `land_at`, after
/// `ticksRemaining` counts down from `total`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flying {
    pub target: [i32; 3],
    pub state: u32,
    pub total: u32,
    pub land_at: u64,
}

/// What the cannon's slot 0 holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Blueprint {
    Empty,
    /// A stack: a schematic's data, or none for anything else.
    Item(Option<SchematicData>),
}

/// A loaded schematic: where it prints and what each of its palette entries is.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub printer: Printer,
    pub entries: Vec<Entry>,
    /// What the level reads where nothing was placed (`Blocks.AIR`).
    pub air: Entry,
}

/// What a cannon reaches of the game.
pub trait CannonWorld {
    fn blueprint(&mut self) -> Blueprint;
    /// Slot 1 holds nothing.
    fn output_empty(&mut self) -> bool;
    /// `finishedPrinting`: the schematic out of slot 0, one more empty schematic in slot 1.
    fn finish_blueprint(&mut self);
    /// An expired or broken schematic: slot 0 emptied, slot 1 a single empty schematic.
    fn expire_blueprint(&mut self);
    /// One gunpowder from slot 4.
    fn own_gunpowder(&mut self) -> bool;
    /// One gunpowder from an inventory beside the cannon.
    fn external_gunpowder(&mut self) -> bool;
    /// A creative crate beside the cannon.
    fn creative_crate(&mut self) -> bool;
    /// `SchematicItem.loadSchematic` and the template placed (an unreadable file is an empty
    /// schematic).
    fn load(&mut self, data: &SchematicData) -> Loaded;
    fn loaded(&mut self, pos: [i32; 3]) -> bool;
    fn spot(&mut self, pos: [i32; 3]) -> Spot;
    /// `grabItemsFromAttachedInventories` of a consumed item.
    fn grab(&mut self, item: &str, count: u32, simulate: bool) -> bool;
    /// `LaunchedItem.place`.
    fn place(&mut self, pos: [i32; 3], state: u32);
    /// A block launched (`playFiringSound`).
    fn fired(&mut self, now: u64) {
        let _ = now;
    }
    /// Printing finished (`SCHEMATICANNON_FINISH`).
    fn dinged(&mut self) {}
}

/// A schematicannon's block entity, typed.
#[derive(Clone, Debug, Default)]
pub struct Cannon {
    pub pos: [i32; 3],
    pub state: State,
    pub status: Status,
    pub fuel: i32,
    pub options: Options,
    pub blocks_placed: i32,
    pub blocks_to_place: i32,
    pub missing: Option<String>,
    pub position_not_loaded: bool,
    pub creative: bool,
    /// `printerCooldown`.
    pub cooldown: u32,
    pub loaded: Option<Loaded>,
    pub flying: Vec<Flying>,
    /// `defaultYaw`: where the barrel rests.
    pub default_yaw: f32,
    /// The game time of the last tick this cannon was ticked to.
    pub last: u64,
    /// At the last tick: whether every tick it sleeps through counts its cooldown down.
    counting: bool,
    /// The last attempt to load what slot 0 holds failed (retried when its slots change).
    pub init_failed: bool,
    has_blueprint: bool,
}

impl Cannon {
    pub fn new(pos: [i32; 3], options: Options) -> Cannon {
        Cannon {
            pos,
            options,
            ..Default::default()
        }
    }

    /// `printer.getCurrentTarget()`: where the barrel points, while printing.
    pub fn target(&self) -> Option<[i32; 3]> {
        self.loaded.as_ref().map(|l| l.printer.target())
    }

    /// The ticks from the last one ticked up to `now` (not included) are counted: what Create
    /// did in them, the cooldown counting down, is done. Before anything changes the cannon from
    /// outside (its slots, a command).
    pub fn settle(&mut self, now: u64) {
        if now > self.last + 1 {
            if self.counting {
                let slept = now - self.last - 1;
                self.cooldown = self
                    .cooldown
                    .saturating_sub(slept.min(u64::from(u32::MAX)) as u32);
            }
            self.last = now - 1;
        }
    }

    /// `SchematicannonBlockEntity.tick` at game time `now`, after the ticks slept through.
    pub fn tick(&mut self, now: u64, w: &mut impl CannonWorld) {
        self.settle(now);
        self.last = now;
        self.creative = w.creative_crate();
        // tickFlyingBlocks.
        let mut i = 0;
        while i < self.flying.len() {
            if self.flying[i].land_at <= now {
                let f = self.flying.remove(i);
                w.place(f.target, f.state);
            } else {
                i += 1;
            }
        }
        // tickPaperPrinter: a schematic in slot 0 is loaded (its checklist book isn't printed).
        let blueprint = w.blueprint();
        self.has_blueprint = blueprint != Blueprint::Empty;
        if self.loaded.is_none() && self.has_blueprint {
            self.initialize(&blueprint, w);
        }
        self.refill(w);
        let mut skips = SKIPS_PER_TICK;
        let mut skipped = true;
        while skipped && skips > 0 {
            skips -= 1;
            skipped = self.tick_printer(now, w);
        }
        self.counting = self.counts();
    }

    /// Whether `tickPrinter` gets as far as its cooldown each tick.
    fn counts(&self) -> bool {
        self.loaded.is_some()
            && self.state != State::Stopped
            && !(self.state == State::Paused
                && !self.position_not_loaded
                && self.missing.is_none()
                && self.fuel > 0)
    }

    /// The next game time this cannon needs a tick, if any: a landing; while it prints, the tick
    /// its cooldown ends, the next one to refill an empty barrel, the next one after 1000 skips;
    /// while it waits for items, fuel or a loaded target, its next shot's tick; and the tick
    /// after a stop, when Create loads its schematic again.
    pub fn next_wake(&self, now: u64) -> Option<u64> {
        let mut wake = self.flying.iter().map(|f| f.land_at).min();
        let mut at = |t: u64| wake = Some(wake.map_or(t, |w| w.min(t)));
        if self.counting {
            if self.cooldown > 0 {
                at(now + u64::from(self.cooldown) + 1);
                if self.fuel <= 0 && !self.creative {
                    at(now + 1);
                }
            } else if matches!(self.status, Status::Searching | Status::Skipping) {
                at(now + 1);
            } else {
                at(now + u64::from(DELAY) + 1);
            }
        }
        if self.loaded.is_none() && self.has_blueprint && !self.init_failed {
            at(now + 1);
        }
        wake
    }

    /// `refillFuelIfPossible`.
    fn refill(&mut self, w: &mut impl CannonWorld) {
        if self.creative {
            return;
        }
        if self.fuel > SHOTS_PER_GUNPOWDER {
            self.fuel = SHOTS_PER_GUNPOWDER;
            return;
        }
        if self.fuel > 0 {
            return;
        }
        if !w.own_gunpowder() && !w.external_gunpowder() {
            return;
        }
        self.fuel += SHOTS_PER_GUNPOWDER;
        if self.status == Status::NoGunpowder {
            if self.blocks_placed > 0 {
                self.state = State::Running;
            }
            self.status = Status::Ready;
        }
    }

    /// `resetPrinter`.
    fn reset(&mut self) {
        self.loaded = None;
        self.missing = None;
        self.blocks_placed = 0;
        self.blocks_to_place = 0;
    }

    /// `initializePrinter`.
    fn initialize(&mut self, blueprint: &Blueprint, w: &mut impl CannonWorld) {
        let data = match blueprint {
            Blueprint::Item(Some(d)) if d.anchor.is_some() => d,
            _ => {
                self.state = State::Stopped;
                self.status = Status::SchematicInvalid;
                self.init_failed = true;
                return;
            }
        };
        if data.deployed != Some(true) {
            self.state = State::Stopped;
            self.status = Status::SchematicNotPlaced;
            self.init_failed = true;
            return;
        }
        let loaded = w.load(data);
        if loaded.printer.is_empty() {
            self.state = State::Stopped;
            self.status = Status::SchematicExpired;
            w.expire_blueprint();
            self.loaded = None;
            self.init_failed = true;
            return;
        }
        let a = loaded.printer.anchor;
        let d = [0, 1, 2].map(|i| f64::from(a[i] - self.pos[i]));
        if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] >= MAX_ANCHOR_DISTANCE * MAX_ANCHOR_DISTANCE {
            self.state = State::Stopped;
            self.status = Status::TargetOutsideRange;
            self.loaded = None;
            self.init_failed = true;
            return;
        }
        self.init_failed = false;
        self.loaded = Some(loaded);
        self.state = State::Paused;
        self.status = Status::Ready;
        // updateChecklist, then `blocksToPlace += blocksPlaced`.
        self.blocks_to_place = self.blocks_placed + self.count_to_place(w);
        self.blocks_to_place += self.blocks_placed;
    }

    /// `markAllBlockRequirements`: the blocks the printer would place now and that cost
    /// something.
    fn count_to_place(&self, w: &mut impl CannonWorld) -> i32 {
        let Some(l) = &self.loaded else { return 0 };
        let mut n = 0;
        for (rel, i) in l.printer.positions() {
            let pos = [0, 1, 2].map(|k| l.printer.anchor[k] + rel[k]);
            if !w.loaded(pos) {
                continue;
            }
            let e = &l.entries[i as usize];
            if !self.should_place_at(w, pos, e) {
                continue;
            }
            if matches!(e.requirement, Requirement::Items(_)) {
                n += 1;
            }
        }
        n
    }

    fn should_place_at(&self, w: &mut impl CannonWorld, pos: [i32; 3], e: &Entry) -> bool {
        if !w.loaded(pos) {
            return false;
        }
        let here = w.spot(pos);
        let other = e
            .other
            .map(|o| w.spot([pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]));
        should_place(self.pos, &self.options, pos, e, here, other)
    }

    /// `tickPrinter`: true when the target was skipped (the tick goes on to the next one).
    fn tick_printer(&mut self, now: u64, w: &mut impl CannonWorld) -> bool {
        if !self.has_blueprint && self.status != Status::Idle && w.output_empty() {
            self.state = State::Stopped;
            self.status = Status::Idle;
            return false;
        }
        if self.state == State::Stopped {
            if self.loaded.is_some() {
                self.reset();
            }
            return false;
        }
        if self.state == State::Paused
            && !self.position_not_loaded
            && self.missing.is_none()
            && self.fuel > 0
        {
            return false;
        }
        if self.loaded.is_none() {
            let blueprint = w.blueprint();
            self.initialize(&blueprint, w);
            return false;
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return false;
        }
        if self.fuel <= 0 && !self.creative {
            self.refill(w);
            if self.fuel <= 0 {
                self.state = State::Paused;
                self.status = Status::NoGunpowder;
                return false;
            }
        }
        if self.creative {
            self.fuel = 0;
            if self.missing.is_some() {
                self.missing = None;
                self.state = State::Running;
            }
        }
        let Some(mut loaded) = self.loaded.take() else {
            return false;
        };
        let skipped = self.print(now, w, &mut loaded);
        // Only `finishedPrinting` stops it here, and it resets the printer.
        if self.state != State::Stopped {
            self.loaded = Some(loaded);
        }
        skipped
    }

    /// `tickPrinter` from "Update Target" on.
    fn print(&mut self, now: u64, w: &mut impl CannonWorld, l: &mut Loaded) -> bool {
        if self.missing.is_none() && !self.position_not_loaded {
            let entries = &l.entries;
            if !l.printer.advance(|i| entries[i as usize].defer) {
                self.finished(w);
                return false;
            }
        }
        let target = l.printer.target();
        if !w.loaded(target) {
            self.position_not_loaded = true;
            self.status = Status::TargetNotLoaded;
            self.state = State::Paused;
            return false;
        } else if self.position_not_loaded {
            self.position_not_loaded = false;
            self.state = State::Running;
        }
        let rel = [0, 1, 2].map(|i| target[i] - l.printer.anchor[i]);
        let entry = match l.printer.at(rel) {
            Some(i) => &l.entries[i as usize],
            None => &l.air,
        };
        if entry.requirement == Requirement::Invalid || !self.should_place_at(w, target, entry) {
            self.status = Status::Searching;
            return true;
        }
        if let Requirement::Items(items) = &entry.requirement {
            for (item, count) in items {
                if !w.grab(item, *count, true) {
                    if self.options.skip_missing {
                        self.status = Status::Skipping;
                        if self.missing.is_some() {
                            self.missing = None;
                            self.state = State::Running;
                        }
                        return true;
                    }
                    self.missing = Some(item.clone());
                    self.state = State::Paused;
                    self.status = Status::MissingBlock;
                    return false;
                }
            }
            for (item, count) in items {
                w.grab(item, *count, false);
            }
        }
        self.state = State::Running;
        self.status = if entry.plain_air {
            Status::Clearing
        } else {
            Status::Placing
        };
        if !entry.air {
            self.blocks_placed += 1;
        }
        let total = flight_ticks(self.pos, target);
        self.flying.push(Flying {
            target,
            state: entry.placed,
            total,
            land_at: now + u64::from(total) + 1,
        });
        w.fired(now);
        self.cooldown = DELAY;
        self.fuel -= 1;
        self.missing = None;
        false
    }

    /// `finishedPrinting` (the water ticks Create schedules round a cleared print are not).
    fn finished(&mut self, w: &mut impl CannonWorld) {
        w.finish_blueprint();
        self.has_blueprint = false;
        self.state = State::Stopped;
        self.status = Status::Finished;
        self.reset();
        w.dinged();
    }

    /// `ConfigureSchematicannonPacket.handle`, at game time `now`.
    pub fn configure(&mut self, now: u64, option: Configure) {
        self.settle(now);
        match option {
            Configure::Mode(m) => self.options.replace_mode = m.min(REPLACE_EMPTY),
            Configure::SkipMissing(v) => self.options.skip_missing = v,
            Configure::ReplaceBlockEntities(v) => self.options.replace_block_entities = v,
            Configure::Play => {
                self.state = State::Running;
                self.status = Status::Running;
            }
            Configure::Pause => {
                self.state = State::Paused;
                self.status = Status::Paused;
            }
            Configure::Stop => {
                self.state = State::Stopped;
                self.status = Status::Stopped;
            }
        }
        self.counting = self.counts();
    }

    /// Its slots changed (a player, a hopper): what slot 0 holds is looked at again.
    pub fn slots_changed(&mut self, now: u64) {
        self.settle(now);
        self.init_failed = false;
    }
}

/// `ConfigureSchematicannonPacket.Option`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Configure {
    Mode(u8),
    SkipMissing(bool),
    ReplaceBlockEntities(bool),
    Play,
    Pause,
    Stop,
}

impl Configure {
    /// `play`, `pause`, `stop`, `mode <0..3|dont_replace|replace_solid|replace_any|
    /// replace_empty>`, `skip_missing <bool>`, `replace_block_entities <bool>`.
    pub fn parse(words: &[&str]) -> Option<Configure> {
        let flag = |w: Option<&&str>| match w.copied() {
            Some("true") => Some(true),
            Some("false") => Some(false),
            _ => None,
        };
        Some(match words {
            ["play"] => Configure::Play,
            ["pause"] => Configure::Pause,
            ["stop"] => Configure::Stop,
            ["mode", m] => Configure::Mode(match *m {
                "0" | "dont_replace" => DONT_REPLACE,
                "1" | "replace_solid" => REPLACE_SOLID,
                "2" | "replace_any" => REPLACE_ANY,
                "3" | "replace_empty" => REPLACE_EMPTY,
                _ => return None,
            }),
            ["skip_missing", ..] => Configure::SkipMissing(flag(words.get(1))?),
            ["replace_block_entities", ..] => Configure::ReplaceBlockEntities(flag(words.get(1))?),
            _ => return None,
        })
    }
}

/// The barrel's aim, as `SchematicannonRenderer.getCannonAngles` works it out (in degrees): the
/// yaw toward the target and the pitch over the distance; at rest, its default yaw and 40.
pub fn aim(pos: [i32; 3], target: Option<[i32; 3]>, default_yaw: f32) -> (f32, f32) {
    let Some(t) = target else {
        return (default_yaw, 40.0);
    };
    let d = [0, 1, 2].map(|i| f64::from(t[i] - pos[i]));
    let yaw = d[0].atan2(d[2]).to_degrees();
    let distance = f64::from(((d[0] * d[0] + d[2] * d[2]) as f32).sqrt());
    let y_offset = distance * 2.0;
    let pitch = distance.atan2(d[1] * 3.0 + y_offset).to_degrees() + 10.0;
    (yaw as f32, pitch as f32)
}

/// `SchematicannonBlock.setPlacedBy`: the placer's yaw (turned round when sneaking) to the
/// nearest sixteenth.
pub fn placed_yaw(yaw: f32, sneaking: bool) -> f32 {
    let y = yaw + if sneaking { 180.0 } else { 0.0 };
    ((-(y * 16.0 / 360.0 + 0.5).floor() as i32) & 15) as f32 * 360.0 / 16.0
}

/// Mod messages to clients: where each cannon's barrel points.
pub const CHANNEL: &str = "create:schematicannon";

/// A cannon's barrel for clients: its position, target (none at rest) and resting yaw; or gone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Aim {
    Set {
        pos: [i32; 3],
        target: Option<[i32; 3]>,
        default_yaw: f32,
    },
    Gone {
        pos: [i32; 3],
    },
}

pub fn encode(aims: &[Aim]) -> Vec<u8> {
    let mut out = Vec::with_capacity(aims.len() * 33);
    for a in aims {
        let put = |out: &mut Vec<u8>, p: [i32; 3]| {
            for v in p {
                out.extend_from_slice(&v.to_le_bytes());
            }
        };
        match a {
            Aim::Set {
                pos,
                target,
                default_yaw,
            } => {
                out.push(if target.is_some() { 1 } else { 0 });
                put(&mut out, *pos);
                put(&mut out, target.unwrap_or_default());
                out.extend_from_slice(&default_yaw.to_le_bytes());
            }
            Aim::Gone { pos } => {
                out.push(2);
                put(&mut out, *pos);
            }
        }
    }
    out
}

pub fn decode(mut d: &[u8]) -> Vec<Aim> {
    let mut out = Vec::new();
    let int = |d: &mut &[u8]| -> Option<i32> {
        let (v, rest) = d.split_first_chunk::<4>()?;
        *d = rest;
        Some(i32::from_le_bytes(*v))
    };
    while let Some((&kind, rest)) = d.split_first() {
        d = rest;
        let Some(pos) = (|| Some([int(&mut d)?, int(&mut d)?, int(&mut d)?]))() else {
            break;
        };
        if kind == 2 {
            out.push(Aim::Gone { pos });
            continue;
        }
        let Some(target) = (|| Some([int(&mut d)?, int(&mut d)?, int(&mut d)?]))() else {
            break;
        };
        let Some(yaw) = int(&mut d) else { break };
        out.push(Aim::Set {
            pos,
            target: (kind == 1).then_some(target),
            default_yaw: f32::from_bits(yaw as u32),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schematic::{BlockInfo, PaletteState};
    use std::collections::HashMap;

    fn structure(size: [i32; 3], blocks: &[([i32; 3], usize)], palette: usize) -> Structure {
        Structure {
            data_version: None,
            size,
            palette: (0..palette).map(|_| PaletteState::air()).collect(),
            blocks: blocks
                .iter()
                .map(|&(pos, state)| BlockInfo {
                    pos,
                    state,
                    nbt: None,
                })
                .collect(),
            entities: Vec::new(),
        }
    }

    fn entry(state: u32, defer: bool) -> Entry {
        Entry {
            state,
            placed: state,
            air: false,
            plain_air: false,
            conductor: true,
            requirement: Requirement::Items(vec![(format!("item{state}"), 1)]),
            ignore: false,
            defer,
            other: None,
        }
    }

    fn air() -> Entry {
        Entry {
            state: 0,
            placed: 0,
            air: true,
            plain_air: true,
            conductor: false,
            requirement: Requirement::Empty,
            ignore: false,
            defer: false,
            other: None,
        }
    }

    #[test]
    fn the_printer_goes_row_by_row_and_defers_brittle_blocks() {
        // 2 x 2 x 2; a torch (deferred) at (0, 0, 1).
        let s = structure(
            [2, 2, 2],
            &[
                ([0, 0, 0], 0),
                ([1, 0, 0], 0),
                ([0, 0, 1], 1),
                ([1, 0, 1], 0),
                ([0, 1, 0], 0),
            ],
            2,
        );
        let mut p = Printer::load(&s, [100, 64, 100], Rotation::None, Mirror::None);
        assert_eq!((p.min, p.max, p.current), ([0; 3], [1, 1, 1], [-1, 0, 0]));
        let defer = |i: u32| i == 1;
        let mut order = Vec::new();
        while p.advance(defer) {
            order.push(p.target());
        }
        // Every position in the bounds, x fastest, then z, then y; the torch last.
        assert_eq!(
            order,
            vec![
                [100, 64, 100],
                [101, 64, 100],
                [101, 64, 101],
                [100, 65, 100],
                [101, 65, 100],
                [100, 65, 101],
                [101, 65, 101],
                [100, 64, 101],
            ]
        );
        assert_eq!(p.stage, Stage::Entities);
    }

    #[test]
    fn a_rotated_schematic_is_placed_round_its_anchor() {
        let s = structure([3, 1, 1], &[([0, 0, 0], 0), ([2, 0, 0], 0)], 1);
        let p = Printer::load(&s, [0, 0, 0], Rotation::Clockwise90, Mirror::None);
        // (2, 0, 0) turns to (0, 0, 2): the bounds run from the origin to there.
        assert_eq!((p.min, p.max), ([0, 0, 0], [0, 0, 2]));
        assert_eq!(p.at([0, 0, 2]), Some(0));
        assert_eq!(p.at([0, 0, 1]), None);
        let m = Printer::load(&s, [0, 0, 0], Rotation::None, Mirror::FrontBack);
        assert_eq!((m.min, m.max), ([-2, 0, 0], [0, 0, 0]));
        assert_eq!(m.positions().count(), 2);
    }

    #[test]
    fn replace_modes_follow_create() {
        let cannon = [0, 0, 0];
        let far = [10, 0, 0];
        let stone = entry(5, false);
        let air_entry = air();
        let solid = Spot {
            state: 7,
            conductor: true,
            ..Default::default()
        };
        let empty = Spot::default();
        let mode = |m: u8| Options {
            replace_mode: m,
            ..Default::default()
        };
        // Don't replace: only where nothing solid stands.
        assert!(!should_place(
            cannon,
            &mode(DONT_REPLACE),
            far,
            &stone,
            solid,
            None
        ));
        assert!(should_place(
            cannon,
            &mode(DONT_REPLACE),
            far,
            &stone,
            Spot { state: 3, ..empty },
            None
        ));
        // Replace solid: a solid block replaces anything; a non-solid one only non-solids.
        assert!(should_place(
            cannon,
            &mode(REPLACE_SOLID),
            far,
            &stone,
            solid,
            None
        ));
        let glass = Entry {
            conductor: false,
            ..entry(6, false)
        };
        assert!(!should_place(
            cannon,
            &mode(REPLACE_SOLID),
            far,
            &glass,
            solid,
            None
        ));
        // Replace any: everything but air; replace with empty clears too.
        assert!(should_place(
            cannon,
            &mode(REPLACE_ANY),
            far,
            &stone,
            solid,
            None
        ));
        assert!(!should_place(
            cannon,
            &mode(REPLACE_ANY),
            far,
            &air_entry,
            solid,
            None
        ));
        assert!(should_place(
            cannon,
            &mode(REPLACE_EMPTY),
            far,
            &air_entry,
            solid,
            None
        ));
        // Never what is already there, unbreakable blocks, block entities (unless asked), the
        // cannon's own neighbours, ignored halves.
        assert!(!should_place(
            cannon,
            &mode(REPLACE_EMPTY),
            far,
            &stone,
            Spot { state: 5, ..empty },
            None
        ));
        assert!(!should_place(
            cannon,
            &mode(REPLACE_EMPTY),
            far,
            &stone,
            Spot {
                unbreakable: true,
                ..solid
            },
            None
        ));
        let chest = Spot {
            has_block_entity: true,
            ..solid
        };
        assert!(!should_place(
            cannon,
            &mode(REPLACE_EMPTY),
            far,
            &stone,
            chest,
            None
        ));
        let be = Options {
            replace_block_entities: true,
            ..mode(REPLACE_EMPTY)
        };
        assert!(should_place(cannon, &be, far, &stone, chest, None));
        assert!(!should_place(
            cannon,
            &mode(REPLACE_EMPTY),
            [1, 1, 0],
            &stone,
            empty,
            None
        ));
        assert!(should_place(
            cannon,
            &mode(REPLACE_EMPTY),
            [2, 0, 0],
            &stone,
            empty,
            None
        ));
        let upper = Entry {
            ignore: true,
            ..entry(5, false)
        };
        assert!(!should_place(
            cannon,
            &mode(REPLACE_EMPTY),
            far,
            &upper,
            empty,
            None
        ));
    }

    /// A small world: blocks by position, items in one inventory, gunpowder in slot 4.
    struct Fake {
        blueprint: Blueprint,
        output: u32,
        gunpowder: u32,
        external: u32,
        items: HashMap<String, u32>,
        blocks: HashMap<[i32; 3], u32>,
        structure: Structure,
        entries: Vec<Entry>,
        loads: u32,
        fired: Vec<u64>,
        now: u64,
    }

    impl CannonWorld for Fake {
        fn blueprint(&mut self) -> Blueprint {
            self.blueprint.clone()
        }
        fn output_empty(&mut self) -> bool {
            self.output == 0
        }
        fn finish_blueprint(&mut self) {
            self.blueprint = Blueprint::Empty;
            self.output += 1;
        }
        fn expire_blueprint(&mut self) {
            self.blueprint = Blueprint::Empty;
            self.output = 1;
        }
        fn own_gunpowder(&mut self) -> bool {
            let had = self.gunpowder > 0;
            self.gunpowder = self.gunpowder.saturating_sub(1);
            had
        }
        fn external_gunpowder(&mut self) -> bool {
            let had = self.external > 0;
            self.external = self.external.saturating_sub(1);
            had
        }
        fn creative_crate(&mut self) -> bool {
            false
        }
        fn load(&mut self, d: &SchematicData) -> Loaded {
            self.loads += 1;
            Loaded {
                printer: Printer::load(&self.structure, d.anchor.unwrap(), d.rotation, d.mirror),
                entries: self.entries.clone(),
                air: air(),
            }
        }
        fn loaded(&mut self, _: [i32; 3]) -> bool {
            true
        }
        fn spot(&mut self, pos: [i32; 3]) -> Spot {
            Spot {
                state: self.blocks.get(&pos).copied().unwrap_or(0),
                ..Default::default()
            }
        }
        fn grab(&mut self, item: &str, count: u32, simulate: bool) -> bool {
            let have = self.items.get(item).copied().unwrap_or(0);
            if have < count {
                return false;
            }
            if !simulate {
                self.items.insert(item.to_owned(), have - count);
            }
            true
        }
        fn place(&mut self, pos: [i32; 3], state: u32) {
            self.blocks.insert(pos, state);
        }
        fn fired(&mut self, now: u64) {
            self.fired.push(now);
        }
    }

    fn deployed() -> Blueprint {
        Blueprint::Item(Some(SchematicData {
            deployed: Some(true),
            anchor: Some([10, 0, 0]),
            ..SchematicData::written("a.nbt", "Alex", [3, 1, 1])
        }))
    }

    fn fake(blocks: usize) -> Fake {
        let s = structure(
            [blocks as i32, 1, 1],
            &(0..blocks)
                .map(|x| ([x as i32, 0, 0], 0))
                .collect::<Vec<_>>(),
            1,
        );
        Fake {
            blueprint: deployed(),
            output: 0,
            gunpowder: 1,
            external: 0,
            items: HashMap::from([("item5".to_owned(), 64)]),
            blocks: HashMap::new(),
            structure: s,
            entries: vec![entry(5, false)],
            loads: 0,
            fired: Vec::new(),
            now: 0,
        }
    }

    /// Ticks the cannon only when it asks to be, from `from` to `to`.
    fn run(c: &mut Cannon, f: &mut Fake, from: u64, to: u64) -> u32 {
        let mut ticks = 0;
        let mut next = Some(from);
        while let Some(t) = next {
            if t > to {
                break;
            }
            f.now = t;
            c.tick(t, f);
            ticks += 1;
            next = c.next_wake(t);
        }
        ticks
    }

    #[test]
    fn a_cannon_prints_one_block_per_delay_and_counts_its_gunpowder() {
        let mut f = fake(3);
        let mut c = Cannon::new([0, 0, 0], Options::default());
        // The schematic goes in: loaded, paused and ready, the gunpowder burnt into fuel.
        c.slots_changed(1);
        c.tick(1, &mut f);
        assert_eq!(
            (c.state, c.status, c.blocks_to_place),
            (State::Paused, Status::Ready, 3)
        );
        assert_eq!((c.fuel, f.gunpowder), (SHOTS_PER_GUNPOWDER, 0));
        assert_eq!(c.next_wake(1), None, "a ready cannon sleeps");
        // Play: a shot every delay + 1 ticks (the cooldown counts 10 ticks down first).
        c.configure(5, Configure::Play);
        let ticks = run(&mut c, &mut f, 5, 100);
        assert_eq!(f.fired, vec![5, 16, 27]);
        assert_eq!((c.state, c.status), (State::Stopped, Status::Finished));
        assert_eq!(c.fuel, SHOTS_PER_GUNPOWDER - 3);
        assert_eq!(f.items["item5"], 61);
        assert_eq!((f.blueprint.clone(), f.output), (Blueprint::Empty, 1));
        // Each block lands max(10, 4 * sqrt(distance)) + 1 ticks after it was fired.
        assert_eq!(f.blocks.len(), 3);
        assert_eq!(flight_ticks([0, 0, 0], [10, 0, 0]), 12);
        // Woken only to shoot, to land a block and to finish: far fewer than the 96 ticks.
        assert!(ticks <= 10, "{ticks} ticks");
        assert_eq!(c.next_wake(100), None, "idle again");
    }

    #[test]
    fn placed_blocks_and_missing_items_skip_or_wait() {
        // The first block is already there: skipped in the same tick, the second fired.
        let mut f = fake(3);
        f.blocks.insert([10, 0, 0], 5);
        let mut c = Cannon::new([0, 0, 0], Options::default());
        c.tick(1, &mut f);
        assert_eq!(c.blocks_to_place, 2);
        c.configure(2, Configure::Play);
        c.tick(2, &mut f);
        assert_eq!(f.fired, vec![2]);
        assert_eq!(c.target(), Some([11, 0, 0]));
        // Out of items: without skipping, it waits on the block (paused, missing it).
        f.items.insert("item5".into(), 0);
        run(&mut c, &mut f, 13, 40);
        assert_eq!((c.state, c.status), (State::Paused, Status::MissingBlock));
        assert_eq!(c.missing.as_deref(), Some("item5"));
        assert_eq!(c.target(), Some([12, 0, 0]));
        assert_eq!(c.next_wake(40), Some(40 + u64::from(DELAY) + 1));
        // Items again: printed on the next try.
        f.items.insert("item5".into(), 1);
        run(&mut c, &mut f, 41, 80);
        assert_eq!(c.status, Status::Finished);
        assert_eq!(f.blocks.get(&[12, 0, 0]), Some(&5));
        // Skipping missing blocks: they are passed by.
        let mut f = fake(3);
        f.items.clear();
        let mut c = Cannon::new([0, 0, 0], Options::placed());
        c.tick(1, &mut f);
        c.configure(1, Configure::Play);
        run(&mut c, &mut f, 2, 10);
        assert_eq!(c.status, Status::Finished);
        assert!(f.fired.is_empty() && f.blocks.is_empty());
    }

    #[test]
    fn gunpowder_runs_out_and_comes_back() {
        let mut f = fake(2);
        f.gunpowder = 0;
        let mut c = Cannon::new([0, 0, 0], Options::default());
        c.tick(1, &mut f);
        // Loaded, but nothing to burn: no gunpowder.
        assert_eq!(
            (c.state, c.status, c.fuel),
            (State::Paused, Status::NoGunpowder, 0)
        );
        // One from an inventory beside it: ready again (not running, nothing was placed yet).
        f.external = 1;
        run(&mut c, &mut f, 2, 30);
        assert_eq!(
            (c.status, c.fuel, f.external),
            (Status::Ready, SHOTS_PER_GUNPOWDER, 0)
        );
        // The last shot of a gunpowder: the next is burnt the tick after.
        c.fuel = 1;
        f.gunpowder = 1;
        c.configure(31, Configure::Play);
        c.tick(31, &mut f);
        assert_eq!((c.fuel, f.gunpowder), (0, 1));
        assert_eq!(c.next_wake(31), Some(32));
        c.tick(32, &mut f);
        assert_eq!((c.fuel, f.gunpowder), (SHOTS_PER_GUNPOWDER, 0));
        // Fuel above a gunpowder's worth is cut back (`refillFuelIfPossible`).
        c.fuel = SHOTS_PER_GUNPOWDER + 50;
        c.tick(33, &mut f);
        assert_eq!(c.fuel, SHOTS_PER_GUNPOWDER);
    }

    #[test]
    fn a_paused_cannon_keeps_its_cooldown_and_a_stop_starts_over() {
        let mut f = fake(3);
        let mut c = Cannon::new([0, 0, 0], Options::default());
        c.tick(1, &mut f);
        c.configure(1, Configure::Play);
        c.tick(2, &mut f);
        assert_eq!(f.fired, vec![2]);
        // Paused 3 ticks later: 7 of its 10 cooldown ticks are left when it plays again.
        c.tick(3, &mut f);
        c.tick(4, &mut f);
        c.tick(5, &mut f);
        c.configure(5, Configure::Pause);
        assert_eq!(c.cooldown, 7);
        assert_eq!(
            c.next_wake(5),
            Some(15),
            "only its block in flight wakes it"
        );
        c.configure(50, Configure::Play);
        let mut next = Some(50);
        while let Some(t) = next {
            if f.fired.len() == 2 || t > 100 {
                break;
            }
            f.now = t;
            c.tick(t, &mut f);
            next = c.next_wake(t);
        }
        assert_eq!(f.fired, vec![2, 57]);
        // Stopped: the printer resets, then loads the schematic again (paused, ready).
        c.configure(60, Configure::Stop);
        c.tick(60, &mut f);
        assert!(c.loaded.is_none() && c.blocks_placed == 0);
        assert_eq!(c.next_wake(60), Some(61));
        c.tick(61, &mut f);
        assert_eq!((c.state, c.status), (State::Paused, Status::Ready));
        assert_eq!(f.loads, 2);
    }

    #[test]
    fn invalid_schematics_stop_the_cannon() {
        let mut f = fake(1);
        f.blueprint = Blueprint::Item(None);
        let mut c = Cannon::new([0, 0, 0], Options::default());
        c.tick(1, &mut f);
        assert_eq!(
            (c.state, c.status),
            (State::Stopped, Status::SchematicInvalid)
        );
        assert_eq!(c.next_wake(1), None);
        f.blueprint = Blueprint::Item(Some(SchematicData::written("a.nbt", "Alex", [1, 1, 1])));
        c.slots_changed(2);
        c.tick(2, &mut f);
        assert_eq!(c.status, Status::SchematicNotPlaced);
        // An empty schematic expires: slot 0 is emptied for a blank one.
        f.structure = structure([0, 0, 0], &[], 0);
        f.blueprint = deployed();
        c.slots_changed(3);
        c.tick(3, &mut f);
        assert_eq!((c.status, f.output), (Status::SchematicExpired, 1));
        // Too far away.
        let mut f = fake(1);
        let mut c = Cannon::new([300, 0, 0], Options::default());
        c.tick(1, &mut f);
        assert_eq!(c.status, Status::TargetOutsideRange);
        // Taken out: idle.
        f.blueprint = Blueprint::Empty;
        c.slots_changed(2);
        c.tick(2, &mut f);
        assert_eq!((c.state, c.status), (State::Stopped, Status::Idle));
    }

    #[test]
    fn the_barrel_aims_and_the_wire_round_trips() {
        assert_eq!(aim([0, 0, 0], None, 90.0), (90.0, 40.0));
        let (yaw, pitch) = aim([0, 0, 0], Some([0, 0, 10]), 0.0);
        assert_eq!(yaw, 0.0);
        assert!((pitch - (10f32.atan2(20.0).to_degrees() + 10.0)).abs() < 1e-4);
        let (yaw, _) = aim([0, 0, 0], Some([10, 0, 0]), 0.0);
        assert!((yaw - 90.0).abs() < 1e-4);
        assert_eq!(placed_yaw(0.0, false), 0.0);
        assert_eq!(placed_yaw(90.0, false), 270.0);
        assert_eq!(placed_yaw(90.0, true), 90.0);
        let aims = vec![
            Aim::Set {
                pos: [1, -2, 3],
                target: Some([4, 5, -6]),
                default_yaw: 22.5,
            },
            Aim::Set {
                pos: [0, 0, 0],
                target: None,
                default_yaw: 0.0,
            },
            Aim::Gone { pos: [7, 8, 9] },
        ];
        assert_eq!(decode(&encode(&aims)), aims);
        assert_eq!(
            Configure::parse(&["mode", "replace_solid"]),
            Some(Configure::Mode(1))
        );
        assert_eq!(
            Configure::parse(&["skip_missing", "true"]),
            Some(Configure::SkipMissing(true))
        );
        assert_eq!(Configure::parse(&["nonsense"]), None);
    }
}
