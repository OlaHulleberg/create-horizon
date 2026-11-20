//! Semantic observations of the logistics blocks that meet belts: a tunnel's flaps and sides
//! (`BeltTunnelBlockEntity.updateTunnelConnections`), a funnel's transfer cooldown
//! (`FunnelBlockEntity.extractionCooldown`) and a chute's item and its position along the chute.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tunnel {
    /// Sides with a flap, by name, sorted.
    pub flaps: Vec<String>,
    /// Open sides (`sides`), by name, sorted.
    pub sides: Vec<String>,
    /// A brass tunnel's distribution state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brass: Option<Brass>,
}

/// `BrassTunnelBlockEntity`: the stack it holds to hand out and the side it came in by
/// (`stackToDistribute`, `stackEnteredFrom`), `distributionProgress`, `previousOutputIndex`,
/// `syncedOutputActive`, `connectedLeft`/`connectedRight`, the selection mode (its name, lower
/// case) and the sided filters (side name to item id).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Brass {
    pub stack: Option<crate::transport::Stack>,
    pub entered_from: Option<String>,
    /// Only while it holds a stack or hands out with its group (`tick` reads it then).
    pub progress: Option<f32>,
    pub previous: i32,
    pub synced: bool,
    pub connected_left: bool,
    pub connected_right: bool,
    pub mode: String,
    pub filters: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Funnel {
    pub cooldown: i32,
    /// A brass funnel's filter (an item id, None for an empty slot).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// A brass funnel's amount (`FilteringBehaviour.count` within the filter's stack size) and
    /// mode (`upTo`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up_to: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chute {
    pub item: Option<crate::transport::Stack>,
    pub position: f32,
}

/// Direction names as Java serializes them, sorted.
pub fn names(dirs: impl IntoIterator<Item = create_engine::kinetics::Dir>) -> Vec<String> {
    let mut v: Vec<String> = dirs
        .into_iter()
        .map(|d| format!("{d:?}").to_lowercase())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// `EjectorBlockEntity`: its state (`charged`, `launching`, `retracting`), power, plate
/// (`lidProgress`), launcher distances, stack size (`maxStackSize`), the stacks in flight and the
/// time an obstacle stops them at (`earlyTargetTime`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ejector {
    pub state: String,
    pub powered: bool,
    pub lid: f32,
    pub h: i32,
    pub v: i32,
    pub max_stack: i32,
    pub flying: Vec<Flying>,
    pub early_time: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Flying {
    pub time: i32,
    pub stack: Option<crate::transport::Stack>,
}

/// Ejector plates and obstacle times compare within the transport tolerance.
pub fn ejector_equal(a: Option<&Ejector>, b: Option<&Ejector>) -> bool {
    let near = |x: f32, y: f32| (x - y).abs() <= 1e-4;
    match (a, b) {
        (Some(a), Some(b)) => {
            (&a.state, a.powered, a.h, a.v, a.max_stack, &a.flying)
                == (&b.state, b.powered, b.h, b.v, b.max_stack, &b.flying)
                && near(a.lid, b.lid)
                && match (a.early_time, b.early_time) {
                    (Some(x), Some(y)) => near(x, y),
                    (x, y) => x.is_none() && y.is_none(),
                }
        }
        (None, None) => true,
        _ => false,
    }
}

/// Chute positions compare within the transport tolerance.
pub fn chute_equal(a: Option<&Chute>, b: Option<&Chute>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.item == b.item && (a.position - b.position).abs() <= 1e-4,
        (None, None) => true,
        _ => false,
    }
}
