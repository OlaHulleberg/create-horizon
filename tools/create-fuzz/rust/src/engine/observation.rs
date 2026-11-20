use super::*;

impl Run {
    pub fn snapshot(&self, sc: &Scenario) -> Snapshot {
        let mut blocks = BTreeMap::new();
        let mut groups: BTreeMap<i64, Vec<String>> = BTreeMap::new();
        let mut sizes = BTreeMap::new();
        for p in sc.region() {
            let id = self.at(p);
            let state = self.name(id);
            let kinetic = self.kinetic(p, id);
            if kinetic.is_some()
                && let Some((network, size)) = self.w.dims[0]
                    .kinetics
                    .network_membership(p, self.observed_sub(p))
            {
                groups.entry(network).or_default().push(key(p));
                sizes.insert(
                    key(p),
                    i32::try_from(size).expect("network size fits Java int"),
                );
            }
            blocks.insert(
                key(p),
                BlockSnap {
                    fluid_storage: self.fluid_storage(p),
                    fluid_machine: self.fluid_machine(p),
                    fluid_filter: self.w.dims[0]
                        .plumbing
                        .filters
                        .get(&p)
                        .map(|f| f.item.clone()),
                    fluid_network: self.fluid_network(p),
                    world_fluid: self.world_fluid_observation(p),
                    payload: self.saved_payload(p),
                    saved: self
                        .persisted
                        .kinetics
                        .get(&(p, self.observed_sub(p)))
                        .and_then(crate::persistence::codec::saved_from_nbt)
                        .map(Into::into),
                    fluid_tank: self.fluid_tank(p),
                    transport: self.transport_observation(p),
                    conveyor_links: self.chains.conveyors.get(&p).map(|c| {
                        let mut v = c.connections.clone();
                        v.sort();
                        v
                    }),
                    belt: self.belt_observation(p),
                    tunnel: self.tunnel_observation(p),
                    funnel: self.funnel_observation(p),
                    chute: self.chute_observation(p),
                    // Version 1 kinetic cases use ejectors as stress loads only.
                    ejector: (sc.version >= 3)
                        .then(|| self.ejector_observation(p))
                        .flatten(),
                    measured_trig: self.measured_trig.get(&p).map(|property| {
                        crate::allow::MeasuredTrigState {
                            tie: Some(crate::allow::NearestTie::Yaw45Pitch0),
                            java: state.clone(),
                            port: state.clone(),
                            properties: vec![property.clone()],
                        }
                    }),
                    bracket: self.brackets.get(&p).map(|b| {
                        format!(
                            "{}[axis_along_first={},facing={},type={}]",
                            b.block(),
                            b.along_first,
                            format!("{:?}", b.facing).to_lowercase(),
                            b.kind.name()
                        )
                    }),
                    state,
                    kinetic,
                    debug: None,
                },
            );
        }
        let mut partitions: Vec<_> = groups.into_values().collect();
        for group in &mut partitions {
            group.sort();
        }
        partitions.sort();
        Snapshot {
            origin: self.native_origin,
            save_phase: self.persisted.phase,
            item_outcomes: self.item_outcomes.clone(),
            side: "rust".into(),
            unavailable: {
                let mut unavailable = Vec::new();
                if self.native_failed {
                    unavailable.push(crate::fluid_world::OUTSIDE.into());
                }
                unavailable
            },
            networks: Some(crate::observation::Networks { partitions, sizes }),
            blocks,
            notes: self.notes.clone(),
            checkpoints: BTreeMap::new(),
        }
    }

    /// Match the Java block entity's driven half for split shafts. Use the same
    /// part for kinetic figures, network membership and network size observations.
    pub(super) fn observed_sub(&self, p: Pos) -> u8 {
        let k = &self.w.dims[0].kinetics;
        u8::from(
            k.network(p, 0).is_some_and(|n| n.2 == 0.0)
                && k.network(p, 1).is_some_and(|n| n.2 > 0.0),
        )
    }

    pub(super) fn context_limit(&self, p: Pos, sub: u8, distance: bool) -> Option<f32> {
        let k = &self.w.dims[0].kinetics;
        if self.w.block_at(0, p) == Some(create_engine::Block::SequencedGearshift) {
            // Java's single entity holds the outgoing context but reads it at the
            // driven shaft's theoretical speed. The engine stores it on the far half.
            let driven = k.theoretical(p, sub)?.0;
            for half in [0, 1] {
                if let Some(value) = k.sequence_limit(p, half, distance) {
                    let speed = k.theoretical(p, half)?.0;
                    return Some(if speed == 0. {
                        0.
                    } else {
                        value * (driven / speed).abs()
                    });
                }
            }
            None
        } else {
            k.sequence_limit(p, sub, distance)
        }
    }

    pub(super) fn kinetic(&self, p: Pos, id: u32) -> Option<Kinetic> {
        let k = &self.w.dims[0].kinetics;
        let (block, info): (create_engine::Block, &StateInfo) = self.w.table.get(id)?;
        let motor_rpm = self
            .w
            .column_settings(0, p[0] >> 4, p[2] >> 4)
            .into_iter()
            .find(|(q, _)| *q == p)
            .map_or(16.0, |(_, rpm)| rpm as f32);
        let settings = Settings {
            belt: self.w.dims[0].belt_of.get(&p).copied(),
            motor_rpm,
            steam: self.w.shaft_steam(0, p),
            flow: self.wheel_scores.get(&p).copied().unwrap_or(0),
            ..Settings::default()
        };
        let both = parts(block, info, &settings);
        // A split shaft is one block entity in Create, holding the figures of the half its
        // source drives: here, the half whose network has a generator.
        let sub = self.observed_sub(p);
        let (theoretical, stress, capacity, overstressed) = k.network(p, sub)?;
        let speed = k.speed(p, sub)?;
        let generated = k.generated(p, sub)?;
        let part = both[sub as usize]?;
        let faces = (both[1].is_some()
            || matches!(
                part.rotation,
                create_engine::kinetics::ShaftRotation::Split(_)
            ))
        .then(|| {
            let mut m = BTreeMap::new();
            for d in create_engine::kinetics::Dir::ALL {
                for (s, half) in both.iter().enumerate() {
                    if half.is_some_and(|h| h.shafts & d.bit() != 0) {
                        m.insert(
                            format!("{d:?}").to_lowercase(),
                            if both[1].is_none() {
                                k.face_speed(p, d).unwrap_or(0.0)
                            } else {
                                k.speed(p, s as u8).unwrap_or(0.0)
                            },
                        );
                    }
                }
            }
            m
        });
        Some(Kinetic {
            speed,
            theoretical,
            overstressed,
            stress,
            capacity,
            impact: part.impact,
            added_capacity: part.capacity,
            generated,
            has_source: k.driven_elsewhere(p, sub),
            comparator: self.w.dims[0]
                .gauges
                .get(&p)
                .map(|g| create_engine::gauge::analog_output(g.shown.dial)),
            sequence_angle: self.context_limit(p, sub, false),
            sequence_distance: self.context_limit(p, sub, true),
            valve: self.handles.get(&p).map(|h| {
                let mut observed = h.observe(info.facing.is_some_and(|f| f.positive()));
                if h.remaining == 0 && k.driven_elsewhere(p, sub) && speed != 0. {
                    observed.angle = None;
                    observed.angle_unavailable =
                        Some("externally driven angle requires client renderer".into());
                }
                observed
            }),
            sequencer: (block == create_engine::Block::SequencedGearshift)
                .then(|| self.w.sequencer_run(0, p))
                .flatten()
                .map(|r| crate::observation::SequenceRun {
                    current: r.current,
                    duration: r.duration,
                    progress: r.progress,
                    timer: r.timer,
                    powered_before: r.powered_before,
                }),
            chain_modifier: (block == create_engine::Block::AdjustableChainGearshift).then(|| {
                create_engine::chain::modifier_for_signal(
                    self.w.saved_chain_signal(0, p).unwrap_or(0),
                )
            }),
            speed_setting: self.w.motor_setting(0, p),
            faces,
        })
    }
}
