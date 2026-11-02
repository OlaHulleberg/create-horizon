//! Create's fluid pipes and mechanical pump (`FluidPipeBlock`, `PumpBlockEntity`,
//! `FluidPropagator`, `FluidNetwork`, `OpenEndedPipe`), with the glass, encased and smart pipes
//! and the fluid valve (`GlassFluidPipeBlock`, `EncasedPipeBlock`, `SmartFluidPipeBlock(Entity)`,
//! `FluidValveBlock(Entity)`): pipes connect to what can hold fluid; a turning pump pulls through
//! the pipes behind it and pushes through those in front, as far as the pump range, from a tank
//! or an open end into a tank or an open end, past open valves and the filters of smart pipes.

use crate::kinetics::{Axis, Dir};
use crate::tank::Fluid;

/// `mechanicalPumpRange`.
pub const PUMP_RANGE: i32 = 16;

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
        Cell::Pipe(_) | Cell::Tank => true,
        Cell::Encased(sides) => sides & toward.opposite().bit() != 0,
        Cell::Axis(a, _) => a == toward.axis(),
        Cell::Port(side) => side == toward.opposite(),
        Cell::Pump(f) => f.axis() == toward.axis(),
        _ => false,
    }
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

/// An end of a pump's pipes: the block fluid comes from or goes to, and the pressure there.
/// `blocked`: a closed valve on the way lets nothing through; `only`: smart pipes on the way let
/// only that fluid through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct End {
    pub pos: [i32; 3],
    pub tank: bool,
    pub pressure: f32,
    pub blocked: bool,
    pub only: Option<Fluid>,
}

impl End {
    /// Whether `fluid` may flow between the pump and this end.
    pub fn passes(&self, fluid: Fluid) -> bool {
        !self.blocked && self.only.is_none_or(|f| f == fluid)
    }
}

/// What the pipes between a pump and a block let through, so far.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Pass {
    blocked: bool,
    only: Option<Fluid>,
}

impl Pass {
    fn through(self, cell: Cell) -> Pass {
        match cell {
            Cell::Axis(_, Gate::Closed) => Pass {
                blocked: true,
                ..self
            },
            Cell::Axis(_, Gate::Only(f)) => match self.only {
                Some(g) if g != f => Pass {
                    blocked: true,
                    ..self
                },
                _ => Pass {
                    only: Some(f),
                    ..self
                },
            },
            _ => self,
        }
    }
}

fn step(p: [i32; 3], d: Dir) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

/// `PumpBlockEntity.distributePressureTo`: the ends reached from the pump's `side` through pipes
/// (breadth first, up to the pump range), the pressure (the pump's speed) shared among the ends
/// at each distance (`parallelBranches`). Pressure goes through closed valves and filters
/// (they have pipe behaviours); only the flow stops there (`End::passes`).
pub fn reach(pump: [i32; 3], side: Dir, speed: f32, cell: impl Fn([i32; 3]) -> Cell) -> Vec<End> {
    let mut ends: Vec<(End, i32)> = Vec::new();
    let end = |pos: [i32; 3], tank: bool, pass: Pass| End {
        pos,
        tank,
        pressure: 0.0,
        blocked: pass.blocked,
        only: pass.only,
    };
    let first = step(pump, side);
    let first_cell = cell(first);
    match first_cell {
        Cell::Tank => ends.push((end(first, true, Pass::default()), 0)),
        Cell::Port(s) if s == side.opposite() => ends.push((end(first, true, Pass::default()), 0)),
        Cell::Pipe(_) | Cell::Encased(_) | Cell::Axis(..) if connects(first_cell, side) => {
            let start = Pass::default().through(first_cell);
            let mut frontier = std::collections::VecDeque::from([(1, first, start)]);
            let mut visited = std::collections::HashSet::new();
            while let Some((distance, at, pass)) = frontier.pop_front() {
                if !visited.insert(at) {
                    continue;
                }
                let sides = openings(cell(at));
                for d in Dir::ALL {
                    if sides & d.bit() == 0 {
                        continue;
                    }
                    let next = step(at, d);
                    if next == pump {
                        continue;
                    }
                    let next_cell = cell(next);
                    match next_cell {
                        Cell::Tank => ends.push((end(next, true, pass), distance)),
                        Cell::Port(s) if s == d.opposite() => {
                            ends.push((end(next, true, pass), distance))
                        }
                        Cell::Open => ends.push((end(next, false, pass), distance)),
                        Cell::Pipe(_) | Cell::Encased(_) | Cell::Axis(..)
                            if openings(next_cell) & d.opposite().bit() != 0 =>
                        {
                            if distance + 1 >= PUMP_RANGE {
                                continue;
                            }
                            if !visited.contains(&next) {
                                frontier.push_back((distance + 1, next, pass.through(next_cell)));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
    let mut out = Vec::new();
    for (e, distance) in &ends {
        let same = ends.iter().filter(|(_, d)| d == distance).count();
        let branches = same.max(1) as f32;
        out.push(End {
            pressure: speed.abs() / branches,
            ..*e
        });
    }
    out
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
    /// At rest, open or shut as its block is (what a loaded valve starts from).
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
        if self.moving() {
            self.value += (self.target - self.value).clamp(-self.speed, self.speed);
            if (self.value - self.target).abs() < 1.0e-6 {
                self.value = self.target;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn end(pos: [i32; 3], tank: bool, pressure: f32) -> End {
        End {
            pos,
            tank,
            pressure,
            blocked: false,
            only: None,
        }
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
    fn a_pump_reaches_the_ends_of_its_pipes() {
        // Pump at x 0 facing east; pipes x 1..=3 along x; a tank at x 4, open space at x -2.
        let cell = |p: [i32; 3]| match p {
            [0, 0, 0] => Cell::Pump(Dir::East),
            [1..=3, 0, 0] | [-1, 0, 0] => Cell::Pipe(Dir::East.bit() | Dir::West.bit()),
            [4, 0, 0] => Cell::Tank,
            [-2, 0, 0] => Cell::Open,
            _ => Cell::Solid,
        };
        let front = reach([0, 0, 0], Dir::East, 16.0, cell);
        assert_eq!(front, vec![end([4, 0, 0], true, 16.0)]);
        let back = reach([0, 0, 0], Dir::West, 16.0, cell);
        assert_eq!(back, vec![end([-2, 0, 0], false, 16.0)]);
        assert_eq!(transfer_speed(16.0), 8);
        // Two ends at the same distance share the pressure.
        let fork = |p: [i32; 3]| match p {
            [1, 0, 0] => Cell::Pipe(Dir::West.bit() | Dir::North.bit() | Dir::South.bit()),
            [1, 0, -1] | [1, 0, 1] => Cell::Tank,
            [0, 0, 0] => Cell::Pump(Dir::East),
            _ => Cell::Solid,
        };
        let ends = reach([0, 0, 0], Dir::East, 16.0, fork);
        assert_eq!(ends.len(), 2);
        assert!(ends.iter().all(|e| e.pressure == 8.0));
    }

    #[test]
    fn valves_and_smart_pipes_gate_the_flow_but_not_the_pressure() {
        // Pump at x 0 facing east; x 1 a valve or smart pipe along x, x 2 an encased pipe open
        // east and west, x 3 a glass pipe, a tank at x 4 and a hose pulley (pipe side west) at
        // x 5 behind it is not reached.
        let line = |gate: Gate| {
            move |p: [i32; 3]| match p {
                [0, 0, 0] => Cell::Pump(Dir::East),
                [1, 0, 0] => Cell::Axis(Axis::X, gate),
                [2, 0, 0] => Cell::Encased(Dir::East.bit() | Dir::West.bit()),
                [3, 0, 0] => Cell::Axis(Axis::X, Gate::Open),
                [4, 0, 0] => Cell::Port(Dir::West),
                _ => Cell::Solid,
            }
        };
        let open = reach([0, 0, 0], Dir::East, 32.0, line(Gate::Open));
        assert_eq!(open, vec![end([4, 0, 0], true, 32.0)]);
        assert!(open[0].passes(Fluid::Lava));
        let shut = reach([0, 0, 0], Dir::East, 32.0, line(Gate::Closed));
        assert_eq!(shut.len(), 1);
        assert!(shut[0].blocked && !shut[0].passes(Fluid::Water));
        let water_only = reach([0, 0, 0], Dir::East, 32.0, line(Gate::Only(Fluid::Water)));
        assert!(water_only[0].passes(Fluid::Water) && !water_only[0].passes(Fluid::Lava));
        // A port facing away from the pipe is no end.
        let wrong_side = |p: [i32; 3]| match p {
            [1, 0, 0] => Cell::Port(Dir::East),
            _ => Cell::Solid,
        };
        assert!(reach([0, 0, 0], Dir::East, 32.0, wrong_side).is_empty());
        // Two filters for different fluids on the way: nothing passes.
        let two = |p: [i32; 3]| match p {
            [1, 0, 0] => Cell::Axis(Axis::X, Gate::Only(Fluid::Water)),
            [2, 0, 0] => Cell::Axis(Axis::X, Gate::Only(Fluid::Lava)),
            [3, 0, 0] => Cell::Tank,
            _ => Cell::Solid,
        };
        assert!(reach([0, 0, 0], Dir::East, 32.0, two)[0].blocked);
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
