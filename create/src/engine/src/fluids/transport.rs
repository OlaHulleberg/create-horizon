//! Connection-owned transport state, following Create's `PipeConnection` and
//! `FluidTransportBehaviour`. World source discovery and Layer III networks are
//! supplied by the caller; they are not endpoint-path projections.

use crate::kinetics::Dir;
use crate::open_end::OpenEnd;
use crate::tank::Fluid;
use std::{cell::RefCell, rc::Rc};

/// `FlowSource`: only open endpoints carry persistent state.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Open(OpenSource),
    Handler,
    OtherPipe,
    Blocked,
}

/// The same OpenEndedPipe can be retained by previousSource and a FluidNetwork.
/// Sharing its tank preserves Java's source identity through pressure resets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenSource(Rc<RefCell<OpenEnd>>);
impl OpenSource {
    pub fn state(&self) -> OpenEnd {
        *self.0.borrow()
    }
    pub fn identity(&self) -> usize {
        Rc::as_ptr(&self.0) as usize
    }
    pub fn drain(
        &self,
        space: &mut impl crate::open_end::Space,
        n: u32,
        only: Option<Fluid>,
        execute: bool,
    ) -> Option<(Fluid, u32)> {
        self.0.borrow_mut().drain(space, n, only, execute)
    }
    pub fn fill(
        &self,
        space: &mut impl crate::open_end::Space,
        fluid: Fluid,
        n: u32,
        execute: bool,
    ) -> u32 {
        self.0.borrow_mut().fill(space, fluid, n, execute)
    }
}
impl Source {
    pub fn open(end: OpenEnd) -> Self {
        Self::Open(OpenSource(Rc::new(RefCell::new(end))))
    }
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
        self.open_ends().into_iter().next()
    }

    /// Deduplicate source, previousSource and the network's cached handler by
    /// identity, as a Java observation must do for shared OpenEndedPipe objects.
    pub fn open_ends(&self) -> Vec<OpenEnd> {
        let mut seen = Vec::new();
        let mut out = Vec::new();
        for source in [
            self.source.as_ref(),
            self.previous_source.as_ref(),
            self.network.as_ref().and_then(|n| n.cached_source()),
        ]
        .into_iter()
        .flatten()
        {
            if let Source::Open(open) = source
                && !seen.contains(&open.identity())
            {
                seen.push(open.identity());
                out.push(open.state());
            }
        }
        out
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
            self.previous_source = self.source.clone();
        }
        self.source = None;
    }

    /// `determineSource` reuses a previous open end when this side is still open.
    pub fn set_source(&mut self, source: Source) {
        self.source = Some(match (source, self.previous_source.clone()) {
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
            open_end: match &self.source {
                Some(Source::Open(end)) => Some(end.state()),
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
        self.source = saved.open_end.map(Source::open);
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
        c.set_source(Source::open(end));
        c.add_pressure(true, 32.0);
        c.manage_flows(None, Some(Fluid::Water));
        c.wipe_pressure();
        assert_eq!(c.pressure(), [0.0; 2]);
        assert!(c.flow.is_some());
        c.set_source(Source::open(OpenEnd::default()));
        assert_eq!(c.source, Some(Source::open(end)));
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
