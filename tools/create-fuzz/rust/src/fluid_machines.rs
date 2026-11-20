//! Semantic fluid-handler storage and item machine state.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Storage {
    pub capacity: u32,
    pub fluid: Option<String>,
    pub amount: u32,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    pub processing: i32,
    pub held: Option<Held>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Held {
    pub item: String,
    pub count: u8,
    pub position: f32,
    pub from: String,
}
