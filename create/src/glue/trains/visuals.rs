//! Trains as players see them (`CarriageContraptionVisual`, `StandardBogeyVisual`): each block
//! a carriage carries is a vanilla block display the server keeps where the block is, turned
//! with the carriage, and each bogey's frame and wheels (Create's partials, as the hidden
//! `create:part_bogey_*` blocks) displays posed with the bogey. Clients draw them with the
//! game's own block models and light, gliding between ticks (`teleport_duration`). An actor a
//! carriage or cart carries has its moving parts as displays of their partials too
//! (`contraptions::actor_draw`, Create's `ActorVisual`s): drill heads, harvester blades and roller
//! wheels turn as the carriage moves their way, saw blades run, deployers stroke as they pass
//! block centres.
//!
//! A carriage has its displays while the chunk under it is loaded (`Carriage.manageEntities`
//! spawns its entity the same way); they go when it leaves, when the train goes, and with the
//! world. They are never saved as Create's: one that comes back with its chunk is discarded and
//! made again from the train (`sweep`).

use super::super::server::ServerLevel;
use super::super::world::with_world;
use crate::glue::contraptions::actor_draw::{self, Doing, Poses};
use create_engine::Block;
use create_engine::parts::StateInfo;
use create_engine::train_controls::Frame;
use create_engine::trains::{BogeySize, yaw_pitch};
use mod_api::{Compound, Tag, Transformation};
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
    /// A bogey's wheels' turn (`CarriageBogey.wheelAngle`), degrees, the radius they roll on
    /// and its size.
    wheel: f64,
    radius: f64,
    size: Option<BogeySize>,
    /// Each actor's head, blade or wheel turn (degrees), by its block's centre in the carriage.
    rolls: Vec<([f64; 3], f32)>,
    /// Each train controls' levers (`LeverAngles`: hand, speed, steering), by its block's
    /// centre in the carriage.
    levers: Vec<([f64; 3], [f32; 3])>,
}

/// A display's place: its point in the carriage's (or bogey's) space, from which the display
/// stands, turned with it; its model's offset and turn there, and whether it is a wheel,
/// turning about its axle as the bogey rolls.
#[derive(Clone, Copy)]
struct Part {
    at: [f64; 3],
    translation: [f32; 3],
    left: [f32; 4],
    /// A bogey's part: its index in `bogey_parts`, posed by the wheels' turn.
    bogey: Option<u8>,
    /// An actor's moving part (`ActorVisual`): the actor's block, its state's facts, its centre in
    /// the carriage's space (unturned) and which of its parts this is.
    actor: Option<(Block, StateInfo, [f64; 3], usize)>,
}

/// What a display group shows: `(level, train, CAR, carriage)`, `(level, train, BOGEY,
/// bogey)` or `(level, cart's UUID, CART, 0)`.
type Key = (usize, u128, u8, usize);

const CAR: u8 = 0;
const BOGEY: u8 = 1;
const CART: u8 = 2;

thread_local! {
    static SHOWN: RefCell<HashMap<Key, Shown>> = RefCell::new(HashMap::new());
    /// This opening of the world's number (`session_tag`), 0 before the first display.
    static SESSION: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The world closed: its displays are saved with their chunks and discarded when they return.
pub fn forget() {
    SHOWN.with_borrow_mut(HashMap::clear);
    SESSION.set(0);
}

/// Ticks between sweeps for displays and seat stand-ins that came back with their chunks.
const SWEEP_TICKS: i64 = 40;

/// Displays and seat stand-ins of ours from before this world opened (saved with their chunk
/// and back with it) are discarded: those made since carry this opening's tag. The game tells
/// mods of no display joining from a save and lists none among a level's entities, so a kill
/// by selector finds them.
fn sweep(lv: &mut ServerLevel<'_>) {
    let now = session_tag();
    for (kind, tag) in [
        ("minecraft:block_display", TAG),
        ("minecraft:item_display", TAG),
        ("minecraft:text_display", TAG),
        ("minecraft:item_display", super::riding::TAG),
    ] {
        lv.run_command(&format!("kill @e[type={kind},tag={tag},tag=!{now}]"));
    }
    // A group missing a display (killed by a command) is made again at the next tick.
    let dim = lv.dim;
    let broken: Vec<Key> = SHOWN.with_borrow(|s| {
        s.iter()
            .filter(|(k, v)| {
                k.0 == dim && v.displays.iter().any(|(d, _)| lv.host.entity(*d).is_none())
            })
            .map(|(k, _)| *k)
            .collect()
    });
    for k in broken {
        if let Some(shown) = SHOWN.with_borrow_mut(|s| s.remove(&k)) {
            for (id, _) in shown.displays {
                if let Some(e) = lv.host.entity(id) {
                    e.discard(lv.host);
                }
            }
        }
    }
}

/// The tag every display and stand-in made since the world opened carries.
pub fn session_tag() -> String {
    let n = SESSION.get();
    let n = if n == 0 {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.subsec_nanos() | 1);
        SESSION.set(t);
        t
    } else {
        n
    };
    format!("create.s{n:x}")
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
    /// A cart's contraption's blocks, when it has no displays yet.
    blocks: Option<Vec<([i32; 3], u32)>>,
    /// What a cart's contraption's block entities hold (`actors::storage_to_nbt`), when it has
    /// no displays yet.
    held: Option<Tag>,
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
    if lv.game_time() % SWEEP_TICKS == 0 {
        sweep(lv);
    }
    let empty = SHOWN.with_borrow(|s| !s.keys().any(|k| k.0 == dim && k.2 != CART));
    let any =
        with_world(|w| w.dims[dim].railway.trains.iter().any(|t| t.train.bound)).unwrap_or(false);
    if empty && !any {
        return;
    }
    let now: Vec<Now> = with_world(|w| {
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
                    key: (dim, u128::from(t.id), CAR, c),
                    at: pivot,
                    yaw,
                    pitch,
                    turn: -a,
                    front,
                    loaded: false,
                    blocks: None,
                    held: None,
                });
            }
            for b in 0..t.train.bogeys.len() {
                let (anchor, front) = t.train.bogey_pose(g, b);
                let (yaw, pitch) = yaw_pitch(front, t.heading);
                let (yaw, pitch) = display_angles(yaw, pitch, t.heading);
                out.push(Now {
                    key: (dim, u128::from(t.id), BOGEY, b),
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
                    blocks: None,
                    held: None,
                });
            }
        }
        out
    })
    .unwrap_or_default();
    sync(lv, now, &[CAR, BOGEY]);
}

/// Carriage `car` of train `train` changed a block (a door opened): its displays are made
/// again from its blocks next tick.
pub fn refresh_car(lv: &mut ServerLevel<'_>, train: u32, car: usize) {
    let key = (lv.dim, u128::from(train), CAR, car);
    if let Some(shown) = SHOWN.with_borrow_mut(|s| s.remove(&key)) {
        for (id, _) in shown.displays {
            if let Some(e) = lv.host.entity(id) {
                e.discard(lv.host);
            }
        }
    }
}

/// A cart's contraption as `carts` takes it.
pub type CartPose<'a> = (
    u128,
    [f64; 3],
    f32,
    f32,
    f32,
    &'a [([i32; 3], u32)],
    Option<&'a Tag>,
);

/// The cart contraptions of a level after its carts moved (`OrientedContraptionEntity`'s
/// drawing): each cart's UUID, its contraption's pivot (its anchor block's centre), its turn
/// (`Mounted::angle`), pitch and the turn of its initial orientation, drawn
/// `Y(angle - base) · Z(pitch) · Y(base)` about the pivot, and the blocks.
pub fn carts(lv: &mut ServerLevel<'_>, list: Vec<CartPose<'_>>) {
    let dim = lv.dim;
    if list.is_empty() && !SHOWN.with_borrow(|s| s.keys().any(|k| k.0 == dim && k.2 == CART)) {
        return;
    }
    let now = list
        .into_iter()
        .map(|(uuid, at, angle, pitch, base, blocks, held)| {
            let key = (dim, uuid, CART, 0);
            let shown = SHOWN.with_borrow(|s| s.contains_key(&key));
            // `Ry(angle - base) Rz(pitch) Ry(base)` is `Ry(angle - base - 90) Rx(pitch)
            // Ry(base + 90)`.
            let yaw = -(f64::from(angle) - f64::from(base) - 90.0);
            Now {
                key,
                at,
                yaw: ((yaw + 540.0).rem_euclid(360.0) - 180.0) as f32,
                pitch,
                turn: f64::from(base) + 90.0,
                front: [0.0; 3],
                loaded: false,
                blocks: (!shown).then(|| blocks.to_vec()),
                held: if shown { None } else { held.cloned() },
            }
        })
        .collect();
    sync(lv, now, &[CART]);
}

/// Display groups of kinds `kinds` as they are now: those that came into loaded chunks get
/// their displays, those that moved move them, and those gone or out of reach lose them.
fn sync(lv: &mut ServerLevel<'_>, mut now: Vec<Now>, kinds: &[u8]) {
    let dim = lv.dim;
    for n in &mut now {
        n.loaded = lv.is_loaded(n.at.map(|v| v.floor() as i32));
    }
    // Gone, or out of loaded chunks: their displays go.
    let gone: Vec<Key> = SHOWN.with_borrow(|s| {
        s.keys()
            .filter(|k| k.0 == dim && kinds.contains(&k.2))
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
        let levers = lever_targets(n);
        let moved = SHOWN.with_borrow(|s| {
            s.get(&n.key).is_some_and(|v| {
                v.last != Some(pose)
                    || levers.iter().any(|(c, t)| {
                        let now = v
                            .levers
                            .iter()
                            .find(|l| l.0 == *c)
                            .map_or([0.0; 3], |l| l.1);
                        (0..3).any(|i| (now[i] - t[i]).abs() > 1e-3)
                    })
            })
        });
        if !moved {
            continue;
        }
        let (displays, (wheel, size), rolls) = SHOWN.with_borrow_mut(|s| {
            let v = s.get_mut(&n.key).expect("known");
            // Each actor's head, blade or wheel turns on by the way it moved
            // (`HarvesterActorVisual.tick`, `DrillActorVisual.tick`).
            if let Some((was, _, _)) = v.last {
                let world: [f64; 3] = std::array::from_fn(|i| n.at[i] - was[i]);
                let relative = to_carriage(n, world);
                let mut rolls = std::mem::take(&mut v.rolls);
                for (_, part) in &v.displays {
                    let Some((block, info, centre, 0)) = part.actor else {
                        continue;
                    };
                    let Some(facing) = info.facing else {
                        continue;
                    };
                    let rate = actor_draw::roll_rate(block, facing, relative, false, false);
                    match rolls.iter_mut().find(|r| r.0 == centre) {
                        Some(r) => r.1 = (r.1 + rate) % 360.0,
                        None => rolls.push((centre, rate % 360.0)),
                    }
                }
                v.rolls = rolls;
            }
            // `CarriageBogey.updateAngles`: the wheels turn by the distance rolled, forward
            // along the bogey's way, over their circumference.
            if let Some((was, _, _)) = v.last
                && v.radius > 0.0
            {
                let d: [f64; 3] = std::array::from_fn(|i| n.at[i] - was[i]);
                let along = d[0] * n.front[0] + d[1] * n.front[1] + d[2] * n.front[2];
                v.wheel = (v.wheel + (along / v.radius).to_degrees()).rem_euclid(360.0);
            }
            let motion = v.last.map_or([0.0; 3], |(was, _, _)| {
                std::array::from_fn(|i| n.at[i] - was[i])
            });
            // `renderInContraption`: each lever chases its target by a fifth a tick
            // (`Chaser.EXP` at 0.2); the speed lever's target is how far it moved this tick.
            let moved_by = motion.iter().map(|m| m * m).sum::<f64>().sqrt().min(0.5) as f32;
            for (centre, target) in &levers {
                let target = [target[0], target[1] * moved_by, target[2]];
                let entry = match v.levers.iter_mut().position(|l| l.0 == *centre) {
                    Some(i) => &mut v.levers[i].1,
                    None => {
                        v.levers.push((*centre, [0.0; 3]));
                        &mut v.levers.last_mut().expect("pushed").1
                    }
                };
                for i in 0..3 {
                    // A hand let go drops the levers at once (`chase(0, 0, ..)`).
                    if target[0] == 0.0 && i > 0 {
                        entry[i] = 0.0;
                    } else {
                        entry[i] += (target[i] - entry[i]) * 0.2;
                    }
                }
            }
            v.last = Some(pose);
            (
                v.displays.clone(),
                (v.wheel, v.size),
                (v.rolls.clone(), motion, v.levers.clone()),
            )
        });
        let (rolls, motion, lever_now) = rolls;
        let relative = to_carriage(n, motion);
        for (id, part) in displays {
            let Some(e) = lv.host.entity(id) else {
                continue;
            };
            e.teleport_to(lv.host, place(n, part), Some([n.yaw, n.pitch]));
            if let Some((block, info, centre, i)) = part.actor
                && let Some(d) = lv.host.display(e)
                && let Some(facing) = info.facing
            {
                // Where it acts (`getActiveAreaOffset`), and the way it faces, in the world.
                let o = facing.offset().map(f64::from);
                let reach = create_engine::actors::Kind::of(block).map_or(0.0, |k| k.reach());
                let local: [f64; 3] = std::array::from_fn(|k| centre[k] + o[k] * reach);
                let point = place(
                    n,
                    Part {
                        at: turn_y(local, n.turn),
                        ..part
                    },
                );
                let way = from_carriage(n, o);
                let horizontal = way[1].abs() < way[0].abs().max(way[2].abs());
                let doing = Doing {
                    roll: rolls.iter().find(|r| r.0 == centre).map_or(0.0, |r| r.1),
                    stroke: actor_draw::deployer_stroke(point),
                    running: actor_draw::saw_runs(facing, relative, horizontal, false, false),
                    lit: true,
                    levers: lever_now
                        .iter()
                        .find(|l| l.0 == centre)
                        .map_or([0.0; 3], |l| l.1),
                    ..Doing::default()
                };
                if let Some((name, facing, pose)) =
                    actor_poses(block, &info, &doing).into_iter().nth(i)
                {
                    let (translation, left) = actor_transform(pose, n.turn);
                    let mut t = Transformation::IDENTITY;
                    t.translation = translation;
                    t.left_rotation = left;
                    d.set_start_interpolation(lv.host, 0);
                    d.set_interpolation_duration(lv.host, 1);
                    d.set_transformation(lv.host, t);
                    if block == Block::MechanicalSaw
                        && let Some(state) = lv
                            .host
                            .block_state_id(&format!("create:{name}[facing={facing}]"))
                    {
                        d.set_block_state(lv.host, state);
                    }
                }
            }
            if let Some(i) = part.bogey
                && let Some(size) = size
                && let Some((_, _, pose)) = bogey_parts(size, wheel as f32).get(usize::from(i))
                && let Some(d) = lv.host.display(e)
            {
                let (translation, left) = bogey_transform(*pose, n.turn);
                let mut t = Transformation::IDENTITY;
                t.translation = translation;
                t.left_rotation = left;
                d.set_start_interpolation(lv.host, 0);
                d.set_interpolation_duration(lv.host, 1);
                d.set_transformation(lv.host, t);
            }
        }
    }
}

/// A display to make: its block and properties, its point in the carriage's space, and its
/// model's corner from there.
type Shape = (
    String,
    Vec<(String, String)>,
    [f64; 3],
    [f64; 3],
    Option<u8>,
);

/// The displays of a carriage or bogey, made where it stands now.
fn spawn(lv: &mut ServerLevel<'_>, n: &Now) -> Shown {
    let (_, id, kind, i) = n.key;
    let train = id as u32;
    let dim = lv.dim;
    // Each display: its state, its point in the carriage's space, and its own turn about that
    // point (the partials' quarter) with the offset of its model's origin from the point.
    let mut parts: Vec<Shape> = Vec::new();
    let mut extras: Vec<([f64; 3], super::contents::Extra)> = Vec::new();
    let mut actors: Vec<(Block, StateInfo, [f64; 3], usize)> = Vec::new();
    let mut radius = 0.0;
    let mut bogey_size = None;
    if kind == BOGEY {
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
                size: None,
                rolls: Vec::new(),
                levers: Vec::new(),
            };
        };
        radius = size.wheel_radius();
        bogey_size = Some(size);
        for (i, (name, facing, _)) in bogey_parts(size, 0.0).into_iter().enumerate() {
            let props = vec![("facing".to_owned(), facing.to_owned())];
            parts.push((
                format!("create:{name}"),
                props,
                [0.0; 3],
                [0.0; 3],
                Some(i as u8),
            ));
        }
    } else {
        let blocks = match &n.blocks {
            Some(b) => b.clone(),
            None => with_world(|w| {
                let t = w.dims[dim].railway.trains.iter().find(|t| t.id == train)?;
                t.cars.get(i).cloned()
            })
            .flatten()
            .unwrap_or_default(),
        };
        // What their block entities hold (`renderBlockEntities`), shown by displays of their own.
        let entities = if kind == CART {
            crate::glue::contraptions::actors::shown_entities_of(lv, n.held.as_ref())
        } else {
            crate::glue::contraptions::actors::shown_entities(lv, super::cargo::key(train, i))
        };
        for (o, s) in &blocks {
            let Some(entity) = entities.get(o) else {
                continue;
            };
            let up = if kind == CART { 0.0 } else { 1.5 };
            let centre = [f64::from(o[0]), f64::from(o[1]) + up, f64::from(o[2])];
            for e in super::contents::extras(lv, *s, entity) {
                extras.push((centre, e));
            }
        }
        for (o, s) in blocks {
            // A cart's anchor is its assembler, not drawn (Create puts its rail back there).
            if kind == CART && o == [0, 0, 0] {
                continue;
            }
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
            // over the pivot, its centre over it; a cart's anchor block is centred on its pivot.
            let up = if kind == CART { 0.0 } else { 1.5 };
            let centre = [f64::from(o[0]), f64::from(o[1]) + up, f64::from(o[2])];
            parts.push((name, props, centre, [-0.5; 3], None));
            // An actor's moving parts, at rest until it moves.
            // Train controls draw their cover and levers on a train only.
            let train_controls = |b: Block| b == Block::Controls && kind == CAR;
            if let Some((block, info)) = crate::glue::contraptions::create_info(s)
                .filter(|(b, _)| actor_draw::draws(*b) || train_controls(*b))
            {
                let doing = Doing {
                    lit: true,
                    ..Doing::default()
                };
                for (i, _) in actor_poses(block, &info, &doing).into_iter().enumerate() {
                    actors.push((block, info, centre, i));
                }
            }
        }
    }
    let mut displays = Vec::new();
    let posed = bogey_size.map(|s| bogey_parts(s, 0.0)).unwrap_or_default();
    for (name, props, at, origin, bogey) in parts {
        // The part's point in the display's turned space (the carriage's space turned back by
        // the heading's quarter), the model's corner from there, and the quarter as the
        // display's left rotation; a bogey's part posed as its renderer poses it.
        let (translation, left) = match bogey.and_then(|i| posed.get(usize::from(i))) {
            Some((_, _, pose)) => bogey_transform(*pose, n.turn),
            None => (turn_y(origin, n.turn).map(|v| v as f32), quat_y(n.turn)),
        };
        let part = Part {
            at: turn_y(at, n.turn),
            translation,
            left,
            bogey,
            actor: None,
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
    for (centre, e) in extras {
        // Posed in its block (from the corner) as the block's display is turned, the block
        // centred on the display's point.
        let Some((t, turn, scale)) = crate::glue::logistics::client::decompose(e.pose) else {
            continue;
        };
        let q = turn.map_or([0.0, 0.0, 0.0, 1.0], |(axis, deg)| {
            actor_draw::quat(axis.map(f64::from), f64::from(deg))
        });
        let left = actor_draw::quat_mul(actor_draw::quat([0.0, 1.0, 0.0], n.turn), q);
        let part = Part {
            at: turn_y(centre, n.turn),
            translation: turn_y(t.map(|v| f64::from(v) - 0.5), n.turn).map(|v| v as f32),
            left: left.map(|v| v as f32),
            bogey: None,
            actor: None,
        };
        let mut nbt = e.nbt;
        let base = display_nbt("", &[], part.translation, part.left, n.yaw, n.pitch);
        for (k, v) in base.iter() {
            if k.to_string_lossy() != "block_state" {
                nbt.insert(k.clone(), v.clone());
            }
        }
        if let Some(Tag::Compound(tr)) = nbt.get_mut(&"transformation".into()) {
            let floats = Tag::List(scale.iter().map(|f| Tag::Float(f.to_bits())).collect());
            tr.insert("scale".into(), floats);
        }
        if let Some(id) = lv.host.summon(lv.dimension, e.kind, place(n, part), nbt) {
            displays.push((id, part));
        }
    }
    for (block, info, centre, i) in actors {
        let doing = Doing {
            lit: true,
            ..Doing::default()
        };
        let Some((name, facing, pose)) = actor_poses(block, &info, &doing).into_iter().nth(i)
        else {
            continue;
        };
        let (translation, left) = actor_transform(pose, n.turn);
        let part = Part {
            at: turn_y(centre, n.turn),
            translation,
            left,
            bogey: None,
            actor: Some((block, info, centre, i)),
        };
        let props = vec![("facing".to_owned(), facing.to_owned())];
        let name = format!("create:{name}");
        let nbt = display_nbt(&name, &props, part.translation, part.left, n.yaw, n.pitch);
        if let Some(id) =
            lv.host
                .summon(lv.dimension, "minecraft:block_display", place(n, part), nbt)
        {
            displays.push((id, part));
        }
    }
    Shown {
        displays,
        last: Some((n.at, n.yaw, n.pitch)),
        wheel: 0.0,
        radius,
        size: bogey_size,
        rolls: Vec::new(),
        levers: Vec::new(),
    }
}

/// The train controls of carriage `n` and their levers' targets: the hand on them (1 while a
/// player holds them), the speed lever's way (`movingBackwards ^ inverted`: -1 or 1, times how
/// far the carriage moves), the steering (right 1, left -1), by the controls' centre.
fn lever_targets(n: &Now) -> Vec<([f64; 3], [f32; 3])> {
    let (dim, id, kind, car) = n.key;
    if kind != CAR {
        return Vec::new();
    }
    let train = id as u32;
    let found = SHOWN.with_borrow(|s| {
        s.get(&n.key).map(|v| {
            v.displays
                .iter()
                .filter_map(|(_, p)| match p.actor {
                    Some((Block::Controls, _, centre, 0)) => Some(centre),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
    });
    let Some(found) = found.filter(|f| !f.is_empty()) else {
        return Vec::new();
    };
    let backwards = with_world(|w| {
        w.dims[dim]
            .railway
            .trains
            .iter()
            .find(|t| t.id == train)
            .map(|t| t.train.speed < 0.0)
    })
    .flatten()
    .unwrap_or(false);
    found
        .into_iter()
        .map(|centre| {
            let local = [
                centre[0].round() as i32,
                (centre[1] - 1.5).round() as i32,
                centre[2].round() as i32,
            ];
            let target = match super::controls::held_at(dim, train, car, local) {
                Some((keys, inverted)) => {
                    let has = |k: u8| keys & (1 << k) != 0;
                    let steer = f32::from(u8::from(has(create_engine::train_controls::RIGHT)))
                        - f32::from(u8::from(has(create_engine::train_controls::LEFT)));
                    [1.0, if backwards ^ inverted { -1.0 } else { 1.0 }, steer]
                }
                None => [0.0; 3],
            };
            (centre, target)
        })
        .collect()
}

/// `StandardBogeyRenderer` (`Small`, `Large`): a bogey's parts with its wheels turned `angle`
/// degrees, posed from its anchor (Create's partials' origin): the two shafts along it, then a
/// small bogey's frame and wheels a block before and behind, or a large one's two shafts across,
/// drive, belt, pistons sliding with the wheels, wheels a block up and pins going round on
/// them. Each part's partial, facing and pose. The large
/// bogey's belt texture does not scroll (`shiftUVScrolling`: a display draws a model as baked).
fn bogey_parts(size: BogeySize, angle: f32) -> Vec<(String, &'static str, actor_draw::Pose)> {
    use actor_draw::Parts as _;
    const X: [f32; 3] = [1.0, 0.0, 0.0];
    const Z: [f32; 3] = [0.0, 0.0, 1.0];
    let mut p = Poses::default();
    for i in [0.0, 1.0] {
        p.push();
        p.translate([-0.5, 0.25, -i]);
        p.rotate_about(Z, angle, [0.5; 3]);
        p.part("part_shaft", "south");
        p.pop();
    }
    match size {
        BogeySize::Small => {
            p.part("part_bogey_frame", "up");
            for side in [1.0, -1.0] {
                p.push();
                p.translate([0.0, 12.0 / 16.0, side]);
                p.rotate_about(X, angle, [0.0; 3]);
                p.part("part_bogey_wheel", "up");
                p.pop();
            }
        }
        BogeySize::Large => {
            for z in [0.5, -1.5] {
                p.push();
                p.translate([-0.5, 0.25, z]);
                p.rotate_about(X, angle, [0.5; 3]);
                p.part("part_shaft", "east");
                p.pop();
            }
            p.part("part_bogey_drive", "up");
            p.part("part_bogey_drive_belt", "up");
            p.push();
            p.translate([0.0, 0.0, 0.25 * angle.to_radians().sin()]);
            p.part("part_bogey_piston", "up");
            p.pop();
            p.push();
            p.translate([0.0, 1.0, 0.0]);
            p.rotate_about(X, angle, [0.0; 3]);
            p.part("part_bogey_drive_wheel", "up");
            p.translate([0.0, 0.25, 0.0]);
            p.rotate_about(X, -angle, [0.0; 3]);
            p.part("part_bogey_pin", "up");
            p.pop();
        }
    }
    p.out
}

/// A display's translation and left rotation for a bogey part posed `pose` from the bogey's
/// anchor, turned `turn` about up as the bogey.
fn bogey_transform(pose: actor_draw::Pose, turn: f64) -> ([f32; 3], [f32; 4]) {
    let q = actor_draw::quat_mul(actor_draw::quat([0.0, 1.0, 0.0], turn), pose.q);
    (turn_y(pose.t, turn).map(|v| v as f32), q.map(|v| v as f32))
}

/// The poses of actor `block`'s moving parts doing `doing` (`actor_draw::actor_parts`).
fn actor_poses(
    block: Block,
    info: &StateInfo,
    doing: &Doing,
) -> Vec<(String, &'static str, actor_draw::Pose)> {
    let mut poses = Poses::default();
    actor_draw::actor_parts(&mut poses, block, info, doing);
    poses.out
}

/// A display's translation and left rotation for a part posed `pose` in its block (from its
/// corner), the block centred on the display and turned `turn` about up as the carriage's.
fn actor_transform(pose: actor_draw::Pose, turn: f64) -> ([f32; 3], [f32; 4]) {
    let q = actor_draw::quat_mul(actor_draw::quat([0.0, 1.0, 0.0], turn), pose.q);
    let t = turn_y(std::array::from_fn(|i| pose.t[i] - 0.5), turn);
    (t.map(|v| v as f32), q.map(|v| v as f32))
}

/// `v` in the world turned into the carriage's own space (`place`'s turn, back).
fn to_carriage(n: &Now, v: [f64; 3]) -> [f64; 3] {
    turn_y(
        turn_x(turn_y(v, f64::from(n.yaw)), -f64::from(n.pitch)),
        -n.turn,
    )
}

/// `v` in the carriage's own space turned into the world, as `place` turns a point.
fn from_carriage(n: &Now, v: [f64; 3]) -> [f64; 3] {
    turn_y(
        turn_x(turn_y(v, n.turn), f64::from(n.pitch)),
        -f64::from(n.yaw),
    )
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
    nbt.insert(
        "Tags".into(),
        Tag::List(vec![
            Tag::String(TAG.into()),
            Tag::String(session_tag().as_str().into()),
        ]),
    );
    nbt
}
