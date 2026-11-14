//! Create's two creative tabs (`AllCreativeModeTabs`): `base` (Create's machines and items) and
//! `palettes` (building blocks), each in registration order, blocks before plain items, without
//! the items Create keeps out of them (`makeExclusionPredicate`) or shows only in search
//! (`makeVisibilityFunc`: the dyed variants but one colour).
//!
//! Registration order is read from the Java: a name's first quoted occurrence in `AllBlocks`,
//! `AllItems`, `AllFluids` and `AllPaletteBlocks` (Create registers them in that order). Names built
//! in a loop (`colour + "_seat"`) are found by their quoted suffix. `AllBlocks` changes to the
//! palettes tab at its `setCreativeTab(...PALETTES_CREATIVE_TAB)`; names found nowhere (the stone
//! types' variants, built from patterns) are palettes, after the rest, by name.

use serde_json::{Value, json};
use std::{fs, path::Path};

/// `makeExclusionPredicate`'s items (the crushed ores of absent metals are kept: their tags are
/// not known here), and the items shown only in search.
const LEFT_OUT: &[&str] = &[
    "incomplete_precision_mechanism",
    "unprocessed_obsidian_sheet",
    "incomplete_track",
    "chromatic_compound",
    "shadow_steel",
    "refined_radiance",
    "copper_backtank_placeable",
    "netherite_backtank_placeable",
    "minecart_contraption",
    "furnace_minecart_contraption",
    "chest_minecart_contraption",
    "schematic",
    "shopping_list",
    "andesite_encased_shaft",
    "brass_encased_shaft",
    "andesite_encased_cogwheel",
    "brass_encased_cogwheel",
    "andesite_encased_large_cogwheel",
    "brass_encased_large_cogwheel",
    "mysterious_cuckoo_clock",
    "elevator_contact",
    "shadow_steel_casing",
    "refined_radiance_casing",
    "blaze_cake_base",
    // Blocks Create registers without an item (declared here with one).
    "lit_blaze_burner",
    "mechanical_piston_head",
    "small_bogey",
    "large_bogey",
];

/// `TagDependentIngredientItem.shouldHide`: crushed ores of metals no tag here holds.
const CRUSHED_SHOWN: &[&str] = &["iron", "gold", "copper", "zinc"];

/// Dyed families: the one colour each shows in its tab (`makeVisibilityFunc`).
const DYED: &[(&str, &str)] = &[
    ("_valve_handle", ""),
    // Dyed sails have no item of their own.
    ("_sail", "white"),
    ("_seat", "red"),
    ("_table_cloth", "red"),
    ("_postbox", "white"),
    ("_toolbox", "brown"),
];

const COLOURS: &[&str] = &[
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

fn shown(name: &str) -> bool {
    if LEFT_OUT.contains(&name) || name.starts_with("rare_") {
        return false;
    }
    if let Some(metal) = name.strip_prefix("crushed_raw_") {
        return CRUSHED_SHOWN.contains(&metal);
    }
    for (suffix, colour) in DYED {
        if let Some(c) = name.strip_suffix(suffix)
            && COLOURS.contains(&c)
        {
            return c == *colour;
        }
    }
    true
}

/// The registering sources in order, and where `AllBlocks` turns to the palettes tab.
struct Sources {
    text: String,
    palettes_from: usize,
    blocks_end: usize,
    palette_blocks: usize,
}

fn sources(java: &Path) -> Sources {
    let read = |p: &str| fs::read_to_string(java.join(p)).unwrap_or_default();
    let blocks = read("com/simibubi/create/AllBlocks.java");
    let marker = "setCreativeTab(AllCreativeModeTabs.PALETTES_CREATIVE_TAB)";
    let split = blocks.find(marker).unwrap_or(blocks.len());
    let blocks_end = blocks.len();
    let mut text = blocks;
    text.push_str(&read("com/simibubi/create/AllItems.java"));
    text.push_str(&read("com/simibubi/create/AllFluids.java"));
    let palette_blocks = text.len();
    text.push_str(&read(
        "com/simibubi/create/content/decoration/palettes/AllPaletteBlocks.java",
    ));
    Sources {
        text,
        palettes_from: split,
        blocks_end,
        palette_blocks,
    }
}

/// Where `name` is registered: its quoted name, else its quoted suffixes (`"_seat"`).
fn position(src: &Sources, name: &str) -> Option<usize> {
    if let Some(at) = src.text.find(&format!("\"{name}\"")) {
        return Some(at);
    }
    let mut rest = name;
    while let Some((_, tail)) = rest.split_once('_') {
        if let Some(at) = src.text.find(&format!("\"_{tail}\"")) {
            return Some(at);
        }
        rest = tail;
    }
    None
}

/// The two tabs' declarations, `(file name, document)`, from the declared items: `blocks` (block
/// items) and `items` (the rest).
pub fn tabs(java: &Path, blocks: &[String], items: &[String]) -> Vec<(&'static str, Value)> {
    let src = sources(java);
    // (in palettes, is a plain item, position, name)
    let mut entries: Vec<(bool, bool, usize, &str)> = Vec::new();
    for (names, item) in [(blocks, false), (items, true)] {
        for name in names.iter().filter(|n| shown(n)) {
            let pos = position(&src, name);
            // Plain items found nowhere (packages, buckets) are `AllItems`' or `AllFluids'`.
            let palettes = pos.map_or(!item, |p| {
                (src.palettes_from..src.blocks_end).contains(&p) || p >= src.palette_blocks
            });
            entries.push((palettes, item, pos.unwrap_or(usize::MAX), name));
        }
    }
    entries.sort_by(|a, b| (a.0, a.1, a.2, a.3).cmp(&(b.0, b.1, b.2, b.3)));
    let list = |palettes: bool| -> Vec<String> {
        let mut out: Vec<String> = entries
            .iter()
            .filter(|e| e.0 == palettes)
            .map(|e| format!("create:{}", e.3))
            .collect();
        if !palettes {
            // `makeOrderings`: the empty blaze burner before the burner, the schedule before the
            // station, the vertical gearbox after the gearbox, packages after the packager.
            reorder(&mut out, "create:empty_blaze_burner", "create:blaze_burner", 0);
            reorder(&mut out, "create:schedule", "create:track_station", 0);
            reorder(&mut out, "create:vertical_gearbox", "create:gearbox", 1);
            let boxes: Vec<String> = out
                .iter()
                .filter(|n| n.starts_with("create:cardboard_package_"))
                .cloned()
                .collect();
            for b in boxes.iter().rev() {
                reorder(&mut out, b, "create:packager", 1);
            }
        }
        out
    };
    vec![
        (
            "base",
            json!({ "title": "itemGroup.create.base", "icon": "create:cogwheel", "items": list(false) }),
        ),
        (
            "palettes",
            json!({ "title": "itemGroup.create.palettes", "icon": "create:ornate_iron_window", "items": list(true) }),
        ),
    ]
}

/// Moves `item` before (`after` 0) or after (1) `anchor`, when both are there.
fn reorder(list: &mut Vec<String>, item: &str, anchor: &str, after: usize) {
    let Some(i) = list.iter().position(|n| n == item) else {
        return;
    };
    let moved = list.remove(i);
    match list.iter().position(|n| n == anchor) {
        Some(a) => list.insert(a + after, moved),
        None => list.insert(i, moved),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_colour_of_each_dyed_family_shows() {
        assert!(shown("red_seat") && !shown("blue_seat"));
        assert!(shown("brown_toolbox") && !shown("red_toolbox"));
        assert!(shown("white_postbox") && !shown("black_postbox"));
        assert!(!shown("red_valve_handle") && shown("copper_valve_handle"));
        assert!(!shown("schematic") && shown("schematic_and_quill"));
        assert!(shown("white_sail") && !shown("red_sail"));
        assert!(shown("crushed_raw_zinc") && !shown("crushed_raw_tin"));
    }
}
