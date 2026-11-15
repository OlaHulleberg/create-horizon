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
    /// A belt segment's shape, printed with its connector (`launchBlockOrBelt`).
    pub belt: Option<BeltPiece>,
    /// `BlockHelper.prepareBlockEntityData` keeps the schematic's block entity data for the
    /// state: a block tagged `create:safe_nbt` (all of it) or a copycat (`writeSafe`).
    pub safe_nbt: bool,
}

/// A belt segment as the printer reads it: its facing (a horizontal offset), slope and part,
/// and the shaft printed in its place when another end of the belt lays it
/// (`stripBeltIfNotLast`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeltPiece {
    pub facing: [i32; 3],
    pub slope: BeltSlope,
    pub part: BeltPart,
    pub shaft: u32,
}

/// `BeltSlope`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeltSlope {
    Horizontal,
    Upward,
    Downward,
    Vertical,
    Sideways,
}

/// `BeltPart` (the pulley is the belt's middle with a shaft through it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeltPart {
    Start,
    Middle,
    End,
    Pulley,
}

impl BeltPiece {
    /// `BeltBlock.nextSegmentPosition`: the next segment from `pos`, forward or back; none past
    /// the belt's end that way.
    pub fn next(&self, pos: [i32; 3], forward: bool) -> Option<[i32; 3]> {
        let offset = if forward { 1 } else { -1 };
        if (self.part == BeltPart::End && forward) || (self.part == BeltPart::Start && !forward) {
            return None;
        }
        let positive = self.facing.iter().sum::<i32>() > 0;
        if self.slope == BeltSlope::Vertical {
            let up = if positive { offset } else { -offset };
            return Some([pos[0], pos[1] + up, pos[2]]);
        }
        let mut p = [0, 1, 2].map(|i| pos[i] + self.facing[i] * offset);
        match self.slope {
            BeltSlope::Upward => p[1] += offset,
            BeltSlope::Downward => p[1] -= offset,
            _ => {}
        }
        Some(p)
    }

    /// `stripBeltIfNotLast`: whether this end lays the belt (the highest end of a slope, else
    /// the end further along a positive facing).
    pub fn is_last(&self) -> bool {
        let start = self.part == BeltPart::Start;
        let end = self.part == BeltPart::End;
        let positive = self.facing.iter().sum::<i32>() > 0;
        match self.slope {
            BeltSlope::Downward => start,
            BeltSlope::Upward => end,
            _ => (positive && end) || (!positive && start),
        }
    }
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
    /// The template's entities where they are placed, relative to the anchor
    /// (`getEntityList().get(i).blockPosition()`), and the one printing (`printingEntityIndex`).
    pub entities: Vec<[i32; 3]>,
    pub entity_index: i32,
    /// The casing of each cased belt segment in the template (its block entity's `Casing`), by
    /// where it is placed: `CasingType.ANDESITE` 1, `BRASS` 2.
    pub casings: std::collections::HashMap<[i32; 3], u8>,
    /// The template's block entity data (`StructureBlockInfo.nbt`), by where it is placed.
    pub data: std::collections::HashMap<[i32; 3], crate::schematic::Nbt>,
}

/// A belt block entity's `Casing` (`CasingType`'s name; the port's own block entity keeps it in
/// lower case): `NONE` 0, `ANDESITE` 1, `BRASS` 2.
pub fn casing_of(nbt: Option<&crate::schematic::Nbt>) -> u8 {
    match nbt
        .and_then(|n| n.get("Casing"))
        .and_then(crate::schematic::Nbt::text)
        .map(str::to_ascii_uppercase)
        .as_deref()
    {
        Some("ANDESITE") => 1,
        Some("BRASS") => 2,
        _ => 0,
    }
}

impl Printer {
    /// `prepareBlockEntityData` for the block at `rel`: the template's data when its state
    /// keeps it (`Entry::safe_nbt`).
    pub fn block_data(&self, rel: [i32; 3], entry: &Entry) -> Option<crate::schematic::Nbt> {
        entry
            .safe_nbt
            .then(|| self.data.get(&rel).cloned())
            .flatten()
    }

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
        let casings = s
            .blocks
            .iter()
            .filter_map(|b| {
                let c = casing_of(b.nbt.as_ref());
                (c != 0).then(|| (transform(b.pos, mirror, rotation, [0; 3]), c))
            })
            .collect();
        let data = s
            .blocks
            .iter()
            .filter_map(|b| Some((transform(b.pos, mirror, rotation, [0; 3]), b.nbt.clone()?)))
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
            entities: s
                .entities
                .iter()
                .map(|e| {
                    crate::schematic::transform_vec(e.pos, mirror, rotation, [0; 3])
                        .map(|v| v.floor() as i32)
                })
                .collect(),
            entity_index: -1,
            casings,
            data,
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
                let next = self.entity_index + 1;
                match self.entities.get(next as usize) {
                    Some(&p) => {
                        self.entity_index = next;
                        self.current = p;
                    }
                    // The end of printing.
                    None => return false,
                }
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
#[derive(Clone, Debug, PartialEq)]
pub struct Flying {
    pub target: [i32; 3],
    pub state: u32,
    /// A belt (`LaunchedItem.ForBelt`): the other end of the belt it lays from `target`.
    pub belt_to: Option<[i32; 3]>,
    /// The belt's segments' casings from `target` (`ForBelt.casings`, `casing_of`'s numbers).
    pub casings: Vec<u8>,
    /// A block's block entity data (`ForBlockState.data`: `prepareBlockEntityData`'s).
    pub data: Option<crate::schematic::Nbt>,
    /// An entity (`LaunchedItem.ForEntity`).
    pub entity: Option<Box<PlacedEntity>>,
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
    /// Each of the template's entities: what it costs, and what is spawned.
    pub entities: Vec<(Requirement, PlacedEntity)>,
    /// What the level reads where nothing was placed (`Blocks.AIR`).
    pub air: Entry,
}

impl Loaded {
    /// [`launch`] for this schematic.
    pub fn launch(
        &self,
        rel: [i32; 3],
        target: [i32; 3],
        entry: &Entry,
    ) -> (u32, Option<[i32; 3]>, Vec<u8>) {
        launch(&self.printer, &self.entries, rel, target, entry)
    }
}

/// `launchBlockOrBelt`: what is shot at `target` (`rel` from the anchor) for `entry`: a belt's
/// end that doesn't lay the belt goes as a shaft; the one that does lays the whole belt
/// (`beltLength` segments, found along the schematic's belt), to the other end, each segment's
/// casing as the walk meets it.
pub fn launch(
    printer: &Printer,
    entries: &[Entry],
    rel: [i32; 3],
    target: [i32; 3],
    entry: &Entry,
) -> (u32, Option<[i32; 3]>, Vec<u8>) {
    match entry.belt {
        Some(b) if !b.is_last() => (b.shaft, None, Vec::new()),
        Some(b) => {
            let forward = b.part != BeltPart::End;
            let mut at = rel;
            let mut piece = b;
            let mut length = 1;
            let mut casings = vec![printer.casings.get(&at).copied().unwrap_or(0)];
            while let Some(next) = piece.next(at, forward) {
                let Some(e) = printer.at(next).and_then(|i| entries[i as usize].belt) else {
                    break;
                };
                at = next;
                piece = e;
                length += 1;
                casings.push(printer.casings.get(&at).copied().unwrap_or(0));
            }
            let step = b.next([0; 3], b.part == BeltPart::Start).unwrap_or([0; 3]);
            let n = length - 1;
            (
                entry.placed,
                Some([0, 1, 2].map(|i| target[i] + step[i] * n)),
                casings,
            )
        }
        None => (entry.placed, None, Vec::new()),
    }
}

/// Items and how many of each.
pub type Counts = Vec<(String, u32)>;

/// A template's entity as the printer launches it (`LaunchedItem.ForEntity`): its saved data,
/// where it lands (the anchor added) and the template's turn and mirror.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedEntity {
    pub nbt: crate::schematic::Nbt,
    pub pos: [f64; 3],
    pub block_pos: [i32; 3],
    pub rotation: Rotation,
    pub mirror: Mirror,
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
    /// `LaunchedItem.ForBlockState.place`: `placeSchematicBlock` with the block entity data the
    /// cannon took (`prepareBlockEntityData`), loaded into the placed block entity.
    fn place_data(&mut self, pos: [i32; 3], state: u32, data: Option<&crate::schematic::Nbt>) {
        let _ = data;
        self.place(pos, state);
    }
    /// `LaunchedItem.ForBelt.place`: a shaft at `pos`, then the belt from it to `to`, each
    /// segment from `pos` then cased as `casings` says (`setCasingType`).
    fn place_belt(&mut self, pos: [i32; 3], state: u32, to: [i32; 3], casings: &[u8]) {
        let _ = (to, casings);
        self.place(pos, state);
    }
    /// Slot 2 (book input) holds something.
    fn book_input(&mut self) -> bool {
        false
    }
    /// Slot 3 (book output) is at its limit.
    fn book_output_full(&mut self) -> bool {
        false
    }
    /// One from slot 2 written as `checklist` into slot 3 (a book, or a clipboard from one).
    fn write_checklist(&mut self, checklist: &Checklist) {
        let _ = checklist;
    }
    /// What the inventories beside the cannon could give, item by item (`updateChecklist`).
    fn inventory_items(&mut self) -> Vec<(String, u32)> {
        Vec::new()
    }
    /// `LaunchedItem.ForEntity.place`: the entity spawned.
    fn place_entity(&mut self, entity: &PlacedEntity) {
        let _ = entity;
    }
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
    /// `bookPrintingProgress`, `dontUpdateChecklist` and the `checklist` it writes.
    pub paper: f32,
    dont_update_checklist: bool,
    pub checklist: Checklist,
    /// A checklist is being written (the cannon ticks every tick for it).
    printing_paper: bool,
}

/// `MaterialChecklist`: the items the print still needs, in the order first required, and what
/// the inventories beside the cannon hold of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checklist {
    pub required: Vec<(String, u32)>,
    pub gathered: Vec<(String, u32)>,
    pub not_loaded: bool,
}

impl Checklist {
    /// `require`: each consumed item counted.
    pub fn require(&mut self, r: &Requirement) {
        let Requirement::Items(items) = r else { return };
        for (item, n) in items {
            if item == "minecraft:air" {
                continue;
            }
            match self.required.iter_mut().find(|(i, _)| i == item) {
                Some(e) => e.1 += n,
                None => self.required.push((item.clone(), *n)),
            }
        }
    }

    /// `collect`: a stack of a required item is counted as gathered.
    pub fn collect(&mut self, item: &str, n: u32) {
        if !self.required.iter().any(|(i, _)| i == item) {
            return;
        }
        match self.gathered.iter_mut().find(|(i, _)| i == item) {
            Some(e) => e.1 += n,
            None => self.gathered.push((item.to_owned(), n)),
        }
    }

    /// `createWrittenBook`'s order: the items still missing (with what is missing), then the
    /// completed ones (with what was required), each sorted by `name` lowercased.
    pub fn entries(&self, name: impl Fn(&str) -> String) -> (Counts, Counts) {
        let mut keys: Vec<&(String, u32)> = self.required.iter().collect();
        keys.sort_by_key(|(i, _)| name(i).to_lowercase());
        let mut missing = Vec::new();
        let mut done = Vec::new();
        for (item, n) in keys {
            let have = self
                .gathered
                .iter()
                .find(|(i, _)| i == item)
                .map_or(0, |e| e.1);
            if *n > have {
                missing.push((item.clone(), n - have));
            } else {
                done.push((item.clone(), *n));
            }
        }
        (missing, done)
    }
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
                match (&f.entity, f.belt_to) {
                    (Some(e), _) => w.place_entity(e),
                    (None, Some(to)) => w.place_belt(f.target, f.state, to, &f.casings),
                    (None, None) => w.place_data(f.target, f.state, f.data.as_ref()),
                }
            } else {
                i += 1;
            }
        }
        // tickPaperPrinter: a schematic in slot 0 is loaded; a book in slot 2 becomes its
        // checklist in 20 ticks.
        let blueprint = w.blueprint();
        self.has_blueprint = blueprint != Blueprint::Empty;
        self.printing_paper = false;
        if self.loaded.is_none() {
            if self.has_blueprint {
                self.initialize(&blueprint, w);
            }
        } else if !w.book_input() || w.book_output_full() {
            self.paper = 0.0;
            self.dont_update_checklist = false;
        } else if self.paper >= 1.0 {
            self.paper = 0.0;
            if !self.dont_update_checklist {
                self.update_checklist(w);
            }
            self.dont_update_checklist = true;
            w.write_checklist(&self.checklist);
            self.printing_paper = true;
        } else {
            self.paper += 0.05;
            self.printing_paper = true;
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
        if self.printing_paper {
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
        self.update_checklist(w);
        self.blocks_to_place += self.blocks_placed;
    }

    /// `updateChecklist`: what the print still needs (`markAllBlockRequirements`, which also
    /// counts `blocksToPlace`, and `markAllEntityRequirements`, which stops at the first entity
    /// that can't be printed, as Create's does), and what the inventories beside it hold of that.
    pub fn update_checklist(&mut self, w: &mut impl CannonWorld) {
        let mut c = Checklist::default();
        if let Some(l) = &self.loaded {
            let mut n = self.blocks_placed;
            for (rel, i) in l.printer.positions() {
                let pos = [0, 1, 2].map(|k| l.printer.anchor[k] + rel[k]);
                if !w.loaded(pos) {
                    c.not_loaded = true;
                    continue;
                }
                let e = &l.entries[i as usize];
                if !self.should_place_at(w, pos, e) {
                    continue;
                }
                if matches!(e.requirement, Requirement::Items(_)) {
                    c.require(&e.requirement);
                    n += 1;
                }
            }
            for (r, _) in &l.entities {
                if !matches!(r, Requirement::Items(_)) {
                    break;
                }
                c.require(r);
            }
            self.blocks_to_place = n;
        }
        for (item, n) in w.inventory_items() {
            c.collect(&item, n);
        }
        self.checklist = c;
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
        // An entity's turn (`PrintStage.ENTITIES`): its own requirement, always placed.
        let entity = if l.printer.stage == Stage::Entities {
            l.entities.get(l.printer.entity_index as usize)
        } else {
            None
        };
        let requirement = entity.map_or(&entry.requirement, |e| &e.0);
        if *requirement == Requirement::Invalid
            || (entity.is_none() && !self.should_place_at(w, target, entry))
        {
            self.status = Status::Searching;
            return true;
        }
        // `grabItemsFromAttachedInventories`: a creative crate supplies everything.
        if let Requirement::Items(items) = requirement
            && !self.creative
        {
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
        self.status = if entry.plain_air && entity.is_none() {
            Status::Clearing
        } else {
            Status::Placing
        };
        if let Some((_, e)) = entity {
            // `launchEntity`.
            self.blocks_placed += 1;
            let total = flight_ticks(self.pos, target);
            self.flying.push(Flying {
                target,
                state: 0,
                belt_to: None,
                casings: Vec::new(),
                data: None,
                entity: Some(Box::new(e.clone())),
                total,
                land_at: now + u64::from(total) + 1,
            });
            w.fired(now);
            self.cooldown = DELAY;
            self.fuel -= 1;
            self.missing = None;
            return false;
        }
        if !entry.air {
            self.blocks_placed += 1;
        }
        let (state, belt_to, casings) = l.launch(rel, target, entry);
        let data = belt_to
            .is_none()
            .then(|| l.printer.block_data(rel, entry))
            .flatten();
        let total = flight_ticks(self.pos, target);
        self.flying.push(Flying {
            target,
            state,
            belt_to,
            casings,
            data,
            entity: None,
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

    #[test]
    fn a_checklist_lists_what_is_missing_then_what_is_done() {
        let mut c = Checklist::default();
        c.require(&Requirement::Items(vec![("minecraft:stone".into(), 3)]));
        c.require(&Requirement::Items(vec![
            ("minecraft:dirt".into(), 2),
            ("minecraft:stone".into(), 1),
        ]));
        c.require(&Requirement::Invalid);
        c.collect("minecraft:dirt", 5);
        c.collect("minecraft:stone", 1);
        c.collect("minecraft:glass", 9);
        let name = |i: &str| i.trim_start_matches("minecraft:").to_owned();
        let (missing, done) = c.entries(name);
        assert_eq!(missing, vec![("minecraft:stone".to_owned(), 3)]);
        assert_eq!(done, vec![("minecraft:dirt".to_owned(), 2)]);
        assert!(c.gathered.iter().all(|(i, _)| i != "minecraft:glass"));
    }

    #[test]
    fn only_states_that_keep_their_data_print_it() {
        // A sign's data at (1, 0, 0), turned a quarter: it lands where the turn puts it.
        let mut s = structure([2, 1, 1], &[([0, 0, 0], 0), ([1, 0, 0], 1)], 2);
        let text =
            crate::schematic::Nbt::compound([("Text", crate::schematic::Nbt::String("hi".into()))]);
        s.blocks[1].nbt = Some(text.clone());
        let p = Printer::load(&s, [0; 3], Rotation::Clockwise90, Mirror::None);
        let at = transform([1, 0, 0], Mirror::None, Rotation::Clockwise90, [0; 3]);
        let mut safe = entry(1, false);
        assert_eq!(p.block_data(at, &safe), None);
        safe.safe_nbt = true;
        assert_eq!(p.block_data(at, &safe), Some(text));
        assert_eq!(p.block_data([0; 3], &safe), None);
    }

    #[test]
    fn a_belt_is_laid_from_its_last_end() {
        let east = |part| BeltPiece {
            facing: [1, 0, 0],
            slope: BeltSlope::Horizontal,
            part,
            shaft: 7,
        };
        // Facing east (positive): the end lays it; the start goes as a shaft.
        assert!(east(BeltPart::End).is_last() && !east(BeltPart::Start).is_last());
        assert_eq!(east(BeltPart::Start).next([0; 3], true), Some([1, 0, 0]));
        assert_eq!(east(BeltPart::End).next([5, 0, 0], false), Some([4, 0, 0]));
        assert_eq!(east(BeltPart::End).next([5, 0, 0], true), None);
        // Upward: the end is the highest, a step up along the facing.
        let up = BeltPiece {
            slope: BeltSlope::Upward,
            ..east(BeltPart::End)
        };
        assert!(up.is_last());
        assert_eq!(up.next([3, 2, 0], false), Some([2, 1, 0]));
        let vertical = BeltPiece {
            facing: [0, 0, -1],
            slope: BeltSlope::Vertical,
            ..east(BeltPart::Start)
        };
        assert_eq!(vertical.next([0, 0, 0], true), Some([0, -1, 0]));
    }
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
            belt: None,
            safe_nbt: false,
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
            belt: None,
            safe_nbt: false,
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
                entities: Vec::new(),
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
