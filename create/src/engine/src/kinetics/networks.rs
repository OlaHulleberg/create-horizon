//! Create's KineticNetwork and TorquePropagator, owned by one dimension.
use super::{NONE, Node};
use std::collections::{HashMap, HashSet};

/// KineticNetwork stores base stress/capacity; actual contributions use current entity speeds.
#[derive(Default)]
pub(super) struct KineticNetwork {
    id: u32,
    members: HashMap<u32, f32>,
    sources: HashMap<u32, f32>,
    stress: f32,
    capacity: f32,
}
impl KineticNetwork {
    /// KineticNetwork.addSilently: enroll an already initialized entity without syncing it.
    pub(super) fn add_silently(&mut self, id: u32, nodes: &[Node]) {
        let n = &nodes[id as usize];
        self.members.entry(id).or_insert(n.part.impact);
        if n.part.generated != 0.0 {
            self.sources.entry(id).or_insert(n.part.capacity);
        }
    }
    /// KineticNetwork.add: register a member; the caller updates it from current network figures.
    pub(super) fn add(&mut self, id: u32, nodes: &[Node]) {
        self.add_silently(id, nodes);
    }
    /// KineticNetwork.remove: detach the member and its capacity contribution.
    pub(super) fn remove(&mut self, id: u32) {
        self.members.remove(&id);
        self.sources.remove(&id);
    }
    pub(super) fn update_capacity_for(&mut self, id: u32, capacity: f32) {
        self.sources.insert(id, capacity);
    }
    pub(super) fn update_stress_for(&mut self, id: u32, stress: f32) {
        self.members.insert(id, stress);
    }
    fn get_actual_capacity_of(&self, id: u32, nodes: &[Node]) -> f32 {
        self.sources[&id] * nodes[id as usize].part.generated.abs()
    }
    fn get_actual_stress_of(&self, id: u32, nodes: &[Node]) -> f32 {
        self.members[&id] * nodes[id as usize].state.theoretical.abs()
    }
    fn calculate_capacity(&mut self, nodes: &[Node]) -> f32 {
        self.sources
            .retain(|&id, _| nodes[id as usize].live && nodes[id as usize].state.net == self.id);
        self.sources
            .keys()
            .map(|&id| self.get_actual_capacity_of(id, nodes))
            .sum()
    }
    fn calculate_stress(&mut self, nodes: &[Node]) -> f32 {
        self.members
            .retain(|&id, _| nodes[id as usize].live && nodes[id as usize].state.net == self.id);
        self.members
            .keys()
            .map(|&id| self.get_actual_stress_of(id, nodes))
            .sum()
    }
    /// KineticNetwork.updateCapacity recalculates and syncs capacity changes immediately.
    pub(super) fn update_capacity(&mut self, nodes: &[Node]) -> Option<NetworkSync> {
        let capacity = self.calculate_capacity(nodes);
        if self.capacity == capacity {
            return None;
        }
        self.capacity = capacity;
        Some(self.sync())
    }
    /// KineticNetwork.updateStress recalculates and syncs stress changes immediately.
    pub(super) fn update_stress(&mut self, nodes: &[Node]) -> Option<NetworkSync> {
        let stress = self.calculate_stress(nodes);
        if self.stress == stress {
            return None;
        }
        self.stress = stress;
        Some(self.sync())
    }
    /// KineticNetwork.updateNetwork: calculate once and sync only when totals changed.
    pub(super) fn update_network(&mut self, nodes: &[Node]) -> Option<NetworkSync> {
        let stress = self.calculate_stress(nodes);
        let capacity = self.calculate_capacity(nodes);
        if stress == self.stress && capacity == self.capacity {
            return None;
        }
        self.stress = stress;
        self.capacity = capacity;
        Some(self.sync())
    }
    /// KineticNetwork.sync enumerates only this network's members.
    fn sync(&self) -> NetworkSync {
        NetworkSync {
            members: self.members.keys().copied().collect(),
            stress: self.stress,
            capacity: self.capacity,
        }
    }
}
/// Values sent to each member's KineticBlockEntity.updateFromNetwork.
pub(super) struct NetworkSync {
    pub members: Vec<u32>,
    pub stress: f32,
    pub capacity: f32,
}
/// TorquePropagator's registry and deduplicated networkDirty work queue.
#[derive(Default)]
pub(super) struct TorquePropagator {
    networks: HashMap<u32, KineticNetwork>,
    dirty: HashSet<u32>,
}
impl TorquePropagator {
    pub(super) fn get_or_create_network_for(&mut self, id: u32) -> &mut KineticNetwork {
        assert_ne!(id, NONE);
        self.networks.entry(id).or_insert_with(|| KineticNetwork {
            id,
            ..Default::default()
        })
    }
    pub(super) fn mark_dirty(&mut self, id: u32) {
        if id != NONE {
            self.dirty.insert(id);
        }
    }
    pub(super) fn has_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }
    pub(super) fn take_dirty(&mut self) -> HashSet<u32> {
        std::mem::take(&mut self.dirty)
    }
    pub(super) fn remove(&mut self, net: u32, id: u32) {
        if let Some(n) = self.networks.get_mut(&net) {
            n.remove(id);
            if n.members.is_empty() {
                self.networks.remove(&net);
                self.dirty.remove(&net);
            } else {
                self.mark_dirty(net);
            }
        }
    }
    pub(super) fn update_network(&mut self, id: u32, nodes: &[Node]) -> Option<NetworkSync> {
        self.networks.get_mut(&id)?.update_network(nodes)
    }
    pub(super) fn members(&self, id: u32) -> impl Iterator<Item = u32> + '_ {
        self.networks
            .get(&id)
            .into_iter()
            .flat_map(|n| n.members.keys().copied())
    }
    pub(super) fn figures(&self, id: u32) -> (f32, f32, u32) {
        self.networks
            .get(&id)
            .map(|n| (n.stress, n.capacity, n.members.len() as u32))
            .unwrap_or_default()
    }
}
