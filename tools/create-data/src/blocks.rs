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
        "shaft" | "powered_shaft" => (
            "minecraft:iron_chain",
            Some(by_axis(&[SIX_VOXEL_POLE])),
            None,
        ),
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
        "hand_crank" => (
            "minecraft:heavy_core",
            Some(by_facing(&CRANK)),
            own(&[("facing", facing), ("waterlogged", bools)]),
        ),
        _ => return None,
    };
    // KineticBlock.getRenderShape: shafts and cogs are all moving parts; casings, gearboxes and
    // motors keep their model and show their shafts as parts.
    let animated = matches!(
        b.name.as_str(),
        "shaft" | "powered_shaft" | "cogwheel" | "large_cogwheel" | "hand_crank" | "belt"
    );
    Some(Decision::Declared {
        template,
        sounds,
        requires_tool: requires_tool(b),
        shapes,
        own,
        animated,
        item: b.name != "belt",
    })
}

/// Create's partial models (`AllPartialModels`) that turn as parts, as hidden blocks of the
/// content mod so Horizon bakes them like any block model: `(block, model, facing variants)`.
/// Nobody can hold or place them (no item, invisible).
/// The third value is the face the model points to as written (`shaft_half` points south, the
/// crank handle north).
pub const PARTIALS: [(&str, &str, &str); 15] = [
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
];

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
    // Fluids are gap 5; copycats need custom models (gap 2).
    if matches!(b.name.as_str(), "honey" | "chocolate") {
        return Some("fluid block (gap 5)".into());
    }
    if b.name.starts_with("copycat") {
        return Some("copycat model (gap 2)".into());
    }
    (!decorative.contains(&class)).then(|| format!("{class}: ported with its subsystem"))
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
    let mut doc = Map::new();
    doc.insert("template".into(), json!(template));
    doc.insert("properties".into(), Value::Object(props));
    doc.insert("item".into(), json!(item));
    if b.stack != 64 {
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
