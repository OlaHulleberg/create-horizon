//! The one-time prepare step. A Create jar holds MIT data (`data/`, generated `assets/`) and All
//! Rights Reserved assets (textures, hand-written models, sounds); neither ships with the port. This
//! reads them from the player's own jar (or folders laid out like one) and writes the content mod
//! Horizon loads: the port's declarations (`content/`), Create's data converted from 1.21.1 to
//! 26.3, and its assets with the item model definitions 26.3 needs. Reading a zip runs no Java.

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
        Value::Object(o) if o.len() == 1 && o.get("item").is_some_and(Value::is_string) => {
            o["item"].clone()
        }
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

/// A recipe in 26.3's syntax, or `None` when its NeoForge conditions name another mod.
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
    Some(doc)
}

/// A 1.21.1 loot table as 26.3 writes it, the form Horizon reads: a condition names its kind in
/// `type` (1.21.1: `condition`), a function in `type` (1.21.1: `function`), and a pool's or
/// entry's functions are its `modifier` list (1.21.1: `functions`).
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
fn kept(path: &str) -> bool {
    let data = |rest: &str| {
        rest.starts_with("create/recipe/")
            || rest.starts_with("create/loot_table/")
            || rest.starts_with("create/tags/")
            || rest.starts_with("minecraft/tags/")
            || rest.starts_with("c/tags/")
    };
    let assets = |rest: &str| {
        rest.starts_with("create/blockstates/")
            || rest.starts_with("create/models/")
            || rest.starts_with("create/textures/")
            || rest.starts_with("create/lang/")
            || rest.starts_with("create/sounds/")
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

/// The `c:` item tag files of `common_tags::ITEM_TAGS`: (tag path, file, contents).
fn common_tag_files() -> impl Iterator<Item = (&'static str, String, Value)> {
    crate::common_tags::ITEM_TAGS.iter().map(|(path, values)| {
        (
            *path,
            format!("data/c/tags/item/{path}.json"),
            json!({ "values": values }),
        )
    })
}

pub fn prepare(content: &Path, sources: &[PathBuf], out: &Path) -> Result<String, String> {
    let files = read_sources(sources)?;
    if !files
        .keys()
        .any(|k| k.starts_with("assets/create/textures/"))
    {
        return Err("no assets/create/textures in the sources: is this Create's jar?".into());
    }
    let tmp = out.with_extension("preparing");
    let _ = fs::remove_dir_all(&tmp);
    let put = |rel: &str, bytes: &[u8]| -> Result<(), String> {
        let p = tmp.join(rel);
        fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))
    };
    let mut ours = BTreeMap::new();
    walk(content, content, &mut ours)?;
    for (rel, bytes) in &ours {
        put(rel, bytes)?;
    }
    let declared: Vec<String> = ours
        .keys()
        .filter_map(|k| {
            k.strip_prefix("data/create/horizon/block/")
                .or_else(|| k.strip_prefix("data/create/horizon/item/"))
                .and_then(|n| n.strip_suffix(".json"))
                .map(str::to_string)
        })
        .collect();

    let (mut recipes, mut skipped, mut copied) = (0, 0, 0);
    for (path, bytes) in &files {
        if !kept(path) {
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
        } else if path.starts_with("data/create/loot_table/") {
            let mut doc: Value =
                serde_json::from_slice(bytes).map_err(|e| format!("{path}: {e}"))?;
            convert_loot(&mut doc);
            put(path, &serde_json::to_vec_pretty(&doc).unwrap())?;
            copied += 1;
        } else {
            put(path, bytes)?;
            copied += 1;
        }
    }
    // NeoForge's common tags Create's recipes name and nobody else defines here.
    let mut common = 0;
    for (_, rel, doc) in common_tag_files() {
        if !files.contains_key(&rel) {
            put(&rel, &serde_json::to_vec_pretty(&doc).unwrap())?;
            common += 1;
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
        let doc = json!({ "model": { "type": "minecraft:model", "model": target } });
        put(
            &format!("assets/create/items/{name}.json"),
            &serde_json::to_vec_pretty(&doc).unwrap(),
        )?;
    }
    let _ = fs::remove_dir_all(out);
    fs::rename(&tmp, out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut report = Map::new();
    report.insert("out".into(), json!(out.display().to_string()));
    report.insert("declared".into(), json!(declared.len()));
    report.insert("recipes".into(), json!(recipes));
    report.insert("recipes_skipped_for_other_mods".into(), json!(skipped));
    report.insert("files_copied".into(), json!(copied));
    report.insert("common_tags_added".into(), json!(common));
    Ok(serde_json::to_string_pretty(&Value::Object(report)).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    #[test]
    fn common_tags_cover_create_s_iron_and_nest_their_parts() {
        let all: Vec<_> = common_tag_files().collect();
        let (_, rel, doc) = all.iter().find(|(p, ..)| *p == "ingots/iron").unwrap();
        assert_eq!(rel, "data/c/tags/item/ingots/iron.json");
        assert_eq!(doc["values"], json!(["minecraft:iron_ingot"]));
        // A tag naming another `c:` tag finds it in the table too.
        for (_, _, doc) in &all {
            for v in doc["values"].as_array().unwrap() {
                if let Some(t) = v.as_str().unwrap().strip_prefix("#c:") {
                    assert!(all.iter().any(|(p, ..)| *p == t), "{t}");
                }
            }
        }
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
    fn keeps_only_the_fence() {
        assert!(kept("data/create/recipe/a.json"));
        assert!(kept("data/minecraft/tags/block/mineable/pickaxe.json"));
        assert!(!kept("data/create/.wiki/block_info/a.json"));
        assert!(!kept("data/neoforge/data_maps/x.json"));
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
