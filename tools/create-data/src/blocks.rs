//! Create's blocks, from its generated blockstates and `.wiki/block_info` files plus the Java facts,
//! and the vanilla template each one is declared from in Horizon's content-mod layer.
//!
//! Horizon gives a mod block its template's state properties, behaviour class and shapes. A Create
//! block whose properties no vanilla block has stays out until Horizon can declare its own states
//! (PORTING.md gap 2); `Decision::Pending` says why.

use crate::java::JavaFacts;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

pub type Props = BTreeMap<String, BTreeSet<String>>;

#[derive(Clone, Debug)]
pub struct CreateBlock {
    pub name: String,
    pub props: Props,
    pub hardness: f64,
    pub blast_resistance: f64,
    pub stack: u32,
    pub java: JavaFacts,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Declared {
        template: &'static str,
        sounds: Option<&'static str>,
        requires_tool: Option<bool>,
        shapes: Option<Value>,
        /// Own state properties (Horizon's `state_properties`), when no template has them.
        own: Option<Value>,
        /// Drawn as turning parts by the native mod, not in the chunk mesh
        /// (Create's `RenderShape.ENTITYBLOCK_ANIMATED`).
        animated: bool,
        /// Whether it has an item of its own (a belt has none: the belt connector places it).
        item: bool,
    },
    Pending(String),
}

/// The properties a blockstate file names, from its variant keys or multipart conditions.
pub fn blockstate_props(doc: &Value) -> Props {
    let mut props = Props::new();
    let mut add = |k: &str, v: &str| {
        for v in v.split('|') {
            props
                .entry(k.to_string())
                .or_default()
                .insert(v.to_string());
        }
    };
    if let Some(Value::Object(variants)) = doc.get("variants") {
        for key in variants.keys().filter(|k| !k.is_empty()) {
            for kv in key.split(',') {
                if let Some((k, v)) = kv.split_once('=') {
                    add(k, v);
                }
            }
        }
    }
    if let Some(Value::Array(parts)) = doc.get("multipart") {
        for part in parts {
            let Some(Value::Object(when)) = part.get("when") else {
                continue;
            };
            let conds: Vec<&Map<String, Value>> = match (when.get("OR"), when.get("AND")) {
                (Some(Value::Array(a)), _) | (_, Some(Value::Array(a))) => {
                    a.iter().filter_map(Value::as_object).collect()
                }
                _ => vec![when],
            };
            for c in conds {
                for (k, v) in c {
                    let v = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    add(k, &v);
                }
            }
        }
    }
    props
}

pub fn read_blocks(generated: &Path, java: &BTreeMap<String, JavaFacts>) -> Vec<CreateBlock> {
    let states = generated.join("assets/create/blockstates");
    let info = generated.join("data/create/.wiki/block_info");
    let mut names: Vec<String> = fs::read_dir(&states)
        .expect("blockstates")
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(".json")
                .map(str::to_string)
        })
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let doc: Value =
                serde_json::from_slice(&fs::read(states.join(format!("{name}.json"))).unwrap())
                    .unwrap();
            let mut props = blockstate_props(&doc);
            let info: Value = fs::read(info.join(format!("{name}.json")))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(Value::Null);
            if info["waterloggable"].as_bool() == Some(true) {
                props
                    .entry("waterlogged".into())
                    .or_default()
                    .extend(["false".to_string(), "true".to_string()]);
            }
            CreateBlock {
                hardness: info["hardness"].as_f64().unwrap_or(1.5),
                blast_resistance: info["blast_resistance"].as_f64().unwrap_or(6.0),
                stack: info["stackable"].as_u64().unwrap_or(64) as u32,
                java: java.get(&name).cloned().unwrap_or_default(),
                name,
                props,
            }
        })
        .collect()
}

/// The vanilla block whose sounds a Create sound type or base block has.
fn sound_block(b: &CreateBlock) -> Option<&'static str> {
    let by_sound = |s: &str| -> Option<&'static str> {
        Some(match s {
            "metal" => "minecraft:iron_block",
            "wood" => "minecraft:oak_planks",
            "netherite_block" => "minecraft:netherite_block",
            "copper" => "minecraft:copper_block",
            "wool" => "minecraft:white_wool",
            "deepslate" => "minecraft:deepslate",
            "glass" => "minecraft:glass",
            "stone" => "minecraft:stone",
            "scaffolding" => "minecraft:scaffolding",
            "anvil" => "minecraft:anvil",
            "chiseled_bookshelf" => "minecraft:chiseled_bookshelf",
            _ => return None,
        })
    };
    if let Some(s) = b.java.sound.as_deref().and_then(by_sound) {
        return Some(s);
    }
    Some(match b.java.base.as_deref()? {
        "stone" => "minecraft:andesite",
        "softMetal" => "minecraft:gold_block",
        "wooden" => "minecraft:stripped_spruce_wood",
        "copperMetal" => "minecraft:copper_block",
        "netheriteMetal" => "minecraft:netherite_block",
        _ => return None,
    })
}

/// Whether the base properties require the right tool for drops (copied from the vanilla block
/// Create's `initialProperties` names).
fn requires_tool(b: &CreateBlock) -> Option<bool> {
    match b.java.base.as_deref()? {
        "wooden" => Some(false),
        "stone" | "softMetal" | "copperMetal" | "netheriteMetal" => Some(true),
        _ => None,
    }
}

fn names(p: &Props) -> Vec<&str> {
    p.keys().map(String::as_str).collect()
}

fn has(name: &str, parts: &[&str]) -> bool {
    parts.iter().any(|p| name.contains(p))
}

/// The template for one block, by its properties, then its name and Java class.
pub fn decide(b: &CreateBlock, shapes: &BTreeMap<&'static str, Value>) -> Decision {
    let n = b.name.as_str();
    let keys = names(&b.props);
    let sounds = sound_block(b);
    let declared = |template: &'static str| Decision::Declared {
        template,
        sounds,
        requires_tool: requires_tool(b),
        shapes: shapes.get(n).cloned(),
        own: None,
        animated: false,
        item: true,
    };
    if let Some(d) = kinetic(b, sounds) {
        return d;
    }
    if let Some(d) = own_states(b) {
        return d;
    }
    if let Some(d) = fluid_blocks(b) {
        return d;
    }
    // AllFluids' `standardFluid` blocks: `LiquidBlock`s, water's states; how they flow is
    // their fluid declaration's (`fluid_declarations`).
    if matches!(n, "honey" | "chocolate") {
        return Decision::Declared {
            template: "minecraft:water",
            sounds,
            requires_tool: None,
            shapes: None,
            own: None,
            animated: false,
            item: false,
        };
    }
    // Blocks with behaviour of their own wait for the phase that ports it.
    if let Some(reason) = later_phase(b) {
        return Decision::Pending(reason);
    }
    match keys.as_slice() {
        [] => {
            if has(n, &["window", "framed_glass", "tiled_glass"]) {
                declared("minecraft:glass")
            } else if n == "zinc_ore" {
                declared("minecraft:iron_ore")
            } else if n == "deepslate_zinc_ore" {
                declared("minecraft:deepslate_iron_ore")
            } else if has(n, &["table_cloth"]) {
                declared("minecraft:white_carpet")
            } else {
                declared("minecraft:andesite")
            }
        }
        ["waterlogged"] => declared("minecraft:heavy_core"),
        ["type", "waterlogged"] => declared("minecraft:stone_slab"),
        ["facing", "half", "shape", "waterlogged"] => declared("minecraft:stone_stairs"),
        ["east", "north", "south", "up", "waterlogged", "west"] => {
            declared("minecraft:stone_brick_wall")
        }
        ["east", "north", "south", "waterlogged", "west"] => {
            if has(n, &["window"]) {
                declared("minecraft:glass_pane")
            } else {
                declared("minecraft:iron_bars")
            }
        }
        ["axis"] => declared("minecraft:quartz_pillar"),
        ["facing", "waterlogged"]
            if has(n, &["ladder"])
                && b.props["facing"]
                    .iter()
                    .all(|f| !matches!(f.as_str(), "up" | "down")) =>
        {
            declared("minecraft:ladder")
        }
        ["bottom", "waterlogged"] if has(n, &["scaffolding"]) => declared("minecraft:scaffolding"),
        _ => Decision::Pending(format!(
            "properties {} have no vanilla template (gap 2)",
            keys.join(",")
        )),
    }
}

/// Phase 2's kinetic blocks: templates where vanilla has their properties (a shaft is an iron
/// chain's axis and waterlogging), own states where it doesn't, and Create's shapes.
fn kinetic(b: &CreateBlock, sounds: Option<&'static str>) -> Option<Decision> {
    use crate::shapes::*;
    let own = |props: &[(&str, &[&str])]| -> Option<Value> {
        let mut m = Map::new();
        for (k, v) in props {
            m.insert((*k).into(), json!(v));
        }
        Some(Value::Object(m))
    };
    let axis: &[&str] = &["x", "y", "z"];
    let bools: &[&str] = &["false", "true"];
    let facing: &[&str] = &["up", "down", "north", "south", "west", "east"];
    let (template, shapes, own) = match b.name.as_str() {
        "shaft" => (
            "minecraft:iron_chain",
            Some(by_axis(&[SIX_VOXEL_POLE])),
            None,
        ),
        // PoweredShaftBlock: AllShapes.EIGHT_VOXEL_POLE, stone's properties; put in place of a
        // shaft by the steam engine two blocks behind it (the glue), no item of its own.
        "powered_shaft" => (
            "minecraft:iron_chain",
            Some(by_axis(&[[4., 0., 4., 12., 16., 12.]])),
            None,
        ),
        // SteamEngineBlock (FaceAttachedHorizontalDirectionalBlock): on the floor, a wall or the
        // ceiling of a tank, copper's properties; its placement on a tank is the glue's. Content
        // shapes select by one property: AllShapes.STEAM_ENGINE / STEAM_ENGINE_CEILING's base
        // and column by `face` (their cross-piece turns with `facing`), a full block on a wall
        // (STEAM_ENGINE_WALL turns with `facing`). Defaults first: FLOOR, NORTH.
        "steam_engine" => {
            let px = |b: &[Px]| -> Value {
                json!(b.iter().map(|b| b.map(|v| v / 16.0)).collect::<Vec<_>>())
            };
            (
                "minecraft:copper_block",
                Some(json!({ "property": "face", "values": {
                    "floor": px(&[[1., 0., 1., 15., 3., 15.], [3., 0., 3., 13., 15., 13.]]),
                    "ceiling": px(&[[1., 13., 1., 15., 16., 15.], [3., 1., 3., 13., 16., 13.]]),
                    "wall": px(&[[0., 0., 0., 16., 16., 16.]]) } })),
                own(&[
                    ("face", &["floor", "wall", "ceiling"]),
                    ("facing", &["north", "south", "west", "east"]),
                    ("waterlogged", bools),
                ]),
            )
        }
        "cogwheel" => (
            "minecraft:iron_chain",
            Some(by_axis(&[SMALL_GEAR, SIX_VOXEL_POLE])),
            None,
        ),
        "large_cogwheel" => (
            "minecraft:iron_chain",
            Some(by_axis(&[LARGE_GEAR, SIX_VOXEL_POLE])),
            None,
        ),
        "gearbox" | "andesite_encased_shaft" | "brass_encased_shaft" => {
            ("minecraft:quartz_pillar", None, None)
        }
        "clutch" | "gearshift" => (
            "minecraft:quartz_pillar",
            None,
            own(&[("axis", axis), ("powered", bools)]),
        ),
        // ChainDriveBlock (RotatedPillarKineticBlock, stone's properties): its axis, the axis
        // its chain runs along and its place in the chain (`none` the default), set by the glue
        // (`updateShape`); ChainGearshiftBlock adds `powered`.
        "encased_chain_drive" => (
            "minecraft:quartz_pillar",
            None,
            own(&[
                ("axis", axis),
                ("axis_along_first", bools),
                ("part", &["none", "start", "middle", "end"]),
            ]),
        ),
        "adjustable_chain_gearshift" => (
            "minecraft:quartz_pillar",
            None,
            own(&[
                ("axis", axis),
                ("axis_along_first", bools),
                ("part", &["none", "start", "middle", "end"]),
                ("powered", bools),
            ]),
        ),
        // SequencedGearshiftBlock (HorizontalAxisKineticBlock, stone's properties): its
        // horizontal axis or `vertical`, and `state` (0 idle, else the instruction running + 1).
        "sequenced_gearshift" => (
            "minecraft:andesite",
            None,
            own(&[
                ("axis", &["x", "z"]),
                ("state", &["0", "1", "2", "3", "4", "5"]),
                ("vertical", bools),
            ]),
        ),
        // SpeedControllerBlock (HorizontalAxisKineticBlock, `SharedProperties::softMetal`):
        // AllShapes.SPEED_CONTROLLER.
        "rotation_speed_controller" => (
            "minecraft:gold_block",
            Some(by_property(
                "axis",
                &["x", "z"],
                &[
                    [0., 0., 0., 16., 4., 16.],
                    [1., 1., 1., 15., 13., 15.],
                    [0., 8., 0., 16., 14., 16.],
                ],
            )),
            own(&[("axis", &["x", "z"])]),
        ),
        "andesite_encased_cogwheel"
        | "brass_encased_cogwheel"
        | "andesite_encased_large_cogwheel"
        | "brass_encased_large_cogwheel" => (
            "minecraft:quartz_pillar",
            None,
            own(&[
                ("axis", axis),
                ("bottom_shaft", bools),
                ("top_shaft", bools),
            ]),
        ),
        "creative_motor" => (
            "minecraft:iron_block",
            Some(by_facing(&[MOTOR_BLOCK])),
            own(&[("facing", facing)]),
        ),
        "water_wheel" => ("minecraft:iron_block", None, own(&[("facing", facing)])),
        // BacktankBlock (HorizontalKineticBlock, waterloggable): copper's or netherite's
        // properties (waxed copper's template: it never weathers), AllShapes.BACKTANK (the tank
        // and the six-voxel pole through it); the glue fills it and keeps its air.
        "copper_backtank" | "netherite_backtank" => (
            if b.name == "copper_backtank" {
                "minecraft:waxed_copper_block"
            } else {
                "minecraft:netherite_block"
            },
            Some(by_property(
                "facing",
                &["north", "south", "west", "east"],
                &[[3., 0., 3., 13., 12., 13.], SIX_VOXEL_POLE],
            )),
            own(&[
                ("facing", &["north", "south", "west", "east"]),
                ("waterlogged", bools),
            ]),
        ),
        // MechanicalPressBlock: a horizontal facing; AllShapes.MECHANICAL_PROCESSOR_SHAPE (a
        // block with a 8x8 hole down its middle, where the head goes).
        "mechanical_press" => (
            "minecraft:iron_block",
            Some(by_property(
                "facing",
                &["north", "south", "west", "east"],
                &[
                    [0., 0., 0., 16., 16., 4.],
                    [0., 0., 12., 16., 16., 16.],
                    [0., 0., 4., 4., 16., 12.],
                    [12., 0., 4., 16., 16., 12.],
                ],
            )),
            own(&[("facing", &["north", "south", "west", "east"])]),
        ),
        // A block entity with item slots needs a template that has one (Horizon's API 15): the
        // barrel's, whose menu the mod keeps closed. AllShapes.MILLSTONE.
        "millstone" => (
            "minecraft:barrel",
            Some(by_property(
                "open",
                &["false", "true"],
                &[[0., 0., 0., 16., 6., 16.], [2., 6., 2., 14., 16., 14.]],
            )),
            None,
        ),
        // ArmBlock (KineticBlock, ICogWheel; `SharedProperties::softMetal`): `ceiling` when
        // placed against a block's underside; AllShapes.MECHANICAL_ARM / MECHANICAL_ARM_CEILING.
        // Its arm and cog are parts.
        "mechanical_arm" => {
            let px = |b: &[Px]| -> Value {
                json!(b.iter().map(|b| b.map(|v| v / 16.0)).collect::<Vec<_>>())
            };
            (
                "minecraft:gold_block",
                Some(json!({ "property": "ceiling", "values": {
                    "false": px(&[
                        [2., 0., 2., 14., 10., 14.],
                        [3., 0., 3., 13., 14., 13.],
                        [0., 0., 0., 16., 6., 16.],
                    ]),
                    "true": px(&[
                        [2., 6., 2., 14., 16., 14.],
                        [3., 2., 3., 13., 16., 13.],
                        [0., 10., 0., 16., 16., 16.],
                    ]) } })),
                own(&[("ceiling", bools)]),
            )
        }
        // EjectorBlock (HorizontalKineticBlock, stone's properties): a depot on a plate, so the
        // barrel's template for its slots; its horizontal facing and waterlogging;
        // AllShapes.CASING_13PX. Its plate and shaft are parts.
        "weighted_ejector" => (
            "minecraft:barrel",
            Some(by_property(
                "facing",
                &["north", "south", "west", "east"],
                &[[0., 0., 0., 16., 13., 16.]],
            )),
            own(&[
                ("facing", &["north", "south", "west", "east"]),
                ("waterlogged", bools),
            ]),
        ),
        // BeltFunnelBlock: a funnel over a belt (placed by the glue in place of a horizontal
        // funnel above one), its facing, `powered`, `shape` and waterlogging, iron's properties
        // as the funnels'; no item of its own (it drops its funnel). Content shapes select by one
        // property: AllShapes.BELT_FUNNEL_RETRACTED for every shape, turned to its facing.
        "andesite_belt_funnel" | "brass_belt_funnel" => (
            "minecraft:iron_block",
            Some(by_horizontal(&[
                [2., -2., 14., 14., 14., 18.],
                [0., -5., 8., 16., 16., 14.],
            ])),
            own(&[
                ("facing", &["north", "south", "west", "east"]),
                ("powered", bools),
                ("shape", &["retracted", "extended", "pushing", "pulling"]),
                ("waterlogged", bools),
            ]),
        ),
        // DepotBlock: its stacks are a block entity's slots, so the barrel's template too.
        // AllShapes.CASING_13PX.
        "depot" => (
            "minecraft:barrel",
            Some(by_property(
                "open",
                &["false", "true"],
                &[[0., 0., 0., 16., 13., 16.]],
            )),
            None,
        ),
        // BeltBlock: drawn by its renderer only (the block model is a particle). Shapes:
        // BeltShapes' flat part (`box(1,3,0,15,13,16)` along the belt) for every slope, for now.
        "belt" => {
            let along_z = json!([[0.0625, 0.1875, 0.0, 0.9375, 0.8125, 1.0]]);
            let along_x = json!([[0.0, 0.1875, 0.0625, 1.0, 0.8125, 0.9375]]);
            (
                "minecraft:iron_block",
                Some(json!({ "property": "facing", "values": {
                    "north": along_z, "south": along_z, "west": along_x, "east": along_x } })),
                own(&[
                    ("facing", &["north", "south", "west", "east"]),
                    (
                        "slope",
                        &["horizontal", "upward", "downward", "vertical", "sideways"],
                    ),
                    ("part", &["start", "middle", "end", "pulley"]),
                    ("casing", bools),
                    ("waterlogged", bools),
                ]),
            )
        }
        // Create's diodes are vanilla diodes (`AbstractDiodeBlock`, REPEATER's properties): the
        // repeater gives shape, placement and survival; the glue gives the latch's rules.
        "powered_toggle_latch" => (
            "minecraft:repeater",
            None,
            own(&[
                ("facing", &["north", "south", "west", "east"]),
                ("powered", bools),
                ("powering", bools),
            ]),
        ),
        "powered_latch" => (
            "minecraft:repeater",
            None,
            own(&[
                ("facing", &["north", "south", "west", "east"]),
                ("powered", bools),
                ("powered_side", bools),
                ("powering", bools),
            ]),
        ),
        // BrassDiodeBlock (`AbstractDiodeBlock`, REPEATER's properties): the repeater gives
        // shape, placement and survival; the glue gives the diode's rules and its block entity.
        "pulse_repeater" | "pulse_extender" | "pulse_timer" => (
            "minecraft:repeater",
            None,
            own(&[
                ("facing", &["north", "south", "west", "east"]),
                ("inverted", bools),
                ("powered", bools),
                ("powering", bools),
            ]),
        ),
        // RoseQuartzLampBlock: REDSTONE_LAMP's properties (glass sounds, strength 0.3) but no
        // redstone conductor (`shouldCheckWeakPower` false): glowstone's template, without its
        // light (`declaration`); the glue gives its signal and its cluster.
        "rose_quartz_lamp" => (
            "minecraft:glowstone",
            None,
            own(&[("activate", bools), ("powered", bools), ("powering", bools)]),
        ),
        // AnalogLeverBlock (FaceAttachedHorizontalDirectionalBlock, `initialProperties(LEVER)`):
        // the lever's shape, placement and survival; its own states without `powered` (its
        // block entity's state is the signal, the glue's), the state definition's first state
        // its default (FLOOR, NORTH).
        "analog_lever" => (
            "minecraft:lever",
            None,
            own(&[
                ("face", &["floor", "wall", "ceiling"]),
                ("facing", &["north", "south", "west", "east"]),
            ]),
        ),
        // SmartObserverBlock and ThresholdSwitchBlock (DirectedDirectionalBlock,
        // `SharedProperties::stone`, no redstone conductor): full blocks facing their target,
        // TARGET WALL by default; glowstone's template without its light, as the contacts.
        "content_observer" => (
            "minecraft:glowstone",
            None,
            own(&[
                ("facing", &["north", "south", "west", "east"]),
                ("powered", bools),
                ("target", &["wall", "floor", "ceiling"]),
            ]),
        ),
        "stockpile_switch" => (
            "minecraft:glowstone",
            None,
            own(&[
                ("facing", &["north", "south", "west", "east"]),
                ("level", &["0", "1", "2", "3", "4", "5"]),
                ("target", &["wall", "floor", "ceiling"]),
            ]),
        ),
        // CuckooClockBlock (HorizontalKineticBlock, `SharedProperties::wooden`): AllShapes
        // .CUCKOO_CLOCK, a shaft out of its back; the hands are parts.
        "cuckoo_clock" | "mysterious_cuckoo_clock" => (
            "minecraft:stripped_spruce_wood",
            Some(by_horizontal(&[[1., 0., 1., 15., 19., 15.]])),
            own(&[("facing", &["north", "south", "west", "east"])]),
        ),
        // DisplayLinkBlock (WrenchableDirectionalBlock, `SharedProperties::softMetal`): on the
        // clicked face, `powered` by its neighbours; AllShapes.DATA_GATHERER to its facing. Not
        // a full block, so no redstone conductor: glowstone's template without its light.
        "display_link" => (
            "minecraft:glowstone",
            Some(by_facing(&[
                [1., 0., 1., 15., 6., 15.],
                [3., 5., 3., 13., 9., 13.],
            ])),
            own(&[("facing", facing), ("powered", bools)]),
        ),
        // FlapDisplayBlock (HorizontalKineticBlock, `SharedProperties::softMetal`): its facing,
        // joined to the boards above and below it, waterlogged; AllShapes.FLAP_DISPLAY. Its
        // cog turns on its facing's axis.
        "display_board" => (
            "minecraft:gold_block",
            Some(by_horizontal(&[[0., 0., 3., 16., 16., 13.]])),
            own(&[
                ("down", bools),
                ("facing", &["north", "south", "west", "east"]),
                ("up", bools),
                ("waterlogged", bools),
            ]),
        ),
        // FluidTankBlock: `noOcclusion`, see-through windows: glass's template (no random
        // ticks, nothing hidden behind it), copper's sounds. Defaults first: TOP, BOTTOM, WINDOW.
        "fluid_tank" => (
            "minecraft:glass",
            None,
            own(&[
                ("bottom", &["true", "false"]),
                (
                    "shape",
                    &[
                        "window",
                        "plain",
                        "window_nw",
                        "window_sw",
                        "window_ne",
                        "window_se",
                    ],
                ),
                ("top", &["true", "false"]),
            ]),
        ),
        // EncasedFanBlock: any facing, a full block of andesite's properties
        // (SharedProperties.stone).
        "encased_fan" => ("minecraft:andesite", None, own(&[("facing", facing)])),
        // WindmillBearingBlock (BearingBlock): any facing, a full block.
        "windmill_bearing" => ("minecraft:iron_block", None, own(&[("facing", facing)])),
        // GantryShaftBlock (DirectionalKineticBlock): AllShapes.EIGHT_VOXEL_POLE on its facing's
        // axis, stone's properties; its place in the line and redstone power.
        "gantry_shaft" => (
            "minecraft:stone",
            Some(by_facing(&[[4., 0., 4., 12., 16., 12.]])),
            own(&[
                ("facing", facing),
                ("part", &["start", "middle", "end", "single"]),
                ("powered", bools),
            ]),
        ),
        // GaugeBlock (DirectionalAxisKineticBlock, `SharedProperties::wooden`): facing and its
        // shaft's axis; AllShapes.GAUGE_SHAPE_UP to its facing (GaugeShaper's quarter turn for
        // `axis_along_first` is not kept: content shapes select by one property). The base is
        // its model; shaft, heads and dials are parts.
        "speedometer" | "stressometer" => (
            "minecraft:stripped_spruce_wood",
            Some(by_facing(&GAUGE)),
            own(&[("axis_along_first", bools), ("facing", facing)]),
        ),
        // GantryCarriageBlock (DirectionalAxisKineticBlock): a full block of stone's properties.
        "gantry_carriage" => (
            "minecraft:stone",
            None,
            own(&[("axis_along_first", bools), ("facing", facing)]),
        ),
        // MechanicalCrafterBlock (HorizontalKineticBlock, `SharedProperties::softMetal`): a full
        // block, its facing and where it points.
        "mechanical_crafter" => (
            "minecraft:gold_block",
            None,
            own(&[
                ("facing", &["north", "south", "west", "east"]),
                ("pointing", &["up", "left", "down", "right"]),
            ]),
        ),
        // LinearChassisBlock (`SharedProperties::wooden`): a pillar on its axis, glued ends.
        "linear_chassis" | "secondary_linear_chassis" => (
            "minecraft:stripped_spruce_wood",
            None,
            own(&[
                ("axis", axis),
                ("sticky_bottom", bools),
                ("sticky_top", bools),
            ]),
        ),
        // MechanicalBearingBlock (BearingBlock): as the windmill bearing.
        "mechanical_bearing" => ("minecraft:iron_block", None, own(&[("facing", facing)])),
        // ClockworkBearingBlock (BearingBlock): as the mechanical bearing.
        "clockwork_bearing" => ("minecraft:iron_block", None, own(&[("facing", facing)])),
        // PulleyBlock (HorizontalAxisKineticBlock, stone's properties, `noOcclusion`): a full
        // block on a horizontal axis; glass's template hides nothing behind it.
        "rope_pulley" => ("minecraft:glass", None, own(&[("axis", &["x", "z"])])),
        // PulleyBlock.RopeBlock: AllShapes.FOUR_VOXEL_POLE upright, waterlogged as it is placed
        // (in water); no item (`dropOther` none: only a pulley places it).
        "rope" => (
            "minecraft:heavy_core",
            Some(by_property(
                "waterlogged",
                bools,
                &[[6., 0., 6., 10., 16., 10.]],
            )),
            None,
        ),
        // PulleyBlock.MagnetBlock: AllShapes.PULLEY_MAGNET (its plate reaching 3 px into the
        // block below, and the pole), stone's properties.
        "pulley_magnet" => (
            "minecraft:heavy_core",
            Some(by_property(
                "waterlogged",
                bools,
                &[[3., -3., 3., 13., 2., 13.], [6., 0., 6., 10., 16., 10.]],
            )),
            None,
        ),
        // ElevatorPulleyBlock (HorizontalKineticBlock, `SharedProperties::softMetal`):
        // AllShapes.ELEVATOR_PULLEY (two walls across its shaft and the box between them).
        "elevator_pulley" => {
            let px = |b: &[Px]| -> Value {
                json!(b.iter().map(|b| b.map(|v| v / 16.0)).collect::<Vec<_>>())
            };
            let along_x = px(&[
                [0., 0., 0., 16., 16., 2.],
                [0., 0., 14., 16., 16., 16.],
                [2., 0., 2., 14., 14., 14.],
            ]);
            let along_z = px(&[
                [0., 0., 0., 2., 16., 16.],
                [14., 0., 0., 16., 16., 16.],
                [2., 0., 2., 14., 14., 14.],
            ]);
            (
                "minecraft:gold_block",
                Some(json!({ "property": "facing", "values": {
                    "east": along_x, "west": along_x, "north": along_z, "south": along_z } })),
                own(&[("facing", &["north", "south", "west", "east"])]),
            )
        }
        // RedstoneContactBlock (WrenchableDirectionalBlock, stone's properties): its facing and
        // `powered`; not a redstone conductor (`shouldCheckWeakPower` false), so glowstone's
        // template without its light, as the rose quartz lamp; the glue gives its signal.
        "redstone_contact" => (
            "minecraft:glowstone",
            None,
            own(&[("facing", facing), ("powered", bools)]),
        ),
        // ElevatorContactBlock (`SharedProperties::softMetal`): facing (south by default),
        // `calling`, `powered`, `powering`; the redstone contact's template.
        "elevator_contact" => (
            "minecraft:glowstone",
            None,
            own(&[
                ("calling", bools),
                ("facing", &["south", "up", "down", "north", "west", "east"]),
                ("powered", bools),
                ("powering", bools),
            ]),
        ),
        // SailBlock: a thin plane on any facing (AllShapes.SAIL / SAIL_FRAME, forDirectional
        // from up), wood's template with scaffolding's sounds.
        "sail_frame" => (
            "minecraft:oak_planks",
            Some(by_facing(&[[0., 5., 0., 16., 9., 16.]])),
            own(&[("facing", facing)]),
        ),
        n if n.ends_with("_sail") => (
            "minecraft:oak_planks",
            Some(by_facing(&[[0., 5., 0., 16., 10., 16.]])),
            own(&[("facing", facing)]),
        ),
        // TrackBlock: its own shapes (TrackShape), drawn by its blockstate's models; collision
        // as Create's (AllShapes.TRACK_COLLISION, none on a slope), used for the outline too.
        "track" => {
            let flat = json!([[0.0, 0.0, 0.0, 1.0, 0.125, 1.0]]);
            // The default first (TrackBlock's default state is ZO).
            let shapes = [
                "zo", "none", "xo", "pd", "nd", "an", "as", "ae", "aw", "tn", "ts", "te", "tw",
                "cr_o", "cr_d", "cr_pdx", "cr_pdz", "cr_ndx", "cr_ndz",
            ];
            let values: Map<String, Value> = shapes
                .iter()
                .map(|s| {
                    let boxes = if matches!(*s, "an" | "as" | "ae" | "aw") {
                        json!([])
                    } else {
                        flat.clone()
                    };
                    (s.to_string(), boxes)
                })
                .collect();
            (
                "minecraft:iron_block",
                Some(json!({ "property": "shape", "values": values })),
                own(&[("shape", &shapes), ("turn", bools), ("waterlogged", bools)]),
            )
        }
        // FunnelBlock: facing (away from the inventory it is on), `extracting`, `powered`;
        // AllShapes.FUNNEL_FLOOR turned to its facing: the half against the inventory.
        "andesite_funnel" | "brass_funnel" => (
            "minecraft:iron_block",
            Some(by_facing(&[[1., 0., 1., 15., 8., 15.]])),
            own(&[
                ("extracting", bools),
                ("facing", facing),
                ("powered", bools),
                ("waterlogged", bools),
            ]),
        ),
        // ItemDrainBlock: a full block here, so items land on top of it.
        "item_drain" => ("minecraft:iron_block", None, None),
        // SpoutBlock: a copper-propertied block, AllShapes.SPOUT approximated by its body.
        "spout" => (
            "minecraft:copper_block",
            Some(by_property(
                "waterlogged",
                bools,
                &[[1., 2., 1., 15., 14., 15.]],
            )),
            own(&[("waterlogged", bools)]),
        ),
        // SawBlock (DirectionalAxisKineticBlock): facing, its shaft's axis, `flipped`; a full
        // block here, so items land on top of it.
        "mechanical_saw" => (
            "minecraft:iron_block",
            None,
            own(&[
                ("axis_along_first", bools),
                ("facing", facing),
                ("flipped", bools),
                ("waterlogged", bools),
            ]),
        ),
        // DeployerBlock (DirectionalAxisKineticBlock): AllShapes.DEPLOYER_INTERACTION (the 12 px
        // casing and its pole through the block) to its facing, stone's properties.
        "deployer" => (
            "minecraft:stone",
            Some(by_facing(&[
                [0., 0., 0., 16., 12., 16.],
                [5., 0., 5., 11., 16., 11.],
            ])),
            own(&[("axis_along_first", bools), ("facing", facing)]),
        ),
        // DrillBlock (DirectionalKineticBlock): any facing, a full block of iron's
        // properties here (Create's is a casing with the head out front).
        "mechanical_drill" => (
            "minecraft:iron_block",
            None,
            own(&[("facing", facing), ("waterlogged", bools)]),
        ),
        // ChuteBlock: a barrel's template for its one slot; gold's properties; its facing
        // (down, or a diagonal's), `shape` and waterlogging; a full block here, so items land on
        // top of it.
        "chute" => (
            "minecraft:barrel",
            Some(by_property(
                "facing",
                &["down", "north", "south", "west", "east"],
                &[[0., 0., 0., 16., 16., 16.]],
            )),
            own(&[
                ("facing", &["down", "north", "south", "west", "east"]),
                ("shape", &["normal", "window", "encased", "intersection"]),
                ("waterlogged", bools),
            ]),
        ),
        // SmartChuteBlock: the chute's barrel template for its one slot, a full block; only
        // `powered`, true by default (`registerDefaultState`), the glue sets it from redstone.
        "smart_chute" => (
            "minecraft:barrel",
            None,
            own(&[("powered", &["true", "false"])]),
        ),
        // ItemVaultBlock: a barrel's template for its 20 slots (`vaultCapacity`), a full block;
        // its horizontal axis (x by default) and `large`, set by the glue as vaults form.
        "item_vault" => (
            "minecraft:barrel",
            None,
            own(&[("axis", &["x", "z"]), ("large", bools)]),
        ),
        // SchematicannonBlock: its five slots are a block entity's, so the barrel's template
        // (dispenser's properties); AllShapes.SCHEMATICANNON_SHAPE. Its barrel is drawn by the
        // glue (`part_schematicannon_*`).
        "schematicannon" => (
            "minecraft:barrel",
            Some(by_property(
                "open",
                &["false", "true"],
                &[[1., 0., 1., 15., 8., 15.], [0.5, 8., 0.5, 15.5, 11., 15.5]],
            )),
            None,
        ),
        // SchematicTableBlock (HorizontalDirectionalBlock): its two slots a block entity's (the
        // barrel's template), `facing` north first; AllShapes.SCHEMATICS_TABLE (symmetric, so
        // as written for north).
        "schematic_table" => (
            "minecraft:barrel",
            Some(by_horizontal(&[
                [4., 0., 4., 12., 12., 12.],
                [0., 11., 2., 16., 14., 14.],
            ])),
            own(&[("facing", &["north", "south", "west", "east"])]),
        ),
        // StationBlock: a full block of gold's properties (its track target is the glue's).
        "track_station" => ("minecraft:gold_block", None, None),
        // SignalBlock: beside a track it splits it into sections; its type and redstone power.
        "track_signal" => (
            "minecraft:iron_block",
            None,
            own(&[
                ("powered", bools),
                ("type", &["entry_signal", "cross_signal"]),
            ]),
        ),
        // StandardBogeyBlock: on the track, along an axis; its wheels drawn by the train.
        "small_bogey" | "large_bogey" => (
            "minecraft:iron_block",
            None,
            own(&[("axis", &["z", "x"]), ("waterlogged", bools)]),
        ),
        // MechanicalPistonBlock (DirectionalAxisKineticBlock): facing, its shaft's axis
        // (`axis_along_first`) and `state`; a full block here (Create's casing is 12px).
        "mechanical_piston" | "sticky_mechanical_piston" => (
            "minecraft:andesite",
            None,
            own(&[
                ("axis_along_first", bools),
                ("facing", facing),
                ("state", &["retracted", "moving", "extended"]),
            ]),
        ),
        // PistonExtensionPoleBlock: AllShapes.FOUR_VOXEL_POLE along its facing.
        "piston_extension_pole" => (
            "minecraft:andesite",
            Some(by_facing(&[[6., 0., 6., 10., 16., 10.]])),
            own(&[("facing", facing)]),
        ),
        // MechanicalPistonHeadBlock: its plate and the pole into it (vanilla's piston head).
        "mechanical_piston_head" => (
            "minecraft:andesite",
            Some(by_facing(&[
                [0., 12., 0., 16., 16., 16.],
                [6., 0., 6., 10., 12., 10.],
            ])),
            own(&[("facing", facing), ("type", &["normal", "sticky"])]),
        ),
        // FluidPipeBlock: a side open where it connects (the glue sets them,
        // `updateBlockState`); copper's properties; the core as its shape.
        "fluid_pipe" => (
            "minecraft:copper_block",
            Some(by_property("north", bools, &[[4., 4., 4., 12., 12., 12.]])),
            own(&[
                ("down", bools),
                ("east", bools),
                ("north", bools),
                ("south", bools),
                ("up", bools),
                ("waterlogged", bools),
                ("west", bools),
            ]),
        ),
        // PumpBlock: any facing, AllShapes.PUMP; copper's properties.
        "mechanical_pump" => (
            "minecraft:copper_block",
            Some(by_facing(&[[2., 0., 2., 14., 16., 14.]])),
            own(&[("facing", facing), ("waterlogged", bools)]),
        ),
        // BasinBlock: a barrel's template for its 18 slots (inputs 0–8, outputs 9–17), stone's
        // properties, netherite's sounds; AllShapes.BASIN_COLLISION_SHAPE (a floor at 5/16
        // inside a rim).
        "basin" => (
            "minecraft:barrel",
            Some(by_property(
                "facing",
                &["down", "north", "south", "west", "east"],
                &BASIN,
            )),
            own(&[("facing", &["down", "north", "south", "west", "east"])]),
        ),
        // MechanicalMixerBlock: a casing of stone's properties; its cog and head are parts.
        "mechanical_mixer" => ("minecraft:andesite", None, None),
        // BlazeBurnerBlock: its heat level and horizontal facing, gold's properties
        // (SharedProperties.softMetal), AllShapes.HEATER_BLOCK_SHAPE; drawn by our multipart
        // blockstate (`blaze_burner_states`). LitBlazeBurnerBlock: its flame instead.
        "blaze_burner" => (
            "minecraft:gold_block",
            Some(by_property("blaze", HEATS, &[[1., 0., 1., 15., 14., 15.]])),
            own(&[
                ("blaze", HEATS),
                ("facing", &["north", "south", "west", "east"]),
            ]),
        ),
        "lit_blaze_burner" => (
            "minecraft:gold_block",
            Some(by_property(
                "flame_type",
                &["regular", "soul"],
                &[[1., 0., 1., 15., 14., 15.]],
            )),
            own(&[
                ("facing", &["north", "south", "west", "east"]),
                ("flame_type", &["regular", "soul"]),
            ]),
        ),
        // CrushingWheelBlock (RotatedPillarKineticBlock): a full block on an axis, stone's
        // properties, not hiding its neighbours; an iron chain's template places it on the
        // clicked face's axis.
        "crushing_wheel" => (
            "minecraft:iron_chain",
            Some(by_axis(&[[0., 0., 0., 16., 16., 16.]])),
            None,
        ),
        // CrushingWheelControllerBlock: air to players (no shape, invisible, replaceable, no
        // drops), placed and removed by the wheels.
        "crushing_wheel_controller" => (
            "minecraft:structure_void",
            Some(json!({ "property": "valid", "values": { "false": [], "true": [] } })),
            own(&[("facing", facing), ("valid", bools)]),
        ),
        // CartAssemblerBlock.MinecartAnchorBlock: a plain block with HORIZONTAL_AXIS, stone's
        // properties and an empty shape (`Shapes.empty()`: no outline, collision or occlusion;
        // so not a solid render, letting light and sky through, as glass's template does). No
        // item: only contraptions hold it.
        "minecart_anchor" => (
            "minecraft:glass",
            Some(json!({ "property": "axis", "values": { "x": [], "z": [] } })),
            own(&[("axis", &["x", "z"])]),
        ),
        "hand_crank" => (
            "minecraft:heavy_core",
            Some(by_facing(&CRANK)),
            own(&[("facing", facing), ("waterlogged", bools)]),
        ),
        // BeltTunnelBlock (andesite): its own `axis` (HORIZONTAL_AXIS) and shape. Content shapes select by one
        // property, so every state takes the cross's boxes (BeltTunnelShapes, all four sides
        // open: a closed side's lower wall is missing).
        "andesite_tunnel" | "brass_tunnel" => {
            let boxes: Vec<Value> = [
                [2., -5., 2., 14., 16., 14.],
                [0., -5., 0., 2., 16., 2.],
                [14., -5., 0., 16., 16., 2.],
                [0., -5., 14., 2., 16., 16.],
                [14., -5., 14., 16., 16., 16.],
                [2., 10., 0., 14., 16., 2.],
                [2., 10., 14., 14., 16., 16.],
                [0., 10., 2., 2., 16., 14.],
                [14., 10., 2., 16., 16., 14.],
            ]
            .iter()
            .map(|b: &Px| json!(b.map(|v| v / 16.0)))
            .collect();
            (
                // BrassTunnelBlock: brass's properties (gold), andesite's for the other.
                if b.name == "brass_tunnel" {
                    "minecraft:gold_block"
                } else {
                    "minecraft:polished_andesite"
                },
                Some(json!({ "property": "axis", "values": {
                    "x": boxes, "z": boxes } })),
                own(&[
                    ("axis", &["x", "z"]),
                    (
                        "shape",
                        &["straight", "window", "closed", "t_left", "t_right", "cross"],
                    ),
                ]),
            )
        }
        // RedstoneLinkBlock (`SharedProperties::wooden`): a tablet on the block behind it,
        // AllShapes.REDSTONE_BRIDGE (VERTICAL_TABLET_SHAPE facing south, the square tablet up or
        // down); the glue gives its signal, network and survival.
        "redstone_link" => {
            let side = |b: Px| json!([b.map(|v| v / 16.0)]);
            (
                "minecraft:stripped_spruce_wood",
                Some(json!({ "property": "facing", "values": {
                    "south": side([3., 1., -1., 13., 15., 3.]),
                    "north": side([3., 1., 13., 13., 15., 17.]),
                    "east": side([-1., 1., 3., 3., 15., 13.]),
                    "west": side([13., 1., 3., 17., 15., 13.]),
                    "up": side([2., -1., 2., 14., 3., 14.]),
                    "down": side([2., 13., 2., 14., 17., 14.]) } })),
                own(&[("facing", facing), ("powered", bools), ("receiver", bools)]),
            )
        }
        // FlywheelBlock (RotatedPillarKineticBlock, `SharedProperties::softMetal`, `noOcclusion`):
        // AllShapes.LARGE_GEAR on its axis; the wheel and its shaft are drawn turning.
        "flywheel" => (
            "minecraft:iron_chain",
            Some(by_axis(&[LARGE_GEAR, SIX_VOXEL_POLE])),
            own(&[("axis", &["y", "x", "z"])]),
        ),
        // TurntableBlock (KineticBlock, `SharedProperties::wooden`): no properties of its own, so
        // no declared shape (content shapes select by a property): a full block, not
        // AllShapes.TURNTABLE_SHAPE. Its plate is drawn turning.
        "turntable" => ("minecraft:oak_planks", None, None),
        // LargeWaterWheelBlock (RotatedPillarKineticBlock, `SharedProperties::wooden`,
        // `noOcclusion`): its axis (y first) and `extension`; glass's template hides nothing
        // beside it (the wheel is drawn as a part, not in the chunk mesh).
        "large_water_wheel" => (
            "minecraft:glass",
            None,
            own(&[("axis", &["y", "x", "z"]), ("extension", bools)]),
        ),
        // WaterWheelStructuralBlock (DirectionalBlock, `noOcclusion`, RenderShape.INVISIBLE):
        // placed around a large wheel by the glue, pointing at it; no item.
        "water_wheel_structure" => (
            "minecraft:glass",
            None,
            own(&[("facing", &["down", "up", "north", "south", "west", "east"])]),
        ),
        // GirderEncasedShaftBlock (HorizontalAxisKineticBlock, `SharedProperties::softMetal`,
        // netherite's sounds): AllShapes.GIRDER_BEAM_SHAFT (the beam across its axis and the
        // shaft along it); `top`/`bottom` as the girder's; no item (a shaft on a girder).
        "metal_girder_encased_shaft" => {
            let along = |a: &str| -> Vec<Value> {
                let (beam, shaft): (Px, Px) = if a == "x" {
                    ([4., 2., 0., 12., 14., 16.], [0., 5., 5., 16., 11., 11.])
                } else {
                    ([0., 2., 4., 16., 14., 12.], [5., 5., 0., 11., 11., 16.])
                };
                [beam, shaft]
                    .iter()
                    .map(|b| json!(b.map(|v| v / 16.0)))
                    .collect()
            };
            (
                "minecraft:iron_chain",
                Some(json!({ "property": "axis", "values": { "x": along("x"), "z": along("z") } })),
                own(&[
                    ("axis", &["x", "z"]),
                    ("bottom", bools),
                    ("top", bools),
                    ("waterlogged", bools),
                ]),
            )
        }
        _ => return None,
    };
    // KineticBlock.getRenderShape: shafts and cogs are all moving parts; casings, gearboxes and
    // motors keep their model and show their shafts as parts.
    let animated = matches!(
        b.name.as_str(),
        "shaft"
            | "powered_shaft"
            | "cogwheel"
            | "large_cogwheel"
            | "hand_crank"
            | "belt"
            | "crushing_wheel"
            | "gantry_shaft"
            | "flywheel"
            | "turntable"
            | "large_water_wheel"
    );
    Some(Decision::Declared {
        template,
        sounds,
        requires_tool: requires_tool(b),
        shapes,
        own,
        animated,
        item: !matches!(
            b.name.as_str(),
            "belt"
                | "crushing_wheel_controller"
                | "minecart_anchor"
                | "powered_shaft"
                | "rope"
                | "pulley_magnet"
                | "andesite_belt_funnel"
                | "brass_belt_funnel"
                | "water_wheel_structure"
                | "metal_girder_encased_shaft"
        ),
    })
}

/// `DyeColor`'s names, in its order.
pub const DYES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// The colour a dyed block's name starts with (`black_toolbox` is black).
fn dyed<'a>(name: &'a str, family: &str) -> Option<&'a str> {
    let colour = name.strip_suffix(family)?.strip_suffix('_')?;
    DYES.contains(&colour).then_some(colour)
}

/// Blocks with state properties no vanilla block has (gap 2) whose behaviour is a template's plus
/// the glue's: valve handles, sliding doors, toolboxes, postboxes and nixie tubes. Each is declared
/// with its own `state_properties`, Create's default state first, and Create's shape selected by
/// one property.
fn own_states(b: &CreateBlock) -> Option<Decision> {
    use crate::shapes::*;
    let n = b.name.as_str();
    let own = |props: &[(&str, &[&str])]| -> Value {
        let mut m = Map::new();
        for (k, v) in props {
            m.insert((*k).into(), json!(v));
        }
        Value::Object(m)
    };
    let bools: &[&str] = &["false", "true"];
    // HORIZONTAL_FACING's values; `any()` is north.
    let horizontal: &[&str] = &["north", "south", "west", "east"];
    let px = |boxes: &[Px]| -> Value {
        json!(
            boxes
                .iter()
                .map(|b| b.map(|v| v / 16.0))
                .collect::<Vec<_>>()
        )
    };
    let declared = |template: &'static str,
                    sounds: &'static str,
                    requires_tool: bool,
                    shapes: Option<Value>,
                    own: Value,
                    item: bool| Decision::Declared {
        template,
        sounds: Some(sounds),
        requires_tool: Some(requires_tool),
        shapes,
        own: Some(own),
        animated: false,
        item,
    };
    // ValveHandleBlock (HandCrankBlock: DirectionalKineticBlock, waterlogged), copper's
    // properties (`BuilderTransformers.valveHandle`): the hand crank's heavy core template,
    // AllShapes.VALVE_HANDLE to its facing. `FACING` has no default of its own: `any()` is down.
    // Its model is drawn in the chunk mesh (Create turns the block's own model; not here).
    if n == "copper_valve_handle" || dyed(n, "valve_handle").is_some() {
        return Some(declared(
            "minecraft:heavy_core",
            "minecraft:copper_block",
            true,
            Some(by_facing(&[
                [5., 0., 5., 11., 4., 11.],
                [1., 3., 1., 15., 8., 15.],
            ])),
            own(&[
                ("facing", &["down", "up", "north", "south", "west", "east"]),
                ("waterlogged", bools),
            ]),
            true,
        ));
    }
    // SlidingDoorBlock (DoorBlock, `initialProperties(IRON_DOOR)`, strength 3/6, the sound of
    // its BlockSetType): the iron door's template (both halves, hinges, redstone, no opening by
    // hand: the glue opens it as Create does) with DoorBlock's `powered` and Create's `visible`
    // (true in `any()`). Without `shapes` it keeps the door's: Create's closed shape; open, its
    // panel slides beside the frame (SlidingDoorShapes), which one property can't select.
    if let Some(sounds) = match n {
        "andesite_door" | "brass_door" | "copper_door" => Some("minecraft:stone"),
        "train_door" => Some("minecraft:netherite_block"),
        "framed_glass_door" => Some("minecraft:glass"),
        _ => None,
    } {
        return Some(declared(
            "minecraft:iron_door",
            sounds,
            true,
            None,
            own(&[
                ("facing", horizontal),
                ("half", &["lower", "upper"]),
                ("hinge", &["left", "right"]),
                ("open", bools),
                ("powered", bools),
                ("visible", &["true", "false"]),
            ]),
            true,
        ));
    }
    // StickerBlock (WrenchableDirectionalBlock, `SharedProperties::stone`): its facing (`any()`
    // is down), `powered` and `extended`, a full block; the glue flips `extended` on each
    // rising signal and contraptions take the block an extended one faces.
    if n == "sticker" {
        return Some(declared(
            "minecraft:andesite",
            "minecraft:stone",
            true,
            None,
            own(&[
                ("extended", bools),
                ("facing", &["down", "up", "north", "south", "west", "east"]),
                ("powered", bools),
            ]),
            true,
        ));
    }
    // HarvesterBlock and PloughBlock (AttachedActorBlock, `SharedProperties::stone`): a
    // horizontal facing and waterlogging, AllShapes.HARVESTER_BASE (written facing south, the
    // plate against the block behind) turned to it; their behaviour is on contraptions.
    if n == "mechanical_harvester" || n == "mechanical_plough" {
        return Some(declared(
            "minecraft:andesite",
            "minecraft:stone",
            true,
            Some(by_horizontal(&[[0., 2., 13., 16., 14., 16.]])),
            own(&[("facing", horizontal), ("waterlogged", bools)]),
            true,
        ));
    }
    // ToolboxBlock (`SharedProperties::wooden`, wood sounds): a container whose menu opens on use,
    // so the barrel's template; AllShapes.TOOLBOX turned to its facing.
    if dyed(n, "toolbox").is_some() {
        return Some(declared(
            "minecraft:barrel",
            "minecraft:oak_planks",
            false,
            Some(by_horizontal(&[[1., 0., 4., 15., 9., 12.]])),
            own(&[("facing", horizontal), ("waterlogged", bools)]),
            true,
        ));
    }
    // PackagerBlock (BuilderTransformers.packager: `SharedProperties::softMetal`, netherite's
    // sounds, not a redstone conductor): facing (DirectionalBlock's, down first), `powered`,
    // `linked`; a full block. Its box and address are the native mod's.
    if n == "packager" {
        let full = [[0., 0., 0., 16., 16., 16.]];
        return Some(declared(
            "minecraft:heavy_core",
            "minecraft:netherite_block",
            true,
            Some(by_property("powered", bools, &full)),
            own(&[
                ("facing", &["down", "up", "north", "south", "west", "east"]),
                ("powered", bools),
                ("linked", bools),
            ]),
            true,
        ));
    }
    // FrogportBlock (`SharedProperties::softMetal`, netherite's sounds): no properties of its
    // own, so heavy core's waterlogging carries AllShapes.PACKAGE_PORT. Its packages, filter and
    // chain are the native mod's.
    if n == "package_frogport" {
        return Some(declared(
            "minecraft:heavy_core",
            "minecraft:netherite_block",
            true,
            Some(by_property(
                "waterlogged",
                bools,
                &[[0., 0., 0., 16., 4., 16.], [2., 2., 2., 14., 14., 14.]],
            )),
            own(&[("waterlogged", bools)]),
            true,
        ));
    }
    // ChainConveyorBlock (KineticBlock, `SharedProperties::stone`): no properties of its own;
    // its collision is a full block (the wider CHAIN_CONVEYOR_INTERACTION reaches past the
    // block, which a declared shape cannot). Its kinetic part and chains are the native mod's.
    if n == "chain_conveyor" {
        return Some(declared(
            "minecraft:heavy_core",
            "minecraft:stone",
            true,
            Some(by_property(
                "waterlogged",
                bools,
                &[[0., 0., 0., 16., 16., 16.]],
            )),
            own(&[("waterlogged", bools)]),
            true,
        ));
    }
    // PostboxBlock (`SharedProperties::wooden`): facing, `open` (its menu is open), waterlogged;
    // AllShapes.POSTBOX (the same box turned any way).
    if dyed(n, "postbox").is_some() {
        return Some(declared(
            "minecraft:heavy_core",
            "minecraft:stripped_spruce_wood",
            false,
            Some(by_horizontal(&[[2., 0., 0., 14., 14., 16.]])),
            own(&[
                ("facing", horizontal),
                ("open", bools),
                ("waterlogged", bools),
            ]),
            true,
        ));
    }
    // ControlsBlock (`SharedProperties::softMetal`, netherite sounds, pickaxe): facing (toward
    // whoever placed it), `open` (on a moving contraption), `virtual`, waterlogged; AllShapes
    // .CONTROLS. A train assembled with it is driven by `/create_debug drive` (riding is gap 3).
    if n == "controls" {
        return Some(declared(
            "minecraft:heavy_core",
            "minecraft:netherite_block",
            true,
            Some(by_horizontal(&[
                [0., 0., 6., 16., 16., 16.],
                [0., 0., 4., 16., 2., 16.],
            ])),
            own(&[
                ("facing", horizontal),
                ("open", bools),
                ("virtual", bools),
                ("waterlogged", bools),
            ]),
            true,
        ));
    }
    // NixieTubeBlock (DoubleFaceAttachedBlock, `SharedProperties::softMetal`, light 5): its face
    // (`double_face`, floor by default), facing and waterlogging. Only the orange one
    // (`nixie_tube`) has an item. Shapes by facing: AllShapes.NIXIE_TUBE on the floor, along
    // the axis clockwise of its facing (one property can't also select the wall and ceiling).
    if n == "nixie_tube" || dyed(n, "nixie_tube").is_some() {
        let along_z = px(&[[9., 0., 5., 15., 12., 11.], [1., 0., 5., 7., 12., 11.]]);
        let along_x = px(&[[5., 0., 9., 11., 12., 15.], [5., 0., 1., 11., 12., 7.]]);
        return Some(declared(
            "minecraft:heavy_core",
            "minecraft:gold_block",
            true,
            Some(json!({ "property": "facing", "values": {
                "north": along_x, "south": along_x, "east": along_z, "west": along_z } })),
            own(&[
                (
                    "double_face",
                    &["floor", "wall", "wall_reversed", "ceiling"],
                ),
                ("facing", horizontal),
                ("waterlogged", bools),
            ]),
            n == "nixie_tube",
        ));
    }
    let facing: &[&str] = &["down", "up", "north", "south", "west", "east"];
    // GirderBlock (`SharedProperties::softMetal`, netherite's sounds): `axis` (y by default),
    // `bottom`, `top`, `x`, `z` set by the glue as Create's `updateState`. Content shapes select
    // by one property: by `x`, AllShapes.GIRDER_CROSS when it runs along x (also for a lone x
    // beam), else GIRDER_BEAM along z and EIGHT_VOXEL_POLE (also for a lone pole).
    if n == "metal_girder" {
        let beam_x = [0., 2., 4., 16., 14., 12.];
        let beam_z = [4., 2., 0., 12., 14., 16.];
        return Some(declared(
            "minecraft:iron_chain",
            "minecraft:netherite_block",
            true,
            Some(json!({ "property": "x", "values": {
                "true": px(&[beam_x, beam_z, [3., 0., 3., 13., 16., 13.]]),
                "false": px(&[beam_z, [4., 0., 4., 12., 16., 12.]]) } })),
            own(&[
                ("axis", &["y", "x", "z"]),
                ("bottom", bools),
                ("top", bools),
                ("waterlogged", bools),
                ("x", bools),
                ("z", bools),
            ]),
            true,
        ));
    }
    // BracketBlock (WrenchableDirectionalBlock): never placed on its own (`BracketBlockItem`
    // puts it on a shaft, cog or pipe); its states are the bracket models the glue draws.
    if n == "metal_bracket" || n == "wooden_bracket" {
        let metal = n == "metal_bracket";
        return Some(declared(
            "minecraft:heavy_core",
            if metal {
                "minecraft:netherite_block"
            } else {
                "minecraft:scaffolding"
            },
            metal,
            None,
            own(&[
                ("axis_along_first", bools),
                ("facing", facing),
                ("type", &["pipe", "cog", "shaft"]),
            ]),
            true,
        ));
    }
    // RadialChassisBlock (AbstractChassisBlock: a pillar, `SharedProperties::wooden`): its axis
    // (y by default) and four glued sides.
    if n == "radial_chassis" {
        return Some(declared(
            "minecraft:stripped_spruce_wood",
            "minecraft:stripped_spruce_wood",
            false,
            None,
            own(&[
                ("axis", &["y", "x", "z"]),
                ("sticky_east", bools),
                ("sticky_north", bools),
                ("sticky_south", bools),
                ("sticky_west", bools),
            ]),
            true,
        ));
    }
    // CreativeCrateBlock (CrateBlock: WrenchableDirectionalBlock, `SharedProperties::stone`):
    // AllShapes.CRATE_BLOCK_SHAPE whatever its facing; its supply is the glue's.
    if n == "creative_crate" {
        return Some(declared(
            "minecraft:andesite",
            "minecraft:stone",
            true,
            Some(by_property(
                "facing",
                facing,
                &[[1., 0., 1., 15., 14., 15.]],
            )),
            own(&[("facing", facing)]),
            true,
        ));
    }
    // PlacardBlock (FaceAttachedHorizontalDirectionalBlock, `SharedProperties::copperMetal`):
    // the lever's template gives its placement, survival and signal (15 while `powered`, strong
    // into the block it hangs on: Create's `getSignal` / `getDirectSignal`); the lever's shapes.
    // FLOOR, NORTH by default.
    if n == "placard" {
        return Some(declared(
            "minecraft:lever",
            "minecraft:copper_block",
            true,
            None,
            own(&[
                ("face", &["floor", "wall", "ceiling"]),
                ("facing", horizontal),
                ("powered", bools),
                ("waterlogged", bools),
            ]),
            true,
        ));
    }
    // ClipboardBlock (FaceAttachedHorizontalDirectionalBlock, `SharedProperties::wooden`): the
    // lever's placement and survival; `written` from the item's pages.
    if n == "clipboard" {
        return Some(declared(
            "minecraft:lever",
            "minecraft:stripped_spruce_wood",
            false,
            None,
            own(&[
                ("face", &["floor", "wall", "ceiling"]),
                ("facing", horizontal),
                ("waterlogged", bools),
                ("written", bools),
            ]),
            true,
        ));
    }
    // DeskBellBlock (WrenchableDirectionalBlock, UP by default): AllShapes.DESK_BELL to its
    // facing; its signal is the glue's.
    if n == "desk_bell" {
        return Some(declared(
            "minecraft:heavy_core",
            "minecraft:stone",
            false,
            Some(by_facing(&[
                [3., 0., 3., 13., 3., 13.],
                [4., 0., 4., 12., 9., 12.],
            ])),
            own(&[
                ("facing", &["up", "down", "north", "south", "west", "east"]),
                ("powered", bools),
                ("waterlogged", bools),
            ]),
            true,
        ));
    }
    // HauntedBellBlock and PeculiarBellBlock (AbstractBellBlock extends vanilla's BellBlock,
    // `BuilderTransformers.bell`: soft metal, anvil sounds): the bell's template (ringing on a
    // hit or a rising signal, its block entity), with the bell's own properties.
    if n == "haunted_bell" || n == "peculiar_bell" {
        return Some(declared(
            "minecraft:bell",
            "minecraft:anvil",
            true,
            None,
            own(&[
                (
                    "attachment",
                    &["floor", "ceiling", "single_wall", "double_wall"],
                ),
                ("facing", horizontal),
                ("powered", bools),
            ]),
            true,
        ));
    }
    // TableClothBlock (`BuilderTransformers.tableCloth`): `entity` (a shop's block entity, false
    // by default); the carpet's template is its collision (TABLE_CLOTH_OCCLUSION). Dyed ones are
    // wool, the andesite one stone, brass and copper metal.
    if n.ends_with("_table_cloth") {
        let (sounds, tool) = match n {
            "andesite_table_cloth" => ("minecraft:andesite", true),
            "brass_table_cloth" => ("minecraft:gold_block", true),
            "copper_table_cloth" => ("minecraft:copper_block", true),
            _ => ("minecraft:white_wool", false),
        };
        return Some(declared(
            "minecraft:white_carpet",
            sounds,
            tool,
            None,
            own(&[("entity", bools)]),
            true,
        ));
    }
    // CardboardBlock (`initialProperties(MUSHROOM_STEM)`, chiseled bookshelf sounds): its
    // horizontal axis, from the player's facing (the glue).
    if n == "cardboard_block" || n == "bound_cardboard_block" {
        return Some(declared(
            "minecraft:oak_planks",
            "minecraft:chiseled_bookshelf",
            false,
            None,
            own(&[("axis", &["x", "z"])]),
            true,
        ));
    }
    // ItemHatchBlock (HorizontalDirectionalBlock, `SharedProperties::softMetal`, netherite's
    // sounds): `facing` toward the inventory, `open` for 10 ticks after a deposit;
    // AllShapes.ITEM_HATCH against that inventory.
    if n == "item_hatch" {
        return Some(declared(
            "minecraft:heavy_core",
            "minecraft:netherite_block",
            true,
            Some(by_horizontal(&[
                [1., 0., 0., 15., 16., 2.],
                [2., 2., 0., 14., 13., 3.8],
                [2., 4., 0., 14., 11., 5.8],
                [2., 6., 0., 14., 9., 7.8],
            ])),
            own(&[
                ("facing", horizontal),
                ("open", bools),
                ("waterlogged", bools),
            ]),
            true,
        ));
    }
    None
}

/// Phase 5's fluid blocks with state of their own (`AllBlocks`: all `SharedProperties
/// ::copperMetal`, copper's sounds, pickaxe): valves, smart, glass and encased pipes, the hose
/// pulley, the creative fluid tank, steam whistles and their extensions. Their behaviour is the
/// glue's; Create's shapes selected by one property.
fn fluid_blocks(b: &CreateBlock) -> Option<Decision> {
    use crate::shapes::*;
    let own = |props: &[(&str, &[&str])]| -> Value {
        let mut m = Map::new();
        for (k, v) in props {
            m.insert((*k).into(), json!(v));
        }
        Value::Object(m)
    };
    let bools: &[&str] = &["false", "true"];
    let facing: &[&str] = &["down", "up", "north", "south", "west", "east"];
    let horizontal: &[&str] = &["north", "south", "west", "east"];
    let sizes: &[&str] = &["medium", "small", "large"];
    let declared = |template: &'static str, shapes: Option<Value>, own: Value, item: bool| {
        Decision::Declared {
            template,
            sounds: Some("minecraft:copper_block"),
            requires_tool: Some(true),
            shapes,
            own: Some(own),
            animated: false,
            item,
        }
    };
    Some(match b.name.as_str() {
        // FluidValveBlock (DirectionalAxisKineticBlock, waterlogged): AllShapes.FLUID_VALVE's
        // middle (its pole runs along the pipe axis, which facing and `axis_along_first` set
        // together: one property can't select it). Not a full block: heavy core's template.
        "fluid_valve" => declared(
            "minecraft:heavy_core",
            Some(by_property(
                "facing",
                facing,
                &[[2., 2., 2., 14., 14., 14.]],
            )),
            own(&[
                ("axis_along_first", bools),
                ("enabled", bools),
                ("facing", facing),
                ("waterlogged", bools),
            ]),
            true,
        ),
        // SmartFluidPipeBlock (FaceAttachedHorizontalDirectionalBlock, waterlogged): the middle
        // of AllShapes.SMART_FLUID_PIPE_* (floor first, north).
        "smart_fluid_pipe" => declared(
            "minecraft:heavy_core",
            Some(by_property(
                "face",
                &["floor", "wall", "ceiling"],
                &[[3., 3., 3., 13., 13., 13.]],
            )),
            own(&[
                ("face", &["floor", "wall", "ceiling"]),
                ("facing", horizontal),
                ("waterlogged", bools),
            ]),
            true,
        ),
        // GlassFluidPipeBlock (AxisPipeBlock: a RotatedPillarBlock, y by default; `alt`,
        // waterlogged; `noOcclusion`): AllShapes.EIGHT_VOXEL_POLE. No item: a wrench makes it
        // from a pipe.
        "glass_fluid_pipe" => declared(
            "minecraft:heavy_core",
            Some(by_axis(&[[4., 0., 4., 12., 16., 12.]])),
            own(&[
                ("alt", bools),
                ("axis", &["y", "x", "z"]),
                ("waterlogged", bools),
            ]),
            false,
        ),
        // EncasedPipeBlock: a copper casing (a full block) open on the sides its pipe had. No
        // item: copper casing on a pipe makes it.
        "encased_fluid_pipe" => declared(
            "minecraft:copper_block",
            None,
            own(&[
                ("down", bools),
                ("east", bools),
                ("north", bools),
                ("south", bools),
                ("up", bools),
                ("west", bools),
            ]),
            false,
        ),
        // HosePulleyBlock (HorizontalKineticBlock, `noOcclusion`): a full block of glass's
        // template (nothing hidden behind it).
        "hose_pulley" => declared(
            "minecraft:glass",
            None,
            own(&[("facing", horizontal)]),
            true,
        ),
        // FluidTankBlock.creative: the fluid tank's states and template.
        "creative_fluid_tank" => declared(
            "minecraft:glass",
            None,
            own(&[
                ("bottom", &["true", "false"]),
                (
                    "shape",
                    &[
                        "window",
                        "plain",
                        "window_nw",
                        "window_sw",
                        "window_ne",
                        "window_se",
                    ],
                ),
                ("top", &["true", "false"]),
            ]),
            true,
        ),
        // WhistleBlock: HORIZONTAL_FACING, `powered`, `size` (medium by default), `wall`;
        // AllShapes.WHISTLE_*_FLOOR by size (a wall whistle's base turns with its facing too).
        "steam_whistle" => {
            let floor =
                |pole: Px| vec![pole, [1., 0., 1., 15., 3., 15.], [5., 0., 5., 11., 8., 11.]];
            let px = |boxes: Vec<Px>| -> Value {
                json!(
                    boxes
                        .iter()
                        .map(|b| b.map(|v| v / 16.0))
                        .collect::<Vec<_>>()
                )
            };
            declared(
                "minecraft:heavy_core",
                Some(json!({ "property": "size", "values": {
                    "small": px(floor([4., 3., 4., 12., 16., 12.])),
                    "medium": px(floor([3., 3., 3., 13., 16., 13.])),
                    "large": px(floor([2., 3., 2., 14., 16., 14.])) } })),
                own(&[
                    ("facing", horizontal),
                    ("powered", bools),
                    ("size", sizes),
                    ("wall", bools),
                ]),
                true,
            )
        }
        // WhistleExtenderBlock: `shape` (single by default) and `size`; AllShapes
        // .WHISTLE_EXTENDER_MEDIUM_* by shape. No item: a whistle used on its whistle adds it.
        "steam_whistle_extension" => declared(
            "minecraft:heavy_core",
            Some(json!({ "property": "shape", "values": {
                "single": [[3. / 16., 0., 3. / 16., 13. / 16., 10. / 16., 13. / 16.]],
                "double": [[3. / 16., 0., 3. / 16., 13. / 16., 18. / 16., 13. / 16.]],
                "double_connected": [[3. / 16., 0., 3. / 16., 13. / 16., 1., 13. / 16.]] } })),
            own(&[
                ("shape", &["single", "double", "double_connected"]),
                ("size", sizes),
            ]),
            false,
        ),
        _ => return None,
    })
}

/// AllShapes.BASIN_COLLISION_SHAPE: shape(0, 2, 0, 16, 13, 16) less (2, 5, 2, 14, 16, 14), and
/// the foot (2, 0, 2, 14, 2, 14).
const BASIN: [[f64; 6]; 6] = [
    [0., 2., 0., 16., 5., 16.],
    [0., 5., 0., 2., 13., 16.],
    [14., 5., 0., 16., 13., 16.],
    [2., 5., 0., 14., 13., 2.],
    [2., 5., 14., 14., 13., 16.],
    [2., 0., 2., 14., 2., 14.],
];

/// `BlazeBurnerBlock.HEAT_LEVEL` (`blaze`), in order.
pub const HEATS: &[&str] = &["none", "smouldering", "fading", "kindled", "seething"];

/// The blaze burner's blockstate: Create draws the cage as its model and the blaze in its
/// renderer (`BlazeBurnerRenderer`: the head chosen by heat, turned to `horizontalAngle(facing) +
/// 180`); here the head at rest is part of the block's model, by heat and facing.
/// AllFluids' honey and chocolate as Horizon fluid declarations: `levelDecreasePerBlock(2)`,
/// `tickRate(25)`, `slopeFindDistance(3)`, `registerFluidInteractions` with lava (obsidian from a
/// source, limestone or scoria from flowing lava), Create's still and flowing textures, untinted
/// (`SolidRenderedPlaceableFluidType.getTintColor`).
pub fn fluid_declarations() -> [(&'static str, Value); 2] {
    let fluid = |name: &str, flowing_stone: &str| {
        json!({
            "block": format!("create:{name}"),
            "bucket": format!("create:{name}_bucket"),
            "tick_delay": 25,
            "drop_off": 2,
            "slope_find_distance": 3,
            "lava_source": "minecraft:obsidian",
            "lava_flowing": format!("create:{flowing_stone}"),
            "still_texture": format!("create:block/fluid/{name}_still"),
            "flowing_texture": format!("create:block/fluid/{name}_flow"),
            "tint": 0xFF_FFFF
        })
    };
    [
        ("honey", fluid("honey", "limestone")),
        ("chocolate", fluid("chocolate", "scoria")),
    ]
}

pub fn blaze_burner_states() -> Value {
    let mut parts = vec![json!({ "apply": { "model": "create:block/blaze_burner/block" } })];
    for (heat, model) in [
        ("smouldering", "inert"),
        ("fading", "idle"),
        ("kindled", "idle"),
        ("seething", "super"),
    ] {
        // Blockstate y turns clockwise from above; the head turns the other way.
        for (facing, y) in [("north", 0), ("east", 90), ("south", 180), ("west", 270)] {
            let mut apply = Map::new();
            apply.insert(
                "model".into(),
                json!(format!("create:block/blaze_burner/blaze/{model}")),
            );
            if y != 0 {
                apply.insert("y".into(), json!(y));
            }
            parts.push(json!({ "when": { "blaze": heat, "facing": facing }, "apply": apply }));
        }
    }
    json!({ "multipart": parts })
}

/// Create's partial models (`AllPartialModels`) that turn as parts, as hidden blocks of the
/// content mod so Horizon bakes them like any block model: `(block, model, facing variants)`.
/// Nobody can hold or place them (no item, invisible).
/// The third value is the face the model points to as written (`shaft_half` points south, the
/// crank handle north).
pub const PARTIALS: [(&str, &str, &str); 62] = [
    ("part_shaft_half", "create:block/shaft_half", "south"),
    (
        "part_hand_crank_handle",
        "create:block/hand_crank/handle",
        "north",
    ),
    ("part_water_wheel", "create:block/water_wheel/wheel", "up"),
    ("part_millstone_cog", "create:block/millstone/inner", "up"),
    // CachedBuffers.partialFacing turns it by AngleHelper.horizontalAngle: 0° for south.
    (
        "part_press_head",
        "create:block/mechanical_press/head",
        "south",
    ),
    // BeltRenderer's segments and pulley, posed by the glue (`native_part_model_posed`): only
    // their unturned `up` state is drawn.
    ("part_belt_start", "create:block/belt/start", "up"),
    ("part_belt_middle", "create:block/belt/middle", "up"),
    ("part_belt_end", "create:block/belt/end", "up"),
    (
        "part_belt_start_bottom",
        "create:block/belt/start_bottom",
        "up",
    ),
    (
        "part_belt_middle_bottom",
        "create:block/belt/middle_bottom",
        "up",
    ),
    ("part_belt_end_bottom", "create:block/belt/end_bottom", "up"),
    (
        "part_belt_diagonal_start",
        "create:block/belt/diagonal_start",
        "up",
    ),
    (
        "part_belt_diagonal_middle",
        "create:block/belt/diagonal_middle",
        "up",
    ),
    (
        "part_belt_diagonal_end",
        "create:block/belt/diagonal_end",
        "up",
    ),
    ("part_belt_pulley", "create:block/belt_pulley", "up"),
    // A fluid's still texture on a unit cube (our own models, `fluid_models`), posed and
    // tinted to a tank's level (`FluidTankRenderer`'s fluid box).
    (
        "part_bearing_top_wooden",
        "create:block/bearing/top_wooden",
        "up",
    ),
    ("part_bearing_top", "create:block/bearing/top", "up"),
    // CachedBuffers.partialFacing, as the shaft half.
    (
        "part_fan_propeller",
        "create:block/encased_fan/propeller",
        "south",
    ),
    // MechanicalMixerRenderer: a shaftless cog, the pole and the head, posed by the glue.
    (
        "part_cogwheel_shaftless",
        "create:block/cogwheel_shaftless",
        "up",
    ),
    (
        "part_mixer_pole",
        "create:block/mechanical_mixer/pole",
        "up",
    ),
    (
        "part_mixer_head",
        "create:block/mechanical_mixer/head",
        "up",
    ),
    // PumpRenderer: the cog, turned to the pump's facing (`partialFacing`, as the shaft half).
    ("part_pump_cog", "create:block/mechanical_pump/cog", "south"),
    // DrillRenderer: the head, `partialFacing` as the shaft half.
    (
        "part_drill_head",
        "create:block/mechanical_drill/head",
        "south",
    ),
    // DeployerRenderer: the pole and the hand (pointing empty, holding an item), turned by
    // `horizontalAngle` and 270/90° up and down, as the shaft half.
    ("part_deployer_pole", "create:block/deployer/pole", "south"),
    (
        "part_deployer_hand_pointing",
        "create:block/deployer/hand_pointing",
        "south",
    ),
    (
        "part_deployer_hand_holding",
        "create:block/deployer/hand_holding",
        "south",
    ),
    // GaugeRenderer: a head and a dial per shown face, `rotateBufferTowards` (0° for east);
    // the dial's needle turned about its pivot by the glue.
    (
        "part_gauge_head_speed",
        "create:block/gauge/speedometer/head",
        "east",
    ),
    (
        "part_gauge_head_stress",
        "create:block/gauge/stressometer/head",
        "east",
    ),
    ("part_gauge_dial", "create:block/gauge/dial", "east"),
    ("part_fluid_water", "create:block/part_fluid_water", "up"),
    ("part_fluid_lava", "create:block/part_fluid_lava", "up"),
    ("part_fluid_honey", "create:block/part_fluid_honey", "up"),
    (
        "part_fluid_chocolate",
        "create:block/part_fluid_chocolate",
        "up",
    ),
    ("part_fluid_milk", "create:block/part_fluid_milk", "up"),
    // A curved track's pieces (`TrackMaterial.TrackModelHolder`), placed by their own transforms
    // (`BezierConnection.SegmentAngles`): drawn as they are, from their south-facing state.
    ("part_track_tie", "create:block/track/tie", "south"),
    // StandardBogeyVisual's frames and wheels (OBJ models centred on the bogey's anchor, along
    // z), and SignalRenderer's indicator lights: posed by the glue from their `up` state.
    (
        "part_bogey_frame",
        "create:block/track/bogey/bogey_frame",
        "up",
    ),
    (
        "part_bogey_wheel",
        "create:block/track/bogey/bogey_wheel",
        "up",
    ),
    (
        "part_bogey_drive",
        "create:block/track/bogey/bogey_drive",
        "up",
    ),
    (
        "part_bogey_drive_wheel",
        "create:block/track/bogey/bogey_drive_wheel",
        "up",
    ),
    (
        "part_signal_indicator_on",
        "create:block/track_signal/indicator_on",
        "up",
    ),
    (
        "part_signal_indicator_off",
        "create:block/track_signal/indicator_off",
        "up",
    ),
    (
        "part_track_segment_left",
        "create:block/track/segment_left",
        "south",
    ),
    (
        "part_track_segment_right",
        "create:block/track/segment_right",
        "south",
    ),
    // ArmRenderer's pieces (centred on the block's middle as Create draws them), posed by the
    // glue from their `up` state; the cog turns as a kinetic part.
    ("part_arm_cog", "create:block/mechanical_arm/cog", "up"),
    ("part_arm_base", "create:block/mechanical_arm/base", "up"),
    (
        "part_arm_lower_body",
        "create:block/mechanical_arm/lower_body",
        "up",
    ),
    (
        "part_arm_upper_body",
        "create:block/mechanical_arm/upper_body",
        "up",
    ),
    (
        "part_arm_claw_base",
        "create:block/mechanical_arm/claw_base",
        "up",
    ),
    (
        "part_arm_upper_claw_grip",
        "create:block/mechanical_arm/upper_claw_grip",
        "up",
    ),
    (
        "part_arm_lower_claw_grip",
        "create:block/mechanical_arm/lower_claw_grip",
        "up",
    ),
    // AnalogLeverRenderer's handle and CuckooClockRenderer's hands, posed by the glue from
    // their `up` state (the model as written).
    (
        "part_analog_lever_handle",
        "create:block/analog_lever/handle",
        "up",
    ),
    (
        "part_cuckoo_hour_hand",
        "create:block/cuckoo_clock/hour_hand",
        "up",
    ),
    (
        "part_cuckoo_minute_hand",
        "create:block/cuckoo_clock/minute_hand",
        "up",
    ),
    // EjectorRenderer's plate (`EJECTOR_TOP`), posed by the glue about its hinge.
    (
        "part_ejector_top",
        "create:block/weighted_ejector/top",
        "up",
    ),
    // SchematicannonRenderer's connector and pipe, turned by the glue to the barrel's aim.
    (
        "part_schematicannon_connector",
        "create:block/schematicannon/connector",
        "up",
    ),
    (
        "part_schematicannon_pipe",
        "create:block/schematicannon/pipe",
        "up",
    ),
    // FluidValveRenderer's pointer, posed by the glue from its `up` state and turned about the
    // valve's facing.
    (
        "part_fluid_valve_pointer",
        "create:block/fluid_valve/pointer",
        "up",
    ),
    // HosePulleyRenderer's coil (`partialFacing`, as the shaft half), and the hose's pieces
    // drawn down to its end from their `up` state (`AbstractPulleyRenderer.renderAt`).
    (
        "part_hose_coil",
        "create:block/hose_pulley/hose_coil",
        "south",
    ),
    (
        "part_hose_magnet",
        "create:block/hose_pulley/pulley_magnet",
        "up",
    ),
    ("part_hose_rope", "create:block/hose_pulley/rope", "up"),
    ("part_hose_half", "create:block/hose_pulley/rope_half", "up"),
    (
        "part_hose_half_magnet",
        "create:block/hose_pulley/rope_half_magnet",
        "up",
    ),
];

/// The unit-cube models of `part_fluid_*`: every face the fluid's still texture, water's
/// tinted (tint index 0).
pub fn fluid_models() -> [(&'static str, Value); 5] {
    let cube = |texture: &str, tint: bool| {
        let face = |_: &str| {
            let mut f = json!({ "uv": [0, 0, 16, 16], "texture": "#fluid" });
            if tint {
                f["tintindex"] = json!(0);
            }
            f
        };
        let faces: Map<String, Value> = ["north", "south", "east", "west", "up", "down"]
            .iter()
            .map(|d| (d.to_string(), face(d)))
            .collect();
        json!({
            "textures": { "fluid": texture, "particle": texture },
            "elements": [{ "from": [0, 0, 0], "to": [16, 16, 16], "faces": faces }]
        })
    };
    [
        (
            "part_fluid_water",
            cube("minecraft:block/water_still", true),
        ),
        ("part_fluid_lava", cube("minecraft:block/lava_still", false)),
        // Create's own still textures (`AllFluids`), not tinted; `prepare` puts them under
        // `textures/block/fluid/`, on the block atlas (NeoForge's fluid types stitch them there).
        (
            "part_fluid_honey",
            cube("create:block/fluid/honey_still", false),
        ),
        (
            "part_fluid_chocolate",
            cube("create:block/fluid/chocolate_still", false),
        ),
        (
            "part_fluid_milk",
            cube("create:block/fluid/milk_still", false),
        ),
    ]
}

/// The blockstate rotation (x, y) that turns a model pointing `from` to point `to`: y turns
/// clockwise seen from above (north, east, south, west), x turns about the x axis.
fn rotation(from: &str, to: &str) -> (i32, i32) {
    let h = ["north", "east", "south", "west"];
    let at = |d: &str| h.iter().position(|x| *x == d);
    // A model pointing up: x 90 turns it north, then y turns it around; x 180 turns it down.
    if from == "up" {
        return match to {
            "up" => (0, 0),
            "down" => (180, 0),
            t => (90, at(t).unwrap_or(0) as i32 * 90),
        };
    }
    match (at(from), to) {
        (Some(f), "up") => (
            if f == 0 { 270 } else { 90 },
            if f % 2 == 0 { 0 } else { 90 * f as i32 + 180 } % 360,
        ),
        (Some(f), "down") => (
            if f == 0 { 90 } else { 270 },
            if f % 2 == 0 { 0 } else { 90 * f as i32 } % 360,
        ),
        (Some(f), t) => (
            0,
            ((at(t).unwrap_or(0) as i32 - f as i32).rem_euclid(4)) * 90,
        ),
        _ => (0, 0),
    }
}

/// A partial's declaration and blockstate: one state per facing, the model turned to it as a
/// blockstate variant turns a model.
pub fn partial_files(model: &str, points: &str) -> (Value, Value) {
    let decl = json!({
        "template": "minecraft:stone",
        "item": false,
        "properties": { "render_shape": "invisible" },
        "state_properties": { "facing": ["north", "east", "south", "west", "up", "down"] }
    });
    let turn = |x: i32, y: i32| {
        let mut v = Map::new();
        v.insert("model".into(), json!(model));
        if x != 0 {
            v.insert("x".into(), json!(x));
        }
        if y != 0 {
            v.insert("y".into(), json!(y));
        }
        Value::Object(v)
    };
    let mut variants = Map::new();
    for to in ["north", "east", "south", "west", "up", "down"] {
        let (x, y) = rotation(points, to);
        variants.insert(format!("facing={to}"), turn(x, y));
    }
    (decl, json!({ "variants": variants }))
}

/// Blocks whose behaviour belongs to a later phase: kinetic, processing, logistics, fluids...
/// They are declared when that phase ports them.
fn later_phase(b: &CreateBlock) -> Option<String> {
    let class = b.java.class.as_deref().unwrap_or("");
    let decorative = [
        "",
        "Block",
        "CasingBlock",
        "WindowBlock",
        "ConnectedGlassBlock",
        "ConnectedGlassPaneBlock",
        "GlassPaneBlock",
        "ConnectedPillarBlock",
        "LayeredBlock",
        "MetalLadderBlock",
        "MetalScaffoldingBlock",
        "IronBarsBlock",
        "ExperienceBlock",
        "WeatheringCopperFullBlock",
        "CopycatBlock",
        "TransparentBlock",
        "RotatedPillarBlock",
    ];
    // Copycats need custom models (gap 2).
    if b.name.starts_with("copycat") {
        return Some("copycats need custom models (gap 2)".into());
    }
    (!decorative.contains(&class)).then(|| format!("{class}: ported with its subsystem"))
}

/// Create's blocks of this port whose class answers `hasAnalogOutputSignal` with true (Create
/// 6.0.11: `ItemVaultBlock`, `ToolboxBlock`, `DepotBlock`, `EjectorBlock`, `BasinBlock`,
/// `FrogportBlock`, `PostboxBlock`, `PackagerBlock`, `ItemDrainBlock`, `SpoutBlock`,
/// `FluidTankBlock`, `BacktankBlock`, `GaugeBlock`, `SequencedGearshiftBlock`,
/// `BlazeBurnerBlock`, `LitBlazeBurnerBlock`, `SignalBlock`, `StationBlock`); the glue answers
/// for each (`create_engine::comparator::has_analog_output`). Portable storage interfaces, the
/// controller rail, the redstone requester and the lectern controller have one too and are not
/// declared.
pub const ANALOG_OUTPUT: [&str; 19] = [
    "item_vault",
    "depot",
    "weighted_ejector",
    "basin",
    "package_frogport",
    "packager",
    "item_drain",
    "spout",
    "fluid_tank",
    "creative_fluid_tank",
    "copper_backtank",
    "netherite_backtank",
    "speedometer",
    "stressometer",
    "sequenced_gearshift",
    "blaze_burner",
    "lit_blaze_burner",
    "track_signal",
    "track_station",
];

fn analog_output(name: &str) -> bool {
    ANALOG_OUTPUT.contains(&name)
        || dyed(name, "toolbox").is_some()
        || dyed(name, "postbox").is_some()
}

/// The declaration file for a declared block.
pub fn declaration(b: &CreateBlock, d: &Decision) -> Option<Value> {
    let Decision::Declared {
        template,
        sounds,
        requires_tool,
        shapes,
        own,
        animated,
        item,
    } = d
    else {
        return None;
    };
    let mut props = Map::new();
    props.insert("hardness".into(), json!(b.hardness));
    props.insert("explosion_resistance".into(), json!(b.blast_resistance));
    if let Some(r) = requires_tool {
        props.insert("requires_tool".into(), json!(r));
    }
    if let Some(s) = sounds {
        props.insert("sounds".into(), json!(s));
    }
    if *animated {
        props.insert("render_shape".into(), json!("entityblock_animated"));
    }
    if matches!(
        b.name.as_str(),
        "crushing_wheel_controller" | "water_wheel_structure"
    ) {
        props.insert("render_shape".into(), json!("invisible"));
    }
    // AllBlocks.ROSE_QUARTZ_LAMP: `mapColor(TERRACOTTA_PINK)` (MapColor id 42); its light by
    // `powering` is per state, which a declaration can't give: none.
    if b.name == "rose_quartz_lamp" {
        props.insert("light".into(), json!(0));
        props.insert("map_color".into(), json!(42));
    }
    // Contacts on glowstone's template: no light of their own (an elevator contact's 10 while
    // powering is per state).
    if matches!(
        b.name.as_str(),
        "redstone_contact"
            | "elevator_contact"
            | "display_link"
            | "content_observer"
            | "stockpile_switch"
    ) {
        props.insert("light".into(), json!(0));
    }
    // AllBlocks' nixie tubes: `lightLevel($ -> 5)`.
    if b.name == "nixie_tube" || dyed(&b.name, "nixie_tube").is_some() {
        props.insert("light".into(), json!(5));
    }
    // Comparators read the block (`hasAnalogOutputSignal`), through the glue's answer.
    if analog_output(&b.name) {
        props.insert("analog_output".into(), json!(true));
    }
    let mut doc = Map::new();
    doc.insert("template".into(), json!(template));
    doc.insert("properties".into(), Value::Object(props));
    doc.insert("item".into(), json!(item));
    if let Some(components) = crate::equipment::backtank_item(&b.name) {
        // The backtank's block item is Create's BacktankItem too: worn on the chest.
        doc.insert("components".into(), components);
    } else if b.stack != 64 && *item {
        doc.insert(
            "components".into(),
            json!({ "minecraft:max_stack_size": b.stack }),
        );
    }
    if let Some(s) = shapes {
        doc.insert("shapes".into(), s.clone());
    }
    if let Some(o) = own {
        doc.insert("state_properties".into(), o.clone());
    }
    Some(Value::Object(doc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partials_turn_to_every_face() {
        // shaft_half points south: west is y 90, north y 180, up x 90.
        assert_eq!(rotation("south", "south"), (0, 0));
        assert_eq!(rotation("south", "west"), (0, 90));
        assert_eq!(rotation("south", "north"), (0, 180));
        assert_eq!(rotation("south", "up"), (90, 0));
        assert_eq!(rotation("south", "down"), (270, 0));
        // A model pointing north: east is y 90, up is x 270 (as the observer's model).
        assert_eq!(rotation("north", "east"), (0, 90));
        assert_eq!(rotation("north", "up"), (270, 0));
        assert_eq!(rotation("north", "down"), (90, 0));
        // A model pointing up (the water wheel's axle).
        assert_eq!(rotation("up", "north"), (90, 0));
        assert_eq!(rotation("up", "east"), (90, 90));
        assert_eq!(rotation("up", "down"), (180, 0));
    }

    /// A declaration's state count (every combination of its own properties) and its default
    /// (each property's first value).
    fn states_and_default(doc: &Value) -> (usize, Vec<(String, String)>) {
        let props = doc["state_properties"].as_object().expect("own properties");
        let count = props
            .values()
            .map(|v| v.as_array().unwrap().len())
            .product();
        let default = props
            .iter()
            .map(|(k, v)| (k.clone(), v[0].as_str().unwrap().to_owned()))
            .collect();
        (count, default)
    }

    fn declare(name: &str, info: (f64, f64, u32)) -> Value {
        let b = CreateBlock {
            name: name.into(),
            props: Props::new(),
            hardness: info.0,
            blast_resistance: info.1,
            stack: info.2,
            java: JavaFacts::default(),
        };
        let d = decide(&b, &BTreeMap::new());
        declaration(&b, &d).unwrap_or_else(|| panic!("{name}: {d:?}"))
    }

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn fluid_blocks_have_create_states_and_defaults() {
        type Expect<'a> = (&'a str, usize, &'a [(&'a str, &'a str)], bool);
        let expect: [Expect; 8] = [
            (
                "fluid_valve",
                48,
                &[
                    ("axis_along_first", "false"),
                    ("enabled", "false"),
                    ("facing", "down"),
                    ("waterlogged", "false"),
                ],
                true,
            ),
            (
                "smart_fluid_pipe",
                24,
                &[
                    ("face", "floor"),
                    ("facing", "north"),
                    ("waterlogged", "false"),
                ],
                true,
            ),
            (
                "glass_fluid_pipe",
                12,
                &[("alt", "false"), ("axis", "y"), ("waterlogged", "false")],
                false,
            ),
            (
                "encased_fluid_pipe",
                64,
                &[
                    ("down", "false"),
                    ("east", "false"),
                    ("north", "false"),
                    ("south", "false"),
                    ("up", "false"),
                    ("west", "false"),
                ],
                false,
            ),
            ("hose_pulley", 4, &[("facing", "north")], true),
            (
                "creative_fluid_tank",
                24,
                &[("bottom", "true"), ("shape", "window"), ("top", "true")],
                true,
            ),
            (
                "steam_whistle",
                48,
                &[
                    ("facing", "north"),
                    ("powered", "false"),
                    ("size", "medium"),
                    ("wall", "false"),
                ],
                true,
            ),
            (
                "steam_whistle_extension",
                9,
                &[("shape", "single"), ("size", "medium")],
                false,
            ),
        ];
        for (name, count, default, item) in expect {
            let doc = declare(name, (3.0, 6.0, 64));
            assert_eq!(
                doc["properties"]["sounds"], "minecraft:copper_block",
                "{name}"
            );
            assert_eq!(states_and_default(&doc), (count, pairs(default)), "{name}");
            assert_eq!(doc["item"], item, "{name}");
        }
    }

    #[test]
    fn gap_two_blocks_have_their_own_states_and_create_defaults() {
        // ValveHandleBlock: facing (any() is down) and waterlogged; copper's properties.
        for name in ["copper_valve_handle", "black_valve_handle"] {
            let doc = declare(name, (3.0, 6.0, 64));
            assert_eq!(doc["template"], "minecraft:heavy_core");
            assert_eq!(doc["properties"]["sounds"], "minecraft:copper_block");
            assert_eq!(doc["properties"]["requires_tool"], true);
            assert_eq!(doc["item"], true);
            assert_eq!(doc["shapes"]["property"], "facing");
            assert_eq!(
                states_and_default(&doc),
                (12, pairs(&[("facing", "down"), ("waterlogged", "false")]))
            );
        }
        // SlidingDoorBlock: DoorBlock's five properties and `visible`, 128 states.
        let doc = declare("train_door", (3.0, 6.0, 64));
        assert_eq!(doc["template"], "minecraft:iron_door");
        assert_eq!(doc["properties"]["sounds"], "minecraft:netherite_block");
        assert_eq!(doc["properties"]["hardness"], 3.0);
        assert_eq!(doc["properties"]["explosion_resistance"], 6.0);
        assert_eq!(
            states_and_default(&doc),
            (
                128,
                pairs(&[
                    ("facing", "north"),
                    ("half", "lower"),
                    ("hinge", "left"),
                    ("open", "false"),
                    ("powered", "false"),
                    ("visible", "true"),
                ])
            )
        );
        assert_eq!(
            declare("framed_glass_door", (3.0, 6.0, 64))["properties"]["sounds"],
            "minecraft:glass"
        );
        // ToolboxBlock: a barrel's container, horizontal facing and waterlogged.
        let doc = declare("red_toolbox", (2.0, 2.0, 64));
        assert_eq!(doc["template"], "minecraft:barrel");
        assert_eq!(doc["properties"]["sounds"], "minecraft:oak_planks");
        assert_eq!(doc["properties"]["requires_tool"], false);
        assert_eq!(
            states_and_default(&doc),
            (8, pairs(&[("facing", "north"), ("waterlogged", "false")]))
        );
        // PostboxBlock: facing, open, waterlogged.
        let doc = declare("lime_postbox", (2.0, 2.0, 64));
        assert_eq!(
            states_and_default(&doc),
            (
                16,
                pairs(&[
                    ("facing", "north"),
                    ("open", "false"),
                    ("waterlogged", "false")
                ])
            )
        );
        // NixieTubeBlock: floor by default, light 5; only the orange one has an item.
        let doc = declare("nixie_tube", (3.0, 6.0, 64));
        assert_eq!(doc["properties"]["light"], 5);
        assert_eq!(doc["item"], true);
        assert_eq!(
            states_and_default(&doc),
            (
                32,
                pairs(&[
                    ("double_face", "floor"),
                    ("facing", "north"),
                    ("waterlogged", "false")
                ])
            )
        );
        let doc = declare("blue_nixie_tube", (3.0, 6.0, 1));
        assert_eq!(doc["item"], false);
        assert!(doc.get("components").is_none());
        // Copycats stay pending.
        let b = CreateBlock {
            name: "copycat_step".into(),
            props: Props::new(),
            hardness: 1.0,
            blast_resistance: 1.0,
            stack: 64,
            java: JavaFacts {
                class: Some("CopycatStepBlock".into()),
                ..JavaFacts::default()
            },
        };
        assert_eq!(
            decide(&b, &BTreeMap::new()),
            Decision::Pending("copycats need custom models (gap 2)".into())
        );
    }

    /// With Create's generated data beside the port: every valve handle, sliding door, toolbox,
    /// postbox and nixie tube is declared, with their state counts.
    #[test]
    fn create_data_declares_the_gap_two_families() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let generated = root.join("src/generated/resources");
        if !generated.is_dir() {
            return;
        }
        let java = crate::java::read_all(&root.join("src/main/java"));
        let all = read_blocks(&generated, &java);
        let mut states = BTreeMap::<&str, (usize, usize)>::new();
        for b in &all {
            let family = ["valve_handle", "_door", "toolbox", "postbox", "nixie_tube"]
                .into_iter()
                .find(|f| b.name.ends_with(f) && !b.name.contains("trapdoor"));
            let Some(family) = family else { continue };
            let d = decide(b, &BTreeMap::new());
            let doc = declaration(b, &d).unwrap_or_else(|| panic!("{}: {d:?}", b.name));
            let e = states.entry(family).or_default();
            e.0 += 1;
            e.1 += states_and_default(&doc).0;
        }
        assert_eq!(states["valve_handle"], (17, 204));
        assert_eq!(states["_door"], (5, 640));
        assert_eq!(states["toolbox"], (16, 128));
        assert_eq!(states["postbox"], (16, 256));
        assert_eq!(states["nixie_tube"], (16, 512));
    }

    /// The decorative and kinetic blocks ported with their glue: Create's states and defaults.
    #[test]
    fn decorative_and_kinetic_blocks_take_create_s_states() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let generated = root.join("src/generated/resources");
        if !generated.is_dir() {
            return;
        }
        let java = crate::java::read_all(&root.join("src/main/java"));
        let all = read_blocks(&generated, &java);
        let doc = |n: &str| {
            let b = all.iter().find(|b| b.name == n).unwrap();
            let d = decide(b, &BTreeMap::new());
            declaration(b, &d).unwrap_or_else(|| panic!("{n}: {d:?}"))
        };
        let expect = [
            // GirderBlock: X, Z, TOP, BOTTOM, AXIS (y), WATERLOGGED.
            ("metal_girder", 96, vec![("axis", "y"), ("x", "false")]),
            ("metal_girder_encased_shaft", 16, vec![("axis", "x")]),
            ("metal_bracket", 36, vec![("type", "pipe")]),
            ("radial_chassis", 48, vec![("axis", "y")]),
            (
                "large_water_wheel",
                6,
                vec![("axis", "y"), ("extension", "false")],
            ),
            ("water_wheel_structure", 6, vec![("facing", "down")]),
            ("flywheel", 3, vec![("axis", "y")]),
            ("placard", 48, vec![("face", "floor"), ("facing", "north")]),
            (
                "desk_bell",
                24,
                vec![("facing", "up"), ("powered", "false")],
            ),
            ("peculiar_bell", 32, vec![("attachment", "floor")]),
            ("clipboard", 48, vec![("written", "false")]),
            ("item_hatch", 16, vec![("open", "false")]),
            ("white_table_cloth", 2, vec![("entity", "false")]),
            ("cardboard_block", 2, vec![("axis", "x")]),
            ("creative_crate", 6, vec![("facing", "down")]),
        ];
        for (n, count, defaults) in expect {
            let d = doc(n);
            let (c, def) = states_and_default(&d);
            assert_eq!(c, count, "{n}");
            for (k, v) in defaults {
                assert!(def.contains(&(k.to_owned(), v.to_owned())), "{n}: {k}");
            }
        }
        assert_eq!(
            doc("turntable")["properties"]["render_shape"],
            "entityblock_animated"
        );
        assert_eq!(doc("water_wheel_structure")["item"], false);
        assert_eq!(doc("metal_girder_encased_shaft")["item"], false);
        assert_eq!(doc("haunted_bell")["template"], "minecraft:bell");
    }

    /// With Create's Java beside the port: the classes answering `hasAnalogOutputSignal` are the
    /// ones `ANALOG_OUTPUT` names (and five not ported), and each of those blocks declared here
    /// says `analog_output`.
    #[test]
    fn comparators_read_the_blocks_create_gives_an_analog_output() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let java_root = root.join("src/main/java");
        let generated = root.join("src/generated/resources");
        if !java_root.is_dir() || !generated.is_dir() {
            return;
        }
        let mut files = Vec::new();
        crate::java::collect(&java_root, &mut files);
        let mut classes: Vec<String> = files
            .iter()
            .filter(|f| {
                std::fs::read_to_string(f)
                    .is_ok_and(|t| t.contains("boolean hasAnalogOutputSignal("))
            })
            .map(|f| f.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        classes.sort();
        assert_eq!(
            classes,
            [
                "BacktankBlock",
                "BasinBlock",
                "BlazeBurnerBlock",
                "ControllerRailBlock",
                "DepotBlock",
                "EjectorBlock",
                "FluidTankBlock",
                "FrogportBlock",
                "GaugeBlock",
                "ItemDrainBlock",
                "ItemVaultBlock",
                "LitBlazeBurnerBlock",
                "PackagerBlock",
                "PortableStorageInterfaceBlock",
                "PostboxBlock",
                "RedstoneRequesterBlock",
                "SequencedGearshiftBlock",
                "SignalBlock",
                "SpoutBlock",
                "StationBlock",
                "ToolboxBlock",
            ]
        );
        let java = crate::java::read_all(&java_root);
        let all = read_blocks(&generated, &java);
        let mut declared = 0;
        for b in &all {
            let Some(doc) = declaration(b, &decide(b, &BTreeMap::new())) else {
                continue;
            };
            let says = doc["properties"]["analog_output"] == true;
            assert_eq!(says, analog_output(&b.name), "{}", b.name);
            declared += usize::from(says);
        }
        // 19 blocks, 16 toolboxes and 16 postboxes.
        assert_eq!(declared, 19 + 16 + 16);
    }

    #[test]
    fn reads_variant_and_multipart_properties() {
        let v = json!({"variants": {"axis=x": {}, "axis=y": {}}});
        assert_eq!(
            blockstate_props(&v)["axis"].iter().collect::<Vec<_>>(),
            ["x", "y"]
        );
        let m = json!({"multipart": [{"when": {"OR": [{"north": "true"}, {"up": "low|tall"}]}}]});
        let p = blockstate_props(&m);
        assert_eq!(p["up"].len(), 2);
        assert!(p.contains_key("north"));
    }
}
