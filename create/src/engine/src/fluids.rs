//! Create's fluid pipes and mechanical pump (`FluidPipeBlock`, `PumpBlockEntity`,
//! `FluidPropagator`, `FluidNetwork`, `OpenEndedPipe`), with the glass, encased and smart pipes
//! and the fluid valve (`GlassFluidPipeBlock`, `EncasedPipeBlock`, `SmartFluidPipeBlock(Entity)`,
//! `FluidValveBlock(Entity)`): pipes connect to what can hold fluid; a turning pump pulls through
//! the pipes behind it and pushes through those in front, as far as the pump range, from a tank
//! or an open end into a tank or an open end, past open valves and the filters of smart pipes.

pub mod runtime;
pub mod transport;

use crate::kinetics::{Axis, Dir};
use crate::tank::Fluid;

/// `mechanicalPumpRange`.
pub const PUMP_RANGE: i32 = 16;

/// FluidPropagator.validateNeighbourChange ignores LiquidBlock and the pipe
/// subclasses which schedule their own propagation. FluidValveBlock and
/// EncasedPipeBlock implement pipe interfaces without extending AxisPipeBlock.
pub fn notifies_pipe_neighbours(block: Option<crate::Block>, liquid: bool) -> bool {
    !liquid
        && !matches!(
            block,
            Some(
                crate::Block::FluidPipe
                    | crate::Block::GlassFluidPipe
                    | crate::Block::SmartFluidPipe
                    | crate::Block::MechanicalPump
            )
        )
}

/// What a block is to the pipes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    /// A fluid pipe, open toward its connected sides (`Dir::bit`).
    Pipe(u8),
    /// An encased fluid pipe (`EncasedPipeBlock`): open toward its sides, which only a wrench or
    /// its casing change; no pipe joins it unless that side is open.
    Encased(u8),
    /// A pipe open along one axis (`IAxisPipe`: a glass pipe, a fluid valve, a smart pipe), and
    /// what it lets through (`canPullFluidFrom`).
    Axis(Axis, Gate),
    /// A pump, its facing.
    Pump(Dir),
    /// A fluid handler (a tank).
    Tank,
    /// A fluid handler reached from one side only (a hose pulley, `hasPipeTowards`): the side
    /// of it a pipe joins.
    Port(Dir),
    /// Open space a pipe can end at (`isOpenEnd`: replaceable, a fluid, waterloggable).
    Open,
    /// `VanillaFluidTargets`: cauldrons and states with `LEVEL_HONEY`.
    /// Connectable during placement, and an open endpoint during transport.
    VanillaTarget,
    /// Anything else.
    Solid,
}

/// What an axis pipe lets through (`canPullFluidFrom`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Anything (a glass pipe, a smart pipe without a filter, an open valve).
    Open,
    /// Nothing (a closed valve: `ENABLED` false).
    Closed,
    /// Only this fluid (a smart pipe's filter, `FilteringBehaviour.forFluids`).
    Only(Fluid),
}

/// `FluidPipeBlock.canConnectTo` (with `canHaveFlowToward` for what has a pipe behaviour):
/// toward a pipe, a tank, a pump or an axis pipe along its axis, an encased pipe open that way,
/// a one-sided handler's side.
pub fn connects(neighbour: Cell, toward: Dir) -> bool {
    match neighbour {
        Cell::Pipe(_) | Cell::Tank | Cell::VanillaTarget => true,
        Cell::Encased(sides) => sides & toward.opposite().bit() != 0,
        Cell::Axis(a, _) => a == toward.axis(),
        Cell::Port(side) => side == toward.opposite(),
        Cell::Pump(f) => f.axis() == toward.axis(),
        _ => false,
    }
}

/// `VanillaFluidTargets.canProvideFluidWithoutCapability`, independent of contents.
pub fn vanilla_target(block: &str, has_honey_level: bool) -> bool {
    has_honey_level
        || matches!(
            block,
            "minecraft:cauldron" | "minecraft:water_cauldron" | "minecraft:lava_cauldron"
        )
}

/// The sides fluid leaves a pipe-like block by (`getPipeConnections`); none for others.
pub fn openings(cell: Cell) -> u8 {
    match cell {
        Cell::Pipe(s) | Cell::Encased(s) => s,
        Cell::Axis(a, _) => Dir::from_axis(a, true).bit() | Dir::from_axis(a, false).bit(),
        _ => 0,
    }
}

/// `FluidPipeBlock.updateBlockState` (no brackets): the sides it connects, at least a straight
/// pipe along `preferred` when nothing is around and it wasn't straight before.
pub fn pipe_sides(previous: u8, neighbours: impl Fn(Dir) -> Cell, preferred: Dir) -> u8 {
    let mut sides = 0u8;
    for d in Dir::ALL {
        if connects(neighbours(d), d) {
            sides |= d.bit();
        }
    }
    match sides.count_ones() {
        0 => {
            if previous.count_ones() == 2 {
                previous
            } else {
                preferred.bit() | preferred.opposite().bit()
            }
        }
        1 => {
            let d = Dir::ALL.into_iter().find(|d| sides & d.bit() != 0).unwrap();
            sides | d.opposite().bit()
        }
        _ => sides,
    }
}

/// `FluidPropagator.getStraightPipeAxis` for a pipe's sides: the axis of a straight pipe (both
/// ends of one axis and nothing else open), none for a bend, a crossing or an end.
pub fn straight_axis(sides: u8) -> Option<Axis> {
    [Axis::X, Axis::Y, Axis::Z].into_iter().find(|&a| {
        let both = Dir::from_axis(a, true).bit() | Dir::from_axis(a, false).bit();
        sides == both
    })
}

/// The aligned pump test in `PumpBlockEntity.hasReachedValidEndpoint`.
/// `toward` points from the current pipe to the other pump; `pull` is the
/// originating pump's `isPullingOnSide(isFront(side))`.
pub fn pump_endpoint(facing: Dir, toward: Dir, pull: bool) -> bool {
    facing.axis() == toward.axis() && (facing != toward.opposite()) != pull
}

fn step(p: [i32; 3], d: Dir) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

/// PipeConnection.tickFlowProgress uses the connection's current pressure.
pub fn flow_speed(pressure: f32) -> f32 {
    1.0 / 32.0 + (pressure / 128.0).clamp(0.0, 1.0) * 31.0 / 32.0
}

/// `FluidNetwork.tick`'s share of `amount` among `outputs` ends: equal parts, the remainder one
/// each to the first, an end that takes less than its part left out of the next round
/// (`availableOutputs`), until all is placed or nobody takes more. `fill` places at an end and
/// says how much it took. What was placed in all.
pub fn share(outputs: usize, amount: u32, mut fill: impl FnMut(usize, u32) -> u32) -> u32 {
    let mut open: Vec<usize> = (0..outputs).collect();
    let mut left = amount;
    while !open.is_empty() && left > 0 {
        let (part, mut extra) = (left / open.len() as u32, left % open.len() as u32);
        let mut still = Vec::new();
        for &i in &open {
            let mut give = part;
            if extra > 0 {
                give += 1;
                extra -= 1;
            }
            if give == 0 || left == 0 {
                still.push(i);
                continue;
            }
            let took = fill(i, give.min(left));
            left -= took;
            if took >= give {
                still.push(i);
            }
        }
        // Each round places something or leaves out an end that took less than its part.
        open = still;
    }
    amount - left
}

/// `FluidNetwork.transferSpeed`: half the pressure where the fluid enters, at least 1 mB a tick.
pub fn transfer_speed(pressure: f32) -> u32 {
    ((pressure / 2.0) as u32).max(1)
}

/// What an open end holds or lets out (`OpenEndedPipe`): the fluid a source block gives (a
/// bucket's worth), placed back as a source once a bucket's worth arrived.
pub fn open_fluid(water: bool, lava: bool) -> Option<Fluid> {
    if water {
        Some(Fluid::Water)
    } else if lava {
        Some(Fluid::Lava)
    } else {
        None
    }
}

/// `FluidValveBlock.getPipeAxis`: of the two axes other than its facing's, the first, or the
/// second when `axis_along_first` (the other one turns its shaft).
pub fn valve_pipe_axis(facing: Dir, along_first: bool) -> Axis {
    let mut others = [Axis::X, Axis::Y, Axis::Z]
        .into_iter()
        .filter(|a| *a != facing.axis());
    let first = others.next().unwrap_or(Axis::X);
    let second = others.next().unwrap_or(Axis::Z);
    if along_first { second } else { first }
}

/// `SmartFluidPipeBlock.getPipeAxis`: up and down on a wall, else along its facing.
pub fn smart_pipe_axis(on_wall: bool, facing: Dir) -> Axis {
    if on_wall { Axis::Y } else { facing.axis() }
}

/// `SmartFluidPipeBlockEntity.SmartPipeFilterSlot`: where its filter's value box is (in the
/// block, `getLocalOffset`), its turn about y (`angleY`) and its tilt about x (`rotate`), in
/// degrees, for a smart pipe on `face` facing `facing`.
pub fn smart_pipe_filter_slot(face: crate::steam::Face, facing: Dir) -> ([f64; 3], f32, f32) {
    use crate::steam::Face;
    let (y, z) = match face {
        Face::Ceiling => (0.55, 4.6),
        Face::Wall => (11.4, 0.55),
        Face::Floor => (15.45, 4.625),
    };
    // `AngleHelper.horizontalAngle` (`toYRot`, negated along x), plus 180 on a wall.
    let mut angle: f32 = match facing {
        Dir::West => -90.0,
        Dir::North => 180.0,
        Dir::East => -270.0,
        _ => 0.0,
    };
    if face == Face::Wall {
        angle += 180.0;
    }
    // `VecHelper.rotateCentered(voxelSpace(8, y, z), angle, Y)`.
    // x: 8 / 16 - 0.5.
    let (x, z) = (0.0, z / 16.0 - 0.5);
    let r = f64::from(angle).to_radians();
    let (sin, cos) = r.sin_cos();
    let offset = [x * cos + z * sin + 0.5, y / 16.0, z * cos - x * sin + 0.5];
    let tilt = if face == Face::Ceiling { -45.0 } else { 45.0 };
    (offset, angle, tilt)
}

/// `ValueBoxTransform.testHit` on the smart pipe's filter slot: `local` (the hit in the block)
/// within half the box's scale (`getScale() * 1.02`) of it.
pub fn smart_pipe_filter_hit(face: crate::steam::Face, facing: Dir, local: [f64; 3]) -> bool {
    let (slot, _, _) = smart_pipe_filter_slot(face, facing);
    let d2: f64 = (0..3).map(|i| (local[i] - slot[i]).powi(2)).sum();
    d2.sqrt() < f64::from(SMART_PIPE_SLOT_SCALE) / 2.0
}

/// `SmartPipeFilterSlot.getScale`: the value box's half a block, times 1.02.
pub const SMART_PIPE_SLOT_SCALE: f32 = 0.5 * 1.02;

/// A smart pipe's filter (`FilteringBehaviour.forFluids`): the item set in its slot, and the
/// fluid it holds (`FilterItemStack.fluid`: none for an item that holds none, which passes
/// nothing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmartFilter {
    pub item: String,
    pub fluid: Option<Fluid>,
}

impl SmartFilter {
    /// What it lets through.
    pub fn gate(&self) -> Gate {
        self.fluid.map_or(Gate::Closed, Gate::Only)
    }
}

/// `SmartFluidPipeBlock.getStateForPlacement`: with pipes (or anything a pipe joins) along one
/// axis around it, a smart pipe turns to that axis: on a wall facing back for up and down
/// (`face`, `facing` as placed, turned), else on the floor (a wall becomes the floor) facing
/// away from the last of the directions nearest the view along it. None: as placed.
pub fn smart_pipe_placement(
    joins: impl Fn(Dir) -> bool,
    looking: [Dir; 6],
    face: crate::steam::Face,
    facing: Dir,
) -> Option<(crate::steam::Face, Dir)> {
    let mut preferred: Option<Axis> = None;
    for d in Dir::ALL {
        if !joins(d) {
            continue;
        }
        if preferred.is_some_and(|a| a != d.axis()) {
            preferred = None;
            break;
        }
        preferred = Some(d.axis());
    }
    let axis = preferred?;
    use crate::steam::Face;
    if axis == Axis::Y {
        return Some((Face::Wall, facing.opposite()));
    }
    let face = if face == Face::Wall {
        Face::Floor
    } else {
        face
    };
    let facing = looking
        .into_iter()
        .rfind(|d| d.axis() == axis)
        .map_or(facing, Dir::opposite);
    Some((face, facing))
}

/// `DirectionalAxisKineticBlock.getStateForPlacement` for a fluid valve: facing away from the
/// player's view (toward it sneaking); its shaft turned toward shafts above or below it or its
/// pipe toward pipes (`FluidValveBlock.prefersConnectionTo`: `shaft` and `pipe` say whether a
/// shaft or something a pipe joins is on that side). `(facing, axis_along_first)`.
pub fn valve_placement(
    nearest: Dir,
    sneaking: bool,
    horizontal: Dir,
    shaft: impl Fn(Dir) -> bool,
    pipe: impl Fn(Dir) -> bool,
) -> (Dir, bool) {
    let facing = if sneaking {
        nearest
    } else {
        nearest.opposite()
    };
    let axis = facing.axis();
    let along_first = if axis != Axis::Y {
        let positive = if axis == Axis::X {
            Dir::South
        } else {
            Dir::East
        };
        if shaft(Dir::Up) || shaft(Dir::Down) || pipe(positive) || pipe(positive.opposite()) {
            axis == Axis::X
        } else {
            axis == Axis::Z
        }
    } else {
        let mut preferred: Option<Dir> = None;
        let mut conflict = false;
        // Iterate.horizontalDirections: south, west, north, east.
        for side in [Dir::South, Dir::West, Dir::North, Dir::East] {
            if !shaft(side) && !pipe(crate::wrench::clockwise(side, Axis::Y)) {
                continue;
            }
            if preferred.is_some_and(|p| p.axis() != side.axis()) {
                conflict = true;
                break;
            }
            preferred = Some(side);
        }
        match preferred.filter(|_| !conflict) {
            Some(p) => p.axis() == Axis::X,
            None => horizontal.axis() == Axis::X,
        }
    };
    (facing, along_first)
}

/// A fluid valve's pointer (`FluidValveBlockEntity.pointer`, catnip's linear `LerpedFloat`):
/// chased toward open (1) while turning forwards, closed (0) otherwise, at `|speed| / 16 / 20`
/// a tick (none while still: it stays where it is).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pointer {
    pub value: f32,
    pub target: f32,
    pub speed: f32,
}

impl Pointer {
    /// A stationary endpoint for rendering before entity data arrives.
    /// New server entities use `Default`, following FluidValveBlockEntity's constructor.
    pub fn at(open: bool) -> Pointer {
        let v = if open { 1.0 } else { 0.0 };
        Pointer {
            value: v,
            target: v,
            speed: 0.0,
        }
    }

    /// `onSpeedChanged`: `chase(speed > 0 ? 1 : 0, getChaseSpeed(), LINEAR)`.
    pub fn speed_changed(&mut self, speed: f32) {
        self.target = if speed > 0.0 { 1.0 } else { 0.0 };
        self.speed = (speed.abs() / 16.0 / 20.0).clamp(0.0, 1.0);
    }

    /// Whether `tickChaser` would move it.
    pub fn moving(&self) -> bool {
        self.value != self.target && self.speed > 0.0
    }

    /// `tick`: the pointer moves (`tickChaser`); the block opens once it reached 1 and shuts
    /// once at 0 (the new `enabled`, when it differs from `open`).
    pub fn tick(&mut self, open: bool) -> Option<bool> {
        // Catnip LerpedFloat.tickChaser tests equality before LINEAR.chase;
        // a small residual produced by this step remains until the next tick.
        if (self.value - self.target).abs() < 1.0e-6 {
            self.value = self.target;
        } else if self.moving() {
            self.value += (self.target - self.value).clamp(-self.speed, self.speed);
        }
        if open && self.value == 0.0 {
            Some(false)
        } else if !open && self.value == 1.0 {
            Some(true)
        } else {
            None
        }
    }

    /// `FluidValveRenderer`: the pointer's turn, `lerp(value, 0, -90)` degrees.
    pub fn angle(&self) -> f32 {
        -90.0 * self.value
    }
}

/// `FluidValveRenderer`'s extra quarter turn of the pointer (`pointerRotationOffset`): 90° when
/// the pipe is horizontal and the shaft along x, or the pipe vertical.
pub fn pointer_offset(pipe: Axis, shaft: Axis) -> f32 {
    if (pipe != Axis::Y && shaft == Axis::X) || pipe == Axis::Y {
        90.0
    } else {
        0.0
    }
}

/// A block as `PipeAttachmentModel` sees it, for the pieces drawn on a pipe's sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piped {
    /// A fluid pipe and its open sides (`Dir::bit`).
    Pipe(u8),
    /// An encased pipe and its open sides.
    Encased(u8),
    /// A glass pipe, a smart pipe or a fluid valve, along an axis (`IAxisPipe`).
    Glass(Axis),
    Smart(Axis),
    Valve(Axis),
    /// A pump, its facing.
    Pump(Dir),
    /// Anything else: whether it holds fluid toward the pipe (`hasFluidCapability`), and
    /// whether it is a hose pulley.
    Other {
        fluid: bool,
        hose: bool,
    },
}

/// `FluidTransportBehaviour.AttachmentTypes`: the pieces drawn on a side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rim {
    None,
    Connection,
    DetailedConnection,
    Rim,
    PartialRim,
    Drain,
    PartialDrain,
}

/// `ComponentPartials`: the pieces of an attachment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RimPiece {
    Connection,
    RimConnector,
    Rim,
    Drain,
}

impl Rim {
    pub const ALL: [Rim; 7] = [
        Rim::None,
        Rim::Connection,
        Rim::DetailedConnection,
        Rim::Rim,
        Rim::PartialRim,
        Rim::Drain,
        Rim::PartialDrain,
    ];

    /// `AttachmentTypes.partials`.
    pub fn pieces(self) -> &'static [RimPiece] {
        match self {
            Rim::None => &[],
            Rim::Connection => &[RimPiece::Connection],
            Rim::DetailedConnection => &[RimPiece::RimConnector],
            Rim::Rim => &[RimPiece::RimConnector, RimPiece::Rim],
            Rim::PartialRim => &[RimPiece::Rim],
            Rim::Drain => &[RimPiece::RimConnector, RimPiece::Drain],
            Rim::PartialDrain => &[RimPiece::Drain],
        }
    }

    /// `withoutConnector`.
    fn without_connector(self) -> Rim {
        match self {
            Rim::Rim => Rim::PartialRim,
            Rim::Drain => Rim::PartialDrain,
            r => r,
        }
    }
}

impl Piped {
    /// `canHaveFlowToward`.
    fn flows_toward(self, d: Dir) -> bool {
        match self {
            Piped::Pipe(s) | Piped::Encased(s) => s & d.bit() != 0,
            Piped::Glass(a) | Piped::Smart(a) | Piped::Valve(a) => d.axis() == a,
            Piped::Pump(f) => f.axis() == d.axis(),
            Piped::Other { .. } => false,
        }
    }

    /// `IAxisPipe.getAxisOf`.
    fn axis(self) -> Option<Axis> {
        match self {
            Piped::Glass(a) | Piped::Smart(a) | Piped::Valve(a) => Some(a),
            _ => None,
        }
    }

    /// Whether a block draws pieces on its sides (`PipeAttachmentModel`).
    pub fn draws_rims(self) -> bool {
        !matches!(self, Piped::Other { .. })
    }
}

/// `getRenderedRimAttachment` of block `own` on its side `d`, with `n` the block beside it
/// there (`FluidTransportBehaviour`, overridden by `FluidPipeBlockEntity`,
/// `StraightPipeBlockEntity` and `PumpBlockEntity`).
pub fn rim(own: Piped, d: Dir, n: Piped) -> Rim {
    let base = if !own.flows_toward(d) {
        Rim::None
    } else {
        match n {
            Piped::Pump(f) if f == d.opposite() => Rim::None,
            Piped::Encased(s) if s & d.opposite().bit() != 0 => Rim::Rim,
            Piped::Other {
                fluid: true,
                hose: false,
            } => Rim::Drain,
            _ => Rim::Rim,
        }
    };
    match own {
        Piped::Pipe(sides) | Piped::Encased(sides) => {
            if matches!(own, Piped::Encased(_)) && base != Rim::Drain {
                return Rim::None;
            }
            if base == Rim::Rim {
                if !matches!(n, Piped::Pipe(_) | Piped::Encased(_) | Piped::Glass(_))
                    && n.flows_toward(d.opposite())
                {
                    return Rim::DetailedConnection;
                }
                // `shouldDrawRim`: not toward another plain pipe.
                if matches!(n, Piped::Pipe(_)) {
                    return if straight_axis(sides) == Some(d.axis()) {
                        Rim::Connection
                    } else {
                        Rim::DetailedConnection
                    };
                }
            }
            if base == Rim::None && matches!(own, Piped::Pipe(_)) && sides & d.bit() != 0 {
                return Rim::DetailedConnection;
            }
            base
        }
        Piped::Glass(axis) | Piped::Smart(axis) | Piped::Valve(axis) => {
            if base == Rim::Rim && matches!(own, Piped::Valve(_)) {
                return Rim::None;
            }
            if base == Rim::Rim && !matches!(own, Piped::Glass(_)) && matches!(n, Piped::Glass(_)) {
                return Rim::PartialRim;
            }
            if base == Rim::Rim && matches!(n, Piped::Pipe(_)) {
                return Rim::None;
            }
            if n.axis() == Some(axis) {
                return Rim::None;
            }
            if let Piped::Valve(a) = n
                && a == d.axis()
            {
                return Rim::None;
            }
            base.without_connector()
        }
        Piped::Pump(_) if base == Rim::Rim => Rim::None,
        _ => base,
    }
}

/// `FluidPipeBlock.shouldDrawCasing`: a pipe joined on more than two sides around an axis.
pub fn draws_casing(sides: u8) -> bool {
    [Axis::X, Axis::Y, Axis::Z].into_iter().any(|a| {
        Dir::ALL
            .into_iter()
            .filter(|d| d.axis() != a && sides & d.bit() != 0)
            .count()
            > 2
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complementary_pump_sides_are_pressure_endpoints() {
        for toward in Dir::ALL {
            assert!(pump_endpoint(toward, toward, false));
            assert!(pump_endpoint(toward.opposite(), toward, true));
            assert!(!pump_endpoint(toward.opposite(), toward, false));
            assert!(!pump_endpoint(toward, toward, true));
            for facing in Dir::ALL {
                if facing.axis() != toward.axis() {
                    assert!(!pump_endpoint(facing, toward, false));
                    assert!(!pump_endpoint(facing, toward, true));
                }
            }
        }
    }

    // Java issue #67 witness: empty cauldron west, player looking south.
    // FluidPipeBlock.updateBlockState opens west and its opposite (mask 48).
    #[test]
    fn vanilla_target_placement_opens_east_west() {
        let sides = pipe_sides(
            0,
            |d| {
                if d == Dir::West {
                    Cell::VanillaTarget
                } else {
                    Cell::Solid
                }
            },
            Dir::South,
        );
        assert_eq!(sides, 48);
    }

    #[test]
    fn vanilla_target_neighbour_replaces_north_south_shape() {
        let previous = Dir::North.bit() | Dir::South.bit();
        assert_eq!(
            pipe_sides(
                previous,
                |d| if d == Dir::West {
                    Cell::VanillaTarget
                } else {
                    Cell::Open
                },
                Dir::North
            ),
            48
        );
    }

    #[test]
    fn the_smart_pipe_slot_sits_on_its_slanted_side() {
        use crate::steam::Face;
        // Facing south (the model unturned), the slot is toward the north (z 4.625 / 16).
        let (south, _, tilt) = smart_pipe_filter_slot(Face::Floor, Dir::South);
        assert!((south[2] - 4.625 / 16.0).abs() < 1e-6 && tilt == 45.0);
        // Facing east (the model turned y 270), the north side is the west.
        let (east, _, _) = smart_pipe_filter_slot(Face::Floor, Dir::East);
        assert!(east[0] < 0.5 && (east[2] - 0.5).abs() < 1e-6);
        let (west, _, _) = smart_pipe_filter_slot(Face::Floor, Dir::West);
        assert!(west[0] > 0.5);
        assert!(smart_pipe_filter_hit(Face::Floor, Dir::East, east));
        assert!(!smart_pipe_filter_hit(
            Face::Floor,
            Dir::East,
            [0.5, 0.5, 0.5]
        ));
    }

    #[test]
    fn rims_follow_create() {
        let x = Dir::East.bit() | Dir::West.bit();
        // A straight pipe between two pipes: plain connections.
        assert_eq!(
            rim(Piped::Pipe(x), Dir::East, Piped::Pipe(x)),
            Rim::Connection
        );
        // Into a tank: a drain; an open end: a rim; a closed side: nothing.
        let tank = Piped::Other {
            fluid: true,
            hose: false,
        };
        let air = Piped::Other {
            fluid: false,
            hose: false,
        };
        assert_eq!(rim(Piped::Pipe(x), Dir::East, tank), Rim::Drain);
        assert_eq!(rim(Piped::Pipe(x), Dir::East, air), Rim::Rim);
        assert_eq!(rim(Piped::Pipe(x), Dir::Up, air), Rim::None);
        // A pump beside the pipe, either way: the pipe joins it with its detailed connector.
        assert_eq!(
            rim(Piped::Pipe(x), Dir::East, Piped::Pump(Dir::West)),
            Rim::DetailedConnection
        );

        assert_eq!(
            rim(Piped::Pipe(x), Dir::East, Piped::Pump(Dir::East)),
            Rim::DetailedConnection
        );
        // A pump's own side: a drain into a tank, nothing else.
        assert_eq!(rim(Piped::Pump(Dir::East), Dir::East, tank), Rim::Drain);
        assert_eq!(rim(Piped::Pump(Dir::East), Dir::East, air), Rim::None);
        // Glass pipes in a row: nothing between them; a rim without its connector at an end.
        assert_eq!(
            rim(Piped::Glass(Axis::X), Dir::East, Piped::Glass(Axis::X)),
            Rim::None
        );
        assert_eq!(rim(Piped::Glass(Axis::X), Dir::East, air), Rim::PartialRim);
        assert!(draws_casing(x | Dir::Up.bit() | Dir::North.bit()));
        assert!(draws_casing(x | Dir::Up.bit()), "a T joint");
        assert!(!draws_casing(Dir::East.bit() | Dir::Up.bit()), "a bend");
    }

    #[test]
    fn a_pipe_joins_its_neighbours_and_keeps_an_open_end() {
        let none = |_: Dir| Cell::Solid;
        // Alone: straight along the preferred direction.
        assert_eq!(
            pipe_sides(0, none, Dir::North),
            Dir::North.bit() | Dir::South.bit()
        );
        // Next to a tank on the west: west and, opposite, east (an open end).
        let tank = |d: Dir| {
            if d == Dir::West {
                Cell::Tank
            } else {
                Cell::Solid
            }
        };
        assert_eq!(
            pipe_sides(0, tank, Dir::Up),
            Dir::West.bit() | Dir::East.bit()
        );
        // A pump beside it but across its axis: not joined.
        let pump = |d: Dir| {
            if d == Dir::Up {
                Cell::Pump(Dir::East)
            } else {
                Cell::Solid
            }
        };
        assert_eq!(
            pipe_sides(0, pump, Dir::North),
            Dir::North.bit() | Dir::South.bit()
        );
    }

    #[test]
    fn axis_pipes_encased_pipes_and_ports_join_only_their_way() {
        // A glass pipe along x joins a pipe east or west of it, not north.
        let glass = Cell::Axis(Axis::X, Gate::Open);
        assert!(connects(glass, Dir::East) && connects(glass, Dir::West));
        assert!(!connects(glass, Dir::North));
        // An encased pipe open to the north only: a pipe south of it (looking north) joins.
        let encased = Cell::Encased(Dir::South.bit());
        assert!(connects(encased, Dir::North));
        assert!(!connects(encased, Dir::South));
        // A hose pulley whose pipe side is west: joined by a pipe west of it (looking east).
        assert!(connects(Cell::Port(Dir::West), Dir::East));
        assert!(!connects(Cell::Port(Dir::West), Dir::West));
        assert_eq!(
            straight_axis(Dir::Up.bit() | Dir::Down.bit()),
            Some(Axis::Y)
        );
        assert_eq!(straight_axis(Dir::Up.bit() | Dir::East.bit()), None);
    }

    #[test]
    fn a_valve_opens_with_forward_rotation_and_stays_when_stopped() {
        // Facing up, not along the first axis: the pipe runs along x (z turns the shaft).
        assert_eq!(valve_pipe_axis(Dir::Up, false), Axis::X);
        assert_eq!(valve_pipe_axis(Dir::Up, true), Axis::Z);
        assert_eq!(valve_pipe_axis(Dir::North, false), Axis::X);
        assert_eq!(valve_pipe_axis(Dir::North, true), Axis::Y);
        let mut p = Pointer::at(false);
        // 32 RPM: 0.1 a tick; open on the 10th tick.
        p.speed_changed(32.0);
        let mut opened = None;
        for t in 1..=12 {
            if let Some(open) = p.tick(false) {
                opened = Some((t, open));
                break;
            }
        }
        assert_eq!(opened, Some((10, true)));
        assert_eq!(p.angle(), -90.0);
        // Stopped: it stays open and does not move.
        p.speed_changed(0.0);
        assert!(!p.moving());
        assert_eq!(p.tick(true), None);
        // Backwards at 256 RPM (clamped chase 0.8): shut after 2 ticks.
        p.speed_changed(-256.0);
        assert_eq!(p.tick(true), None);
        assert_eq!(p.tick(true), Some(false));
        assert_eq!(pointer_offset(Axis::Y, Axis::Z), 90.0);
        assert_eq!(pointer_offset(Axis::Z, Axis::Y), 0.0);
    }

    #[test]
    fn neighbour_notifications_follow_the_java_block_classes() {
        use crate::Block;
        for block in [
            Block::FluidPipe,
            Block::GlassFluidPipe,
            Block::SmartFluidPipe,
            Block::MechanicalPump,
        ] {
            assert!(!notifies_pipe_neighbours(Some(block), false));
        }
        for block in [Block::FluidValve, Block::EncasedFluidPipe] {
            assert!(notifies_pipe_neighbours(Some(block), false));
        }
        assert!(!notifies_pipe_neighbours(None, true));
        assert!(notifies_pipe_neighbours(None, false));
    }

    #[test]
    fn a_valve_closes_after_the_linear_chaser_residual_tick() {
        // Java at -64 RPM leaves a residual on subtraction five;
        // Catnip tickChaser snaps it on the sixth turn.
        let mut p = Pointer::at(true);
        p.value = 1.;
        p.speed_changed(-64.);
        for _ in 0..5 {
            assert_eq!(p.tick(true), None);
        }
        assert_eq!(p.value, 2.9802322e-8);
        assert_eq!(p.tick(true), Some(false));
    }

    #[test]
    fn a_placed_valve_turns_its_pipe_to_the_pipes() {
        let none = |_: Dir| false;
        // Looking down onto it: facing up; pipes east and west put its pipe along x.
        let (facing, along) =
            valve_placement(Dir::Down, false, Dir::North, none, |d| d.axis() == Axis::X);
        assert_eq!(facing, Dir::Up);
        assert_eq!(valve_pipe_axis(facing, along), Axis::X);
        let (facing, along) =
            valve_placement(Dir::Down, false, Dir::North, none, |d| d.axis() == Axis::Z);
        assert_eq!(valve_pipe_axis(facing, along), Axis::Z);
        // Looking north at a wall: facing south, its pipe up and down by default.
        let (facing, along) = valve_placement(Dir::North, false, Dir::East, none, none);
        assert_eq!(facing, Dir::South);
        assert_eq!(valve_pipe_axis(facing, along), Axis::Y);
        // A pipe to the east (or a shaft above) turns it along x.
        let (_, along) = valve_placement(Dir::North, false, Dir::East, none, |d| d == Dir::East);
        assert_eq!(valve_pipe_axis(Dir::South, along), Axis::X);
        let (_, along) = valve_placement(Dir::North, false, Dir::East, |d| d == Dir::Up, none);
        assert_eq!(valve_pipe_axis(Dir::South, along), Axis::X);
    }

    #[test]
    fn a_smart_pipe_turns_to_the_pipes_around_it() {
        use crate::steam::Face;
        let looking = [
            Dir::North,
            Dir::Up,
            Dir::East,
            Dir::West,
            Dir::Down,
            Dir::South,
        ];
        // Pipes above and below: on a wall, its facing turned back.
        let vertical = |d: Dir| d.axis() == Axis::Y;
        assert_eq!(
            smart_pipe_placement(vertical, looking, Face::Floor, Dir::North),
            Some((Face::Wall, Dir::South))
        );
        // Pipes east and west: on the floor, facing away from the last view along x (west).
        let x = |d: Dir| d.axis() == Axis::X;
        assert_eq!(
            smart_pipe_placement(x, looking, Face::Wall, Dir::North),
            Some((Face::Floor, Dir::East))
        );
        // Pipes on two axes: as placed.
        let two = |d: Dir| matches!(d, Dir::Up | Dir::East);
        assert_eq!(
            smart_pipe_placement(two, looking, Face::Floor, Dir::North),
            None
        );
        assert_eq!(smart_pipe_axis(true, Dir::East), Axis::Y);
        assert_eq!(smart_pipe_axis(false, Dir::East), Axis::X);
    }
}
