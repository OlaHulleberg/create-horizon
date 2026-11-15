//! Actors as players see them: the parts Create draws over their block models, placed and riding
//! a moving structure. Placed, a harvester's blade (`HarvesterRenderer`), a roller's wheel and
//! frame (`RollerRenderer`) and an interface's middle and top (`PortableStorageInterfaceRenderer`,
//! `PIInstance`: out towards a docked contraption, lit while connected) are drawn by their block
//! entity. On a moving structure each actor is drawn by the structure's anchor as Create's actor
//! visuals draw it (`DrillActorVisual`, `HarvesterActorVisual`, `RollerActorVisual`,
//! `SawRenderer.renderInContraption`, `DeployerActorVisual`, `PSIActorVisual`): drill heads turn,
//! harvester blades and roller wheels roll with the distance travelled, saw blades run, deployer
//! poles stroke as their hands pass block centres, and interfaces reach out to a stationary one
//! they dock at, from the structure's motion as the client draws it.
//!
//! The server tells the anchor's clients what they cannot see (`ACTORS`): whether the structure
//! is stalled, which actors contraption controls turned off, which deployers punch, and where each
//! interface's reach is chasing. A stationary interface's timer and reach go to its clients in its
//! own data (`PSI`), which run it on as Create's client does.

use super::super::kinetics::dir_name;
use super::super::server::ServerLevel;
use super::super::states;
use create_engine::Block;
use create_engine::actors::{self, Carried, Kind};
use create_engine::kinetics::{Axis, Dir};
use create_engine::parts::StateInfo;
use create_engine::psi::{self, ANIMATION, TIMEOUT};
use mod_api::{
    BlockEntityDrawContext, BlockEntityPart, BlockEntitySubmitEvent, BlockPart, ClientHost,
    Compound, ModBlockEntity, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// The key in a structure's anchor's data under which its actors' state is.
pub const ACTORS: &str = "Actors";
/// The key in a stationary interface's data under which its timer and reach are.
pub const PSI: &str = "Psi";

// ---------------------------------------------------------------------------------------------
// Shared

/// `AngleHelper.horizontalAngle`: south 0, west -90, north 180, east -270 (as 90).
fn horizontal_angle(f: Dir) -> f32 {
    match f {
        Dir::South => 0.0,
        Dir::West => -90.0,
        Dir::North => 180.0,
        Dir::East => 90.0,
        Dir::Up | Dir::Down => 0.0,
    }
}

const CENTRE: [f32; 3] = [0.5; 3];
const UP: [f32; 3] = [0.0, 1.0, 0.0];
const EAST: [f32; 3] = [1.0, 0.0, 0.0];
const SOUTH: [f32; 3] = [0.0, 0.0, 1.0];

/// Where an actor's parts go: a block entity's draw (`Drawing`), or the poses of displays
/// (`Poses`), with a pose stack as `PoseStack`'s.
pub trait Parts {
    fn push(&mut self);
    fn pop(&mut self);
    fn translate(&mut self, by: [f32; 3]);
    /// Turns what comes next `degrees` about `axis` through `pivot` (counterclockwise seen from
    /// the axis' positive end).
    fn rotate_about(&mut self, axis: [f32; 3], degrees: f32, pivot: [f32; 3]);
    /// Partial `name` (`part_*`) in its state facing `facing`.
    fn part(&mut self, name: &str, facing: &'static str);
}

/// A block entity's draw as a `Parts`.
pub struct Drawing<'a>(pub &'a mut dyn BlockEntityDrawContext);

impl Parts for Drawing<'_> {
    fn push(&mut self) {
        self.0.push();
    }
    fn pop(&mut self) {
        self.0.pop();
    }
    fn translate(&mut self, by: [f32; 3]) {
        self.0.translate(by);
    }
    fn rotate_about(&mut self, axis: [f32; 3], degrees: f32, pivot: [f32; 3]) {
        self.0.rotate_about(axis, degrees, pivot);
    }
    fn part(&mut self, name: &str, facing: &'static str) {
        if let Some(key) = states::partial(name) {
            self.0.block(key, &[("facing", facing)]);
        }
    }
}

/// A rigid pose: a model point `v` goes to `t + q·v` (`q` a quaternion `[x, y, z, w]`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub q: [f64; 4],
    pub t: [f64; 3],
}

impl Pose {
    pub const IDENTITY: Pose = Pose {
        q: [0.0, 0.0, 0.0, 1.0],
        t: [0.0; 3],
    };
}

/// The quaternion of `degrees` about `axis` (a unit vector).
pub fn quat(axis: [f64; 3], degrees: f64) -> [f64; 4] {
    let (s, c) = (degrees.to_radians() / 2.0).sin_cos();
    [axis[0] * s, axis[1] * s, axis[2] * s, c]
}

/// `a·b`: `b` first, then `a`.
pub fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// `v` turned by `q`.
pub fn quat_apply(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let p = quat_mul(
        quat_mul(q, [v[0], v[1], v[2], 0.0]),
        [-q[0], -q[1], -q[2], q[3]],
    );
    [p[0], p[1], p[2]]
}

/// The poses of an actor's parts, for displays: each part's name, facing state and pose in its
/// block (from the block's corner).
#[derive(Default)]
pub struct Poses {
    stack: Vec<Pose>,
    now: Option<Pose>,
    pub out: Vec<(String, &'static str, Pose)>,
}

impl Poses {
    fn now(&self) -> Pose {
        self.now.unwrap_or(Pose::IDENTITY)
    }
}

impl Parts for Poses {
    fn push(&mut self) {
        let now = self.now();
        self.stack.push(now);
    }
    fn pop(&mut self) {
        self.now = self.stack.pop();
    }
    fn translate(&mut self, by: [f32; 3]) {
        let p = self.now();
        let d = quat_apply(p.q, by.map(f64::from));
        self.now = Some(Pose {
            q: p.q,
            t: [0, 1, 2].map(|i| p.t[i] + d[i]),
        });
    }
    fn rotate_about(&mut self, axis: [f32; 3], degrees: f32, pivot: [f32; 3]) {
        let p = self.now();
        let r = quat(axis.map(f64::from), f64::from(degrees));
        let pivot = pivot.map(f64::from);
        let turned = quat_apply(r, pivot);
        let d = quat_apply(p.q, [0, 1, 2].map(|i| pivot[i] - turned[i]));
        self.now = Some(Pose {
            q: quat_mul(p.q, r),
            t: [0, 1, 2].map(|i| p.t[i] + d[i]),
        });
    }
    fn part(&mut self, name: &str, facing: &'static str) {
        let now = self.now();
        self.out.push((name.to_owned(), facing, now));
    }
}

/// `HarvesterActorVisual.beginFrame` (and `HarvesterRenderer` at rest): the blade turned to the
/// harvester's facing and rolled `roll` degrees about its pivot.
fn draw_blade(draw: &mut dyn Parts, facing: Dir, roll: f32) {
    draw.push();
    draw.rotate_about(UP, horizontal_angle(facing), CENTRE);
    draw.rotate_about(EAST, roll, [0.5, 0.375, 0.5625]);
    draw.part("part_harvester_blade", "up");
    draw.pop();
}

/// `RollerActorVisual.beginFrame` (and `RollerRenderer` at rest): the wheel in front of the
/// roller, a quarter block down, rolled `roll` degrees; the frame turned to its back.
fn draw_roller(draw: &mut dyn Parts, facing: Dir, roll: f32) {
    let angle = horizontal_angle(facing);
    draw.push();
    draw.rotate_about(UP, angle, CENTRE);
    draw.translate([0.0, -0.25, 17.0 / 16.0]);
    draw.rotate_about(EAST, roll, [0.0; 3]);
    draw.translate([0.0, -0.5, 0.5]);
    draw.rotate_about(UP, 90.0, [0.0; 3]);
    draw.part("part_roller_wheel", "up");
    draw.pop();
    draw.push();
    draw.rotate_about(UP, angle + 180.0, CENTRE);
    draw.part("part_roller_frame", "up");
    draw.pop();
}

/// `PIInstance.beginFrame`: the middle (lit: powered) and top turned to the interface's facing,
/// out by `progress`.
fn draw_interface(draw: &mut dyn Parts, fluid: bool, facing: Dir, lit: bool, progress: f32) {
    let kind = if fluid { "pfi" } else { "psi" };
    let x = match facing {
        Dir::Up => 0.0,
        Dir::Down => 180.0,
        _ => 90.0,
    };
    let middle = if lit {
        format!("part_{kind}_middle_powered")
    } else {
        format!("part_{kind}_middle")
    };
    for (name, out) in [
        (middle, progress * 0.5 + 0.375),
        (format!("part_{kind}_top"), progress),
    ] {
        draw.push();
        draw.rotate_about(UP, horizontal_angle(facing), CENTRE);
        draw.rotate_about(EAST, x, CENTRE);
        draw.translate([0.0, out, 0.0]);
        draw.part(&name, "up");
        draw.pop();
    }
}

/// `SawRenderer.renderInContraption`'s blade (`running`: the active one), as the saw's own
/// renderer places it: its partial facing the saw's way, a vertical saw's turned a quarter when
/// its shaft runs along the first axis.
fn draw_saw_blade(draw: &mut dyn Parts, facing: Dir, along_first: bool, running: bool) {
    let vertical = facing.axis() == Axis::Y;
    let name = format!(
        "part_saw_blade_{}_{}",
        if vertical { "vertical" } else { "horizontal" },
        if running { "active" } else { "inactive" }
    );
    draw.push();
    if vertical && along_first {
        draw.rotate_about(UP, 90.0, CENTRE);
    }
    draw.part(&name, dir_name(facing));
    draw.pop();
}

/// `DrillActorVisual.beginFrame`: the head facing the drill's way, turned `roll` degrees about it.
fn draw_drill_head(draw: &mut dyn Parts, facing: Dir, roll: f32) {
    draw.push();
    draw.rotate_about(facing.offset().map(|v| v as f32), roll, CENTRE);
    draw.part("part_drill_head", dir_name(facing));
    draw.pop();
}

/// What an actor's moving parts are doing (`ActorVisual`s' state): a drill head's, harvester
/// blade's or roller wheel's turn, a deployer's stroke and fist, a saw's running blade, an
/// interface's reach and light.
#[derive(Clone, Copy, Debug, Default)]
pub struct Doing {
    pub roll: f32,
    pub stroke: f32,
    pub punch: bool,
    pub running: bool,
    pub reach: f32,
    pub lit: bool,
}

/// Draws actor `block`'s moving parts (state facts `info`) doing `d`, under its block's pose.
pub fn actor_parts(draw: &mut dyn Parts, block: Block, info: &StateInfo, d: &Doing) {
    let Some(facing) = info.facing else {
        return;
    };
    match block {
        Block::MechanicalDrill => draw_drill_head(draw, facing, d.roll),
        Block::MechanicalHarvester => draw_blade(draw, facing, d.roll),
        Block::MechanicalRoller => draw_roller(draw, facing, d.roll),
        Block::MechanicalSaw => draw_saw_blade(draw, facing, info.along_first, d.running),
        Block::Deployer => draw_deployer(draw, facing, info.along_first, d.punch, d.stroke),
        Block::PortableStorageInterface | Block::PortableFluidInterface => draw_interface(
            draw,
            block == Block::PortableFluidInterface,
            facing,
            d.lit,
            d.reach,
        ),
        _ => {}
    }
}

/// `MovementContext.getAnimationSpeed` on a client.
fn client_animation_speed(relative: [f64; 3], disabled: bool, stalled: bool) -> f32 {
    if disabled {
        0.0
    } else if stalled {
        700.0
    } else {
        actors::animation_speed(relative, false)
    }
}

/// The degrees a tick actor `block` facing `facing` turns its head, blade or wheel, moving
/// `relative` blocks a tick in its structure's frame (`DrillActorVisual.tick`: the animation speed
/// over 20; `HarvesterActorVisual.tick`: the arc over the blade's or wheel's radius, whole
/// degrees, a quarter more), none while off or moving backwards (a stalled harvester or roller
/// neither).
pub fn roll_rate(
    block: Block,
    facing: Dir,
    relative: [f64; 3],
    disabled: bool,
    stalled: bool,
) -> f32 {
    if disabled || actors::pointing_towards(relative, facing.opposite()) {
        return 0.0;
    }
    match block {
        Block::MechanicalDrill => client_animation_speed(relative, disabled, stalled) / 20.0,
        Block::MechanicalHarvester | Block::MechanicalRoller if !stalled => {
            let radius = if block == Block::MechanicalRoller {
                16.5
            } else {
                6.5
            };
            let length = relative.iter().map(|v| v * v).sum::<f64>().sqrt();
            let deg = (length * 16.0 / radius).to_degrees() as f32;
            ((deg * 3000.0) as i32 / 3000) as f32 * 1.25
        }
        _ => 0.0,
    }
}

/// `SawRenderer.renderInContraption`'s `shouldAnimate`: a saw cuts while stalled facing sideways
/// in the world (`horizontal`), or while moving and not backwards.
pub fn saw_runs(
    facing: Dir,
    relative: [f64; 3],
    horizontal: bool,
    disabled: bool,
    stalled: bool,
) -> bool {
    let backwards = actors::pointing_towards(relative, facing.opposite());
    let moving = client_animation_speed(relative, disabled, stalled) != 0.0;
    (stalled && horizontal) || (!stalled && !backwards && moving)
}

/// `DeployerActorVisual.deploymentDistance` while moving: out half a block at a block's centre,
/// in as it passes between blocks; `world` is where it acts (its centre two blocks out).
pub fn deployer_stroke(world: [f64; 3]) -> f32 {
    let centre = world.map(|v| v.floor() + 0.5);
    let d = (0..3)
        .map(|i| (world[i] - centre[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    (0.5 - d.clamp(0.0, 1.0)) as f32
}

// ---------------------------------------------------------------------------------------------
// Stationary interfaces' timers (`PortableStorageInterfaceBlockEntity`)

/// The client's timer `elapsed` ticks after the server sent `timer` (`tick` on the client: down
/// while above `ANIMATION`, held there until the server says it let go, then down to 0).
fn client_timer(timer: i32, elapsed: i64) -> i32 {
    let e = elapsed.clamp(0, i64::from(i32::MAX)) as i32;
    if timer > ANIMATION {
        (timer - e).max(ANIMATION)
    } else if timer == ANIMATION {
        ANIMATION
    } else {
        (timer - e).max(0)
    }
}

/// `connectionAnimation`'s value at `timer` (`tick`).
fn timer_progress(timer: i32) -> f32 {
    let a = ANIMATION as f32;
    if (ANIMATION..=TIMEOUT + ANIMATION).contains(&timer) {
        1.0
    } else if timer >= TIMEOUT + ANIMATION {
        1.0 - (timer - TIMEOUT - ANIMATION) as f32 / a
    } else {
        timer as f32 / a
    }
}

/// A stationary interface's timer, reach and the game time its clients were told them.
type Told = (i32, f32, i64);

/// An actor's angle at a time, and the degrees a tick it turns since.
type Roll = (f64, f64, f32);

/// An actor by its structure's anchor and its block in it.
type RollKey = ([i32; 3], [i32; 3]);

thread_local! {
    /// What each stationary interface's clients were last told: its timer, reach and the game time.
    static PSI_SENT: RefCell<HashMap<(usize, [i32; 3]), Told>> =
        RefCell::new(HashMap::new());
    /// What each structure's clients were last told of its actors, and each interface's chase
    /// (its value now and the target it chases) by its block, by level and anchor.
    static SENT: RefCell<HashMap<(usize, [i32; 3]), Sent>> = RefCell::new(HashMap::new());
}

#[derive(Default)]
struct Sent {
    data: Option<Compound>,
    /// Each interface's value, target and since when it chases that target.
    chase: HashMap<[i32; 3], (f32, f32, i64, f32)>,
}

/// The world closed.
pub(super) fn forget() {
    PSI_SENT.with_borrow_mut(HashMap::clear);
    SENT.with_borrow_mut(HashMap::clear);
}

/// Puts `tag` (none: takes it away) under `key` in the data of the block entity at `pos`.
fn write_key(lv: &mut ServerLevel<'_>, pos: [i32; 3], key: &str, tag: Option<Tag>) {
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let Some(mut data) = entity.data(&*lv.host) else {
        return;
    };
    match tag {
        Some(t) => {
            data.insert(key.into(), t);
        }
        None => {
            if data.remove(&key.into()).is_none() {
                return;
            }
        }
    }
    let _ = entity.replace_data(&mut *lv.host, data);
}

/// A stationary interface at `pos` now has `s`: its clients hear of it when their own run of its
/// timer would differ (`sendData` as it connects, transfers and lets go), and once it is idle.
pub(super) fn sync_interface(lv: &mut ServerLevel<'_>, pos: [i32; 3], s: &psi::Stationary) {
    let dim = lv.dim;
    let now = lv.game_time();
    let sent = PSI_SENT.with_borrow(|m| m.get(&(dim, pos)).copied());
    let stale = match sent {
        Some((timer, distance, at)) => {
            client_timer(timer, now - at) != s.timer
                || distance != s.distance
                || (s.timer == 0 && timer != 0)
        }
        None => s.timer != 0,
    };
    if !stale {
        return;
    }
    PSI_SENT.with_borrow_mut(|m| m.insert((dim, pos), (s.timer, s.distance, now)));
    let mut c = Compound::new();
    c.insert("Timer".into(), Tag::Int(s.timer));
    c.insert("Distance".into(), Tag::Float(s.distance.to_bits()));
    c.insert("Since".into(), Tag::Long(now));
    write_key(lv, pos, PSI, Some(Tag::Compound(c)));
}

/// A stationary interface's timer and reach as its client runs them: (timer now, timer a tick
/// ago, reach).
fn interface_timer(nbt: &Compound, game_time: i64) -> Option<(i32, i32, f32)> {
    let Some(Tag::Compound(c)) = super::super::kinetics::synced(nbt, PSI) else {
        return None;
    };
    let timer = match c.get(&"Timer".into()) {
        Some(Tag::Int(t)) => *t,
        _ => return None,
    };
    let distance = match c.get(&"Distance".into()) {
        Some(Tag::Float(b)) => f32::from_bits(*b),
        _ => 0.0,
    };
    let since = match c.get(&"Since".into()) {
        Some(Tag::Long(t)) => *t,
        _ => game_time,
    };
    let e = game_time - since;
    Some((client_timer(timer, e), client_timer(timer, e - 1), distance))
}

// ---------------------------------------------------------------------------------------------
// Placed actors

/// Whether the placed block draws something that moves each frame: an interface while its timer
/// runs.
pub fn animated(block: Block, entity: &ModBlockEntity) -> bool {
    matches!(
        block,
        Block::PortableStorageInterface | Block::PortableFluidInterface
    ) && matches!(
        super::super::kinetics::synced(&entity.nbt, PSI),
        Some(Tag::Compound(c)) if matches!(c.get(&"Timer".into()), Some(Tag::Int(t)) if *t > 0)
    )
}

/// The part drawing placed actors' parts, on Create's kept block entity type.
pub(super) struct PlacedActors;

#[mod_api::horizon_part]
impl BlockPart for PlacedActors {}

#[mod_api::horizon_part]
impl BlockEntityPart for PlacedActors {
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some(id) = states::id(event.state) else {
            return;
        };
        let Some((block, info)) =
            crate::glue::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
        else {
            return;
        };
        let Some(facing) = info.facing else {
            return;
        };
        let draw = &mut *event.draw;
        match block {
            Block::MechanicalHarvester => draw_blade(&mut Drawing(draw), facing, 0.0),
            Block::MechanicalRoller => draw_roller(&mut Drawing(draw), facing, 0.0),
            Block::PortableStorageInterface | Block::PortableFluidInterface => {
                let fluid = block == Block::PortableFluidInterface;
                let (now, before, distance) =
                    interface_timer(&event.entity.nbt, draw.game_time()).unwrap_or((0, 0, 0.0));
                // `getExtensionDistance`: the animation's value squared, times half the reach.
                let p = timer_progress(before)
                    + (timer_progress(now) - timer_progress(before)) * draw.partial();
                let lit = (ANIMATION..=TIMEOUT + ANIMATION).contains(&now);
                let reach = p * p * distance / 2.0;
                draw_interface(&mut Drawing(draw), fluid, facing, lit, reach);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Server: what clients cannot see of a structure's actors

/// Contraption `key`'s actors (its structure drawn by `anchor`) after this tick: its clients hear
/// when its stall, its actors turned off, its punching deployers or an interface's chase change.
/// Each interface chases half its stationary interface's reach while the structure is stopped at
/// one, and 0 once it has none (`PortableStorageInterfaceMovement.findInterface`, `reset`).
pub(super) fn show_actors(
    lv: &mut ServerLevel<'_>,
    anchor: [i32; 3],
    key: [i32; 3],
    carried: &Carried,
) {
    let dim = lv.dim;
    let now = lv.game_time();
    if carried.actors.is_empty() {
        return;
    }
    let mut sent = SENT
        .with_borrow_mut(|m| m.remove(&(dim, anchor)))
        .unwrap_or_default();
    let mut c = Compound::new();
    c.insert("S".into(), Tag::Byte(i8::from(carried.stalled)));
    let locals = |f: &dyn Fn(&actors::Actor) -> bool| {
        Tag::IntArray(
            carried
                .actors
                .iter()
                .filter(|a| f(a))
                .flat_map(|a| a.local)
                .collect(),
        )
    };
    c.insert("Off".into(), locals(&|a| a.disabled));
    c.insert(
        "Punch".into(),
        locals(&|a| a.kind == Kind::Deployer && super::actors::deployer_punches(dim, key, a.local)),
    );
    let mut chases = Vec::new();
    for a in carried
        .actors
        .iter()
        .filter(|a| matches!(a.kind, Kind::Interface { .. }))
    {
        let (mut value, target, since, from) = sent
            .chase
            .get(&a.local)
            .copied()
            .unwrap_or((0.0, 0.0, now, 0.0));
        // `tickChaser`: 0.25 a tick towards its target.
        value = approach(value, target, 0.25);
        let still = carried.stalled || a.motion.iter().all(|v| *v == 0.0);
        let new_target = match a.interface.working {
            None => 0.0,
            Some(at) if still => with_world_distance(dim, at).map_or(target, |d| d / 2.0),
            Some(_) => target,
        };
        let entry = if new_target == target {
            (value, target, since, from)
        } else {
            (value, new_target, now, value)
        };
        sent.chase.insert(a.local, entry);
        let (_, target, since, from) = entry;
        let mut p = Compound::new();
        p.insert("At".into(), Tag::IntArray(a.local.to_vec()));
        p.insert("From".into(), Tag::Float(from.to_bits()));
        p.insert("To".into(), Tag::Float(target.to_bits()));
        p.insert("Since".into(), Tag::Long(since));
        chases.push(Tag::Compound(p));
    }
    c.insert("Psi".into(), Tag::List(chases));
    let changed = sent.data.as_ref() != Some(&c);
    if changed {
        sent.data = Some(c.clone());
        write_key(lv, anchor, ACTORS, Some(Tag::Compound(c)));
    }
    SENT.with_borrow_mut(|m| m.insert((dim, anchor), sent));
}

/// The structure of `anchor` is gone: its actors' state with it.
pub(super) fn hide_actors(lv: &mut ServerLevel<'_>, anchor: [i32; 3]) {
    let dim = lv.dim;
    if SENT.with_borrow_mut(|m| m.remove(&(dim, anchor))).is_some() {
        write_key(lv, anchor, ACTORS, None);
    }
}

fn with_world_distance(dim: usize, at: [i32; 3]) -> Option<f32> {
    super::super::world::with_world(|w| w.dims[dim].psis.get(&at).map(|s| s.distance)).flatten()
}

/// `LerpedFloat.Chaser.LINEAR`: `value` moved `step` towards `target`.
fn approach(value: f32, target: f32, step: f32) -> f32 {
    if (target - value).abs() <= step {
        target
    } else {
        value + step * (target - value).signum()
    }
}

// ---------------------------------------------------------------------------------------------
// Client: actors on a moving structure

/// What the server told of a structure's actors.
#[derive(Clone, Debug, Default)]
pub struct View {
    stalled: bool,
    off: Vec<[i32; 3]>,
    punch: Vec<[i32; 3]>,
    /// Each interface's chase: its block, the value it started from, its target and since when.
    chases: Vec<([i32; 3], f32, f32, i64)>,
}

fn locals(c: &Compound, key: &str) -> Vec<[i32; 3]> {
    match c.get(&key.into()) {
        Some(Tag::IntArray(v)) => v.as_chunks::<3>().0.to_vec(),
        _ => Vec::new(),
    }
}

/// The actors' state in an anchor's data, if its structure has actors.
pub fn view(nbt: &Compound) -> Option<View> {
    let Some(Tag::Compound(c)) = super::super::kinetics::synced(nbt, ACTORS) else {
        return None;
    };
    let float = |c: &Compound, k: &str| match c.get(&k.into()) {
        Some(Tag::Float(b)) => f32::from_bits(*b),
        _ => 0.0,
    };
    let chases = match c.get(&"Psi".into()) {
        Some(Tag::List(l)) => l
            .iter()
            .filter_map(|t| {
                let Tag::Compound(p) = t else { return None };
                let at = locals(p, "At").first().copied()?;
                let since = match p.get(&"Since".into()) {
                    Some(Tag::Long(t)) => *t,
                    _ => 0,
                };
                Some((at, float(p, "From"), float(p, "To"), since))
            })
            .collect(),
        _ => Vec::new(),
    };
    Some(View {
        stalled: matches!(c.get(&"S".into()), Some(Tag::Byte(1))),
        off: locals(c, "Off"),
        punch: locals(c, "Punch"),
        chases,
    })
}

/// How a structure moves this frame, for its actors.
#[derive(Clone, Copy, Debug)]
pub enum Motion {
    /// Along a line: each block's offset in the world from its anchor's corner is its own
    /// offset plus `shift`, moving `velocity` blocks a tick.
    Glide { shift: [f32; 3], velocity: [f32; 3] },
    /// About `axis` through the anchor's centre, at `angle` degrees now, turning `rate` degrees a
    /// tick.
    Turn { axis: Axis, angle: f32, rate: f32 },
}

/// A structure's actors this frame: its anchor, the server's word, its motion and the time.
pub struct Frame<'a> {
    pub anchor: [i32; 3],
    pub view: &'a View,
    pub motion: Motion,
    /// The game time with the partial tick.
    pub now: f64,
}

thread_local! {
    /// Each turning or rolling actor's angle at a time and its rate since, by anchor and block.
    static ROLLS: RefCell<HashMap<RollKey, Roll>> =
        RefCell::new(HashMap::new());
}

/// The client left its world.
pub fn client_reset() {
    ROLLS.with_borrow_mut(HashMap::clear);
}

/// The structure of `anchor` is no longer drawn.
pub fn client_forget(anchor: [i32; 3]) {
    ROLLS.with_borrow_mut(|r| r.retain(|k, _| k.0 != anchor));
}

/// An actor's angle now, at `rate` degrees a tick from where it was (`rotation += ...` each tick,
/// `angleLerp` between).
fn roll(frame: &Frame<'_>, local: [i32; 3], rate: f32) -> f32 {
    ROLLS.with_borrow_mut(|r| {
        let e = r
            .entry((frame.anchor, local))
            .or_insert((0.0, frame.now, rate));
        if e.2 != rate {
            let here = e.0 + f64::from(e.2) * (frame.now - e.1);
            *e = (here % 360.0, frame.now, rate);
        }
        ((e.0 + f64::from(e.2) * (frame.now - e.1)) % 360.0) as f32
    })
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn axis_of(a: Axis) -> [f64; 3] {
    match a {
        Axis::X => [1.0, 0.0, 0.0],
        Axis::Y => [0.0, 1.0, 0.0],
        Axis::Z => [0.0, 0.0, 1.0],
    }
}

/// Whether `block` is an actor this draws on a moving structure.
pub fn draws(block: Block) -> bool {
    matches!(
        block,
        Block::MechanicalDrill
            | Block::MechanicalSaw
            | Block::MechanicalHarvester
            | Block::MechanicalRoller
            | Block::Deployer
            | Block::PortableStorageInterface
            | Block::PortableFluidInterface
    )
}

/// Draws actor `block` (state facts `info`) at block `local` of the structure, under the pose of
/// its block: its moving parts as Create's actor visuals draw them this frame.
pub fn draw_moving(
    draw: &mut dyn BlockEntityDrawContext,
    frame: &Frame<'_>,
    block: Block,
    info: &StateInfo,
    local: [i32; 3],
) {
    let Some(kind) = Kind::of(block) else {
        return;
    };
    let Some(facing) = info.facing else {
        return;
    };
    let view = frame.view;
    let o = facing.offset().map(f64::from);
    // Where it acts in the structure, from the anchor's corner.
    let point = [0, 1, 2].map(|i| f64::from(local[i]) + 0.5 + o[i] * kind.reach());
    // `relativeMotion` (in the structure's own frame) and the point in the world, from the
    // anchor's corner; the way it faces in the world.
    let (relative, world, world_facing) = match frame.motion {
        Motion::Glide { shift, velocity } => (
            velocity.map(f64::from),
            [0, 1, 2].map(|i| point[i] + f64::from(shift[i])),
            facing,
        ),
        Motion::Turn { axis, angle, rate } => {
            let r = [0, 1, 2].map(|i| point[i] - 0.5);
            let w = f64::from(rate).to_radians();
            let v = cross(axis_of(axis), r).map(|c| c * w);
            let turned = actors::turn_vec(r, axis, f64::from(angle));
            (
                v,
                [0, 1, 2].map(|i| turned[i] + 0.5),
                actors::turned_facing(facing, axis, f64::from(angle)),
            )
        }
    };
    let disabled = view.off.contains(&local);
    let stalled = view.stalled;
    let mut d = Doing {
        punch: view.punch.contains(&local),
        lit: true,
        ..Doing::default()
    };
    match block {
        Block::MechanicalDrill | Block::MechanicalHarvester | Block::MechanicalRoller => {
            d.roll = roll(
                frame,
                local,
                roll_rate(block, facing, relative, disabled, stalled),
            );
        }
        Block::MechanicalSaw => {
            let horizontal = world_facing.axis() != Axis::Y;
            d.running = saw_runs(facing, relative, horizontal, disabled, stalled);
        }
        Block::Deployer => {
            d.stroke = if disabled {
                0.0
            } else if stalled {
                ((frame.now * 0.5).sin() * 0.25 + 0.25) as f32
            } else {
                deployer_stroke(world)
            };
        }
        Block::PortableStorageInterface | Block::PortableFluidInterface => {
            if let Some(&(_, from, to, since)) = view.chases.iter().find(|c| c.0 == local) {
                let elapsed = (frame.now - since as f64).max(0.0) as f32;
                d.reach = approach(from, to, 0.25 * elapsed);
                d.lit = d.reach == to;
            }
        }
        _ => {}
    }
    actor_parts(&mut Drawing(draw), block, info, &d);
}

/// `DeployerActorVisual`: the pole (turned a quarter about the facing when its shaft runs across
/// it) and the hand (`getHandPose`: a fist when it punches), out `factor` along the facing.
fn draw_deployer(draw: &mut dyn Parts, facing: Dir, along_first: bool, punch: bool, factor: f32) {
    let y = horizontal_angle(facing);
    let x = match facing {
        Dir::Up => 270.0,
        Dir::Down => 90.0,
        _ => 0.0,
    };
    let z = if along_first ^ (facing.axis() == Axis::Z) {
        90.0
    } else {
        0.0
    };
    let hand = if punch {
        "part_deployer_hand_punching"
    } else {
        "part_deployer_hand_pointing"
    };
    let o = facing.offset().map(|v| v as f32 * factor);
    for (name, roll) in [("part_deployer_pole", z), (hand, 0.0)] {
        draw.push();
        draw.translate(o);
        draw.rotate_about(UP, y, CENTRE);
        draw.rotate_about(EAST, x, CENTRE);
        draw.rotate_about(SOUTH, roll, CENTRE);
        draw.part(name, "south");
        draw.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_runs_an_interface_timer_down_to_its_hold_then_to_zero() {
        assert_eq!(client_timer(14, 0), 14);
        assert_eq!(client_timer(14, 5), 9);
        assert_eq!(client_timer(14, 100), ANIMATION);
        assert_eq!(client_timer(ANIMATION, 50), ANIMATION);
        assert_eq!(client_timer(ANIMATION - 1, 1), ANIMATION - 2);
        assert_eq!(client_timer(ANIMATION - 1, 10), 0);
    }

    #[test]
    fn an_interface_is_out_while_connected_and_draws_back_over_four_ticks() {
        assert_eq!(timer_progress(14), 1.0);
        assert_eq!(timer_progress(TIMEOUT + ANIMATION), 1.0);
        assert_eq!(timer_progress(2), 0.5);
        assert_eq!(timer_progress(0), 0.0);
    }

    #[test]
    fn a_pose_turns_about_its_pivot_and_moves_in_its_turned_frame() {
        let mut p = Poses::default();
        p.rotate_about(UP, 90.0, CENTRE);
        p.translate([0.0, 0.0, 1.0]);
        p.part("x", "up");
        let pose = p.out[0].2;
        // The corner (0, 0, 0) turned a quarter about the centre goes to (0, 0, 1); +z then
        // points to +x.
        let at = |v: [f64; 3]| {
            let r = quat_apply(pose.q, v);
            [0, 1, 2].map(|i| ((pose.t[i] + r[i]) * 1e6).round() / 1e6)
        };
        assert_eq!(at([0.0; 3]), [1.0, 0.0, 1.0]);
        assert_eq!(at([0.0, 0.0, 1.0]), [2.0, 0.0, 1.0]);
    }

    #[test]
    fn a_chase_moves_a_quarter_a_tick() {
        assert_eq!(approach(0.0, 1.0, 0.25), 0.25);
        assert_eq!(approach(0.9, 1.0, 0.25), 1.0);
        assert_eq!(approach(1.0, 0.0, 0.5), 0.5);
    }
}
