//! Create's three transport layers: pump graph pressure, connection flows, and
//! `FluidNetwork` traversal. The host supplies world/capability operations only.
use super::transport::{FluidTransportBehaviour, Source, UpdatePhase};
use super::{Cell, Gate, PUMP_RANGE, openings, step};
use crate::{kinetics::Dir, tank::Fluid};
use std::collections::{HashMap, HashSet, VecDeque};
pub type Pos = [i32; 3];
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Face {
    pub pos: Pos,
    pub side: Dir,
}
impl Face {
    pub fn connected(self) -> Pos {
        step(self.pos, self.side)
    }
    pub fn opposite(self) -> Self {
        Self {
            pos: self.connected(),
            side: self.side.opposite(),
        }
    }
}
/// World reads and fluid capabilities, including OpenEndedPipe's owned tank.
pub trait Host {
    fn cell(&self, pos: Pos, toward: Dir) -> Cell;
    fn loaded(&self, _pos: Pos) -> bool {
        true
    }
    fn speed(&self, pos: Pos) -> f32;
    fn time(&self) -> i64;
    fn tank_fluids(&self, face: Face) -> Vec<Fluid>;
    fn notifies_pipe_neighbours(&self, pos: Pos) -> bool {
        !matches!(
            self.cell(pos, Dir::Down),
            Cell::Pipe(_) | Cell::Axis(..) | Cell::Pump(_)
        )
    }
    /// FlowSource.provideFluid first probes a generic drain. A rejected probe
    /// may only fall back to matching fluid actually exposed in the handler's tanks.
    fn provide_fluid(&mut self, face: Face, source: &mut Source, gate: Gate) -> Option<Fluid> {
        let accepts = |fluid: Option<Fluid>| match gate {
            Gate::Open => true,
            Gate::Closed => false,
            Gate::Only(wanted) => fluid == Some(wanted),
        };
        let immediate = self.offer(face, source, 1, None).map(|(fluid, _)| fluid);
        if accepts(immediate) {
            return immediate;
        }
        let contained = match source {
            Source::Open(open) => open
                .state()
                .tank
                .filter(|(_, n)| *n > 0)
                .map(|(f, _)| vec![f])
                .unwrap_or_default(),
            Source::Handler => self.tank_fluids(face),
            _ => Vec::new(),
        };
        for fluid in contained {
            if accepts(Some(fluid)) {
                return self.offer(face, source, 1, Some(fluid)).map(|(f, _)| f);
            }
        }
        None
    }
    fn offer(
        &mut self,
        face: Face,
        source: &mut Source,
        amount: u32,
        fluid: Option<Fluid>,
    ) -> Option<(Fluid, u32)>;
    fn drain(&mut self, face: Face, source: &mut Source, fluid: Fluid, amount: u32) -> u32;
    fn fill(
        &mut self,
        face: Face,
        source: &mut Source,
        fluid: Fluid,
        amount: u32,
        execute: bool,
    ) -> u32;
    fn identity(&self, face: Face) -> Pos {
        face.connected()
    }
    /// FlowSource.keepAlive for portable interfaces with a live frontier.
    fn keep_alive(&mut self, face: Face);
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pump {
    pub speed: f32,
    pub pending: u8,
}
/// A behaviour callback directly propagates its pipe; a block change also
/// runs the pipe neighbours' class-specific neighbourChanged admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Pipe(Pos),
    Block(Pos),
}

/// Owned per dimension; each pipe owns its side interfaces and each inbound
/// endpoint connection owns its network. No endpoint paths or pump clocks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transport {
    pub pipes: HashMap<Pos, FluidTransportBehaviour>,
    pub pumps: HashMap<Pos, Pump>,
    pub changed: Vec<Change>,
    scheduled: Vec<Change>,
}
pub fn sides(cell: Cell) -> u8 {
    match cell {
        Cell::Pump(f) => f.bit() | f.opposite().bit(),
        c => openings(c),
    }
}
fn endpoint(cell: Cell, side: Dir, pull: bool) -> bool {
    match cell {
        Cell::Pump(f) => super::pump_endpoint(f, side, pull),
        Cell::Tank | Cell::Open | Cell::VanillaTarget => true,
        Cell::Port(s) => s == side.opposite(),
        _ => false,
    }
}
impl Transport {
    pub fn register(&mut self, pos: Pos, cell: Cell, speed: f32) {
        if sides(cell) == 0 {
            return;
        }
        let restored = self.pipes.contains_key(&pos);
        self.pipes
            .entry(pos)
            .or_insert_with(|| FluidTransportBehaviour::new(sides(cell)));
        let pipe = self.pipes.get_mut(&pos).unwrap();
        for side in Dir::ALL {
            if sides(cell) & side.bit() != 0 && pipe.connection(side).is_none() {
                pipe.interfaces.push((side, Default::default()));
            }
        }
        if matches!(cell, Cell::Pump(_)) {
            self.pumps.entry(pos).or_insert(Pump {
                speed: if restored { speed.abs() } else { 0. },
                pending: if restored { 0 } else { 3 },
            });
        }
    }
    pub fn remove(&mut self, pos: Pos) {
        self.pipes.remove(&pos);
        self.pumps.remove(&pos);
        self.changed.push(Change::Block(pos));
    }
    pub fn connection(&self, face: Face) -> Option<&super::transport::PipeConnection> {
        self.pipes
            .get(&face.pos)?
            .interfaces
            .iter()
            .find(|(d, _)| *d == face.side)
            .map(|(_, c)| c)
    }
    pub fn connection_mut(&mut self, face: Face) -> Option<&mut super::transport::PipeConnection> {
        self.pipes.get_mut(&face.pos)?.connection(face.side)
    }
    /// FluidPropagator.propagateChangedPipe: stop at pumps, walk past the
    /// configured reach only through pipes which still carry pressure.
    pub fn propagate_changed_pipe(&mut self, host: &impl Host, start: Pos) {
        let mut queue = VecDeque::from([(0, start)]);
        let mut visited = HashSet::new();
        while let Some((distance, pos)) = queue.pop_front() {
            if !visited.insert(pos) {
                continue;
            }
            let cell = host.cell(pos, Dir::Down);
            if let Some(pipe) = self.pipes.get_mut(&pos) {
                pipe.wipe_pressure(sides(cell));
            } else {
                continue;
            }
            for side in Dir::ALL {
                if sides(cell) & side.bit() == 0 {
                    continue;
                }
                let next = step(pos, side);
                if !host.loaded(next) {
                    continue;
                }
                let target = host.cell(next, side);
                if let Cell::Pump(f) = target {
                    if f.axis() == side.axis() {
                        if let Some(pump) = self.pumps.get_mut(&next) {
                            pump.pending |= if f == side.opposite() { 1 } else { 2 };
                        }
                        if let Some(pipe) = self.pipes.get_mut(&next) {
                            pipe.wipe_pressure(sides(target));
                        }
                    }
                    continue;
                }
                if distance >= PUMP_RANGE
                    && !self
                        .pipes
                        .get(&next)
                        .is_some_and(|p| p.interfaces.iter().any(|(_, c)| c.has_pressure()))
                {
                    continue;
                }
                if sides(target) & side.opposite().bit() != 0 {
                    queue.push_back((distance + 1, next));
                }
            }
        }
    }
    /// FluidPipeBlock.neighborChanged/onPlace schedule a block tick with delay
    /// one. Freeze the due callbacks before this level's scheduled fluid ticks;
    /// block changes made during those ticks belong to the next level tick.
    pub fn begin_tick(&mut self) {
        self.changed.retain(|change| {
            if matches!(change, Change::Block(_)) {
                self.scheduled.push(*change);
                false
            } else {
                true
            }
        });
    }

    pub fn prepare(&mut self, host: &impl Host) {
        self.pipes
            .retain(|p, _| sides(host.cell(*p, Dir::Down)) != 0);
        self.pumps
            .retain(|p, _| matches!(host.cell(*p, Dir::Down), Cell::Pump(_)));
        let mut changed = std::mem::take(&mut self.scheduled);
        self.changed.retain(|change| {
            if matches!(change, Change::Pipe(_)) {
                changed.push(*change);
                false
            } else {
                true
            }
        });
        for change in changed {
            let pos = match change {
                Change::Pipe(p) | Change::Block(p) => p,
            };
            if self.pipes.contains_key(&pos) {
                self.propagate_changed_pipe(host, pos);
            }
            if matches!(change, Change::Block(_)) && host.notifies_pipe_neighbours(pos) {
                for side in Dir::ALL {
                    let next = step(pos, side);
                    let cell = host.cell(next, side);
                    let straight = match cell {
                        Cell::Pipe(mask) => super::straight_axis(mask).is_some(),
                        Cell::Encased(_) | Cell::Axis(..) | Cell::Pump(_) => true,
                        _ => false,
                    };
                    if straight && sides(cell) & side.opposite().bit() != 0 {
                        self.propagate_changed_pipe(host, next);
                    }
                }
            }
        }
    }
    /// PumpBlockEntity.distributePressureTo: BFS constructs the directed graph;
    /// the recursive endpoint search retains all successful faces by distance.
    pub fn distribute_pressure_to(&mut self, host: &impl Host, pos: Pos, side: Dir, pull: bool) {
        let speed = host.speed(pos).abs();
        if speed == 0. {
            return;
        }
        let start = Face { pos, side };
        if !pull {
            self.reset_affected_networks(Face {
                pos,
                side: side.opposite(),
            });
        }
        if endpoint(host.cell(start.connected(), side), side, pull) {
            return;
        }
        type Graph = HashMap<Pos, (i32, HashMap<Dir, bool>)>;
        let mut graph = Graph::new();
        let mut targets = HashSet::new();
        graph.entry(pos).or_default().1.insert(side, pull);
        graph
            .entry(start.connected())
            .or_insert_with(|| (1, HashMap::new()))
            .1
            .insert(side.opposite(), !pull);
        let mut queue = VecDeque::from([(1, start.connected())]);
        let mut visited = HashSet::new();
        while let Some((distance, at)) = queue.pop_front() {
            if !host.loaded(at) || !visited.insert(at) {
                continue;
            }
            let cell = host.cell(at, side);
            if sides(cell) == 0 {
                continue;
            }
            for d in Dir::ALL {
                if sides(cell) & d.bit() == 0 {
                    continue;
                }
                let face = Face { pos: at, side: d };
                let next = face.connected();
                if !host.loaded(next) || face == start || face.opposite() == start {
                    continue;
                }
                let target = host.cell(next, d);
                if endpoint(target, d, pull) {
                    graph
                        .entry(at)
                        .or_insert_with(|| (distance, HashMap::new()))
                        .1
                        .insert(d, pull);
                    targets.insert(face);
                    continue;
                }
                if sides(target) == 0 || matches!(target, Cell::Pump(_)) || visited.contains(&next)
                {
                    continue;
                }
                graph
                    .entry(at)
                    .or_insert_with(|| (distance, HashMap::new()))
                    .1
                    .insert(d, pull);
                if distance + 1 >= PUMP_RANGE {
                    targets.insert(face);
                    continue;
                }
                graph
                    .entry(next)
                    .or_insert_with(|| (distance + 1, HashMap::new()))
                    .1
                    .insert(d.opposite(), !pull);
                queue.push_back((distance + 1, next));
            }
        }
        fn search(
            graph: &Graph,
            targets: &HashSet<Face>,
            valid: &mut HashMap<i32, HashSet<Face>>,
            current: Face,
            pull: bool,
        ) -> bool {
            let Some((distance, map)) = graph.get(&current.pos) else {
                return false;
            };
            let mut success = false;
            for d in Dir::ALL {
                if d == current.side || !map.contains_key(&d) {
                    continue;
                }
                let face = Face {
                    pos: current.pos,
                    side: d,
                };
                if targets.contains(&face)
                    || (map[&d] == pull && search(graph, targets, valid, face.opposite(), pull))
                {
                    valid.entry(*distance).or_default().insert(face);
                    success = true;
                }
            }
            if success {
                valid.entry(*distance).or_default().insert(current);
            }
            success
        }
        let mut valid = HashMap::new();
        search(
            &graph,
            &targets,
            &mut valid,
            Face {
                pos,
                side: side.opposite(),
            },
            pull,
        );
        for faces in valid.values() {
            let amount = speed / (faces.len().saturating_sub(1).max(1) as f32);
            for face in faces {
                if face.pos == pos {
                    continue;
                }
                let inbound = graph[&face.pos].1[&face.side];
                if let Some(c) = self.connection_mut(*face) {
                    c.add_pressure(inbound, amount);
                }
            }
        }
    }
    pub fn reset_affected_networks(&mut self, start: Face) {
        let mut queue = VecDeque::from([start.pos]);
        let mut visited = HashSet::new();
        while let Some(pos) = queue.pop_front() {
            if !visited.insert(pos) {
                continue;
            }
            for d in Dir::ALL {
                if pos == start.pos && d != start.side {
                    continue;
                }
                let f = Face { pos, side: d };
                if let Some(c) = self.connection_mut(f) {
                    if !c.flow.is_some_and(|f| f.inbound) {
                        continue;
                    }
                    if let Some(n) = &mut c.network {
                        n.reset();
                    }
                    queue.push_back(f.connected());
                }
            }
        }
    }
    fn determine_source(&mut self, host: &impl Host, face: Face) {
        if self.connection(face).is_none_or(|c| c.source.is_some())
            || !host.loaded(face.connected())
        {
            return;
        }
        let cell = host.cell(face.connected(), face.side);
        let source = match cell {
            Cell::Open | Cell::VanillaTarget => Source::Open(Default::default()),
            Cell::Tank => Source::Handler,
            Cell::Port(d) if d == face.side.opposite() => Source::Handler,
            _ if sides(cell) != 0 => Source::OtherPipe,
            _ => Source::Blocked,
        };
        self.connection_mut(face).unwrap().set_source(source);
    }
    /// PumpBlockEntity.onSpeedChanged invokes updatePressureChange immediately,
    /// including when an earlier kinetic entity attaches before this pipe's turn.
    fn refresh_pump_pressure(&mut self, host: &impl Host) {
        let changed: Vec<_> = self
            .pumps
            .iter()
            .filter_map(|(pos, pump)| {
                let speed = host.speed(*pos).abs();
                (pump.speed.abs() != speed).then_some((*pos, speed))
            })
            .collect();
        for (pos, speed) in changed {
            let Cell::Pump(facing) = host.cell(pos, Dir::Down) else {
                continue;
            };
            self.propagate_changed_pipe(host, step(pos, facing));
            self.propagate_changed_pipe(host, step(pos, facing.opposite()));
            if let Some(pipe) = self.pipes.get_mut(&pos) {
                pipe.wipe_pressure(sides(Cell::Pump(facing)));
            }
            if let Some(pump) = self.pumps.get_mut(&pos) {
                pump.pending = 3;
                pump.speed = speed;
            }
        }
    }

    pub fn tick_pipe(&mut self, host: &mut impl Host, pos: Pos) -> Option<(Fluid, Fluid)> {
        self.refresh_pump_pressure(host);
        let collision = self.tick_behaviour(host, pos);
        if let Cell::Pump(f) = host.cell(pos, Dir::Down) {
            let speed = host.speed(pos).abs();
            for (d, c) in &mut self.pipes.get_mut(&pos)?.interfaces {
                c.set_pressure(if *d == f { [0., speed] } else { [speed, 0.] });
            }
            let p = self.pumps.entry(pos).or_default();
            let pending = p.pending;
            p.pending = 0;
            p.speed = speed;
            for (flag, side, pull) in [(1, f, false), (2, f.opposite(), true)] {
                if pending & flag != 0 {
                    self.distribute_pressure_to(host, pos, side, pull);
                }
            }
        }
        collision
    }

    fn tick_behaviour(&mut self, host: &mut impl Host, pos: Pos) -> Option<(Fluid, Fluid)> {
        let phase = self.pipes.get(&pos)?.phase;
        if phase == UpdatePhase::WaitForPumps {
            self.pipes.get_mut(&pos)?.phase = UpdatePhase::FlipFlows;
        } else {
            let faces: Vec<_> = self.pipes[&pos]
                .interfaces
                .iter()
                .map(|(side, _)| Face { pos, side: *side })
                .collect();
            for &face in &faces {
                self.connection_mut(face)?.flip_flows();
                self.determine_source(host, face);
            }
            if phase == UpdatePhase::FlipFlows {
                self.pipes.get_mut(&pos)?.phase = UpdatePhase::Idle;
            } else {
                let mut available = None;
                let mut single = None;
                for &face in &faces {
                    if let Some(f) = self.connection(face)?.provided_fluid(true) {
                        match available {
                            None => {
                                available = Some(f);
                                single = Some(face.side);
                            }
                            Some(a) if a == f => single = None,
                            Some(a) => return Some((a, f)),
                        }
                    }
                }
                for face in faces.iter().copied() {
                    let internal = if single == Some(face.side) {
                        None
                    } else {
                        available
                    };
                    let connection = self.connection(face)?;
                    let needs_external =
                        connection.flow.map_or(connection.pressure()[0] != 0., |f| {
                            f.inbound || connection.compare_pressure() < 0.
                        });
                    let source = self.connection(face)?.source.clone();
                    let external = if !needs_external {
                        None
                    } else {
                        match source {
                            Some(Source::OtherPipe) => self
                                .connection(face.opposite())
                                .and_then(|c| c.provided_fluid(false)),
                            Some(mut source @ (Source::Handler | Source::Open(_))) => {
                                let gate = match host.cell(pos, face.side) {
                                    Cell::Axis(_, gate) => gate,
                                    _ => Gate::Open,
                                };
                                let fluid = host.provide_fluid(face, &mut source, gate);
                                self.connection_mut(face)?.source = Some(source);
                                fluid
                            }
                            _ => None,
                        }
                    };
                    let allowed = match host.cell(pos, face.side) {
                        Cell::Axis(_, Gate::Closed) => false,
                        Cell::Axis(_, Gate::Only(f)) => external == Some(f),
                        _ => true,
                    };
                    let before = self.connection(face)?.flow;
                    let changed = self
                        .connection_mut(face)?
                        .manage_flows(internal, external.filter(|_| allowed));
                    let after = self.connection(face)?.flow;
                    if changed && before.is_some() && after.is_some() {
                        self.reset_affected_networks(face);
                    }
                    let c = self.connection_mut(face)?;
                    let endpoint = matches!(c.source, Some(Source::Handler | Source::Open(_)));
                    if !changed && c.flow.is_some_and(|f| f.inbound) && endpoint {
                        let mut network = c
                            .network
                            .take()
                            .unwrap_or_else(|| FluidNetwork::new(face, c.source.clone().unwrap()));
                        network.tick(self, host);
                        self.connection_mut(face)?.network = Some(network);
                    } else {
                        c.network = None;
                    }
                }
                for face in faces {
                    self.connection_mut(face)?.tick_flow_progress();
                }
            }
        }
        None
    }
}
/// `FluidNetwork`: queues and frontier are live connection locations; a network
/// waits for complete inbound and outbound half-flows before reaching any target.
#[derive(Clone, Debug, PartialEq)]
pub struct FluidNetwork {
    start: Face,
    pause: u8,
    queued: Vec<Face>,
    frontier: Vec<Face>,
    visited: HashSet<Pos>,
    fluid: Option<Fluid>,
    targets: Vec<(Face, Source)>,
    source_supplier: Source,
    source: Option<Source>,
    transfer_speed: u32,
}
impl FluidNetwork {
    pub fn new(start: Face, source_supplier: Source) -> Self {
        let mut n = Self {
            start,
            pause: 0,
            queued: Vec::new(),
            frontier: Vec::new(),
            visited: HashSet::new(),
            fluid: None,
            targets: Vec::new(),
            transfer_speed: 0,
            source_supplier,
            source: None,
        };
        n.reset();
        n
    }
    pub fn cached_source(&self) -> Option<&Source> {
        self.source.as_ref()
    }
    pub fn reset(&mut self) {
        self.pause = 2;
        self.queued = vec![self.start];
        self.frontier.clear();
        self.visited.clear();
        self.fluid = None;
        self.targets.clear();
    }
    pub fn tick(&mut self, world: &mut Transport, host: &mut impl Host) {
        if self.pause > 0 {
            self.pause -= 1;
            return;
        }
        for _ in 0..16 {
            let mut again = false;
            let mut queued = std::mem::take(&mut self.queued);
            for face in queued.drain(..) {
                if !host.loaded(face.pos) {
                    self.queued.push(face);
                    continue;
                }
                if let Some(c) = world.connection(face) {
                    if face == self.start {
                        self.transfer_speed = super::transfer_speed(c.pressure()[0]);
                    }
                    if !self.frontier.contains(&face) {
                        self.frontier.push(face);
                    }
                }
            }
            let frontier = std::mem::take(&mut self.frontier);
            for face in frontier {
                let Some(c) = world.connection(face) else {
                    continue;
                };
                let Some(flow) = c.flow else {
                    self.frontier.push(face);
                    continue;
                };
                if self.fluid.is_some_and(|f| f != flow.fluid) {
                    continue;
                }
                if !flow.inbound {
                    if c.compare_pressure() < 0. {
                        self.frontier.push(face);
                    }
                    continue;
                }
                if !flow.complete {
                    self.frontier.push(face);
                    continue;
                }
                if self.fluid.is_none() {
                    self.fluid = Some(flow.fluid);
                }
                let mut remove = true;
                for side in Dir::ALL {
                    if side == face.side {
                        continue;
                    }
                    let adjacent = Face {
                        pos: face.pos,
                        side,
                    };
                    let Some(c) = world.connection(adjacent) else {
                        continue;
                    };
                    let Some(out) = c.flow else {
                        if c.has_pressure() && c.pressure()[1] > 0. {
                            remove = false;
                        }
                        continue;
                    };
                    if out.inbound {
                        if c.compare_pressure() > 0. {
                            remove = false;
                        }
                        continue;
                    }
                    if !out.complete {
                        remove = false;
                        continue;
                    }
                    world.determine_source(host, adjacent);
                    let source = world.connection(adjacent).and_then(|c| c.source.clone());
                    if source.is_none() {
                        remove = false;
                        continue;
                    }
                    if matches!(source, Some(Source::Handler | Source::Open(_))) {
                        self.targets.push((adjacent, source.unwrap()));
                        continue;
                    }
                    if self.visited.insert(adjacent.connected()) {
                        self.queued.push(adjacent.opposite());
                        again = true;
                    }
                }
                if !remove {
                    self.frontier.push(face);
                }
            }
            if !again {
                break;
            }
        }
        if self.source.is_none() {
            self.source = Some(self.source_supplier.clone());
        }
        if !self.frontier.is_empty() {
            host.keep_alive(self.start);
        }
        if self.targets.is_empty() {
            return;
        }
        if host.time() % 40 == 0 {
            for (face, source) in &mut self.targets {
                if let Some(current) = world.connection(*face).and_then(|c| c.source.clone())
                    && matches!(&current, Source::Handler | Source::Open(_))
                {
                    *source = current;
                }
            }
        }
        let Some(fluid) = self.fluid else {
            return;
        };
        let source = self.source.as_mut().unwrap();
        let Some((offered, n)) = host.offer(self.start, source, self.transfer_speed, Some(fluid))
        else {
            return;
        };
        if offered != fluid {
            return;
        }
        let mut amount = n;
        for execute in [false, true] {
            let mut left = if execute {
                host.drain(self.start, source, fluid, amount)
            } else {
                amount
            };
            let mut outputs = self.targets.clone();
            #[derive(Clone, Copy, PartialEq, Eq, Hash)]
            enum Capability {
                Open(usize),
                Handler(Pos),
            }
            let mut accumulated = HashMap::<Capability, u32>::new();
            while !outputs.is_empty() && left > 0 {
                let divided = left / outputs.len() as u32;
                let mut remainder = left % outputs.len() as u32;
                outputs.retain_mut(|(face, target)| {
                    let give = divided + u32::from(remainder > 0);
                    remainder = remainder.saturating_sub(1);
                    let identity = match target {
                        Source::Open(open) => Capability::Open(open.identity()),
                        _ => Capability::Handler(host.identity(*face)),
                    };
                    let previous = if execute {
                        0
                    } else {
                        *accumulated.get(&identity).unwrap_or(&0)
                    };
                    let filled = host.fill(*face, target, fluid, give + previous, execute);
                    accumulated.insert(identity, filled);
                    left = left.saturating_sub(filled.saturating_sub(previous));
                    filled >= give + previous
                });
            }
            if !execute {
                amount -= left;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct World {
        cells: HashMap<Pos, Cell>,
        tanks: HashMap<Pos, (Fluid, u32)>,
        kept: u32,
        rpm: Option<f32>,
    }
    impl Host for World {
        fn cell(&self, p: Pos, _: Dir) -> Cell {
            self.cells.get(&p).copied().unwrap_or(Cell::Solid)
        }
        fn time(&self) -> i64 {
            0
        }
        fn speed(&self, _: Pos) -> f32 {
            self.rpm.unwrap_or(32.)
        }
        fn tank_fluids(&self, face: Face) -> Vec<Fluid> {
            self.tanks
                .get(&face.connected())
                .filter(|(_, n)| *n > 0)
                .map(|(f, _)| vec![*f])
                .unwrap_or_default()
        }
        fn offer(
            &mut self,
            f: Face,
            source: &mut Source,
            n: u32,
            fluid: Option<Fluid>,
        ) -> Option<(Fluid, u32)> {
            if let Source::Open(open) = source
                && let Some((f, have)) = open
                    .state()
                    .tank
                    .filter(|(f, _)| fluid.is_none_or(|wanted| wanted == *f))
            {
                return Some((f, have.min(n)));
            }
            self.tanks
                .get(&f.connected())
                .copied()
                .filter(|(f, n)| *n > 0 && fluid.is_none_or(|wanted| wanted == *f))
                .map(|(f, k)| (f, k.min(n)))
        }
        fn drain(&mut self, f: Face, _: &mut Source, _: Fluid, n: u32) -> u32 {
            let tank = self.tanks.get_mut(&f.connected()).unwrap();
            let took = n.min(tank.1);
            tank.1 -= took;
            took
        }
        fn fill(&mut self, f: Face, _: &mut Source, fluid: Fluid, n: u32, execute: bool) -> u32 {
            let t = self.tanks.entry(f.connected()).or_insert((fluid, 0));
            let took = if t.0 == fluid { n.min(8000 - t.1) } else { 0 };
            if execute {
                t.1 += took;
            }
            took
        }
        fn keep_alive(&mut self, _: Face) {
            self.kept += 1;
        }
    }
    fn register(world: &World) -> Transport {
        let mut transport = Transport::default();
        for (&p, &cell) in &world.cells {
            transport.register(p, cell, world.speed(p));
        }
        transport
    }
    #[test]
    fn pump_speed_callbacks_wipe_pressure_before_an_earlier_pipe_turn() {
        let mut world = World::default();
        world.cells.insert([0, 0, 0], Cell::Pump(Dir::East));
        world
            .cells
            .insert([-1, 0, 0], Cell::Pipe(Dir::East.bit() | Dir::West.bit()));
        world.cells.insert([-2, 0, 0], Cell::Open);
        let mut transport = register(&world);
        transport.distribute_pressure_to(&world, [0, 0, 0], Dir::West, true);
        transport.pumps.get_mut(&[0, 0, 0]).unwrap().speed = 32.;
        let face = Face {
            pos: [-1, 0, 0],
            side: Dir::West,
        };
        assert_eq!(transport.connection(face).unwrap().pressure(), [32., 0.]);
        world.rpm = Some(0.);
        transport.tick_pipe(&mut world, face.pos);
        assert_eq!(transport.connection(face).unwrap().pressure(), [0., 0.]);
        assert_eq!(transport.pumps[&[0, 0, 0]].pending, 3);
    }

    #[test]
    fn scheduled_fluid_changes_invalidate_on_the_next_block_tick() {
        let mut world = World::default();
        world
            .cells
            .insert([0, 0, 0], Cell::Pipe(Dir::East.bit() | Dir::West.bit()));
        let mut transport = register(&world);
        let face = Face {
            pos: [0, 0, 0],
            side: Dir::West,
        };
        transport
            .connection_mut(face)
            .unwrap()
            .add_pressure(true, 32.);
        transport.pipes.get_mut(&face.pos).unwrap().phase = UpdatePhase::Idle;
        transport.begin_tick();
        // A LiquidBlock's scheduled tick changes it to air during this tick.
        transport.changed.push(Change::Block([-1, 0, 0]));
        transport.prepare(&world);
        assert_eq!(transport.connection(face).unwrap().pressure(), [32., 0.]);
        assert_eq!(transport.pipes[&face.pos].phase, UpdatePhase::Idle);
        transport.begin_tick();
        transport.prepare(&world);
        assert_eq!(transport.connection(face).unwrap().pressure(), [0., 0.]);
        assert_eq!(transport.pipes[&face.pos].phase, UpdatePhase::WaitForPumps);
    }

    #[test]
    fn a_valves_shaft_neighbour_does_not_invalidate_its_fluid_axis() {
        let mut world = World::default();
        world
            .cells
            .insert([0, 0, 0], Cell::Axis(crate::kinetics::Axis::X, Gate::Open));
        let mut transport = register(&world);
        transport.pipes.get_mut(&[0, 0, 0]).unwrap().phase = UpdatePhase::Idle;
        transport
            .connection_mut(Face {
                pos: [0, 0, 0],
                side: Dir::West,
            })
            .unwrap()
            .add_pressure(true, 32.);
        transport.changed.push(Change::Block([0, 1, 0]));
        transport.begin_tick();
        transport.prepare(&world);
        assert_eq!(transport.pipes[&[0, 0, 0]].phase, UpdatePhase::Idle);
        assert_eq!(
            transport
                .connection(Face {
                    pos: [0, 0, 0],
                    side: Dir::West
                })
                .unwrap()
                .pressure(),
            [32., 0.]
        );
    }

    #[test]
    fn changing_one_pump_side_does_not_wipe_the_other_side() {
        let mut world = World::default();
        world.cells.extend([
            ([-2, 0, 0], Cell::Tank),
            ([-1, 0, 0], Cell::Pipe(Dir::East.bit() | Dir::West.bit())),
            ([0, 0, 0], Cell::Pump(Dir::East)),
            (
                [1, 0, 0],
                Cell::Axis(crate::kinetics::Axis::X, Gate::Only(Fluid::Water)),
            ),
            ([2, 0, 0], Cell::Tank),
        ]);
        let mut transport = register(&world);
        transport.pumps.get_mut(&[0, 0, 0]).unwrap().pending = 0;
        transport.distribute_pressure_to(&world, [0, 0, 0], Dir::West, true);
        transport.pipes.get_mut(&[-1, 0, 0]).unwrap().phase = UpdatePhase::Idle;
        transport.changed.push(Change::Pipe([1, 0, 0]));
        transport.prepare(&world);
        assert_eq!(
            transport
                .connection(Face {
                    pos: [-1, 0, 0],
                    side: Dir::West
                })
                .unwrap()
                .pressure(),
            [32., 0.]
        );
        assert_eq!(transport.pipes[&[-1, 0, 0]].phase, UpdatePhase::Idle);
        assert_eq!(transport.pumps[&[0, 0, 0]].pending, 1);
    }

    #[test]
    fn extraction_filter_does_not_bypass_an_open_ends_retained_fluid() {
        let mut world = World::default();
        world.cells.extend([
            ([-1, 0, 0], Cell::Open),
            (
                [0, 0, 0],
                Cell::Axis(crate::kinetics::Axis::X, Gate::Only(Fluid::Water)),
            ),
        ]);
        world.tanks.insert([-1, 0, 0], (Fluid::Water, 1000));
        let mut transport = register(&world);
        let pipe = transport.pipes.get_mut(&[0, 0, 0]).unwrap();
        pipe.phase = UpdatePhase::Idle;
        let c = pipe.connection(Dir::West).unwrap();
        c.add_pressure(true, 32.);
        c.set_source(Source::open(crate::open_end::OpenEnd {
            tank: Some((Fluid::Honey, 500)),
            pulling: true,
        }));
        transport.tick_pipe(&mut world, [0, 0, 0]);
        assert!(
            transport
                .connection(Face {
                    pos: [0, 0, 0],
                    side: Dir::West
                })
                .unwrap()
                .flow
                .is_none()
        );
    }

    #[test]
    fn a_lone_input_cannot_supply_its_own_outward_flow() {
        let mut host = World::default();
        host.cells.extend([
            ([-1, 0, 0], Cell::Tank),
            ([0, 0, 0], Cell::Pipe(Dir::West.bit())),
        ]);
        host.tanks.insert([-1, 0, 0], (Fluid::Water, 1000));
        let mut transport = register(&host);
        let pipe = transport.pipes.get_mut(&[0, 0, 0]).unwrap();
        pipe.phase = UpdatePhase::Idle;
        let c = pipe.connection(Dir::West).unwrap();
        c.set_pressure([32., 64.]);
        c.flow = Some(super::super::transport::Flow {
            fluid: Fluid::Water,
            inbound: true,
            progress: 1.,
            complete: true,
        });
        transport.tick_pipe(&mut host, [0, 0, 0]);
        assert!(
            transport
                .connection(Face {
                    pos: [0, 0, 0],
                    side: Dir::West
                })
                .unwrap()
                .flow
                .unwrap()
                .inbound
        );
    }

    #[test]
    fn network_source_and_previous_source_share_one_open_tank_across_resets() {
        let face = Face {
            pos: [0, 0, 0],
            side: Dir::West,
        };
        let mut c = super::super::transport::PipeConnection::default();
        c.set_source(Source::open(crate::open_end::OpenEnd {
            tank: Some((Fluid::Water, 984)),
            pulling: true,
        }));
        let mut network = FluidNetwork::new(face, c.source.clone().unwrap());
        network.source = Some(network.source_supplier.clone());
        c.network = Some(network);
        c.wipe_pressure();
        c.set_source(Source::Open(Default::default()));
        let Some(Source::Open(open)) = &c.source else {
            panic!()
        };
        let Some(Source::Open(cached)) = c.network.as_ref().unwrap().cached_source() else {
            panic!()
        };
        assert_eq!(open.identity(), cached.identity());
        assert_eq!(c.open_ends().len(), 1);
        c.set_source(Source::Handler);
        c.wipe_pressure();
        // previousSource now refers to the handler; the network alone retains
        // the old OpenEndedPipe, exactly as FluidNetwork.reset in Java does.
        assert_eq!(
            c.open_ends(),
            vec![crate::open_end::OpenEnd {
                tank: Some((Fluid::Water, 984)),
                pulling: true
            }]
        );
    }

    #[test]
    fn pump_refreshes_its_pressure_even_when_its_flows_collide() {
        let mut world = World::default();
        world.cells.extend([
            ([-1, 0, 0], Cell::Tank),
            ([0, 0, 0], Cell::Pump(Dir::East)),
            ([1, 0, 0], Cell::Tank),
        ]);
        let mut transport = register(&world);
        transport.pumps.get_mut(&[0, 0, 0]).unwrap().speed = 32.;
        let pipe = transport.pipes.get_mut(&[0, 0, 0]).unwrap();
        pipe.phase = UpdatePhase::Idle;
        for (side, c) in &mut pipe.interfaces {
            c.flow = Some(super::super::transport::Flow {
                fluid: if *side == Dir::East {
                    Fluid::Lava
                } else {
                    Fluid::Water
                },
                inbound: true,
                progress: 1.,
                complete: true,
            });
        }
        assert!(transport.tick_pipe(&mut world, [0, 0, 0]).is_some());
        assert_eq!(
            transport
                .connection(Face {
                    pos: [0, 0, 0],
                    side: Dir::East
                })
                .unwrap()
                .pressure(),
            [0., 32.]
        );
    }

    #[test]
    fn unloading_an_earlier_column_does_not_redistribute_restored_pressure() {
        let _guard = crate::states::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::states::set_range(10, 1);
        // Issue #73: Java restores 128, not 256, between the two 128 RPM pumps.
        // SmartBlockEntity.onChunkUnloaded suppresses remove(); it does not
        // propagate a changed pipe into the columns still awaiting their unload.
        let mut host = World {
            rpm: Some(128.),
            ..World::default()
        };
        host.cells.extend([
            ([2, 4, 0], Cell::Pump(Dir::East)),
            ([3, 4, 0], Cell::Pipe(Dir::East.bit() | Dir::West.bit())),
            ([4, 4, 0], Cell::Pump(Dir::East)),
        ]);
        let mut transport = register(&host);
        for pump in transport.pumps.values_mut() {
            pump.speed = 128.;
            pump.pending = 0;
        }
        transport.distribute_pressure_to(&host, [2, 4, 0], Dir::East, false);
        let mut world = crate::world::CreateWorld::new(crate::world::StateTable::new(
            10,
            [("create:shaft", [("axis", "x")].into_iter())].into_iter(),
        ));
        world.ensure_dimension(0);
        world.block_loaded(0, [2, 4, -1], 10);
        world.dims[0].fluid_transport = transport;
        world.column_unloaded(0, 0, -1);
        crate::states::clear_range();
        let mut transport = std::mem::take(&mut world.dims[0].fluid_transport);
        transport.prepare(&host);
        for pos in [[2, 4, 0], [3, 4, 0], [4, 4, 0]] {
            transport.tick_pipe(&mut host, pos);
        }
        for (side, expected) in [(Dir::West, [128., 0.]), (Dir::East, [0., 128.])] {
            assert_eq!(
                transport
                    .connection(Face {
                        pos: [3, 4, 0],
                        side
                    })
                    .unwrap()
                    .pressure(),
                expected
            );
        }
    }

    #[test]
    fn cold_load_keeps_the_saved_shared_pressure_without_redistributing_it() {
        let mut world = World::default();
        world.cells.extend([
            ([0, 0, 0], Cell::Pump(Dir::East)),
            ([1, 0, 0], Cell::Pipe(Dir::West.bit() | Dir::East.bit())),
            ([2, 0, 0], Cell::Pump(Dir::East)),
        ]);
        let mut live = register(&world);
        live.distribute_pressure_to(&world, [0, 0, 0], Dir::East, false);
        live.distribute_pressure_to(&world, [2, 0, 0], Dir::West, true);
        let mut restored = Transport::default();
        for (&pos, pipe) in &live.pipes {
            restored.pipes.insert(
                pos,
                FluidTransportBehaviour::read(&pipe.write(), sides(world.cell(pos, Dir::Down))),
            );
        }
        for pos in [[0, 0, 0], [1, 0, 0], [2, 0, 0]] {
            restored.register(pos, world.cell(pos, Dir::Down), world.speed(pos));
        }
        restored.prepare(&world);
        for pos in [[0, 0, 0], [1, 0, 0], [2, 0, 0]] {
            restored.tick_pipe(&mut world, pos);
        }
        assert_eq!(
            restored
                .connection(Face {
                    pos: [1, 0, 0],
                    side: Dir::West
                })
                .unwrap()
                .pressure(),
            [64., 0.]
        );
        assert_eq!(
            restored
                .connection(Face {
                    pos: [1, 0, 0],
                    side: Dir::East
                })
                .unwrap()
                .pressure(),
            [0., 64.]
        );
    }

    #[test]
    fn two_pumps_add_sixty_four_pressure_to_the_shared_pipe() {
        // Java issue #62: each 32 RPM pump contributes 32 to the same channels.
        let mut world = World::default();
        world.cells.extend([
            ([0, 0, 0], Cell::Pump(Dir::East)),
            ([1, 0, 0], Cell::Pipe(Dir::East.bit() | Dir::West.bit())),
            ([2, 0, 0], Cell::Pump(Dir::East)),
        ]);
        let mut transport = register(&world);
        transport.distribute_pressure_to(&world, [0, 0, 0], Dir::East, false);
        transport.distribute_pressure_to(&world, [2, 0, 0], Dir::West, true);
        assert_eq!(
            transport
                .connection(Face {
                    pos: [1, 0, 0],
                    side: Dir::West
                })
                .unwrap()
                .pressure(),
            [64., 0.]
        );
        assert_eq!(
            transport
                .connection(Face {
                    pos: [1, 0, 0],
                    side: Dir::East
                })
                .unwrap()
                .pressure(),
            [0., 64.]
        );
    }
    #[test]
    fn range_boundary_pressurizes_the_reachable_prefix_without_an_endpoint() {
        let mut world = World::default();
        world.cells.insert([0, 0, 0], Cell::Pump(Dir::East));
        for x in 1..=17 {
            world
                .cells
                .insert([x, 0, 0], Cell::Pipe(Dir::East.bit() | Dir::West.bit()));
        }
        world.cells.insert([18, 0, 0], Cell::Tank);
        let mut transport = register(&world);
        transport.distribute_pressure_to(&world, [0, 0, 0], Dir::East, false);
        assert_eq!(
            transport
                .connection(Face {
                    pos: [15, 0, 0],
                    side: Dir::East
                })
                .unwrap()
                .pressure(),
            [0., 32.]
        );
        assert_eq!(
            transport
                .connection(Face {
                    pos: [16, 0, 0],
                    side: Dir::East
                })
                .unwrap()
                .pressure(),
            [0.; 2]
        );
    }
    #[test]
    fn network_waits_for_both_halves_before_transfer_and_keeps_source_engaged() {
        // Issue #61's observed 32 RPM progression: 0.2734375 per tick,
        // six half-flows; the final outward half must complete before delivery.
        let mut world = World::default();
        let line = Cell::Pipe(Dir::East.bit() | Dir::West.bit());
        world.cells.extend([
            ([-2, 0, 0], Cell::Tank),
            ([-1, 0, 0], line),
            ([0, 0, 0], Cell::Pump(Dir::East)),
            ([1, 0, 0], line),
            ([2, 0, 0], Cell::Tank),
        ]);
        world.tanks.insert([-2, 0, 0], (Fluid::Water, 1000));
        let mut transport = register(&world);
        transport.pumps.get_mut(&[0, 0, 0]).unwrap().speed = 32.;
        let mut first = None;
        for tick in 0..30 {
            transport.prepare(&world);
            for p in [[-1, 0, 0], [0, 0, 0], [1, 0, 0]] {
                transport.tick_pipe(&mut world, p);
            }
            if world.tanks.get(&[2, 0, 0]).is_some_and(|(_, n)| *n > 0) && first.is_none() {
                first = Some(tick);
            }
            assert_eq!(world.tanks.values().map(|(_, n)| n).sum::<u32>(), 1000);
        }
        assert_eq!(first, Some(24));
        assert!(world.kept > 0);
        assert!(
            transport
                .connection(Face {
                    pos: [1, 0, 0],
                    side: Dir::East
                })
                .unwrap()
                .flow
                .unwrap()
                .complete
        );
    }
}
