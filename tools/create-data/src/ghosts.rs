//! The placement helpers' ghosts (catnip's `GhostBlocks`): the block models of the blocks a
//! placement helper places, baked from the player's jar at `prepare` into a list of quads per
//! block state part, which the glue draws translucent through world pipelines
//! (`create/src/glue/interaction/ghost.rs`). The mod's draw of a block model cannot be made
//! translucent (`API_GAPS.md`), so it draws the quads itself.
//!
//! Only the conversion is the port's: the geometry comes from Create's jar on the player's
//! machine and goes only to the prepared mod folder, as the bytes of
//! `src/glue/interaction/ghost_models.rs` (the committed one holds none; the native build
//! compiles the prepared crate's sources only).
//!
//! The format, little endian: `CGH1`; the sprites (u16 count, each a u16 length and its id);
//! the blocks (u16 count), each its id, whether it is a multipart (u8), its parts (u16 count),
//! each its condition (`a=x|y,b=z;c=w`: alternatives of property tests, empty for always), its
//! quads (u32 count), each its sprite (u16), four corners (3 f32 each, in blocks) and their UVs
//! (2 f32 each, 0 to 1 across the sprite).

use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Where the prepared mod's crate finds the baked models.
pub const RS: &str = "src/glue/interaction/ghost_models.rs";

/// The blocks whose items have a placement helper (`IPlacementHelper.matchesItem`), by name.
fn ghosted(name: &str) -> bool {
    matches!(
        name,
        "shaft"
            | "cogwheel"
            | "large_cogwheel"
            | "metal_girder"
            | "white_sail"
            | "sail_frame"
            | "display_board"
    ) || name.ends_with("_ladder")
        || name.ends_with("_table_cloth")
}

/// A face's corners as `FaceInfo` orders them: top left, bottom left, bottom right, top right
/// seen from outside, as `[x, y, z]` picks of from (false) or to (true). Down, up, north,
/// south, west, east.
const CORNERS: [[[bool; 3]; 4]; 6] = [
    [
        [false, false, true],
        [false, false, false],
        [true, false, false],
        [true, false, true],
    ],
    [
        [false, true, false],
        [false, true, true],
        [true, true, true],
        [true, true, false],
    ],
    [
        [true, true, false],
        [true, false, false],
        [false, false, false],
        [false, true, false],
    ],
    [
        [false, true, true],
        [false, false, true],
        [true, false, true],
        [true, true, true],
    ],
    [
        [false, true, false],
        [false, false, false],
        [false, false, true],
        [false, true, true],
    ],
    [
        [true, true, true],
        [true, false, true],
        [true, false, false],
        [true, true, false],
    ],
];

const FACES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

/// A face's UV when the model gives none: where the element lies on that side.
fn default_uv(face: usize, f: [f64; 3], t: [f64; 3]) -> [f64; 4] {
    match face {
        0 => [f[0], 16.0 - t[2], t[0], 16.0 - f[2]],
        1 => [f[0], f[2], t[0], t[2]],
        2 => [16.0 - t[0], 16.0 - t[1], 16.0 - f[0], 16.0 - f[1]],
        3 => [f[0], 16.0 - t[1], t[0], 16.0 - f[1]],
        4 => [f[2], 16.0 - t[1], t[2], 16.0 - f[1]],
        _ => [16.0 - t[2], 16.0 - t[1], 16.0 - f[2], 16.0 - f[1]],
    }
}

fn vec3(v: Option<&Value>) -> Option<[f64; 3]> {
    let a = v?.as_array()?;
    Some([
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2)?.as_f64()?,
    ])
}

/// `p` turned `degrees` about `axis` (0 x, 1 y, 2 z) through `origin`, counterclockwise seen
/// from the axis's positive end, the other two axes scaled by `scale` from the origin.
fn rotate(p: [f64; 3], axis: usize, degrees: f64, origin: [f64; 3], scale: f64) -> [f64; 3] {
    let (s, c) = degrees.to_radians().sin_cos();
    let mut d = [0, 1, 2].map(|i| p[i] - origin[i]);
    let (a, b) = match axis {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    };
    let (x, y) = (d[a], d[b]);
    d[a] = (x * c - y * s) * scale;
    d[b] = (x * s + y * c) * scale;
    [0, 1, 2].map(|i| d[i] + origin[i])
}

/// A model id as a jar path: `create:block/x` and `block/x` (vanilla's).
fn model_path(id: &str) -> String {
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    format!("assets/{ns}/models/{path}.json")
}

fn full_id(id: &str) -> String {
    if id.contains(':') {
        id.to_owned()
    } else {
        format!("minecraft:{id}")
    }
}

/// A `neoforge:composite` model as Create's files write it (the nixie tube, display and stock
/// links): a model of named `children`, each with its own textures and elements. Its children's
/// elements, in the file's order, become the model's own, their textures joined under the
/// model's. This is private to the local ghost baker; the game loads the original model.
fn flatten_composite(doc: &mut Value) {
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

/// A model's elements (its own or its nearest parent's) and its textures, children's over
/// parents'. None when the model itself is missing; a missing parent (vanilla's `block/block`)
/// adds nothing.
fn resolve(
    files: &BTreeMap<String, Vec<u8>>,
    id: &str,
) -> Option<(Vec<Value>, Map<String, Value>)> {
    let mut textures = Map::new();
    let mut elements = None;
    let mut next = Some(id.to_owned());
    let mut first = true;
    for _ in 0..16 {
        let Some(cur) = next.take() else { break };
        let Some(bytes) = files.get(&model_path(&cur)) else {
            if first {
                return None;
            }
            break;
        };
        first = false;
        let mut doc: Value = serde_json::from_slice(bytes).ok()?;
        flatten_composite(&mut doc);
        if let Some(Value::Object(t)) = doc.get("textures") {
            for (k, v) in t {
                textures.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        if elements.is_none()
            && let Some(Value::Array(e)) = doc.get("elements")
        {
            elements = Some(e.clone());
        }
        next = doc.get("parent").and_then(Value::as_str).map(str::to_owned);
    }
    Some((elements.unwrap_or_default(), textures))
}

/// A texture reference (`#0`) followed to a sprite id.
fn texture(textures: &Map<String, Value>, name: &str) -> Option<String> {
    let mut name = name.to_owned();
    for _ in 0..16 {
        match name.strip_prefix('#') {
            Some(key) => name = textures.get(key)?.as_str()?.to_owned(),
            None => return Some(full_id(&name)),
        }
    }
    None
}

/// One baked quad: its sprite, corners in blocks and UVs across the sprite.
struct Quad {
    sprite: String,
    corners: [[f64; 3]; 4],
    uv: [[f64; 2]; 4],
}

/// Model `id`'s quads with a block state's `x` and `y` turns, as `FaceBakery` bakes them
/// (`tesselateBlock` with no culling draws every face).
fn bake(files: &BTreeMap<String, Vec<u8>>, id: &str, x: f64, y: f64) -> Option<Vec<Quad>> {
    let (elements, textures) = resolve(files, &full_id(id))?;
    let mut out = Vec::new();
    for e in &elements {
        let (Some(f), Some(t)) = (vec3(e.get("from")), vec3(e.get("to"))) else {
            continue;
        };
        let turn = e.get("rotation").and_then(|r| {
            let axis = match r.get("axis")?.as_str()? {
                "x" => 0,
                "y" => 1,
                _ => 2,
            };
            let angle = r.get("angle")?.as_f64()?;
            let origin = vec3(r.get("origin")).unwrap_or([8.0; 3]);
            let rescale = r.get("rescale").and_then(Value::as_bool) == Some(true);
            let scale = if rescale && angle != 0.0 {
                1.0 / angle.to_radians().cos()
            } else {
                1.0
            };
            Some((axis, angle, origin, scale))
        });
        let Some(Value::Object(faces)) = e.get("faces") else {
            continue;
        };
        for (i, name) in FACES.iter().enumerate() {
            let Some(face) = faces.get(*name) else {
                continue;
            };
            let Some(sprite) = face
                .get("texture")
                .and_then(Value::as_str)
                .and_then(|n| texture(&textures, n))
            else {
                continue;
            };
            let uv = face
                .get("uv")
                .and_then(Value::as_array)
                .and_then(|a| {
                    Some([
                        a.first()?.as_f64()?,
                        a.get(1)?.as_f64()?,
                        a.get(2)?.as_f64()?,
                        a.get(3)?.as_f64()?,
                    ])
                })
                .unwrap_or_else(|| default_uv(i, f, t));
            // `BlockFaceUV`: corner k shows the UV corner `k + rotation / 90`.
            let shift = (face.get("rotation").and_then(Value::as_i64).unwrap_or(0) / 90)
                .rem_euclid(4) as usize;
            let uv_corner = |k: usize| match (k + shift) % 4 {
                0 => [uv[0], uv[1]],
                1 => [uv[0], uv[3]],
                2 => [uv[2], uv[3]],
                _ => [uv[2], uv[1]],
            };
            let corners = CORNERS[i].map(|pick| {
                let mut p = [0, 1, 2].map(|a| if pick[a] { t[a] } else { f[a] });
                if let Some((axis, angle, origin, scale)) = turn {
                    p = rotate(p, axis, angle, origin, scale);
                }
                // The block state's turns, `x` then `y`, both clockwise seen from the axis's
                // positive end (`BlockModelRotation`), about the block's centre.
                p = rotate(p, 0, -x, [8.0; 3], 1.0);
                p = rotate(p, 1, -y, [8.0; 3], 1.0);
                p.map(|v| v / 16.0)
            });
            out.push(Quad {
                sprite,
                corners,
                uv: [0, 1, 2, 3].map(|k| uv_corner(k).map(|v| v / 16.0)),
            });
        }
    }
    Some(out)
}

/// A multipart `when` as alternatives of tests (`a=x|y,b=z;c=w`).
fn condition(when: &Value) -> String {
    let tests = |o: &Map<String, Value>| -> String {
        o.iter()
            .map(|(k, v)| {
                let v = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                format!("{k}={v}")
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    let Some(o) = when.as_object() else {
        return String::new();
    };
    if let Some(Value::Array(any)) = o.get("OR") {
        return any
            .iter()
            .filter_map(Value::as_object)
            .map(tests)
            .collect::<Vec<_>>()
            .join(";");
    }
    if let Some(Value::Array(all)) = o.get("AND") {
        return all
            .iter()
            .filter_map(Value::as_object)
            .map(tests)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(",");
    }
    tests(o)
}

/// A model reference of a block state (`{"model", "x", "y"}`, or the first of a weighted list).
fn applied(v: &Value) -> Option<(&str, f64, f64)> {
    let v = match v {
        Value::Array(a) => a.first()?,
        v => v,
    };
    Some((
        v.get("model")?.as_str()?,
        v.get("x").and_then(Value::as_f64).unwrap_or(0.0),
        v.get("y").and_then(Value::as_f64).unwrap_or(0.0),
    ))
}

struct Ghost {
    id: String,
    multipart: bool,
    parts: Vec<(String, Vec<Quad>)>,
}

fn ghost(files: &BTreeMap<String, Vec<u8>>, name: &str, doc: &Value) -> Option<Ghost> {
    let mut parts = Vec::new();
    let multipart = if let Some(Value::Object(variants)) = doc.get("variants") {
        for (key, v) in variants {
            let (model, x, y) = applied(v)?;
            parts.push((key.clone(), bake(files, model, x, y)?));
        }
        false
    } else if let Some(Value::Array(list)) = doc.get("multipart") {
        for p in list {
            let (model, x, y) = applied(p.get("apply")?)?;
            let when = p.get("when").map(condition).unwrap_or_default();
            parts.push((when, bake(files, model, x, y)?));
        }
        true
    } else {
        return None;
    };
    Some(Ghost {
        id: format!("create:{name}"),
        multipart,
        parts,
    })
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn encode(ghosts: &[Ghost]) -> Vec<u8> {
    let mut sprites: Vec<&str> = ghosts
        .iter()
        .flat_map(|g| g.parts.iter().flat_map(|p| p.1.iter()))
        .map(|q| q.sprite.as_str())
        .collect();
    sprites.sort_unstable();
    sprites.dedup();
    let mut out = b"CGH1".to_vec();
    out.extend_from_slice(&(sprites.len() as u16).to_le_bytes());
    for s in &sprites {
        put_str(&mut out, s);
    }
    out.extend_from_slice(&(ghosts.len() as u16).to_le_bytes());
    for g in ghosts {
        put_str(&mut out, &g.id);
        out.push(u8::from(g.multipart));
        out.extend_from_slice(&(g.parts.len() as u16).to_le_bytes());
        for (when, quads) in &g.parts {
            put_str(&mut out, when);
            out.extend_from_slice(&(quads.len() as u32).to_le_bytes());
            for q in quads {
                let s = sprites.binary_search(&q.sprite.as_str()).unwrap_or(0);
                out.extend_from_slice(&(s as u16).to_le_bytes());
                for c in q.corners {
                    for v in c {
                        out.extend_from_slice(&(v as f32).to_le_bytes());
                    }
                }
                for c in q.uv {
                    for v in c {
                        out.extend_from_slice(&(v as f32).to_le_bytes());
                    }
                }
            }
        }
    }
    out
}

/// `bytes` as the prepared crate's `ghost_models.rs`: a byte string.
fn source(bytes: &[u8]) -> String {
    let mut out = String::from(
        "//! Written by `create-data prepare`: the helped blocks' models, baked from Create's jar.\n\
         pub static DATA: &[u8] = b\"",
    );
    for (i, b) in bytes.iter().enumerate() {
        if i % 64 == 63 {
            out.push_str("\\\n");
        }
        let _ = write!(out, "\\x{b:02x}");
    }
    out.push_str("\";\n");
    out
}

/// The ghosts of the helped blocks in `files` (Create's jar), as the prepared crate's
/// `ghost_models.rs` (to go at [`RS`]), and how many blocks.
pub fn source_file(files: &BTreeMap<String, Vec<u8>>) -> Result<(String, usize), String> {
    let mut ghosts = Vec::new();
    for (path, bytes) in files {
        let Some(name) = path
            .strip_prefix("assets/create/blockstates/")
            .and_then(|n| n.strip_suffix(".json"))
        else {
            continue;
        };
        if !ghosted(name) {
            continue;
        }
        let doc: Value = serde_json::from_slice(bytes).map_err(|e| format!("{path}: {e}"))?;
        if let Some(g) = ghost(files, name, &doc) {
            ghosts.push(g);
        }
    }
    Ok((source(&encode(&ghosts)), ghosts.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn files(entries: &[(&str, Value)]) -> BTreeMap<String, Vec<u8>> {
        entries
            .iter()
            .map(|(p, v)| ((*p).to_owned(), serde_json::to_vec(v).unwrap()))
            .collect()
    }

    #[test]
    fn bakes_an_element_turned_by_its_block_state() {
        // A made-up slab-like element: its top face, turned a quarter about x by the state.
        let f = files(&[(
            "assets/create/models/block/t.json",
            json!({"parent": "block/block", "textures": {"0": "create:block/t"},
                "elements": [{"from": [0, 0, 0], "to": [16, 8, 16],
                    "faces": {"up": {"texture": "#0"}}}]}),
        )]);
        let q = bake(&f, "create:block/t", 0.0, 0.0).unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].sprite, "create:block/t");
        assert_eq!(q[0].corners[0], [0.0, 0.5, 0.0]);
        assert_eq!(q[0].uv, [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]]);
        // `x: 90` turns the top toward north (-z): the slab's top goes to z = 0.5.
        let q = bake(&f, "create:block/t", 90.0, 0.0).unwrap();
        let c = q[0].corners[0];
        assert!((c[2] - 0.5).abs() < 1e-9 && c[1].abs() < 1e-9, "{c:?}");
        // `y: 90` turns north to east.
        let p = rotate([8.0, 8.0, 0.0], 1, -90.0, [8.0; 3], 1.0);
        assert!(
            (p[0] - 16.0).abs() < 1e-9 && (p[2] - 8.0).abs() < 1e-9,
            "{p:?}"
        );
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
    fn writes_multipart_conditions() {
        assert_eq!(
            condition(&json!({"x": "true", "z": "false"})),
            "x=true,z=false"
        );
        assert_eq!(
            condition(&json!({"OR": [{"a": "1|2"}, {"b": "c"}]})),
            "a=1|2;b=c"
        );
    }

    #[test]
    fn encodes_helped_blocks_only() {
        let model = json!({"textures": {"0": "create:block/t"},
            "elements": [{"from": [0, 0, 0], "to": [16, 16, 16], "faces": {"up": {"texture": "#0"}}}]});
        let f = files(&[
            ("assets/create/models/block/t.json", model),
            (
                "assets/create/blockstates/shaft.json",
                json!({"variants": {"axis=y": {"model": "create:block/t"}}}),
            ),
            (
                "assets/create/blockstates/gearbox.json",
                json!({"variants": {"": {"model": "create:block/t"}}}),
            ),
        ]);
        let (rs, n) = source_file(&f).unwrap();
        assert_eq!(n, 1);
        assert!(rs.contains("b\"\\x43\\x47\\x48\\x31"));
        let bin = encode(&[ghost(
            &f,
            "shaft",
            &serde_json::from_slice(&f["assets/create/blockstates/shaft.json"]).unwrap(),
        )
        .unwrap()]);
        assert!(bin.starts_with(b"CGH1"));
        // One sprite, one block, one part, one quad of 2 + 80 bytes.
        let len = 4 + 2 + (2 + 14) + 2 + (2 + 12) + 1 + 2 + (2 + 6) + 4 + 82;
        assert_eq!(bin.len(), len);
    }
}
