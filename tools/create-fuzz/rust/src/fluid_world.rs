//! Host block/fluid states used by fluid glue callbacks. This catalogue is independent of
//! Create's block-entity state table: vanilla fluids must not be registered as Create blocks.
use crate::scenario::{format_state, parse_state};
use create_engine::tank::Fluid;
use serde::{Deserialize, Serialize};
pub const FIRST: u32 = 19;
pub const OUTSIDE: &str = "fluid-world-outside-region";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub fluid: String,
    pub source: bool,
    pub amount: u8,
    pub falling: bool,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Rules {
    pub water_source_conversion: bool,
    pub lava_source_conversion: bool,
    pub evaporates: bool,
}
impl Default for Rules {
    fn default() -> Self {
        Self {
            water_source_conversion: true,
            lava_source_conversion: false,
            evaporates: false,
        }
    }
}
pub fn states() -> &'static [String] {
    static STATES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    STATES.get_or_init(|| {
        let mut states = Vec::new();
        for name in [
            "minecraft:water",
            "minecraft:lava",
            "create:honey",
            "create:chocolate",
        ] {
            for level in 0..16 {
                states.push(format!("{name}[level={level}]"));
            }
        }
        for name in [
            "minecraft:cobblestone",
            "minecraft:obsidian",
            "create:limestone",
            "create:scoria",
            "minecraft:cauldron",
            "minecraft:lava_cauldron",
        ] {
            states.push(name.into());
        }
        for level in 1..=3 {
            states.push(format!("minecraft:water_cauldron[level={level}]"));
        }
        for block in ["minecraft:stone_slab", "minecraft:oak_slab"] {
            for kind in ["bottom", "top", "double"] {
                for water in ["false", "true"] {
                    states.push(format!("{block}[type={kind},waterlogged={water}]"));
                }
            }
        }
        // `create:passive_boiler_heaters` as block states: magma heats, a lit campfire heats
        // and an unlit one does not (`BlockHelper.isNotUnheated`).
        states.push("minecraft:magma_block".into());
        for block in ["minecraft:campfire", "minecraft:soul_campfire"] {
            for facing in ["north", "south", "west", "east"] {
                for lit in ["true", "false"] {
                    states.push(format!(
                        "{block}[facing={facing},lit={lit},signal_fire=false,waterlogged=false]"
                    ));
                }
            }
        }
        for block in ["minecraft:beehive", "minecraft:bee_nest"] {
            for facing in ["north", "south", "west", "east"] {
                for honey in 0..=5 {
                    states.push(format!("{block}[facing={facing},honey_level={honey}]"));
                }
            }
        }
        states
    })
}
pub fn id(state: &str) -> Option<u32> {
    let (n, p) = parse_state(state);
    let state = format_state(&n, &p);
    states()
        .iter()
        .position(|s| s == &state)
        .map(|i| FIRST + i as u32)
}
pub fn name(id: u32) -> Option<String> {
    states().get(id.checked_sub(FIRST)? as usize).cloned()
}
pub fn fluid(state: &str) -> Option<(Fluid, u8)> {
    let (name, props) = parse_state(state);
    if props.iter().any(|(k, v)| k == "waterlogged" && v == "true") {
        return Some((Fluid::Water, 0));
    }
    let fluid = match name.as_str() {
        "minecraft:water" => Fluid::Water,
        "minecraft:lava" => Fluid::Lava,
        "create:honey" => Fluid::Honey,
        "create:chocolate" => Fluid::Chocolate,
        _ => return None,
    };
    let level = props
        .iter()
        .find(|(k, _)| k == "level")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    Some((fluid, level))
}
pub fn observe(state: &str) -> Option<Observation> {
    let (f, level) = fluid(state)?;
    Some(Observation {
        fluid: f.id().into(),
        source: level == 0,
        amount: if level == 0 || level >= 8 {
            8
        } else {
            8 - level
        },
        falling: level >= 8,
    })
}

pub fn enabled(sc: &crate::scenario::Scenario) -> bool {
    sc.blocks
        .iter()
        .map(|b| b.state.as_str())
        .chain(sc.events.iter().filter_map(|e| e.state.as_deref()))
        .any(|state| {
            id(state).is_some()
                || fluid(state).is_some()
                || state.contains("cauldron")
                || state.contains("honey_level")
        })
        || sc.events.iter().any(|e| match &e.action {
            Some(crate::actions::Action::Use { item, .. }) => {
                item.ends_with("bucket") || crate::fluid_recipes::contents(item).is_some()
            }
            Some(crate::actions::Action::Item { input }) => match input {
                crate::transport::Input::DropOn { stack } => {
                    stack.item.ends_with("bucket")
                        || crate::fluid_recipes::contents(&stack.item).is_some()
                }
                crate::transport::Input::Use { stack, .. } => stack.as_ref().is_some_and(|s| {
                    s.item.ends_with("bucket") || crate::fluid_recipes::contents(&s.item).is_some()
                }),
            },
            _ => false,
        })
}

pub fn generate(seed: u64) -> crate::scenario::Scenario {
    use crate::{
        actions::{Action, Face},
        scenario::{Change, Placed, Scenario},
    };
    let mut sc = Scenario {
        version: 3,
        seed,
        ticks: 220,
        day_time: 0,
        checkpoints: (50..=160).collect(),
        blocks: vec![],
        events: vec![],
    };
    for x in -2..=2 {
        for z in -2..=2 {
            sc.blocks.push(Placed {
                pos: [x, 3, z],
                state: "minecraft:stone".into(),
                motor: None,
            });
            if x == -2 || x == 2 || z == -2 || z == 2 {
                if seed & 64 != 0 {
                    sc.blocks.push(Placed {
                        pos: [x, 5, z],
                        state: "minecraft:stone".into(),
                        motor: None,
                    });
                }
                sc.blocks.push(Placed {
                    pos: [x, 4, z],
                    state: "minecraft:stone".into(),
                    motor: None,
                });
            }
        }
    }
    let falling = seed & 64 != 0;
    let bucket_actions = if seed & 128 != 0 {
        vec![
            (50, [-1, 3, 0], "minecraft:water_bucket", Face::Up),
            (60, [1, 3, 0], "minecraft:water_bucket", Face::Up),
            (90, [0, 4, 0], "minecraft:bucket", Face::Up),
        ]
    } else if falling {
        vec![
            (70, [-2, 5, 0], "minecraft:water_bucket", Face::East),
            (150, [-1, 5, 0], "minecraft:bucket", Face::Up),
        ]
    } else {
        vec![
            (50, [0, 3, 0], "minecraft:water_bucket", Face::Up),
            (90, [0, 4, 0], "minecraft:bucket", Face::Up),
        ]
    };
    sc.ticks = 240;
    sc.checkpoints = (50..=180).collect();
    for (tick, pos, item, face) in bucket_actions {
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
                yaw: 0.,
                pitch: 90.,
                sneaking: false,
            }),
        });
    }
    if seed & 256 != 0 {
        let event = |tick, item: &str| Change {
            tick,
            pos: [0, 3 + if tick == 48 { 1 } else { 0 }, 0],
            state: None,
            motor: None,
            turn: None,
            action: Some(Action::Use {
                item: item.into(),
                face: if tick == 48 { Face::North } else { Face::Up },
                hit: if tick == 48 {
                    [0.5, 0.5, 0.]
                } else {
                    [0.5, 1., 0.5]
                },
                yaw: 0.,
                pitch: 90.,
                sneaking: false,
            }),
        };
        sc.events.splice(
            0..0,
            [event(46, "create:fluid_pipe"), event(48, "create:wrench")],
        );
    }
    sc
}
