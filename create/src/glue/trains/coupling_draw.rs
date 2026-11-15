//! Couplings as clients draw them (`CouplingRenderer.renderCoupling`): the server tells each
//! player of its level which carts are coupled by players (a contraption's couplings are not
//! drawn: `hasContraptionCoupling`), and the client draws at each cart the attachment, turned as
//! the cart faces the coupling (`getSuitableCartEndpoint`), and the ring, turned along the
//! connector, and the connector from the first end to the second, stretched to its length. The
//! carts' places are the client's own, eased between its ticks by the frame's partial tick.

use create_engine::coupling;
use create_engine::mounted::RailShape;
use mod_api::{
    ClientHost, Identifier, LightCoords, LightLayer, ModelInstance, ModelRef, Overlay,
    RenderLevelStage, RenderLevelStageEvent,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// The server's list of a level's players' couplings: each one's leading and coupled cart's
/// entity ids, as little-endian `u32` pairs.
pub const CHANNEL: &str = coupling::CHANNEL;

/// `AllPartialModels.COUPLING_ATTACHMENT`, `COUPLING_RING` and `COUPLING_CONNECTOR`, as
/// standalone models.
pub const MODELS: [&str; 3] = [
    "create:block/minecart_coupling/attachment",
    "create:block/minecart_coupling/ring",
    "create:block/minecart_coupling/connector",
];

/// The pairs as sent.
pub fn encode(pairs: &[(u32, u32)]) -> Vec<u8> {
    pairs
        .iter()
        .flat_map(|(a, b)| a.to_le_bytes().into_iter().chain(b.to_le_bytes()))
        .collect()
}

fn decode(data: &[u8]) -> Vec<(u32, u32)> {
    data.as_chunks::<8>()
        .0
        .iter()
        .map(|c| {
            (
                u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                u32::from_le_bytes([c[4], c[5], c[6], c[7]]),
            )
        })
        .collect()
}

#[derive(Default)]
struct Client {
    pairs: Vec<(u32, u32)>,
    /// Each coupled cart's place at the client's last two ticks.
    carts: HashMap<u32, ([f64; 3], [f64; 3])>,
    models: Option<[ModelRef; 3]>,
}

thread_local! {
    static CLIENT: RefCell<Client> = RefCell::new(Client::default());
}

pub fn reset() {
    CLIENT.with_borrow_mut(|c| {
        c.pairs.clear();
        c.carts.clear();
    });
}

/// `model_baking_completed`: the three models, looked up again.
pub fn models_baked(host: &dyn ClientHost) {
    let found = MODELS.map(|id| {
        Identifier::parse(id)
            .ok()
            .and_then(|i| host.render_assets().model(&i).ok())
    });
    let models = match found {
        [Some(a), Some(r), Some(c)] => Some([a, r, c]),
        _ => {
            crate::create_log!("[create] the coupling's models are missing");
            None
        }
    };
    CLIENT.with_borrow_mut(|c| c.models = models);
}

pub fn server_message(data: &[u8]) {
    let pairs = decode(data);
    CLIENT.with_borrow_mut(|c| {
        c.carts
            .retain(|id, _| pairs.iter().any(|(a, b)| a == id || b == id));
        c.pairs = pairs;
    });
}

/// Each coupled cart's place this tick, the last one kept.
pub fn tick(host: &dyn ClientHost) {
    CLIENT.with_borrow_mut(|c| {
        if c.pairs.is_empty() {
            return;
        }
        let ids: Vec<u32> = c.pairs.iter().flat_map(|(a, b)| [*a, *b]).collect();
        for id in ids {
            let Some(at) = host.entity(id).and_then(|e| e.position(host)) else {
                c.carts.remove(&id);
                continue;
            };
            let e = c.carts.entry(id).or_insert((at, at));
            *e = (e.1, at);
        }
    });
}

/// The shape of the rail at `p` this client shows (`BlockTags.RAILS`).
fn rail(host: &dyn ClientHost, p: [i32; 3]) -> Option<RailShape> {
    let state = host.level(host.dimension())?.block_state(host, p)?;
    super::carts::rail_facts(host, state).map(|r| r.0)
}

fn light(host: &dyn ClientHost, at: [f64; 3]) -> LightCoords {
    let block = at.map(|v| v.floor() as i32);
    host.level(host.dimension())
        .and_then(|l| {
            let b = l.brightness(host, LightLayer::Block, block)?;
            let s = l.brightness(host, LightLayer::Sky, block)?;
            LightCoords::new(b.clamp(0, 15) as u8, s.clamp(0, 15) as u8)
        })
        .unwrap_or(LightCoords::new(0, 15).unwrap_or(LightCoords::FULL_BRIGHT))
}

/// A turn about up by `yaw` degrees, then about z by `pitch`, then a stretch of x by
/// `stretch`, as a row-major 3×4 transform with no move (the instance's origin places it).
fn turn(yaw: f32, pitch: f32, stretch: f32) -> [f32; 12] {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    // R = Ry(yaw) · Rz(pitch) · S(stretch, 1, 1).
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cp, -sp, 0.0], [sp, cp, 0.0], [0.0, 0.0, 1.0]];
    let mut m = [0.0f32; 12];
    for (i, row) in ry.iter().enumerate() {
        for j in 0..3 {
            let v: f32 = (0..3).map(|k| row[k] * rz[k][j]).sum();
            m[i * 4 + j] = if j == 0 { v * stretch } else { v };
        }
    }
    m
}

/// `CouplingRenderer.renderAll`, after the opaque terrain.
pub fn render_level_stage(host: &dyn ClientHost, event: &mut RenderLevelStageEvent<'_>) {
    if event.stage != RenderLevelStage::AfterOpaqueBlocks {
        return;
    }
    let t = f64::from(event.view.partial_tick);
    CLIENT.with_borrow(|c| {
        let Some([attachment, ring, connector]) = c.models else {
            return;
        };
        let rails = |p: [i32; 3]| rail(host, p);
        for (a, b) in &c.pairs {
            let (Some(pa), Some(pb)) = (c.carts.get(a), c.carts.get(b)) else {
                continue;
            };
            let at = |(prev, now): ([f64; 3], [f64; 3])| {
                [0, 1, 2].map(|k| prev[k] + (now[k] - prev[k]) * t)
            };
            let ends = [at(*pa), at(*pb)];
            let centre = [0, 1, 2].map(|k| (ends[0][k] + ends[1][k]) / 2.0);
            let jitter = |id: u32| coupling::jitter(id as i32);
            let (e0, t0) = coupling::endpoint(ends[0], centre, jitter(*a), &rails);
            let (e1, t1) = coupling::endpoint(ends[1], centre, jitter(*b), &rails);
            let (yaw, pitch, length) = coupling::connector(e0, e1);
            let lights = [light(host, ends[0]), light(host, ends[1])];
            let mut draw = |model: ModelRef, origin: [f64; 3], m: [f32; 12], l: LightCoords| {
                let instance = ModelInstance::new(origin, m, 0xFFFF_FFFF, l, Overlay::NONE);
                if let Err(e) = event.draw.draw_model_instances(model, &[instance]) {
                    crate::create_log!("[create] a coupling could not be drawn: {e:?}");
                }
            };
            for (e, turn_e, l) in [(e0, t0, lights[0]), (e1, t1, lights[1])] {
                draw(attachment, e, turn(turn_e, 0.0, 1.0), l);
                draw(ring, e, turn(yaw, 0.0, 1.0), l);
            }
            // The connector in the mean of both ends' light.
            let mean = LightCoords::new(
                (lights[0].block + lights[1].block) / 2,
                (lights[0].sky + lights[1].sky) / 2,
            )
            .unwrap_or(lights[0]);
            draw(connector, e0, turn(yaw, pitch, length as f32), mean);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_cross_the_wire() {
        let p = vec![(1, 2), (u32::MAX, 7)];
        assert_eq!(decode(&encode(&p)), p);
    }

    #[test]
    fn a_turn_stretches_along_its_own_x() {
        // A quarter turn about up takes x to -z (`rotateYDegrees`), stretched twice.
        let m = turn(90.0, 0.0, 2.0);
        let x = [m[0], m[4], m[8]];
        assert!((x[0]).abs() < 1e-6 && (x[2] + 2.0).abs() < 1e-6, "{x:?}");
    }
}
