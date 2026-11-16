//! The common (`c:`) item tags Create's recipes and tags name. Create's own data defines some of
//! them (zinc, brass, plates and the rest, under `data/c/tags/item/`); the others come from the
//! platform Create runs on, and a vanilla game has none. This port gives each `c:` item tag
//! Create's data names the vanilla 26.3 items its name points to, by a rule of its own: a
//! `c:<kind>/<material>` tag holds vanilla's `<material>` item of that kind
//! (`c:ingots/iron` is `minecraft:iron_ingot`, `c:storage_blocks/raw_gold` is
//! `minecraft:raw_gold_block`, `c:dyes/red` is `minecraft:red_dye`), and the names that rule
//! doesn't fit are listed by hand ([`irregular`]), each read from what the recipes using it ask
//! for. A tag Create defines keeps its own members and gains vanilla's (`c:glass_blocks/colorless`
//! holds Create's framed glass and vanilla's glass). A tag the rule gives nothing for (another
//! mod's metals, fluid tags) is left alone: the recipes naming them are for other mods.

/// The vanilla metals with ingots, nuggets, raw forms and storage blocks.
const INGOTS: [&str; 4] = ["iron", "gold", "copper", "netherite"];
const NUGGETS: [&str; 3] = ["iron", "gold", "copper"];
const RAW: [&str; 3] = ["iron", "gold", "copper"];

/// Vanilla's sixteen dye colours.
pub const COLOURS: [&str; 16] = [
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

/// Vanilla's overworld woods with logs and woods (`<wood>_log`, `<wood>_wood`), the nether's
/// stems (`<stem>_stem`, `<stem>_hyphae`).
const WOODS: [&str; 10] = [
    "oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "pale_oak",
    "poplar",
];
const STEMS: [&str; 2] = ["crimson", "warped"];

fn mc(id: &str) -> String {
    format!("minecraft:{id}")
}

/// The vanilla members (item ids, or `#tag`s) of `c:<path>`, by the rule or the hand-written
/// list; empty for a tag the port gives nothing.
pub fn members(path: &str) -> Vec<String> {
    if let Some(list) = irregular(path) {
        return list;
    }
    let Some((kind, material)) = path.split_once('/') else {
        return Vec::new();
    };
    let one = |ok: bool, id: String| if ok { vec![mc(&id)] } else { Vec::new() };
    match kind {
        "ingots" => one(INGOTS.contains(&material), format!("{material}_ingot")),
        "nuggets" => one(NUGGETS.contains(&material), format!("{material}_nugget")),
        "raw_materials" => one(RAW.contains(&material), format!("raw_{material}")),
        "storage_blocks" => match material.strip_prefix("raw_") {
            Some(m) => one(RAW.contains(&m), format!("raw_{m}_block")),
            None => one(INGOTS.contains(&material), format!("{material}_block")),
        },
        "dyes" => one(COLOURS.contains(&material), format!("{material}_dye")),
        _ => Vec::new(),
    }
}

/// The names the rule doesn't fit.
fn irregular(path: &str) -> Option<Vec<String>> {
    let ids = |ids: &[&str]| ids.iter().map(|i| mc(i)).collect::<Vec<_>>();
    Some(match path {
        // Plural names of a single vanilla item.
        "ender_pearls" => ids(&["ender_pearl"]),
        "feathers" => ids(&["feather"]),
        "gunpowders" => ids(&["gunpowder"]),
        "leathers" => ids(&["leather"]),
        "netherracks" => ids(&["netherrack"]),
        "obsidians" => ids(&["obsidian"]),
        "slimeballs" => ids(&["slime_ball"]),
        "strings" => ids(&["string"]),
        // Vanilla's own tag of its eggs.
        "eggs" => vec!["#minecraft:eggs".into()],
        "gems/quartz" => ids(&["quartz"]),
        "gems/lapis" => ids(&["lapis_lazuli"]),
        "dusts/redstone" => ids(&["redstone"]),
        "rods/wooden" => ids(&["stick"]),
        "sands/colorless" => ids(&["sand"]),
        "sands/red" => ids(&["red_sand"]),
        "barrels" | "barrels/wooden" => ids(&["barrel"]),
        "chests/wooden" => ids(&["chest", "trapped_chest"]),
        "player_workstations/crafting_tables" => ids(&["crafting_table"]),
        // Cobblestone and its deepslate kind (lava from cobble, blackstone by haunting).
        "cobblestones" => ids(&["cobblestone", "cobbled_deepslate"]),
        // The overworld's base stones, the members of vanilla's block tag
        // `minecraft:base_stone_overworld` (an item tag can't name a block tag).
        "stones" => ids(&[
            "stone",
            "granite",
            "diorite",
            "andesite",
            "tuff",
            "deepslate",
        ]),
        "glass_blocks/colorless" => ids(&["glass"]),
        // Every glass block: the colourless ones (Create's and vanilla's), stained and tinted.
        "glass_blocks" => std::iter::once("#c:glass_blocks/colorless".to_owned())
            .chain(COLOURS.iter().map(|c| mc(&format!("{c}_stained_glass"))))
            .chain([mc("tinted_glass")])
            .collect(),
        "stripped_logs" => WOODS
            .iter()
            .map(|w| mc(&format!("stripped_{w}_log")))
            .chain(STEMS.iter().map(|s| mc(&format!("stripped_{s}_stem"))))
            .chain([mc("stripped_bamboo_block")])
            .collect(),
        "stripped_woods" => WOODS
            .iter()
            .map(|w| mc(&format!("stripped_{w}_wood")))
            .chain(STEMS.iter().map(|s| mc(&format!("stripped_{s}_hyphae"))))
            .collect(),
        _ => return None,
    })
}

/// The `c:` tags a data file names: `#c:<path>` strings and `{"tag": "c:<path>"}` objects.
pub fn named(doc: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
    use serde_json::Value;
    match doc {
        Value::String(s) => {
            if let Some(p) = s.strip_prefix("#c:") {
                out.insert(p.to_owned());
            }
        }
        Value::Array(a) => a.iter().for_each(|v| named(v, out)),
        Value::Object(o) => {
            if let Some(p) = o
                .get("tag")
                .and_then(Value::as_str)
                .and_then(|t| t.strip_prefix("c:"))
            {
                out.insert(p.to_owned());
            }
            o.values().for_each(|v| named(v, out));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_and_material_is_vanilla_s_item_of_that_kind() {
        assert_eq!(members("ingots/iron"), ["minecraft:iron_ingot"]);
        assert_eq!(members("nuggets/copper"), ["minecraft:copper_nugget"]);
        assert_eq!(members("raw_materials/gold"), ["minecraft:raw_gold"]);
        assert_eq!(
            members("storage_blocks/raw_iron"),
            ["minecraft:raw_iron_block"]
        );
        assert_eq!(members("storage_blocks/copper"), ["minecraft:copper_block"]);
        assert_eq!(members("dyes/light_blue"), ["minecraft:light_blue_dye"]);
        // Another mod's metal, a fluid tag: nothing.
        assert!(members("ingots/tin").is_empty());
        assert!(members("storage_blocks/raw_lead").is_empty());
        assert!(members("milk").is_empty());
    }

    #[test]
    fn irregular_names_and_nesting() {
        assert_eq!(members("slimeballs"), ["minecraft:slime_ball"]);
        assert_eq!(members("rods/wooden"), ["minecraft:stick"]);
        let glass = members("glass_blocks");
        assert_eq!(glass[0], "#c:glass_blocks/colorless");
        assert_eq!(glass.len(), 1 + 16 + 1);
        assert_eq!(
            members("stripped_logs").len(),
            WOODS.len() + STEMS.len() + 1
        );
        // A `c:` tag a member names is given by the rule too.
        for p in ["glass_blocks", "eggs", "stones", "stripped_woods"] {
            for m in members(p) {
                if let Some(t) = m.strip_prefix("#c:") {
                    assert!(!members(t).is_empty(), "{t}");
                }
            }
        }
    }

    #[test]
    fn finds_the_tags_a_file_names() {
        let doc = serde_json::json!({"key": {"A": {"tag": "c:ingots/iron"}},
            "ingredients": [{"tag": "minecraft:logs"}, "#c:dyes/red", {"item": "c:not_a_tag"}],
            "values": ["#c:stones", {"id": "#c:sands/red", "required": false}]});
        let mut out = std::collections::BTreeSet::new();
        named(&doc, &mut out);
        let want: Vec<&str> = vec!["dyes/red", "ingots/iron", "sands/red", "stones"];
        assert_eq!(out.iter().map(String::as_str).collect::<Vec<_>>(), want);
    }
}
