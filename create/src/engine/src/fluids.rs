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
    /// Pipes between the pump and it.
    pub distance: i32,
    /// The way from the last pipe (or the pump) to it (`OpenEndedPipe`'s face).
    pub face: Dir,
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
    reach_toward(pump, side, speed, |p, _| cell(p))
}

/// [`reach`] with `cell` told the way it is entered by (from the pipe before it): whether open
/// space is an open end depends on its face toward the pipe (`isOpenEnd`'s
/// `hasBlockSolidSide`).
pub fn reach_toward(
    pump: [i32; 3],
    side: Dir,
    speed: f32,
    cell: impl Fn([i32; 3], Dir) -> Cell,
) -> Vec<End> {
    let mut ends: Vec<(End, i32)> = Vec::new();
    let end = |pos: [i32; 3], tank: bool, pass: Pass, face: Dir| End {
        pos,
        tank,
        pressure: 0.0,
        blocked: pass.blocked,
        only: pass.only,
        distance: 0,
        face,
    };
    let first = step(pump, side);
    let first_cell = cell(first, side);
    match first_cell {
        Cell::Tank => ends.push((end(first, true, Pass::default(), side), 0)),
        Cell::Port(s) if s == side.opposite() => {
            ends.push((end(first, true, Pass::default(), side), 0))
        }
        // A pump right against water or air is an open end itself (`isOpenEnd` from the pump).
        Cell::Open => ends.push((end(first, false, Pass::default(), side), 0)),
        Cell::Pipe(_) | Cell::Encased(_) | Cell::Axis(..) if connects(first_cell, side) => {
            let start = Pass::default().through(first_cell);
            let mut frontier = std::collections::VecDeque::from([(1, first, start)]);
            let mut visited = std::collections::HashSet::new();
            while let Some((distance, at, pass)) = frontier.pop_front() {
                if !visited.insert(at) {
                    continue;
                }
                let sides = openings(cell(at, side));
                for d in Dir::ALL {
                    if sides & d.bit() == 0 {
                        continue;
                    }
                    let next = step(at, d);
                    if next == pump {
                        continue;
                    }
                    let next_cell = cell(next, d);
                    match next_cell {
                        Cell::Tank => ends.push((end(next, true, pass, d), distance)),
                        Cell::Port(s) if s == d.opposite() => {
                            ends.push((end(next, true, pass, d), distance))
                        }
                        Cell::Open => ends.push((end(next, false, pass, d), distance)),
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
            distance: *distance,
            ..*e
        });
    }
    out
}

/// A pipe on the way between a pump and one of its ends: where it is, its side toward the pump
/// (`back`) and toward the end (`on`), and what it lets through (`canPullFluidFrom`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    pub pos: [i32; 3],
    pub back: Dir,
    pub on: Dir,
    pub gate: Gate,
}

/// The pipes on the way from the pump out of its `side` to each end `reach` finds the same way
/// (breadth first, the first way found), from the pump outward, by the end's position.
pub fn ways(
    pump: [i32; 3],
    side: Dir,
    cell: impl Fn([i32; 3]) -> Cell,
) -> Vec<([i32; 3], Vec<Step>)> {
    ways_toward(pump, side, |p, _| cell(p))
}

/// [`ways`] with `cell` told the way it is entered by, as [`reach_toward`].
pub fn ways_toward(
    pump: [i32; 3],
    side: Dir,
    cell: impl Fn([i32; 3], Dir) -> Cell,
) -> Vec<([i32; 3], Vec<Step>)> {
    let first = step(pump, side);
    let first_cell = cell(first, side);
    let piped = |c: Cell| matches!(c, Cell::Pipe(_) | Cell::Encased(_) | Cell::Axis(..));
    if !piped(first_cell) || !connects(first_cell, side) {
        return Vec::new();
    }
    let gate = |c: Cell| match c {
        Cell::Axis(_, g) => g,
        _ => Gate::Open,
    };
    // Each pipe's way back: the pipe before it and the side it was entered by.
    let mut parent: std::collections::HashMap<[i32; 3], ([i32; 3], Dir)> =
        std::collections::HashMap::new();
    let mut gates = std::collections::HashMap::from([(first, gate(first_cell))]);
    let mut ends: Vec<([i32; 3], [i32; 3], Dir)> = Vec::new();
    let mut frontier = std::collections::VecDeque::from([(1, first)]);
    let mut visited = std::collections::HashSet::from([first]);
    while let Some((distance, at)) = frontier.pop_front() {
        let sides = openings(cell(at, side));
        for d in Dir::ALL {
            if sides & d.bit() == 0 {
                continue;
            }
            let next = step(at, d);
            if next == pump {
                continue;
            }
            let next_cell = cell(next, d);
            match next_cell {
                Cell::Tank | Cell::Open => ends.push((next, at, d)),
                Cell::Port(s) if s == d.opposite() => ends.push((next, at, d)),
                c if piped(c) && openings(c) & d.opposite().bit() != 0 => {
                    if distance + 1 >= PUMP_RANGE || !visited.insert(next) {
                        continue;
                    }
                    parent.insert(next, (at, d.opposite()));
                    gates.insert(next, gate(c));
                    frontier.push_back((distance + 1, next));
                }
                _ => {}
            }
        }
    }
    ends.into_iter()
        .map(|(end, last, toward)| {
            let mut way = Vec::new();
            let (mut at, mut on) = (Some(last), toward);
            while let Some(p) = at {
                let (before, back) = parent
                    .get(&p)
                    .map_or((None, side.opposite()), |(q, b)| (Some(*q), *b));
                way.push(Step {
                    pos: p,
                    back,
                    on,
                    gate: gates.get(&p).copied().unwrap_or(Gate::Open),
                });
                on = back.opposite();
                at = before;
            }
            way.reverse();
            (end, way)
        })
        .collect()
}

/// A pipe connection with fluid flowing in it (`PipeConnection.Flow`): the block and its side,
/// whether the fluid comes in by that side (`inbound`) and whether the side is an open end
/// (`OpenEndedPipe`), when the flow starts in ticks after the pump's flows did, how far it grows
/// a tick (`flow_speed`), and its place along the way, which is when it goes once the source
/// gives no more (a connection a tick, from the source).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlowAt {
    pub pos: [i32; 3],
    pub side: Dir,
    pub inbound: bool,
    pub open: bool,
    pub start: f32,
    pub speed: f32,
    pub order: u32,
}

impl FlowAt {
    /// How far across its half of the pipe the flow is `ticks` after the pump's flows started
    /// (`Flow.progress`, 0 to 1).
    pub fn progress(&self, ticks: f32) -> f32 {
        ((ticks - self.start) * self.speed).clamp(0.0, 1.0)
    }
}

/// The flows a turning pump at `pump` facing `facing` keeps (`manageFlows`, connection by
/// connection): from end `from` behind it (`pull`: the pipes between, from the pump outward)
/// into the pump and out through the pipes `push` to each end in front, as far as `fluid` may go
/// (a closed valve or another fluid's filter holds it before the pipe). Each half of a pipe
/// (a connection) fills at its end's pressure, the pump's own two at its speed.
pub fn flows(
    pump: [i32; 3],
    facing: Dir,
    speed: f32,
    from: &End,
    pull: &[Step],
    push: &[(End, Vec<Step>)],
    fluid: Fluid,
) -> Vec<FlowAt> {
    let mut out: Vec<FlowAt> = Vec::new();
    let mut t = 0.0f32;
    let mut order = 0u32;
    let add = |out: &mut Vec<FlowAt>, t: &mut f32, order: &mut u32, at: FlowAt| {
        let half = 1.0 / at.speed;
        let at = FlowAt {
            start: *t,
            order: *order,
            ..at
        };
        *t += half;
        *order += 1;
        match out
            .iter_mut()
            .find(|f| f.pos == at.pos && f.side == at.side)
        {
            Some(f) if f.start > at.start => *f = at,
            Some(_) => {}
            None => out.push(at),
        }
    };
    let conn = |pos, side, inbound, open, pressure: f32| FlowAt {
        pos,
        side,
        inbound,
        open,
        start: 0.0,
        speed: flow_speed(pressure),
        order: 0,
    };
    // Behind: from the end toward the pump.
    let n = pull.len();
    for (i, s) in pull.iter().rev().enumerate() {
        let open = i == 0 && !from.tank;
        add(
            &mut out,
            &mut t,
            &mut order,
            conn(s.pos, s.on, true, open, from.pressure),
        );
        add(
            &mut out,
            &mut t,
            &mut order,
            conn(s.pos, s.back, false, false, from.pressure),
        );
    }
    let back = facing.opposite();
    let pump_in = conn(pump, back, true, n == 0 && !from.tank, speed.abs());
    add(&mut out, &mut t, &mut order, pump_in);
    let start = (t, order);
    for (to, way) in push {
        let (mut t, mut order) = start;
        add(
            &mut out,
            &mut t,
            &mut order,
            conn(pump, facing, false, way.is_empty() && !to.tank, speed.abs()),
        );
        let last = way.len().saturating_sub(1);
        for (i, s) in way.iter().enumerate() {
            let held = match s.gate {
                Gate::Open => false,
                Gate::Closed => true,
                Gate::Only(f) => f != fluid,
            };
            if held {
                break;
            }
            add(
                &mut out,
                &mut t,
                &mut order,
                conn(s.pos, s.back, true, false, to.pressure),
            );
            let open = i == last && !to.tank;
            add(
                &mut out,
                &mut t,
                &mut order,
                conn(s.pos, s.on, false, open, to.pressure),
            );
        }
    }
    out
}

/// `PipeConnection.manageFlows`: how far a flow gets across half a pipe in a tick at
/// `pressure` (`1/32 + clamp(pressure / 128, 0, 1) * 31/32`).
pub fn flow_speed(pressure: f32) -> f32 {
    1.0 / 32.0 + (pressure / 128.0).clamp(0.0, 1.0) * 31.0 / 32.0
}

/// Ticks from a pump starting until fluid from end `from` behind it reaches end `to` in front
/// (`FluidNetwork.tick` finds a target only once every flow on the way is complete): each pipe
/// two connections (a half each) at its end's pressure, and the pump's own two at its speed.
pub fn arrival_ticks(from: &End, to: &End, speed: f32) -> u32 {
    let halves = |n: i32, pressure: f32| 2.0 * n as f32 / flow_speed(pressure);
    let t = halves(from.distance, from.pressure)
        + halves(1, speed.abs())
        + halves(to.distance, to.pressure);
    t.ceil() as u32
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

    fn end(pos: [i32; 3], tank: bool, pressure: f32) -> End {
        End {
            pos,
            tank,
            pressure,
            blocked: false,
            only: None,
            distance: 0,
            // The tests' ends lie along x from the pump.
            face: if pos[0] < 0 { Dir::West } else { Dir::East },
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
    fn flows_go_connection_by_connection_from_the_source_to_the_ends() {
        // Pump at x 0 facing east: open space at x -2, a pipe at x -1; a glass pipe at x 1,
        // a valve at x 2 and a tank at x 3.
        let straight = Dir::East.bit() | Dir::West.bit();
        let cell = |closed: bool| {
            move |p: [i32; 3]| match p {
                [0, 0, 0] => Cell::Pump(Dir::East),
                [-1, 0, 0] => Cell::Pipe(straight),
                [1, 0, 0] => Cell::Axis(Axis::X, Gate::Open),
                [2, 0, 0] => Cell::Axis(Axis::X, if closed { Gate::Closed } else { Gate::Open }),
                [3, 0, 0] => Cell::Tank,
                [-2, 0, 0] => Cell::Open,
                _ => Cell::Solid,
            }
        };
        let pull = ways([0, 0, 0], Dir::West, cell(false));
        assert_eq!(pull.len(), 1);
        assert_eq!(pull[0].0, [-2, 0, 0]);
        assert_eq!(
            pull[0].1,
            vec![Step {
                pos: [-1, 0, 0],
                back: Dir::East,
                on: Dir::West,
                gate: Gate::Open
            }]
        );
        let push = ways([0, 0, 0], Dir::East, cell(false));
        assert_eq!(push[0].0, [3, 0, 0]);
        assert_eq!(push[0].1.len(), 2);
        assert_eq!(
            (push[0].1[1].pos, push[0].1[1].back, push[0].1[1].on),
            ([2, 0, 0], Dir::West, Dir::East)
        );
        let from = reach([0, 0, 0], Dir::West, 128.0, cell(false))[0];
        let to = reach([0, 0, 0], Dir::East, 128.0, cell(false))[0];
        let push_ways = vec![(to, push[0].1.clone())];
        let f = flows(
            [0, 0, 0],
            Dir::East,
            128.0,
            &from,
            &pull[0].1,
            &push_ways,
            Fluid::Water,
        );
        // At 128 rpm each half fills in a tick: the open end's side first, inbound and open.
        let at =
            |pos: [i32; 3], side: Dir| *f.iter().find(|x| x.pos == pos && x.side == side).unwrap();
        let first = at([-1, 0, 0], Dir::West);
        assert!(first.inbound && first.open);
        assert_eq!((first.start, first.order), (0.0, 0));
        assert_eq!(at([-1, 0, 0], Dir::East).start, 1.0);
        assert_eq!(at([0, 0, 0], Dir::West).start, 2.0);
        assert_eq!(at([0, 0, 0], Dir::East).start, 3.0);
        let glass_in = at([1, 0, 0], Dir::West);
        assert!(glass_in.inbound && !glass_in.open);
        assert_eq!(glass_in.start, 4.0);
        let last = at([2, 0, 0], Dir::East);
        assert!(!last.inbound && !last.open);
        assert_eq!((last.start, last.order), (7.0, 7));
        assert_eq!(last.progress(7.5), 0.5);
        // A closed valve holds the flow in the pipe before it.
        let push = ways([0, 0, 0], Dir::East, cell(true));
        let push_ways = vec![(to, push[0].1.clone())];
        let f = flows(
            [0, 0, 0],
            Dir::East,
            128.0,
            &from,
            &pull[0].1,
            &push_ways,
            Fluid::Water,
        );
        assert!(f.iter().any(|x| x.pos == [1, 0, 0]));
        assert!(!f.iter().any(|x| x.pos == [2, 0, 0]));
        // Slower: 1/32 + 16/128 * 31/32 a tick.
        assert!((flow_speed(16.0) - 0.152_343_75).abs() < 1e-6);
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
        assert_eq!(
            front,
            vec![End {
                distance: 3,
                ..end([4, 0, 0], true, 16.0)
            }]
        );
        let back = reach([0, 0, 0], Dir::West, 16.0, cell);
        assert_eq!(
            back,
            vec![End {
                distance: 1,
                ..end([-2, 0, 0], false, 16.0)
            }]
        );
        assert_eq!(transfer_speed(16.0), 8);
        // A pump right against water (no pipe) draws from it as an open end.
        let bare = |p: [i32; 3]| match p {
            [0, 0, 0] => Cell::Pump(Dir::East),
            [-1, 0, 0] => Cell::Open,
            _ => Cell::Solid,
        };
        assert_eq!(
            reach([0, 0, 0], Dir::West, 16.0, bare),
            vec![end([-1, 0, 0], false, 16.0)]
        );
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
        assert_eq!(
            open,
            vec![End {
                distance: 3,
                ..end([4, 0, 0], true, 32.0)
            }]
        );
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

    #[test]
    fn fluid_takes_its_time_through_the_pipes_and_is_shared() {
        // 16 rpm: a half pipe in 1 / (1/32 + 16/128 * 31/32) ≈ 6.6 ticks.
        assert!((flow_speed(16.0) - 0.152_343_75).abs() < 1e-6);
        let at = |distance: i32, pressure: f32| End {
            pos: [0; 3],
            tank: true,
            pressure,
            blocked: false,
            only: None,
            distance,
            face: Dir::Up,
        };
        let end = |distance: i32| at(distance, 16.0);
        // A tank right behind, one 5 pipes in front: (0 + 2 + 10) halves at 6.56 ticks.
        assert_eq!(arrival_ticks(&end(0), &end(5), 16.0), 79);
        // Further is later; faster is sooner.
        assert!(arrival_ticks(&end(0), &end(10), 16.0) > 79);
        assert!(arrival_ticks(&at(0, 256.0), &at(5, 256.0), 256.0) < 20);
        // 10 mB among three ends that take all: 4, 3, 3.
        let mut got = [0u32; 3];
        assert_eq!(
            share(3, 10, |i, n| {
                got[i] += n;
                n
            }),
            10
        );
        assert_eq!(got, [4, 3, 3]);
        // One end full after 1 mB: the rest goes to the others.
        let mut got = [0u32; 3];
        let cap = [1u32, 100, 100];
        let placed = share(3, 10, |i, n| {
            let take = n.min(cap[i] - got[i]);
            got[i] += take;
            take
        });
        assert_eq!((placed, got), (10, [1, 5, 4]));
        // Nobody takes anything: nothing placed, and it ends.
        assert_eq!(share(2, 10, |_, _| 0), 0);
    }
}
