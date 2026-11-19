//! Deterministic Sunrise 26.3 overlays from Create's pinned, MIT-licensed data.

pub(crate) mod visuals;

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

/// 1.21.1 ingredients (`{"item": x}`, `{"tag": t}`, lists of them) as 26.3 writes them (`"x"`,
/// `"#t"`, lists of strings). Anything else is left as it is.
fn ingredient(v: &Value) -> Value {
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
fn convert_recipe(mut doc: Value) -> Option<Value> {
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
/// `GearboxBlock.getDrops`: a gearbox on a horizontal axis drops one vertical gearbox, whatever
/// the loot table says (no explosion decay); an upright one keeps the table's drop.
fn gearbox_drops(doc: &mut Value) {
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

/// A 1.21.1 loot table as 26.3 writes it, the form Horizon reads: a condition names its kind in
/// `type` (1.21.1: `condition`), a function in `type` (1.21.1: `function`), and a pool's or
/// entry's functions are its `modifier` list (1.21.1: `functions`).
fn convert_loot(v: &mut Value) {
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
/// The data 26.3 reads, outside loadable Create data that needs an omission overlay.
fn kept(path: &str) -> bool {
    path.ends_with(".json")
        && !path.contains("/.")
        && [
            "data/create/recipe/",
            "data/create/loot_table/",
            "data/create/tags/",
            "data/minecraft/tags/",
            "data/c/tags/",
            "data/create/worldgen/",
            "data/create/neoforge/biome_modifier/",
            "data/create/enchantment/",
        ]
        .iter()
        .any(|prefix| path.starts_with(prefix))
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

/// The mod's own file that keeps a data file of the jar layer from loading: a data map removes
/// the jar's entries (layers of data maps merge, and the host reads a data map's `remove` list),
/// any other file fails a load condition (the top layer's file is the one read): Create's own
/// condition form, `neoforge:mod_loaded`, naming a mod id no mod has. `None` where the mod cannot: tags merge and have no removal in
/// 26.3, and a mod's files stay in its own namespace (the jar's `data/neoforge/data_maps` load).
fn left_out(path: &str, bytes: &[u8]) -> Option<Value> {
    // Create's damage types load as the jar has them: `EntityRef::hurt` takes a data-driven type.
    if !path.starts_with("data/create/")
        || path.contains("/tags/")
        || path.starts_with("data/create/damage_type/")
    {
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

// The tag points to ac0c444d9828da3453ae8cc65338e8de063286fb, matching the pinned 6.0.10 jar.
// Pin the src/generated subtree rather than walking src/main/resources/assets (All Rights Reserved).
const GENERATED_TREE: &str = "a537ebce356393ce32d5fbcf209c5c447a53683f";
// SHA-256 of the sorted `resources/` blob paths (relative to resources), each followed by
// a NUL, its Git blob id and a newline. A saved tree is the same input as the API response.
const GENERATED_BLOBS: &str = "8bd3ff1825175b5fdacccfe70a92cbf49287014bdf67c420cb1ec2447d46a996";

fn generated_tree(local: Option<&Path>) -> Result<BTreeMap<String, String>, String> {
    let input: Box<dyn Read> = if let Some(path) = local {
        Box::new(fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?)
    } else {
        let url = format!(
            "https://api.github.com/repos/Creators-of-Create/Create/git/trees/{GENERATED_TREE}?recursive=1"
        );
        // Send the CI credential only to the pinned GitHub API URL, never to a redirect.
        let agent = ureq::AgentBuilder::new()
            .user_agent("create-data")
            .redirects(0)
            .build();
        let mut request = agent.get(&url);
        if let Ok(token) = std::env::var("GITHUB_TOKEN")
            && !token.is_empty()
        {
            request = request.set("Authorization", &format!("Bearer {token}"));
        }
        request
            .call()
            .map_err(|e| format!("{url}: {e}"))?
            .into_reader()
    };
    let tree: Value = serde_json::from_reader(input).map_err(|e| format!("MIT tree: {e}"))?;
    if tree["sha"] != GENERATED_TREE || tree["truncated"] != false {
        return Err("expected the complete pinned Create src/generated tree".into());
    }
    let entries = tree["tree"].as_array().ok_or("missing MIT tree entries")?;
    let blobs: BTreeMap<String, String> = entries
        .iter()
        .filter(|entry| entry["type"] == "blob")
        .filter_map(|entry| {
            let path = entry["path"].as_str()?.strip_prefix("resources/")?;
            Some((path.to_owned(), entry["sha"].as_str()?.to_owned()))
        })
        .collect();
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for (path, sha) in &blobs {
        digest.update(path);
        digest.update(b"\0");
        digest.update(sha);
        digest.update(b"\n");
    }
    if format!("{:x}", digest.finalize()) != GENERATED_BLOBS {
        return Err("MIT tree blob list differs from the pinned source".into());
    }
    Ok(blobs)
}

fn open_jar(path: &Path) -> Result<zip::ZipArchive<fs::File>, String> {
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", path.display()))
}

fn jar_bytes(jar: &mut zip::ZipArchive<fs::File>, path: &str) -> Result<Vec<u8>, String> {
    let mut entry = jar.by_name(path).map_err(|e| format!("{path}: {e}"))?;
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{path}: {e}"))?;
    Ok(bytes)
}

fn read_data(jar: &mut zip::ZipArchive<fs::File>) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let paths: Vec<String> = jar
        .file_names()
        .filter(|p| {
            p.ends_with(".json")
                && !p.contains("/.")
                && (p.starts_with("data/create/")
                    || p.starts_with("data/minecraft/tags/")
                    || p.starts_with("data/c/tags/"))
        })
        .map(str::to_owned)
        .collect();
    paths
        .into_iter()
        .map(|p| Ok((p.clone(), jar_bytes(jar, &p)?)))
        .collect()
}

fn git_blob(bytes: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut digest = Sha1::new();
    digest.update(format!("blob {}\0", bytes.len()));
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

/// `compile` owns the startup declarations and top-level advancements. `registry` owns all
/// other Create data JSON, its additions to vanilla and convention tags, and simple item definitions.
fn compile_advancement(path: &str) -> bool {
    path.strip_prefix("data/create/advancement/")
        .is_some_and(|name| !name.contains('/') && name.ends_with(".json"))
}

const MANDATORY_NOTICES: &[(&str, &[u8])] = &[
    ("LICENSE", include_bytes!("../../../LICENSE")),
    (
        "NOTICE.md",
        include_bytes!("../../../THIRD_PARTY_NOTICES.md"),
    ),
];

fn owned(path: &str) -> bool {
    (path.starts_with("data/create/")
        && !path.starts_with("data/create/sunrise/")
        && !compile_advancement(path))
        || path.starts_with("data/minecraft/tags/")
        || path.starts_with("data/c/tags/")
        || path.starts_with("assets/create/items/")
        || visuals::owns(path)
}

fn data_overlays(files: &BTreeMap<String, Vec<u8>>) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let unplaced = unplaced_features(files);
    let mut out = BTreeMap::new();
    for (path, bytes) in files {
        if !owned(path) || !kept(path) || path.contains("/tags/") {
            continue;
        }
        let mut doc: Value = serde_json::from_slice(bytes).map_err(|e| format!("{path}: {e}"))?;
        if path.starts_with("data/create/recipe/") {
            let Some(converted) = convert_recipe(doc) else {
                continue;
            };
            doc = converted;
        } else if path.starts_with("data/create/worldgen/")
            || path.starts_with("data/create/neoforge/biome_modifier/")
        {
            if !convert_worldgen(path, &mut doc, &unplaced) {
                continue;
            }
        } else {
            // Loot tables and enchantments use the same condition codec.
            convert_loot(&mut doc);
            if path == "data/create/loot_table/blocks/gearbox.json" {
                gearbox_drops(&mut doc);
            }
        }
        // 26.3's directory for configured features is `feature`.
        let dest = path.replace("/worldgen/configured_feature/", "/worldgen/feature/");
        out.insert(dest, crate::pretty(&doc));
    }
    // Omission overlays also cover source paths renamed above, but leave the mergeable tags
    // and compile-owned advancements alone. The jar still supplies unchanged files.
    for (path, bytes) in files {
        if owned(path)
            && !out.contains_key(path)
            && let Some(doc) = left_out(path, bytes)
        {
            out.insert(path.clone(), crate::pretty(&doc));
        }
    }
    for (path, doc) in c_tag_files(files) {
        out.insert(path, crate::pretty(&doc));
    }
    Ok(out)
}

pub fn generate(
    mod_dir: &Path,
    jar_path: &Path,
    tree_path: Option<&Path>,
    audit_path: Option<&Path>,
    check: bool,
) -> Result<(), String> {
    let pin = &crate::jar::CREATE;
    let sha =
        crate::jar::sha512_of(jar_path).map_err(|e| format!("{}: {e}", jar_path.display()))?;
    if sha != pin.sha512 {
        return Err(format!(
            "{}: expected pinned Create {} (SHA-512 {})",
            jar_path.display(),
            pin.version,
            pin.sha512
        ));
    }
    let tree = generated_tree(tree_path)?;
    let mut jar = open_jar(jar_path)?;
    let data = read_data(&mut jar)?;
    for (path, bytes) in &data {
        if tree.get(path) != Some(&git_blob(bytes)) {
            return Err(format!(
                "{path}: the pinned jar entry does not match an MIT source blob"
            ));
        }
    }
    let mut output = data_overlays(&data)?;
    let data_count = output.len();

    let visual_assets = visuals::assets(|path| jar.index_for_name(path).is_some())?;
    let visual_count = visual_assets.len();
    let mut declarations = BTreeMap::new();
    crate::prepare::walk(
        mod_dir,
        &mod_dir.join("data/create/sunrise"),
        &mut declarations,
    )?;
    let models = crate::prepare::item_models(&declarations, |p| jar.index_for_name(p).is_some());
    let mut audit = String::from(
        "# Create registry item-definition licence audit\n\nPinned jar: Create 6.0.10 for Minecraft 1.21.1, Modrinth `UjX6dr61`. Source: `mc1.21.1-6.0.10` (`ac0c444d9828da3453ae8cc65338e8de063286fb`), MIT `src/generated` tree `a537ebce356393ce32d5fbcf209c5c447a53683f`. Create's `LICENSE.md` excludes `src/main/resources/assets/`, which is not used. Simple `minecraft:model` wrappers name only a model id, not model geometry or textures. For these wrappers the pinned jar model must be byte-identical to a Git blob in the generated MIT tree. The port's three special tool models name original parts by id. Dependent art remains a runtime reference in both cases. The remaining custom definitions, tinted models and connected texture tiles remain local to `prepare`.\n\n| Definition | Decision | Evidence or reason |\n|---|---|---|\n",
    );
    use std::fmt::Write as _;
    for (path, model) in models {
        let target = model.jar_path();
        let blob = tree.get(&target);
        let (decision, reason) = if visual_assets.contains_key(&path) {
            (
                "In",
                "Port-authored special model; only references original part IDs".to_owned(),
            )
        } else if model.custom.is_some() {
            (
                "Out",
                "Custom rendering needs local asset preparation".to_owned(),
            )
        } else if jar.index_for_name(&target).is_none() {
            ("Out", format!("No `{target}` in the pinned jar"))
        } else if let Some(blob) = blob {
            if git_blob(&jar_bytes(&mut jar, &target)?) == *blob {
                output.insert(path.clone(), crate::pretty(&model.definition()));
                ("In", format!("`{target}` matches MIT blob `{blob}`"))
            } else {
                ("Out", format!("`{target}` differs from MIT blob `{blob}`"))
            }
        } else {
            ("Out", format!("`{target}` not in the MIT generated tree"))
        };
        writeln!(audit, "| `{path}` | {decision} | {reason} |").unwrap();
    }
    if let Some(path) = audit_path {
        fs::write(path, audit).map_err(|e| format!("{}: {e}", path.display()))?;
    }

    let item_count = output.len() - data_count;
    output.extend(visual_assets);

    let mut current = BTreeMap::new();
    for tree in ["data", "assets"] {
        let dir = mod_dir.join(tree);
        if dir.exists() {
            crate::prepare::walk(mod_dir, &dir, &mut current)?;
        }
    }
    current.retain(|path, _| owned(path));
    for &(path, bytes) in MANDATORY_NOTICES {
        output.insert(path.into(), bytes.to_vec());
        match fs::read(mod_dir.join(path)) {
            Ok(bytes) => {
                current.insert(path.into(), bytes);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{path}: {e}")),
        }
    }
    for stale in current.keys().filter(|path| !output.contains_key(*path)) {
        if check {
            return Err(format!(
                "stale registry output: {stale} (rerun create-data registry)"
            ));
        }
        fs::remove_file(mod_dir.join(stale)).map_err(|e| format!("{stale}: {e}"))?;
    }
    for (path, bytes) in output {
        if current.get(&path) == Some(&bytes) {
            continue;
        }
        if check {
            return Err(format!(
                "missing or changed registry output: {path} (rerun create-data registry)"
            ));
        }
        let file = mod_dir.join(path);
        fs::create_dir_all(file.parent().unwrap())
            .map_err(|e| format!("{}: {e}", file.display()))?;
        fs::write(&file, bytes).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    println!(
        "{data_count} data overlays, {item_count} verified item definitions and {visual_count} visual assets"
    );
    Ok(())
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn registry_overlays_do_not_replace_compile_outputs_or_unchanged_tags() {
        let files = BTreeMap::from([
            ("data/create/advancement/root.json".into(), b"{}".to_vec()),
            (
                "data/create/advancement/recipes/chain.json".into(),
                b"{}".to_vec(),
            ),
            (
                "data/create/sunrise/block/shaft.json".into(),
                b"{}".to_vec(),
            ),
            ("data/create/tags/item/raw.json".into(), b"{}".to_vec()),
            (
                "data/create/recipe/shaft.json".into(),
                br#"{ "ingredient": { "item": "minecraft:chain" } }"#.to_vec(),
            ),
        ]);
        let overlays = data_overlays(&files).unwrap();
        assert_eq!(overlays.len(), 2);
        let recipe: Value =
            serde_json::from_slice(&overlays["data/create/recipe/shaft.json"]).unwrap();
        assert_eq!(recipe["ingredient"], "minecraft:iron_chain");
        let unlock: Value =
            serde_json::from_slice(&overlays["data/create/advancement/recipes/chain.json"])
                .unwrap();
        assert_eq!(unlock["neoforge:conditions"][0]["modid"], NO_MOD);
    }

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
        assert!(left_out("data/create/damage_type/crush.json", b"{}").is_none());
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
    use std::path::PathBuf;

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
        let files = read_data(&mut open_jar(&jar).unwrap()).unwrap();
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
