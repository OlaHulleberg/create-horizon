//! Shared server-save fields of KineticBlockEntity; runtime network identity is excluded.
pub mod codec;
pub mod fluids;
pub mod payload;
use crate::scenario::Pos;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Saved {
    pub speed: f32,
    pub needs_speed_update: bool,
    pub source: Option<Pos>,
    pub network: Option<Network>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Network {
    pub stress: f32,
    pub capacity: f32,
    pub size: u32,
    pub added_stress: f32,
    pub added_capacity: f32,
}
impl From<create_engine::kinetics::SavedKineticState> for Saved {
    fn from(s: create_engine::kinetics::SavedKineticState) -> Self {
        Self {
            speed: s.speed,
            needs_speed_update: s.needs_speed_update,
            source: s.source.map(|p| p.pos),
            network: s.network.map(|n| Network {
                stress: n.stress,
                capacity: n.capacity,
                size: n.size,
                added_stress: n.added_stress,
                added_capacity: n.added_capacity,
            }),
        }
    }
}

pub fn equal(a: &Option<Saved>, b: &Option<Saved>) -> bool {
    let close = |a: f32, b: f32| (a - b).abs() <= 1e-3 * a.abs().max(b.abs()).max(1.);
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            close(a.speed, b.speed)
                && a.needs_speed_update == b.needs_speed_update
                && a.source == b.source
                && match (&a.network, &b.network) {
                    (None, None) => true,
                    (Some(a), Some(b)) => {
                        a.size == b.size
                            && close(a.stress, b.stress)
                            && close(a.capacity, b.capacity)
                            && close(a.added_stress, b.added_stress)
                            && close(a.added_capacity, b.added_capacity)
                    }
                    _ => false,
                }
        }
        _ => false,
    }
}

/// Reuse every existing scenario family; sample placement, action and settled boundaries.
pub fn schedule(sc: &mut crate::scenario::Scenario) {
    let mut phases = vec![
        1,
        (sc.blocks.len() / 2) as u32,
        sc.blocks.len() as u32 + 1,
        sc.ticks.saturating_sub(10),
    ];
    phases.extend(sc.events.iter().map(|e| e.tick.saturating_add(1)));
    phases.retain(|t| *t < sc.ticks);
    phases.sort_unstable();
    phases.dedup();
    let tick = phases[(sc.seed as usize / 64) % phases.len()];
    sc.version = 3;
    sc.ticks = sc.ticks.max(tick + crate::generate::SETTLE);
    sc.events.push(crate::scenario::Change {
        tick,
        pos: sc.blocks.first().map_or([0, 1, 0], |b| b.pos),
        action: Some(crate::actions::Action::Reload),
        state: None,
        motor: None,
        turn: None,
    });
    for t in tick..=(tick + 5).min(sc.ticks) {
        sc.checkpoints.push(t);
    }
    sc.ticks = sc
        .ticks
        .max(sc.events.iter().map(|e| e.tick).max().unwrap_or(0) + crate::generate::SETTLE);
    sc.checkpoints.sort_unstable();
    sc.checkpoints.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;
    use create_engine::kinetics::{SavedKineticState, SavedNetwork};

    #[test]
    fn identity_is_excluded_but_saved_network_figures_are_compared() {
        let saved = |id| SavedKineticState {
            speed: 32.,
            source: None,
            sequence: None,
            needs_speed_update: false,
            network: Some(SavedNetwork {
                id,
                stress: 64.,
                capacity: 512.,
                size: 2,
                added_stress: 2.,
                added_capacity: 16.,
            }),
        };
        let a = Some(Saved::from(saved(1)));
        let mut b = Some(Saved::from(saved(9999)));
        assert!(equal(&a, &b));
        b.as_mut().unwrap().network.as_mut().unwrap().size = 3;
        assert!(!equal(&a, &b));
        assert!(!equal(&a, &None));
    }

    #[test]
    fn lifecycle_flags_and_source_positions_remain_observable() {
        let a = Saved {
            speed: 0.,
            needs_speed_update: true,
            source: Some([1, 2, 3]),
            network: None,
        };
        let mut b = a.clone();
        b.needs_speed_update = false;
        assert!(!equal(&Some(a.clone()), &Some(b.clone())));
        b = a.clone();
        b.source = Some([2, 2, 3]);
        assert!(!equal(&Some(a), &Some(b)));
    }
}

pub mod connections;
