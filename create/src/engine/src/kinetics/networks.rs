//! Create's KineticNetwork and TorquePropagator, owned by one dimension.
use super::Part;
use std::collections::HashMap;
type Member = ([i32; 3], u8);
/// KineticNetwork's identity, members and generating sources, with synchronized totals.
#[derive(Default)]
struct KineticNetwork {
    id: u32,
    members: HashMap<Member, f32>,
    sources: HashMap<Member, f32>,
    stress: f32,
    capacity: f32,
}
/// TorquePropagator's per-world registry. Network identity survives membership updates.
#[derive(Default)]
pub(super) struct TorquePropagator {
    networks: HashMap<u32, KineticNetwork>,
}
impl TorquePropagator {
    pub(super) fn clear_members(&mut self) {
        for n in self.networks.values_mut() {
            n.members.clear();
            n.sources.clear();
        }
    }
    /// KineticNetwork.add: stress uses theoretical RPM; capacity uses generated RPM.
    pub(super) fn add(&mut self, id: u32, pos: [i32; 3], sub: u8, part: Part, speed: f32) {
        let n = self.networks.entry(id).or_insert_with(|| KineticNetwork {
            id,
            ..Default::default()
        });
        n.members.insert((pos, sub), part.impact * speed.abs());
        if part.generated != 0.0 {
            n.sources
                .insert((pos, sub), part.capacity * part.generated.abs());
        }
    }
    /// KineticNetwork.updateNetwork recalculates capacity and stress together.
    pub(super) fn update_networks(&mut self) {
        self.networks.retain(|_, n| !n.members.is_empty());
        for (&id, n) in &mut self.networks {
            debug_assert_eq!(id, n.id);
            n.stress = n.members.values().sum();
            n.capacity = n.sources.values().sum();
        }
    }
    pub(super) fn figures(&self, id: u32) -> (f32, f32, u32) {
        self.networks
            .get(&id)
            .map(|n| (n.stress, n.capacity, n.members.len() as u32))
            .unwrap_or_default()
    }
}
