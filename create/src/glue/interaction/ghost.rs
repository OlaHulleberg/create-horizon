//! The placement helpers' ghost (catnip's `IPlacementHelper.displayGhost` and
//! `GhostBlockRenderer.transparent()`): the block a helper would place, drawn at its target
//! from its block model, white, translucent at `getBreathingAlpha() * 0.75` of the indicator's
//! fade-in, full bright, shrunk to 0.85 about its centre.
//!
//! The mod's draw of a block model (`draw_model_instances`) cannot be made translucent
//! (`API_GAPS.md`), so the ghost is drawn as quads through one world pipeline per sprite: the
//! helped blocks' models are baked from Create's jar by `create-data prepare` into the prepared
//! crate's `ghost_models.bin` (`tools/create-data/src/ghosts.rs` writes the format); the
//! committed `ghost_models.rs` embeds none, and with none no ghost is drawn. A copycat's ghost is
//! the copycat base cut to its shape, as its block entity draws an empty one.

use super::super::states;
use super::ghost_models::DATA;
use create_engine::copycat::{self as cc, Shape};
use mod_api::{
    ClientHost, Direction, Identifier, PipelineBlend, PipelineQuad, PipelineTexture,
    RegisterRenderPipelinesEvent, RenderPipeline, WorldDepth,
};
use std::collections::HashMap;
use std::sync::OnceLock;

/// The copycat base's sprite, which the baked models need not hold.
const COPYCAT_BASE: &str = "create:block/copycat_base";

/// A quad to draw: its sprite's index in `sprites()`, its corners in a block's units and UVs.
type GhostQuad = (usize, [[f32; 3]; 4], [[f32; 2]; 4]);

struct Quad {
    sprite: u16,
    corners: [[f32; 3]; 4],
    uv: [[f32; 2]; 4],
}

/// A block state part: its condition (alternatives of property tests) and its quads.
struct Part {
    when: Vec<Vec<(String, Vec<String>)>>,
    quads: Vec<Quad>,
}

struct Model {
    multipart: bool,
    parts: Vec<Part>,
}

struct Models {
    sprites: Vec<String>,
    blocks: HashMap<String, Model>,
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
}

/// `a=x|y,b=z;c=w`.
fn condition(s: &str) -> Vec<Vec<(String, Vec<String>)>> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split(';')
        .map(|all| {
            all.split(',')
                .filter_map(|t| t.split_once('='))
                .map(|(k, v)| (k.to_owned(), v.split('|').map(str::to_owned).collect()))
                .collect()
        })
        .collect()
}

fn parse(data: &[u8]) -> Option<Models> {
    let mut r = Reader(data);
    if r.take(4)? != b"CGH1" {
        return None;
    }
    let sprites = (0..r.u16()?).map(|_| r.str()).collect::<Option<Vec<_>>>()?;
    let mut blocks = HashMap::new();
    for _ in 0..r.u16()? {
        let id = r.str()?;
        let multipart = r.u8()? != 0;
        let mut parts = Vec::new();
        for _ in 0..r.u16()? {
            let when = condition(&r.str()?);
            let mut quads = Vec::new();
            for _ in 0..r.u32()? {
                let sprite = r.u16()?;
                let mut corners = [[0.0; 3]; 4];
                for c in &mut corners {
                    for v in c.iter_mut() {
                        *v = r.f32()?;
                    }
                }
                let mut uv = [[0.0; 2]; 4];
                for c in &mut uv {
                    for v in c.iter_mut() {
                        *v = r.f32()?;
                    }
                }
                quads.push(Quad {
                    sprite,
                    corners,
                    uv,
                });
            }
            parts.push(Part { when, quads });
        }
        blocks.insert(id, Model { multipart, parts });
    }
    Some(Models { sprites, blocks })
}

fn models() -> Option<&'static Models> {
    static MODELS: OnceLock<Option<Models>> = OnceLock::new();
    MODELS.get_or_init(|| parse(DATA)).as_ref()
}

/// The sprites the ghosts draw: the baked models' and the copycat base.
fn sprites() -> Vec<&'static str> {
    let mut all: Vec<&str> = models()
        .map(|m| m.sprites.iter().map(String::as_str).collect())
        .unwrap_or_default();
    if !all.contains(&COPYCAT_BASE) {
        all.push(COPYCAT_BASE);
    }
    all
}

fn pipeline_id(i: usize) -> String {
    format!("create:ghost_{i}")
}

/// `register_render_pipelines`: a translucent pipeline sampling each ghost sprite.
pub(crate) fn register(event: &mut RegisterRenderPipelinesEvent<'_>) {
    let Ok(shader) = Identifier::parse("create:textured") else {
        return;
    };
    for (i, s) in sprites().into_iter().enumerate() {
        let (Ok(id), Ok(sprite)) = (Identifier::parse(&pipeline_id(i)), Identifier::parse(s))
        else {
            continue;
        };
        event.pipelines.push(
            RenderPipeline::new(
                id,
                shader.clone(),
                PipelineBlend::Translucent,
                WorldDepth::Tested,
            )
            .textured(PipelineTexture::Sprite(sprite))
            .optional(),
        );
    }
}

fn matches(when: &[Vec<(String, Vec<String>)>], props: &[(&str, &str)]) -> bool {
    when.is_empty()
        || when.iter().any(|all| {
            all.iter().all(|(k, vs)| {
                props
                    .iter()
                    .any(|(pk, pv)| pk == k && vs.iter().any(|v| v == pv))
            })
        })
}

/// The quads of block state `state` (`create:cogwheel[axis=x]`), in a block's units, each with
/// its sprite's index in `sprites()`.
fn quads(host: &dyn ClientHost, state: &str) -> Vec<GhostQuad> {
    let (id, props) = states::split(state);
    let mut out = Vec::new();
    if let Some(shape) = copycat_shape(id, &props) {
        let base = sprites().iter().position(|s| *s == COPYCAT_BASE);
        let sprites = host
            .block_state_id(cc::BASE)
            .and_then(|b| host.block_sprites(&[b]).pop().flatten());
        let (Some(base), Some(sprites)) = (base, sprites) else {
            return out;
        };
        for [from, to] in shape.boxes() {
            for side in Direction::ALL {
                for face in &sprites.sides[side as usize] {
                    if face.inset != 0.0 {
                        continue;
                    }
                    let Some((corners, [u0, v0, u1, v1])) = face.quad_on(side, from, to) else {
                        continue;
                    };
                    let uv = [[u0, v0], [u0, v1], [u1, v1], [u1, v0]].map(|c| c.map(|t| t / 16.0));
                    out.push((base, corners, uv));
                }
            }
        }
        return out;
    }
    let Some(model) = models().and_then(|m| m.blocks.get(id)) else {
        return out;
    };
    for part in &model.parts {
        if !matches(&part.when, &props) {
            continue;
        }
        out.extend(
            part.quads
                .iter()
                .map(|q| (usize::from(q.sprite), q.corners, q.uv)),
        );
        // A variant block state draws its first matching variant only.
        if !model.multipart {
            break;
        }
    }
    out
}

/// A copycat's shape from its state's properties (`CopycatPanelModel`, `CopycatStepModel`).
fn copycat_shape(id: &str, props: &[(&str, &str)]) -> Option<Shape> {
    let get = |k: &str| props.iter().find(|(p, _)| *p == k).map(|(_, v)| *v);
    match id {
        "create:copycat_panel" => Some(Shape::Panel(cc::dir_named(get("facing")?)?)),
        "create:copycat_step" => Some(Shape::Step {
            facing: cc::dir_named(get("facing")?)?,
            top: get("half") == Some("top"),
        }),
        _ => None,
    }
}

/// `GhostBlocks.getBreathingAlpha`: 0.35 to 0.75 and back every 2.5 seconds of the clock.
fn breathing_alpha() -> f32 {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let timer = (ms % 2500) as f64;
    (0.55 - 0.2 * ((2.0 / 2500.0) * std::f64::consts::PI * timer).cos()) as f32
}

/// `TransparentGhostBlockRenderer.render`: `state` at `pos`, at `progress` of the indicator's
/// fade-in (`PlacementClient.getCurrentAlpha`).
pub(super) fn draw(
    host: &dyn ClientHost,
    event: &mut mod_api::RenderLevelStageEvent<'_>,
    state: &str,
    pos: [i32; 3],
    progress: f32,
) {
    let alpha = breathing_alpha() * 0.75 * progress;
    // `ColoringVertexConsumer(1, 1, 1, alpha)` over the model's vertex colours, which are white:
    // the virtual level's shade is 1 and its light full.
    let color = (((alpha.clamp(0.0, 1.0) * 255.0) as u32) << 24) | 0x00FF_FFFF;
    let mut by_sprite: HashMap<usize, Vec<PipelineQuad>> = HashMap::new();
    let at = |p: [f32; 3]| {
        // `translate(.5)`, `scale(.85)`, `translate(-.5)` in the block at `pos`.
        [0, 1, 2].map(|i| f64::from(pos[i]) + 0.5 + (f64::from(p[i]) - 0.5) * 0.85)
    };
    for (sprite, corners, uv) in quads(host, state) {
        by_sprite
            .entry(sprite)
            .or_default()
            .push(PipelineQuad::new(corners.map(at), uv, color));
    }
    for (sprite, quads) in by_sprite {
        let Ok(id) = Identifier::parse(&pipeline_id(sprite)) else {
            continue;
        };
        if let Ok(p) = host.render_assets().render_pipeline(&id) {
            let _ = event.draw.pipeline_quads(p, &quads);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_conditions() {
        let when = condition("x=true,z=false;top=true|false");
        assert!(matches(&when, &[("x", "true"), ("z", "false")]));
        assert!(matches(&when, &[("top", "false")]));
        assert!(!matches(&when, &[("x", "true"), ("z", "true")]));
        assert!(matches(&condition(""), &[]));
    }

    #[test]
    fn parses_the_baked_format() {
        let mut d = b"CGH1".to_vec();
        let s = |d: &mut Vec<u8>, t: &str| {
            d.extend_from_slice(&(t.len() as u16).to_le_bytes());
            d.extend_from_slice(t.as_bytes());
        };
        d.extend_from_slice(&1u16.to_le_bytes());
        s(&mut d, "create:block/t");
        d.extend_from_slice(&1u16.to_le_bytes());
        s(&mut d, "create:shaft");
        d.push(0);
        d.extend_from_slice(&1u16.to_le_bytes());
        s(&mut d, "axis=y");
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes());
        for v in 0..20 {
            d.extend_from_slice(&(v as f32).to_le_bytes());
        }
        let m = parse(&d).unwrap();
        assert_eq!(m.sprites, ["create:block/t"]);
        let q = &m.blocks["create:shaft"].parts[0].quads[0];
        assert_eq!(q.corners[1], [3.0, 4.0, 5.0]);
        assert_eq!(q.uv[3], [18.0, 19.0]);
        assert!(parse(&d[..d.len() - 1]).is_none());
        // The committed crate embeds none.
        assert!(parse(&[]).is_none());
    }
}
