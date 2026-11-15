//! The couplings between a train's carriages as clients draw them (`CarriageCouplingRenderer`):
//! a coupling head at the trailing bogey of each carriage and at the leading bogey of the next
//! (`CarriageBogey.couplingAnchors`: a block out along the train from the bogey, 7/32 up), and
//! the cable between them in quarter-block pieces stretched to the gap. The server sends each
//! level's players the anchors when they move; clients ease them between ticks and draw
//! Create's `coupling_head` and `coupling_cable` models after the opaque terrain.

use super::super::net;
use super::super::server::ServerLevel;
use super::super::world::with_world;
use mod_api::{
    ClientHost, Identifier, LightCoords, LightLayer, ModelInstance, ModelRef, Overlay,
    RenderLevelStage, RenderLevelStageEvent,
};
use std::cell::RefCell;
use std::collections::HashMap;

pub const CHANNEL: &str = "create:train_couplings";

/// `AllPartialModels.TRAIN_COUPLING_HEAD` and `TRAIN_COUPLING_CABLE`, as standalone models.
pub const MODELS: [&str; 2] = [
    "create:block/track/bogey/coupling_head",
    "create:block/track/bogey/coupling_cable",
];

/// One coupling: its two anchors and the carriages' spacing (`carriageSpacing`).
type Coupling = ([f64; 3], [f64; 3], f64);

fn encode(list: &[Coupling]) -> Vec<u8> {
    let mut out = Vec::with_capacity(list.len() * 28);
    for (a, b, s) in list {
        for v in a.iter().chain(b).chain([s]) {
            out.extend((*v as f32).to_le_bytes());
        }
    }
    out
}

fn decode(data: &[u8]) -> Vec<Coupling> {
    data.as_chunks::<28>()
        .0
        .iter()
        .map(|c| {
            let f = |i: usize| {
                f64::from(f32::from_le_bytes([
                    c[4 * i],
                    c[4 * i + 1],
                    c[4 * i + 2],
                    c[4 * i + 3],
                ]))
            };
            ([f(0), f(1), f(2)], [f(3), f(4), f(5)], f(6))
        })
        .collect()
}

/// The client's couplings a tick ago and now, and the models.
type Shown = (Vec<Coupling>, Vec<Coupling>, Option<[ModelRef; 2]>);

thread_local! {
    /// The server's: what each level's players were last sent.
    static SENT: RefCell<HashMap<usize, Vec<u8>>> = RefCell::new(HashMap::new());
    /// The client's: the couplings now and a tick ago, and the models.
    static CLIENT: RefCell<Shown> =
        const { RefCell::new((Vec::new(), Vec::new(), None)) };
}

pub fn forget() {
    SENT.with_borrow_mut(HashMap::clear);
}

pub fn reset() {
    CLIENT.with_borrow_mut(|c| {
        c.0.clear();
        c.1.clear();
    });
}

/// The level's couplings, sent to its players when they changed.
pub fn tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let list: Vec<Coupling> = with_world(|w| {
        let r = &w.dims[dim].railway;
        let g = &r.net.graph;
        let mut out = Vec::new();
        for t in r.trains.iter().filter(|t| t.train.bound) {
            let cars = &t.train.carriages;
            for i in 0..cars.len().saturating_sub(1) {
                let trailing = cars[i].first + usize::from(cars[i].two);
                let leading = cars[i + 1].first;
                if leading >= t.train.bogeys.len() {
                    continue;
                }
                let (a, fa) = t.train.bogey_pose(g, trailing);
                let (b, fb) = t.train.bogey_pose(g, leading);
                // Out from each bogey toward the other carriage.
                let up = 7.0 / 32.0;
                let a = [a[0] - fa[0], a[1] - fa[1] + up, a[2] - fa[2]];
                let b = [b[0] + fb[0], b[1] + fb[1] + up, b[2] + fb[2]];
                let spacing = t.train.spacing.get(i).copied().unwrap_or(0);
                out.push((a, b, f64::from(spacing)));
            }
        }
        out
    })
    .unwrap_or_default();
    let data = encode(&list);
    let same = SENT.with_borrow(|s| s.get(&dim).map_or(list.is_empty(), |d| *d == data));
    // Sent again now and then for players who came since.
    if same && (list.is_empty() || lv.game_time() % 100 != 0) {
        return;
    }
    SENT.with_borrow_mut(|s| s.insert(dim, data.clone()));
    let players = lv.players();
    net::send_to_players(lv.host, &players, CHANNEL, &data);
}

pub fn server_message(data: &[u8]) {
    let now = decode(data);
    CLIENT.with_borrow_mut(|c| {
        c.0 = if c.1.len() == now.len() {
            std::mem::take(&mut c.1)
        } else {
            now.clone()
        };
        c.1 = now;
    });
}

pub fn models_baked(host: &dyn ClientHost) {
    let found = MODELS.map(|id| {
        Identifier::parse(id)
            .ok()
            .and_then(|i| host.render_assets().model(&i).ok())
    });
    CLIENT.with_borrow_mut(|c| {
        c.2 = match found {
            [Some(h), Some(k)] => Some([h, k]),
            _ => None,
        }
    });
}

/// A 3×4 row-major affine transform.
type M = [[f64; 4]; 3];

fn mul(a: &M, b: &M) -> M {
    let mut out = [[0.0; 4]; 3];
    for i in 0..3 {
        for j in 0..4 {
            out[i][j] =
                (0..3).map(|k| a[i][k] * b[k][j]).sum::<f64>() + if j == 3 { a[i][3] } else { 0.0 };
        }
    }
    out
}

fn rot_y(deg: f64) -> M {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, 0.0, s, 0.0], [0.0, 1.0, 0.0, 0.0], [-s, 0.0, c, 0.0]]
}

fn rot_x(deg: f64) -> M {
    let (s, c) = deg.to_radians().sin_cos();
    [[1.0, 0.0, 0.0, 0.0], [0.0, c, -s, 0.0], [0.0, s, c, 0.0]]
}

fn shift_z(z: f64) -> M {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, z],
    ]
}

fn stretch_z(k: f64) -> M {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, k, 0.0],
    ]
}

fn flat(m: &M) -> [f32; 12] {
    std::array::from_fn(|i| m[i / 4][i % 4] as f32)
}

fn light(host: &dyn ClientHost, at: [f64; 3]) -> LightCoords {
    let block = at.map(|v| v.floor() as i32);
    host.level(host.dimension())
        .and_then(|l| {
            let b = l.brightness(host, LightLayer::Block, block)?;
            let s = l.brightness(host, LightLayer::Sky, block)?;
            LightCoords::new(b.clamp(0, 15) as u8, s.clamp(0, 15) as u8)
        })
        .unwrap_or(LightCoords::FULL_BRIGHT)
}

/// `CarriageCouplingRenderer.renderAll`.
pub fn render_level_stage(host: &dyn ClientHost, event: &mut RenderLevelStageEvent<'_>) {
    if event.stage != RenderLevelStage::AfterOpaqueBlocks {
        return;
    }
    let t = f64::from(event.view.partial_tick);
    CLIENT.with_borrow(|(prev, now, models)| {
        let Some([head, cable]) = *models else {
            return;
        };
        for (i, (a1, b1, spacing)) in now.iter().enumerate() {
            let (a0, b0) = prev.get(i).map_or((*a1, *b1), |p| (p.0, p.1));
            let lerp = |p: [f64; 3], q: [f64; 3]| [0, 1, 2].map(|k| p[k] + (q[k] - p[k]) * t);
            let (a, b) = (lerp(a0, *a1), lerp(b0, *b1));
            let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let y_rot = d[2].atan2(d[0]).to_degrees() + 90.0;
            let x_rot = d[1].atan2((d[0] * d[0] + d[2] * d[2]).sqrt()).to_degrees();
            let la = light(host, a);
            let lb = light(host, b);
            let mut draw = |model: ModelRef, origin: [f64; 3], m: &M, l: LightCoords| {
                let instance = ModelInstance::new(origin, flat(m), 0xFFFF_FFFF, l, Overlay::NONE);
                let _ = event.draw.draw_model_instances(model, &[instance]);
            };
            draw(head, a, &mul(&rot_y(-y_rot), &rot_x(x_rot)), la);
            // The cable in quarter-block pieces from the first head, stretched to the gap.
            let margin = 3.0 / 16.0;
            let distance = spacing - 2.0 * margin - 2.0;
            let segments = (distance * 4.0).round().max(1.0);
            let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let stretch = ((length - 2.0 * margin) * 4.0) / segments;
            let base = mul(
                &mul(&rot_y(-y_rot + 180.0), &rot_x(-x_rot)),
                &mul(&shift_z(margin + 2.0 / 16.0), &stretch_z(stretch)),
            );
            for j in 0..segments as i32 {
                draw(cable, a, &mul(&base, &shift_z(f64::from(j) / 4.0)), la);
            }
            draw(head, b, &mul(&rot_y(-y_rot + 180.0), &rot_x(-x_rot)), lb);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn couplings_cross_the_wire() {
        let l = vec![([1.0, 2.0, 3.0], [4.5, 5.0, -6.25], 3.0)];
        assert_eq!(decode(&encode(&l)), l);
    }
}
