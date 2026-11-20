//! Fluid tank contract and player histories. Native world-fluid simulation is a separate input.
use crate::scenario::Pos;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tank {
    pub controller: Pos,
    pub width: u8,
    pub height: u8,
    pub capacity: u32,
    pub fluid: Option<String>,
    pub amount: u32,
}
#[derive(Deserialize)]
pub struct Contract {
    pub blocks: Vec<Entry>,
    pub unimplemented: std::collections::BTreeMap<String, String>,
}
#[derive(Deserialize)]
pub struct Entry {
    pub id: String,
    pub properties: std::collections::BTreeMap<String, Vec<String>>,
}
impl Entry {
    pub fn specifications(&self) -> Vec<String> {
        let mut combinations = vec![Vec::new()];
        for (key, values) in &self.properties {
            combinations = combinations
                .into_iter()
                .flat_map(|props| {
                    values.iter().map(move |value| {
                        let mut next = props.clone();
                        next.push((key.clone(), value.clone()));
                        next
                    })
                })
                .collect();
        }
        combinations
            .into_iter()
            .map(|props| crate::scenario::format_state(&self.id, &props))
            .collect()
    }
}
pub fn contract() -> &'static Contract {
    static CONTRACT: std::sync::OnceLock<Contract> = std::sync::OnceLock::new();
    CONTRACT
        .get_or_init(|| serde_json::from_str(include_str!("fluids.json")).expect("fluid contract"))
}
pub fn validate_state(id: &str, props: &[(String, String)]) -> Option<Result<(), String>> {
    let entry = contract().blocks.iter().find(|e| e.id == id)?;
    let mut seen = std::collections::BTreeSet::new();
    Some(
        if props.iter().all(|(k, v)| {
            seen.insert(k)
                && entry
                    .properties
                    .get(k)
                    .is_some_and(|values| values.contains(v))
        }) {
            Ok(())
        } else {
            Err("invalid fluid tank state".into())
        },
    )
}
pub fn report(manual: &[crate::scenario::Scenario], generated: &[crate::scenario::Scenario]) {
    for entry in &contract().blocks {
        let uses = |cases: &[crate::scenario::Scenario]| {
            cases.iter().flat_map(|s| &s.events).filter(|e| matches!(&e.action, Some(crate::actions::Action::Use { item, .. }) if item == &entry.id)).count()
        };
        println!(
            "fluid {}: {:?}; domain={}; player placement submissions generated={} manual={}",
            entry.id,
            entry.properties,
            entry.specifications().len(),
            uses(generated),
            uses(manual)
        );
    }
    for (group, reason) in &contract().unimplemented {
        println!("fluid excluded {group}: {reason}");
    }
}
pub fn generate(seed: u64) -> crate::scenario::Scenario {
    use crate::{
        actions::{Action, Face},
        scenario::{Change, Placed, Scenario},
    };
    let width = 1 + (seed / 32 % 3) as i32;
    let height = 1 + (seed / 96 % 3) as i32;
    let mut sc = Scenario {
        version: 3,
        seed,
        day_time: 0,
        ticks: 150,
        checkpoints: vec![30, 40, 60, 80],
        blocks: vec![],
        events: vec![],
    };
    for x in 0..width {
        for z in 0..width {
            sc.blocks.push(Placed {
                pos: [x, 3, z],
                state: "minecraft:stone".into(),
                motor: None,
            });
        }
    }
    let mut tick = 12;
    for y in 0..height {
        for x in 0..width {
            for z in 0..width {
                sc.events.push(Change {
                    tick,
                    pos: [x, 3 + y, z],
                    action: Some(Action::Use {
                        item: "create:fluid_tank".into(),
                        face: Face::Up,
                        hit: [0.5, 1., 0.5],
                        yaw: 0.,
                        pitch: 0.,
                        sneaking: true,
                    }),
                    state: None,
                    motor: None,
                    turn: None,
                });
                tick += 1;
            }
        }
    }
    sc.events.push(Change {
        tick: 55,
        pos: [0, 4, 0],
        action: Some(Action::Use {
            item: "create:wrench".into(),
            face: Face::North,
            hit: [0.5, 0.5, 0.],
            yaw: 0.,
            pitch: 0.,
            sneaking: false,
        }),
        state: None,
        motor: None,
        turn: None,
    });
    sc.events.push(Change {
        tick: 70,
        pos: [0, 4, 0],
        action: Some(Action::Remove),
        state: None,
        motor: None,
        turn: None,
    });
    sc
}

#[cfg(test)]
mod tests {
    #[test]
    fn player_placement_forms_tank_and_wrench_changes_windows() {
        let sc = super::generate(40);
        let snap = crate::engine::run(&sc);
        let before = &snap.checkpoints[&40].blocks["0,4,0"];
        let tank = before.fluid_tank.as_ref().expect("player placed a tank");
        assert_eq!((tank.width, tank.height, tank.capacity), (2, 1, 32000));
        assert!(before.state.contains("shape=window_nw"));
        assert!(
            snap.checkpoints[&60].blocks["0,4,0"]
                .state
                .contains("shape=plain")
        );
        assert_eq!(snap.blocks["0,4,0"].state, "minecraft:air");
        assert!(
            super::validate_state("create:fluid_tank", &[("shape".into(), "bogus".into())])
                .unwrap()
                .is_err()
        );
    }
}

#[cfg(test)]
mod fidelity {
    /// Bypass scenario placement and state propagation. The independent engine tank API must
    /// retain exactly the same partition and windows as the glue-driven scenario adapter.
    #[test]
    fn controller_removal_matches_engine_without_driver() {
        for seed in [40, 232] {
            let sc = super::generate(seed);
            let observed = crate::engine::run(&sc);
            let mut tanks = create_engine::tank::Tanks::default();
            let mut positions = Vec::new();
            for tick in 0..sc.ticks {
                for event in sc.events.iter().filter(|e| e.tick == tick) {
                    if let Some(crate::actions::Action::Use { item, .. }) = &event.action {
                        if item == "create:fluid_tank" {
                            let mut p = event.pos;
                            p[1] += 1;
                            tanks.place(p);
                            positions.push(p);
                        } else if item == "create:wrench" {
                            let (_, m) = tanks.get_mut(event.pos).unwrap();
                            m.window = !m.window;
                        }
                    } else if event.action == Some(crate::actions::Action::Remove) {
                        tanks.remove(event.pos);
                        positions.retain(|p| *p != event.pos);
                    }
                }
                for &p in &positions {
                    tanks.tick(p);
                }
            }
            for p in positions {
                let (controller, raw) = tanks.get(p).unwrap();
                let snap = &observed.blocks[&crate::scenario::key(p)];
                let tank = snap.fluid_tank.as_ref().unwrap();
                assert_eq!(tank.controller, controller);
                assert_eq!((tank.width, tank.height), (raw.width, raw.height));
                assert!(snap.state.contains(&format!(
                    "shape={}",
                    tanks.block_shape(p).unwrap().shape.name()
                )));
            }
        }
    }
}
