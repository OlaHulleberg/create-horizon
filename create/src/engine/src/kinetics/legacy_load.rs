//! Migration of this port's pre-KineticState saves. This is deliberately not Create behaviour.
//!
//! Loaded legacy entities remain eligible while neighbouring columns arrive. Reconstruct a
//! consistent source tree in lexicographic (x,y,z,port) order, or extend authoritative saved
//! state in mixed-format components. Never use this entry point for player placements.
use super::*;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum LoadOrigin {
    #[default]
    Placement,
    Saved,
    Legacy,
    MigratedLegacy,
}
impl LoadOrigin {
    fn is_legacy(self) -> bool {
        matches!(self, Self::Legacy | Self::MigratedLegacy)
    }
}

impl Kinetics {
    /// Marks a part loaded from the old save format, before its first solve. Later column
    /// arrivals can re-evaluate its legacy component; subsequent saves use KineticState.
    pub fn loading_save_without_kinetic_state(&mut self, pos: [i32; 3], sub: u8) {
        if let Some(id) = self
            .at
            .get(&pos)
            .and_then(|ids| ids.get(sub as usize))
            .copied()
            .filter(|&id| id != NONE)
        {
            self.legacy_loading = true;
            self.nodes[id as usize].load_origin = LoadOrigin::Legacy;
            self.legacy_loads.push((id, self.nodes[id as usize].stamp));
        }
    }

    /// Whether this block is waiting for a consistent legacy reconstruction. Do not
    /// overwrite its old block-entity data with an invented empty KineticState.
    pub fn awaiting_legacy_load(&self, pos: [i32; 3]) -> bool {
        self.at.get(&pos).is_some_and(|ids| {
            ids.iter()
                .any(|&id| id != NONE && self.nodes[id as usize].load_origin == LoadOrigin::Legacy)
        })
    }

    /// Startup inputs (flow, redstone, crank expiry) can change a loaded part before
    /// migration runs. Capture their configuration without starting fresh propagation.
    pub(super) fn prepare_legacy_part_change(&mut self, id: u32, part: Part) -> bool {
        let n = &mut self.nodes[id as usize];
        if n.load_origin != LoadOrigin::Legacy {
            return false;
        }
        n.part = part;
        n.stamp = self.stamp;
        self.legacy_loads.push((id, n.stamp));
        self.dirty.push(id);
        self.mark_changed(id);
        true
    }

    pub(super) fn migrate_saves_without_kinetic_state(&mut self) {
        let pending = std::mem::take(&mut self.legacy_loads);
        let mut visited = HashSet::new();
        for (start, stamp) in pending {
            if !self.nodes[start as usize].live
                || self.nodes[start as usize].stamp != stamp
                || visited.contains(&start)
            {
                continue;
            }
            let mut group = Vec::new();
            let mut queue = VecDeque::from([start]);
            visited.insert(start);
            while let Some(id) = queue.pop_front() {
                group.push(id);
                for m in self.get_connected_neighbours(id) {
                    if self.nodes[m as usize].load_origin != LoadOrigin::Placement
                        && visited.insert(m)
                    {
                        queue.push_back(m);
                    }
                }
            }
            if !group
                .iter()
                .any(|&id| self.nodes[id as usize].load_origin.is_legacy())
            {
                continue;
            }
            group.sort_by_key(|&id| {
                let n = &self.nodes[id as usize];
                (n.pos, n.sub)
            });
            let original = group
                .iter()
                .map(|&id| self.nodes[id as usize].state.clone())
                .collect::<Vec<_>>();
            let anchors = group
                .iter()
                .copied()
                .filter(|&id| {
                    self.nodes[id as usize].load_origin == LoadOrigin::Saved
                        && self.nodes[id as usize].state.net.is_some()
                        && self.nodes[id as usize].state.theoretical != 0.0
                })
                .collect::<Vec<_>>();
            let roots = if anchors.is_empty() {
                group
                    .iter()
                    .copied()
                    .filter(|&id| self.nodes[id as usize].part.generated != 0.0)
                    .collect::<Vec<_>>()
            } else {
                let net = self.nodes[anchors[0] as usize].state.net;
                group
                    .iter()
                    .copied()
                    .find(|&id| {
                        self.nodes[id as usize].part.generated != 0.0
                            && Some(network_id(self.nodes[id as usize].pos)) == net
                    })
                    .into_iter()
                    .collect()
            };
            let mut inferred = None;
            for root in roots {
                for (&id, state) in group.iter().zip(&original) {
                    self.nodes[id as usize].state =
                        if self.nodes[id as usize].load_origin == LoadOrigin::Saved {
                            state.clone()
                        } else {
                            KineticState::default()
                        };
                }
                if self.infer_legacy_tree(&group, root, &anchors) {
                    inferred = Some(
                        group
                            .iter()
                            .map(|&id| self.nodes[id as usize].state.clone())
                            .collect::<Vec<_>>(),
                    );
                    break;
                }
            }
            // A component with no generator and no turning saved anchor is stationary.
            if inferred.is_none()
                && group
                    .iter()
                    .all(|&id| self.nodes[id as usize].part.generated == 0.0)
                && anchors.is_empty()
            {
                inferred = Some(group.iter().map(|_| KineticState::default()).collect());
            }
            for (&id, state) in group.iter().zip(&original) {
                self.nodes[id as usize].state = state.clone();
            }
            let Some(inferred) = inferred else {
                continue;
            }; // Defer missing source columns or inconsistent topology without destructive guesses.
            // Migration reconstructs one complete component. Admit its authoritative
            // saved anchors before enrolling the inferred legacy members, so the
            // first member tick calculates the complete migrated source set.
            for &anchor in &anchors {
                self.initialize_network(anchor);
            }
            for ((&id, old), mut state) in group.iter().zip(&original).zip(inferred) {
                if !self.nodes[id as usize].load_origin.is_legacy() {
                    continue;
                }
                if old.net != state.net {
                    self.networks.remove(old.net, id, &mut self.nodes);
                }
                let first = old.lifecycle.is_new();
                state.lifecycle = state::Lifecycle::Ticked(self.base_pass);
                state.update_speed = false;
                state.overstressed = old.overstressed;
                state.stress = old.stress;
                state.capacity = old.capacity;
                state.speed = if state.overstressed {
                    0.0
                } else {
                    state.theoretical
                };
                self.nodes[id as usize].state = state;
                self.nodes[id as usize].load_origin = LoadOrigin::MigratedLegacy;
                let net = self.nodes[id as usize].state.net;
                if net.is_some() {
                    let n = &self.nodes[id as usize];
                    let (cap, impact) = (n.part.capacity, n.part.impact);
                    let network = self.networks.get_or_create_network_for(net);
                    if !anchors.is_empty() {
                        network.add_silently(id, &mut self.nodes, cap, impact);
                    } else {
                        network.add(id, &mut self.nodes);
                    }
                }
                let (stress, capacity, size) = self.networks.figures(net);
                self.update_from_network(id, capacity, stress, size);
                if net.is_some() {
                    self.networks.mark_entity_dirty(id, &self.nodes);
                }
                if first {
                    self.scheduler
                        .validate_later(self.now, id, self.nodes[id as usize].stamp);
                }
                self.mark_changed(id);
            }
        }
    }

    fn infer_legacy_tree(&mut self, group: &[u32], root: u32, anchors: &[u32]) -> bool {
        let allowed = group.iter().copied().collect::<HashSet<_>>();
        let mut reached = anchors.iter().copied().collect::<HashSet<_>>();
        let mut queue = VecDeque::from(anchors.to_vec());
        if !reached.contains(&root) {
            let n = &mut self.nodes[root as usize];
            n.state.theoretical = n.part.generated;
            n.state.net = Some(network_id(n.pos));
            reached.insert(root);
            queue.push_front(root);
        }
        while let Some(id) = queue.pop_front() {
            for m in self.get_connected_neighbours(id) {
                if !allowed.contains(&m) {
                    continue;
                }
                let speed = self.get_conveyed_speed(id, m);
                if speed == 0.0 {
                    continue;
                }
                if !speed.is_finite() || speed.abs() > MAX_SPEED {
                    return false;
                }
                if reached.contains(&m) {
                    let current = self.nodes[m as usize].state.theoretical;
                    if (speed - current).abs() > current.abs() / 65536.0 + 1e-4 {
                        return false;
                    }
                    continue;
                }
                if self.nodes[m as usize].load_origin == LoadOrigin::Saved {
                    return false;
                }
                let (net, seq, source) = {
                    let n = &self.nodes[id as usize];
                    (n.state.net, n.state.seq, self_location(n))
                };
                let n = &mut self.nodes[m as usize];
                n.state.theoretical = speed;
                n.state.net = net;
                n.state.source = Some(source);
                n.state.seq = seq;
                reached.insert(m);
                queue.push_back(m);
            }
        }
        group.iter().all(|&id| {
            let n = &self.nodes[id as usize];
            let (g, t) = (n.part.generated, n.state.theoretical);
            g == 0.0
                || (reached.contains(&id)
                    && (id == root || g <= t || (t.abs() >= g.abs() && t.signum() == g.signum())))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parts() -> [Part; 3] {
        [
            Part {
                generated: 16.0,
                capacity: 64.0,
                ..Part::half(Dir::East)
            },
            Part::shaft(Axis::X),
            Part {
                generated: -16.0,
                capacity: 64.0,
                ..Part::half(Dir::West)
            },
        ]
    }
    #[test]
    fn missing_saved_source_defers_legacy_data_until_its_column_arrives() {
        let mut original = Kinetics::new();
        for x in 0..3 {
            original.set([x, 0, 0], 0, parts()[x as usize]);
        }
        original.solve(&mut Vec::new());
        let mut k = Kinetics::new();
        k.set([2, 0, 0], 0, parts()[2]);
        k.restore_state([2, 0, 0], 0, original.saved_state([2, 0, 0], 0).unwrap());
        k.solve(&mut Vec::new());
        k.set([1, 0, 0], 0, parts()[1]);
        k.loading_save_without_kinetic_state([1, 0, 0], 0);
        let mut events = Vec::new();
        k.solve(&mut events);
        assert!(!events.iter().any(|e| matches!(e, Event::Break { .. })));
        assert!(k.awaiting_legacy_load([1, 0, 0]));
        assert_eq!(k.saved_state([1, 0, 0], 0), None);
        k.set([0, 0, 0], 0, parts()[0]);
        k.loading_save_without_kinetic_state([0, 0, 0], 0);
        events.clear();
        k.solve(&mut events);
        assert!(!events.iter().any(|e| matches!(e, Event::Break { .. })));
        assert!(!k.awaiting_legacy_load([1, 0, 0]));
        assert_eq!(k.saved_state([0, 0, 0], 0).unwrap().source, None);
        assert_eq!(
            k.saved_state([0, 0, 0], 0)
                .unwrap()
                .network
                .unwrap()
                .capacity,
            2048.0
        );
    }

    #[test]
    fn skips_a_lexically_earlier_generator_that_cannot_keep_the_other_as_a_member() {
        let mut k = Kinetics::new();
        let mut p = parts();
        p[0].generated = -16.0;
        p[2].generated = 16.0;
        for x in [2, 1, 0] {
            k.set([x, 0, 0], 0, p[x as usize]);
            k.loading_save_without_kinetic_state([x, 0, 0], 0);
        }
        let mut events = Vec::new();
        k.solve(&mut events);
        assert!(!events.iter().any(|e| matches!(e, Event::Break { .. })));
        assert_eq!(k.saved_state([2, 0, 0], 0).unwrap().source, None);
        for x in 0..3 {
            assert_eq!(k.speed([x, 0, 0], 0), Some(16.0));
        }
    }

    #[test]
    fn legacy_motor_pair_migrates_in_both_orders_even_across_ticks() {
        for order in [[0, 1, 2], [2, 1, 0]] {
            let mut k = Kinetics::new();
            for x in order {
                k.set([x as i32, 0, 0], 0, parts()[x]);
                k.loading_save_without_kinetic_state([x as i32, 0, 0], 0);
                let mut events = Vec::new();
                k.solve(&mut events);
                assert!(
                    !events.iter().any(|e| matches!(e, Event::Break { .. })),
                    "{events:?}"
                );
            }
            for x in 0..3 {
                assert_eq!(k.speed([x, 0, 0], 0), Some(16.0));
            }
            assert_eq!(k.saved_state([0, 0, 0], 0).unwrap().source, None);
            assert!(k.saved_state([2, 0, 0], 0).unwrap().source.is_some());
            assert_eq!(
                k.saved_state([0, 0, 0], 0).unwrap().network.unwrap().id,
                network_id([0, 0, 0])
            );
        }
    }
    #[test]
    fn mixed_saved_and_legacy_parts_keep_authoritative_sources() {
        let mut original = Kinetics::new();
        for x in 0..3 {
            original.set([x, 0, 0], 0, parts()[x as usize]);
        }
        original.solve(&mut Vec::new());
        for saved_x in [0, 2] {
            for order in [[0, 1, 2], [2, 1, 0]] {
                let mut k = Kinetics::new();
                // Columns may arrive on different ticks; saved anchors stay authoritative.
                for x in order {
                    k.set([x as i32, 0, 0], 0, parts()[x]);
                    if x == saved_x {
                        k.restore_state(
                            [x as i32, 0, 0],
                            0,
                            original.saved_state([x as i32, 0, 0], 0).unwrap(),
                        );
                    } else {
                        k.loading_save_without_kinetic_state([x as i32, 0, 0], 0);
                    }
                    let mut events = Vec::new();
                    k.solve(&mut events);
                    assert!(
                        !events.iter().any(|e| matches!(e, Event::Break { .. })),
                        "{events:?}"
                    );
                }
                let mut events = Vec::new();
                k.solve(&mut events);
                assert!(
                    !events.iter().any(|e| matches!(e, Event::Break { .. })),
                    "{events:?}"
                );
                for x in 0..3 {
                    assert_eq!(k.speed([x, 0, 0], 0), Some(16.0));
                }
                assert_eq!(k.saved_state([0, 0, 0], 0).unwrap().source, None);
                assert!(k.saved_state([2, 0, 0], 0).unwrap().source.is_some());
                assert_eq!(
                    k.saved_state([0, 0, 0], 0)
                        .unwrap()
                        .network
                        .unwrap()
                        .capacity,
                    2048.0
                );
            }
        }
    }
}
