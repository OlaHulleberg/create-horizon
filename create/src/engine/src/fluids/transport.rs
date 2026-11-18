//! Connection-owned transport state, following Create's `PipeConnection` and
//! `FluidTransportBehaviour`. World source discovery and Layer III networks are
//! supplied by the caller; they are not endpoint-path projections.

use crate::kinetics::Dir;
use crate::open_end::OpenEnd;
use crate::tank::Fluid;

/// `FlowSource`: only open endpoints carry persistent state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    Open(OpenEnd),
    Handler,
    OtherPipe,
    Blocked,
}

/// `PipeConnection.Flow`; progress belongs to this half-pipe, not to a pump clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flow {
    pub fluid: Fluid,
    pub inbound: bool,
    pub progress: f32,
    pub complete: bool,
}

/// Contents of a side's compound in `PipeConnection.serializeNBT`.
/// `pressure` corresponds to optional `Pressure` [inbound, outward]; absent
/// `Progress` means a completed flow. Runtime sources other than OpenEnd are omitted.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SavedConnection {
    pub pressure: Option<[f32; 2]>,
    pub open_end: Option<OpenEnd>,
    pub flow: Option<SavedFlow>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedFlow {
    pub fluid: Fluid,
    pub inbound: bool,
    pub progress: Option<f32>,
}

/// One `PipeConnection`, owned by a transport behaviour's side.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PipeConnection {
    pressure: [f32; 2],
    pub flow: Option<Flow>,
    pub source: Option<Source>,
    previous_source: Option<Source>,
    pub network: Option<super::runtime::FluidNetwork>,
}

impl PipeConnection {
    /// Open-ended source retained across wipePressure for reuse. Observers must
    /// count this tank even during WAIT_FOR_PUMPS, as Java's previousSource does.
    pub fn open_end(&self) -> Option<OpenEnd> {
        match self.source.or(self.previous_source) {
            Some(Source::Open(e)) => Some(e),
            _ => None,
        }
    }

    pub fn set_pressure(&mut self, pressure: [f32; 2]) {
        self.pressure = pressure;
    }

    pub fn pressure(&self) -> [f32; 2] {
        self.pressure
    }

    pub fn has_pressure(&self) -> bool {
        self.pressure != [0.0; 2]
    }

    pub fn compare_pressure(&self) -> f32 {
        self.pressure[1] - self.pressure[0]
    }

    /// `addPressure`: contributions accumulate independently in both directions.
    pub fn add_pressure(&mut self, inbound: bool, amount: f32) {
        self.pressure[usize::from(!inbound)] += amount;
    }

    /// `wipePressure` preserves flows and the previous source. Network reset is
    /// the caller's responsibility; it must not discard an open end's tank.
    pub fn wipe_pressure(&mut self) {
        self.pressure = [0.0; 2];
        if let Some(n) = &mut self.network {
            n.reset();
        }
        if self.source.is_some() {
            self.previous_source = self.source;
        }
        self.source = None;
    }

    /// `determineSource` reuses a previous open end when this side is still open.
    pub fn set_source(&mut self, source: Source) {
        self.source = Some(match (source, self.previous_source) {
            (Source::Open(_), Some(previous @ Source::Open(_))) => previous,
            (source, _) => source,
        });
    }

    /// `getProvidedFluid` / `provideOutboundFlow`.
    pub fn provided_fluid(&self, inbound: bool) -> Option<Fluid> {
        self.flow
            .filter(|f| f.inbound == inbound && f.complete)
            .map(|f| f.fluid)
    }

    /// `flipFlowsIfPressureReversed`: only a single nonzero pressure flips here.
    pub fn flip_flows(&mut self) -> bool {
        let comparison = self.compare_pressure();
        let single = comparison != 0.0 && self.pressure.contains(&0.0);
        let Some(flow) = &mut self.flow else {
            return false;
        };
        if !single || (comparison < 0.0) == flow.inbound {
            return false;
        }
        flow.inbound = !flow.inbound;
        if !flow.complete {
            self.flow = None;
        }
        true
    }

    /// Layer II of `manageFlows`. `external` is the source's filtered fluid;
    /// `internal` excludes this connection's own provided input. A missing source
    /// means an unloaded neighbour and defers management, just as determineSource.
    pub fn manage_flows(&mut self, internal: Option<Fluid>, external: Option<Fluid>) -> bool {
        if self.source.is_none() {
            return false;
        }
        if let Some(flow) = self.flow {
            let provided = if flow.inbound { external } else { internal };
            if !self.has_pressure() || provided != Some(flow.fluid) {
                self.flow = None;
                return true;
            }
            if flow.inbound != (self.compare_pressure() < 0.0) {
                let inbound = !flow.inbound;
                if (inbound && provided.is_some()) || (!inbound && internal.is_some()) {
                    let fluid = if inbound { external } else { internal };
                    if let Some(fluid) = fluid {
                        self.start_flow(inbound, fluid);
                    }
                    return true;
                }
            }
            return false;
        }
        if !self.has_pressure() {
            return false;
        }
        let prioritize_inbound = self.compare_pressure() < 0.0;
        for inbound in [prioritize_inbound, !prioritize_inbound] {
            if self.pressure[usize::from(!inbound)] == 0.0 {
                continue;
            }
            if let Some(fluid) = if inbound { external } else { internal } {
                self.start_flow(inbound, fluid);
                return true;
            }
        }
        false
    }

    fn start_flow(&mut self, inbound: bool, fluid: Fluid) {
        self.flow = Some(Flow {
            fluid,
            inbound,
            progress: 0.0,
            complete: false,
        });
    }

    /// `tickFlowProgress` reads the current stored pressure every tick.
    pub fn tick_flow_progress(&mut self) {
        if let Some(flow) = &mut self.flow {
            flow.progress = (flow.progress
                + super::flow_speed(self.pressure[usize::from(!flow.inbound)]))
            .min(1.0);
            if flow.progress >= 1.0 {
                flow.complete = true;
            }
        }
    }

    pub fn write(&self) -> SavedConnection {
        SavedConnection {
            pressure: self.has_pressure().then_some(self.pressure),
            open_end: match self.source {
                Some(Source::Open(end)) => Some(end),
                _ => None,
            },
            flow: self.flow.map(|f| SavedFlow {
                fluid: f.fluid,
                inbound: f.inbound,
                progress: (!f.complete).then_some(f.progress),
            }),
        }
    }

    /// `deserializeNBT` clears absent fields and never restores runtime sources.
    pub fn read(&mut self, saved: SavedConnection) {
        self.pressure = saved.pressure.unwrap_or_default();
        self.source = saved.open_end.map(Source::Open);
        self.flow = saved.flow.map(|f| Flow {
            fluid: f.fluid,
            inbound: f.inbound,
            progress: f.progress.unwrap_or(1.0),
            complete: f.progress.is_none(),
        });
    }
}

/// `FluidTransportBehaviour.UpdatePhase` is deliberately transient on load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UpdatePhase {
    #[default]
    WaitForPumps,
    FlipFlows,
    Idle,
}

/// A pipe's own interfaces. Missing sides cannot receive pressure.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FluidTransportBehaviour {
    pub interfaces: Vec<(Dir, PipeConnection)>,
    pub phase: UpdatePhase,
}

impl FluidTransportBehaviour {
    pub fn new(sides: u8) -> Self {
        Self {
            interfaces: Dir::ALL
                .into_iter()
                .filter(|d| sides & d.bit() != 0)
                .map(|d| (d, PipeConnection::default()))
                .collect(),
            phase: UpdatePhase::WaitForPumps,
        }
    }

    pub fn connection(&mut self, side: Dir) -> Option<&mut PipeConnection> {
        self.interfaces
            .iter_mut()
            .find(|(d, _)| *d == side)
            .map(|(_, c)| c)
    }

    pub fn add_pressure(&mut self, side: Dir, inbound: bool, amount: f32) {
        if let Some(connection) = self.connection(side) {
            connection.add_pressure(inbound, amount);
        }
    }

    /// `wipePressure` reconciles interfaces with current block openings first.
    pub fn wipe_pressure(&mut self, sides: u8) {
        self.interfaces.retain(|(d, _)| sides & d.bit() != 0);
        for side in Dir::ALL {
            if sides & side.bit() != 0 && self.connection(side).is_none() {
                self.interfaces.push((side, PipeConnection::default()));
            }
        }
        self.phase = UpdatePhase::WaitForPumps;
        for (_, c) in &mut self.interfaces {
            c.wipe_pressure();
        }
    }

    /// Layer II of `FluidTransportBehaviour.tick`. Source discovery must precede
    /// this call. External inputs are already filtered by canPullFluidFrom.
    /// Returns the two colliding fluids for the world's reaction handler.
    pub fn tick(&mut self, external: impl Fn(Dir) -> Option<Fluid>) -> Option<(Fluid, Fluid)> {
        if self.phase == UpdatePhase::WaitForPumps {
            self.phase = UpdatePhase::FlipFlows;
            return None;
        }
        for (_, c) in &mut self.interfaces {
            c.flip_flows();
        }
        if self.phase == UpdatePhase::FlipFlows {
            self.phase = UpdatePhase::Idle;
            return None;
        }
        let mut available = None;
        let mut single = None;
        for (side, c) in &self.interfaces {
            if let Some(fluid) = c.provided_fluid(true) {
                match available {
                    None => {
                        available = Some(fluid);
                        single = Some(*side);
                    }
                    Some(previous) if previous == fluid => single = None,
                    Some(previous) => return Some((previous, fluid)),
                }
            }
        }
        for (side, c) in &mut self.interfaces {
            let internal = if single == Some(*side) {
                None
            } else {
                available
            };
            c.manage_flows(internal, external(*side));
        }
        for (_, c) in &mut self.interfaces {
            c.tick_flow_progress();
        }
        None
    }

    pub fn write(&self) -> Vec<(Dir, SavedConnection)> {
        self.interfaces
            .iter()
            .map(|(d, c)| (*d, c.write()))
            .collect()
    }

    /// Saved side compounds define the interfaces; empty legacy data defers to
    /// the caller's runtime openings, as `FluidTransportBehaviour.read` does.
    pub fn read(saved: &[(Dir, SavedConnection)], sides: u8) -> Self {
        if saved.is_empty() {
            return Self::new(sides);
        }
        Self {
            interfaces: saved
                .iter()
                .map(|(d, s)| {
                    let mut c = PipeConnection::default();
                    c.read(*s);
                    (*d, c)
                })
                .collect(),
            phase: UpdatePhase::WaitForPumps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behaviour_waits_for_pumps_and_does_not_feed_a_lone_input_to_itself() {
        let mut b = FluidTransportBehaviour::new(Dir::East.bit() | Dir::West.bit());
        for (side, c) in &mut b.interfaces {
            c.set_source(Source::OtherPipe);
            c.add_pressure(*side == Dir::West, 128.0);
        }
        let external = |side| (side == Dir::West).then_some(Fluid::Water);
        b.tick(external);
        assert!(b.interfaces.iter().all(|(_, c)| c.flow.is_none()));
        b.tick(external);
        assert!(b.interfaces.iter().all(|(_, c)| c.flow.is_none()));
        b.tick(external);
        assert_eq!(
            b.connection(Dir::West).unwrap().provided_fluid(true),
            Some(Fluid::Water)
        );
        assert!(b.connection(Dir::East).unwrap().flow.is_none());
        b.tick(external);
        assert_eq!(
            b.connection(Dir::East).unwrap().provided_fluid(false),
            Some(Fluid::Water)
        );
    }

    #[test]
    fn pressure_accumulates_and_changes_speed_mid_flow() {
        let mut c = PipeConnection::default();
        c.set_source(Source::Handler);
        c.add_pressure(true, 16.0);
        c.add_pressure(true, 16.0);
        c.add_pressure(false, 8.0);
        assert_eq!(c.pressure(), [32.0, 8.0]);
        assert!(c.manage_flows(None, Some(Fluid::Water)));
        c.tick_flow_progress();
        assert_eq!(c.flow.unwrap().progress, 0.2734375);
        c.add_pressure(true, 96.0);
        c.tick_flow_progress();
        assert!(c.flow.unwrap().complete);
        assert_eq!(c.provided_fluid(true), Some(Fluid::Water));
    }

    #[test]
    fn wipe_retains_flow_and_open_end_until_flow_management() {
        let mut c = PipeConnection::default();
        let end = OpenEnd {
            tank: Some((Fluid::Water, 500)),
            pulling: true,
        };
        c.set_source(Source::Open(end));
        c.add_pressure(true, 32.0);
        c.manage_flows(None, Some(Fluid::Water));
        c.wipe_pressure();
        assert_eq!(c.pressure(), [0.0; 2]);
        assert!(c.flow.is_some());
        c.set_source(Source::Open(OpenEnd::default()));
        assert_eq!(c.source, Some(Source::Open(end)));
        assert!(c.manage_flows(None, Some(Fluid::Water)));
        assert!(c.flow.is_none());
    }

    #[test]
    fn save_preserves_both_pressures_and_omits_completed_progress() {
        let mut b = FluidTransportBehaviour::new(Dir::East.bit());
        b.add_pressure(Dir::East, true, 128.0);
        b.add_pressure(Dir::East, false, 16.0);
        b.add_pressure(Dir::West, true, 64.0);
        let c = b.connection(Dir::East).unwrap();
        c.set_source(Source::Handler);
        c.manage_flows(None, Some(Fluid::Lava));
        c.tick_flow_progress();
        let saved = b.write();
        assert_eq!(saved[0].1.pressure, Some([128.0, 16.0]));
        assert_eq!(saved[0].1.flow.unwrap().progress, None);
        let loaded = FluidTransportBehaviour::read(&saved, 0);
        assert_eq!(loaded.write(), saved);
        assert_eq!(loaded.phase, UpdatePhase::WaitForPumps);
        assert_eq!(loaded.interfaces[0].1.source, None);
    }
}
