//! Normalized fluid transport observations; raw ids and serialized BE layout stay local.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    #[serde(default)]
    pub pressure_model: String,
    pub connections: BTreeMap<String, Connection>,
    pub pump_pressure: Option<f32>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    #[serde(default)]
    pub buffer: Buffer,
    pub pressure: [f32; 2],
    pub flow: Option<Flight>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Flight {
    pub fluid: String,
    pub inbound: bool,
    pub progress: f32,
    pub complete: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Buffer {
    pub fluids: BTreeMap<String, u32>,
}
#[cfg(test)]
impl Buffer {
    pub fn amount(&self) -> u32 {
        self.fluids.values().sum()
    }
}
