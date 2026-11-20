//! Common server-save semantics; render randomness and host-specific layouts are excluded.
use crate::{program, scenario::Pos, transport::Stack};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Payload {
    Sequencer {
        instructions: Vec<program::Instruction>,
        current: i32,
        duration: i32,
        progress: f32,
        timer: i32,
        powered: bool,
    },
    Valve {
        in_use: u8,
        backwards: bool,
        start_angle: i32,
        selected: i32,
        total: i32,
        target: i32,
    },
    Crank {
        in_use: u8,
        backwards: bool,
    },
    Belt {
        controller: Pos,
        length: i32,
        index: i32,
        casing: String,
        inventory: Option<Inventory>,
    },
    Depot {
        held: Option<Stack>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Inventory {
    pub positive: bool,
    pub items: Vec<Item>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub stack: Stack,
    pub position: f32,
    pub locked: bool,
}

pub fn equal(a: &Option<Payload>, b: &Option<Payload>) -> bool {
    fn eq(a: &serde_json::Value, b: &serde_json::Value) -> bool {
        use serde_json::Value;
        match (a, b) {
            (Value::Number(a), Value::Number(b)) => {
                let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
                (a - b).abs() <= 0.001 * a.abs().max(b.abs()).max(1.)
            }
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| eq(a, b))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|b| eq(v, b)))
            }
            _ => a == b,
        }
    }
    eq(
        &serde_json::to_value(a).unwrap(),
        &serde_json::to_value(b).unwrap(),
    )
}
