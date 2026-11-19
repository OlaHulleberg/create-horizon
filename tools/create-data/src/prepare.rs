//! The one-time prepare step. Assemble the mod crate and its committed registry overlays from
//! this repository, then generate what 26.3 needs from Create's All Rights Reserved assets
//! (remaining item definitions, tinted models, connected texture tiles). The pinned
//! jar provides unchanged files through the `[[jar]]` layer; an alternate jar or loose asset
//! folder can supply local assets instead. Assets made from the player's jar are for their own
//! game, never for redistribution. Data conversion belongs to `create-data registry`.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The sources' assets, by path inside the jar; a later source replaces an earlier one.
fn read_sources(sources: &[PathBuf]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut files = BTreeMap::new();
    for source in sources {
        if source.is_dir() {
            let assets = source.join("assets");
            if assets.is_dir() {
                walk(source, &assets, &mut files)?;
            }
        } else {
            let f = fs::File::open(source).map_err(|e| format!("{}: {e}", source.display()))?;
            let mut zip =
                zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", source.display()))?;
            for i in 0..zip.len() {
                let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
                if entry.is_dir() || !entry.name().starts_with("assets/") {
                    continue;
                }
                let name = entry.name().to_string();
                if entry.enclosed_name().is_none() {
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

pub(crate) fn walk(
    root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
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

/// The item models of Create's `CustomRenderedItemModelRenderer`s, as 26.3 item definitions: the
/// item's own model with the partial models its renderer draws over it, and the cardboard
/// sword's held model outside the GUI. The wand's bits stand still. None for an item drawn by
/// its plain model.
fn custom_rendered(name: &str) -> Option<Value> {
    let model =
        |m: &str| json!({ "type": "minecraft:model", "model": format!("create:item/{name}/{m}") });
    let parts: &[&str] = match name {
        // `SymmetryWandItemRenderer`: CORE, CORE_GLOW, BITS.
        "wand_of_symmetry" => &["item", "core", "core_glow", "bits"],
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
    Some(json!({
        "type": "minecraft:composite",
        "models": parts.iter().map(|part| model(part)).collect::<Vec<_>>(),
    }))
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

pub(crate) struct ItemModel {
    target: String,
    pub(crate) custom: Option<Value>,
}

impl ItemModel {
    pub(crate) fn jar_path(&self) -> String {
        format!("assets/create/models/{}.json", self.target)
    }

    pub(crate) fn definition(&self) -> Value {
        json!({ "model": self.custom.clone().unwrap_or_else(|| json!({ "type": "minecraft:model", "model": format!("create:{}", self.target) })) })
    }
}

/// 26.3 looks up `assets/<namespace>/items/<id>.json`, which 1.21.1 did not use.
/// A world fluid adds a bucket item; the hidden partial-block declarations add no items.
pub(crate) fn item_models(
    files: &BTreeMap<String, Vec<u8>>,
    has_asset: impl Fn(&str) -> bool,
) -> BTreeMap<String, ItemModel> {
    let mut models = BTreeMap::new();
    for path in files.keys() {
        let block_or_item = path
            .strip_prefix("data/create/sunrise/block/")
            .or_else(|| path.strip_prefix("data/create/sunrise/item/"));
        let fluid = path.strip_prefix("data/create/sunrise/fluid/");
        let Some(name) = block_or_item
            .or(fluid)
            .and_then(|n| n.strip_suffix(".json"))
        else {
            continue;
        };
        if name.starts_with("part_") {
            continue;
        }
        let name = if fluid.is_some() {
            format!("{name}_bucket")
        } else {
            name.to_owned()
        };
        let kind =
            if fluid.is_some() || has_asset(&format!("assets/create/models/item/{name}.json")) {
                "item"
            } else {
                "block"
            };
        models.insert(
            format!("assets/create/items/{name}.json"),
            ItemModel {
                target: format!("{kind}/{name}"),
                custom: custom_rendered(&name),
            },
        );
    }
    models
}

fn kept_asset(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("assets/") else {
        return false;
    };
    (rest.starts_with("create/blockstates/")
        || rest.starts_with("create/models/")
        || rest.starts_with("create/textures/")
        || rest.starts_with("create/lang/")
        || rest.starts_with("create/sounds/")
        || rest.starts_with("create/particles/")
        || rest == "create/sounds.json")
        && [".json", ".png", ".ogg", ".mcmeta", ".obj", ".mtl"]
            .iter()
            .any(|e| rest.ends_with(e))
}

/// The mod folder's (`create/`) files that make up its crate, besides `src/`, `data/` and
/// `assets/`.
const CRATE_FILES: &[&str] = &["sunrise-mod.toml", "Cargo.toml", "LICENSE", "NOTICE.md"];

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

/// The extendo grip's cog angles within its 45° repeat, and its swing's steps (the glue's
/// `GRIP_COG_STEPS`, `GRIP_SWING_STEPS`).
const GRIP_COG_STEPS: usize = 10;
const GRIP_SWING_STEPS: usize = 8;

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
    crate_files.extend(files_of(repo, &["README.md"])?);
    for (rel, bytes) in &crate_files {
        put(rel, bytes)?;
    }
    let mut ours = BTreeMap::new();
    walk(&mod_dir, &mod_dir.join("data"), &mut ours)?;
    walk(&mod_dir, &mod_dir.join("assets"), &mut ours)?;
    for (rel, bytes) in &ours {
        put(rel, bytes)?;
    }
    let models = item_models(&ours, |path| files.contains_key(path));
    let model_count = models.len();
    let mut copied = 0;
    for (path, bytes) in &files {
        // Our own files (a blockstate we draw differently) stand over Create's.
        if !kept_asset(path) || ours.contains_key(path) {
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
        if let Some(door) = sliding_door_states(path, bytes) {
            put(path, &serde_json::to_vec_pretty(&door).unwrap())?;
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
    // The committed registry definitions are already in `ours`; only make the rest locally.
    for (path, model) in models {
        if !ours.contains_key(&path) {
            put(&path, &crate::pretty(&model.definition()))?;
        }
    }
    // The placement helpers' ghosts, baked from the jar's models (`ghosts.rs`).
    let (ghost_models, ghosts) = crate::ghosts::source_file(&files)?;
    put(crate::ghosts::RS, ghost_models.as_bytes())?;
    let _ = fs::remove_dir_all(out);
    fs::rename(&tmp, out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut report = Map::new();
    report.insert("out".into(), json!(out.display().to_string()));
    report.insert("item_definitions".into(), json!(model_count));
    let from_jar = from_jar.borrow().len();
    report.insert(
        "files_copied".into(),
        json!((copied as usize).saturating_sub(from_jar)),
    );
    report.insert("files_left_to_the_jar_layer".into(), json!(from_jar));
    report.insert("connected_texture_files".into(), json!(ct_files.len()));
    report.insert("connected_textures_left_out".into(), json!(ct_left_out));
    report.insert("placement_ghosts".into(), json!(ghosts));
    Ok(serde_json::to_string_pretty(&Value::Object(report)).unwrap())
}

#[cfg(test)]
mod tests {

    use super::*;

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
    fn keeps_only_local_asset_types() {
        assert!(kept_asset("assets/create/textures/block/a.png"));
        assert!(!kept_asset("assets/create/ponder/a.nbt"));
        assert!(!kept_asset("assets/create/lang/README.md"));
        assert!(kept_asset(
            "assets/create/models/block/water_wheel/water_wheel.obj"
        ));
        assert!(kept_asset(
            "assets/create/models/block/water_wheel/water_wheel.mtl"
        ));
    }
}

/// What the repository may hold under `create/assets`: generated block states, models, simple
/// item definitions and atlas references, the port's shader and Catnip's MIT placement indicator. Everything under Create's `src/main/resources/assets/` is All Rights
/// Reserved: textures, models, particles, sounds and translations of Create's come from the
/// player's jar in `prepare`, never from a committed file. Where Create's jar is at hand
/// (`$CREATE_JAR`, else the `prepare` download cache), no committed file under `create/assets`
/// or `create/data` may be one of the jar's assets, byte for byte or as JSON (a file that holds
/// all of a jar asset's JSON at its own path, with keys added, counts as a copy).
#[cfg(test)]
mod committed_assets {
    use crate::registry::visuals::{SPINNING_TOOLS, spinner_definition};
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
    fn only_approved_asset_types_are_committed() {
        let root = repo().join("create/assets");
        let mut all = Vec::new();
        files(&root, &root, &mut all);
        let refused: Vec<&String> = all
            .iter()
            .filter(|f| {
                if let Some(name) = f.strip_prefix("create/items/").and_then(|name| name.strip_suffix(".json")) {
                    let Ok(bytes) = std::fs::read(root.join(f)) else { return true };
                    let Ok(doc) = serde_json::from_slice::<Value>(&bytes) else { return true };
                    if SPINNING_TOOLS.iter().any(|(item, _)| *item == name) {
                        return doc != spinner_definition(name);
                    }
                    return !["block", "item"].into_iter().any(|kind| {
                        doc == serde_json::json!({ "model": { "type": "minecraft:model", "model": format!("create:{kind}/{name}") } })
                    });
                }
                !(f.starts_with("create/blockstates/") && f.ends_with(".json")
                    || f.starts_with("create/models/block/") && f.ends_with(".json")
                    || matches!(f.as_str(), "create/textures/gui/placement_indicator.png"
                        | "create/shaders/pipeline/textured.wgsl" | "minecraft/atlases/blocks.json"))
            })
            .collect();
        assert!(
            refused.is_empty(),
            "unapproved asset type; Create's protected assets come from the player's jar: {refused:?}"
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
