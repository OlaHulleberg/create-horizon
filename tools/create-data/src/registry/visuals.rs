//! Reference-only visuals and port-authored assets for the registry package.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

const COUPLING: &str = "assets/create/models/block/minecart_coupling/";
const CHAIN: &str = "assets/create/models/block/chain_conveyor/chain_";
const BLOCK_ATLAS: &str = "assets/minecraft/atlases/blocks.json";
const SHADER: &str = "assets/create/shaders/pipeline/textured.wgsl";
const CONNECTED: &str = "assets/create/sunrise/connected/";
const CONNECTED_TABLES: &str = "assets/create/sunrise/connected_tables/";

pub(crate) const SPINNING_TOOLS: [(&str, &[&str]); 3] = [
    ("wrench", &["item", "gear"]),
    ("potato_cannon", &["item", "cog"]),
    (
        "handheld_worldshaper",
        &["item", "core", "core_glow", "accelerator"],
    ),
];

/// The shell supplies the display transform; the special renderer names the original parts.
pub(crate) fn spinner_definition(item: &str) -> Value {
    json!({ "model": {
        "type": "minecraft:special",
        "base": format!("create:item/{item}/item"),
        "model": { "type": format!("create:{item}") },
    } })
}

/// The asset paths this generator owns, including obsolete generated frames.
pub(super) fn owns(path: &str) -> bool {
    matches!(path, BLOCK_ATLAS | SHADER)
        || (path.ends_with(".json")
            && [COUPLING, CHAIN, CONNECTED, CONNECTED_TABLES]
                .iter()
                .any(|dir| path.starts_with(dir)))
}

/// The jar supplies the coupling and spinning tools' models and its 23 original steam sprites.
/// Check the references by entry name only; this generator never reads their bytes.
pub(super) fn assets(contains: impl Fn(&str) -> bool) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut out = BTreeMap::new();
    for name in ["attachment", "connector", "ring"] {
        let model = format!("entity/minecart_coupling/{name}");
        let target = format!("assets/create/models/{model}.json");
        if !contains(&target) {
            return Err(format!(
                "{target}: missing coupling model in the pinned jar"
            ));
        }
        out.insert(
            format!("{COUPLING}{name}.json"),
            crate::pretty(&json!({ "parent": format!("create:{model}") })),
        );
    }
    for (item, parts) in SPINNING_TOOLS {
        for part in parts {
            let target = format!("assets/create/models/item/{item}/{part}.json");
            if !contains(&target) {
                return Err(format!(
                    "{target}: missing spinning tool part in the pinned jar"
                ));
            }
        }
        out.insert(
            format!("assets/create/items/{item}.json"),
            crate::pretty(&spinner_definition(item)),
        );
    }
    let mut sources = Vec::new();
    for frame in 0..23 {
        let name = format!("steam_jet_{frame}");
        let texture = format!("assets/create/textures/particle/{name}.png");
        if !contains(&texture) {
            return Err(format!("{texture}: missing steam frame in the pinned jar"));
        }
        sources.push(json!({
            "type": "minecraft:single",
            "resource": format!("create:particle/{name}"),
            "sprite": format!("create:block/special/{name}"),
        }));
    }
    let (connected, sheets) = crate::ct::native_assets();
    for sheet in sheets {
        let path = format!(
            "assets/create/textures/{}.png",
            sheet.trim_start_matches("create:")
        );
        if !contains(&path) {
            return Err(format!(
                "{path}: missing connected texture sheet in the pinned jar"
            ));
        }
        sources.push(json!({ "type": "minecraft:single", "resource": sheet }));
    }
    out.extend(connected);
    out.insert(
        BLOCK_ATLAS.into(),
        crate::pretty(&json!({ "sources": sources })),
    );
    out.insert(SHADER.into(), TEXTURED_SHADER.as_bytes().to_vec());
    for k in 1..16 {
        out.insert(
            format!("{CHAIN}{k}.json"),
            crate::pretty(&scrolled_chain(k)),
        );
    }
    Ok(out)
}

/// The fragment stage of the glue's textured world pipelines.
const TEXTURED_SHADER: &str = "fn pipeline_fragment(input: PipelineInput) -> vec4<f32> {
    return pipeline_texture(input.uv) * input.color;
}
";

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_names_every_steam_sprite_without_packing_pixels() {
        let generated = assets(|_| true).unwrap();
        let atlas: Value = serde_json::from_slice(&generated[BLOCK_ATLAS]).unwrap();
        let sources = atlas["sources"].as_array().unwrap();
        for (frame, source) in sources[..23].iter().enumerate() {
            assert_eq!(
                source,
                &json!({
                    "type": "minecraft:single",
                    "resource": format!("create:particle/steam_jet_{frame}"),
                    "sprite": format!("create:block/special/steam_jet_{frame}"),
                })
            );
        }
        assert!(!generated.keys().any(|path| path.ends_with(".png")));
        assert!(
            assets(|path| !path.ends_with("/steam_jet_12.png"))
                .unwrap_err()
                .contains("steam_jet_12.png")
        );
    }

    #[test]
    fn the_registry_stitches_each_referenced_connected_sheet_once() {
        let generated = assets(|_| true).unwrap();
        let atlas: Value = serde_json::from_slice(&generated[BLOCK_ATLAS]).unwrap();
        let sources = atlas["sources"].as_array().unwrap();
        let (rules, sheets) = crate::ct::native_assets();
        assert_eq!(sources.len(), 23 + sheets.len());
        for sheet in sheets {
            let expected = json!({ "type": "minecraft:single", "resource": sheet });
            assert_eq!(sources.iter().filter(|s| **s == expected).count(), 1);
        }
        for (path, bytes) in rules {
            assert!(owns(&path));
            assert_eq!(generated[&path], bytes);
        }
        assert!(
            assets(|path| !path.ends_with("/framed_glass_connected.png"))
                .unwrap_err()
                .contains("framed_glass_connected.png")
        );
    }

    #[test]
    fn model_aliases_and_special_items_do_not_contain_geometry() {
        let generated = assets(|_| true).unwrap();
        let coupling: Value =
            serde_json::from_slice(&generated[&format!("{COUPLING}ring.json")]).unwrap();
        assert_eq!(
            coupling,
            json!({ "parent": "create:entity/minecart_coupling/ring" })
        );
        let wrench: Value =
            serde_json::from_slice(&generated["assets/create/items/wrench.json"]).unwrap();
        assert_eq!(
            wrench,
            json!({ "model": {
            "type": "minecraft:special",
            "base": "create:item/wrench/item",
            "model": { "type": "create:wrench" },
        } })
        );
        assert!(
            assets(|path| !path.ends_with("/wrench/gear.json"))
                .unwrap_err()
                .contains("/wrench/gear.json")
        );
        assert!(
            assets(|path| !path.ends_with("/minecart_coupling/connector.json"))
                .unwrap_err()
                .contains("/minecart_coupling/connector.json")
        );
    }
}
