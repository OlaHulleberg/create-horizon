//! Create's KineticNetwork and TorquePropagator, owned by one dimension.
use super::Node;
use std::collections::{HashMap, HashSet};

/// Generation identity of Create's KineticBlockEntity within this dimension. Shaft faces are ports,
/// not separate entities (`SplitShaftBlockEntity.getRotationSpeedModifier`).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Entity(u64);

/// One Java network member, with all engine ports that must receive its figures.
#[derive(Clone)]
struct Member {
    ports: HashSet<u32>,
    stress: f32,
}

/// KineticNetwork stores base stress/capacity; actual contributions use current entity speeds.
#[derive(Default)]
pub(super) struct KineticNetwork {
    members: HashMap<Entity, Member>,
    sources: HashMap<Entity, Member>,
    stress: f32,
    capacity: f32,
    initialized: bool,
    unloaded_capacity: f32,
    unloaded_stress: f32,
    unloaded_members: u32,
}
impl KineticNetwork {
    #[cfg(test)]
    pub(super) fn unloaded_count(&self) -> u32 {
        self.unloaded_members
    }

    /// KineticNetwork.initFromTE accounts for members not yet loaded.
    pub(super) fn init_from_te(&mut self, capacity: f32, stress: f32, size: u32) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.capacity = capacity;
        self.stress = stress;
        self.unloaded_capacity = capacity;
        self.unloaded_stress = stress;
        self.unloaded_members = size;
    }
    /// KineticNetwork.addSilently subtracts the saved contribution before adding the live one.
    pub(super) fn add_silently(
        &mut self,
        id: u32,
        nodes: &mut [Node],
        last_capacity: f32,
        last_stress: f32,
    ) {
        let n = &nodes[id as usize];
        if self.members.contains_key(&Entity(n.entity)) {
            self.add(id, nodes);
            return;
        }
        if n.part.generated != 0.0 {
            self.unloaded_capacity =
                (self.unloaded_capacity - last_capacity * n.part.generated.abs()).max(0.0);
        }
        self.unloaded_stress =
            (self.unloaded_stress - last_stress * n.state.theoretical.abs()).max(0.0);
        self.unloaded_members = self.unloaded_members.saturating_sub(1);
        self.add(id, nodes);
    }
    /// KineticNetwork.add enrolls an entity; only a new member receives the initial
    /// updateFromNetwork and networkDirty transition (returned to the caller).
    pub(super) fn add(&mut self, id: u32, nodes: &mut [Node]) -> bool {
        self.initialized = true;
        let n = &mut nodes[id as usize];
        let enrolled = self.members.contains_key(&Entity(n.entity));
        self.members
            .entry(Entity(n.entity))
            .or_insert_with(|| Member {
                ports: HashSet::new(),
                stress: n.part.impact,
            })
            .ports
            .insert(id);
        // KineticNetwork.add returns before updating sources or syncing an existing entity.
        if enrolled {
            return false;
        }
        n.calculate_stress_applied();
        if n.part.generated != 0.0 {
            n.calculate_added_stress_capacity();
            self.sources.insert(
                Entity(n.entity),
                Member {
                    ports: HashSet::from([id]),
                    stress: n.part.capacity,
                },
            );
        }
        true
    }
    /// Detach a port; the entity leaves only when its last enrolled port leaves.
    pub(super) fn remove(&mut self, id: u32, nodes: &[Node]) {
        self.members.retain(|entity, member| {
            if entity.0 != nodes[id as usize].entity {
                return true;
            }
            member.ports.remove(&id);
            if member.ports.is_empty() {
                self.sources.remove(entity);
                false
            } else {
                true
            }
        });
    }
    pub(super) fn update_capacity_for(&mut self, id: u32, capacity: f32, nodes: &[Node]) {
        self.sources.insert(
            Entity(nodes[id as usize].entity),
            Member {
                ports: HashSet::from([id]),
                stress: capacity,
            },
        );
    }
    pub(super) fn update_stress_for(&mut self, id: u32, stress: f32, nodes: &[Node]) {
        // KineticNetwork.updateStressFor: `members.put(be, stress)`, nothing else.
        let member = self
            .members
            .entry(Entity(nodes[id as usize].entity))
            .or_insert_with(|| Member {
                ports: HashSet::new(),
                stress,
            });
        member.ports.insert(id);
        member.stress = stress;
    }
    fn calculate_capacity(&mut self, nodes: &[Node]) -> f32 {
        // KineticNetwork.calculateCapacity prunes sources independently of members.
        self.sources.retain(|entity, source| {
            source
                .ports
                .retain(|&id| nodes[id as usize].live && nodes[id as usize].entity == entity.0);
            !source.ports.is_empty()
        });
        self.sources
            .values()
            .map(|source| {
                source.stress
                    * source
                        .ports
                        .iter()
                        .map(|&id| nodes[id as usize].part.generated.abs())
                        .fold(0.0_f32, f32::max)
            })
            .sum::<f32>()
            + self.unloaded_capacity
    }
    fn calculate_stress(&mut self, nodes: &[Node]) -> f32 {
        // Java compares the world block entity with the retained object reference.
        self.members.retain(|entity, member| {
            member
                .ports
                .retain(|&id| nodes[id as usize].live && nodes[id as usize].entity == entity.0);
            !member.ports.is_empty()
        });
        self.members
            .values()
            .map(|member| {
                // A split shaft has one theoretical speed; face modifiers only affect
                // conveyed rotation. Prefer its externally driven port to its relay.
                let id = member
                    .ports
                    .iter()
                    .copied()
                    .min_by_key(|&id| {
                        let n = &nodes[id as usize];
                        (
                            n.state.source.is_none_or(|source| source.pos == n.pos),
                            n.sub,
                        )
                    })
                    .unwrap();
                member.stress * nodes[id as usize].state.theoretical.abs()
            })
            .sum::<f32>()
            + self.unloaded_stress
    }
    /// KineticNetwork.updateCapacity recalculates and syncs capacity changes immediately.
    pub(super) fn update_capacity(&mut self, nodes: &[Node]) -> Option<NetworkSync> {
        let capacity = self.calculate_capacity(nodes);
        if self.capacity == capacity {
            return None;
        }
        self.capacity = capacity;
        Some(self.sync(nodes))
    }
    /// KineticNetwork.updateStress recalculates and syncs stress changes immediately.
    pub(super) fn update_stress(&mut self, nodes: &[Node]) -> Option<NetworkSync> {
        let stress = self.calculate_stress(nodes);
        if self.stress == stress {
            return None;
        }
        self.stress = stress;
        Some(self.sync(nodes))
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
        Some(self.sync(nodes))
    }
    /// KineticNetwork.sync enumerates only this network's members.
    fn sync(&self, nodes: &[Node]) -> NetworkSync {
        NetworkSync {
            members: self
                .members
                .iter()
                .flat_map(|(entity, m)| {
                    m.ports.iter().copied().filter(|&id| {
                        nodes[id as usize].live && nodes[id as usize].entity == entity.0
                    })
                })
                .collect(),
            stress: self.stress,
            capacity: self.capacity,
            size: self.members.len() as u32 + self.unloaded_members,
        }
    }
}
/// Values sent to each member's KineticBlockEntity.updateFromNetwork.
pub(super) struct NetworkSync {
    pub members: Vec<u32>,
    pub stress: f32,
    pub capacity: f32,
    pub size: u32,
}
/// TorquePropagator's registry and deduplicated networkDirty work queue.
#[derive(Default)]
pub(super) struct TorquePropagator {
    networks: HashMap<i64, KineticNetwork>,
    dirty: HashSet<Option<i64>>,
}
impl TorquePropagator {
    pub(super) fn get_or_create_network_for(&mut self, id: Option<i64>) -> &mut KineticNetwork {
        let id = id.expect("network identity");
        self.networks.entry(id).or_default()
    }
    pub(super) fn mark_dirty(&mut self, id: Option<i64>) {
        if id.is_some() {
            self.dirty.insert(id);
        }
    }
    pub(super) fn has_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }
    pub(super) fn take_dirty(&mut self) -> HashSet<Option<i64>> {
        std::mem::take(&mut self.dirty)
    }
    pub(super) fn remove(&mut self, net: Option<i64>, id: u32, nodes: &[Node]) {
        let Some(key) = net else {
            return;
        };
        if let Some(n) = self.networks.get_mut(&key) {
            n.remove(id, nodes);
            if n.members.is_empty() {
                self.networks.remove(&key);
                self.dirty.remove(&net);
            } else {
                self.mark_dirty(net);
            }
        }
    }
    pub(super) fn update_network(
        &mut self,
        id: Option<i64>,
        nodes: &[Node],
    ) -> Option<NetworkSync> {
        self.networks.get_mut(&id?)?.update_network(nodes)
    }
    pub(super) fn members<'a>(
        &'a self,
        id: Option<i64>,
        nodes: &'a [Node],
    ) -> impl Iterator<Item = u32> + 'a {
        id.and_then(|id| self.networks.get(&id))
            .into_iter()
            .flat_map(|n| {
                n.members.iter().flat_map(|(entity, m)| {
                    m.ports.iter().copied().filter(|&id| {
                        nodes[id as usize].live && nodes[id as usize].entity == entity.0
                    })
                })
            })
    }
    pub(super) fn figures(&self, id: Option<i64>) -> (f32, f32, u32) {
        id.and_then(|id| self.networks.get(&id))
            .map(|n| {
                (
                    n.stress,
                    n.capacity,
                    n.members.len() as u32 + n.unloaded_members,
                )
            })
            .unwrap_or_default()
    }
}
