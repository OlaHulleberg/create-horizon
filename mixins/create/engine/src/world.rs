//! Create's state for one open world: what each Create block state is (decoded once), and each
//! dimension's kinetic network. The game glue feeds it block changes and chunk loads; nothing here
//! runs per tick unless something changed.

use crate::Block;
use crate::kinetics::{Event, Kinetics, SolveStats};
use crate::parts::{Settings, StateInfo, is_kinetic, parts};
use crate::states;
use std::collections::HashMap;

/// Every Create block state, by `state - first`: its block, its decoded properties, and whether it
/// holds kinetic parts. Built once when a world opens, never per tick.
pub struct StateTable {
    first: u32,
    block: Vec<Block>,
    info: Vec<StateInfo>,
    kinetic: Vec<bool>,
}

impl StateTable {
    /// From the registry's states `first..first + n`: each name (`create:shaft`) and properties.
    pub fn new<'a, P>(first: u32, states: impl Iterator<Item = (&'a str, P)>) -> StateTable
    where
        P: Iterator<Item = (&'a str, &'a str)>,
    {
        let mut t = StateTable {
            first,
            block: Vec::new(),
            info: Vec::new(),
            kinetic: Vec::new(),
        };
        for (name, props) in states {
            let block = name
                .strip_prefix("create:")
                .and_then(Block::from_name)
                // A Create id this build doesn't know (another version's): inert.
                .unwrap_or(Block::AndesiteCasing);
            t.kinetic.push(is_kinetic(block) || has_state(block));
            t.block.push(block);
            t.info.push(StateInfo::read(props));
        }
        t
    }

    /// The Create block of a state id, if it is one.
    #[inline]
    pub fn get(&self, state: u32) -> Option<(Block, &StateInfo)> {
        let i = states::index(state)? as usize;
        Some((*self.block.get(i)?, self.info.get(i)?))
    }

    /// Whether a state is a Create block this keeps (kinetic, or with state of its own like a
    /// depot): the bit test of the hot hooks.
    #[inline]
    pub fn is_kinetic(&self, state: u32) -> bool {
        states::index(state).is_some_and(|i| self.kinetic.get(i as usize).copied().unwrap_or(false))
    }

    pub fn first(&self) -> u32 {
        self.first
    }

    pub fn len(&self) -> usize {
        self.block.len()
    }

    pub fn is_empty(&self) -> bool {
        self.block.is_empty()
    }
}

/// One dimension's Create state.
#[derive(Default)]
pub struct Dimension {
    pub kinetics: Kinetics,
    /// Kinetic block positions per loaded column, to forget them when it unloads.
    columns: HashMap<(i32, i32), Vec<[i32; 3]>>,
    /// Block entity settings that change a part (a motor's speed), by position.
    settings: HashMap<[i32; 3], Settings>,
    /// The state of each kinetic block, by position.
    states: HashMap<[i32; 3], u32>,
    /// Columns whose saved data changed, to mark unsaved when the dimension is in place.
    unsaved: Vec<(i32, i32)>,
    /// Hand cranks being turned: ticks left (`HandCrankBlockEntity.inUse`). Empty at rest, so
    /// cranks cost nothing per tick unless turned.
    cranks: HashMap<[i32; 3], u8>,
    /// Millstones and their state (`MillstoneBlockEntity.timer`, last recipe).
    pub millstones: HashMap<[i32; 3], crate::processing::Millstone>,
    /// Water wheels, to look at their flow every 60 ticks (`lazyTick`), staggered by position.
    wheels: Vec<[i32; 3]>,
    /// Water wheels to look at now: placed, or a neighbour changed.
    wheel_checks: Vec<[i32; 3]>,
    /// Blocks whose redstone power may have changed (a clutch or gearshift that was placed or
    /// whose neighbour changed), checked after the level's tick.
    redstone: Vec<[i32; 3]>,
    /// Depots and their held stack's motion; the stacks are in their block entities' slots.
    pub depots: HashMap<[i32; 3], Option<crate::depot::Held>>,
    /// Depots whose held stack still slides: the only ones that tick.
    pub sliding: Vec<[i32; 3]>,
    /// Depots whose contents clients must see again (put down, taken, filled by a hopper).
    pub depot_changes: Vec<[i32; 3]>,
    /// Mechanical presses and their cycle.
    pub presses: HashMap<[i32; 3], crate::press::Press>,
    /// Depots that ask the press above them each tick (`DepotBehaviour.tick` after its stack
    /// arrived), until the press has nothing to do with them.
    pub depot_work: Vec<[i32; 3]>,
    /// Presses whose cycle clients must see again (started, head down, done).
    pub press_changes: Vec<[i32; 3]>,
    /// Belts by controller, and each segment's controller.
    pub belts: HashMap<[i32; 3], Belt>,
    pub belt_of: HashMap<[i32; 3], [i32; 3]>,
    /// Belt segments placed or loaded whose chain is looked for after the tick.
    belt_dirty: Vec<[i32; 3]>,
    /// Belts whose items or shape clients must see again.
    pub belt_changes: Vec<[i32; 3]>,
    /// Segments to replace because their neighbour broke, and items a broken belt drops (with
    /// the controller and segment they were on).
    pub belt_breaks: Vec<BeltBreak>,
    pub belt_drops: Vec<([i32; 3], crate::belt::Segment, i32, crate::belt::Item)>,
    /// Items saved with a belt, kept until its chain is whole again after loading.
    belt_saved: HashMap<[i32; 3], crate::belt::Inventory>,
    /// Latches whose inputs may have changed, looked at after the tick
    /// (`checkTickOnNeighbor`), and their scheduled ticks (position, game time due).
    pub latch_checks: Vec<[i32; 3]>,
    pub latch_ticks: Vec<([i32; 3], i64)>,
    /// Fluid tanks and their contents; contents saved with a column wait here for their tank.
    pub tanks: HashMap<[i32; 3], crate::tank::Tank>,
    tank_saved: HashMap<[i32; 3], crate::tank::Tank>,
    /// Tanks clients must see again.
    pub tank_changes: Vec<[i32; 3]>,
    /// Windmill bearings whose sails are assembled, by bearing; ones saved with a column wait
    /// for their bearing; right-clicks waiting for the next tick (`assembleNextTick`); bearings
    /// clients must see again; and structures whose bearing broke, to put back.
    pub windmills: HashMap<[i32; 3], Windmill>,
    windmill_saved: HashMap<[i32; 3], Windmill>,
    pub windmill_assemble: Vec<[i32; 3]>,
    pub windmill_changes: Vec<[i32; 3]>,
    pub windmill_orphans: Vec<([i32; 3], Windmill)>,
    /// Encased fans, and the ones turning: only those tick.
    pub fans: HashMap<[i32; 3], Fan>,
    pub running_fans: Vec<[i32; 3]>,
    /// Item entities' fan progress (Create keeps it in their persistent data), by entity id; kept
    /// while they are in a current.
    pub item_fan: HashMap<u32, crate::fan::Progress>,
    /// Crushing wheels whose controllers must be looked at (placed, removed, speed changed), and
    /// the controllers: their state and crushing speed; the turning ones tick. Contents saved with
    /// a column wait for their controller.
    pub crusher_checks: Vec<[i32; 3]>,
    pub crushers: HashMap<[i32; 3], (crate::crusher::Crusher, f32)>,
    pub running_crushers: Vec<[i32; 3]>,
    crusher_saved: HashMap<[i32; 3], crate::crusher::Crusher>,
}

/// An assembled windmill: its blocks (offset from the bearing, state) and its sails.
#[derive(Clone, Debug, PartialEq)]
pub struct Windmill {
    pub blocks: Vec<([i32; 3], u32)>,
    pub sails: u32,
    pub clockwise: bool,
}

/// An encased fan (`EncasedFanBlockEntity`): its facing, speed and air current, and the depots
/// and belt segments in the current with the processing each gets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fan {
    pub facing: Option<crate::kinetics::Dir>,
    pub speed: f32,
    pub current: crate::fan::Current,
    /// `airCurrentUpdateCooldown`: ticks until the current is looked at again.
    pub cooldown: u32,
    /// `updateAirFlow`: rebuild the current this tick.
    pub rebuild: bool,
    pub handlers: Vec<([i32; 3], Option<crate::fan::Kind>)>,
    /// Clients must see its propeller and current again.
    pub changed: bool,
    /// `entitySearchCooldown` and the item entities its last search caught (`caughtEntities`).
    pub search_cooldown: u8,
    pub caught: Vec<u32>,
}

/// Blocks that keep state of their own here but are not kinetic: depots.
fn has_state(block: Block) -> bool {
    matches!(
        block,
        Block::Depot
            | Block::CrushingWheelController
            | Block::Belt
            | Block::PoweredLatch
            | Block::PoweredToggleLatch
            | Block::FluidTank
            | Block::WindmillBearing
    )
}

/// Create's latches, by block.
pub fn latch_kind(block: Block) -> Option<crate::latch::Kind> {
    match block {
        Block::PoweredLatch => Some(crate::latch::Kind::Powered),
        Block::PoweredToggleLatch => Some(crate::latch::Kind::Toggle),
        _ => None,
    }
}

/// A whole belt: its segments from the controller, the controller's segment, its items.
#[derive(Clone, Debug)]
pub struct Belt {
    pub chain: Vec<[i32; 3]>,
    pub segment: crate::belt::Segment,
    pub items: crate::belt::Inventory,
}

/// What breaking a belt segment does to the next ones (`BeltBlock.onRemove`): each becomes a
/// shaft on the belt's axis (a pulley) or air.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeltBreak {
    pub pos: [i32; 3],
    pub shaft: Option<crate::kinetics::Axis>,
}

/// Blocks that follow redstone power (`powered`): Create's split shafts.
fn follows_power(block: Block) -> bool {
    matches!(block, Block::Clutch | Block::Gearshift)
}

/// Create's state for the open world: three vanilla dimensions, by index.
pub struct CreateWorld {
    pub table: StateTable,
    pub dims: [Dimension; 3],
}

/// `minecraft:overworld` 0, `minecraft:the_nether` 1, `minecraft:the_end` 2.
pub fn dimension_index(id: &str) -> Option<usize> {
    match id {
        "minecraft:overworld" => Some(0),
        "minecraft:the_nether" => Some(1),
        "minecraft:the_end" => Some(2),
        _ => None,
    }
}

impl CreateWorld {
    pub fn new(table: StateTable) -> CreateWorld {
        CreateWorld {
            table,
            dims: Default::default(),
        }
    }

    fn place(&mut self, dim: usize, pos: [i32; 3], state: u32) {
        let Some((block, info)) = self.table.get(state) else {
            return;
        };
        let d = &mut self.dims[dim];
        if block == Block::Millstone {
            d.millstones.entry(pos).or_default();
        }
        if block == Block::MechanicalPress && !d.presses.contains_key(&pos) {
            d.presses.insert(pos, crate::press::Press::default());
            // A press placed over a depot looks at what it holds.
            d.depot_work.push([pos[0], pos[1] - 2, pos[2]]);
        }
        if latch_kind(block).is_some() {
            d.latch_checks.push(pos);
        }
        if block == Block::CrushingWheel {
            d.crusher_checks.push(pos);
        }
        if block == Block::CrushingWheelController && !d.crushers.contains_key(&pos) {
            let c = d.crusher_saved.remove(&pos).unwrap_or_default();
            d.crushers.insert(pos, (c, 0.0));
        }
        if block == Block::EncasedFan {
            // Placed, loaded or turned by a wrench: its current is looked at again.
            let fan = d.fans.entry(pos).or_default();
            fan.facing = info.facing;
            fan.rebuild = true;
        }
        if block == Block::WindmillBearing
            && !d.windmills.contains_key(&pos)
            && let Some(w) = d.windmill_saved.remove(&pos)
        {
            let speed = crate::contraption::windmill_speed(w.sails, w.clockwise);
            d.windmills.insert(pos, w);
            d.windmill_changes.push(pos);
            let old = d.settings.get(&pos).copied().unwrap_or_default();
            d.settings.insert(
                pos,
                Settings {
                    windmill: speed,
                    ..old
                },
            );
        }
        let d = &mut self.dims[dim];
        if block == Block::FluidTank && !d.tanks.contains_key(&pos) {
            let t = d.tank_saved.remove(&pos).unwrap_or_default();
            d.tanks.insert(pos, t);
            d.tank_changes.push(pos);
        }
        if block == Block::Belt && !d.belt_of.contains_key(&pos) {
            d.belt_dirty.push(pos);
        }
        if block == Block::Depot && !d.depots.contains_key(&pos) {
            // Placed or loaded: clients draw what its block entity holds.
            d.depots.insert(pos, None);
            d.depot_changes.push(pos);
        }
        let settings = d.settings.get(&pos).copied().unwrap_or_default();
        let [a, b] = parts(block, info, &settings);
        d.kinetics.remove(pos);
        d.states.insert(pos, state);
        d.columns
            .entry((pos[0] >> 4, pos[2] >> 4))
            .or_default()
            .push(pos);
        if let Some(a) = a {
            d.kinetics.set(pos, 0, a);
        }
        if let Some(b) = b {
            d.kinetics.set(pos, 1, b);
        }
    }

    /// A committed block change (`native_block_changed`). The first lines are the hot path: two
    /// range tests that miss for every non-Create block.
    #[inline]
    pub fn block_changed(&mut self, dim: usize, pos: [i32; 3], old: u32, new: u32) {
        let (was, is) = (self.table.is_kinetic(old), self.table.is_kinetic(new));
        if !was && !is {
            return;
        }
        if is {
            if self.table.get(new).is_some_and(|(b, _)| follows_power(b))
                && self.table.get(old).map(|(b, _)| b) != self.table.get(new).map(|(b, _)| b)
            {
                // ClutchBlock.getStateForPlacement: powered as placed.
                self.dims[dim].redstone.push(pos);
            }
            if self.table.get(new).map(|(b, _)| b) == Some(Block::Millstone) {
                self.dims[dim].millstones.entry(pos).or_default();
            }
            if self.table.get(new).map(|(b, _)| b) == Some(Block::WaterWheel)
                && self.table.get(old).map(|(b, _)| b) != Some(Block::WaterWheel)
            {
                self.dims[dim].wheels.push(pos);
                self.dims[dim].wheel_checks.push(pos);
            }
            self.place(dim, pos, new);
        } else {
            let d = &mut self.dims[dim];
            d.kinetics.remove(pos);
            d.states.remove(&pos);
            d.cranks.remove(&pos);
            d.wheels.retain(|p| *p != pos);
            d.millstones.remove(&pos);
            d.presses.remove(&pos);
            if self.table.get(old).map(|(b, _)| b) == Some(Block::CrushingWheel) {
                d.crusher_checks.push(pos);
            }
            if d.crushers.remove(&pos).is_some() {
                d.running_crushers.retain(|p| *p != pos);
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if d.fans.remove(&pos).is_some() {
                d.running_fans.retain(|p| *p != pos);
            }
            if let Some(w) = d.windmills.remove(&pos) {
                d.windmill_orphans.push((pos, w));
                d.windmill_changes.push(pos);
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if d.tanks.remove(&pos).is_some() {
                d.tank_changes.push(pos);
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if let Some(seg) = self.table.get(old).and_then(|(_, i)| i.belt) {
                self.belt_removed(dim, pos, seg);
            }
            let d = &mut self.dims[dim];
            if d.depots.remove(&pos).is_some() {
                d.sliding.retain(|p| *p != pos);
                d.depot_changes.push(pos);
            }
            if d.settings.remove(&pos).is_some() {
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if let Some(list) = d.columns.get_mut(&(pos[0] >> 4, pos[2] >> 4)) {
                list.retain(|p| *p != pos);
            }
        }
    }

    /// One section of a column that joined the level: `base` is its lowest corner, `states` its
    /// 4,096 states in vanilla's index order (`y << 8 | z << 4 | x`).
    pub fn section_loaded(&mut self, dim: usize, base: [i32; 3], states: &[u32; 4096]) {
        for (i, &s) in states.iter().enumerate() {
            if self.table.is_kinetic(s) {
                let pos = [
                    base[0] + (i & 15) as i32,
                    base[1] + (i >> 8) as i32,
                    base[2] + ((i >> 4) & 15) as i32,
                ];
                self.place(dim, pos, s);
            }
        }
    }

    /// A column left the level: its parts leave the network (their neighbours re-solve).
    pub fn column_unloaded(&mut self, dim: usize, cx: i32, cz: i32) {
        let d = &mut self.dims[dim];
        if let Some(list) = d.columns.remove(&(cx, cz)) {
            for pos in list {
                d.kinetics.remove(pos);
                d.states.remove(&pos);
                d.cranks.remove(&pos);
            }
            d.wheels.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.millstones
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.depots.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.presses.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.fans.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.crushers.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.running_crushers
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.crusher_saved
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.running_fans
                .retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.tanks.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.windmills
                .retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            // A belt reaching into the column stops being whole until it is back.
            let gone: Vec<[i32; 3]> = d
                .belt_of
                .iter()
                .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
                .map(|(_, c)| *c)
                .collect();
            for c in gone {
                if let Some(b) = d.belts.remove(&c) {
                    for p in &b.chain {
                        d.belt_of.remove(p);
                        if (p[0] >> 4, p[2] >> 4) != (cx, cz) {
                            d.belt_dirty.push(*p);
                        }
                    }
                }
            }
            d.belt_dirty.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.depot_work.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
            d.sliding.retain(|p| (p[0] >> 4, p[2] >> 4) != (cx, cz));
        }
        d.settings.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
    }

    /// A block entity setting that changes a part (a motor's speed), then re-places the block.
    pub fn set_settings(&mut self, dim: usize, pos: [i32; 3], state: u32, settings: Settings) {
        self.dims[dim].settings.insert(pos, settings);
        if self.table.is_kinetic(state) {
            self.place(dim, pos, state);
        }
    }

    /// Re-solves what changed in a dimension: nothing at all when nothing did.
    #[inline]
    pub fn tick(&mut self, dim: usize, events: &mut Vec<Event>) -> SolveStats {
        let d = &mut self.dims[dim];
        if !d.kinetics.has_dirty() {
            return SolveStats::default();
        }
        let from = events.len();
        let stats = d.kinetics.solve(events);
        // Fans follow their speed (`onSpeedChanged`: the current is rebuilt); crushing wheels
        // look at their controllers again.
        for e in &events[from..] {
            if let Event::Speed { pos, .. } = *e
                && d.states
                    .get(&pos)
                    .and_then(|&s| self.table.get(s))
                    .is_some_and(|(b, _)| b == Block::CrushingWheel)
            {
                d.crusher_checks.push(pos);
            }
            if let Event::Speed { pos, speed, .. } = *e
                && let Some(fan) = d.fans.get_mut(&pos)
                && fan.speed != speed
            {
                fan.speed = speed;
                fan.rebuild = true;
                if !d.running_fans.contains(&pos) {
                    d.running_fans.push(pos);
                }
            }
        }
        stats
    }

    /// Fans to tick: each turning one, and one that just stopped (its current goes). Those whose
    /// current is due to be looked at again (`fanBlockCheckRate`) are marked to rebuild.
    pub fn fans_to_tick(&mut self, dim: usize) -> Vec<[i32; 3]> {
        let d = &mut self.dims[dim];
        let mut out = Vec::with_capacity(d.running_fans.len());
        d.running_fans.retain(|pos| {
            let Some(fan) = d.fans.get_mut(pos) else {
                return false;
            };
            if fan.cooldown == 0 {
                fan.cooldown = crate::fan::CHECK_RATE;
                fan.rebuild = true;
            } else {
                fan.cooldown -= 1;
            }
            out.push(*pos);
            fan.speed != 0.0
        });
        out
    }

    /// A player scrolls on the creative motor at `pos`: its speed moves by one per step, between
    /// -256 and 256, never 0 (Create 5's scroll value; Create 6 sets it on a value board).
    /// Returns the new speed, or None when there is no motor there.
    pub fn scroll_motor(&mut self, dim: usize, pos: [i32; 3], steps: i32) -> Option<f32> {
        let state = *self.dims[dim].states.get(&pos)?;
        let (block, _) = self.table.get(state)?;
        if block != Block::CreativeMotor || steps == 0 {
            return None;
        }
        let d = &mut self.dims[dim];
        let old = d.settings.get(&pos).copied().unwrap_or_default().motor_rpm as i32;
        let mut new = old;
        for _ in 0..steps.unsigned_abs().min(512) {
            new += steps.signum();
            if new == 0 {
                new += steps.signum();
            }
        }
        let new = new.clamp(-256, 256) as f32;
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        self.set_settings(
            dim,
            pos,
            state,
            Settings {
                motor_rpm: new,
                ..old
            },
        );
        Some(new)
    }

    /// The settings of a column's blocks, to save with it: `(position, motor speed)`.
    pub fn column_settings(&self, dim: usize, cx: i32, cz: i32) -> Vec<([i32; 3], i32)> {
        let mut out: Vec<([i32; 3], i32)> = self.dims[dim]
            .settings
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
            .filter(|(p, _)| {
                self.dims[dim]
                    .states
                    .get(*p)
                    .and_then(|s| self.table.get(*s))
                    .map(|(b, _)| b)
                    == Some(Block::CreativeMotor)
            })
            .map(|(p, s)| (*p, s.motor_rpm as i32))
            .collect();
        out.sort();
        out
    }

    /// A player turns the hand crank at `pos` (`HandCrankBlockEntity.turn`): it generates for
    /// the next 10 ticks, backwards when `back` (sneaking). False when there is no crank there.
    pub fn turn_crank(&mut self, dim: usize, pos: [i32; 3], back: bool) -> bool {
        let Some(&state) = self.dims[dim].states.get(&pos) else {
            return false;
        };
        if self.table.get(state).map(|(b, _)| b) != Some(Block::HandCrank) {
            return false;
        }
        let d = &mut self.dims[dim];
        d.cranks.insert(pos, 10);
        let crank = if back { -1 } else { 1 };
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        if old.crank != crank {
            self.set_settings(dim, pos, state, Settings { crank, ..old });
        }
        true
    }

    /// One tick of the cranks being turned: those whose time ran out stop.
    pub fn tick_cranks(&mut self, dim: usize) {
        if self.dims[dim].cranks.is_empty() {
            return;
        }
        let mut stopped = Vec::new();
        for (pos, left) in self.dims[dim].cranks.iter_mut() {
            *left -= 1;
            if *left == 0 {
                stopped.push(*pos);
            }
        }
        for pos in stopped {
            let d = &mut self.dims[dim];
            d.cranks.remove(&pos);
            if let (Some(&state), Some(old)) = (d.states.get(&pos), d.settings.get(&pos).copied()) {
                self.set_settings(dim, pos, state, Settings { crank: 0, ..old });
            }
        }
    }

    /// Settings saved with a column, before its blocks are scanned.
    pub fn load_settings(&mut self, dim: usize, saved: impl Iterator<Item = ([i32; 3], i32)>) {
        for (pos, rpm) in saved {
            self.dims[dim].settings.insert(
                pos,
                Settings {
                    motor_rpm: rpm as f32,
                    ..Settings::default()
                },
            );
        }
    }

    /// A neighbour of the block `state` at `pos` changed: a block that follows redstone is
    /// checked after the tick. The first line is the hot path for every other block.
    #[inline]
    pub fn neighbor_changed(&mut self, dim: usize, pos: [i32; 3], state: u32) {
        if !self.table.is_kinetic(state) {
            return;
        }
        match self.table.get(state).map(|(b, _)| b) {
            Some(b) if follows_power(b) => self.dims[dim].redstone.push(pos),
            Some(Block::WaterWheel) => self.dims[dim].wheel_checks.push(pos),
            Some(b) if latch_kind(b).is_some() => self.dims[dim].latch_checks.push(pos),
            _ => {}
        }
    }

    /// The water wheels to look at in this tick (`tick` counts ticks): those whose neighbour
    /// changed, and one in 60 of the others (Create's `lazyTick` every 60 ticks), with their axis.
    pub fn take_wheel_checks(
        &mut self,
        dim: usize,
        tick: u64,
    ) -> Vec<([i32; 3], crate::kinetics::Axis)> {
        let d = &mut self.dims[dim];
        let mut list = std::mem::take(&mut d.wheel_checks);
        list.extend(d.wheels.iter().filter(|p| {
            (p[0].wrapping_mul(31) ^ p[2].wrapping_mul(17) ^ p[1]).rem_euclid(60) as u64
                == tick % 60
        }));
        list.sort();
        list.dedup();
        list.into_iter()
            .filter_map(|p| {
                let state = *self.dims[dim].states.get(&p)?;
                let (block, info) = self.table.get(state)?;
                (block == Block::WaterWheel).then_some((p, info.facing?.axis()))
            })
            .collect()
    }

    /// Millstones that turn: the only ones that do anything this tick (Create's tick returns at
    /// once without speed), with their speed.
    pub fn turning_millstones(&self, dim: usize) -> Vec<([i32; 3], f32)> {
        let d = &self.dims[dim];
        d.millstones
            .keys()
            .filter_map(|p| Some((*p, d.kinetics.speed(*p, 0).filter(|s| *s != 0.0)?)))
            .collect()
    }

    /// A water wheel's new flow score: it turns again when the score changed.
    pub fn set_wheel_flow(&mut self, dim: usize, pos: [i32; 3], score: i32) {
        let flow = score.clamp(-1, 1) as i8;
        let d = &mut self.dims[dim];
        let Some(&state) = d.states.get(&pos) else {
            return;
        };
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        if old.flow != flow {
            self.set_settings(dim, pos, state, Settings { flow, ..old });
        }
    }

    /// Blocks to check for redstone power, with whether each is powered now.
    pub fn take_redstone(&mut self, dim: usize) -> Vec<[i32; 3]> {
        let mut v = std::mem::take(&mut self.dims[dim].redstone);
        v.sort();
        v.dedup();
        v
    }

    /// Whether the block `state` is powered (`powered`), for a block that follows redstone.
    pub fn powered(&self, state: u32) -> Option<bool> {
        let (block, info) = self.table.get(state)?;
        follows_power(block).then_some(info.powered)
    }

    /// Columns whose saved data changed since the last call.
    /// A stack was put on the depot at `pos` with this motion (it slides from now on), or its
    /// held stack left (None).
    pub fn depot_put(&mut self, dim: usize, pos: [i32; 3], held: Option<crate::depot::Held>) {
        let d = &mut self.dims[dim];
        if let Some(h) = d.depots.get_mut(&pos) {
            *h = held;
            if held.is_some_and(|h| !h.sliding()) && !d.depot_work.contains(&pos) {
                d.depot_work.push(pos);
            }
            if held.is_some_and(|h| h.sliding()) && !d.sliding.contains(&pos) {
                d.sliding.push(pos);
            }
            d.depot_changes.push(pos);
        }
    }

    /// A belt segment left (`BeltBlock.onRemove`): its belt drops its items, and the segments
    /// next to it break in turn (a pulley leaves its shaft).
    fn belt_removed(&mut self, dim: usize, pos: [i32; 3], seg: crate::belt::Segment) {
        let d = &mut self.dims[dim];
        if let Some(c) = d.belt_of.remove(&pos)
            && let Some(belt) = d.belts.remove(&c)
        {
            let length = belt.chain.len() as i32;
            for item in belt.items.items {
                d.belt_drops.push((c, belt.segment, length, item));
            }
            for p in &belt.chain {
                d.belt_of.remove(p);
            }
            d.belt_changes.push(c);
        }
        for forward in [true, false] {
            let Some(next) = seg.next(pos, forward) else {
                continue;
            };
            let Some(n) = d
                .states
                .get(&next)
                .and_then(|s| self.table.get(*s))
                .and_then(|(_, i)| i.belt)
            else {
                continue;
            };
            d.belt_breaks.push(BeltBreak {
                pos: next,
                shaft: n.has_pulley().then(|| n.rotation_axis()),
            });
        }
    }

    /// Belts whose segments were placed or loaded: their chains, once whole (Create's
    /// `initBelt`), become belts whose segments turn together. Nothing to do when no belt
    /// changed.
    pub fn settle_belts(&mut self, dim: usize) {
        if self.dims[dim].belt_dirty.is_empty() {
            return;
        }
        let dirty = std::mem::take(&mut self.dims[dim].belt_dirty);
        for pos in dirty {
            if self.dims[dim].belt_of.contains_key(&pos) {
                continue;
            }
            let table = &self.table;
            let states = &self.dims[dim].states;
            let segment_at = |p: [i32; 3]| {
                states
                    .get(&p)
                    .and_then(|s| table.get(*s))
                    .and_then(|(b, i)| (b == Block::Belt).then_some(i.belt).flatten())
            };
            let Some(controller) = crate::belt::controller_of(pos, segment_at) else {
                continue;
            };
            let chain = crate::belt::chain(controller, segment_at);
            let whole = chain.len() >= 2
                && chain
                    .last()
                    .and_then(|p| segment_at(*p))
                    .is_some_and(|s| s.part == crate::belt::Part::End);
            if !whole {
                continue;
            }
            let Some(segment) = segment_at(controller) else {
                continue;
            };
            let d = &mut self.dims[dim];
            for p in &chain {
                d.belt_of.insert(*p, controller);
            }
            let items = d.belt_saved.remove(&controller).unwrap_or_default();
            d.belts.insert(
                controller,
                Belt {
                    chain: chain.clone(),
                    segment,
                    items,
                },
            );
            d.belt_changes.push(controller);
            for p in chain {
                let Some(&state) = self.dims[dim].states.get(&p) else {
                    continue;
                };
                let old = self.dims[dim].settings.get(&p).copied().unwrap_or_default();
                self.dims[dim].settings.insert(
                    p,
                    Settings {
                        belt: Some(controller),
                        ..old
                    },
                );
                self.place(dim, p, state);
            }
        }
    }

    /// Items saved with belts in a column (`chunk_data_save`), for when their chains form.
    pub fn load_belt_items(
        &mut self,
        dim: usize,
        saved: impl Iterator<Item = ([i32; 3], crate::belt::Inventory)>,
    ) {
        self.dims[dim].belt_saved.extend(saved);
    }

    /// The belts whose controller is in a column, with their items, to save.
    pub fn column_belts(
        &self,
        dim: usize,
        cx: i32,
        cz: i32,
    ) -> Vec<([i32; 3], crate::belt::Inventory)> {
        let mut out: Vec<_> = self.dims[dim]
            .belts
            .iter()
            .filter(|(c, b)| (c[0] >> 4, c[2] >> 4) == (cx, cz) && !b.items.is_empty())
            .map(|(c, b)| (*c, b.items.to_save()))
            .collect();
        out.sort_by_key(|(c, _)| *c);
        out
    }

    /// A windmill's sails were assembled (`assemble`): it generates from the next solve on.
    pub fn windmill_assembled(&mut self, dim: usize, pos: [i32; 3], state: u32, w: Windmill) {
        let speed = crate::contraption::windmill_speed(w.sails, w.clockwise);
        let d = &mut self.dims[dim];
        d.windmills.insert(pos, w);
        d.windmill_changes.push(pos);
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        self.set_settings(
            dim,
            pos,
            state,
            Settings {
                windmill: speed,
                ..old
            },
        );
    }

    /// A windmill was taken apart (`disassemble`): its structure, to put back; it stops.
    pub fn windmill_disassembled(
        &mut self,
        dim: usize,
        pos: [i32; 3],
        state: u32,
    ) -> Option<Windmill> {
        let d = &mut self.dims[dim];
        let w = d.windmills.remove(&pos)?;
        d.windmill_changes.push(pos);
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        let old = d.settings.get(&pos).copied().unwrap_or_default();
        self.set_settings(
            dim,
            pos,
            state,
            Settings {
                windmill: 0.0,
                ..old
            },
        );
        Some(w)
    }

    /// Windmills saved with a column, for when their bearings are found.
    pub fn load_windmills(
        &mut self,
        dim: usize,
        saved: impl Iterator<Item = ([i32; 3], Windmill)>,
    ) {
        self.dims[dim].windmill_saved.extend(saved);
    }

    /// The assembled windmills of a column, to save.
    pub fn column_windmills(&self, dim: usize, cx: i32, cz: i32) -> Vec<([i32; 3], Windmill)> {
        let mut out: Vec<_> = self.dims[dim]
            .windmills
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
            .map(|(p, w)| (*p, w.clone()))
            .collect();
        out.sort_by_key(|(p, _)| *p);
        out
    }

    /// A controller's crushing speed (`updateSpeed`): it ticks while not 0.
    pub fn set_crusher_speed(&mut self, dim: usize, pos: [i32; 3], speed: f32) {
        let d = &mut self.dims[dim];
        let Some(c) = d.crushers.get_mut(&pos) else {
            return;
        };
        c.1 = speed;
        d.running_crushers.retain(|p| *p != pos);
        if speed != 0.0 {
            d.running_crushers.push(pos);
        }
    }

    /// Controller contents saved with a column, for when their controllers are found.
    pub fn load_crushers(
        &mut self,
        dim: usize,
        saved: impl Iterator<Item = ([i32; 3], crate::crusher::Crusher)>,
    ) {
        self.dims[dim].crusher_saved.extend(saved);
    }

    /// The controllers of a column that hold something, to save.
    pub fn column_crushers(
        &self,
        dim: usize,
        cx: i32,
        cz: i32,
    ) -> Vec<([i32; 3], crate::crusher::Crusher)> {
        let mut out: Vec<_> = self.dims[dim]
            .crushers
            .iter()
            .filter(|(p, c)| (p[0] >> 4, p[2] >> 4) == (cx, cz) && !c.0.slots.is_empty())
            .map(|(p, c)| (*p, c.0.clone()))
            .collect();
        out.sort_by_key(|(p, _)| *p);
        out
    }

    /// Tank contents saved with a column, for when its tanks are found.
    pub fn load_tanks(
        &mut self,
        dim: usize,
        saved: impl Iterator<Item = ([i32; 3], crate::tank::Tank)>,
    ) {
        self.dims[dim].tank_saved.extend(saved);
    }

    /// The tanks of a column that hold something, to save.
    pub fn column_tanks(&self, dim: usize, cx: i32, cz: i32) -> Vec<([i32; 3], crate::tank::Tank)> {
        let mut out: Vec<_> = self.dims[dim]
            .tanks
            .iter()
            .filter(|(p, t)| (p[0] >> 4, p[2] >> 4) == (cx, cz) && t.fluid.is_some())
            .map(|(p, t)| (*p, *t))
            .collect();
        out.sort_by_key(|(p, _)| *p);
        out
    }

    /// The column of `pos` must be saved again (its belt's items moved).
    pub fn mark_unsaved(&mut self, dim: usize, pos: [i32; 3]) {
        self.dims[dim].unsaved.push((pos[0] >> 4, pos[2] >> 4));
    }

    /// Belts that hold items: the only ones that tick.
    pub fn busy_belts(&self, dim: usize) -> Vec<[i32; 3]> {
        self.dims[dim]
            .belts
            .iter()
            .filter(|(_, b)| !b.items.is_empty())
            .map(|(c, _)| *c)
            .collect()
    }

    /// The depot at `pos` changed in a way clients must see (`notifyUpdate`).
    pub fn depot_changed(&mut self, dim: usize, pos: [i32; 3]) {
        if self.dims[dim].depots.contains_key(&pos) {
            self.dims[dim].depot_changes.push(pos);
        }
    }

    /// The depot at `pos` asks the press above it from the next tick on (its stack arrived or
    /// changed, the press finished or started turning).
    pub fn wake_depot(&mut self, dim: usize, pos: [i32; 3]) {
        let d = &mut self.dims[dim];
        if d.depots.contains_key(&pos) && !d.depot_work.contains(&pos) {
            d.depot_work.push(pos);
        }
    }

    /// `DepotBehaviour.tick` for the depots whose stack slides; `upright(pos)` says whether the
    /// stack there stands (`BeltHelper.isItemUpright`). Depots at rest cost nothing.
    pub fn tick_depots(&mut self, dim: usize, upright: impl Fn([i32; 3]) -> bool) {
        let d = &mut self.dims[dim];
        if d.sliding.is_empty() {
            return;
        }
        let depots = &mut d.depots;
        d.sliding.retain(|p| {
            let Some(Some(h)) = depots.get_mut(p) else {
                return false;
            };
            if h.slide(upright(*p)) && !d.depot_work.contains(p) {
                // Arrived: from now on Create's depot asks the processing above it.
                d.depot_work.push(*p);
            }
            h.sliding()
        });
    }

    /// Depots whose contents changed since the last call, once each.
    pub fn take_depot_changes(&mut self, dim: usize) -> Vec<[i32; 3]> {
        let mut v = std::mem::take(&mut self.dims[dim].depot_changes);
        v.sort();
        v.dedup();
        v
    }

    pub fn take_unsaved(&mut self, dim: usize) -> Vec<(i32, i32)> {
        let mut v = std::mem::take(&mut self.dims[dim].unsaved);
        v.sort();
        v.dedup();
        v
    }

    /// `IRotate.hasShaftTowards`: whether the kinetic block at `pos` has a shaft on `face`.
    pub fn has_shaft_towards(&self, dim: usize, pos: [i32; 3], face: crate::kinetics::Dir) -> bool {
        let d = &self.dims[dim];
        let Some((block, info)) = d.states.get(&pos).and_then(|&s| self.table.get(s)) else {
            return false;
        };
        let settings = d.settings.get(&pos).copied().unwrap_or_default();
        parts(block, info, &settings)
            .iter()
            .flatten()
            .any(|p| p.shafts & face.bit() != 0)
    }

    pub fn speed(&self, dim: usize, pos: [i32; 3]) -> Option<f32> {
        self.dims[dim].kinetics.speed(pos, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny registry: two vanilla states, then Create's shaft (x, y, z) and creative motor
    /// facing east and west.
    fn world() -> CreateWorld {
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:shaft", vec![("axis", "x")]),
            ("create:shaft", vec![("axis", "y")]),
            ("create:shaft", vec![("axis", "z")]),
            ("create:creative_motor", vec![("facing", "east")]),
            ("create:creative_motor", vec![("facing", "west")]),
            ("create:andesite_casing", vec![]),
        ];
        states::set_range(2, names.len() as u32);
        let table = StateTable::new(2, names.iter().map(|(n, p)| (*n, p.iter().copied())));
        CreateWorld::new(table)
    }

    #[test]
    fn worlds_solve_on_change_and_pay_nothing_when_steady() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = world();
        let (shaft_x, motor_east, casing) = (2, 5, 7);
        assert!(!w.table.is_kinetic(0) && !w.table.is_kinetic(casing));
        // Vanilla changes miss at once.
        w.block_changed(0, [5, 5, 5], 0, 1);
        assert_eq!(w.tick(0, &mut Vec::new()), SolveStats::default());
        w.block_changed(0, [0, 64, 0], 0, motor_east);
        w.block_changed(0, [1, 64, 0], 0, shaft_x);
        let mut e = Vec::new();
        assert_eq!(w.tick(0, &mut e).components, 1);
        assert_eq!(w.speed(0, [1, 64, 0]), Some(16.0));
        assert_eq!(w.tick(0, &mut e), SolveStats::default(), "steady: no work");
        // The motor's speed setting re-solves it.
        w.set_settings(
            0,
            [0, 64, 0],
            motor_east,
            Settings {
                motor_rpm: 64.0,
                ..Settings::default()
            },
        );
        w.tick(0, &mut e);
        assert_eq!(w.speed(0, [1, 64, 0]), Some(64.0));
        // Breaking the motor stops the shaft.
        w.block_changed(0, [0, 64, 0], motor_east, 0);
        w.tick(0, &mut e);
        assert_eq!(w.speed(0, [1, 64, 0]), Some(0.0));
        states::clear_range();
    }

    #[test]
    fn scrolling_a_motor_skips_zero_and_saves_with_its_column() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = world();
        let motor_east = 5;
        w.block_changed(0, [1, 64, 1], 0, motor_east);
        assert_eq!(
            w.scroll_motor(0, [1, 64, 1], -16),
            Some(-1.0),
            "16 down 16 skips 0"
        );
        assert_eq!(w.scroll_motor(0, [1, 64, 1], 1), Some(1.0));
        assert_eq!(w.scroll_motor(0, [1, 64, 1], 1000), Some(256.0));
        assert_eq!(w.scroll_motor(0, [2, 64, 1], 1), None, "no motor there");
        assert_eq!(w.take_unsaved(0), [(0, 0)]);
        assert_eq!(w.column_settings(0, 0, 0), [([1, 64, 1], 256)]);
        // A reload brings the setting back before the motor is found.
        w.column_unloaded(0, 0, 0);
        assert!(w.column_settings(0, 0, 0).is_empty());
        w.load_settings(0, [([1, 64, 1], 256)].into_iter());
        w.block_changed(0, [1, 64, 1], 0, motor_east);
        w.block_changed(0, [2, 64, 1], 0, 2);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [2, 64, 1]), Some(256.0));
        states::clear_range();
    }

    #[test]
    fn split_shafts_are_checked_for_power_when_placed_or_their_neighbour_changes() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            ("create:clutch", vec![("axis", "x"), ("powered", "false")]),
            ("create:clutch", vec![("axis", "x"), ("powered", "true")]),
            ("create:shaft", vec![("axis", "x")]),
        ];
        states::set_range(10, 3);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        w.block_changed(0, [0, 0, 0], 0, 10);
        assert_eq!(w.take_redstone(0), [[0, 0, 0]], "placed: checked once");
        w.block_changed(0, [0, 0, 0], 10, 11);
        assert!(
            w.take_redstone(0).is_empty(),
            "its own power change is no new check"
        );
        w.neighbor_changed(0, [0, 0, 0], 11);
        w.neighbor_changed(0, [1, 0, 0], 12);
        w.neighbor_changed(0, [2, 0, 0], 3);
        assert_eq!(
            w.take_redstone(0),
            [[0, 0, 0]],
            "shafts and vanilla blocks are not"
        );
        assert_eq!(w.powered(11), Some(true));
        assert_eq!(w.powered(12), None);
        states::clear_range();
    }

    #[test]
    fn a_depot_ticks_only_while_its_stack_slides() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![("create:depot", vec![])];
        states::set_range(10, 1);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        w.block_changed(0, [0, 0, 0], 0, 10);
        assert_eq!(
            w.take_depot_changes(0),
            [[0, 0, 0]],
            "placed: clients see it"
        );
        assert!(w.dims[0].sliding.is_empty(), "an empty depot does nothing");
        let held = crate::depot::Held::put_by_player(0, false, crate::depot::Side::East);
        w.depot_put(0, [0, 0, 0], Some(held));
        assert_eq!(w.take_depot_changes(0), [[0, 0, 0]]);
        let mut ticks = 0;
        while !w.dims[0].sliding.is_empty() {
            w.tick_depots(0, |_| false);
            ticks += 1;
        }
        assert!((5..30).contains(&ticks), "{ticks}");
        let at = w.dims[0].depots[&[0, 0, 0]].unwrap();
        assert!(0.5 - at.position <= 1.0 / 512.0);
        assert!(
            w.take_depot_changes(0).is_empty(),
            "arriving is no change to send"
        );
        w.column_unloaded(0, 0, 0);
        assert!(w.dims[0].depots.is_empty());
        states::clear_range();
    }

    #[test]
    fn a_belt_turns_as_one_and_breaks_as_create_s_does() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let belt = |part: &'static str| {
            (
                "create:belt",
                vec![
                    ("facing", "east"),
                    ("slope", "horizontal"),
                    ("part", part),
                    ("casing", "false"),
                    ("waterlogged", "false"),
                ],
            )
        };
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            belt("start"),
            belt("middle"),
            belt("end"),
            ("create:creative_motor", vec![("facing", "south")]),
        ];
        states::set_range(10, 4);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        // A belt east along x from 0 to 2; its pulleys turn on z. A motor north of the start
        // faces south into it.
        w.block_changed(0, [0, 0, 0], 0, 10);
        w.block_changed(0, [1, 0, 0], 0, 11);
        w.block_changed(0, [2, 0, 0], 0, 12);
        w.block_changed(0, [0, 0, -1], 0, 13);
        w.settle_belts(0);
        assert_eq!(w.dims[0].belts[&[0, 0, 0]].chain.len(), 3);
        assert_eq!(w.dims[0].belt_of[&[2, 0, 0]], [0, 0, 0]);
        w.tick(0, &mut Vec::new());
        // The motor turns the start's pulley; the segments carry it to the end.
        let start = w.speed(0, [0, 0, 0]).unwrap();
        assert!(start != 0.0);
        assert_eq!(w.speed(0, [1, 0, 0]), Some(start));
        assert_eq!(w.speed(0, [2, 0, 0]), Some(start));
        // Breaking the middle: the start and end become shafts (they are pulleys).
        w.block_changed(0, [1, 0, 0], 11, 0);
        assert!(w.dims[0].belts.is_empty());
        let breaks = std::mem::take(&mut w.dims[0].belt_breaks);
        assert_eq!(breaks.len(), 2);
        assert!(
            breaks
                .iter()
                .all(|b| b.shaft == Some(crate::kinetics::Axis::Z))
        );
        states::clear_range();
    }

    #[test]
    fn a_turned_crank_drives_for_ten_ticks() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let names: Vec<(&str, Vec<(&str, &str)>)> = vec![
            (
                "create:hand_crank",
                vec![("facing", "west"), ("waterlogged", "false")],
            ),
            ("create:shaft", vec![("axis", "x")]),
        ];
        states::set_range(10, 2);
        let mut w = CreateWorld::new(StateTable::new(
            10,
            names.iter().map(|(n, p)| (*n, p.iter().copied())),
        ));
        // Facing west, hanging from the block to its east: the shaft at x + 1.
        w.block_changed(0, [0, 0, 0], 0, 10);
        w.block_changed(0, [1, 0, 0], 0, 11);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [1, 0, 0]), Some(0.0));
        assert!(w.turn_crank(0, [0, 0, 0], false));
        w.tick(0, &mut Vec::new());
        // convertToDirection(32, west): -32 along x.
        assert_eq!(w.speed(0, [1, 0, 0]), Some(-32.0));
        for _ in 0..9 {
            w.tick_cranks(0);
        }
        w.tick(0, &mut Vec::new());
        assert_eq!(
            w.speed(0, [1, 0, 0]),
            Some(-32.0),
            "still turning at tick 9"
        );
        w.tick_cranks(0);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [1, 0, 0]), Some(0.0), "stopped after 10 ticks");
        assert!(!w.turn_crank(0, [1, 0, 0], false), "a shaft is no crank");
        states::clear_range();
    }

    #[test]
    fn loaded_sections_join_and_unloaded_columns_leave() {
        let _guard = states::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = world();
        let mut states = [0u32; 4096];
        // x 0..3 at y 0, z 0 of a section at (16, 64, 32): a motor then two shafts.
        states[0] = 5;
        states[1] = 2;
        states[2] = 2;
        w.section_loaded(0, [16, 64, 32], &states);
        w.tick(0, &mut Vec::new());
        assert_eq!(w.speed(0, [18, 64, 32]), Some(16.0));
        w.column_unloaded(0, 1, 2);
        assert_eq!(w.speed(0, [18, 64, 32]), None);
        assert!(w.dims[0].kinetics.is_empty());
        states::clear_range();
    }
}
