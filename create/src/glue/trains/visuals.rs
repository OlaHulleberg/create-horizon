//! Trains as players see them (`CarriageContraptionVisual`, `StandardBogeyVisual`): each block
//! a carriage carries is a vanilla block display the server keeps where the block is, turned
//! with the carriage, and each bogey's frame and wheels (Create's partials, as the hidden
//! `create:part_bogey_*` blocks) displays posed with the bogey. Clients draw them with the
//! game's own block models and light, gliding between ticks (`teleport_duration`).
//!
//! A carriage has its displays while the chunk under it is loaded (`Carriage.manageEntities`
//! spawns its entity the same way); they go when it leaves, when the train goes, and with the
//! world. They are never saved as Create's: one that comes back with its chunk is discarded and
//! made again from the train.

use super::super::server::ServerLevel;
use super::super::world::with_world;
use create_engine::train_controls::Frame;
use create_engine::trains::{BogeySize, yaw_pitch};
use mod_api::{Compound, ServerHost, Tag, Transformation};
use std::cell::RefCell;
use std::collections::HashMap;

/// The tag on the displays.
pub const TAG: &str = "create.carriage";

/// Ticks a display glides to where it was moved (`Display.teleport_duration`).
const GLIDE: i32 = 2;

/// What one carriage or bogey shows: its displays, each with its pose in the carriage (or
/// bogey) and where they were last put.
struct Shown {
    displays: Vec<(u32, Part)>,
    last: Option<([f64; 3], f32, f32)>,
    /// A bogey's wheels' turn (`CarriageBogey.wheelAngle`), degrees, and the radius they
    /// roll on.
    wheel: f64,
    radius: f64,
}

/// A display's place: its point in the carriage's (or bogey's) space, from which the display
/// stands, turned with it; its model's offset and turn there, and whether it is a wheel,
/// turning about its axle as the bogey rolls.
#[derive(Clone, Copy)]
struct Part {
    at: [f64; 3],
    translation: [f32; 3],
    left: [f32; 4],
    wheel: bool,
}

/// What a carriage or bogey is: `(level, train, false, carriage)` or `(level, train, true,
/// bogey)`.
type Key = (usize, u32, bool, usize);

thread_local! {
    static SHOWN: RefCell<HashMap<Key, Shown>> = RefCell::new(HashMap::new());
    /// Displays tagged as ours that joined a level, for its next tick.
    static JOINED: RefCell<Vec<(usize, u32)>> = const { RefCell::new(Vec::new()) };
}

/// The world closed: its displays are saved with their chunks and discarded when they return.
pub fn forget() {
    SHOWN.with_borrow_mut(HashMap::clear);
    JOINED.with_borrow_mut(Vec::clear);
}

/// A display tagged as ours joined level `dim`.
pub fn joined(dim: usize, id: u32) {
    JOINED.with_borrow_mut(|j| j.push((dim, id)));
}

/// One carriage or bogey as this tick has it.
struct Now {
    key: Key,
    /// Where it stands (a carriage's pivot, a bogey's anchor), and its rotation's yaw and
    /// pitch as a display takes them.
    at: [f64; 3],
    yaw: f32,
    pitch: f32,
    /// The rotation from the display's turn to the carriage's (its left rotation's half).
    turn: f64,
    /// The way it runs (its front).
    front: [f64; 3],
    loaded: bool,
}

/// The heading's quarter `a` (degrees): `Ry(a)` turns +x onto the train's side (`heading`
/// × up), about which the carriage tilts.
fn side_turn(heading: [i32; 2]) -> f64 {
    (-f64::from(heading[0]))
        .atan2(-f64::from(heading[1]))
        .to_degrees()
}

/// The display's turn for a frame turned `yaw` (counterclockwise looking down) and tilted
/// `pitch` about the side of `heading`: `Ry(yaw + a) Rx(pitch)` with the entity's yaw turning
/// the other way.
fn display_angles(yaw: f64, pitch: f64, heading: [i32; 2]) -> (f32, f32) {
    let y = -(yaw + side_turn(heading));
    (((y + 540.0).rem_euclid(360.0) - 180.0) as f32, pitch as f32)
}

/// `v` turned `deg` about up (`Ry`).
fn turn_y(v: [f64; 3], deg: f64) -> [f64; 3] {
    let (s, c) = deg.to_radians().sin_cos();
    [v[0] * c + v[2] * s, v[1], v[2] * c - v[0] * s]
}

/// `v` turned `deg` about the x axis (`Rx`).
fn turn_x(v: [f64; 3], deg: f64) -> [f64; 3] {
    let (s, c) = deg.to_radians().sin_cos();
    [v[0], v[1] * c - v[2] * s, v[1] * s + v[2] * c]
}

/// The quaternion of `deg` about the x axis, `[x, y, z, w]`.
fn quat_x(deg: f64) -> [f32; 4] {
    let (s, c) = (deg.to_radians() / 2.0).sin_cos();
    [s as f32, 0.0, 0.0, c as f32]
}

/// The quaternion of `deg` about up, `[x, y, z, w]`.
fn quat_y(deg: f64) -> [f32; 4] {
    let (s, c) = (deg.to_radians() / 2.0).sin_cos();
    [0.0, s as f32, 0.0, c as f32]
}

/// Where a display of `part` stands for `now`: the point turned as the display turns it.
fn place(now: &Now, part: Part) -> [f64; 3] {
    let v = turn_y(turn_x(part.at, f64::from(now.pitch)), -f64::from(now.yaw));
    std::array::from_fn(|i| now.at[i] + v[i])
}

/// The level's tick, after the trains moved: carriages and bogeys that came into loaded chunks
/// get their displays, those that moved move them, and those gone or out of reach lose them.
pub fn tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let joined: Vec<u32> = JOINED.with_borrow_mut(|j| {
        let (here, rest): (Vec<_>, Vec<_>) =
            std::mem::take(j).into_iter().partition(|e| e.0 == dim);
        *j = rest;
        here.into_iter().map(|e| e.1).collect()
    });
    for id in joined {
        let ours =
            SHOWN.with_borrow(|s| s.values().any(|v| v.displays.iter().any(|(d, _)| *d == id)));
        if !ours && let Some(e) = lv.host.entity(id) {
            e.discard(lv.host);
        }
    }
    let empty = SHOWN.with_borrow(|s| !s.keys().any(|k| k.0 == dim));
    let any =
        with_world(|w| w.dims[dim].railway.trains.iter().any(|t| t.train.bound)).unwrap_or(false);
    if empty && !any {
        return;
    }
    let mut now: Vec<Now> = with_world(|w| {
        let r = &w.dims[dim].railway;
        let g = &r.net.graph;
        let mut out = Vec::new();
        for t in r.trains.iter().filter(|t| t.train.bound) {
            let a = side_turn(t.heading);
            for c in 0..t.train.carriages.len().min(t.cars.len()) {
                let (pivot, front) = t.train.carriage_pose(g, c);
                let f = Frame::new(pivot, front, t.heading);
                let (yaw, pitch) = display_angles(f.yaw, f.pitch, t.heading);
                out.push(Now {
                    key: (dim, t.id, false, c),
                    at: pivot,
                    yaw,
                    pitch,
                    turn: -a,
                    front,
                    loaded: false,
                });
            }
            for b in 0..t.train.bogeys.len() {
                let (anchor, front) = t.train.bogey_pose(g, b);
                let (yaw, pitch) = yaw_pitch(front, t.heading);
                let (yaw, pitch) = display_angles(yaw, pitch, t.heading);
                out.push(Now {
                    key: (dim, t.id, true, b),
                    at: anchor,
                    yaw,
                    pitch,
                    // The partials run along z; the heading's turn about up from there.
                    turn: f64::from(t.heading[0])
                        .atan2(f64::from(t.heading[1]))
                        .to_degrees()
                        - a,
                    front,
                    loaded: false,
                });
            }
        }
        out
    })
    .unwrap_or_default();
    for n in &mut now {
        n.loaded = lv.is_loaded(n.at.map(|v| v.floor() as i32));
    }
    // Gone, or out of loaded chunks: their displays go.
    let gone: Vec<Key> = SHOWN.with_borrow(|s| {
        s.keys()
            .filter(|k| k.0 == dim)
            .filter(|k| !now.iter().any(|n| n.key == **k && n.loaded))
            .copied()
            .collect()
    });
    for k in gone {
        if let Some(shown) = SHOWN.with_borrow_mut(|s| s.remove(&k)) {
            for (id, _) in shown.displays {
                if let Some(e) = lv.host.entity(id) {
                    e.discard(lv.host);
                }
            }
        }
    }
    for n in now.iter().filter(|n| n.loaded) {
        let known = SHOWN.with_borrow(|s| s.contains_key(&n.key));
        if !known {
            let shown = spawn(lv, n);
            SHOWN.with_borrow_mut(|s| s.insert(n.key, shown));
            continue;
        }
        let pose = (n.at, n.yaw, n.pitch);
        let moved = SHOWN.with_borrow(|s| s.get(&n.key).is_some_and(|v| v.last != Some(pose)));
        if !moved {
            continue;
        }
        let (displays, wheel) = SHOWN.with_borrow_mut(|s| {
            let v = s.get_mut(&n.key).expect("known");
            // `CarriageBogey.updateAngles`: the wheels turn by the distance rolled, forward
            // along the bogey's way, over their circumference.
            if let Some((was, _, _)) = v.last
                && v.radius > 0.0
            {
                let d: [f64; 3] = std::array::from_fn(|i| n.at[i] - was[i]);
                let along = d[0] * n.front[0] + d[1] * n.front[1] + d[2] * n.front[2];
                v.wheel = (v.wheel + (along / v.radius).to_degrees()).rem_euclid(360.0);
            }
            v.last = Some(pose);
            (v.displays.clone(), v.wheel)
        });
        for (id, part) in displays {
            let Some(e) = lv.host.entity(id) else {
                continue;
            };
            e.teleport_to(lv.host, place(n, part), Some([n.yaw, n.pitch]));
            if part.wheel
                && let Some(d) = lv.host.display(e)
            {
                let mut t = Transformation::IDENTITY;
                t.translation = part.translation;
                t.left_rotation = part.left;
                t.right_rotation = quat_x(wheel);
                d.set_start_interpolation(lv.host, 0);
                d.set_interpolation_duration(lv.host, 1);
                d.set_transformation(lv.host, t);
            }
        }
    }
}

/// A display to make: its block and properties, its point in the carriage's space, and its
/// model's corner from there.
type Shape = (String, Vec<(String, String)>, [f64; 3], [f64; 3], bool);

/// The displays of a carriage or bogey, made where it stands now.
fn spawn(lv: &mut ServerLevel<'_>, n: &Now) -> Shown {
    let (_, train, bogey, i) = n.key;
    let dim = lv.dim;
    // Each display: its state, its point in the carriage's space, and its own turn about that
    // point (the partials' quarter) with the offset of its model's origin from the point.
    let mut parts: Vec<Shape> = Vec::new();
    let mut radius = 0.0;
    if bogey {
        let Some(size) = with_world(|w| {
            let t = w.dims[dim].railway.trains.iter().find(|t| t.id == train)?;
            t.train.bogeys.get(i).copied()
        })
        .flatten() else {
            return Shown {
                displays: Vec::new(),
                last: None,
                wheel: 0.0,
                radius: 0.0,
            };
        };
        radius = size.wheel_radius();
        // `StandardBogeyVisual`: a small bogey's frame with a wheel 12 px up a block before
        // and behind; a large one's drive with its wheels a block up.
        let (frame, wheel, wheels): (&str, &str, &[[f64; 3]]) = match size {
            BogeySize::Small => (
                "create:part_bogey_frame",
                "create:part_bogey_wheel",
                &[[0.0, 0.75, -1.0], [0.0, 0.75, 1.0]],
            ),
            BogeySize::Large => (
                "create:part_bogey_drive",
                "create:part_bogey_drive_wheel",
                &[[0.0, 1.0, 0.0]],
            ),
        };
        let up = vec![("facing".to_owned(), "up".to_owned())];
        parts.push((frame.to_owned(), up.clone(), [0.0; 3], [0.0; 3], false));
        for w in wheels {
            parts.push((wheel.to_owned(), up.clone(), *w, [0.0; 3], true));
        }
    } else {
        let blocks = with_world(|w| {
            let t = w.dims[dim].railway.trains.iter().find(|t| t.id == train)?;
            t.cars.get(i).cloned()
        })
        .flatten()
        .unwrap_or_default();
        for (o, s) in blocks {
            let Some(state) = super::car_state(s) else {
                continue;
            };
            let Some((name, props)) = super::state_parts(&*lv.host, state) else {
                continue;
            };
            if name == "minecraft:air" {
                continue;
            }
            // `Frame::to_world`: block `o` spans `o` to `o + 1`, the bogey's block a block
            // over the pivot, its centre over it.
            let centre = [f64::from(o[0]), f64::from(o[1]) + 1.5, f64::from(o[2])];
            parts.push((name, props, centre, [-0.5; 3], false));
        }
    }
    let mut displays = Vec::new();
    for (name, props, at, origin, wheel) in parts {
        // The part's point in the display's turned space (the carriage's space turned back by
        // the heading's quarter), the model's corner from there, and the quarter as the
        // display's left rotation.
        let part = Part {
            at: turn_y(at, n.turn),
            translation: turn_y(origin, n.turn).map(|v| v as f32),
            left: quat_y(n.turn),
            wheel,
        };
        let nbt = display_nbt(&name, &props, part.translation, part.left, n.yaw, n.pitch);
        let pos = place(n, part);
        if let Some(id) = lv
            .host
            .summon(lv.dimension, "minecraft:block_display", pos, nbt)
        {
            displays.push((id, part));
        }
    }
    Shown {
        displays,
        last: Some((n.at, n.yaw, n.pitch)),
        wheel: 0.0,
        radius,
    }
}

/// A block display's compound (`/summon minecraft:block_display`'s): the state, its model
/// moved by `translation` and turned by `left` (a quaternion), facing `yaw` and `pitch`,
/// gliding between moves, tagged as ours.
fn display_nbt(
    name: &str,
    props: &[(String, String)],
    translation: [f32; 3],
    left: [f32; 4],
    yaw: f32,
    pitch: f32,
) -> Compound {
    let float = |v: f32| Tag::Float(v.to_bits());
    let floats = |v: &[f32]| Tag::List(v.iter().map(|&f| float(f)).collect());
    let mut state = Compound::new();
    state.insert("Name".into(), Tag::String(name.into()));
    if !props.is_empty() {
        let mut p = Compound::new();
        for (k, v) in props {
            p.insert(k.as_str().into(), Tag::String(v.as_str().into()));
        }
        state.insert("Properties".into(), Tag::Compound(p));
    }
    let mut transformation = Compound::new();
    transformation.insert("translation".into(), floats(&translation));
    transformation.insert("left_rotation".into(), floats(&left));
    transformation.insert("scale".into(), floats(&[1.0, 1.0, 1.0]));
    transformation.insert("right_rotation".into(), floats(&[0.0, 0.0, 0.0, 1.0]));
    let mut nbt = Compound::new();
    nbt.insert("block_state".into(), Tag::Compound(state));
    nbt.insert("transformation".into(), Tag::Compound(transformation));
    nbt.insert("teleport_duration".into(), Tag::Int(GLIDE));
    nbt.insert("view_range".into(), float(2.0));
    nbt.insert("Rotation".into(), floats(&[yaw, pitch]));
    nbt.insert("Tags".into(), Tag::List(vec![Tag::String(TAG.into())]));
    nbt
}

/// Whether `host`'s entity `id` is one of our displays (by its tag).
pub fn is_ours(host: &dyn ServerHost, id: u32) -> bool {
    host.entity(id)
        .and_then(|e| e.tags(host))
        .is_some_and(|t| t.iter().any(|t| t == TAG))
}
