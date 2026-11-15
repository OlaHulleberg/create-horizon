//! Choosing a frogport's place on a chain with the frogport item
//! (`ChainConveyorInteractionHandler`, `PackagePortTargetSelectionHandler`). On the client, while
//! a frogport is held, the point of a chain conveyor's wheel or chain under the crosshair is
//! outlined (`ChainConveyorShape`: the wheel's box, each chain's oriented box); a use on it
//! chooses it (the server keeps the choice). The next frogport placed takes it, when it is not
//! below the frogport nor more than `packagePortRange` away (`validateDiff`).
//!
//! While a frogport with a chosen point is held, the block a click would place it at is
//! outlined, the point boxed green or red as it may be reached from there, with the reason on
//! the action bar (the client asks its server to say it: `S`), and dust runs from the block to
//! the point; a wrench on a frogport shows its target the same way
//! (`PackagePortTargetSelectionHandler.tick`).

use super::super::net;
use super::super::server::ServerLevel;
use super::{chain, with};
use create_engine::port::Target;
use mod_api::{ClientHost, PlayerRef, ServerHost, Text};
use std::cell::RefCell;
use std::collections::HashMap;

/// Client to server: a point chosen (conveyor, chain position, connection or none, the exact
/// point); server to client: the choice ended.
pub(super) const CHANNEL: &str = "create:frogport_target";

/// `packagePortRange` (Create's default).
const RANGE: f64 = 5.0;

/// A chosen point: the conveyor, its position along the chain (or angle round the wheel), the
/// chain (None: the wheel), and where it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Point {
    pub conveyor: [i32; 3],
    pub chain_pos: f32,
    pub connection: Option<[i32; 3]>,
    pub at: [f64; 3],
}

thread_local! {
    /// The server's: each player's chosen point, by level.
    static CHOSEN: RefCell<HashMap<u128, (usize, Point)>> = RefCell::new(HashMap::new());
    /// Frogports placed with a point, set up at their level's next tick.
    static PENDING: RefCell<Vec<(usize, [i32; 3], Point)>> = const { RefCell::new(Vec::new()) };
    /// The client's: conveyors drawn (their chains), the point under the crosshair, the chosen.
    static CLIENT: RefCell<Client> = RefCell::new(Client::default());
}

#[derive(Default)]
struct Client {
    conveyors: HashMap<[i32; 3], (bool, Vec<[i32; 3]>)>,
    hovered: Option<Point>,
    chosen: Option<Point>,
    /// This tick's selection (`tick`): the point, the block a frogport would go on (if it can
    /// go there), and whether it may reach the point.
    selection: Option<([f64; 3], Option<[i32; 3]>, bool)>,
    /// Whether the chosen point is a station, for a postbox (`isPostbox`: it may reach down).
    postbox: bool,
    /// The status last asked for and when (client ticks), and the ticks counted.
    status: Option<(u8, u64)>,
    ticks: u64,
}

/// `validateDiff`'s answers: may reach, can't reach down, too far.
const STATUS: [(&str, u32); 3] = [
    ("create.package_port.valid", 0x9EDE73),
    ("create.package_port.cannot_reach_down", 0xFF7171),
    ("create.package_port.too_far", 0xFF7171),
];

/// `validateDiff(target, placedPos)`: 0 when it may reach; a postbox may reach down.
fn validate(target: [f64; 3], pos: [i32; 3], postbox: bool) -> u8 {
    let source = [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]),
        f64::from(pos[2]) + 0.5,
    ];
    let diff = [0, 1, 2].map(|i| target[i] - source[i]);
    if diff[1] < 0.0 && !postbox {
        1
    } else if (diff[0] * diff[0] + diff[1] * diff[1] + diff[2] * diff[2]).sqrt() > RANGE {
        2
    } else {
        0
    }
}

/// `animateConnection`: four dust specks of `rgb` running from `source` to `target` over ten
/// ticks, a third as fast as the render time.
fn animate_connection(
    host: &mut dyn ClientHost,
    ticks: u64,
    source: [f64; 3],
    target: [f64; 3],
    rgb: u32,
) {
    let Some(&key) = super::DUST_PARTICLE.get() else {
        return;
    };
    let colour = [16, 8, 0].map(|s| ((rgb >> s) & 0xFF) as f32 / 255.0);
    let options = mod_api::ParticleOptions::dust(key, colour, 1.0);
    let total = 10.0;
    let segments = 10 / 3 + 1;
    let offset = total / f64::from(segments);
    for i in 0..segments {
        let t = ((ticks as f64 / 3.0) % offset) + f64::from(i) * offset;
        let v = [0, 1, 2].map(|k| source[k] + (target[k] - source[k]) * t / total);
        host.add_particle(&options, v, [0.0; 3]);
    }
}

/// `PackagePortTargetSelectionHandler.tick`: a wrench on a frogport shows its target; a
/// frogport held with a point chosen shows where it would go and whether it reaches.
fn selection_tick(host: &mut dyn ClientHost, player: PlayerRef) {
    let held = super::super::items::held(host, player, mod_api::Hand::Main);
    let id = held.as_ref().map(|s| s.item().id(host)).unwrap_or_default();
    let wrench = id == "create:wrench";
    let ticks = CLIENT.with_borrow_mut(|c| {
        c.ticks += 1;
        c.selection = None;
        c.ticks
    });
    let (chosen, postbox) = CLIENT.with_borrow(|c| (c.chosen, c.postbox));
    let holds_postbox = id.starts_with("create:") && id.ends_with("_postbox");
    let held_kind = if postbox {
        holds_postbox
    } else {
        id == "create:package_frogport"
    };
    if !wrench && (chosen.is_none() || !held_kind) {
        return;
    }
    let Some(mod_api::CrosshairTarget::Block { pos, hit, .. }) = host.crosshair_target() else {
        // Looking at nothing: the point, green.
        if !wrench && let Some(c) = chosen {
            CLIENT.with_borrow_mut(|cl| cl.selection = Some((c.at, None, true)));
        }
        return;
    };
    let mut pos = [pos.x, pos.y, pos.z];
    let Some(level) = host.level(host.dimension()) else {
        return;
    };
    if wrench {
        // `getExactTargetLocation` of the frogport looked at, from its data.
        let target = host
            .block_entity(level, pos)
            .and_then(|e| e.data(host))
            .and_then(|d| match d.get(&super::frog_draw::FROG.into()) {
                Some(mod_api::Tag::Compound(f)) => match f.get(&"Target".into()) {
                    Some(mod_api::Tag::List(l)) if l.len() == 3 => {
                        let v: Vec<f64> = l
                            .iter()
                            .map(|t| match t {
                                mod_api::Tag::Double(d) => f64::from_bits(*d),
                                _ => 0.0,
                            })
                            .collect();
                        Some([v[0], v[1], v[2]])
                    }
                    _ => None,
                },
                _ => None,
            });
        if let Some(target) = target {
            let source = [
                f64::from(pos[0]) + 0.5,
                f64::from(pos[1]),
                f64::from(pos[2]) + 0.5,
            ];
            animate_connection(host, ticks, source, target, STATUS[0].1);
            CLIENT.with_borrow_mut(|c| c.selection = Some((target, None, true)));
        }
        return;
    }
    let Some(chosen) = chosen else { return };
    let replaceable = |p: [i32; 3]| {
        level.block_state(host, p).is_some_and(|s| {
            mod_api::HostBlockStates::block_states(&*host).replaceable(s) == Some(true)
        })
    };
    if !replaceable(pos) {
        let o = hit.face;
        pos = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
    }
    let code = validate(chosen.at, pos, postbox);
    let refresh = CLIENT.with_borrow_mut(|c| {
        let due = c
            .status
            .is_none_or(|(last, at)| last != code || ticks - at >= 20);
        if due {
            c.status = Some((code, ticks));
        }
        due
    });
    let free = replaceable(pos);
    if refresh {
        net::send_to_server(host, CHANNEL, &[b'S', code]);
    }
    CLIENT.with_borrow_mut(|c| c.selection = Some((chosen.at, free.then_some(pos), code == 0)));
    if free {
        let source = [
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]),
            f64::from(pos[2]) + 0.5,
        ];
        animate_connection(host, ticks, source, chosen.at, STATUS[usize::from(code)].1);
    }
}

pub(super) fn reset_server() {
    CHOSEN.with_borrow_mut(HashMap::clear);
    PENDING.with_borrow_mut(Vec::clear);
}

pub(super) fn reset_client() {
    CLIENT.with_borrow_mut(|c| *c = Client::default());
}

/// A conveyor drawn this frame, with its chains.
pub(super) fn seen(pos: [i32; 3], reversed: bool, connections: Vec<[i32; 3]>) {
    CLIENT.with_borrow_mut(|c| {
        c.conveyors.insert(pos, (reversed, connections));
    });
}

fn put_f(m: &mut Vec<u8>, v: f64) {
    m.extend(v.to_le_bytes());
}

fn put_i(m: &mut Vec<u8>, v: i32) {
    m.extend(v.to_le_bytes());
}

// ---------------------------------------------------------------------------------------------
// Shapes

/// Catnip's `VecHelper.rotate` about X (`x`) or Y.
fn rotate(v: [f64; 3], deg: f64, x: bool) -> [f64; 3] {
    let (s, c) = deg.to_radians().sin_cos();
    let [a, b, d] = v;
    if x {
        [a, b * c - d * s, d * c + b * s]
    } else {
        [a * c + d * s, b, d * c - a * s]
    }
}

/// `AABB.clip`: where the segment from `from` to `to` enters the box.
fn clip(lo: [f64; 3], hi: [f64; 3], from: [f64; 3], to: [f64; 3]) -> Option<[f64; 3]> {
    let d = [0, 1, 2].map(|i| to[i] - from[i]);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for i in 0..3 {
        if d[i].abs() < 1e-12 {
            if from[i] < lo[i] || from[i] > hi[i] {
                return None;
            }
            continue;
        }
        let (a, b) = ((lo[i] - from[i]) / d[i], (hi[i] - from[i]) / d[i]);
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }
    Some([0, 1, 2].map(|i| from[i] + d[i] * t0))
}

/// `ChainConveyorOBB`: a chain's box in its own frame (`pivot` the start, `yaw`, `pitch`).
struct Obb {
    connection: [i32; 3],
    pivot: [f64; 3],
    yaw: f64,
    pitch: f64,
    length: f64,
}

const OBB_RADIUS: f64 = 0.175;

impl Obb {
    fn new(connection: [i32; 3], start: [f64; 3], end: [f64; 3]) -> Self {
        let d = [0, 1, 2].map(|i| end[i] - start[i]);
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let flat = (d[0] * d[0] + d[2] * d[2]).sqrt();
        Obb {
            connection,
            pivot: start,
            yaw: d[0].atan2(d[2]).to_degrees(),
            pitch: (-d[1]).atan2(flat).to_degrees(),
            length,
        }
    }

    fn counter(&self, v: [f64; 3]) -> [f64; 3] {
        let v = [0, 1, 2].map(|i| v[i] - self.pivot[i]);
        let v = rotate(v, -self.yaw, false);
        let v = rotate(v, -self.pitch, true);
        [0, 1, 2].map(|i| v[i] + self.pivot[i])
    }

    fn transform(&self, v: [f64; 3]) -> [f64; 3] {
        let v = [0, 1, 2].map(|i| v[i] - self.pivot[i]);
        let v = rotate(v, self.pitch, true);
        let v = rotate(v, self.yaw, false);
        [0, 1, 2].map(|i| v[i] + self.pivot[i])
    }

    fn intersect(&self, from: [f64; 3], to: [f64; 3]) -> Option<[f64; 3]> {
        let p = self.pivot;
        let lo = [p[0] - OBB_RADIUS, p[1] - OBB_RADIUS, p[2]];
        let hi = [p[0] + OBB_RADIUS, p[1] + OBB_RADIUS, p[2] + self.length];
        clip(lo, hi, self.counter(from), self.counter(to)).map(|r| self.transform(r))
    }

    /// `getChainPosition`: a whole number of blocks along, within its margins.
    fn chain_position(&self, hit: [f64; 3]) -> f32 {
        let c = self.connection.map(f64::from);
        let dots = ((c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt() - 3.0).round();
        let d: f64 = (0..3).map(|i| (hit[i] - self.pivot[i]).powi(2)).sum();
        let selection = d.sqrt().min(self.length);
        let margin = self.length - dots;
        let selection = (selection - margin).clamp(0.0, (self.length - margin * 2.0).max(0.0));
        (selection.round() + margin + 0.025) as f32
    }

    /// `getVec`: the point `position` along the chain's centre line.
    fn vec(&self, position: f32) -> [f64; 3] {
        let t = (f64::from(position) / self.length.max(1e-9)).clamp(0.0, 1.0);
        let local = [
            self.pivot[0],
            self.pivot[1],
            self.pivot[2] + self.length * t,
        ];
        self.transform(local)
    }
}

/// `ChainConveyorBB`: the wheel's box (the block's bottom centre, half a block up, a block out).
fn wheel_box() -> ([f64; 3], [f64; 3]) {
    ([-0.5, 0.0, -0.5], [1.5, 0.5, 1.5])
}

/// `ChainConveyorBB.getChainPosition`: the angle round the wheel, to the nearest 45.
fn wheel_position(hit: [f64; 3]) -> f32 {
    let c = [0.5, 0.25, 0.5];
    let d = [c[0] - hit[0], c[2] - hit[2]];
    let angle = (d[0].atan2(d[1]).to_degrees() + 360.0 + 180.0).rem_euclid(360.0);
    ((angle / 45.0).round() * 45.0) as f32
}

/// `ChainConveyorBB.getVec`.
fn wheel_vec(position: f32) -> [f64; 3] {
    let v = rotate([0.0, 0.0, 0.875], f64::from(position), false);
    [0.5 + v[0], 0.25 + v[1] - 0.125, 0.5 + v[2]]
}

// ---------------------------------------------------------------------------------------------
// Client

/// `ChainConveyorInteractionHandler.clientTick` while a frogport is in the main hand.
pub(super) fn client_tick(host: &mut dyn ClientHost) {
    let Some(player) = host.local_player() else {
        return;
    };
    let holding = super::super::items::held(host, player, mod_api::Hand::Main)
        .is_some_and(|s| s.item().id(host) == "create:package_frogport");
    // Conveyors no longer there are forgotten.
    let level = host.level(host.dimension());
    CLIENT.with_borrow_mut(|c| {
        c.conveyors.retain(|p, _| {
            level
                .and_then(|l| l.block_state(host, *p))
                .and_then(super::states::id)
                .and_then(|id| {
                    super::super::client_table(host, |t| t.get(id).map(|(b, _)| b)).flatten()
                })
                == Some(create_engine::Block::ChainConveyor)
        });
        c.hovered = None;
    });
    selection_tick(host, player);
    if !holding {
        return;
    }
    // The camera (`getEyePosition`, `getViewVector`).
    let Some(cam) = host.camera() else {
        return;
    };
    let eye = cam.position;
    let (yaw, pitch) = (
        f64::from(cam.yaw).to_radians(),
        f64::from(cam.pitch).to_radians(),
    );
    let look = [
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    ];

    let range = 5.5;
    let to = [0, 1, 2].map(|i| eye[i] + look[i] * range);
    let mut best = match host.crosshair_target() {
        Some(mod_api::CrosshairTarget::Block { hit, .. }) => {
            (0..3).map(|i| (hit.point[i] - eye[i]).powi(2)).sum::<f64>()
        }
        _ => f64::MAX,
    };
    let conveyors: Vec<([i32; 3], bool, Vec<[i32; 3]>)> = CLIENT.with_borrow(|c| {
        c.conveyors
            .iter()
            .map(|(p, (r, k))| (*p, *r, k.clone()))
            .collect()
    });
    let mut found = None;
    for (pos, reversed, connections) in conveyors {
        let base = pos.map(f64::from);
        let from_l = [0, 1, 2].map(|i| eye[i] - base[i]);
        let to_l = [0, 1, 2].map(|i| to[i] - base[i]);
        let (lo, hi) = wheel_box();
        let mut consider = |hit: [f64; 3], point: Point| {
            let w = [0, 1, 2].map(|i| hit[i] + base[i]);
            let d: f64 = (0..3).map(|i| (w[i] - eye[i]).powi(2)).sum();
            if d > best {
                return;
            }
            best = d;
            found = Some(point);
        };
        if let Some(hit) = clip(lo, hi, from_l, to_l) {
            let p = wheel_position(hit);
            let v = wheel_vec(p);
            consider(
                hit,
                Point {
                    conveyor: pos,
                    chain_pos: p,
                    connection: None,
                    at: [0, 1, 2].map(|i| v[i] + base[i]),
                },
            );
        }
        for k in connections {
            let s = chain::connection_stats([0, 0, 0], k, reversed);
            let obb = Obb::new(k, s.start, s.end);
            if let Some(hit) = obb.intersect(from_l, to_l) {
                let p = obb.chain_position(hit);
                let v = obb.vec(p);
                consider(
                    hit,
                    Point {
                        conveyor: pos,
                        chain_pos: p,
                        connection: Some(k),
                        at: [0, 1, 2].map(|i| v[i] + base[i]),
                    },
                );
            }
        }
    }
    CLIENT.with_borrow_mut(|c| c.hovered = found);
}

/// `ChainConveyorInteractionHandler.onUse` with a frogport: the point under the crosshair is
/// chosen. Whether there was one (the use is taken).
pub(super) fn use_on_chain(host: &mut dyn ClientHost) -> bool {
    let Some(p) = CLIENT.with_borrow(|c| c.hovered) else {
        return false;
    };
    CLIENT.with_borrow_mut(|c| {
        c.chosen = Some(p);
        c.postbox = false;
    });
    let mut m = Vec::new();
    for v in p.conveyor {
        put_i(&mut m, v);
    }
    m.extend(p.chain_pos.to_le_bytes());
    match p.connection {
        Some(k) => {
            m.push(1);
            for v in k {
                put_i(&mut m, v);
            }
        }
        None => m.push(0),
    }
    for v in p.at {
        put_f(&mut m, v);
    }
    net::send_to_server(host, CHANNEL, &m);
    true
}

/// The server ended the choice (a frogport was placed).
pub(super) fn server_message(channel: &str) -> bool {
    if channel != CHANNEL {
        return false;
    }
    CLIENT.with_borrow_mut(|c| c.chosen = None);
    true
}

/// The point under the crosshair (white) and the chosen one (green), as small boxes.
pub(super) fn render_level_stage(
    host: &dyn ClientHost,
    event: &mut mod_api::RenderLevelStageEvent<'_>,
) {
    if event.stage != mod_api::RenderLevelStage::AfterTranslucentBlocks {
        return;
    }
    let held = host
        .local_player()
        .and_then(|p| super::super::items::held(host, p, mod_api::Hand::Main))
        .map(|s| s.item().id(host));
    let postbox = CLIENT.with_borrow(|c| c.postbox);
    let holding = match held.as_deref() {
        Some(id) if postbox => id.starts_with("create:") && id.ends_with("_postbox"),
        id => id == Some("create:package_frogport"),
    };
    let (hovered, chosen, selection) = CLIENT.with_borrow(|c| (c.hovered, c.chosen, c.selection));
    let mut lines = Vec::new();
    let point = |lines: &mut Vec<_>, at: [f64; 3], color: u32| {
        let r = 1.0 / 12.0;
        crate::glue::logistics::client::box_lines(
            lines,
            at.map(|v| v - r),
            at.map(|v| v + r),
            color,
        );
    };
    // `ChainPointSelected` (green, or red where it can't be reached from the block looked at)
    // and `TargetedFrogPos`: the block's floor, an eighth in from each side.
    if let Some((at, block, ok)) = selection {
        let color = if ok { 0xFF9E_DE73 } else { 0xFFFF_7171 };
        point(&mut lines, at, color);
        if let Some(b) = block {
            let b = b.map(f64::from);
            crate::glue::logistics::client::box_lines(
                &mut lines,
                [b[0] + 0.125, b[1], b[2] + 0.125],
                [b[0] + 0.875, b[1], b[2] + 0.875],
                color,
            );
        }
    } else if holding && let Some(p) = chosen {
        point(&mut lines, p.at, 0xFF9E_DE73);
    }
    if holding && let Some(p) = hovered {
        point(&mut lines, p.at, 0xFFFF_FFFF);
    }
    if !lines.is_empty() {
        let _ = event.draw.lines(&lines);
    }
}

// ---------------------------------------------------------------------------------------------
// Server

/// A point chosen by a player's client.
/// `onUse` with a postbox on the station at `pos` (a station of the client's: the server checks
/// it is one): chosen, its centre the point. Whether it was a station.
pub(super) fn choose_station(host: &mut dyn ClientHost, pos: [i32; 3]) -> bool {
    let Some(level) = host.level(host.dimension()) else {
        return false;
    };
    let station = level
        .block_state(&*host, pos)
        .is_some_and(|s| s.block() == super::states::key(create_engine::Block::TrackStation));
    if !station {
        return false;
    }
    let point = Point {
        conveyor: pos,
        chain_pos: 0.0,
        connection: None,
        at: pos.map(|v| f64::from(v) + 0.5),
    };
    CLIENT.with_borrow_mut(|c| {
        c.chosen = Some(point);
        c.postbox = true;
    });
    let mut m = vec![b'P'];
    for v in pos {
        put_i(&mut m, v);
    }
    net::send_to_server(host, CHANNEL, &m);
    true
}

pub(super) fn client_message(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    // A station chosen with a postbox (`P`).
    if data.len() == 13 && data[0] == b'P' {
        let v =
            |i: usize| i32::from_le_bytes(data[1 + i * 4..5 + i * 4].try_into().unwrap_or([0; 4]));
        let pos = [v(0), v(1), v(2)];
        let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
            return;
        };
        let Some(lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        if super::block_of(lv.block(pos)) == Some(create_engine::Block::TrackStation) {
            super::station_ports::choose(player, lv.dim, pos);
        }
        return;
    }
    // The status line a client asks for (`sendStatus`).
    if let [b'S', code] = *data {
        if let Some((key, rgb)) = STATUS.get(usize::from(code)) {
            let mut style = mod_api::text::TextStyle::default();
            style.color = Some(format!("#{rgb:06x}"));
            let _ = player.action_bar(host, Text::translate(*key, Vec::new()).with_style(style));
        }
        return;
    }
    let mut d = data;
    let mut take = |n: usize| -> Option<&[u8]> {
        if d.len() < n {
            return None;
        }
        let (a, b) = d.split_at(n);
        d = b;
        Some(a)
    };
    let mut int = || -> Option<i32> { Some(i32::from_le_bytes(take(4)?.try_into().ok()?)) };
    let (Some(x), Some(y), Some(z)) = (int(), int(), int()) else {
        return;
    };
    let mut take = |n: usize| -> Option<&[u8]> {
        if d.len() < n {
            return None;
        }
        let (a, b) = d.split_at(n);
        d = b;
        Some(a)
    };
    let Some(chain_pos) = take(4).and_then(|b| Some(f32::from_le_bytes(b.try_into().ok()?))) else {
        return;
    };
    let Some(&[flag]) = take(1) else { return };
    let mut ints = Vec::new();
    if flag == 1 {
        for _ in 0..3 {
            let Some(v) = take(4).and_then(|b| Some(i32::from_le_bytes(b.try_into().ok()?))) else {
                return;
            };
            ints.push(v);
        }
    }
    let mut at = [0.0; 3];
    for v in &mut at {
        let Some(f) = take(8).and_then(|b| Some(f64::from_le_bytes(b.try_into().ok()?))) else {
            return;
        };
        *v = f;
    }
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return;
    };
    let Some(dim) = super::level_index(&dimension) else {
        return;
    };
    let point = Point {
        conveyor: [x, y, z],
        chain_pos: if chain_pos.is_finite() {
            chain_pos
        } else {
            0.0
        },
        connection: (ints.len() == 3).then(|| [ints[0], ints[1], ints[2]]),
        at,
    };
    CHOSEN.with_borrow_mut(|c| c.insert(player.uuid(), (dim, point)));
}

/// `PackagePortPlacementPacket` on a frogport placed by `player` at `pos`: its chosen point,
/// checked (`validateDiff`), set up at the level's next tick; or why not.
pub(super) fn placed(host: &mut dyn ServerHost, player: PlayerRef, dim: usize, pos: [i32; 3]) {
    let chosen = CHOSEN.with_borrow_mut(|c| c.remove(&player.uuid()));
    net::send_to_player(host, player, CHANNEL, &[]);
    let say = |host: &mut dyn ServerHost, key: &str, red: bool| {
        let mut style = mod_api::text::TextStyle::default();
        if red {
            style.color = Some("#ff7171".to_owned());
        }
        let _ = player.action_bar(host, Text::translate(key, Vec::new()).with_style(style));
    };
    let Some((d, point)) = chosen.filter(|c| c.0 == dim) else {
        say(
            host,
            "create.gui.package_port.not_targeting_anything",
            false,
        );
        return;
    };
    let _ = d;
    let source = [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]),
        f64::from(pos[2]) + 0.5,
    ];
    let diff = [0, 1, 2].map(|i| point.at[i] - source[i]);
    if diff[1] < 0.0 {
        say(host, "create.package_port.cannot_reach_down", true);
        return;
    }
    if (diff[0] * diff[0] + diff[1] * diff[1] + diff[2] * diff[2]).sqrt() > RANGE {
        say(host, "create.package_port.too_far", true);
        return;
    }
    PENDING.with_borrow_mut(|p| p.push((dim, pos, point)));
}

/// Frogports placed with a point: set up on it, now their level knows them.
pub(super) fn level_tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mine: Vec<([i32; 3], Point)> = PENDING.with_borrow_mut(|p| {
        let (mine, rest): (Vec<_>, Vec<_>) =
            std::mem::take(p).into_iter().partition(|x| x.0 == dim);
        *p = rest;
        mine.into_iter().map(|(_, pos, pt)| (pos, pt)).collect()
    });
    for (pos, point) in mine {
        with(dim, |pk| {
            if !pk.ports.get(&pos).is_some_and(|p| p.frog) {
                return;
            }
            let Some(c) = pk.chains.conveyors.get(&point.conveyor) else {
                return;
            };
            if point
                .connection
                .is_some_and(|k| !c.connections.contains(&k))
            {
                return;
            }
            let flipped = c.speed < 0.0;
            if let Some(port) = pk.ports.get_mut(&pos) {
                port.target = Some(Target {
                    conveyor: point.conveyor,
                    chain_pos: point.chain_pos,
                    connection: point.connection,
                    flipped,
                });
            }
            pk.register(pos);
            pk.wake(pos);
            pk.touch(pos);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_segment_enters_a_box_on_its_face() {
        let hit = clip([0.0; 3], [1.0; 3], [-1.0, 0.5, 0.5], [2.0, 0.5, 0.5]).unwrap();
        assert!((hit[0] - 0.0).abs() < 1e-9);
        assert!(clip([0.0; 3], [1.0; 3], [-1.0, 2.0, 0.5], [2.0, 2.0, 0.5]).is_none());
    }

    #[test]
    fn a_chain_is_hit_along_its_length() {
        // A chain running east from the origin, 4 blocks.
        let obb = Obb::new([6, 0, 0], [0.0, 0.0, 0.0], [4.0, 0.0, 0.0]);
        let hit = obb
            .intersect([2.0, 1.0, 0.0], [2.0, -1.0, 0.0])
            .expect("crosses the chain");
        assert!((hit[0] - 2.0).abs() < 1e-6 && (hit[1] - OBB_RADIUS).abs() < 1e-6);
        let v = obb.vec(2.0);
        assert!((v[0] - 2.0).abs() < 1e-6 && v[2].abs() < 1e-6, "{v:?}");
    }
}
