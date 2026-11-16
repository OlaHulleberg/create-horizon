//! Create's partial models (`AllPartialModels`) that the glue draws element by element, read by
//! id from the client's resources: the jar layer under the mod holds Create's model files, and
//! the client reads them there (`ResourcesHost::resource`), once per resource load. Their
//! geometry is never in this repository. A part block (`part_*`) is drawn whole in its model's
//! own colours; what Create colours (`SuperByteBuffer.color`) or draws additively
//! (`RenderTypes.additive()`) is drawn from these elements instead: as a block entity's quads
//! with a tint ([`cubes`]), or as quads of one of the glue's world pipelines ([`quads`]). A face
//! without its own `uv` is left out (Create's partials give every face one).

use super::pipelines::{Pose, face_corners};
use mod_api::serde_json::{Map, Value};
use mod_api::{BlockEntityDrawContext, ClientHost, Identifier, PackType};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// `AllPartialModels.FLEXPEATER_INDICATOR`.
pub const DIODE_INDICATOR: &str = "create:block/diodes/indicator";
/// `AllPartialModels.ANALOG_LEVER_INDICATOR`.
pub const ANALOG_LEVER_INDICATOR: &str = "create:block/analog_lever/indicator";
/// `AllPartialModels.DISPLAY_LINK_GLOW`.
pub const DISPLAY_LINK_GLOW: &str = "create:block/display_link/glow";
/// `AllPartialModels.SIGNAL_WHITE_CUBE`.
pub const SIGNAL_WHITE_CUBE: &str = "create:block/track_signal/white_cube";
/// `AllPartialModels.SIGNAL_RED_GLOW`, `SIGNAL_YELLOW_GLOW`, `SIGNAL_WHITE_GLOW`.
pub const SIGNAL_RED_GLOW: &str = "create:block/track_signal/red_glow";
pub const SIGNAL_YELLOW_GLOW: &str = "create:block/track_signal/yellow_glow";
pub const SIGNAL_WHITE_GLOW: &str = "create:block/track_signal/white_glow";
/// `AllPartialModels.SIGNAL_RED`, `SIGNAL_YELLOW`, `SIGNAL_WHITE`: the lights' tubes.
pub const SIGNAL_RED: &str = "create:block/track_signal/red_tube";
pub const SIGNAL_YELLOW: &str = "create:block/track_signal/yellow_tube";
pub const SIGNAL_WHITE: &str = "create:block/track_signal/white_tube";

/// A model face: its sprite, its `uv` (texels, 16 a side) and its `rotation` in degrees.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub sprite: String,
    pub uv: [f32; 4],
    pub rotation: u32,
}

/// A model element: a box from `from` to `to` (texels), turned by `rotation` (axis, degrees,
/// origin in texels), with its faces in `Direction` order (down, up, north, south, west, east).
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub rotation: Option<([f32; 3], f32, [f32; 3])>,
    pub faces: [Option<Face>; 6],
}

/// A model's elements, its parents' when it has none of its own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub elements: Vec<Element>,
}

thread_local! {
    /// The models read since the last resource load, by id (None: unreadable).
    static MODELS: RefCell<HashMap<&'static str, Option<Rc<Model>>>> = RefCell::new(HashMap::new());
}

/// The resources changed or the client left its world: read the models again when next drawn.
pub fn forget() {
    MODELS.with_borrow_mut(HashMap::clear);
}

/// The model `id` (`create:block/diodes/indicator`) as the client's resources give it.
pub fn get(host: &dyn ClientHost, id: &'static str) -> Option<Rc<Model>> {
    if let Some(m) = MODELS.with_borrow(|m| m.get(id).cloned()) {
        return m;
    }
    let read = |id: &str| -> Option<Map<String, Value>> {
        let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
        let name = Identifier::parse(&format!("{ns}:models/{path}.json")).ok()?;
        let bytes = host.resource(PackType::ClientResources, &name)?;
        match mod_api::serde_json::from_slice(&bytes).ok()? {
            Value::Object(o) => Some(o),
            _ => None,
        }
    };
    let model = load(id, read).map(Rc::new);
    if model.is_none() {
        crate::create_log!("[create] model {id} could not be read from the resources");
    }
    MODELS.with_borrow_mut(|m| m.insert(id, model.clone()));
    model
}

/// `id`'s model through `read` (a model file by id): the elements of the nearest of it and its
/// parents that has any, their textures named through the textures of all of them (the
/// nearest first).
pub fn load(id: &str, read: impl Fn(&str) -> Option<Map<String, Value>>) -> Option<Model> {
    let mut textures: Map<String, Value> = Map::new();
    let mut elements = None;
    let mut next = Some(id.to_owned());
    for _ in 0..8 {
        let Some(at) = next.take() else { break };
        // The model itself must be there; a parent that isn't adds nothing.
        let Some(doc) = read(&at) else {
            if at == id {
                return None;
            }
            break;
        };
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
    let sprite = |name: &str| -> Option<String> {
        let mut name = name.to_owned();
        for _ in 0..8 {
            match name.strip_prefix('#') {
                Some(key) => name = textures.get(key)?.as_str()?.to_owned(),
                None if name.contains(':') => return Some(name),
                None => return Some(format!("minecraft:{name}")),
            }
        }
        None
    };
    let floats = |v: Option<&Value>| -> Option<Vec<f32>> {
        v?.as_array()?
            .iter()
            .map(|n| n.as_f64().map(|f| f as f32))
            .collect()
    };
    let vec3 = |v: Option<&Value>| -> Option<[f32; 3]> {
        let f = floats(v)?;
        (f.len() == 3).then(|| [f[0], f[1], f[2]])
    };
    let mut out = Vec::new();
    for e in elements? {
        let from = vec3(e.get("from"))?;
        let to = vec3(e.get("to"))?;
        let rotation = e.get("rotation").and_then(|r| {
            let angle = r.get("angle")?.as_f64()? as f32;
            let axis = match r.get("axis")?.as_str()? {
                "x" => [1.0, 0.0, 0.0],
                "y" => [0.0, 1.0, 0.0],
                _ => [0.0, 0.0, 1.0],
            };
            let origin = vec3(r.get("origin")).unwrap_or([8.0; 3]);
            (angle != 0.0).then_some((axis, angle, origin))
        });
        let mut faces: [Option<Face>; 6] = Default::default();
        for (i, dir) in ["down", "up", "north", "south", "west", "east"]
            .iter()
            .enumerate()
        {
            let Some(f) = e.get("faces").and_then(|f| f.get(*dir)) else {
                continue;
            };
            let uv = floats(f.get("uv")).filter(|u| u.len() == 4);
            let (Some(uv), Some(texture)) = (uv, f.get("texture").and_then(Value::as_str)) else {
                continue;
            };
            faces[i] = Some(Face {
                sprite: sprite(texture)?,
                uv: [uv[0], uv[1], uv[2], uv[3]],
                rotation: f
                    .get("rotation")
                    .and_then(Value::as_u64)
                    .map_or(0, |r| (r % 360) as u32),
            });
        }
        out.push(Element {
            from,
            to,
            rotation,
            faces,
        });
    }
    Some(Model { elements: out })
}

/// Draws `model`'s faces under `draw`'s pose, shaded and lit as a block entity's quads are,
/// times the 0xRRGGBB `tint`, each face's uv turned by its `rotation` as a baked face's is.
pub fn cubes(draw: &mut dyn BlockEntityDrawContext, model: &Model, tint: u32) {
    for e in &model.elements {
        if let Some((axis, angle, origin)) = e.rotation {
            draw.push();
            draw.rotate_about(axis, angle, origin.map(|v| v / 16.0));
        }
        for (side, f) in e.faces.iter().enumerate() {
            let Some(f) = f else { continue };
            let corners = face_corners(e.from.map(|v| v / 16.0), e.to.map(|v| v / 16.0), side);
            // `quad` gives its corners the uv rectangle's corners in order: start the corners
            // where the face's rotation starts the rectangle.
            let turn = (f.rotation / 90) as usize;
            let turned = std::array::from_fn(|i| corners[(i + 4 - turn) % 4]);
            draw.quad(&f.sprite, turned, f.uv, tint);
        }
        if e.rotation.is_some() {
            draw.pop();
        }
    }
}

/// `model`'s faces placed by `pose`, in `color`, as quads of a world pipeline that samples one
/// sprite: each face's uv in that sprite, turned by its `rotation` as a baked face's are.
pub fn quads(pose: &Pose, model: &Model, color: u32, out: &mut Vec<mod_api::PipelineQuad>) {
    for e in &model.elements {
        let pose = match e.rotation {
            Some((axis, angle, origin)) => pose.rotate(
                axis.map(f64::from),
                f64::from(angle),
                origin.map(|v| f64::from(v) / 16.0),
            ),
            None => *pose,
        };
        for (side, f) in e.faces.iter().enumerate() {
            if let Some(f) = f {
                pose.face(
                    e.from.map(f64::from),
                    e.to.map(f64::from),
                    side,
                    f.uv,
                    f.rotation,
                    color,
                    out,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mod_api::serde_json::json;

    /// A made-up model and parent, in the format's shape.
    fn file(id: &str) -> Option<Map<String, Value>> {
        let v = match id {
            "test:block/child" => json!({
                "parent": "test:block/parent",
                "textures": { "a": "test:block/sprite" },
                "elements": [{
                    "from": [1, 2, 3], "to": [4, 5, 6],
                    "rotation": { "angle": 0, "axis": "y", "origin": [8, 8, 8] },
                    "faces": {
                        "up": { "uv": [0, 0, 2, 2], "texture": "#b", "rotation": 90 },
                        "north": { "uv": [1, 1, 3, 3], "texture": "#a" }
                    }
                }]
            }),
            "test:block/parent" => json!({ "textures": { "b": "#a", "a": "block/ignored" } }),
            _ => return None,
        };
        v.as_object().cloned()
    }

    #[test]
    fn a_model_names_its_sprites_through_its_and_its_parents_textures() {
        let m = load("test:block/child", file).unwrap();
        assert_eq!(m.elements.len(), 1);
        let e = &m.elements[0];
        assert_eq!(
            (e.from, e.to, e.rotation),
            ([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], None)
        );
        let up = e.faces[1].as_ref().unwrap();
        assert_eq!(
            (up.sprite.as_str(), up.uv, up.rotation),
            ("test:block/sprite", [0.0, 0.0, 2.0, 2.0], 90)
        );
        assert_eq!(e.faces[2].as_ref().unwrap().uv, [1.0, 1.0, 3.0, 3.0]);
        assert!(e.faces[0].is_none() && e.faces[3].is_none());
        assert!(load("test:block/missing", file).is_none());
    }
}
