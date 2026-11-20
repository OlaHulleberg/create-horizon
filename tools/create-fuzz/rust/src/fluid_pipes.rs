//! Player histories for fluid topology and casing, without direct fluid-state injection.
use crate::{
    actions::{Action, Face},
    scenario::{Change, Placed, Scenario},
};
pub fn generate(seed: u64) -> Scenario {
    let (yaw, pitch) = match (seed / 32) % 6 {
        0 => (0., 0.),
        1 => (90., 0.),
        2 => (180., 0.),
        3 => (270., 0.),
        4 => (0., 90.),
        _ => (0., -90.),
    };
    let mut sc = Scenario {
        version: 3,
        seed,
        ticks: 220,
        day_time: 0,
        checkpoints: (10..=130).collect(),
        blocks: vec![],
        events: vec![],
    };
    for x in 0..5 {
        sc.blocks.push(Placed {
            pos: [x, 3, 0],
            state: "minecraft:stone".into(),
            motor: None,
        });
    }
    let mut use_at = |tick, pos, item: &str, face: Face, sneaking| {
        sc.events.push(Change {
            tick,
            pos,
            state: None,
            motor: None,
            turn: None,
            action: Some(Action::Use {
                item: item.into(),
                face,
                hit: face.point(0.5, 0.5),
                yaw,
                pitch,
                sneaking,
            }),
        })
    };
    for (tick, x, item) in [
        (10, 0, "create:fluid_pipe"),
        (14, 1, "create:fluid_pipe"),
        (18, 2, "create:mechanical_pump"),
        (22, 3, "create:fluid_pipe"),
        (26, 4, "create:fluid_tank"),
    ] {
        use_at(tick, [x, 3, 0], item, Face::Up, true);
    }
    for face in [Face::North, Face::South, Face::Up] {
        use_at(30, [1, 4, 0], "create:fluid_pipe", face, true);
    }
    use_at(34, [1, 4, 0], "create:wrench", Face::North, false);
    use_at(42, [1, 4, 0], "create:copper_casing", Face::North, false);
    use_at(50, [1, 4, 0], "create:wrench", Face::North, false);
    use_at(58, [1, 4, 0], "create:fluid_pipe", Face::Up, true);
    use_at(66, [1, 4, 0], "create:wrench", Face::South, false);
    use_at(74, [2, 4, 0], "create:wrench", Face::Up, false);
    use_at(82, [3, 4, 0], "create:copper_casing", Face::South, false);
    use_at(90, [3, 4, 0], "create:wrench", Face::South, true);
    use_at(98, [1, 5, 0], "create:wrench", Face::South, true);
    use_at(106, [1, 4, 0], "create:wrench", Face::East, false);
    use_at(114, [1, 4, 0], "create:wrench", Face::East, true);
    sc
}

#[cfg(test)]
mod tests {
    #[test]
    fn conversions_preserve_topology_and_sneak_pickup_removes_casing_with_pipe() {
        let snap = crate::engine::run(&super::generate(16));
        let state = |tick, pos: &str| &snap.checkpoints[&tick].blocks[pos].state;
        assert!(state(33, "1,4,0").contains("north=true"));
        assert!(state(33, "1,4,0").contains("up=true"));
        assert!(state(35, "1,4,0").starts_with("create:glass_fluid_pipe"));
        assert!(state(43, "1,4,0").starts_with("create:encased_fluid_pipe"));
        assert!(state(51, "1,4,0").starts_with("create:fluid_pipe"));
        assert_eq!(state(91, "3,4,0"), "minecraft:air");
        assert_ne!(state(73, "2,4,0"), state(75, "2,4,0"));
    }
}
