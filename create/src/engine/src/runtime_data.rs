//! Create's `RuntimeDataGenerator`: the recipes it makes from the item registry when the game
//! loads, which Create's data files leave out: a saw's wood cutting (a log or wood to its stripped
//! form, a stripped one to planks, planks to their stairs, slabs, fences, doors and the like) and
//! a fan's washing of concrete powder into concrete.

use crate::processing::{Output, Recipe};
use std::collections::{BTreeMap, HashSet};

/// The wood kinds `STRIPPED_WOODS_PREFIX_REGEX` and its siblings end in.
const TYPES: [&str; 5] = ["_log", "_wood", "_stem", "_hyphae", "_block"];

/// `\w`: an ASCII letter, digit or underscore.
fn word(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `(\w*)(_log|_wood|_stem|_hyphae|_block|(?<!_)wood)` matching all of `s`, the first group
/// greedy: the name and the kind.
fn name_and_type(s: &str) -> Option<(&str, &str)> {
    if !word(s) {
        return None;
    }
    // The greedy name takes all it can: the shortest kind that ends `s`.
    if let Some(name) = s.strip_suffix("wood")
        && !name.is_empty()
        && !name.ends_with('_')
    {
        return Some((name, "wood"));
    }
    TYPES
        .iter()
        .find_map(|t| s.strip_suffix(t).map(|name| (name, *t)))
}

/// One cutting recipe of `RuntimeDataGenerator.simpleWoodRecipe` (50 ticks).
fn wood_recipe(from_ns: &str, from: &str, input: Vec<String>, out: &str, count: u8) -> Recipe {
    let to = out.split_once(':').map_or(out, |(_, p)| p);
    Recipe {
        id: format!("create:cutting/runtime_generated/compat/{from_ns}/{from}_to_{to}"),
        ingredients: vec![input],
        results: vec![Output {
            item: out.to_string(),
            count,
            chance: 1.0,
        }],
        time: 50,
    }
}

/// `RuntimeDataGenerator.cuttingRecipes` over every item id (`namespace:path`, in registry
/// order): the saw's wood recipes, one per input and output (a later one of the same id
/// replaces an earlier one, as the generated files do).
pub fn cutting(items: &[String]) -> Vec<Recipe> {
    let known: HashSet<&str> = items.iter().map(String::as_str).collect();
    let has = |id: &str| known.contains(id);
    let mut out: BTreeMap<String, Recipe> = BTreeMap::new();
    // Items with no stripped form, by the tag `runtime_generated/compat/<ns>/<base>` they go in.
    let mut tagged: BTreeMap<(String, String), (Vec<String>, String, u8)> = BTreeMap::new();
    let mut add = |r: Recipe| {
        out.insert(r.id.clone(), r);
    };
    for id in items {
        let Some((ns, path)) = id.split_once(':') else {
            continue;
        };
        // (prefix, wood name, kind, suffix), as the three patterns read the path.
        let mut matched: Option<(String, String, String, String)> = None;
        let mut stripped_in_prefix = false;
        let mut no_stripped_variant = false;
        // STRIPPED_WOODS_PREFIX_REGEX: `(\w*)??stripped_(\w*)(kind)()$`, found anywhere.
        if word(path) {
            matched = path.match_indices("stripped_").find_map(|(i, _)| {
                let (name, t) = name_and_type(&path[i + "stripped_".len()..])?;
                Some((
                    path[..i].to_string(),
                    name.to_string(),
                    t.to_string(),
                    String::new(),
                ))
            });
            stripped_in_prefix = matched.is_some();
        }
        // STRIPPED_WOOD_SUFFIX_REGEX: `(\w*)(kind)(\w*)_stripped(\w*)`, the groups greedy.
        if matched.is_none() && word(path) {
            matched = path.rmatch_indices("_stripped").find_map(|(i, _)| {
                let before = &path[..i];
                let after = &path[i + "_stripped".len()..];
                // The greedy name ends at the last kind the part before `_stripped` has.
                let (at, t) = TYPES
                    .iter()
                    .filter_map(|t| before.rfind(t).map(|at| (at, *t)))
                    .chain(
                        before
                            .rfind("wood")
                            .filter(|&at| at > 0 && !before[..at].ends_with('_'))
                            .map(|at| (at, "wood")),
                    )
                    .max_by_key(|(at, t)| (*at, t.len()))?;
                let name = &before[..at];
                let suffix = format!("{}{after}", &before[at + t.len()..]);
                Some((String::new(), name.to_string(), t.to_string(), suffix))
            });
        }
        // NON_STRIPPED_WOODS_REGEX, for an item with no stripped form:
        // `^(?!stripped_)([a-z_]+)(kind but bioshroom_block)(([a-z_]+)(?<!_stripped))?$`.
        if matched.is_none()
            && !has(&format!("{ns}:stripped_{path}"))
            && !has(&format!("{ns}:{path}_stripped"))
            && !path.starts_with("stripped_")
            && path.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        {
            matched = TYPES
                .iter()
                .filter_map(|t| path.rfind(t).map(|at| (at, *t)))
                .filter(|(at, t)| {
                    *at > 0
                        && !path[at + t.len()..].ends_with("_stripped")
                        && !(*t == "_block" && path[..*at].ends_with("bioshroom"))
                })
                .max_by_key(|(at, _)| *at)
                .map(|(at, t)| {
                    let name = path[..at].to_string();
                    (String::new(), name, t.to_string(), String::new())
                });
            no_stripped_variant = matched.is_some();
        }
        let Some((prefix, name, kind, suffix)) = matched else {
            continue;
        };
        let prefix = if stripped_in_prefix {
            prefix
        } else {
            String::new()
        };
        let suffix = if !stripped_in_prefix && !no_stripped_variant {
            suffix
        } else {
            String::new()
        };
        // Botania's livingwood keeps its `wood`; Ars Nouveau and Deeper Darker name their
        // planks after another wood (`MISMATCHED_WOOD_NAMES`).
        let mut base = format!("{ns}:{name}{}", if kind == "wood" { "wood" } else { "" });
        base = match base.as_str() {
            "ars_nouveau:blue_archwood" => "ars_nouveau:archwood".into(),
            "deeperdarker:blooming" => "deeperdarker:bloom".into(),
            _ => base,
        };
        let non_stripped = format!("{ns}:{prefix}{name}{kind}{suffix}");
        let planks = format!("{base}_planks");
        let (base_ns, base_path) = base.split_once(':').unwrap_or(("minecraft", &base));
        let planks_in_name = matches!(base_ns, "quark" | "botania");
        let planks_count = if kind.contains("block") { 3 } else { 6 };
        if !no_stripped_variant {
            if has(&non_stripped) {
                let from = non_stripped.split_once(':').map_or("", |(_, p)| p);
                add(wood_recipe(ns, from, vec![non_stripped.clone()], id, 1));
            }
            if has(&planks) {
                add(wood_recipe(
                    ns,
                    path,
                    vec![id.clone()],
                    &planks,
                    planks_count,
                ));
            }
        } else if has(&planks) {
            let key = (base_ns.to_string(), base_path.to_string());
            let e = tagged
                .entry(key)
                .or_insert_with(|| (Vec::new(), planks.clone(), planks_count));
            e.0.push(id.clone());
        }
        if !path.contains("_wood") && !path.contains("_hyphae") && has(&planks) {
            let from = planks.split_once(':').map_or("", |(_, p)| p);
            let pns = planks.split_once(':').map_or("", |(n, _)| n);
            let stairs = if planks_in_name {
                "_planks_stairs"
            } else {
                "_stairs"
            };
            let slab = if planks_in_name {
                "_planks_slab"
            } else {
                "_slab"
            };
            for (to, count) in [
                (stairs, 1),
                (slab, 2),
                ("_fence", 1),
                ("_fence_gate", 1),
                ("_door", 1),
                ("_trapdoor", 1),
                ("_pressure_plate", 1),
                ("_button", 1),
                ("_sign", 1),
            ] {
                let made = format!("{base}{to}");
                if has(&made) {
                    add(wood_recipe(pns, from, vec![planks.clone()], &made, count));
                }
            }
        }
    }
    // `simpleWoodRecipe(TagKey, ..)`: a tag's items to planks, the tag in Create's namespace.
    for ((ns, base), (input, planks, count)) in tagged {
        let tag = format!("tag_runtime_generated/compat/{ns}/{base}");
        add(wood_recipe("create", &tag, input, &planks, count));
    }
    out.into_values().collect()
}

/// `RuntimeDataGenerator.washingRecipes`: each concrete powder washes into its concrete
/// (`ConcretePowderBlock.concrete`, which vanilla names after the powder).
pub fn splashing(items: &[String]) -> Vec<Recipe> {
    let known: HashSet<&str> = items.iter().map(String::as_str).collect();
    items
        .iter()
        .filter_map(|id| {
            let concrete = id.strip_suffix("_concrete_powder")?.to_string() + "_concrete";
            if !known.contains(concrete.as_str()) {
                return None;
            }
            let (ns, from) = id.split_once(':')?;
            let to = concrete.split_once(':')?.1;
            Some(Recipe {
                id: format!("create:splashing/runtime_generated/compat/{ns}/{from}_to_{to}"),
                ingredients: vec![vec![id.clone()]],
                results: vec![Output {
                    item: concrete.clone(),
                    count: 1,
                    chance: 1.0,
                }],
                time: 0,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| format!("minecraft:{s}")).collect()
    }

    fn made(rs: &[Recipe], from: &str) -> Vec<(String, u8)> {
        let mut v: Vec<_> = rs
            .iter()
            .filter(|r| {
                r.ingredients[0]
                    .iter()
                    .any(|i| i == &format!("minecraft:{from}"))
            })
            .map(|r| (r.results[0].item.clone(), r.results[0].count))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn logs_strip_then_make_planks_and_planks_make_their_blocks() {
        let items = ids(&[
            "oak_log",
            "stripped_oak_log",
            "oak_wood",
            "stripped_oak_wood",
            "oak_planks",
            "oak_stairs",
            "oak_slab",
            "oak_fence",
            "oak_door",
            "oak_sign",
            "bamboo_block",
            "stripped_bamboo_block",
            "bamboo_planks",
            "iron_block",
        ]);
        let rs = cutting(&items);
        assert_eq!(
            made(&rs, "oak_log"),
            [("minecraft:stripped_oak_log".into(), 1)]
        );
        assert_eq!(
            made(&rs, "oak_wood"),
            [("minecraft:stripped_oak_wood".into(), 1)]
        );
        assert_eq!(
            made(&rs, "stripped_oak_log"),
            [("minecraft:oak_planks".into(), 6)]
        );
        assert_eq!(
            made(&rs, "stripped_oak_wood"),
            [("minecraft:oak_planks".into(), 6)]
        );
        assert_eq!(
            made(&rs, "stripped_bamboo_block"),
            [("minecraft:bamboo_planks".into(), 3)]
        );
        assert_eq!(
            made(&rs, "oak_planks"),
            [
                ("minecraft:oak_door".into(), 1),
                ("minecraft:oak_fence".into(), 1),
                ("minecraft:oak_sign".into(), 1),
                ("minecraft:oak_slab".into(), 2),
                ("minecraft:oak_stairs".into(), 1),
            ]
        );
        assert!(made(&rs, "iron_block").is_empty(), "no planks, no recipe");
        let r = rs
            .iter()
            .find(|r| r.results[0].item == "minecraft:stripped_oak_log")
            .unwrap();
        assert_eq!(
            r.id,
            "create:cutting/runtime_generated/compat/minecraft/oak_log_to_stripped_oak_log"
        );
        assert_eq!(r.time, 50);
    }

    #[test]
    fn concrete_powder_washes_into_concrete() {
        let rs = splashing(&ids(&["red_concrete_powder", "red_concrete", "sand"]));
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].results[0].item, "minecraft:red_concrete");
    }
}
