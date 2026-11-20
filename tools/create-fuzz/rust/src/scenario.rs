//! The scenario and snapshot formats, shared with the Java side (`java/`).

use serde::{Deserialize, Serialize};

pub type Pos = [i32; 3];

/// One case: blocks placed one per tick in order (block i before tick i), changes at given
/// ticks, `ticks` ticks in all.
/// Positions are relative to the case's origin; the Java side places them on a stone floor.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Scenario {
    #[serde(default = "legacy_version")]
    pub version: u32,
    #[serde(default)]
    pub seed: u64,
    /// Completed tick counts to observe before the next tick's actions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checkpoints: Vec<u32>,
    pub ticks: u32,
    /// The level's day time when the case starts (`Level.setDayTime`): Create reads it (a
    /// cuckoo clock's hands, its pig and creeper at 12:00 and 18:32). One batch shares a level,
    /// so its cases share a day time.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub day_time: i64,
    pub blocks: Vec<Placed>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Change>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Placed {
    pub pos: Pos,
    /// A block state as `/setblock` takes it: `create:shaft[axis=x]`.
    pub state: String,
    /// A creative motor's speed setting (rpm, -256..=256, never 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motor: Option<i32>,
}

/// At `tick` (before that tick runs): place a block (`state`; air removes it), or a player
/// turns the hand crank at `pos` (`turn`: true backwards, as when sneaking).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Change {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<crate::actions::Action>,
    pub tick: u32,
    pub pos: Pos,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motor: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<bool>,
}

impl Change {
    pub fn set(tick: u32, pos: Pos, state: &str) -> Change {
        Change {
            action: None,
            tick,
            pos,
            state: Some(state.into()),
            motor: None,
            turn: None,
        }
    }
    pub fn turn(tick: u32, pos: Pos, back: bool) -> Change {
        Change {
            action: None,
            tick,
            pos,
            state: None,
            motor: None,
            turn: Some(back),
        }
    }
    pub fn placed(&self) -> Option<Placed> {
        Some(Placed {
            pos: self.pos,
            state: self.state.clone()?,
            motor: self.motor,
        })
    }
}

pub use crate::observation::{BlockSnap, Kinetic, Snapshot};

pub fn key(p: Pos) -> String {
    format!("{},{},{}", p[0], p[1], p[2])
}

/// `create:shaft[axis=x,waterlogged=false]` -> (`create:shaft`, [(axis, x), ...]).
pub fn parse_state(s: &str) -> (String, Vec<(String, String)>) {
    match s.split_once('[') {
        None => (s.to_string(), Vec::new()),
        Some((name, rest)) => {
            let props = rest
                .trim_end_matches(']')
                .split(',')
                .filter(|p| !p.is_empty())
                .filter_map(|p| p.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            (name.to_string(), props)
        }
    }
}

pub fn format_state(name: &str, props: &[(String, String)]) -> String {
    if props.is_empty() {
        return name.to_string();
    }
    let mut sorted = props.to_vec();
    sorted.sort();
    let inner: Vec<String> = sorted.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("{name}[{}]", inner.join(","))
}

/// The same state with one property changed (added when missing).
pub fn with_prop(state: &str, key: &str, value: &str) -> String {
    let (name, mut props) = parse_state(state);
    match props.iter_mut().find(|(k, _)| k == key) {
        Some(p) => p.1 = value.to_string(),
        None => props.push((key.to_string(), value.to_string())),
    }
    format_state(&name, &props)
}

impl Scenario {
    /// Closed bounding region around every action, including one block of neighbours.
    pub fn region(&self) -> Vec<Pos> {
        let positions: Vec<_> = self
            .blocks
            .iter()
            .map(|b| b.pos)
            .chain(self.events.iter().map(|e| e.pos))
            .collect();
        if positions.is_empty() {
            return Vec::new();
        }
        let mut lo = positions[0];
        let mut hi = lo;
        for p in positions {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        let padding = if crate::fluid_world::enabled(self) {
            2
        } else {
            1
        };
        for a in 0..3 {
            lo[a] -= padding;
            hi[a] += padding;
        }
        assert!(
            lo[0] >= -24 && lo[2] >= -24 && lo[1] >= 0 && hi[0] <= 24 && hi[2] <= 24 && hi[1] <= 24,
            "region exceeds isolated server slot"
        );
        let mut out = Vec::new();
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    out.push([x, y, z]);
                }
            }
        }
        out
    }
}

fn is_zero(t: &i64) -> bool {
    *t == 0
}

fn legacy_version() -> u32 {
    1
}
