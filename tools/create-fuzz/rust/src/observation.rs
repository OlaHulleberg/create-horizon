//! Shared normalized semantic observations, separate from scenario actions.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What both sides write after the last tick.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    /// Native reference frame for reconstructing the host's rounded hit point.
    #[serde(default)]
    pub origin: crate::scenario::Pos,
    #[serde(default)]
    pub save_phase: u8,
    #[serde(default)]
    pub item_outcomes: Vec<crate::transport::Outcome>,
    pub side: String,
    /// Adapter gaps are explicit metadata, never inferred from equal speed/stress.
    #[serde(default)]
    pub unavailable: Vec<String>,
    #[serde(default)]
    pub networks: Option<Networks>,
    /// Every non-air block at a scenario position, keyed `x,y,z`.
    pub blocks: BTreeMap<String, BlockSnap>,
    /// Notes from the side that ran it (errors, skipped parts).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub checkpoints: BTreeMap<u32, Snapshot>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct BlockSnap {
    #[serde(default)]
    pub fluid_storage: Option<crate::fluid_machines::Storage>,
    #[serde(default)]
    pub fluid_machine: Option<crate::fluid_machines::Machine>,
    #[serde(default)]
    pub fluid_filter: Option<String>,
    #[serde(default)]
    pub fluid_network: Option<crate::fluid_network::Observation>,
    #[serde(default)]
    pub world_fluid: Option<crate::fluid_world::Observation>,
    #[serde(default)]
    pub payload: Option<crate::persistence::payload::Payload>,
    #[serde(default)]
    pub saved: Option<crate::persistence::Saved>,
    pub fluid_tank: Option<crate::fluids::Tank>,
    #[serde(default)]
    pub transport: Option<crate::transport::Contents>,
    #[serde(default)]
    pub conveyor_links: Option<Vec<crate::scenario::Pos>>,
    #[serde(default)]
    pub belt: Option<crate::belts::Observation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel: Option<crate::logistics::Tunnel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub funnel: Option<crate::logistics::Funnel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chute: Option<crate::logistics::Chute>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ejector: Option<crate::logistics::Ejector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured_trig: Option<crate::allow::MeasuredTrigState>,
    #[serde(default)]
    pub bracket: Option<String>,
    pub state: String,
    /// Present for kinetic blocks only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinetic: Option<Kinetic>,
    /// The Java block entity's saved data, to read mismatches by; not compared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Kinetic {
    /// `getSpeed()`: 0 when overstressed.
    pub speed: f32,
    #[serde(default)]
    pub sequencer: Option<SequenceRun>,
    #[serde(default)]
    pub valve: Option<crate::handles::Observation>,
    #[serde(default)]
    pub speed_setting: Option<f32>,
    #[serde(default)]
    pub chain_modifier: Option<f32>,
    /// `getTheoreticalSpeed()`.
    pub theoretical: f32,
    pub overstressed: bool,
    /// The network's stress and capacity as the block holds them (`stress`, `capacity`).
    pub stress: f32,
    pub capacity: f32,
    /// Its own stress impact and capacity per rpm (`calculateStressApplied`,
    /// `calculateAddedStressCapacity`).
    pub impact: f32,
    pub added_capacity: f32,
    /// `getGeneratedSpeed()`.
    pub generated: f32,
    pub has_source: bool,
    #[serde(default)]
    pub comparator: Option<i32>,
    #[serde(default)]
    pub sequence_angle: Option<f32>,
    #[serde(default)]
    pub sequence_distance: Option<f32>,
    /// A clutch's or gearshift's halves: the speed each shaft face turns at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faces: Option<BTreeMap<String, f32>>,
}

/// Network identity normalized to sorted position partitions; no runtime ids cross this interface.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Networks {
    pub partitions: Vec<Vec<String>>,
    pub sizes: BTreeMap<String, i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SequenceRun {
    pub current: i32,
    pub duration: i32,
    pub progress: f32,
    pub timer: i32,
    pub powered_before: bool,
}
