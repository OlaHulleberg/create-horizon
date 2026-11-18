//! Rotation networks, after Create's `RotationPropagator`, `KineticNetwork` and `KineticBlockEntity`.
//!
//! Per-node sources and network identities follow Create's block entity propagation.

mod legacy_load;
mod networks;
use legacy_load::LoadOrigin;
mod propagation;
mod scheduling;
mod state;
use networks::TorquePropagator;
use state::KineticState;
pub use state::{PartLocation, SavedKineticState, SavedNetwork};

use crate::block_entity_order::{BlockEntityOrder, View};
use std::collections::{HashMap, HashSet};

/// Create's `maxRotationSpeed` (CKinetics, default 256).
pub const MAX_SPEED: f32 = 256.0;

/// `RotationPropagator.MAX_FLICKER_SCORE`: a part whose flicker score is past this breaks when
/// speed next reaches it.
pub const MAX_FLICKER_SCORE: i32 = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub fn of(v: [i32; 3], axis: Axis) -> i32 {
        match axis {
            Axis::X => v[0],
            Axis::Y => v[1],
            Axis::Z => v[2],
        }
    }
}

/// Vanilla's `Direction`, in its data order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Dir {
    pub const ALL: [Dir; 6] = [
        Dir::Down,
        Dir::Up,
        Dir::North,
        Dir::South,
        Dir::West,
        Dir::East,
    ];

    pub fn offset(self) -> [i32; 3] {
        match self {
            Dir::Down => [0, -1, 0],
            Dir::Up => [0, 1, 0],
            Dir::North => [0, 0, -1],
            Dir::South => [0, 0, 1],
            Dir::West => [-1, 0, 0],
            Dir::East => [1, 0, 0],
        }
    }
    pub fn axis(self) -> Axis {
        match self {
            Dir::Down | Dir::Up => Axis::Y,
            Dir::North | Dir::South => Axis::Z,
            Dir::West | Dir::East => Axis::X,
        }
    }
    pub fn positive(self) -> bool {
        matches!(self, Dir::Up | Dir::South | Dir::East)
    }
    pub fn opposite(self) -> Dir {
        match self {
            Dir::Down => Dir::Up,
            Dir::Up => Dir::Down,
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::West => Dir::East,
            Dir::East => Dir::West,
        }
    }
    pub fn bit(self) -> u8 {
        1 << self as u8
    }
    pub fn from_offset(o: [i32; 3]) -> Option<Dir> {
        Dir::ALL.into_iter().find(|d| d.offset() == o)
    }
    pub fn from_axis(axis: Axis, positive: bool) -> Dir {
        match (axis, positive) {
            (Axis::X, true) => Dir::East,
            (Axis::X, false) => Dir::West,
            (Axis::Y, true) => Dir::Up,
            (Axis::Y, false) => Dir::Down,
            (Axis::Z, true) => Dir::South,
            (Axis::Z, false) => Dir::North,
        }
    }
}

/// Create's `KineticBlockEntity.convertToDirection`: a setting (a motor's RPM) as speed along the
/// axis of the face it turns, negated for a face on the axis' negative side.
pub fn convert_to_direction(speed: f32, facing: Dir) -> f32 {
    if facing.positive() { speed } else { -speed }
}

/// Create's `convertToAngular`: RPM as degrees per tick.
pub fn convert_to_angular(rpm: f32) -> f32 {
    rpm * 360.0 / 60.0 / 20.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cog {
    None,
    Small,
    Large,
}

/// Create's DirectionalShaftHalvesBlockEntity shaft modifiers; speed stays in the source frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShaftRotation {
    Straight,
    Gearbox,
    Split(f32),
}
impl ShaftRotation {
    /// RotationPropagator.getAxisModifier: no source yet means modifier one.
    pub fn modifier(self, source: Option<Dir>, face: Dir) -> f32 {
        let Some(source) = source else { return 1.0 };
        match self {
            Self::Straight => 1.0,
            Self::Split(m) => {
                if face == source {
                    1.0
                } else {
                    m
                }
            }
            Self::Gearbox => {
                if face.axis() == source.axis() {
                    if face == source { 1.0 } else { -1.0 }
                } else if face.positive() == source.positive() {
                    -1.0
                } else {
                    1.0
                }
            }
        }
    }
}

/// One kinetic part as the solver sees it: what it connects to and what it adds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    /// Generator initialization entry point used when a saved entity returns.
    pub initialization: GeneratorInitialization,
    /// KineticBlockEntity subclass notification after setSpeed.
    pub speed_changed: SpeedChanged,
    /// The rotation axis.
    pub axis: Axis,
    /// Faces with a shaft end (`Dir::bit`), Create's `hasShaftTowards`.
    pub shafts: u8,
    /// Source-relative shaft behaviour (`RotationPropagator.getAxisModifier`).
    pub rotation: ShaftRotation,
    pub cog: Cog,
    /// RPM this part generates, signed along its axis (0 for none).
    pub generated: f32,
    /// Stress capacity per RPM of generated speed (SU).
    pub capacity: f32,
    /// Stress impact per RPM (SU).
    pub impact: f32,
    /// A belt segment's controller (its chain's start): segments of one belt turn together
    /// (`BeltBlockEntity.propagateRotationTo`).
    pub belt: Option<[i32; 3]>,
    /// A sloped belt segment reaches its neighbours on the slope, across an edge
    /// (`BeltBlockEntity.canPropagateDiagonally`).
    pub diagonal: bool,
    /// An encased chain drive's chain to its neighbours (`ChainDriveBlock`).
    pub chain: Option<Chain>,
    /// A speed set across to another part instead of a ratio: a rotation speed controller's, or a
    /// sequenced gearshift's second half.
    pub couple: Option<Couple>,
    /// Whether copySequenceContextFrom inherits or is overridden with a no-op.
    /// The context value itself lives only in KineticState.seq (and its saved record).
    pub context: SequenceContextSource,
    /// A gantry shaft or carriage, which a powered shaft and the carriage on it join across
    /// (`GantryShaftBlockEntity.isCustomConnection`, `propagateRotationTo`).
    pub gantry: Option<Gantry>,
}

/// Server-side onSpeedChanged callback owned by the kinetic entity subclass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeedChanged {
    Base,
    /// StressGaugeBlockEntity refreshes its cached network size and clears networkDirty.
    StressGauge,
}

/// CreativeMotorBlockEntity.initialize updates generation on every load; hand cranks
/// inherit KineticBlockEntity.initialize and only change generation on use/expiry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeneratorInitialization {
    EveryLoad,
    Placement,
}

/// The gantry's side of a part: a gantry shaft's facing and power, or a carriage's facing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gantry {
    Shaft { facing: Dir, powered: bool },
    Carriage { facing: Dir },
}

/// `GantryCarriageBlockEntity.getGantryPinionModifier`: the speed a powered gantry shaft facing
/// `shaft` and the carriage facing `pinion` on it turn each other at.
pub fn gantry_pinion_modifier(shaft: Dir, pinion: Dir) -> f32 {
    let direction = if shaft.positive() { 1.0 } else { -1.0 };
    let reversed = match shaft.axis() {
        Axis::Y => matches!(pinion, Dir::North | Dir::East),
        Axis::X => matches!(pinion, Dir::Down | Dir::South),
        Axis::Z => matches!(pinion, Dir::Up | Dir::West),
    };
    if reversed { -direction } else { direction }
}

/// `SequencedGearshiftBlockEntity.SequenceContext` as its holder sets it: turn so many degrees
/// (`TURN_ANGLE`) or metres (`TURN_DISTANCE`). `run` tells one start of it from the next, as
/// Create's holder detaches and attaches its network again for each.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Context {
    pub distance: bool,
    pub value: f32,
    pub run: u32,
}

/// `KineticBlockEntity.copySequenceContextFrom` normally inherits the source's context.
/// Valve handles and sequenced gearshifts override it with a no-op and own even an absent
/// context (`HandCrankBlockEntity.tick`, `SequencedGearshiftBlockEntity.run`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SequenceContextSource {
    Inherited,
    Owned,
}

/// The chain of an encased chain drive or adjustable chain gearshift: the axis it runs along
/// (`getConnectionAxis`), whether it goes on toward the positive (`START`, `MIDDLE`) and the
/// negative (`MIDDLE`, `END`) side, and the speed it gives the chain
/// (`ChainGearshiftBlockEntity.getModifier`, 1 for a plain drive).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chain {
    pub along: Axis,
    pub positive: bool,
    pub negative: bool,
    pub modifier: f32,
}

/// A part that sets another's speed, whichever turns first driving the other
/// (`RotationPropagator.getConveyedSpeed`, `SplitShaftBlockEntity.getRotationSpeedModifier`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Couple {
    /// A rotation speed controller's target (RPM): the large cog on a horizontal axis across it,
    /// above it, turns at it while the controller turns, and the controller at it while only
    /// the cog does (`SpeedControllerBlockEntity.getDesiredOutputSpeed`).
    Controller(f32),
    /// On a split shaft's second half: the half away from the one turned turns at this times
    /// its speed (a sequenced gearshift's instruction modifier; 0 cuts).
    Relay(f32),
}

impl Part {
    /// A part on `axis` with shafts at both ends of it.
    pub fn shaft(axis: Axis) -> Part {
        Part {
            initialization: GeneratorInitialization::Placement,
            speed_changed: SpeedChanged::Base,
            axis,
            shafts: Dir::from_axis(axis, true).bit() | Dir::from_axis(axis, false).bit(),
            rotation: ShaftRotation::Straight,
            cog: Cog::None,
            generated: 0.0,
            capacity: 0.0,
            impact: 0.0,
            belt: None,
            diagonal: false,
            chain: None,
            couple: None,
            context: SequenceContextSource::Inherited,
            gantry: None,
        }
    }
    pub fn cog(axis: Axis, large: bool) -> Part {
        Part {
            cog: if large { Cog::Large } else { Cog::Small },
            ..Part::shaft(axis)
        }
    }
    /// GearboxBlockEntity: four shaft faces around the axis, relative to getSourceFacing.
    pub fn gearbox(axis: Axis) -> Part {
        Part {
            shafts: Dir::ALL
                .into_iter()
                .filter(|d| d.axis() != axis)
                .fold(0, |mask, d| mask | d.bit()),
            rotation: ShaftRotation::Gearbox,
            ..Part::shaft(axis)
        }
    }
    /// One SplitShaftBlockEntity, with the output modifier relative to its source.
    pub fn split_shaft(axis: Axis, modifier: f32) -> Part {
        Part {
            rotation: ShaftRotation::Split(modifier),
            ..Part::shaft(axis)
        }
    }
    /// A shaft end on one face only (a motor's output, one half of a split shaft).
    pub fn half(face: Dir) -> Part {
        Part {
            shafts: face.bit(),
            ..Part::shaft(face.axis())
        }
    }
}

/// Create's `getRotationSpeedModifier(from, to)` for two parts, `diff` = to - from.
pub fn ratio(a: &Part, b: &Part, diff: [i32; 3]) -> f32 {
    // BeltBlockEntity.propagateRotationTo: a belt's segments, side by side, unless their
    // shafts already meet.
    if a.belt.is_some() && a.belt == b.belt {
        let via_axes = Dir::from_offset(diff)
            .is_some_and(|d| a.shafts & d.bit() != 0 && b.shafts & d.opposite().bit() != 0);
        if !via_axes {
            return 1.0;
        }
    }
    if let Some(d) = Dir::from_offset(diff)
        && a.shafts & d.bit() != 0
        && b.shafts & d.opposite().bit() != 0
    {
        return 1.0;
    }
    // A powered gantry shaft and the carriage on its side facing away from it, either way
    // (`GantryShaftBlockEntity.propagateRotationTo`, `GantryCarriageBlockEntity
    // .propagateRotationTo`); an unpowered one conveys nothing to it.
    if let (Some(d), Some(x), Some(y)) = (Dir::from_offset(diff), a.gantry, b.gantry) {
        match (x, y) {
            (Gantry::Shaft { facing, powered }, Gantry::Carriage { facing: c })
            | (Gantry::Carriage { facing: c }, Gantry::Shaft { facing, powered })
                if (if matches!(x, Gantry::Shaft { .. }) {
                    d
                } else {
                    d.opposite()
                }) == c =>
            {
                return if powered {
                    gantry_pinion_modifier(facing, c)
                } else {
                    0.0
                };
            }
            _ => {}
        }
    }
    // Attached encased chain drives: ChainDriveBlock.areBlocksConnected, at
    // `ChainDriveBlock.getRotationSpeedModifier` (from's modifier over to's).
    if let (Some(d), Some(x), Some(y)) = (Dir::from_offset(diff), a.chain, b.chain) {
        let connected = x.along == y.along
            && d.axis() == x.along
            && if d.positive() { x.positive } else { x.negative };
        return if connected {
            x.modifier / y.modifier
        } else {
            0.0
        };
    }
    let axes = [Axis::X, Axis::Y, Axis::Z];
    // Large <-> large on crossed axes, diagonally.
    if a.cog == Cog::Large && b.cog == Cog::Large && a.axis != b.axis {
        let fits = axes.iter().all(|&x| {
            let c = Axis::of(diff, x);
            if x == a.axis || x == b.axis {
                c != 0
            } else {
                c == 0
            }
        });
        if fits {
            let s = Axis::of(diff, a.axis) > 0;
            let t = Axis::of(diff, b.axis) > 0;
            return if s ^ t { -1.0 } else { 1.0 };
        }
    }
    let large_to_small = |l: &Part, s: &Part| {
        l.axis == s.axis
            && Axis::of(diff, l.axis) == 0
            && axes
                .iter()
                .filter(|&&x| x != l.axis)
                .all(|&x| Axis::of(diff, x).abs() == 1)
    };
    if a.cog == Cog::Large && b.cog == Cog::Small && large_to_small(a, b) {
        return -2.0;
    }
    if b.cog == Cog::Large && a.cog == Cog::Small && large_to_small(b, a) {
        return -0.5;
    }
    if a.cog == Cog::Small && b.cog == Cog::Small {
        let manhattan: i32 = diff.iter().map(|c| c.abs()).sum();
        if let Some(d) = Dir::from_offset(diff)
            && manhattan == 1
            && d.axis() != a.axis
            && a.axis == b.axis
        {
            return -1.0;
        }
    }
    0.0
}

/// Positions a part may connect to: the six faces, and for small cogs and sloped belts the
/// edge diagonals across their axis (Create's `getPotentialNeighbourLocations` and
/// `addPropagationLocations`; a large cog's diagonals are its small neighbours' too).
fn neighbour_offsets(p: &Part) -> impl Iterator<Item = [i32; 3]> {
    let faces = Dir::ALL.into_iter().map(Dir::offset);
    let axis = p.axis;
    let across = p.diagonal;
    let diagonals = (p.cog != Cog::None || across)
        .then(|| {
            (-1..=1).flat_map(move |x| {
                (-1..=1).flat_map(move |y| {
                    (-1..=1)
                        .map(move |z| [x, y, z])
                        .filter(|o| o.iter().map(|c| c * c).sum::<i32>() == 2)
                        .filter(move |o| !across || Axis::of(*o, axis) == 0)
                })
            })
        })
        .into_iter()
        .flatten();
    faces.chain(diagonals)
}

fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// What a solve changed, for the game to apply: speeds to show and act on, blocks to break.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A part's speed (0 when stopped or overstressed) and whether its network is overstressed.
    Speed {
        pos: [i32; 3],
        sub: u8,
        speed: f32,
        overstressed: bool,
        /// The speed its network turns it at, overstressed or not (`getTheoreticalSpeed`).
        theoretical: f32,
    },
    /// Create destroys this block: a conflicting direction, a loop that disagrees, or too fast.
    Break { pos: [i32; 3] },
}

#[derive(Clone, Debug)]
struct Node {
    pos: [i32; 3],
    entity: u64,
    sub: u8,
    part: Part,
    /// Incarnation for queued block entity work.
    stamp: u64,
    state: KineticState,
    live: bool,
    /// Set or replaced since the last solve: reported even when its speed is unchanged, so the
    /// game shows every part (a still one too).
    fresh: bool,
    report_queued: bool,
    load_origin: LoadOrigin,
}

impl Node {
    /// KineticBlockEntity.calculateStressApplied, including its cached save value.
    fn calculate_stress_applied(&mut self) -> f32 {
        self.state.last_stress = self.part.impact;
        self.state.last_stress
    }

    /// KineticBlockEntity.calculateAddedStressCapacity (also callable when not a source).
    fn calculate_added_stress_capacity(&mut self) -> f32 {
        self.state.last_capacity = self.part.capacity;
        self.state.last_capacity
    }
}

/// The kinetic parts of one dimension.
#[derive(Default)]
pub struct Kinetics {
    nodes: Vec<Node>,
    free: Vec<u32>,
    /// Nodes at a position: index 0, and a split shaft's second half at 1.
    at: HashMap<[i32; 3], [u32; 2]>,
    /// Kinetic entities in level registration order, indexed independently of recycled ports.
    order: BlockEntityOrder,
    /// Parts joined to parts beyond their neighbours at ratio 1 (chain conveyors' chains,
    /// `ChainConveyorBlockEntity.propagateRotationTo`): both sides list each other.
    far: HashMap<[i32; 3], Vec<[i32; 3]>>,
    /// Wake-up work only; update_speed on each entity owns attachment and order owns traversal.
    dirty: Vec<u32>,
    legacy_loads: Vec<(u32, u64)>,
    legacy_loading: bool,
    changed: Vec<u32>,
    saved_dirty: HashSet<[i32; 3]>,
    pending_events: Vec<Event>,
    /// Synchronous KineticBlockEntity.onSpeedChanged callbacks, separate from reports.
    speed_notifications: Vec<([i32; 3], f32)>,
    networks: TorquePropagator,
    scheduler: scheduling::TickScheduler,
    reactivation_due: Vec<(u32, u64)>,
    /// Positions whose parts left since the last `take_removed`.
    removed: Vec<[i32; 3]>,
    stamp: u64,
    /// Positions whose network's figures someone reads (gauges), and those of them a solve
    /// looked at since the last `take_touched`.
    watched: HashSet<[i32; 3]>,
    touched: Vec<[i32; 3]>,
    /// The ticks counted (`advance`), for the parts' flicker scores.
    now: u64,
    /// Monotonic base-tick pass, including phases run before solve.
    base_pass: u64,
}

/// Create's BlockPos.asLong, used by GeneratingKineticBlockEntity.createNetworkId.
pub fn network_id(pos: [i32; 3]) -> i64 {
    ((pos[0] as i64 & 0x3ffffff) << 38)
        | ((pos[2] as i64 & 0x3ffffff) << 12)
        | (pos[1] as i64 & 0xfff)
}
fn self_location(n: &Node) -> PartLocation {
    PartLocation {
        pos: n.pos,
        sub: n.sub,
    }
}

const NONE: u32 = u32::MAX;

/// The networks a solve looked at, for tests and benchmarks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SolveStats {
    pub components: u32,
    pub nodes: u32,
}

impl Kinetics {
    fn resolve_source(&self, source: Option<PartLocation>) -> Option<u32> {
        let source = source?;
        self.at
            .get(&source.pos)
            .map(|ids| ids[source.sub as usize])
            .filter(|&id| id != NONE)
    }

    /// An engine-owned field of a kinetic block entity changed (Java write/sendData).
    pub fn mark_saved_changed(&mut self, pos: [i32; 3]) {
        self.saved_dirty.insert(pos);
    }

    /// Positions whose persistent kinetic fields changed, without scanning idle parts.
    pub fn take_saved_changes(&mut self) -> HashSet<[i32; 3]> {
        std::mem::take(&mut self.saved_dirty)
    }

    /// KineticBlockEntity.write: a portable snapshot, independent of graph slot identities.
    pub fn saved_state(&self, pos: [i32; 3], sub: u8) -> Option<SavedKineticState> {
        let id = *self.at.get(&pos)?.get(sub as usize)?;
        let n = self.nodes.get(id as usize)?;
        if n.load_origin == LoadOrigin::Legacy {
            return None;
        }
        Some(SavedKineticState {
            speed: n.state.theoretical,
            sequence: n.state.seq,
            source: n.state.source,
            network: n.state.net.map(|id| SavedNetwork {
                id,
                stress: n.state.stress,
                capacity: n.state.capacity,
                size: n.state.network_size,
                added_stress: n.state.last_stress,
                added_capacity: n.state.last_capacity,
            }),
            needs_speed_update: n.state.update_speed,
        })
    }

    /// KineticBlockEntity.read, before the first tick. Source positions may
    /// refer to entities whose columns have not loaded yet.
    pub fn restore_state(&mut self, pos: [i32; 3], sub: u8, saved: SavedKineticState) -> bool {
        let Some(id) = self
            .at
            .get(&pos)
            .map(|ids| ids[sub as usize])
            .filter(|&id| id != NONE)
        else {
            return false;
        };
        if !self.nodes[id as usize].state.lifecycle.is_new() {
            return false;
        }
        let n = &mut self.nodes[id as usize];
        n.load_origin = LoadOrigin::Saved;
        n.state.theoretical = saved.speed;
        n.state.seq = saved.sequence;
        n.state.source = saved.source;
        if let Some(net) = saved.network {
            n.state.net = Some(net.id);
            n.state.stress = net.stress;
            n.state.capacity = net.capacity;
            n.state.network_size = net.size;
            n.state.last_stress = net.added_stress;
            n.state.last_capacity = net.added_capacity;
            n.state.overstressed = net.capacity < net.stress;
        }
        let n = &mut self.nodes[id as usize];
        n.state.speed = if n.state.overstressed {
            0.0
        } else {
            saved.speed
        };
        // read deliberately leaves the constructor's updateSpeed=true, as Create does.
        n.state.update_speed = true;
        if self.legacy_loading {
            self.legacy_loads.push((id, n.stamp));
        }
        self.mark_changed(id);
        true
    }

    /// Explicit assignment to KineticBlockEntity.sequenceContext by activate/run/tick.
    /// read restores the same field through restore_state; part settings never replace it.
    pub fn set_sequence_context(
        &mut self,
        pos: [i32; 3],
        sub: u8,
        sequence: Option<(Context, f32)>,
    ) {
        let Some(id) = self
            .at
            .get(&pos)
            .map(|ids| ids[sub as usize])
            .filter(|&id| id != NONE)
        else {
            return;
        };
        if self.nodes[id as usize].state.seq != sequence {
            self.nodes[id as usize].state.seq = sequence;
            self.mark_changed(id);
        }
    }

    /// SequencedGearshiftBlockEntity.run detaches its output even when the next
    /// instruction has the same modifier. The input half retains its source/network.
    pub(crate) fn restart_sequence_output(&mut self, pos: [i32; 3]) {
        let Some(id) = self.at.get(&pos).map(|ids| ids[1]).filter(|&id| id != NONE) else {
            return;
        };
        let mut events = Vec::new();
        self.handle_removed(id, &mut events);
        self.remove_source(id);
        self.nodes[id as usize].state.update_speed = true;
        self.dirty.push(id);
        self.pending_events.extend(events);
    }

    /// Effects of synchronous RotationPropagator calls (including destroyBlock).
    /// Hosts apply these before publishing an accepted interaction, without ticking.
    pub fn take_pending_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.pending_events)
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len() - self.free.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn has_dirty(&self) -> bool {
        !self.legacy_loads.is_empty()
            || !self.dirty.is_empty()
            || !self.reactivation_due.is_empty()
            || !self.pending_events.is_empty()
            || !self.changed.is_empty()
            || self.networks.has_dirty()
    }

    /// One tick went by: every part's flicker score falls by one (`KineticBlockEntity.tick`).
    pub fn advance(&mut self) {
        self.now += 1;
        while let Some(due) = self.scheduler.take_reactivation(self.now) {
            self.reactivation_due.extend(due);
        }
        while let Some(due) = self.scheduler.take_due(self.now) {
            for (id, stamp) in due {
                if !self.nodes[id as usize].live || self.nodes[id as usize].stamp != stamp {
                    continue;
                }
                let mut events = Vec::new();
                self.validate_kinetics(id, &mut events);
                self.pending_events.extend(events);
                self.scheduler.validate_later(self.now, id, stamp);
            }
        }
    }

    /// KineticBlockEntity.validateKinetics, including its sourceless-generator rule.
    fn validate_kinetics(&mut self, id: u32, events: &mut Vec<Event>) {
        let source = self.nodes[id as usize].state.source;
        if source.is_some() {
            if self.nodes[id as usize].state.net.is_none() {
                self.remove_source(id);
            } else if self.resolve_source(source).is_some_and(|i| {
                !self.nodes[i as usize].live || self.nodes[i as usize].state.theoretical == 0.0
            }) {
                self.remove_source(id);
                self.handle_removed(id, events);
            }
        } else if self.nodes[id as usize].part.generated == 0.0
            && self.nodes[id as usize].state.theoretical != 0.0
        {
            self.change_speed(id, 0.0);
        }
    }

    /// Whether the part at `pos` is waiting for its first solve: a part just placed or loaded
    /// has no speed yet, and its first solve reports one. False where there is none.
    pub fn unsolved(&self, pos: [i32; 3]) -> bool {
        self.at
            .get(&pos)
            .is_some_and(|s| s[0] != NONE && self.nodes[s[0] as usize].fresh)
    }

    /// The flicker score of the part at `pos` (`getFlickerScore`).
    pub fn flicker_score(&self, pos: [i32; 3]) -> i32 {
        self.at
            .get(&pos)
            .and_then(|s| (s[0] != NONE).then(|| self.nodes[s[0] as usize].state.flicker(self.now)))
            .unwrap_or(0)
    }

    /// Adds or replaces the part at `pos` (`sub` 1 is a split shaft's second half).
    pub fn set(&mut self, pos: [i32; 3], sub: u8, part: Part) {
        self.stamp += 1;
        let slot = self.at.entry(pos).or_insert([NONE, NONE]);
        let existing = slot[sub as usize];
        if existing != NONE {
            if self.nodes[existing as usize].part == part {
                return;
            }
            if self.prepare_legacy_part_change(existing, part) {
                return;
            }
            let old = self.nodes[existing as usize].part;
            let mut events = Vec::new();
            // Scroll settings invoke updateGeneratedRotation immediately. Connection changes
            // detach the old state before installing the new state (GearshiftBlock).
            if old.generated != part.generated {
                self.nodes[existing as usize].part = part;
                self.update_generated_rotation(existing, &mut events);
            } else if matches!(part.couple, Some(Couple::Controller(_)))
                && old.couple != part.couple
            {
                // SpeedControllerBlockEntity.updateTargetRotation is the value callback,
                // not a deferred block-state connection change.
                self.nodes[existing as usize].part = part;
                self.leave_network(existing);
                self.handle_removed(existing, &mut events);
                self.remove_source(existing);
                self.nodes[existing as usize].state.update_speed = false;
                self.propagate_new_source(existing, &mut events);
            } else {
                self.handle_removed(existing, &mut events);
                self.remove_source(existing);
                self.nodes[existing as usize].part = part;
                self.nodes[existing as usize].state.update_speed = true;
            }
            self.pending_events.extend(events);
            let n = &mut self.nodes[existing as usize];
            n.stamp = self.stamp;
            n.fresh = true;
            self.scheduler.validate_later(self.now, existing, n.stamp);
            self.mark_changed(existing);
            self.dirty.push(existing);
            return;
        }
        let entity = slot
            .iter()
            .copied()
            .find(|&id| id != NONE)
            .map(|id| self.nodes[id as usize].entity)
            .unwrap_or(self.stamp);
        let node = Node {
            pos,
            entity,
            sub,
            part,
            stamp: self.stamp,
            state: KineticState::default(),
            live: true,
            fresh: true,
            report_queued: false,
            load_origin: LoadOrigin::Placement,
        };
        let id = match self.free.pop() {
            Some(id) => {
                self.nodes[id as usize] = node;
                id
            }
            None => {
                self.nodes.push(node);
                (self.nodes.len() - 1) as u32
            }
        };
        slot[sub as usize] = id;
        self.order.register(pos, None);
        self.mark_changed(id);
        self.dirty.push(id);
    }

    /// Block entity `read`: install its deserialized configuration without detach,
    /// source removal, notifications or initialization. Kinetic caches remain the
    /// values read by KineticBlockEntity.read until the entity's first tick.
    pub(crate) fn read_part(&mut self, pos: [i32; 3], sub: u8, part: Part) {
        if let Some(id) = self.at.get(&pos).map(|ids| ids[sub as usize])
            && id != NONE
        {
            self.nodes[id as usize].part = part;
        }
    }

    /// `KineticBlock.onPlace` / `updateIndirectNeighbourShapes`: retain the entity,
    /// changing its state-derived part without invoking updateGeneratedRotation.
    pub fn change_block_state(&mut self, pos: [i32; 3], sub: u8, part: Part, equivalent: bool) {
        let Some(id) = self
            .at
            .get(&pos)
            .map(|ids| ids[sub as usize])
            .filter(|id| *id != NONE)
        else {
            self.set(pos, sub, part);
            return;
        };
        let n = &mut self.nodes[id as usize];
        n.part = part;
        if !equivalent {
            // KineticBlockEntity.clearKineticInformation (does not detach or initialize).
            n.state.speed = 0.0;
            n.state.theoretical = 0.0;
            n.state.source = None;
            n.state.net = None;
            n.state.overstressed = false;
            n.state.stress = 0.0;
            n.state.capacity = 0.0;
            n.state.last_stress = 0.0;
            n.state.last_capacity = 0.0;
            n.state.update_speed = true;
        }
        n.fresh = true;
        self.mark_changed(id);
        self.dirty.push(id);
    }

    /// `ChainGearshiftBlockEntity.neighbourChanged`: defer the analog change until its tick.
    pub fn chain_signal_changed(&mut self, pos: [i32; 3], modifier: f32) {
        if let Some(ids) = self.at.get(&pos)
            && ids[0] != NONE
        {
            let id = ids[0];
            self.nodes[id as usize].state.chain_signal = Some(modifier);
            self.dirty.push(id);
        }
    }

    /// `GearshiftBlock.detachKinetics`: remove downstream rotation and attach next tick,
    /// retaining the entity's own source and network.
    pub fn detach_transmission(&mut self, pos: [i32; 3]) {
        let Some(ids) = self.at.get(&pos).copied() else {
            return;
        };
        let mut events = Vec::new();
        for id in ids.into_iter().filter(|id| *id != NONE) {
            self.handle_removed(id, &mut events);
            self.nodes[id as usize].state.update_speed = true;
            self.dirty.push(id);
        }
        self.pending_events.extend(events);
    }

    /// `KineticBlockEntity.switchToBlockState`: detach before setting a non-equivalent
    /// state; a `GeneratingKineticBlockEntity` (`generating`) reactivates on its next tick,
    /// even for an equivalent rotation.
    pub fn switch_block_state(&mut self, pos: [i32; 3], equivalent: bool, generating: bool) {
        let Some(ids) = self.at.get(&pos).copied() else {
            return;
        };
        let mut events = Vec::new();
        for id in ids.into_iter().filter(|id| *id != NONE) {
            if !equivalent {
                self.leave_network(id);
                self.handle_removed(id, &mut events);
                self.remove_source(id);
                self.nodes[id as usize].state.update_speed = true;
            }
            let n = &mut self.nodes[id as usize];
            if generating {
                n.state.reactivate_source = true;
                self.scheduler.reactivate_later(self.now, id, n.stamp);
            }
            self.dirty.push(id);
        }
        self.pending_events.extend(events);
    }

    /// Whether a part is at `pos` (`sub` 1: a split shaft's second half).
    pub fn has_part(&self, pos: [i32; 3], sub: u8) -> bool {
        self.at.get(&pos).is_some_and(|p| p[sub as usize] != NONE)
    }

    /// SmartBlockEntity.onChunkUnloaded/setRemoved: invalidate the world identity,
    /// retaining network references until calculateStress/calculateCapacity prune them.
    pub fn unload(&mut self, pos: [i32; 3]) {
        let Some(ids) = self.at.remove(&pos) else {
            return;
        };
        self.order.remove(pos);
        for id in ids.into_iter().filter(|&id| id != NONE) {
            self.networks.clear_entity_dirty(id, &self.nodes);
            self.nodes[id as usize].live = false;
            self.free.push(id);
        }
        self.removed.push(pos);
    }

    /// Removes every part at `pos`; its neighbours re-solve.
    pub fn remove(&mut self, pos: [i32; 3]) {
        let Some(ids) = self.at.remove(&pos) else {
            return;
        };
        self.order.remove(pos);
        // destroyBlock makes the position absent before block entity removal callbacks.
        // Existing propagation frames retain their old entity's speed, as Java references do.
        for id in ids.into_iter().filter(|&i| i != NONE) {
            let net = self.nodes[id as usize].state.net;
            self.networks.remove(net, id, &mut self.nodes);
            self.networks.clear_entity_dirty(id, &self.nodes);
            self.nodes[id as usize].live = false;
        }
        let mut breaks = Vec::new();
        for id in ids.into_iter().filter(|&i| i != NONE) {
            self.handle_removed(id, &mut breaks);
        }
        self.pending_events.extend(breaks);
        for p in self.far.get(&pos).into_iter().flatten() {
            if let Some(n) = self.at.get(p) {
                self.dirty.extend(n.iter().copied().filter(|&i| i != NONE));
            }
        }
        for id in ids.into_iter().filter(|&i| i != NONE) {
            let part = self.nodes[id as usize].part;
            for o in neighbour_offsets(&part) {
                if let Some(n) = self.at.get(&add(pos, o)) {
                    self.dirty.extend(n.iter().copied().filter(|&i| i != NONE));
                }
            }
            self.nodes[id as usize].live = false;
            self.free.push(id);
        }
        self.removed.push(pos);
    }

    /// The positions whose parts left since the last call (for the game to hide them).
    pub fn take_removed(&mut self) -> Vec<[i32; 3]> {
        std::mem::take(&mut self.removed)
    }

    /// Watches (or stops watching) `pos`: each solve of its network lists it in `take_touched`.
    pub fn watch(&mut self, pos: [i32; 3], on: bool) {
        if on {
            self.watched.insert(pos);
        } else {
            self.watched.remove(&pos);
        }
    }

    /// The watched positions whose network solved since the last call, moved into `out`.
    pub fn take_touched(&mut self, out: &mut Vec<[i32; 3]>) {
        out.append(&mut self.touched);
    }

    /// Every part: position, half (`sub`), shown speed and whether its network is overstressed.
    pub fn parts(&self) -> impl Iterator<Item = ([i32; 3], u8, f32, bool)> + '_ {
        self.nodes
            .iter()
            .filter(|n| n.live)
            .map(|n| (n.pos, n.sub, n.state.speed, n.state.overstressed))
    }

    /// `SequenceContext.getEffectiveValue` of the context the part at `pos` took, when it is a
    /// turn by distance (`distance`) or by angle: what it may turn before it stops
    /// (`sequencedAngleLimit`, `sequencedOffsetLimit`), scaled by its speed against its holder's.
    pub fn sequence_limit(&self, pos: [i32; 3], sub: u8, distance: bool) -> Option<f32> {
        let id = *self.at.get(&pos)?.get(sub as usize)?;
        let n = self.nodes.get(id as usize).filter(|_| id != NONE)?;
        let (c, relative) = n.state.seq?;
        (c.distance == distance).then(|| (relative * n.state.theoretical).abs())
    }

    /// KineticBlockEntity.calculateStressApplied: return base impact and cache it for
    /// write. Network admission and goggles/tooltips call this even for consumers.
    /// Like Java, the calculation alone does not mark the entity for saving.
    pub fn calculate_stress_applied(&mut self, pos: [i32; 3], sub: u8) -> Option<f32> {
        let id = *self.at.get(&pos)?.get(sub as usize)?;
        Some(self.nodes.get_mut(id as usize)?.calculate_stress_applied())
    }

    /// KineticBlockEntity.calculateAddedStressCapacity: return base capacity and cache
    /// it for write, independently of generated speed or network membership. Generator
    /// goggles/tooltips call this for externally driven generators too.
    pub fn calculate_added_stress_capacity(&mut self, pos: [i32; 3], sub: u8) -> Option<f32> {
        let id = *self.at.get(&pos)?.get(sub as usize)?;
        Some(
            self.nodes
                .get_mut(id as usize)?
                .calculate_added_stress_capacity(),
        )
    }

    /// A part's theoretical speed, its network's stress and capacity, and whether it is
    /// overstressed, as of the last solve (what a stressometer reads).
    pub fn network(&self, pos: [i32; 3], sub: u8) -> Option<(f32, f32, f32, bool)> {
        let id = *self.at.get(&pos)?.get(sub as usize)?;
        (id != NONE).then(|| {
            let n = &self.nodes[id as usize];
            (
                n.state.theoretical,
                n.state.stress,
                n.state.capacity,
                n.state.overstressed,
            )
        })
    }

    /// Create's `KineticBlockEntity.network` and `KineticNetwork.getSize` for a part.
    /// Returns the source's position-based network id and member count, including unloaded
    /// members restored from saves. Returns `None` for an absent part or one without a network.
    /// Reads the existing TorquePropagator without creating a network or copying its state.
    pub fn network_membership(&self, pos: [i32; 3], sub: u8) -> Option<(i64, u32)> {
        let id = *self.at.get(&pos)?.get(sub as usize)?;
        if id == NONE {
            return None;
        }
        let net = self.nodes[id as usize].state.net?;
        Some((net, self.networks.figures(Some(net)).2))
    }

    /// The speed a part generates (`getGeneratedSpeed`), 0 for one that does not.
    pub fn generated(&self, pos: [i32; 3], sub: u8) -> Option<f32> {
        let id = *self.at.get(&pos)?.get(sub as usize)?;
        (id != NONE).then(|| self.nodes[id as usize].part.generated)
    }

    pub fn speed(&self, pos: [i32; 3], sub: u8) -> Option<f32> {
        let id = *self.at.get(&pos)?.get(sub as usize)?;
        (id != NONE).then(|| self.nodes[id as usize].state.speed)
    }

    /// The speed the part's network turns it at, shown or not (`getTheoreticalSpeed`), and
    /// whether that network is overstressed.
    pub fn theoretical(&self, pos: [i32; 3], sub: u8) -> Option<(f32, bool)> {
        let id = *self.at.get(&pos)?.get(sub as usize)?;
        (id != NONE).then(|| {
            let n = &self.nodes[id as usize];
            (n.state.theoretical, n.state.overstressed)
        })
    }

    /// Whether a generator other than the part at `pos` (`sub`) turns its network, through
    /// shafts, cogs and couplings: what `KineticBlockEntity.hasSource` says of a generator that
    /// another source drives. A search of the network: asked only when a generator's own
    /// direction disagrees with its network's.
    pub fn driven_elsewhere(&self, pos: [i32; 3], sub: u8) -> bool {
        let Some(&start) = self.at.get(&pos).and_then(|p| p.get(sub as usize)) else {
            return false;
        };
        if start == NONE {
            return false;
        }
        self.nodes[start as usize].state.source.is_some()
    }

    /// DirectionalShaftHalvesBlockEntity.getSourceFacing.
    pub fn source_facing(&self, pos: [i32; 3]) -> Option<Dir> {
        let id = self.at.get(&pos)?[0];
        let n = self.nodes.get(id as usize)?;
        Dir::from_offset(sub(n.state.source?.pos, pos))
    }

    /// A shaft face's displayed speed, using the entity's source-relative modifier.
    pub fn face_speed(&self, pos: [i32; 3], face: Dir) -> Option<f32> {
        let id = self.at.get(&pos)?[0];
        let n = self.nodes.get(id as usize)?;
        Some(n.state.speed * n.part.rotation.modifier(self.source_facing(pos), face))
    }

    /// The nodes connected to `id`, with the ratio from it to each.
    fn edges(&self, id: u32, out: &mut Vec<(u32, f32)>) {
        out.clear();
        let n = &self.nodes[id as usize];
        let (pos, part) = (n.pos, n.part);
        for o in neighbour_offsets(&part) {
            let Some(pair) = self.at.get(&add(pos, o)) else {
                continue;
            };
            for &m in pair.iter().filter(|&&m| m != NONE) {
                let other = &self.nodes[m as usize].part;
                let mut r = ratio(&part, other, o);
                if r == 0.0 {
                    // Create checks both ways (`isConnected(a, b) || isConnected(b, a)`).
                    let back = ratio(other, &part, sub([0, 0, 0], o));
                    if back != 0.0 {
                        r = 1.0 / back;
                    }
                }
                if r != 0.0 {
                    out.push((m, r));
                }
            }
        }
        if n.sub == 0
            && let Some(far) = self.far.get(&pos)
        {
            for p in far {
                let joined = self.far.get(p).is_some_and(|b| b.contains(&pos));
                if let Some(&[m, _]) = self.at.get(p)
                    && m != NONE
                    && joined
                {
                    out.push((m, 1.0));
                }
            }
        }
    }

    /// The parts at `pos` and at each of `to` turn together (ratio 1) where both list each
    /// other; the networks of all of them solve again.
    pub fn set_far(&mut self, pos: [i32; 3], to: Vec<[i32; 3]>) {
        let mut events = Vec::new();
        if let Some(ids) = self.at.get(&pos).copied() {
            for id in ids.into_iter().filter(|&i| i != NONE) {
                self.handle_removed(id, &mut events);
            }
        }
        self.pending_events.extend(events);
        if let Some(ids) = self.at.get(&pos).copied() {
            for id in ids.into_iter().filter(|&i| i != NONE) {
                self.remove_source(id);
            }
        }
        let old = self.far.remove(&pos).unwrap_or_default();
        for p in old.iter().chain(&to).chain(std::iter::once(&pos)) {
            if let Some(ids) = self.at.get(p) {
                for &id in ids.iter().filter(|&&id| id != NONE) {
                    self.nodes[id as usize].state.update_speed = true;
                    self.dirty.push(id);
                }
            }
        }
        if !to.is_empty() {
            self.far.insert(pos, to);
        }
    }

    /// The parts `id` sets the speed of, or that set its speed (`Couple`), as couplings from the
    /// part that holds the setting; `pull` gets the parts on the other side, to solve with it.
    fn couplings(&self, id: u32, out: &mut Vec<Coupling>, pull: &mut Vec<u32>) {
        let n = &self.nodes[id as usize];
        let at =
            |p: [i32; 3], sub: usize| self.at.get(&p).map(|pair| pair[sub]).filter(|&m| m != NONE);
        match n.part.couple {
            Some(Couple::Controller(target)) => {
                if let Some(cog) = at(add(n.pos, [0, 1, 0]), 0)
                    && controls(&n.part, &self.nodes[cog as usize].part)
                {
                    out.push(Coupling {
                        from: id,
                        to: cog,
                        kind: Couple::Controller(target),
                    });
                    pull.push(cog);
                }
            }
            Some(Couple::Relay(m)) if n.sub == 1 => {
                if let Some(first) = at(n.pos, 0) {
                    out.push(Coupling {
                        from: first,
                        to: id,
                        kind: Couple::Relay(m),
                    });
                    pull.push(first);
                }
            }
            _ => {}
        }
        // The other side: the controller below a large cog, a split shaft's second half.
        if n.part.cog == Cog::Large
            && n.part.axis != Axis::Y
            && let Some(c) = at(add(n.pos, [0, -1, 0]), 0)
            && controls(&self.nodes[c as usize].part, &n.part)
        {
            pull.push(c);
        }
        if n.sub == 0
            && n.part.shafts.count_ones() == 1
            && let Some(second) = at(n.pos, 1)
            && matches!(
                self.nodes[second as usize].part.couple,
                Some(Couple::Relay(_))
            )
        {
            pull.push(second);
        }
    }

    /// KineticBlockEntity.sendData: enqueue a part once when its observable state changes.
    fn mark_changed(&mut self, id: u32) {
        let n = &mut self.nodes[id as usize];
        self.saved_dirty.insert(n.pos);
        if !n.report_queued {
            n.report_queued = true;
            self.changed.push(id);
        }
    }

    /// KineticBlockEntity.updateFromNetwork: sync figures and notify on overstress changes.
    fn update_from_network(&mut self, id: u32, capacity: f32, stress: f32, size: u32) {
        self.networks.clear_entity_dirty(id, &self.nodes);
        let n = &mut self.nodes[id as usize];
        self.saved_dirty.insert(n.pos);
        n.state.network_size = size;
        n.state.capacity = capacity;
        n.state.stress = stress;
        let over = stress > capacity;
        if over != n.state.overstressed {
            let shown = if over { 0.0 } else { n.state.theoretical };
            n.state.flicker = n.state.flicker(self.now) + flicker_gain(n.state.speed, shown);
            n.state.flicker_at = self.now;
            n.state.overstressed = over;
            n.state.speed = shown;
            if n.sub == 0 {
                self.speed_notifications.push((n.pos, shown));
            }
            self.mark_changed(id);
            self.on_speed_changed(id);
        }
    }

    /// KineticNetwork.sync calls updateFromNetwork for each member of that network.
    fn sync_network(&mut self, sync: Option<networks::NetworkSync>) {
        if let Some(sync) = sync {
            for id in sync.members {
                self.update_from_network(id, sync.capacity, sync.stress, sync.size);
            }
        }
    }

    /// KineticNetwork.remove resets the departing member's cached figures.
    fn leave_network(&mut self, id: u32) {
        let old = self.nodes[id as usize].state.net;
        // KineticNetwork.remove only calls updateFromNetwork for an enrolled member.
        let enrolled = self
            .networks
            .members(old, &self.nodes)
            .any(|member| member == id);
        self.networks.remove(old, id, &mut self.nodes);
        if enrolled {
            self.update_from_network(id, 0.0, 0.0, 0);
        }
    }

    /// KineticBlockEntity.setNetwork leaves the old network before enrolling in the new one.
    fn set_network(&mut self, id: u32, net: Option<i64>) {
        let old = self.nodes[id as usize].state.net;
        if old == net {
            return;
        }
        self.leave_network(id);
        self.nodes[id as usize].state.net = net;
        self.saved_dirty.insert(self.nodes[id as usize].pos);
        if net.is_some() {
            // KineticNetwork.add owns the initial sync and networkDirty transition.
            if self
                .networks
                .get_or_create_network_for(net)
                .add(id, &mut self.nodes)
            {
                let (stress, capacity, size) = self.networks.figures(net);
                self.update_from_network(id, capacity, stress, size);
                self.networks.mark_entity_dirty(id, &self.nodes);
            }
        }
    }

    /// KineticBlockEntity.initialize: enroll this entity only on its first tick.
    fn initialize_network(&mut self, id: u32) {
        let state = self.nodes[id as usize].state.clone();
        if let Some(net) = state.net {
            let network = self.networks.get_or_create_network_for(Some(net));
            let syncs = network.init_from_te(
                state.capacity,
                state.stress,
                state.network_size,
                &self.nodes,
            );
            for sync in syncs {
                self.sync_network(Some(sync));
            }
            let network = self.networks.get_or_create_network_for(Some(net));
            network.add_silently(id, &mut self.nodes, state.last_capacity, state.last_stress);
        }
    }

    /// A split shaft's ports share one entity; retain the former (entity, node id)
    /// tie order without allocating or sorting a per-tick list.
    fn ordered_ports(&self, pos: [i32; 3]) -> [u32; 2] {
        let mut ids = self.at.get(&pos).copied().unwrap_or([NONE; 2]);
        if ids[0] > ids[1] {
            ids.swap(0, 1);
        }
        ids
    }

    /// Deliver onSpeedChanged before a subclass continues its tick.
    pub(crate) fn take_speed_notifications(&mut self) -> Vec<([i32; 3], f32)> {
        std::mem::take(&mut self.speed_notifications)
    }

    /// SmartBlockEntity initializes new entities before the first subclass tick.
    pub fn tick_base_through(&mut self, pos: [i32; 3]) {
        let Some(ids) = self.at.get(&pos).copied() else {
            return;
        };
        if !ids
            .into_iter()
            .any(|id| id != NONE && self.nodes[id as usize].state.lifecycle.is_new())
        {
            return;
        }
        // SmartBlockEntity.tick initializes earlier registered entities before this
        // handle's HandCrankBlockEntity.tick can expire its use.
        let mut pending = self.order.walk_through(View::All, pos);
        let mut events = Vec::new();
        while let Some(pos) = pending.next(&self.order) {
            for id in self.ordered_ports(pos).into_iter().filter(|&id| id != NONE) {
                if self.nodes[id as usize].state.lifecycle.is_new() {
                    self.tick_base(id, self.now + 1, &mut events);
                }
            }
        }
        self.pending_events.extend(events);
    }

    /// KineticBlockEntity.tick on this live entity's registration turn. The
    /// world supplies the shared entity order, including non-kinetic pipes.
    pub fn tick_registered_base(&mut self, pos: [i32; 3]) {
        let mut events = Vec::new();
        for id in self.ordered_ports(pos).into_iter().filter(|&id| id != NONE) {
            self.tick_base(id, self.now + 1, &mut events);
        }
        self.pending_events.extend(events);
    }

    fn tick_base(&mut self, id: u32, tick: u64, events: &mut Vec<Event>) {
        if !self.nodes[id as usize].live
            || self.nodes[id as usize]
                .state
                .lifecycle
                .ran(self.base_pass + 1)
        {
            return;
        }
        if self.nodes[id as usize].load_origin == LoadOrigin::Legacy {
            return;
        }
        if self.nodes[id as usize].state.update_speed {
            self.nodes[id as usize].state.update_speed = false;
            // attachKinetics changes the serialized NeedsSpeedUpdate flag, even at rest.
            self.saved_dirty.insert(self.nodes[id as usize].pos);
            self.propagate_new_source(id, events);
        }
        if !self.nodes[id as usize].live {
            return;
        }
        // SmartBlockEntity.tick invokes KineticBlockEntity.initialize after
        // attachKinetics, before the generator's own initialize callback.
        if self.nodes[id as usize].state.lifecycle.is_new() {
            self.initialize_network(id);
        }
        let n = &self.nodes[id as usize];
        if n.state.lifecycle.is_new()
            && (n.load_origin == LoadOrigin::Placement
                || n.part.initialization == GeneratorInitialization::EveryLoad)
            && (n.state.source.is_none() || n.part.generated > n.state.theoretical)
        {
            self.update_generated_rotation(id, events);
        }
        // KineticBlockEntity.tick validates on its first callback: the Java
        // constructor leaves validationCountdown at zero. Reuse Lifecycle so
        // tick_base_through and solve cannot repeat initialization in this pass.
        if self.nodes[id as usize].state.lifecycle.is_new() {
            self.validate_kinetics(id, events);
        }
        if self.nodes[id as usize].state.lifecycle.is_new() {
            self.scheduler
                .validate_later(tick, id, self.nodes[id as usize].stamp);
        }
        self.nodes[id as usize].state.lifecycle = state::Lifecycle::Ticked(self.base_pass + 1);
        if self.networks.take_entity_dirty(id, &self.nodes) {
            let net = self.nodes[id as usize].state.net;
            let sync = self.networks.update_network(net, &self.nodes);
            if let Some(ref sync) = sync {
                for &member in &sync.members {
                    let n = &self.nodes[member as usize];
                    if n.sub == 0 && self.watched.contains(&n.pos) {
                        self.touched.push(n.pos);
                    }
                }
            }
            self.sync_network(sync);
        }
        // GeneratingKineticBlockEntity.tick follows the base callback, including
        // its networkDirty update, before the registration walk visits the next entity.
        if self.nodes[id as usize].state.reactivate_source {
            self.update_generated_rotation(id, events);
            self.nodes[id as usize].state.reactivate_source = false;
        }
        // ChainGearshiftBlockEntity.tick runs the base tick before analogSignalChanged.
        if let Some(modifier) = self.nodes[id as usize].state.chain_signal.take() {
            self.saved_dirty.insert(self.nodes[id as usize].pos);
            self.handle_removed(id, events);
            self.remove_source(id);
            if let Some(chain) = &mut self.nodes[id as usize].part.chain {
                chain.modifier = modifier;
            }
            self.propagate_new_source(id, events);
        }
    }

    /// BeltBlock.initBelt invokes attachKinetics immediately for each chain member.
    pub(crate) fn attach_belt(&mut self, pos: [i32; 3]) {
        if let Some(id) = self.at.get(&pos).map(|ids| ids[0])
            && id != NONE
        {
            let mut events = Vec::new();
            self.nodes[id as usize].state.update_speed = false;
            // attachKinetics changes the serialized NeedsSpeedUpdate flag, even at rest.
            self.saved_dirty.insert(self.nodes[id as usize].pos);
            self.propagate_new_source(id, &mut events);
            self.pending_events.extend(events);
        }
    }

    /// Runs queued block entity attachments and generator updates, then syncs network stress.
    pub fn solve(&mut self, events: &mut Vec<Event>) -> SolveStats {
        if !self.has_dirty() {
            self.base_pass += 1;
            return SolveStats::default();
        }
        self.migrate_saves_without_kinetic_state();
        events.append(&mut self.pending_events);
        self.dirty.clear();
        // KineticBlockEntity.tick runs for registered entities in level order. The
        // indexed walk allocates nothing, excludes later registrations and skips removals.
        // Dirtied members earlier in this order wait until their next base tick.
        let mut pending = self.order.walk(View::All);
        while let Some(pos) = pending.next(&self.order) {
            for id in self.ordered_ports(pos).into_iter().filter(|&id| id != NONE) {
                self.tick_base(id, self.now, events);
            }
        }
        // Keep only outstanding tickets, without allocating or visiting other nodes.
        // A source dirtied after tick_base_through must keep the next pass active:
        // its Lifecycle guard correctly prevents another callback in this pass.
        self.reactivation_due.retain(|&(id, stamp)| {
            let n = &self.nodes[id as usize];
            n.live && n.stamp == stamp && n.state.reactivate_source
        });
        self.dirty.clear();
        let mut stats = SolveStats::default();
        self.base_pass += 1;
        let mut touched_nets = HashSet::new();
        let mut changed = std::mem::take(&mut self.changed);
        for &id in &changed {
            let n = &mut self.nodes[id as usize];
            if !n.live || !n.report_queued {
                continue;
            }
            n.report_queued = false;
            n.fresh = false;
            events.push(Event::Speed {
                pos: n.pos,
                sub: n.sub,
                speed: n.state.speed,
                theoretical: n.state.theoretical,
                overstressed: n.state.overstressed,
            });
            if n.state.net.is_some() {
                touched_nets.insert(n.state.net);
            } else {
                stats.nodes += 1;
                // KineticBlockEntity.removeSource calls onSpeedChanged after setNetwork(null).
                // Gauges must read the stopped part even though no network can enumerate it.
                if n.sub == 0 && self.watched.contains(&n.pos) {
                    self.touched.push(n.pos);
                }
            }
        }
        changed.clear();
        self.changed = changed;
        for &net in &touched_nets {
            for id in self.networks.members(net, &self.nodes) {
                let n = &self.nodes[id as usize];
                stats.nodes += 1;
                if n.sub == 0 && self.watched.contains(&n.pos) {
                    self.touched.push(n.pos);
                }
            }
        }
        stats.components = touched_nets.len() as u32;
        events.append(&mut self.pending_events);
        stats
    }

    /// `edges`, in the order Create visits neighbours: a split shaft's other half (the same
    /// block entity in Create), the six faces, the diagonals with x fastest, then far parts.
    fn java_order(&self, id: u32) -> Vec<(u32, f32)> {
        let mut out = Vec::new();
        self.edges(id, &mut out);
        let pos = self.nodes[id as usize].pos;
        let key = |m: u32| {
            let o = sub(self.nodes[m as usize].pos, pos);
            match o {
                [0, 0, 0] => -1,
                _ => match Dir::from_offset(o) {
                    Some(d) => d as i32,
                    None if o.iter().all(|c| c.abs() <= 1) => {
                        6 + (o[2] + 1) * 9 + (o[1] + 1) * 3 + (o[0] + 1)
                    }
                    None => 100,
                },
            }
        };
        out.sort_by_key(|&(m, _)| key(m));
        out
    }
}

/// What `KineticBlockEntity.onSpeedChanged` adds to a part's flicker tally when its speed goes
/// from `before` to `after`: 5 when it starts, stops or turns round.
fn flicker_gain(before: f32, after: f32) -> i32 {
    let from_or_to_zero = (before == 0.0) != (after == 0.0);
    let swap = !from_or_to_zero && before != 0.0 && before.signum() != after.signum();
    if from_or_to_zero || swap { 5 } else { 0 }
}

/// A coupling found while solving: `from` holds the setting (the controller, a split shaft's
/// first half), `dir` says which side drives (1 `from`'s, -1 `to`'s, 0 not known yet).
#[derive(Clone, Copy, Debug)]
struct Coupling {
    from: u32,
    to: u32,
    kind: Couple,
}

/// `RotationPropagator.isLargeCogToSpeedController`: a large cog on a horizontal axis right above
/// a speed controller, across the controller's axis.
fn controls(controller: &Part, cog: &Part) -> bool {
    matches!(controller.couple, Some(Couple::Controller(_)))
        && cog.cog == Cog::Large
        && cog.axis != Axis::Y
        && cog.axis != controller.axis
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_base_turn_attaches_deferred_kinetic_entities() {
        let mut k = Kinetics::new();
        let drive = [1, 0, 0];
        let pump = [2, 0, 0];
        k.set([0, 0, 0], 0, motor(Dir::East, 64.));
        k.set(drive, 0, Part::shaft(Axis::X));
        k.set(pump, 0, Part::shaft(Axis::X));
        solve(&mut k);
        k.advance();
        // onPlace on a retained entity defers attachKinetics until its turn.
        k.change_block_state(drive, 0, Part::shaft(Axis::X), false);
        assert_eq!(k.speed(drive, 0), Some(0.));
        k.tick_registered_base(drive);
        k.tick_registered_base(pump);
        assert_eq!(k.speed(drive, 0), Some(64.));
        assert_eq!(k.speed(pump, 0), Some(64.));
    }

    #[test]
    fn source_changed_after_base_callback_waits_for_next_pass() {
        let mut k = Kinetics::new();
        let pos = [0, 0, 0];
        k.set(pos, 0, motor(Dir::East, 16.0));
        k.tick_base_through(pos);
        assert_eq!(k.speed(pos, 0), Some(16.0));
        // KineticBlockEntity.switchToBlockState schedules generator reactivation.
        // Its base callback already ran, so Java waits until the next entity tick.
        k.switch_block_state(pos, false, true);
        k.advance();
        solve(&mut k);
        assert_eq!(k.speed(pos, 0), Some(0.0));
        k.advance();
        solve(&mut k);
        assert_eq!(k.speed(pos, 0), Some(16.0));
    }

    #[test]
    fn network_dirty_membership_is_cancelled_before_slot_reuse() {
        let mut k = Kinetics::new();
        let pos = [0, 0, 0];
        k.set(pos, 0, motor(Dir::East, 16.0));
        solve(&mut k);
        let id = k.at[&pos][0];
        let net = k.nodes[id as usize].state.net;
        assert!(!k.networks.has_dirty());
        k.networks.mark_entity_dirty(id, &k.nodes);
        assert!(k.networks.has_dirty());
        let (stress, capacity, size) = k.networks.figures(net);
        k.update_from_network(id, capacity, stress, size);
        assert!(
            !k.networks.has_dirty(),
            "updateFromNetwork cancels the queued flag"
        );
        k.networks.mark_entity_dirty(id, &k.nodes);
        k.unload(pos);
        assert!(!k.networks.has_dirty(), "an unloaded entity cannot tick");
        k.set(pos, 0, motor(Dir::East, 16.0));
        assert_eq!(k.at[&pos][0], id, "exercise a recycled node slot");
        k.networks.mark_dirty(net, &mut k.nodes);
        assert!(
            !k.networks.has_dirty(),
            "the old member must not dirty its replacement"
        );
        k.networks.mark_entity_dirty(id, &k.nodes);
        k.remove(pos);
        assert!(
            !k.networks.has_dirty(),
            "removal cancels the new entity's flag too"
        );
    }

    #[test]
    fn early_reload_keeps_caches_until_a_member_syncs_changed_totals() {
        // Create Java issue #59: motor/press reloaded at tick 1, then gauge
        // and valve admitted. At tick 3 the motor still caches stress 0, size 1;
        // the gauge caches 0, size 3. Tick 4 synchronizes stress 256 and size 4.
        let mut k = Kinetics::new();
        let motor = Part {
            initialization: GeneratorInitialization::EveryLoad,
            generated: 16.0,
            capacity: 16384.0,
            ..Part::half(Dir::East)
        };
        let press = Part {
            impact: 8.0,
            ..Part::shaft(Axis::X)
        };
        k.set([0, 8, 0], 0, motor);
        solve(&mut k);
        k.set([1, 8, 0], 0, press);
        let motor = Part {
            generated: 32.0,
            ..motor
        };
        k.set([0, 8, 0], 0, motor);
        let saved = [
            k.saved_state([0, 8, 0], 0).unwrap(),
            k.saved_state([1, 8, 0], 0).unwrap(),
        ];
        for x in 0..2 {
            k.unload([x, 8, 0]);
        }
        for (x, part) in [motor, press].into_iter().enumerate() {
            k.set([x as i32, 8, 0], 0, part);
            k.restore_state([x as i32, 8, 0], 0, saved[x]);
        }
        solve(&mut k);
        k.set(
            [2, 8, 0],
            0,
            Part {
                speed_changed: SpeedChanged::StressGauge,
                ..Part::shaft(Axis::X)
            },
        );
        solve(&mut k);
        let cached = |k: &Kinetics, x| k.saved_state([x, 8, 0], 0).unwrap().network.unwrap();
        let m = cached(&k, 0);
        assert_eq!((m.stress, m.capacity, m.size), (0.0, 524288.0, 1));
        let g = cached(&k, 2);
        assert_eq!((g.stress, g.size), (0.0, 3));
        k.set([3, 8, 0], 0, Part::half(Dir::West));
        solve(&mut k);
        for x in 0..4 {
            let m = cached(&k, x);
            assert_eq!((m.stress, m.capacity, m.size), (256.0, 524288.0, 4));
        }
    }

    #[test]
    fn gearbox_keeps_incoming_speed_in_each_source_face_frame() {
        // Observed Create gearbox-*.json: entity takes the motor's 16 RPM;
        // RotationPropagator.getAxisModifier sets each other face relative to it.
        for axis in [Axis::X, Axis::Y, Axis::Z] {
            for input in Dir::ALL.into_iter().filter(|d| d.axis() != axis) {
                let mut k = Kinetics::new();
                let pos = [0, 0, 0];
                k.set(pos, 0, Part::gearbox(axis));
                k.set(
                    input.offset(),
                    0,
                    motor(
                        input.opposite(),
                        convert_to_direction(16.0, input.opposite()),
                    ),
                );
                solve(&mut k);
                assert_eq!(k.speed(pos, 0), Some(16.0));
                assert_eq!(k.saved_state(pos, 0).unwrap().speed, 16.0);
                assert_eq!(k.face_speed(pos, input), Some(16.0));
                assert_eq!(k.face_speed(pos, input.opposite()), Some(-16.0));
            }
        }
    }

    #[test]
    fn split_shaft_keeps_one_incoming_speed_from_either_end() {
        // ClutchBlockEntity/GearshiftBlockEntity.getRotationSpeedModifier:
        // powered output 0/-1, source face 1; the saved entity speed stays 16.
        for axis in [Axis::X, Axis::Y, Axis::Z] {
            for positive in [false, true] {
                for modifier in [0.0, -1.0, 1.0] {
                    let input = Dir::from_axis(axis, positive);
                    let mut k = Kinetics::new();
                    k.set([0, 0, 0], 0, Part::split_shaft(axis, modifier));
                    k.set(
                        input.offset(),
                        0,
                        motor(
                            input.opposite(),
                            convert_to_direction(16.0, input.opposite()),
                        ),
                    );
                    k.set(input.opposite().offset(), 0, Part::shaft(axis));
                    solve(&mut k);
                    assert_eq!(k.saved_state([0, 0, 0], 0).unwrap().speed, 16.0);
                    assert_eq!(k.face_speed([0, 0, 0], input), Some(16.0));
                    assert_eq!(k.speed(input.opposite().offset(), 0), Some(16.0 * modifier));
                }
            }
        }
    }

    #[test]
    fn source_membership_follows_the_immediate_driver() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        k.set([2, 0, 0], 0, Part::shaft(Axis::X));
        solve(&mut k);
        let middle = k.at[&[1, 0, 0]][0];
        let end = k.at[&[2, 0, 0]][0];
        assert_eq!(
            k.nodes[end as usize].state.source,
            Some(self_location(&k.nodes[middle as usize]))
        );
        assert_eq!(k.nodes[end as usize].state.net, Some(network_id([0, 0, 0])));
        assert_eq!(k.networks.figures(Some(network_id([0, 0, 0]))).2, 3);
        k.remove([1, 0, 0]);
        solve(&mut k);
        assert_eq!(k.nodes[end as usize].state.source, None);
        assert_eq!(k.speed([2, 0, 0], 0), Some(0.0));
        assert_eq!(k.networks.figures(Some(network_id([0, 0, 0]))).2, 1);
    }

    #[test]
    fn validation_clears_a_source_whose_block_entity_disappeared() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        solve(&mut k);
        // Simulate a source disappearing without handleRemoved (e.g. external unload).
        let root = k.at[&[0, 0, 0]][0];
        k.nodes[root as usize].live = false;
        for _ in 0..60 {
            k.advance();
        }
        assert_eq!(k.speed([1, 0, 0], 0), Some(16.0));
        k.advance();
        assert!(k.has_dirty());
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(0.0));
    }

    #[test]
    fn removing_an_overpowered_member_updates_capacity_without_reattaching_the_source() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        k.set([2, 0, 0], 0, motor(Dir::West, -8.0));
        solve(&mut k);
        k.watch([1, 0, 0], true);
        k.remove([2, 0, 0]);
        let e = solve(&mut k);
        assert!(!e.iter().any(|e| matches!(e, Event::Break { .. })));
        assert_eq!(k.network([1, 0, 0], 0).unwrap().2, 16.0 * 64.0);
        assert_eq!(k.speed([1, 0, 0], 0), Some(16.0));
        let mut touched = Vec::new();
        k.take_touched(&mut touched);
        assert_eq!(touched, [[1, 0, 0]]);
    }

    #[test]
    fn changing_one_network_leaves_unrelated_networks_and_events_alone() {
        let mut k = Kinetics::new();
        for z in [0, 10] {
            k.set([0, 0, z], 0, motor(Dir::East, 16.0));
            k.set(
                [1, 0, z],
                0,
                Part {
                    impact: 8.0,
                    ..Part::shaft(Axis::X)
                },
            );
        }
        solve(&mut k);
        let untouched = k.network([1, 0, 10], 0);
        k.watch([1, 0, 10], true);
        k.set([0, 0, 0], 0, motor(Dir::East, 32.0));
        let e = solve(&mut k);
        assert_eq!(k.network([1, 0, 10], 0), untouched);
        assert!(
            e.iter()
                .all(|e| matches!(e, Event::Speed { pos: [_, _, 0], .. }))
        );
        let mut touched = Vec::new();
        k.take_touched(&mut touched);
        assert!(touched.is_empty());
        assert!(!k.has_dirty());
    }

    #[test]
    fn leaving_an_overstressed_network_resets_only_the_departing_member() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set(
            [1, 0, 0],
            0,
            Part {
                impact: 128.0,
                ..Part::shaft(Axis::X)
            },
        );
        solve(&mut k);
        assert_eq!(k.theoretical([1, 0, 0], 0), Some((16.0, true)));
        k.remove([1, 0, 0]);
        let e = solve(&mut k);
        assert_eq!(k.theoretical([0, 0, 0], 0), Some((16.0, false)));
        assert!(e.iter().any(|e| matches!(
            e,
            Event::Speed {
                pos: [0, 0, 0],
                overstressed: false,
                ..
            }
        )));
        assert_eq!(k.network([0, 0, 0], 0).unwrap().1, 0.0);
    }

    #[test]
    fn destroyed_generator_finishes_network_updates() {
        let mut k = Kinetics::new();
        let (a, b) = ([5, 7, -3], [5, 8, -3]);
        let motor = |face, rpm| Part {
            capacity: 16384.0,
            ..motor(face, rpm)
        };
        k.set(a, 0, motor(Dir::Up, -32.0));
        solve(&mut k);
        k.set(b, 0, motor(Dir::Down, 16.0));
        solve(&mut k);
        k.set(a, 0, motor(Dir::Up, 32.0));
        k.set(b, 0, motor(Dir::Down, 32.0));
        let saved = k.saved_state(a, 0).unwrap().network.unwrap();
        assert_eq!(saved.capacity, 524288.0);
        assert_eq!(saved.size, 1);
    }

    #[test]
    fn reapplying_generator_retains_network_identity() {
        let mut k = Kinetics::new();
        let p = [5, 7, -3];
        k.set(p, 0, motor(Dir::Up, -32.0));
        solve(&mut k);
        let id = k.at[&p][0];
        // applyNewSpeed's independent reapply branch does not call setNetwork.
        k.set_network(id, Some(123));
        k.set(p, 0, motor(Dir::Up, 32.0));
        assert_eq!(k.nodes[id as usize].state.net, Some(123));
    }

    #[test]
    fn restored_member_motors_keep_sources_in_either_load_order() {
        let mut original = Kinetics::new();
        let parts = [
            motor(Dir::East, 16.0),
            Part::shaft(Axis::X),
            motor(Dir::West, 16.0),
        ];
        for (x, part) in parts.into_iter().enumerate() {
            original.set([x as i32, 0, 0], 0, part);
        }
        solve(&mut original);
        let saved = (0..3)
            .map(|x| original.saved_state([x, 0, 0], 0).unwrap())
            .collect::<Vec<_>>();
        for order in [[0, 1, 2], [2, 1, 0]] {
            let mut loaded = Kinetics::new();
            for x in order {
                loaded.set([x as i32, 0, 0], 0, parts[x]);
                assert!(loaded.restore_state([x as i32, 0, 0], 0, saved[x]));
                // A different column may not arrive until a later tick.
                let events = solve(&mut loaded);
                assert!(
                    !events.iter().any(|e| matches!(e, Event::Break { .. })),
                    "{events:?}"
                );
            }
            let events = solve(&mut loaded);
            assert!(
                !events.iter().any(|e| matches!(e, Event::Break { .. })),
                "{events:?}"
            );
            for x in 0..3 {
                assert_eq!(loaded.speed([x, 0, 0], 0), Some(16.0));
                let restored = loaded.saved_state([x, 0, 0], 0).unwrap();
                assert_eq!(restored.source, saved[x as usize].source);
                assert_eq!(restored.network, saved[x as usize].network);
            }
        }
    }

    #[test]
    fn partial_network_restore_preserves_unloaded_contributions() {
        let mut original = Kinetics::new();
        original.set([-17, 3, 18], 0, motor(Dir::East, 32.0));
        original.set(
            [-16, 3, 18],
            0,
            Part {
                impact: 8.0,
                ..Part::shaft(Axis::X)
            },
        );
        solve(&mut original);
        let saved = original.saved_state([-16, 3, 18], 0).unwrap();
        assert_eq!(saved.network.unwrap().id, network_id([-17, 3, 18]));
        let mut loaded = Kinetics::new();
        loaded.set(
            [-16, 3, 18],
            0,
            Part {
                impact: 8.0,
                ..Part::shaft(Axis::X)
            },
        );
        loaded.restore_state([-16, 3, 18], 0, saved);
        solve(&mut loaded);
        assert_eq!(
            loaded.saved_state([-16, 3, 18], 0).unwrap().network,
            saved.network
        );
        loaded.set([-17, 3, 18], 0, motor(Dir::East, 32.0));
        loaded.restore_state(
            [-17, 3, 18],
            0,
            original.saved_state([-17, 3, 18], 0).unwrap(),
        );
        solve(&mut loaded);
        assert_eq!(
            loaded.saved_state([-16, 3, 18], 0).unwrap().network,
            saved.network
        );
    }

    /// A creative motor set to `rpm`, turning the shaft on `face` (CreativeMotorBlockEntity).
    fn motor(face: Dir, rpm: f32) -> Part {
        Part {
            generated: convert_to_direction(rpm, face),
            capacity: 16384.0 / 256.0,
            ..Part::half(face)
        }
    }

    fn solve(k: &mut Kinetics) -> Vec<Event> {
        let mut e = Vec::new();
        k.solve(&mut e);
        e
    }

    #[test]
    fn scrolling_motor_overspeed_breaks_conveying_cog() {
        let mut k = Kinetics::new();
        k.set([0, 6, -1], 0, Part::cog(Axis::Y, false));
        solve(&mut k);
        k.set([1, 6, -2], 0, Part::cog(Axis::Y, true));
        solve(&mut k);
        k.set([1, 7, -2], 0, motor(Dir::Down, 16.0));
        solve(&mut k);
        k.set([1, 7, -2], 0, motor(Dir::Down, 183.0));
        let events = solve(&mut k);
        let breaks: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, Event::Break { .. }))
            .collect();
        assert_eq!(breaks, [&Event::Break { pos: [1, 6, -2] }]);
    }

    #[test]
    fn a_motor_drives_a_line_of_shafts() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 32.0));
        for x in 1..=5 {
            k.set([x, 0, 0], 0, Part::shaft(Axis::X));
        }
        solve(&mut k);
        for x in 1..=5 {
            assert_eq!(k.speed([x, 0, 0], 0), Some(32.0));
        }
        // A steady network does nothing.
        let mut e = Vec::new();
        assert_eq!(k.solve(&mut e), SolveStats::default());
        assert!(e.is_empty());
        // Removing a shaft stops what is past it.
        k.remove([3, 0, 0]);
        solve(&mut k);
        assert_eq!(k.speed([2, 0, 0], 0), Some(32.0));
        assert_eq!(k.speed([4, 0, 0], 0), Some(0.0));
    }

    #[test]
    fn new_parts_are_reported_even_when_still_and_removals_are_kept() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, Part::shaft(Axis::X));
        let e = solve(&mut k);
        assert_eq!(
            e,
            [Event::Speed {
                pos: [0, 0, 0],
                sub: 0,
                speed: 0.0,
                overstressed: false,
                theoretical: 0.0
            }]
        );
        assert!(solve(&mut k).is_empty());
        k.remove([0, 0, 0]);
        assert_eq!(k.take_removed(), [[0, 0, 0]]);
        assert!(k.take_removed().is_empty());
        assert_eq!(k.parts().count(), 0);
    }

    #[test]
    fn shafts_on_another_axis_do_not_connect() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, Part::shaft(Axis::Z));
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(0.0));
    }

    #[test]
    fn cogs_reverse_and_large_cogs_change_speed() {
        let mut k = Kinetics::new();
        k.set([0, 0, -1], 0, motor(Dir::South, 16.0));
        k.set([0, 0, 0], 0, Part::cog(Axis::Z, false));
        k.set([1, 0, 0], 0, Part::cog(Axis::Z, false));
        // A large cog diagonal to the second small one: small -> large is -0.5.
        k.set([2, 1, 0], 0, Part::cog(Axis::Z, true));
        solve(&mut k);
        assert_eq!(k.speed([0, 0, 0], 0), Some(16.0));
        assert_eq!(k.speed([1, 0, 0], 0), Some(-16.0));
        assert_eq!(k.speed([2, 1, 0], 0), Some(8.0));
    }

    #[test]
    fn large_cogs_on_crossed_axes_mesh() {
        let mut k = Kinetics::new();
        k.set([0, -1, 0], 0, motor(Dir::Up, 10.0));
        k.set([0, 0, 0], 0, Part::cog(Axis::Y, true));
        k.set([1, 1, 0], 0, Part::cog(Axis::X, true));
        solve(&mut k);
        // diff (1, 1, 0): along Y (from) +1, along X (to) +1: same sign, ratio 1.
        assert_eq!(k.speed([1, 1, 0], 0), Some(10.0));
    }

    #[test]
    fn a_gearbox_turns_the_corner() {
        let mut k = Kinetics::new();
        k.set([-1, 0, 0], 0, motor(Dir::East, 20.0));
        k.set([0, 0, 0], 0, Part::gearbox(Axis::Y));
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        k.set([0, 0, 1], 0, Part::shaft(Axis::Z));
        k.set([0, 0, -1], 0, Part::shaft(Axis::Z));
        solve(&mut k);
        // RotationPropagator.getAxisModifier with the source on the west face: east (same axis,
        // other side) -1, south (positive, unlike the west source) 1, north (negative like it) -1.
        assert_eq!(k.speed([1, 0, 0], 0), Some(-20.0));
        assert_eq!(k.speed([0, 0, 1], 0), Some(20.0));
        assert_eq!(k.speed([0, 0, -1], 0), Some(-20.0));
    }

    #[test]
    fn angular_speed_is_create_s() {
        assert_eq!(convert_to_angular(32.0), 9.6);
        assert_eq!(convert_to_direction(16.0, Dir::West), -16.0);
    }

    #[test]
    fn opposing_motors_break_the_newest() {
        // A motor placed on a network turning the other way attaches before it starts
        // (`attachKinetics` runs before `initialize`), so it turns with the network and adds
        // its capacity: Create keeps both (create-fuzz `cases/opposing-motors.json`: both at 16,
        // the network's capacity both motors' at 16 rpm).
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        k.set([2, 0, 0], 0, motor(Dir::West, 16.0));
        let e = solve(&mut k);
        assert!(!e.iter().any(|x| matches!(x, Event::Break { .. })), "{e:?}");
        assert_eq!(k.speed([2, 0, 0], 0), Some(16.0));
        assert_eq!(
            k.network([1, 0, 0], 0).map(|n| n.2),
            Some(2.0 * 16.0 * (16384.0 / 256.0))
        );
        // Two running the other way apart, joined by a shaft placed between them: the shaft
        // breaks and both motors go on.
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        k.set([3, 0, 0], 0, motor(Dir::West, 16.0));
        solve(&mut k);
        k.set([2, 0, 0], 0, Part::shaft(Axis::X));
        let e = solve(&mut k);
        assert!(e.contains(&Event::Break { pos: [2, 0, 0] }), "{e:?}");
        assert_eq!(k.speed([1, 0, 0], 0), Some(16.0));
        assert_eq!(k.speed([3, 0, 0], 0), Some(-16.0));
    }

    #[test]
    fn the_faster_motor_drives_and_too_fast_breaks() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        // Facing west, -64 is +64 along X: the same way as the first, and faster.
        k.set([2, 0, 0], 0, motor(Dir::West, -64.0));
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(64.0));
        // Two large-to-small steps from 128 RPM: 256 is allowed, 512 is not.
        let mut k = Kinetics::new();
        k.set([0, 0, -1], 0, motor(Dir::South, 128.0));
        k.set([0, 0, 0], 0, Part::cog(Axis::Z, true));
        k.set([1, 1, 0], 0, Part::cog(Axis::Z, false));
        let e = solve(&mut k);
        assert!(e.is_empty() || !e.iter().any(|x| matches!(x, Event::Break { .. })));
        assert_eq!(k.speed([1, 1, 0], 0), Some(-256.0));
        // A large cog on the small one's shaft turns at -256 and doubles again: 512 is too fast.
        k.set([1, 1, 1], 0, Part::cog(Axis::Z, true));
        k.set([2, 2, 1], 0, Part::cog(Axis::Z, false));
        let e = solve(&mut k);
        // RotationPropagator: "if (tooFast || speedChangedTooOften) { world.destroyBlock(pos, true); return; }"
        // The conveying large cog is currentTE, even though the small cog was placed later.
        assert!(e.contains(&Event::Break { pos: [1, 1, 1] }), "{e:?}");
        assert_eq!(k.speed([2, 2, 1], 0), Some(0.0));
    }

    #[test]
    fn loaded_shaft_enrolls_before_valve_expiry() {
        // Issue #50, Java tick 14: both handle and shaft cache zero capacity.
        let shaft = [0, 7, 0];
        let handle = [0, 8, 0];
        let valve = Part {
            generated: -32.0,
            capacity: 8.0,
            ..Part::half(Dir::Down)
        };
        let mut k = Kinetics::new();
        k.set(shaft, 0, Part::shaft(Axis::Y));
        k.set(handle, 0, valve);
        solve(&mut k);
        let saved = [shaft, handle].map(|p| k.saved_state(p, 0).unwrap());
        for p in [shaft, handle] {
            k.unload(p);
        }
        for ((p, part), saved) in [shaft, handle]
            .into_iter()
            .zip([Part::shaft(Axis::Y), valve])
            .zip(saved)
        {
            k.set(p, 0, part);
            assert!(k.restore_state(p, 0, saved));
        }
        k.tick_base_through(handle);
        k.set(
            handle,
            0,
            Part {
                generated: 0.0,
                ..valve
            },
        );
        solve(&mut k);
        assert_eq!(k.network(shaft, 0).unwrap().2, 0.0);
        assert_eq!(k.network(handle, 0).unwrap().2, 0.0);
    }

    #[test]
    fn loaded_motor_initializes_before_press_enrollment() {
        // Issue #38: Java completed tick 23 caches stress=0, size=1 on
        // the motor, although addSilently later makes the actual size two.
        let positions = [[-1, 8, 0], [0, 8, 0]];
        let parts = [
            Part {
                initialization: GeneratorInitialization::EveryLoad,
                capacity: 16384.0,
                ..motor(Dir::East, 32.0)
            },
            Part {
                impact: 8.0,
                ..Part::shaft(Axis::X)
            },
        ];
        let mut k = Kinetics::new();
        for (pos, part) in positions.into_iter().zip(parts) {
            k.set(pos, 0, part);
            solve(&mut k);
        }
        let saved = positions.map(|pos| k.saved_state(pos, 0).unwrap());
        for pos in positions {
            k.unload(pos);
        }
        for ((pos, part), saved) in positions.into_iter().zip(parts).zip(saved) {
            k.set(pos, 0, part);
            assert!(k.restore_state(pos, 0, saved));
        }
        solve(&mut k);
        let motor = k.saved_state(positions[0], 0).unwrap().network.unwrap();
        assert_eq!(
            (motor.stress, motor.size, motor.capacity),
            (0.0, 1, 524288.0)
        );
        assert_eq!(k.networks.figures(Some(motor.id)).2, 2);
    }

    #[test]
    fn unloaded_generation_survives_until_independent_network_pruning() {
        // Issue #34: Java crank 32 rpm, capacity 256, getSize 1 -> 2.
        let pos = [0, 8, 0];
        let part = Part {
            initialization: GeneratorInitialization::Placement,
            capacity: 8.0,
            ..motor(Dir::Up, 32.0)
        };
        let mut k = Kinetics::new();
        k.set(pos, 0, part);
        solve(&mut k);
        let saved = k.saved_state(pos, 0).unwrap();
        let net = saved.network.unwrap().id;
        assert_eq!(k.networks.figures(Some(net)), (0.0, 256.0, 1));
        k.unload(pos);
        assert_eq!(k.networks.figures(Some(net)).2, 1);
        k.set(pos, 0, part);
        k.restore_state(pos, 0, saved);
        solve(&mut k);
        assert_eq!(k.networks.figures(Some(net)), (0.0, 256.0, 2));
        assert_eq!(
            k.networks
                .get_or_create_network_for(Some(net))
                .unloaded_count(),
            0
        );
        let network = k.networks.get_or_create_network_for(Some(net));
        // calculateCapacity removes sources only, leaving both member identities.
        network.update_capacity(&k.nodes);
        assert_eq!(k.networks.figures(Some(net)).2, 2);
        k.networks
            .get_or_create_network_for(Some(net))
            .update_stress(&k.nodes);
        assert_eq!(k.networks.figures(Some(net)).2, 1);
    }

    #[test]
    fn driven_crank_calculation_is_saved_across_reload() {
        // Seed 113: Observations.kinetic calls calculateAddedStressCapacity
        // on the dormant, externally driven hand crank before Persistence.reload.
        // Java then saves AddedCapacity=8, although this entity is not a source.
        let pos = [-5, 8, 2];
        let crank = Part {
            capacity: 8.0,
            ..Part::half(Dir::Up)
        };
        let mut k = Kinetics::new();
        k.set(pos, 0, crank);
        solve(&mut k);
        k.set(
            [-5, 9, 2],
            0,
            Part {
                capacity: 16384.0,
                ..motor(Dir::Down, 32.0)
            },
        );
        solve(&mut k);
        assert_eq!(
            k.saved_state(pos, 0)
                .unwrap()
                .network
                .unwrap()
                .added_capacity,
            0.0
        );
        assert_eq!(k.calculate_added_stress_capacity(pos, 0), Some(8.0));
        let saved = k.saved_state(pos, 0).unwrap();
        assert_eq!(saved.network.unwrap().added_capacity, 8.0);
        let mut loaded = Kinetics::new();
        loaded.set(pos, 0, crank);
        assert!(loaded.restore_state(pos, 0, saved));
        assert_eq!(
            loaded
                .saved_state(pos, 0)
                .unwrap()
                .network
                .unwrap()
                .added_capacity,
            8.0
        );
    }

    #[test]
    fn consumer_stress_calculation_updates_its_saved_cache() {
        let pos = [0, 0, 0];
        let mut k = Kinetics::new();
        k.set(
            pos,
            0,
            Part {
                impact: 8.0,
                ..Part::shaft(Axis::X)
            },
        );
        let id = k.at[&pos][0];
        assert_eq!(k.nodes[id as usize].state.last_stress, 0.0);
        assert_eq!(k.calculate_stress_applied(pos, 0), Some(8.0));
        assert_eq!(k.nodes[id as usize].state.last_stress, 8.0);
        assert!(k.calculate_stress_applied([1, 0, 0], 0).is_none());
    }

    #[test]
    fn saved_network_size_is_the_last_entity_sync() {
        // Create issue #33, saved-size.min.json: the first shaft saves size 2,
        // capacity 524288, even after the third member joins. KineticNetwork
        // only syncs all members when stress or capacity changes.
        let mut k = Kinetics::new();
        let pos = [0, 8, -3];
        k.set(pos, 0, Part::shaft(Axis::X));
        solve(&mut k);
        k.set(
            [1, 8, -3],
            0,
            Part {
                capacity: 16384.0,
                ..motor(Dir::West, 32.0)
            },
        );
        solve(&mut k);
        k.set([-1, 8, -3], 0, Part::shaft(Axis::X));
        solve(&mut k);
        let saved = k.saved_state(pos, 0).unwrap().network.unwrap();
        assert_eq!(k.network_membership(pos, 0).unwrap().1, 3);
        assert_eq!(saved.size, 2);
        assert_eq!(saved.capacity, 524288.0);
        assert_eq!(saved.stress, 0.0);
        assert_eq!(saved.added_capacity, 0.0);
        assert_eq!(saved.added_stress, 0.0);
    }

    #[test]
    fn saved_contributions_are_calculation_caches() {
        // KineticBlockEntity.clearKineticInformation zeroes the contribution
        // caches. Changing block state does not calculate them again: the
        // existing network member causes KineticNetwork.add to return early.
        let mut k = Kinetics::new();
        let pos = [1, 8, -3];
        k.set(
            pos,
            0,
            Part {
                capacity: 16384.0,
                ..motor(Dir::Up, -32.0)
            },
        );
        solve(&mut k);
        k.change_block_state(
            pos,
            0,
            Part {
                capacity: 16384.0,
                ..motor(Dir::West, -32.0)
            },
            false,
        );
        solve(&mut k);
        let id = k.at[&pos][0];
        // Rejoin the retained network just as setNetwork does, without a new
        // stress calculation. Java write saves the cleared cache, not config.
        k.set_network(id, Some(network_id(pos)));
        assert_eq!(
            k.saved_state(pos, 0)
                .unwrap()
                .network
                .unwrap()
                .added_capacity,
            0.0
        );
    }

    #[test]
    fn changed_motor_reuses_membership_without_syncing_unchanged_capacity() {
        // Java issue #15: up/-32 -> west, then scroll to 32. The retained
        // entity's network still contains it after clearKineticInformation.
        let mut k = Kinetics::new();
        let pos = [1, 8, -3];
        let motor = |face, rpm| Part {
            capacity: 16384.0,
            ..motor(face, rpm)
        };
        k.set(pos, 0, motor(Dir::Up, -32.0));
        solve(&mut k);
        let id = k.at[&pos][0];
        let net = k.nodes[id as usize].state.net;
        assert_eq!(k.nodes[id as usize].state.capacity, 524288.0);
        k.change_block_state(pos, 0, motor(Dir::West, -32.0), false);
        solve(&mut k);
        // KineticNetwork.calculateStress checks entity identity, not network id.
        k.networks.mark_dirty(net, &mut k.nodes);
        solve(&mut k);
        assert_eq!(k.networks.figures(net).2, 1);
        k.set(pos, 0, motor(Dir::West, 32.0));
        solve(&mut k);
        assert_eq!(k.speed(pos, 0), Some(-32.0));
        assert_eq!(k.nodes[id as usize].state.capacity, 0.0);
        assert_eq!(k.networks.figures(net), (0.0, 524288.0, 1));
        // A real total change subsequently syncs the retained member.
        k.set(pos, 0, motor(Dir::West, 64.0));
        solve(&mut k);
        assert_eq!(k.nodes[id as usize].state.capacity, 1048576.0);
    }

    #[test]
    fn detached_member_speed_change_does_not_enroll_it_again() {
        // switchToBlockState removes membership before detachKinetics. The entity
        // still has its old network id during the removal callbacks.
        let mut k = Kinetics::new();
        k.set([0, 8, 0], 0, motor(Dir::East, 32.0));
        k.set([1, 8, 0], 0, Part::shaft(Axis::X));
        k.set([2, 8, 0], 0, Part::shaft(Axis::X));
        solve(&mut k);
        let id = k.at[&[2, 8, 0]][0];
        let net = k.nodes[id as usize].state.net;
        k.networks.remove(net, id, &mut k.nodes);
        k.change_speed(id, 0.0);
        assert_eq!(k.networks.figures(net).2, 2);
    }

    #[test]
    fn a_loop_that_disagrees_breaks_only_once_it_turns() {
        // Two small and two large cogs, each small on a large one's shaft and meshing with the
        // other large one: round the loop the speed doubles twice. In Create nothing breaks
        // while nothing turns (`propagateNewSource` skips a pair conveying no speed).
        let mut k = Kinetics::new();
        k.set([0, 7, 0], 0, Part::cog(Axis::Y, false));
        k.set([0, 6, 0], 0, Part::cog(Axis::Y, true));
        k.set([-1, 7, 1], 0, Part::cog(Axis::Y, true));
        k.set([-1, 6, 1], 0, Part::cog(Axis::Y, false));
        let e = solve(&mut k);
        assert!(!e.iter().any(|x| matches!(x, Event::Break { .. })), "{e:?}");
        // A motor under it: now the loop turns, and a part on it breaks.
        k.set([0, 5, 0], 0, motor(Dir::Up, 16.0));
        let e = solve(&mut k);
        assert!(e.iter().any(|x| matches!(x, Event::Break { .. })), "{e:?}");
    }

    #[test]
    fn too_fast_breaks_the_part_that_conveys_it() {
        // A small cog, a large cog meshing with it, then a motor on the large cog: the small one
        // would turn at 366. Create's propagation runs from the motor and breaks the large cog,
        // which conveys the speed (`propagateNewSource` destroys its current block).
        let mut k = Kinetics::new();
        k.set([0, 6, -1], 0, Part::cog(Axis::Y, false));
        solve(&mut k);
        k.set([1, 6, -2], 0, Part::cog(Axis::Y, true));
        solve(&mut k);
        k.set([1, 7, -2], 0, motor(Dir::Down, 183.0));
        let e = solve(&mut k);
        let broken: Vec<_> = e
            .iter()
            .filter(|x| matches!(x, Event::Break { .. }))
            .collect();
        assert_eq!(broken, [&Event::Break { pos: [1, 6, -2] }]);
        // Placed onto a large cog that already turns, the small cog breaks itself.
        let mut k = Kinetics::new();
        k.set([1, 7, -2], 0, motor(Dir::Down, 183.0));
        k.set([1, 6, -2], 0, Part::cog(Axis::Y, true));
        solve(&mut k);
        k.set([0, 6, -1], 0, Part::cog(Axis::Y, false));
        let e = solve(&mut k);
        let broken: Vec<_> = e
            .iter()
            .filter(|x| matches!(x, Event::Break { .. }))
            .collect();
        assert_eq!(broken, [&Event::Break { pos: [0, 6, -1] }]);
    }

    #[test]
    fn a_break_in_a_loop_can_break_the_part_that_reached_it_too() {
        // Three large cogs meshing at right angles, a loop that reverses once. Create's
        // propagation from the motor goes z -> y -> x, where the loop disagrees, and breaks x;
        // z's call still lists x, at its last speed, disagrees with it again and breaks z too;
        // y is left with nothing turning it.
        let mut k = Kinetics::new();
        k.set([1, 8, -1], 0, Part::cog(Axis::Z, true));
        solve(&mut k);
        k.set([1, 7, 0], 0, Part::cog(Axis::Y, true));
        solve(&mut k);
        k.set([2, 8, 0], 0, Part::cog(Axis::X, true));
        solve(&mut k);
        k.set([1, 8, -2], 0, motor(Dir::South, -47.0));
        let e = solve(&mut k);
        let broken: Vec<_> = e
            .iter()
            .filter(|x| matches!(x, Event::Break { .. }))
            .collect();
        assert_eq!(
            broken,
            [
                &Event::Break { pos: [2, 8, 0] },
                &Event::Break { pos: [1, 8, -1] }
            ]
        );
        assert_eq!(k.speed([1, 7, 0], 0), Some(0.0));
    }

    #[test]
    fn a_new_motor_against_a_network_sped_up_the_other_way_breaks() {
        // A motor sped up to 256 (its scroll acts at once) while a second is placed facing it:
        // the new one attaches at -256 and, starting its own way (16 > -256), meets a network
        // no slower the other way and breaks (`applyNewSpeed` with a source). The first runs on.
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::North, 16.0));
        solve(&mut k);
        k.set([0, 0, -1], 0, motor(Dir::South, 16.0));
        k.set([0, 0, 0], 0, motor(Dir::North, 256.0));
        let e = solve(&mut k);
        let broken: Vec<_> = e
            .iter()
            .filter(|x| matches!(x, Event::Break { .. }))
            .collect();
        assert_eq!(broken, [&Event::Break { pos: [0, 0, -1] }]);
        assert_eq!(k.speed([0, 0, 0], 0), Some(-256.0));
    }

    #[test]
    fn newly_initialized_motors_use_the_signed_speed_rule() {
        // Fresh entities initialized in opposite orders exercise the signed motor rule.
        for flip in [false, true] {
            let mut k = Kinetics::new();
            let parts = [
                ([0, 0, 0], motor(Dir::East, 16.0)),
                ([1, 0, 0], Part::shaft(Axis::X)),
                ([2, 0, 0], motor(Dir::West, 16.0)),
            ];
            let order: Vec<_> = if flip {
                parts.iter().rev().collect()
            } else {
                parts.iter().collect()
            };
            for (p, part) in order {
                k.set(*p, 0, *part);
            }
            let e = solve(&mut k);
            // CreativeMotorBlockEntity.initialize: "if (!hasSource() || getGeneratedSpeed() > getTheoreticalSpeed())"
            // compares signed speeds. No saved sources are supplied by this test's set calls.
            assert_eq!(e.contains(&Event::Break { pos: [0, 0, 0] }), flip, "{e:?}");
            assert_eq!(k.speed([1, 0, 0], 0), Some(if flip { -16.0 } else { 16.0 }));
        }
    }

    #[test]
    fn a_loop_of_four_cogs_agrees() {
        // Four small cogs in a square each reverse the next: the loop agrees, nothing breaks.
        let mut k = Kinetics::new();
        k.set([0, 0, -1], 0, motor(Dir::South, 8.0));
        for p in [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0]] {
            k.set(p, 0, Part::cog(Axis::Z, false));
        }
        let e = solve(&mut k);
        assert!(!e.iter().any(|x| matches!(x, Event::Break { .. })), "{e:?}");
        assert_eq!(k.speed([1, 1, 0], 0), Some(8.0));
    }

    #[test]
    fn overstress_stops_the_network() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0)); // capacity 64 SU/RPM * 16 = 1024 SU
        let heavy = Part {
            impact: 32.0,
            ..Part::shaft(Axis::X)
        };
        k.set([1, 0, 0], 0, heavy); // 32 * 16 = 512 SU
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(16.0));
        k.set([2, 0, 0], 0, heavy);
        k.set([3, 0, 0], 0, heavy); // 1536 SU > 1024 SU
        let e = solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(0.0));
        assert!(e.iter().any(|x| matches!(
            x,
            Event::Speed {
                overstressed: true,
                ..
            }
        )));
    }

    #[test]
    fn split_shaft_is_one_network_entity() {
        // Create Java, issue #12's seed 4675: motor + unpowered clutch,
        // 32 rpm, network size 2 at both positions (not 3 shaft ports).
        // Exercise either input face and GearshiftBlockEntity's -1 modifier too.
        for input in [Dir::Up, Dir::Down] {
            for modifier in [1.0, -1.0] {
                let mut k = Kinetics::new();
                let pos = [-5, 8, 2];
                let motor_pos = add(pos, input.offset());
                k.set(pos, 0, Part::split_shaft(Axis::Y, modifier));
                solve(&mut k);
                k.set(motor_pos, 0, motor(input.opposite(), 32.0));
                solve(&mut k);
                let expected = Some((network_id(motor_pos), 2));
                assert_eq!(k.network_membership(motor_pos, 0), expected);
                assert_eq!(k.network_membership(pos, 0), expected);
                assert_eq!(k.saved_state(pos, 0).unwrap().network.unwrap().size, 2);
                let mut restored = Kinetics::new();
                restored.set(pos, 0, Part::split_shaft(Axis::Y, modifier));
                restored.set(motor_pos, 0, motor(input.opposite(), 32.0));
                assert!(restored.restore_state(pos, 0, k.saved_state(pos, 0).unwrap()));
                assert!(restored.restore_state(motor_pos, 0, k.saved_state(motor_pos, 0).unwrap()));
                solve(&mut restored);
                assert_eq!(restored.network_membership(pos, 0), expected);
                assert_eq!(restored.network_membership(motor_pos, 0), expected);
                k.remove(pos);
                solve(&mut k);
                assert_eq!(
                    k.network_membership(motor_pos, 0),
                    Some((network_id(motor_pos), 1))
                );
            }
        }
    }

    #[test]
    fn split_shafts_cut_or_reverse() {
        let mut k = Kinetics::new();
        k.set([-1, 0, 0], 0, motor(Dir::East, 12.0));
        k.set([0, 0, 0], 0, Part::split_shaft(Axis::X, -1.0));
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        solve(&mut k);
        assert_eq!(
            k.speed([1, 0, 0], 0),
            Some(-12.0),
            "a powered gearshift reverses"
        );
        k.set([0, 0, 0], 0, Part::split_shaft(Axis::X, 0.0));
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(0.0), "a powered clutch cuts");
        assert_eq!(k.speed([0, 0, 0], 0), Some(12.0));
    }

    #[test]
    fn a_clutch_on_a_fast_clock_breaks_what_it_drives() {
        // A clutch toggled every tick: the shaft it drives starts and stops each tick, 5 more
        // each time and one less each tick, until speed reaching it past 128 breaks it.
        let clutch = |k: &mut Kinetics, on: bool| {
            k.set(
                [0, 0, 0],
                0,
                Part::split_shaft(Axis::X, if on { 0.0 } else { 1.0 }),
            );
        };
        let mut k = Kinetics::new();
        k.set([-1, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([0, 0, 0], 0, Part::half(Dir::West));
        clutch(&mut k, false);
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        k.advance();
        solve(&mut k);
        let mut broke = None;
        for t in 1..200 {
            k.advance();
            clutch(&mut k, t % 2 == 1);
            let e = solve(&mut k);
            if e.contains(&Event::Break { pos: [1, 0, 0] }) {
                broke = Some(t);
                break;
            }
        }
        // 4 a tick net (5 a change, 1 a tick): past 128 at the start on tick 32.
        let t = broke.expect("the driven shaft breaks");
        assert!((30..=34).contains(&t), "broke at {t}");
        assert_eq!(k.speed([0, 0, 0], 0), Some(16.0), "the clutch stays");
        // Toggled every 20 ticks it never breaks: the tally falls back to 0 between.
        let mut k = Kinetics::new();
        k.set([-1, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([0, 0, 0], 0, Part::half(Dir::West));
        clutch(&mut k, false);
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        for t in 0..2000 {
            k.advance();
            if t % 20 == 0 {
                clutch(&mut k, t % 40 == 0);
            }
            let e = solve(&mut k);
            assert!(!e.iter().any(|x| matches!(x, Event::Break { .. })));
        }
        assert!(k.flicker_score([1, 0, 0]) <= 5);
    }

    #[test]
    fn only_the_changed_component_is_solved() {
        let mut k = Kinetics::new();
        for n in 0..3 {
            let z = n * 10;
            k.set([0, 0, z], 0, motor(Dir::East, 16.0));
            for x in 1..=100 {
                k.set([x, 0, z], 0, Part::shaft(Axis::X));
            }
        }
        k.solve(&mut Vec::new());
        k.set([101, 0, 10], 0, Part::shaft(Axis::X));
        let stats = k.solve(&mut Vec::new());
        assert_eq!(
            stats,
            SolveStats {
                components: 1,
                nodes: 102
            }
        );
    }

    /// A chain drive on the x axis whose chain runs along z.
    fn drive(positive: bool, negative: bool, modifier: f32) -> Part {
        Part {
            chain: Some(Chain {
                along: Axis::Z,
                positive,
                negative,
                modifier,
            }),
            ..Part::shaft(Axis::X)
        }
    }

    #[test]
    fn chain_conflict_breaks_the_first_gearshift() {
        // Issue 55: Java checkpoint 43 is air at the motor-driven gearshift,
        // while the far gearshift survives. Placement shapes update the previous
        // drive after the new block's onPlace, retaining its kinetic entity.
        let mut k = Kinetics::new();
        k.set([-1, 0, 0], 0, motor(Dir::East, 32.0));
        solve(&mut k);
        for z in 0..5 {
            k.set([0, 0, z], 0, drive(false, z != 0, 1.0));
            if z > 0 {
                k.change_block_state([0, 0, z - 1], 0, drive(true, z > 1, 1.0), true);
            }
            solve(&mut k);
        }
        k.chain_signal_changed([0, 0, 0], 1.1875); // signal 2
        k.chain_signal_changed([0, 0, 4], 1.875); // signal 13
        assert!(
            !solve(&mut k)
                .iter()
                .any(|e| matches!(e, Event::Break { .. }))
        );
        // The case reloads at checkpoint 39, resetting the runtime flicker tally.
        let records: Vec<_> = k
            .nodes
            .iter()
            .filter(|n| n.live)
            .map(|n| (n.pos, n.part, k.saved_state(n.pos, 0).unwrap()))
            .collect();
        k = Kinetics::new();
        for (pos, part, saved) in records {
            k.set(pos, 0, part);
            k.restore_state(pos, 0, saved);
        }
        solve(&mut k);
        k.chain_signal_changed([0, 0, 0], 1.25); // signal 3, tick 42
        k.chain_signal_changed([0, 0, 4], 1.8125); // signal 12
        let events = solve(&mut k);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Break { pos: [0, 0, 0] }))
        );
        assert!(k.speed([0, 0, 4], 0).is_some());
    }

    #[test]
    fn chain_drives_carry_their_speed_times_the_gearshift_s_modifier() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, drive(true, false, 1.0));
        k.set([1, 0, 1], 0, drive(true, true, 1.0));
        k.set([1, 0, 2], 0, drive(false, true, 1.0));
        k.set([2, 0, 2], 0, Part::shaft(Axis::X));
        // Off the chain's line: a drive beside it on x does not join.
        k.set([1, 1, 0], 0, drive(true, true, 1.0));
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 2], 0), Some(16.0));
        assert_eq!(k.speed([2, 0, 2], 0), Some(16.0));
        assert_eq!(k.speed([1, 1, 0], 0), Some(0.0));
        // An adjustable chain gearshift at 2 (signal 15) as the first: the chain turns twice as
        // fast (from 2 to 1), its own shaft as the motor.
        k.set([1, 0, 0], 0, drive(true, false, 2.0));
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(16.0));
        assert_eq!(k.speed([1, 0, 1], 0), Some(32.0));
        assert_eq!(k.speed([2, 0, 2], 0), Some(32.0));
        // Neither going on toward the other (an end, then a start): they do not connect. One
        // toward the other is enough (Create checks both ways).
        k.set([1, 0, 1], 0, drive(false, true, 1.0));
        k.set([1, 0, 2], 0, drive(true, false, 1.0));
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 2], 0), Some(0.0));
    }

    fn controller(target: f32) -> Part {
        Part {
            couple: Some(Couple::Controller(target)),
            ..Part::shaft(Axis::X)
        }
    }

    #[test]
    fn a_speed_controller_sets_the_large_cog_above_it() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, controller(64.0));
        k.set([1, 1, 0], 0, Part::cog(Axis::Z, true));
        k.set([2, 2, 0], 0, Part::cog(Axis::Z, false));
        solve(&mut k);
        assert_eq!(k.speed([1, 0, 0], 0), Some(16.0));
        assert_eq!(k.speed([1, 1, 0], 0), Some(64.0));
        assert_eq!(k.speed([2, 2, 0], 0), Some(-128.0));
        // A cog on the controller's own axis is not set.
        let mut k2 = Kinetics::new();
        k2.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k2.set([1, 0, 0], 0, controller(64.0));
        k2.set([1, 1, 0], 0, Part::cog(Axis::X, true));
        solve(&mut k2);
        assert_eq!(k2.speed([1, 1, 0], 0), Some(0.0));
        // The controller not turning: nothing turns (getDesiredOutputSpeed: no source, 0).
        k.remove([0, 0, 0]);
        solve(&mut k);
        assert_eq!(k.speed([1, 1, 0], 0), Some(0.0));
        // Target 0: the cog stays still.
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.remove([1, 0, 0]);
        k.set([1, 0, 0], 0, controller(0.0));
        solve(&mut k);
        assert_eq!(k.speed([1, 1, 0], 0), Some(0.0));
    }

    #[test]
    fn a_turning_cog_drives_a_still_controller_at_its_target() {
        let mut k = Kinetics::new();
        k.set([1, 0, 0], 0, controller(-48.0));
        k.set([2, 0, 0], 0, Part::shaft(Axis::X));
        k.set([1, 1, 0], 0, Part::cog(Axis::Z, true));
        k.set([1, 1, -1], 0, motor(Dir::South, 16.0));
        solve(&mut k);
        assert_eq!(k.speed([1, 1, 0], 0), Some(16.0));
        assert_eq!(k.speed([2, 0, 0], 0), Some(-48.0));
    }

    #[test]
    fn a_controlled_cog_shares_its_controller_s_stress() {
        let mut k = Kinetics::new();
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0)); // 1024 SU
        k.set([1, 0, 0], 0, controller(128.0));
        k.set(
            [1, 1, 0],
            0,
            Part {
                impact: 8.0, // 8 * 128 = 1024 SU: at capacity
                ..Part::cog(Axis::Z, true)
            },
        );
        solve(&mut k);
        assert_eq!(k.speed([1, 1, 0], 0), Some(128.0));
        k.set(
            [1, 1, 1],
            0,
            Part {
                impact: 1.0,
                ..Part::shaft(Axis::Z)
            },
        );
        solve(&mut k);
        assert_eq!(k.speed([1, 1, 0], 0), Some(0.0), "overstressed");
        assert_eq!(
            k.theoretical([1, 1, 0], 0),
            Some((128.0, true)),
            "still turned at its target in theory"
        );
        assert_eq!(
            k.speed([1, 0, 0], 0),
            Some(0.0),
            "the controller's side too"
        );
    }

    fn sequencer(k: &mut Kinetics, modifier: f32) {
        k.set([0, 0, 0], 0, Part::half(Dir::West));
        k.set(
            [0, 0, 0],
            1,
            Part {
                couple: Some(Couple::Relay(modifier)),
                ..Part::half(Dir::East)
            },
        );
    }

    #[test]
    fn a_relay_turns_the_far_half_from_whichever_side_turns() {
        for (m, expect) in [(2.0, 24.0), (-1.0, -12.0), (-2.0, -24.0), (0.0, 0.0)] {
            // Turned from the west.
            let mut k = Kinetics::new();
            k.set([-1, 0, 0], 0, motor(Dir::East, 12.0));
            sequencer(&mut k, m);
            k.set([1, 0, 0], 0, Part::shaft(Axis::X));
            solve(&mut k);
            assert_eq!(k.speed([1, 0, 0], 0), Some(expect), "west, {m}");
            assert_eq!(k.speed([0, 0, 0], 0), Some(12.0));
            // Turned from the east: the west half turns at the modifier times it too.
            let mut k = Kinetics::new();
            k.set([1, 0, 0], 0, motor(Dir::West, -12.0));
            sequencer(&mut k, m);
            k.set([-1, 0, 0], 0, Part::shaft(Axis::X));
            solve(&mut k);
            assert_eq!(k.speed([-1, 0, 0], 0), Some(expect), "east, {m}");
        }
        // Past 256 RPM breaks it.
        let mut k = Kinetics::new();
        k.set([-1, 0, 0], 0, motor(Dir::East, 200.0));
        sequencer(&mut k, 2.0);
        let e = solve(&mut k);
        assert!(e.iter().any(|x| matches!(x, Event::Break { .. })), "{e:?}");
    }

    #[test]
    fn a_sequence_context_reaches_what_the_relay_or_valve_turns_scaled_by_its_speed() {
        let ctx = |distance, value| Context {
            distance,
            value,
            run: 1,
        };
        // A gearshift turned from the west running "turn 90 degrees" at double speed: the far
        // half's side takes it; a large cog geared down from it turns half as far.
        let mut k = Kinetics::new();
        k.set([-1, 0, 0], 0, motor(Dir::East, 12.0));
        k.set([0, 0, 0], 0, Part::half(Dir::West));
        k.set(
            [0, 0, 0],
            1,
            Part {
                couple: Some(Couple::Relay(2.0)),
                context: SequenceContextSource::Owned,
                ..Part::half(Dir::East)
            },
        );
        k.set_sequence_context([0, 0, 0], 1, Some((ctx(false, 90.0), 90.0 / 24.0)));
        k.set([1, 0, 0], 0, Part::cog(Axis::X, false));
        k.set([1, 1, 1], 0, Part::cog(Axis::X, true));
        solve(&mut k);
        assert_eq!(k.speed([1, 1, 1], 0), Some(-12.0));
        assert_eq!(k.sequence_limit([1, 0, 0], 0, false), Some(90.0));
        assert_eq!(k.sequence_limit([1, 1, 1], 0, false), Some(45.0));
        assert_eq!(k.sequence_limit([1, 0, 0], 0, true), None, "not a distance");
        assert_eq!(
            k.sequence_limit([-1, 0, 0], 0, false),
            None,
            "the side turning it"
        );
        // A new run of the same turn tells the parts again, unchanged speeds and all.
        k.set(
            [0, 0, 0],
            1,
            Part {
                couple: Some(Couple::Relay(2.0)),
                context: SequenceContextSource::Owned,
                ..Part::half(Dir::East)
            },
        );
        k.detach_transmission([0, 0, 0]);
        k.set_sequence_context(
            [0, 0, 0],
            1,
            Some((
                Context {
                    run: 2,
                    ..ctx(false, 90.0)
                },
                90.0 / 24.0,
            )),
        );
        let e = solve(&mut k);
        assert!(
            e.iter()
                .any(|x| matches!(x, Event::Speed { pos: [1, 1, 1], .. })),
            "{e:?}"
        );
        // A valve handle in use: its whole side, by its own speed.
        let mut k = Kinetics::new();
        k.set(
            [0, 0, 0],
            0,
            Part {
                context: SequenceContextSource::Owned,
                ..motor(Dir::East, 32.0)
            },
        );
        k.set_sequence_context([0, 0, 0], 0, Some((ctx(false, 45.0), 45.0 / 32.0)));
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        solve(&mut k);
        assert_eq!(k.sequence_limit([1, 0, 0], 0, false), Some(45.0));
    }

    #[test]
    fn an_idle_context_owner_does_not_copy_its_sources_sequence() {
        // ValveHandleBlockEntity and SequencedGearshiftBlockEntity override
        // copySequenceContextFrom with an empty method, even when their own context is null.
        let mut k = Kinetics::new();
        k.set(
            [-1, 0, 0],
            0,
            Part {
                context: SequenceContextSource::Owned,
                ..motor(Dir::East, 32.0)
            },
        );
        k.set_sequence_context(
            [-1, 0, 0],
            0,
            Some((
                Context {
                    distance: false,
                    value: 90.0,
                    run: 1,
                },
                90.0 / 32.0,
            )),
        );
        k.set(
            [0, 0, 0],
            0,
            Part {
                context: SequenceContextSource::Owned,
                ..Part::shaft(Axis::X)
            },
        );
        solve(&mut k);
        assert_eq!(k.speed([0, 0, 0], 0), Some(32.0));
        assert_eq!(k.sequence_limit([0, 0, 0], 0, false), None);
        assert_eq!(k.saved_state([0, 0, 0], 0).unwrap().sequence, None);
    }

    #[test]
    fn a_controller_feedback_loop_follows_ordered_network_checks() {
        // Motor -> controller -> small cog -> large cog -> controlled cog.
        let build = |target: f32| {
            let mut k = Kinetics::new();
            k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
            k.set([1, 0, 0], 0, controller(target));
            k.set([2, 0, 0], 0, Part::cog(Axis::X, false));
            k.set([2, 1, 1], 0, Part::cog(Axis::X, true));
            k.set([1, 1, 0], 0, Part::cog(Axis::Z, true));
            let e = solve(&mut k);
            (k, e)
        };
        // RotationPropagator: "if (!currentTE.hasNetwork() || currentTE.network.equals(neighbourTE.network))"
        // checks the source network during the walk, rather than solving the loop globally.
        for target in [-8.0, 0.0, 4.0, 8.0, 64.0, -4.0] {
            let (k, e) = build(target);
            // SpeedControllerBlockEntity.getConveyedSpeed: "return Math.max(desiredOutputSpeed, compareSpeed);"
            // and getDesiredOutputSpeed: "if (targetSpeed == 0) return 0;".
            // An agreeing loop survives; others break during propagation, before a global solution.
            assert_eq!(
                e.iter().any(|e| matches!(e, Event::Break { .. })),
                target != -8.0,
                "{target}: {e:?}"
            );
            if target == -8.0 {
                assert_eq!(k.speed([1, 1, 0], 0), Some(-8.0));
            }
            if target == 0.0 {
                assert!(e.contains(&Event::Break { pos: [2, 1, 1] }));
            }
        }
    }

    #[test]
    fn a_relay_whose_halves_a_loop_joins_breaks_unless_it_agrees() {
        // The far half reached around the sequencer by cogs, at the near half's speed.
        for (m, breaks) in [(1.0, false), (2.0, true), (-1.0, true), (0.0, false)] {
            let mut k = Kinetics::new();
            k.set([-2, 0, 0], 0, motor(Dir::East, 12.0));
            for p in [[-1, 0, 0], [-1, 1, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]] {
                k.set(p, 0, Part::cog(Axis::X, false));
            }
            sequencer(&mut k, m);
            let e = solve(&mut k);
            assert_eq!(
                e.iter().any(|x| matches!(x, Event::Break { .. })),
                breaks,
                "{m}: {e:?}"
            );
        }
    }

    #[test]
    fn membership_reads_the_existing_network_and_its_size() {
        let mut k = Kinetics::new();
        assert_eq!(k.network_membership([0, 0, 0], 0), None);
        k.set([0, 0, 0], 0, motor(Dir::East, 16.0));
        k.set([1, 0, 0], 0, Part::shaft(Axis::X));
        k.set([0, 0, 10], 0, motor(Dir::East, 16.0));
        k.solve(&mut Vec::new());
        assert_eq!(
            k.network_membership([0, 0, 0], 0),
            Some((network_id([0, 0, 0]), 2))
        );
        assert_eq!(
            k.network_membership([1, 0, 0], 0),
            k.network_membership([0, 0, 0], 0)
        );
        assert_eq!(
            k.network_membership([0, 0, 10], 0),
            Some((network_id([0, 0, 10]), 1))
        );
        assert_eq!(k.network_membership([0, 0, 0], 1), None);
        k.remove([0, 0, 0]);
        k.solve(&mut Vec::new());
        assert_eq!(k.network_membership([1, 0, 0], 0), None);
    }

    #[test]
    fn only_watched_parts_of_solved_networks_are_touched() {
        let mut k = Kinetics::new();
        for z in [0, 10] {
            k.set([0, 0, z], 0, motor(Dir::East, 16.0));
            k.set([1, 0, z], 0, Part::shaft(Axis::X));
        }
        k.watch([1, 0, 0], true);
        k.solve(&mut Vec::new());
        let mut out = Vec::new();
        k.take_touched(&mut out);
        assert_eq!(out, [[1, 0, 0]]);
        // The other network solves: nothing to read.
        k.set([2, 0, 10], 0, Part::shaft(Axis::X));
        k.solve(&mut Vec::new());
        out.clear();
        k.take_touched(&mut out);
        assert!(out.is_empty());
        // Its own does, until it is no longer watched.
        k.set([2, 0, 0], 0, Part::shaft(Axis::X));
        k.solve(&mut Vec::new());
        k.take_touched(&mut out);
        assert_eq!(out, [[1, 0, 0]]);
        k.watch([1, 0, 0], false);
        k.set([3, 0, 0], 0, Part::shaft(Axis::X));
        k.solve(&mut Vec::new());
        out.clear();
        k.take_touched(&mut out);
        assert!(out.is_empty());
    }
}
