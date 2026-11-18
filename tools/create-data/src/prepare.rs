//! The one-time prepare step. A Create jar holds MIT data (`data/`, generated `assets/`) and All
//! Rights Reserved assets (textures, hand-written models, sounds); neither ships with the port. This
//! reads them from Create's jar on the player's machine (`jar.rs` downloads the pinned one from
//! Modrinth; a jar or folders laid out like one may be given) and writes the native mod the
//! launcher builds: the mod's crate (`create/`: `sunrise-mod.toml`, `Cargo.toml`, `src/`) and
//! declarations (`data/`, `assets/`), Create's data converted from 1.21.1 to 26.3, and what 26.3
//! needs made from its assets (item model definitions, flattened and tinted models, connected
//! texture tiles). `sunrise-mod.toml` names the pinned jar (`[[jar]]`): the launcher fetches it
//! and layers its assets and data under the mod, so a file the jar already holds as it is is not
//! written again. Reading a zip runs no Java. The folder holds what is made from Create's assets:
//! it is for the player's own game, never for redistribution.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Every file of the sources, by path inside the jar; a later source replaces an earlier one.
fn read_sources(sources: &[PathBuf]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut files = BTreeMap::new();
    for source in sources {
        if source.is_dir() {
            walk(source, source, &mut files)?;
        } else {
            let f = fs::File::open(source).map_err(|e| format!("{}: {e}", source.display()))?;
            let mut zip =
                zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", source.display()))?;
            for i in 0..zip.len() {
                let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
                if entry.is_dir() {
                    continue;
                }
                let name = entry.name().to_string();
                if name.contains("..") {
                    return Err(format!("unsafe path in jar: {name}"));
                }
                let mut bytes = Vec::with_capacity(entry.size() as usize);
                entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                files.insert(name, bytes);
            }
        }
    }
    Ok(files)
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    for e in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let p = e.map_err(|e| e.to_string())?.path();
        if p.is_dir() {
            walk(root, &p, out)?;
        } else {
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(rel, fs::read(&p).map_err(|e| e.to_string())?);
        }
    }
    Ok(())
}

/// 1.21.1 ingredients (`{"item": x}`, `{"tag": t}`, lists of them) as 26.3 writes them (`"x"`,
/// `"#t"`, lists of strings). Anything else is left as it is.
pub fn ingredient(v: &Value) -> Value {
    match v {
        // A `neoforge:block_tag` ingredient (Create's data) keeps its shape; only the tag's 26.3
        // name changes.
        Value::Object(o) if o.get("type").and_then(Value::as_str) == Some("neoforge:block_tag") => {
            let mut o = o.clone();
            if let Some(t) = o.get("tag").and_then(Value::as_str) {
                let t = renamed_tag(t).to_string();
                o.insert("tag".into(), Value::String(t));
            }
            Value::Object(o)
        }
        Value::Object(o) if o.len() == 1 && o.get("item").is_some_and(Value::is_string) => {
            Value::String(renamed_item(o["item"].as_str().unwrap()).to_string())
        }
        Value::String(s) if !s.starts_with('#') => Value::String(renamed_item(s).to_string()),
        Value::Object(o) if o.len() == 1 && o.get("tag").is_some_and(Value::is_string) => {
            Value::String(format!("#{}", o["tag"].as_str().unwrap()))
        }
        Value::Array(a) => {
            let items: Vec<Value> = a.iter().map(ingredient).collect();
            if items.len() == 1 && items[0].as_str().is_some_and(|s| s.starts_with('#')) {
                items[0].clone()
            } else {
                Value::Array(items)
            }
        }
        other => other.clone(),
    }
}

/// A vanilla tag Create's 1.21.1 data names by a name 26.3 changed.
fn renamed_tag(tag: &str) -> &str {
    match tag {
        "minecraft:convertable_to_mud" => "minecraft:convertible_to_mud",
        t => t,
    }
}

/// A vanilla item Create's 1.21.1 data names by a name 26.3 changed (1.21.9 renamed the chain).
fn renamed_item(item: &str) -> &str {
    match item {
        "minecraft:chain" => "minecraft:iron_chain",
        i => i,
    }
}

/// A recipe in 26.3's syntax, or `None` when its `neoforge:conditions` name another mod.
pub fn convert_recipe(mut doc: Value) -> Option<Value> {
    let o = doc.as_object_mut()?;
    if let Some(Value::Array(conds)) = o.remove("neoforge:conditions") {
        for c in conds {
            let ok = match c["type"].as_str() {
                Some("neoforge:mod_loaded") => {
                    matches!(c["modid"].as_str(), Some("create" | "minecraft"))
                }
                Some("neoforge:not") => match c["value"]["type"].as_str() {
                    Some("neoforge:mod_loaded") => {
                        !matches!(c["value"]["modid"].as_str(), Some("create" | "minecraft"))
                    }
                    _ => false,
                },
                _ => false,
            };
            if !ok {
                return None;
            }
        }
    }
    for key in ["ingredient", "base", "template", "addition"] {
        if let Some(v) = o.get(key) {
            let c = ingredient(v);
            o.insert(key.into(), c);
        }
    }
    if let Some(Value::Object(k)) = o.get_mut("key") {
        for v in k.values_mut() {
            *v = ingredient(v);
        }
    }
    if let Some(Value::Array(list)) = o.get_mut("ingredients") {
        for v in list.iter_mut() {
            *v = ingredient(v);
        }
    }
    // A result names its item by `id` (crafting, cooking) or, in Create's 1.21.1 processing
    // recipes, also by `item`.
    let rename = |r: &mut Value| {
        for key in ["id", "item"] {
            if let Some(Value::String(i)) = r.get_mut(key) {
                *i = renamed_item(i).to_string();
            }
        }
    };
    if let Some(r) = o.get_mut("result") {
        rename(r);
    }
    if let Some(Value::Array(list)) = o.get_mut("results") {
        list.iter_mut().for_each(rename);
    }
    // A sequenced assembly's steps are recipes of their own, nested.
    if let Some(Value::Array(steps)) = o.get_mut("sequence") {
        for step in steps.iter_mut() {
            if let Some(c) = convert_recipe(step.take()) {
                *step = c;
            }
        }
    }
    Some(doc)
}

/// A 1.21.1 loot table as 26.3 writes it, the form Horizon reads: a condition names its kind in
/// `type` (1.21.1: `condition`), a function in `type` (1.21.1: `function`), and a pool's or
/// entry's functions are its `modifier` list (1.21.1: `functions`).
/// The partial models Create's item renderers turn: item, part, the numeric item model property
/// that picks its frame (the glue's `items::spin`), the axis and the pivot in model pixels
/// (`WrenchItemRenderer`: the gear about y, 1 px off the centre; `PotatoCannonItemRenderer`: the
/// cog about z, half a pixel up; `WorldshaperItemRenderer`: the accelerator about z, 2.5 px
/// down).
pub const SPINNERS: [(&str, &str, &str, char, [f64; 3]); 3] = [
    ("wrench", "gear", "create:wrench_gear", 'y', [9.0, 7.0, 8.0]),
    (
        "potato_cannon",
        "cog",
        "create:potato_cannon_cog",
        'z',
        [8.0, 8.5, 8.0],
    ),
    (
        "handheld_worldshaper",
        "accelerator",
        "create:worldshaper_accelerator",
        'z',
        [8.0, 5.5, 8.0],
    ),
];

/// The frames a turning partial model is cut into, a whole turn.
pub const SPIN_FRAMES: usize = 64;

/// Frame `frame` of a turning partial model: each element turned about the pivot by the frame's
/// angle on top of its own turn about the same axis (the parts' elements turn about that axis
/// through the pivot, or not at all).
fn spin_frame(doc: &Value, axis: char, pivot: [f64; 3], frame: usize) -> Value {
    let turn = frame as f64 * 360.0 / SPIN_FRAMES as f64;
    let mut out = doc.clone();
    let axis_name = axis.to_string();
    if let Some(elements) = out.get_mut("elements").and_then(Value::as_array_mut) {
        for e in elements {
            let (own, origin) = match e.get("rotation") {
                Some(r) if r.get("axis").and_then(Value::as_str) == Some(axis_name.as_str()) => (
                    r.get("angle").and_then(Value::as_f64).unwrap_or(0.0),
                    r.get("origin").cloned(),
                ),
                Some(_) => continue,
                None => (0.0, None),
            };
            let origin = if own == 0.0 {
                json!(pivot)
            } else {
                origin.unwrap_or(json!(pivot))
            };
            let angle = (own + turn + 180.0).rem_euclid(360.0) - 180.0;
            e["rotation"] = json!({ "angle": angle, "axis": axis_name, "origin": origin });
        }
    }
    out
}

/// The item models of Create's `CustomRenderedItemModelRenderer`s, as 26.3 item definitions: the
/// item's own model with the partial models its renderer draws over it, the turning ones
/// (`SPINNERS`) as a frame picked by their property (the wand's bits, which turn about another
/// axis than their own elements', stand still), and the cardboard sword's held model outside the
/// GUI (`CardboardSwordItemRenderer`). None for an item drawn by its plain model.
fn custom_rendered(name: &str) -> Option<Value> {
    let model =
        |m: &str| json!({ "type": "minecraft:model", "model": format!("create:item/{name}/{m}") });
    let parts: &[&str] = match name {
        // `SymmetryWandItemRenderer`: CORE, CORE_GLOW, BITS.
        "wand_of_symmetry" => &["item", "core", "core_glow", "bits"],
        // `WrenchItemRenderer`: GEAR.
        "wrench" => &["item", "gear"],
        // `PotatoCannonItemRenderer`: COG.
        "potato_cannon" => &["item", "cog"],
        // `WorldshaperItemRenderer`: CORE, CORE_GLOW, ACCELERATOR.
        "handheld_worldshaper" => &["item", "core", "core_glow", "accelerator"],
        // `ExtendoGripItemRenderer`: its arms, hand and cog posed by the mod's special model
        // renderer (`create:extendo_grip`, `glue/equipment.rs`).
        // The glue's `create:extendo_grip_pose` node picks one by where it is drawn and the
        // time (`item_model_update`): at rest away from a first-person hand, else as the swing
        // left it; the cog turned to a 1/10 of its 45° repeat; the hand pointing, punching or
        // holding.
        "extendo_grip" => {
            let special = |animation: f64, cog: f64, hand: &str| {
                json!({
                    "type": "minecraft:special",
                    "base": "create:item/extendo_grip",
                    "model": {
                        "type": "create:extendo_grip",
                        "animation": animation,
                        "cog": cog,
                        "hand": hand,
                    },
                })
            };
            let mut models = Map::new();
            for c in 0..GRIP_COG_STEPS {
                let cog = c as f64 * 45.0 / GRIP_COG_STEPS as f64;
                models.insert(format!("rest_{c}"), special(0.25, cog, "pointing"));
                for a in 0..GRIP_SWING_STEPS {
                    let animation = a as f64 / (GRIP_SWING_STEPS - 1) as f64;
                    for hand in ["punching", "holding"] {
                        models.insert(
                            format!("hand_{a}_{c}_{hand}"),
                            special(animation, cog, hand),
                        );
                    }
                }
            }
            return Some(json!({ "type": "create:extendo_grip_pose", "models": models }));
        }
        "cardboard_sword" => {
            return Some(json!({
                "type": "minecraft:select",
                "property": "minecraft:display_context",
                "cases": [{ "when": "gui", "model": model("item") }],
                "fallback": model("item_in_hand"),
            }));
        }
        _ => return None,
    };
    let part = |p: &&str| match SPINNERS.iter().find(|s| s.0 == name && s.1 == *p) {
        Some((_, _, property, ..)) => json!({
            "type": "minecraft:range_dispatch",
            "property": property,
            "entries": (0..SPIN_FRAMES)
                .map(|i| json!({ "threshold": i as f64, "model": model(&format!("{p}_spin/{i}")) }))
                .collect::<Vec<_>>(),
            "fallback": model(p),
        }),
        None => model(p),
    };
    Some(
        json!({ "type": "minecraft:composite", "models": parts.iter().map(part).collect::<Vec<_>>() }),
    )
}

/// A `neoforge:composite` model as Create's files write it (the nixie tube, display and stock
/// links): a model of named `children`, each with its own textures and elements. Its children's
/// elements, in the file's order, become the model's own, their textures joined under the
/// model's, which vanilla's baker reads; anything else a child holds is left out.
pub fn flatten_composite(doc: &mut Value) {
    let Some(obj) = doc.as_object_mut() else {
        return;
    };
    if obj.get("loader").and_then(Value::as_str) != Some("neoforge:composite") {
        return;
    }
    obj.remove("loader");
    let children = obj.remove("children");
    let mut elements = Vec::new();
    let mut textures = obj
        .get("textures")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(Value::Object(children)) = children {
        for child in children.values() {
            let Some(child) = child.as_object() else {
                continue;
            };
            if let Some(Value::Object(t)) = child.get("textures") {
                for (k, v) in t {
                    textures.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            if let Some(Value::Array(e)) = child.get("elements") {
                elements.extend(e.iter().cloned());
            }
        }
    }
    if !textures.is_empty() {
        obj.insert("textures".into(), Value::Object(textures));
    }
    obj.insert("elements".into(), Value::Array(elements));
}

/// `GearboxBlock.getDrops`: a gearbox on a horizontal axis drops one vertical gearbox, whatever
/// the loot table says (no explosion decay); an upright one keeps the table's drop.
pub fn gearbox_drops(doc: &mut Value) {
    let Some(pool) = doc
        .get_mut("pools")
        .and_then(Value::as_array_mut)
        .and_then(|p| p.first_mut())
    else {
        return;
    };
    if let Some(conditions) = pool.get_mut("conditions").and_then(Value::as_array_mut) {
        conditions.push(serde_json::json!({
            "type": "minecraft:block_state_property",
            "block": "create:gearbox",
            "properties": { "axis": "y" }
        }));
    }
    let horizontal = serde_json::json!({
        "bonus_rolls": 0.0,
        "conditions": [{
            "type": "minecraft:inverted",
            "term": {
                "type": "minecraft:block_state_property",
                "block": "create:gearbox",
                "properties": { "axis": "y" }
            }
        }],
        "entries": [{ "type": "minecraft:item", "name": "create:vertical_gearbox" }],
        "rolls": 1.0
    });
    if let Some(pools) = doc.get_mut("pools").and_then(Value::as_array_mut) {
        pools.push(horizontal);
    }
}

pub fn convert_loot(v: &mut Value) {
    match v {
        Value::Object(o) => {
            if let Some(f) = o.remove("functions") {
                o.insert("modifier".into(), f);
            }
            for key in ["condition", "function"] {
                if !o.contains_key("type")
                    && let Some(Value::String(kind)) = o.get(key).cloned()
                {
                    o.remove(key);
                    o.insert("type".into(), Value::String(kind));
                }
            }
            for child in o.values_mut() {
                convert_loot(child);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(convert_loot),
        _ => {}
    }
}

/// Data and asset paths the content mod keeps: Create's own namespace, plus additions to vanilla's
/// and the convention (`c`) tags. NeoForge's biome modifiers, Curios and other loaders' files stay
/// out (Horizon's content fence refuses them).
/// Create's configured features of its own feature types (`create:layered_ore`, the striated
/// stone layers): Horizon has no such feature, so they, the placed features using them and the
/// biome modifiers adding only those are left out. Their ids, as `create:<name>`.
fn unplaced_features(files: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    files
        .iter()
        .filter_map(|(path, bytes)| {
            let name = path
                .strip_prefix("data/create/worldgen/configured_feature/")?
                .strip_suffix(".json")?;
            let doc: Value = serde_json::from_slice(bytes).ok()?;
            let kind = doc["type"].as_str()?;
            (!kind.starts_with("minecraft:")).then(|| format!("create:{name}"))
        })
        .collect()
}

/// A worldgen or biome modifier file as Horizon reads it, or false to leave it out: configured
/// and placed features of `unplaced` and modifiers adding only those go; Create's
/// `create:config_filter` placement goes from the rest (`ConfigPlacementFilter`: it passes every
/// position unless `disableWorldGen`, off by default, and draws no random).
fn convert_worldgen(path: &str, doc: &mut Value, unplaced: &[String]) -> bool {
    let unplaced = |id: &str| unplaced.iter().any(|u| u == id);
    let name = path
        .rsplit('/')
        .next()
        .and_then(|n| n.strip_suffix(".json"))
        .map(|n| format!("create:{n}"))
        .unwrap_or_default();
    if path.contains("/configured_feature/") {
        if unplaced(&name) {
            return false;
        }
        // 26.3 keeps a feature's configuration beside its type, not under `config`, and writes
        // block states as `{"id", "properties"}` (a bare id without properties).
        if let Some(Value::Object(config)) = doc.as_object_mut().and_then(|o| o.remove("config"))
            && let Some(o) = doc.as_object_mut()
        {
            o.extend(config);
        }
        convert_states(doc);
        return true;
    }
    if path.contains("/placed_feature/") {
        if doc["feature"].as_str().is_some_and(unplaced) {
            return false;
        }
        if let Some(Value::Array(steps)) = doc.get_mut("placement") {
            steps.retain(|p| p["type"] != "create:config_filter");
        }
        return true;
    }
    // A biome modifier: its placed features are named as the configured ones are.
    let features: Vec<&str> = match &doc["features"] {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    !features.iter().all(|f| unplaced(f))
}

/// The sliding doors whose block entity draws them while they are not `visible`.
const SLIDING_DOORS: [&str; 5] = [
    "andesite_door",
    "brass_door",
    "copper_door",
    "train_door",
    "framed_glass_door",
];

/// A sliding door's blockstate (`SlidingDoorBlock.getRenderShape`: `ENTITYBLOCK_ANIMATED` while
/// not `visible`): Create's variants as multipart cases, without those of hidden states, so a
/// hidden door draws nothing of its own (its block entity draws it sliding).
fn sliding_door_states(path: &str, bytes: &[u8]) -> Option<Value> {
    let name = path
        .strip_prefix("assets/create/blockstates/")?
        .strip_suffix(".json")?;
    if !SLIDING_DOORS.contains(&name) {
        return None;
    }
    let doc: Value = serde_json::from_slice(bytes).ok()?;
    let cases: Vec<Value> = doc["variants"]
        .as_object()?
        .iter()
        .filter(|(key, _)| !key.split(',').any(|kv| kv == "visible=false"))
        .map(|(key, model)| {
            let when: serde_json::Map<String, Value> = key
                .split(',')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (k.to_owned(), json!(v)))
                .collect();
            json!({ "when": when, "apply": model })
        })
        .collect();
    Some(json!({ "multipart": cases }))
}

/// 1.21.1's block states (`{"Name", "Properties"}`) as 26.3's codec reads them, everywhere inside.
fn convert_states(v: &mut Value) {
    match v {
        Value::Object(o) => {
            if let Some(Value::String(name)) = o.get("Name").cloned() {
                *v = match o.get("Properties") {
                    Some(p @ Value::Object(_)) => json!({"id": name, "properties": p}),
                    _ => Value::String(name),
                };
                return;
            }
            o.values_mut().for_each(convert_states);
        }
        Value::Array(a) => a.iter_mut().for_each(convert_states),
        _ => {}
    }
}

/// The tinted copy's path of a model the glue draws coloured (`<model>_tinted.json`).
fn tinted_model(path: &str) -> Option<String> {
    let rest = path.strip_prefix("assets/create/models/block/")?;
    let coloured = rest.starts_with("factory_gauge/connections/")
        || rest == "mechanical_arm/lower_body.json"
        || rest == "mechanical_arm/upper_body.json"
        || rest == "track_overlay/assembling.json";
    let stem = rest.strip_suffix(".json").filter(|_| coloured)?;
    Some(format!("assets/create/models/block/{stem}_tinted.json"))
}

/// Every face of a model's elements takes tint index 0.
fn tint_faces(doc: &mut Value) {
    let Some(elements) = doc.get_mut("elements").and_then(Value::as_array_mut) else {
        return;
    };
    for e in elements {
        if let Some(faces) = e.get_mut("faces").and_then(Value::as_object_mut) {
            for f in faces.values_mut() {
                if let Some(f) = f.as_object_mut() {
                    f.insert("tintindex".into(), json!(0));
                }
            }
        }
    }
}

fn kept(path: &str) -> bool {
    let data = |rest: &str| {
        rest.starts_with("create/recipe/")
            || rest.starts_with("create/loot_table/")
            || rest.starts_with("create/tags/")
            || rest.starts_with("minecraft/tags/")
            || rest.starts_with("c/tags/")
            || rest.starts_with("create/worldgen/")
            || rest.starts_with("create/neoforge/biome_modifier/")
            || rest.starts_with("create/enchantment/")
    };
    let assets = |rest: &str| {
        rest.starts_with("create/blockstates/")
            || rest.starts_with("create/models/")
            || rest.starts_with("create/textures/")
            || rest.starts_with("create/lang/")
            || rest.starts_with("create/sounds/")
            || rest.starts_with("create/particles/")
            || rest == "create/sounds.json"
    };
    match path.split_once('/') {
        Some(("data", rest)) => data(rest) && !rest.contains("/.") && path.ends_with(".json"),
        Some(("assets", rest)) => {
            assets(rest)
                && [".json", ".png", ".ogg", ".mcmeta", ".obj", ".mtl"]
                    .iter()
                    .any(|e| rest.ends_with(e))
        }
        _ => false,
    }
}

/// The `c:` item tag files the port writes (`c_tags`): for each `c:` tag Create's recipes and
/// item tags name (and the `c:` tags those name in turn), the vanilla members the port gives it
/// after the members of Create's own file at that path, if there is one; (file, contents).
fn c_tag_files(files: &BTreeMap<String, Vec<u8>>) -> Vec<(String, Value)> {
    let mut named = std::collections::BTreeSet::new();
    for (path, bytes) in files {
        let item_data = path.starts_with("data/create/recipe/") || path.contains("/tags/item/");
        if item_data
            && kept(path)
            && let Ok(doc) = serde_json::from_slice::<Value>(bytes)
        {
            crate::c_tags::named(&doc, &mut named);
        }
    }
    let mut out = Vec::new();
    let mut done = std::collections::BTreeSet::new();
    while let Some(path) = named.pop_first() {
        if !done.insert(path.clone()) {
            continue;
        }
        let members = crate::c_tags::members(&path);
        if members.is_empty() {
            continue;
        }
        for m in &members {
            if let Some(t) = m.strip_prefix("#c:") {
                named.insert(t.to_owned());
            }
        }
        let rel = format!("data/c/tags/item/{path}.json");
        let mut values: Vec<Value> = files
            .get(&rel)
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
            .and_then(|d| d["values"].as_array().cloned())
            .unwrap_or_default();
        for m in members {
            if !values.iter().any(|v| v.as_str() == Some(m.as_str())) {
                values.push(Value::String(m));
            }
        }
        out.push((rel, json!({ "values": values })));
    }
    out
}

/// The mod folder's (`create/`) files that make up its crate, besides `src/`, `data/` and
/// `assets/`.
const CRATE_FILES: &[&str] = &["sunrise-mod.toml", "Cargo.toml"];

/// The repository's notices that go into the package beside them.
const NOTICES: &[&str] = &["LICENSE", "README.md", "THIRD_PARTY_NOTICES.md"];

/// How Horizon moves Create's particles that the glue spawns by name. Create's particle files
/// (`assets/create/particles/*.json`, its sprite lists) are All Rights Reserved: they come from
/// the player's jar, and only this behaviour, which is the port's own, is added to them here.
fn particle_behaviour(path: &str) -> Option<Value> {
    match path {
        "assets/create/particles/rotation_indicator.json"
        | "assets/create/particles/soul.json"
        | "assets/create/particles/soul_base.json"
        | "assets/create/particles/air.json" => Some(json!({ "velocity": [0.0, 0.0, 0.0] })),
        "assets/create/particles/steam_jet.json" => Some(json!({
            "velocity": [0.0, 0.0, 0.0],
            "animation": "age",
            "gravity": 0.0,
            "friction": 1.0,
            "lifetime": 21,
            "size": [0.375, 0.375]
        })),
        _ => None,
    }
}

/// Whether the sources are exactly the jar `create/sunrise-mod.toml` names, which the launcher
/// layers under the mod: then what would be copied from it unchanged is left to that layer.
fn layered(sources: &[PathBuf]) -> Result<bool, String> {
    let [source] = sources else {
        return Ok(false);
    };
    if source.is_dir() {
        return Ok(false);
    }
    let hash = crate::jar::sha512_of(source).map_err(|e| format!("{}: {e}", source.display()))?;
    if hash != crate::jar::CREATE.sha512 {
        eprintln!(
            "{}: not the jar create/sunrise-mod.toml names (Create {}, {}); copying all of it",
            source.display(),
            crate::jar::CREATE.version,
            crate::jar::CREATE.file
        );
    }
    Ok(hash == crate::jar::CREATE.sha512)
}

/// The mod's own file that keeps a data file of the jar layer from loading: a data map removes
/// the jar's entries (layers of data maps merge, and the host reads a data map's `remove` list),
/// any other file fails a load condition (the top layer's file is the one read): Create's own
/// condition form, `neoforge:mod_loaded`, naming a mod id no mod has. `None` where the mod cannot: tags merge and have no removal in
/// 26.3, and a mod's files stay in its own namespace (the jar's `data/neoforge/data_maps` load).
fn left_out(path: &str, bytes: &[u8]) -> Option<Value> {
    if !path.starts_with("data/create/") || path.contains("/tags/") {
        return None;
    }
    if path.contains("/data_maps/") {
        let doc: Value = serde_json::from_slice(bytes).ok()?;
        let keys: Vec<&String> = doc["values"].as_object()?.keys().collect();
        return Some(json!({ "remove": keys }));
    }
    // World generation reads every layer's placed features and biome modifiers, load conditions
    // or not, so the jar's modifiers still add these to biomes: a placed feature that runs no
    // times stands in for each.
    if path.starts_with("data/create/worldgen/placed_feature/") {
        return Some(json!({
            "feature": "minecraft:ore_dirt",
            "placement": [{ "type": "minecraft:count", "count": 0 }]
        }));
    }
    Some(json!({ "neoforge:conditions": [
        { "type": "neoforge:mod_loaded", "modid": NO_MOD }
    ] }))
}

/// A mod id no mod has (ids are lower case, and this one is the port's own).
const NO_MOD: &str = "create_horizon_left_out";

/// Vanilla's `template_chain` with `minecraft:block/iron_chain`, its texture scrolled `k` pixels
/// down: the plane's top part shows the texture from `k` on, the bottom part its first `k` rows.
fn scrolled_chain(k: u32) -> Value {
    let k = f64::from(k);
    let rotation = json!({ "origin": [8, 8, 8], "axis": "y", "angle": 45 });
    let mut elements = Vec::new();
    // (from, to, the face names, each face's u range)
    let planes = [
        (
            [6.5, 8.0],
            [9.5, 8.0],
            ["north", "south"],
            [[3.0, 0.0], [0.0, 3.0]],
        ),
        (
            [8.0, 6.5],
            [8.0, 9.5],
            ["west", "east"],
            [[6.0, 3.0], [3.0, 6.0]],
        ),
    ];
    for (from, to, faces, us) in planes {
        // (y from, y to, v from, v to), v running down from the face's top: the top part shows
        // rows `k` to 16, the bottom part the wrapped rows 0 to `k`.
        for (y0, y1, v0, v1) in [(k, 16.0, k, 16.0), (0.0, k, 0.0, k)] {
            let mut f = Map::new();
            for (name, u) in faces.iter().zip(us) {
                f.insert(
                    (*name).into(),
                    json!({ "uv": [u[0], v0, u[1], v1], "texture": "#all" }),
                );
            }
            elements.push(json!({
                "from": [from[0], y0, from[1]],
                "to": [to[0], y1, to[1]],
                "rotation": rotation,
                "shade_direction_override": "up",
                "faces": f,
            }));
        }
    }
    json!({
        "parent": "minecraft:block/block",
        "textures": { "particle": "minecraft:block/iron_chain", "all": "minecraft:block/iron_chain" },
        "elements": elements,
    })
}

/// Steam jet frames a row of its grid sprite (the glue's `JET_GRID`).
const JET_GRID: usize = 5;

/// The extendo grip's cog angles within its 45° repeat, and its swing's steps (the glue's
/// `GRIP_COG_STEPS`, `GRIP_SWING_STEPS`).
const GRIP_COG_STEPS: usize = 10;
const GRIP_SWING_STEPS: usize = 8;

/// The fragment stage of the glue's textured world pipelines.
const TEXTURED_SHADER: &str = "fn pipeline_fragment(input: PipelineInput) -> vec4<f32> {
    return pipeline_texture(input.uv) * input.color;
}
";

pub fn prepare(repo: &Path, sources: &[PathBuf], out: &Path) -> Result<String, String> {
    let layered = layered(sources)?;
    let files = read_sources(sources)?;
    if !files
        .keys()
        .any(|k| k.starts_with("assets/create/textures/"))
    {
        return Err("no assets/create/textures in the sources: is this Create's jar?".into());
    }
    let tmp = out.with_extension("preparing");
    let _ = fs::remove_dir_all(&tmp);
    // Files the jar layer already gives as they are, so they are not written again.
    let from_jar = std::cell::RefCell::new(std::collections::BTreeSet::new());
    let put = |rel: &str, bytes: &[u8]| -> Result<(), String> {
        if layered && files.get(rel).is_some_and(|b| b == bytes) {
            from_jar.borrow_mut().insert(rel.to_owned());
            return Ok(());
        }
        let p = tmp.join(rel);
        fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))
    };
    let mod_dir = repo.join("create");
    let mut crate_files = BTreeMap::new();
    walk(&mod_dir, &mod_dir.join("src"), &mut crate_files)?;
    let files_of = |dir: &Path, names: &[&str]| -> Result<Vec<(String, Vec<u8>)>, String> {
        names
            .iter()
            .map(|name| {
                let p = dir.join(name);
                Ok((
                    (*name).to_owned(),
                    fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?,
                ))
            })
            .collect()
    };
    crate_files.extend(files_of(&mod_dir, CRATE_FILES)?);
    crate_files.extend(files_of(repo, NOTICES)?);
    for (rel, bytes) in &crate_files {
        put(rel, bytes)?;
    }
    let mut ours = BTreeMap::new();
    walk(&mod_dir, &mod_dir.join("data"), &mut ours)?;
    walk(&mod_dir, &mod_dir.join("assets"), &mut ours)?;
    for (rel, bytes) in &ours {
        put(rel, bytes)?;
    }
    let declared: Vec<String> = ours
        .keys()
        .filter_map(|k| {
            k.strip_prefix("data/create/sunrise/block/")
                .or_else(|| k.strip_prefix("data/create/sunrise/item/"))
                .and_then(|n| n.strip_suffix(".json"))
                .map(str::to_string)
        })
        .collect();

    let unplaced = unplaced_features(&files);
    let (mut recipes, mut skipped, mut copied, mut worldgen_left_out) = (0, 0, 0, 0);
    for (path, bytes) in &files {
        // Our own files (a blockstate we draw differently) stand over Create's.
        if !kept(path) || ours.contains_key(path) {
            continue;
        }
        if let Some(behaviour) = particle_behaviour(path) {
            let mut doc: Value =
                serde_json::from_slice(bytes).map_err(|e| format!("{path}: {e}"))?;
            doc["sunrise:behaviour"] = behaviour;
            put(path, &serde_json::to_vec_pretty(&doc).unwrap())?;
            copied += 1;
            continue;
        }
        if path.starts_with("data/create/recipe/") {
            let doc: Value = serde_json::from_slice(bytes).map_err(|e| format!("{path}: {e}"))?;
            match convert_recipe(doc) {
                Some(doc) => {
                    put(path, &serde_json::to_vec_pretty(&doc).unwrap())?;
                    recipes += 1;
                }
                None => skipped += 1,
            }
        } else if path.starts_with("assets/create/models/")
            && path.ends_with(".json")
            && bytes.windows(18).any(|w| w == b"neoforge:composite")
        {
            let mut doc: Value =
                serde_json::from_slice(bytes).map_err(|e| format!("{path}: {e}"))?;
            flatten_composite(&mut doc);
            put(path, &serde_json::to_vec_pretty(&doc).unwrap())?;
            copied += 1;
        } else if let Some(door) = sliding_door_states(path, bytes) {
            put(path, &serde_json::to_vec_pretty(&door).unwrap())?;
            copied += 1;
        } else if path.starts_with("data/create/worldgen/")
            || path.starts_with("data/create/neoforge/biome_modifier/")
        {
            let mut doc: Value =
                serde_json::from_slice(bytes).map_err(|e| format!("{path}: {e}"))?;
            if convert_worldgen(path, &mut doc, &unplaced) {
                // 26.3 reads configured features from `worldgen/feature/` (1.21.1's
                // `worldgen/configured_feature/`).
                let path = path.replace("/worldgen/configured_feature/", "/worldgen/feature/");
                put(&path, &serde_json::to_vec_pretty(&doc).unwrap())?;
                copied += 1;
            } else {
                worldgen_left_out += 1;
            }
        } else if path.starts_with("data/create/enchantment/") {
            // Capacity and Potato Recovery: an effect's requirements name their kind in `type`
            // (1.21.1: `condition`), as loot conditions do.
            let mut doc: Value =
                serde_json::from_slice(bytes).map_err(|e| format!("{path}: {e}"))?;
            convert_loot(&mut doc);
            put(path, &serde_json::to_vec_pretty(&doc).unwrap())?;
            copied += 1;
        } else if path.starts_with("data/create/loot_table/") {
            let mut doc: Value =
                serde_json::from_slice(bytes).map_err(|e| format!("{path}: {e}"))?;
            convert_loot(&mut doc);
            if path == "data/create/loot_table/blocks/gearbox.json" {
                gearbox_drops(&mut doc);
            }
            put(path, &serde_json::to_vec_pretty(&doc).unwrap())?;
            copied += 1;
        } else {
            put(path, bytes)?;
            copied += 1;
            // Fluid textures go on the block atlas too (Create's `FluidRenderer` reads fluid
            // sprites from the block atlas; vanilla's atlas reads `textures/block/`): the
            // tanks' fluid parts use them.
            if let Some(rest) = path.strip_prefix("assets/create/textures/fluid/") {
                put(&format!("assets/create/textures/block/fluid/{rest}"), bytes)?;
            }
            // The coupling's entity models (`CouplingRenderer`) are drawn as part blocks: their
            // texture on the block atlas, the models beside the block models pointing at it.
            // Create's special textures (`AllSpecialTextures`: the outliner's checkered and glue
            // faces) on the block atlas too, where a world pipeline samples them.
            if let Some(rest) = path.strip_prefix("assets/create/textures/special/") {
                put(
                    &format!("assets/create/textures/block/special/{rest}"),
                    bytes,
                )?;
            }
            // The icon atlas (`AllIcons`), for the value boxes' frames and icons in the world.
            if path == "assets/create/textures/gui/icons.png" {
                put("assets/create/textures/block/special/gui_icons.png", bytes)?;
            }
            if path == "assets/create/textures/entity/coupling.png" {
                put("assets/create/textures/block/entity/coupling.png", bytes)?;
            }
            if let Some(rest) = path.strip_prefix("assets/create/models/entity/minecart_coupling/")
                && rest.ends_with(".json")
            {
                let text = String::from_utf8_lossy(bytes)
                    .replace("create:entity/coupling", "create:block/entity/coupling");
                put(
                    &format!("assets/create/models/block/minecart_coupling/{rest}"),
                    text.as_bytes(),
                )?;
            }
        }
    }
    // Tinted copies of the models the glue colours as Create does (`SuperByteBuffer.color`): a
    // factory gauge's arrows (`FactoryPanelRenderer.renderPath`) and a dancing arm's body
    // (`ArmRenderer`'s rainbow). Made from the player's jar, never committed.
    let mut tinted = 0;
    for (path, bytes) in &files {
        let Some(name) = tinted_model(path) else {
            continue;
        };
        let mut doc: Value = serde_json::from_slice(bytes).map_err(|e| format!("{path}: {e}"))?;
        tint_faces(&mut doc);
        put(&name, &serde_json::to_vec_pretty(&doc).unwrap())?;
        tinted += 1;
    }
    let _ = tinted;
    // Connected textures, as OptiFine's: a ctm properties file per block and sheet, the tiles.
    let read = |p: &str| -> Option<Vec<u8>> {
        fs::read(tmp.join(p)).ok().or_else(|| files.get(p).cloned())
    };
    let (ct_files, ct_left_out) = crate::ct::files(&read);
    for (rel, bytes) in &ct_files {
        put(rel, bytes)?;
    }
    // The `c:` item tags Create's data names, with the vanilla members the port gives them.
    let c_tags = c_tag_files(&files);
    for (rel, doc) in &c_tags {
        put(rel, &serde_json::to_vec_pretty(doc).unwrap())?;
    }
    // The frames of the partial models Create's item renderers turn.
    for (item, part, _, axis, pivot) in SPINNERS {
        let rel = format!("assets/create/models/item/{item}/{part}.json");
        let Some(doc) = read(&rel).and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else {
            continue;
        };
        for frame in 0..SPIN_FRAMES {
            put(
                &format!("assets/create/models/item/{item}/{part}_spin/{frame}.json"),
                &serde_json::to_vec_pretty(&spin_frame(&doc, axis, pivot, frame)).unwrap(),
            )?;
        }
    }
    // 26.3 finds an item's model through `assets/<ns>/items/<id>.json`; 1.21.1 had none.
    // Hidden partial-model blocks (`part_*`) have no item.
    for name in declared.iter().filter(|n| !n.starts_with("part_")) {
        let model = format!("assets/create/models/item/{name}.json");
        let target = if files.contains_key(&model) {
            format!("create:item/{name}")
        } else {
            format!("create:block/{name}")
        };
        let doc = json!({ "model": custom_rendered(name).unwrap_or_else(|| json!({ "type": "minecraft:model", "model": target })) });
        put(
            &format!("assets/create/items/{name}.json"),
            &serde_json::to_vec_pretty(&doc).unwrap(),
        )?;
    }
    // A fluid in the world brings its bucket, `<fluid>_bucket`, whose model is Create's.
    for name in ours.keys().filter_map(|k| {
        k.strip_prefix("data/create/sunrise/fluid/")?
            .strip_suffix(".json")
    }) {
        let doc = json!({ "model": { "type": "minecraft:model", "model": format!("create:item/{name}_bucket") } });
        put(
            &format!("assets/create/items/{name}_bucket.json"),
            &serde_json::to_vec_pretty(&doc).unwrap(),
        )?;
    }
    // The chain conveyor's chain scrolled a pixel at a time (`ChainConveyorRenderer.renderChain`'s
    // `minV`): vanilla's chain model with its texture shifted down `k` pixels, each plane split
    // where the texture wraps. Made here as it names vanilla's chain sprite by id.
    for k in 1..16 {
        put(
            &format!("assets/create/models/block/chain_conveyor/chain_{k}.json"),
            &serde_json::to_vec_pretty(&scrolled_chain(k)).unwrap(),
        )?;
    }
    // The steam jet's frames (`particles/steam_jet.json`) on the block atlas, in a square grid
    // (`JET_GRID` a row, left to right then down) as an atlas sprite is square: the glue draws
    // the jet's quads itself (`SteamJetParticle.render`) through a world pipeline, which samples
    // one block atlas sprite.
    let frames: Vec<&Vec<u8>> = (0..)
        .map_while(|i| {
            files.get(&format!(
                "assets/create/textures/particle/steam_jet_{i}.png"
            ))
        })
        .collect();
    if !frames.is_empty() {
        let images = frames
            .iter()
            .map(|b| crate::ct::decode(b))
            .collect::<Result<Vec<_>, _>>()?;
        let (w, h) = (images[0].w as usize, images[0].h as usize);
        let side = JET_GRID * w;
        let mut px = vec![0u8; side * JET_GRID * h * 4];
        for (i, im) in images.iter().enumerate().take(JET_GRID * JET_GRID) {
            if (im.w as usize, im.h as usize) != (w, h) {
                return Err("steam jet frames of different sizes".into());
            }
            let (cx, cy) = (i % JET_GRID, i / JET_GRID);
            for y in 0..h {
                let src = &im.px[y * w * 4..(y + 1) * w * 4];
                let at = ((cy * h + y) * side + cx * w) * 4;
                px[at..at + src.len()].copy_from_slice(src);
            }
        }
        let grid = crate::ct::Image {
            w: side as u32,
            h: (JET_GRID * h) as u32,
            px,
        };
        put(
            "assets/create/textures/block/special/steam_jet.png",
            &crate::ct::encode(&grid),
        )?;
    }
    // The glue's world pipelines' fragment shader (`create:textured`): the sampled sprite tinted
    // by the quad's colour, as a `position_tex_color` render type draws it. The port's own code.
    put(
        "assets/create/shaders/pipeline/textured.wgsl",
        TEXTURED_SHADER.as_bytes(),
    )?;
    // The placement helpers' ghosts, baked from the jar's models (`ghosts.rs`).
    let (ghost_models, ghosts) = crate::ghosts::source_file(&files)?;
    put(crate::ghosts::RS, ghost_models.as_bytes())?;
    // The jar layer (`[[jar]]` in sunrise-mod.toml) also holds the data left out above (recipes for
    // other mods, the worldgen 26.3 cannot place, recipe advancements...): the mod's own file at
    // the same path keeps each out, as it was when the mod carried copies of what it kept.
    let mut kept_out = 0;
    {
        let from_jar = from_jar.borrow();
        for (path, bytes) in &files {
            if !path.ends_with(".json") || from_jar.contains(path) || tmp.join(path).exists() {
                continue;
            }
            if let Some(doc) = left_out(path, bytes) {
                put(path, &serde_json::to_vec_pretty(&doc).unwrap())?;
                kept_out += 1;
            }
        }
    }
    let _ = fs::remove_dir_all(out);
    fs::rename(&tmp, out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut report = Map::new();
    report.insert("out".into(), json!(out.display().to_string()));
    report.insert("declared".into(), json!(declared.len()));
    report.insert("recipes".into(), json!(recipes));
    report.insert("recipes_skipped_for_other_mods".into(), json!(skipped));
    report.insert("worldgen_left_out".into(), json!(worldgen_left_out));
    let from_jar = from_jar.borrow().len();
    report.insert(
        "files_copied".into(),
        json!((copied as usize).saturating_sub(from_jar)),
    );
    report.insert("files_left_to_the_jar_layer".into(), json!(from_jar));
    report.insert("jar_files_kept_out".into(), json!(kept_out));
    report.insert("c_tags_written".into(), json!(c_tags.len()));
    report.insert("connected_texture_files".into(), json!(ct_files.len()));
    report.insert("connected_textures_left_out".into(), json!(ct_left_out));
    report.insert("placement_ghosts".into(), json!(ghosts));
    Ok(serde_json::to_string_pretty(&Value::Object(report)).unwrap())
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn keeps_left_out_jar_data_out() {
        let recipe = left_out("data/create/recipe/crushing/tin_ore.json", b"{}").unwrap();
        assert_eq!(
            recipe["neoforge:conditions"][0],
            json!({ "type": "neoforge:mod_loaded", "modid": NO_MOD })
        );
        let placed = left_out(
            "data/create/worldgen/placed_feature/striated_ores_overworld.json",
            b"{}",
        )
        .unwrap();
        assert_eq!(placed["placement"][0]["count"], 0);
        let map = left_out(
            "data/create/data_maps/item/fuels.json",
            br#"{"values": {"create:a": 1, "create:b": 2}}"#,
        )
        .unwrap();
        assert_eq!(map, json!({ "remove": ["create:a", "create:b"] }));
        assert!(left_out("data/create/tags/item/x.json", b"{}").is_none());
        assert!(left_out("data/neoforge/data_maps/block/waxables.json", b"{}").is_none());
    }

    #[test]
    fn converts_1_21_1_ingredients() {
        let r = json!({"type": "minecraft:crafting_shaped",
            "key": {"A": {"item": "create:andesite_alloy"}, "B": {"tag": "c:ingots/iron"}},
            "pattern": ["AB"], "result": {"id": "create:shaft", "count": 8}});
        let c = convert_recipe(r).unwrap();
        assert_eq!(c["key"]["A"], "create:andesite_alloy");
        assert_eq!(c["key"]["B"], "#c:ingots/iron");
        let s = json!({"type": "minecraft:crafting_shapeless",
            "ingredients": [[{"item": "a:b"}, {"item": "c:d"}], {"tag": "x:y"}]});
        let c = convert_recipe(s).unwrap();
        assert_eq!(c["ingredients"][0], json!(["a:b", "c:d"]));
        assert_eq!(c["ingredients"][1], "#x:y");
        let a = json!({"type": "create:sequenced_assembly", "ingredient": {"tag": "c:plates/gold"},
            "sequence": [{"type": "create:deploying",
                "ingredients": [{"item": "a:b"}, [{"tag": "c:nuggets/iron"}, {"tag": "c:nuggets/zinc"}]]}]});
        let c = convert_recipe(a).unwrap();
        assert_eq!(c["ingredient"], "#c:plates/gold");
        assert_eq!(c["sequence"][0]["ingredients"][0], "a:b");
        assert_eq!(
            c["sequence"][0]["ingredients"][1],
            json!(["#c:nuggets/iron", "#c:nuggets/zinc"])
        );
        let m = json!({"type": "create:mixing", "ingredients": [
            {"type": "neoforge:block_tag", "tag": "minecraft:convertable_to_mud"}]});
        let c = convert_recipe(m).unwrap();
        assert_eq!(c["ingredients"][0]["tag"], "minecraft:convertible_to_mud");
        // 26.3's names for items 1.21.9 renamed, in results and ingredients alike.
        let z = json!({"type": "minecraft:crafting_shaped", "key": {"N": {"item": "minecraft:chain"}},
            "pattern": ["N"], "result": {"id": "minecraft:chain", "count": 1}});
        let c = convert_recipe(z).unwrap();
        assert_eq!(c["result"]["id"], "minecraft:iron_chain");
        assert_eq!(c["key"]["N"], "minecraft:iron_chain");
    }

    #[test]
    fn writes_the_c_tags_create_s_data_names() {
        let mut files = BTreeMap::new();
        files.insert(
            "data/create/recipe/a.json".to_owned(),
            br##"{"key": {"A": {"tag": "c:ingots/iron"}, "B": {"tag": "c:glass_blocks"},
                 "C": {"tag": "c:ingots/tin"}}}"##
                .to_vec(),
        );
        files.insert(
            "data/c/tags/item/glass_blocks/colorless.json".to_owned(),
            br#"{"values": ["create:framed_glass"]}"#.to_vec(),
        );
        let tags = c_tag_files(&files);
        let get = |p: &str| {
            let rel = format!("data/c/tags/item/{p}.json");
            tags.iter().find(|(r, _)| *r == rel).map(|(_, d)| d.clone())
        };
        assert_eq!(
            get("ingots/iron").unwrap()["values"],
            json!(["minecraft:iron_ingot"])
        );
        // A tag a member names is written too, after Create's own members.
        assert_eq!(
            get("glass_blocks/colorless").unwrap()["values"],
            json!(["create:framed_glass", "minecraft:glass"])
        );
        assert!(get("glass_blocks").is_some());
        assert!(get("ingots/tin").is_none());
    }

    #[test]
    fn drops_recipes_for_other_mods() {
        let r = json!({"type": "create:milling", "neoforge:conditions": [
            {"type": "neoforge:mod_loaded", "modid": "byg"}]});
        assert!(convert_recipe(r).is_none());
        let r = json!({"type": "create:milling", "neoforge:conditions": [
            {"type": "neoforge:not", "value": {"type": "neoforge:mod_loaded", "modid": "byg"}}]});
        assert!(convert_recipe(r).is_some());
    }

    #[test]
    fn converts_1_21_1_loot_tables() {
        let mut t = json!({"type": "minecraft:block", "pools": [{
            "conditions": [{"condition": "minecraft:survives_explosion"}],
            "entries": [{"type": "minecraft:item", "name": "create:shaft",
                "functions": [{"function": "minecraft:set_count", "count": 2.0,
                    "conditions": [{"condition": "minecraft:inverted",
                        "term": {"condition": "minecraft:random_chance", "chance": 0.5}}]}]}],
            "rolls": 1.0}]});
        convert_loot(&mut t);
        let pool = &t["pools"][0];
        assert_eq!(
            pool["conditions"][0],
            json!({"type": "minecraft:survives_explosion"})
        );
        let f = &pool["entries"][0]["modifier"][0];
        assert_eq!(f["type"], "minecraft:set_count");
        assert_eq!(
            f["conditions"][0]["term"]["type"],
            "minecraft:random_chance"
        );
        assert!(pool["entries"][0].get("functions").is_none());
    }

    #[test]
    fn horizontal_gearboxes_drop_vertical_gearboxes() {
        let mut t = json!({"type": "minecraft:block", "pools": [{
            "conditions": [{"type": "minecraft:survives_explosion"}],
            "entries": [{"type": "minecraft:item", "name": "create:gearbox"}],
            "rolls": 1.0}]});
        gearbox_drops(&mut t);
        let pools = t["pools"].as_array().unwrap();
        assert_eq!(pools.len(), 2);
        assert_eq!(pools[0]["conditions"][1]["properties"]["axis"], "y");
        assert_eq!(pools[1]["conditions"][0]["type"], "minecraft:inverted");
        assert_eq!(pools[1]["entries"][0]["name"], "create:vertical_gearbox");
    }

    #[test]
    fn flattens_composite_models() {
        let mut m = serde_json::json!({
            "loader": "neoforge:composite", "parent": "block/block",
            "textures": {"particle": "create:block/a"},
            "children": {
                "glass": {"render_type": "minecraft:translucent", "textures": {"1": "create:block/g"},
                          "elements": [{"from": [0, 0, 0], "to": [1, 1, 1]}]},
                "base": {"textures": {"0": "create:block/b"},
                         "elements": [{"from": [2, 2, 2], "to": [3, 3, 3]}, {"from": [4, 4, 4], "to": [5, 5, 5]}]}
            }
        });
        flatten_composite(&mut m);
        assert!(m.get("loader").is_none() && m.get("children").is_none());
        assert_eq!(m["elements"].as_array().unwrap().len(), 3);
        assert_eq!(m["textures"]["0"], "create:block/b");
        assert_eq!(m["textures"]["1"], "create:block/g");
        assert_eq!(m["textures"]["particle"], "create:block/a");
        // Anything else stays as it was.
        let mut plain = serde_json::json!({"parent": "block/cube_all"});
        flatten_composite(&mut plain);
        assert_eq!(plain, serde_json::json!({"parent": "block/cube_all"}));
    }

    #[test]
    fn keeps_only_the_fence() {
        assert!(kept("data/create/recipe/a.json"));
        assert!(kept("data/minecraft/tags/block/mineable/pickaxe.json"));
        assert!(!kept("data/create/.wiki/block_info/a.json"));
        assert!(!kept("data/neoforge/data_maps/x.json"));
        assert!(kept("data/create/worldgen/placed_feature/zinc_ore.json"));
        assert!(kept("data/create/neoforge/biome_modifier/zinc_ore.json"));
        assert!(kept("data/create/enchantment/capacity.json"));
    }

    #[test]
    fn hidden_sliding_doors_have_no_model() {
        let states = json!({ "variants": {
            "facing=north,half=lower,open=false,visible=true": { "model": "a" },
            "facing=north,half=lower,open=true,visible=false": { "model": "a", "y": 90 }
        }});
        let bytes = serde_json::to_vec(&states).unwrap();
        let door =
            sliding_door_states("assets/create/blockstates/brass_door.json", &bytes).unwrap();
        assert_eq!(door["multipart"].as_array().unwrap().len(), 1);
        assert_eq!(door["multipart"][0]["when"]["visible"], "true");
        assert!(sliding_door_states("assets/create/blockstates/oak_window.json", &bytes).is_none());
    }

    #[test]
    fn zinc_ore_is_carried_without_create_s_filter_and_layered_ores_are_left_out() {
        let unplaced = vec!["create:striated_ores_overworld".to_string()];
        let mut placed = json!({"feature": "create:zinc_ore", "placement": [
            {"type": "minecraft:count", "count": 8}, {"type": "create:config_filter"}]});
        assert!(convert_worldgen(
            "data/create/worldgen/placed_feature/zinc_ore.json",
            &mut placed,
            &unplaced
        ));
        assert_eq!(
            placed["placement"],
            json!([{"type": "minecraft:count", "count": 8}])
        );
        let mut layered = json!({"feature": "create:striated_ores_overworld", "placement": []});
        assert!(!convert_worldgen(
            "data/create/worldgen/placed_feature/striated_ores_overworld.json",
            &mut layered,
            &unplaced
        ));
        let mut configured = json!({"type": "create:layered_ore"});
        assert!(!convert_worldgen(
            "data/create/worldgen/configured_feature/striated_ores_overworld.json",
            &mut configured,
            &unplaced
        ));
        let mut modifier =
            json!({"type": "neoforge:add_features", "features": "create:striated_ores_overworld"});
        assert!(!convert_worldgen(
            "data/create/neoforge/biome_modifier/striated_ores_overworld.json",
            &mut modifier,
            &unplaced
        ));
        let mut ore = json!({"type": "minecraft:ore", "config": {"size": 12, "targets": [
            {"state": {"Name": "create:zinc_ore"}, "target": {"predicate_type": "minecraft:tag_match",
            "tag": "minecraft:stone_ore_replaceables"}},
            {"state": {"Name": "create:x", "Properties": {"axis": "y"}}, "target": {}}]}});
        assert!(convert_worldgen(
            "data/create/worldgen/configured_feature/zinc_ore.json",
            &mut ore,
            &unplaced
        ));
        assert_eq!(ore["size"], 12);
        assert!(ore.get("config").is_none());
        assert_eq!(ore["targets"][0]["state"], "create:zinc_ore");
        assert_eq!(
            ore["targets"][1]["state"],
            json!({"id": "create:x", "properties": {"axis": "y"}})
        );
        let mut zinc = json!({"type": "neoforge:add_features", "features": "create:zinc_ore"});
        assert!(convert_worldgen(
            "data/create/neoforge/biome_modifier/zinc_ore.json",
            &mut zinc,
            &unplaced
        ));
        assert!(kept("assets/create/textures/block/a.png"));
        assert!(!kept("assets/create/ponder/a.nbt"));
        assert!(!kept("assets/create/lang/README.md"));
        assert!(kept(
            "assets/create/models/block/water_wheel/water_wheel.obj"
        ));
        assert!(kept(
            "assets/create/models/block/water_wheel/water_wheel.mtl"
        ));
    }
}

/// Every `c:` item tag a recipe the port loads names (Create's processing, sequenced assembly,
/// mechanical crafting, crafting, cooking and stonecutting recipes alike) holds at least one
/// item: a recipe whose ingredient names an empty or missing tag finds nothing, so a machine or
/// crafting table never takes its items. Runs where Create's jar is at hand (`$CREATE_JAR`, else
/// the `prepare` download cache).
#[cfg(test)]
mod c_tags_resolve {
    use super::*;
    use std::collections::{BTreeSet, HashSet};

    /// The `c:` item tags `v` names: `#c:` strings and `{"tag": "c:.."}` objects, not fluid
    /// ingredients (those with an `amount`).
    fn item_tags(v: &Value, out: &mut BTreeSet<String>) {
        match v {
            Value::String(s) => {
                if let Some(t) = s.strip_prefix("#c:") {
                    out.insert(t.to_owned());
                }
            }
            Value::Array(a) => a.iter().for_each(|x| item_tags(x, out)),
            Value::Object(o) => {
                if o.get("amount").is_none()
                    && let Some(t) = o.get("tag").and_then(Value::as_str)
                    && let Some(t) = t.strip_prefix("c:")
                {
                    out.insert(t.to_owned());
                }
                o.values().for_each(|x| item_tags(x, out));
            }
            _ => {}
        }
    }

    #[test]
    fn every_c_tag_a_loaded_recipe_names_holds_an_item() {
        let jar = std::env::var_os("CREATE_JAR")
            .map(PathBuf::from)
            .or_else(|| {
                crate::jar::cache_dir()
                    .ok()
                    .map(|d| d.join(crate::jar::CREATE.file))
            })
            .filter(|p| p.is_file());
        let Some(jar) = jar else {
            eprintln!("no Create jar at hand ($CREATE_JAR or the prepare cache): skipped");
            return;
        };
        let files = read_sources(&[jar]).unwrap();
        // The port's file stands over the jar's at the same path.
        let written: BTreeMap<String, Value> = c_tag_files(&files).into_iter().collect();
        let values = |path: &str| -> Option<Vec<Value>> {
            let rel = format!("data/c/tags/item/{path}.json");
            let doc = written
                .get(&rel)
                .cloned()
                .or_else(|| serde_json::from_slice(files.get(&rel)?).ok())?;
            doc["values"].as_array().cloned()
        };
        let decl = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../create/data/create/sunrise");
        let mut items = HashSet::new();
        for kind in ["item", "block"] {
            for e in fs::read_dir(decl.join(kind)).unwrap() {
                let name = e.unwrap().file_name().to_string_lossy().into_owned();
                if let Some(n) = name.strip_suffix(".json")
                    && !n.starts_with("part_")
                {
                    items.insert(format!("create:{n}"));
                }
            }
        }
        fn holds(
            path: &str,
            values: &dyn Fn(&str) -> Option<Vec<Value>>,
            items: &HashSet<String>,
            seen: &mut Vec<String>,
        ) -> bool {
            if seen.iter().any(|s| s == path) {
                return false;
            }
            seen.push(path.to_owned());
            values(path).unwrap_or_default().iter().any(|v| {
                let id = v.as_str().or_else(|| v["id"].as_str()).unwrap_or("");
                match id.strip_prefix('#') {
                    Some(t) => match t.strip_prefix("c:") {
                        Some(c) => holds(c, values, items, seen),
                        // Vanilla's own item tags.
                        None => t.starts_with("minecraft:"),
                    },
                    None => id.starts_with("minecraft:") || items.contains(id),
                }
            })
        }
        let (mut recipes, mut named) = (0, BTreeSet::new());
        let mut empty = Vec::new();
        for (path, bytes) in &files {
            if !path.starts_with("data/create/recipe/") || !kept(path) {
                continue;
            }
            let doc: Value = serde_json::from_slice(bytes).unwrap();
            if convert_recipe(doc.clone()).is_none() {
                continue;
            }
            recipes += 1;
            let mut tags = BTreeSet::new();
            item_tags(&doc, &mut tags);
            for t in tags {
                if !holds(&t, &values, &items, &mut Vec::new()) {
                    empty.push(format!("{path}: c:{t}"));
                }
                named.insert(t);
            }
        }
        assert!(recipes > 1000, "{recipes} recipes read from the jar");
        assert!(named.contains("nuggets/iron") && named.contains("nuggets/zinc"));
        assert!(
            empty.is_empty(),
            "recipes naming a c: tag that holds no item: {empty:?}"
        );
    }
}

/// What the repository may hold under `create/assets`: the generator's own block states and
/// block models (which name Create's models and textures, never copy them) and Catnip's MIT
/// placement indicator. Everything under Create's `src/main/resources/assets/` is All Rights
/// Reserved: textures, models, particles, sounds and translations of Create's come from the
/// player's jar in `prepare`, never from a committed file. Where Create's jar is at hand
/// (`$CREATE_JAR`, else the `prepare` download cache), no committed file under `create/assets`
/// or `create/data` may be one of the jar's assets, byte for byte or as JSON (a file that holds
/// all of a jar asset's JSON at its own path, with keys added, counts as a copy).
#[cfg(test)]
mod committed_assets {
    use serde_json::Value;
    use std::collections::{BTreeMap, HashSet};
    use std::io::Read;
    use std::path::{Path, PathBuf};

    fn files(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                files(&p, root, out);
            } else {
                out.push(
                    p.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn only_generated_states_and_models_and_catnip_s_indicator() {
        let root = repo().join("create/assets");
        let mut all = Vec::new();
        files(&root, &root, &mut all);
        let refused: Vec<&String> = all
            .iter()
            .filter(|f| {
                !(f.starts_with("create/blockstates/") && f.ends_with(".json")
                    || f.starts_with("create/models/block/") && f.ends_with(".json")
                    || *f == "create/textures/gui/placement_indicator.png")
            })
            .collect();
        assert!(
            refused.is_empty(),
            "only generated block states and models belong in create/assets; Create's own assets \
             are All Rights Reserved and come from the player's jar in `prepare`: {refused:?}"
        );
    }

    /// Whether `a` holds all of `b`: the same, or an object with each of `b`'s keys holding
    /// all of `b`'s value there.
    fn holds(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Object(a), Value::Object(b)) => {
                b.iter().all(|(k, v)| a.get(k).is_some_and(|w| holds(w, v)))
            }
            (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
            _ => a == b,
        }
    }

    #[test]
    fn no_committed_file_is_one_of_create_s_assets() {
        let jar = std::env::var_os("CREATE_JAR")
            .map(PathBuf::from)
            .or_else(|| {
                crate::jar::cache_dir()
                    .ok()
                    .map(|d| d.join(crate::jar::CREATE.file))
            })
            .filter(|p| p.is_file());
        let Some(jar) = jar else {
            eprintln!("no Create jar at hand ($CREATE_JAR or the prepare cache): skipped");
            return;
        };
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&jar).unwrap()).unwrap();
        let mut bytes_seen: HashSet<Vec<u8>> = HashSet::new();
        let mut json_seen: HashSet<String> = HashSet::new();
        let mut json_at: BTreeMap<String, Value> = BTreeMap::new();
        for i in 0..zip.len() {
            let mut e = zip.by_index(i).unwrap();
            let name = e.name().to_owned();
            if e.is_dir() || !name.starts_with("assets/") {
                continue;
            }
            let mut b = Vec::new();
            e.read_to_end(&mut b).unwrap();
            if name.ends_with(".json")
                && let Ok(v) = serde_json::from_slice::<Value>(&b)
            {
                json_seen.insert(v.to_string());
                json_at.insert(name, v);
            }
            bytes_seen.insert(b);
        }
        let mut copies = Vec::new();
        for top in ["create/assets", "create/data"] {
            let root = repo().join(top);
            let mut all = Vec::new();
            files(&root, &root, &mut all);
            for f in all {
                let b = std::fs::read(root.join(&f)).unwrap();
                let at = format!("{}/{f}", top.trim_start_matches("create/"));
                let json = serde_json::from_slice::<Value>(&b).ok();
                let copy = bytes_seen.contains(&b)
                    || json.as_ref().is_some_and(|v| {
                        json_seen.contains(&v.to_string())
                            || json_at.get(&at).is_some_and(|j| holds(v, j))
                    });
                if copy {
                    copies.push(format!("{top}/{f}"));
                }
            }
        }
        assert!(
            copies.is_empty(),
            "committed files that are Create's All Rights Reserved assets: {copies:?}"
        );
    }

    #[test]
    fn a_file_with_keys_added_still_holds_the_original() {
        let jar = serde_json::json!({"textures": ["a", "b"], "n": 1});
        let ours = serde_json::json!({"textures": ["a", "b"], "n": 1.0, "horizon:x": {}});
        assert!(holds(&ours, &jar));
        assert!(!holds(&serde_json::json!({"textures": ["a"]}), &jar));
    }
}
