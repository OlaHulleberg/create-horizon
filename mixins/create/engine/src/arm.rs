//! Create's mechanical arm (`ArmBlockEntity`, `ArmInteractionPoint`, `AllArmInteractionPointTypes`,
//! `ArmAngleTarget`, `ArmRenderer`): it takes items from its input points and puts them into its
//! output points, moving to each at a speed from its rotation. Its points, phase and held stack
//! are typed state here; the glue says what each point holds and takes. An arm with nothing to
//! do sleeps until one of its points changes (or, for points whose changes are not reported, a
//! lazy tick comes round), and clients see it again only when its target or phase changes.

use crate::kinetics::Dir;
use crate::place::{mth_cos, mth_sin};
use glam::{Mat4, Vec3};
use std::collections::HashMap;

/// `mechanicalArmRange`: points must lie closer than this to the arm.
pub const RANGE: i32 = 5;
/// `SmartBlockEntity`'s lazy tick rate: a lazy tick every 11 ticks (the counter runs 10 to 0).
pub const LAZY_RATE: u8 = 10;

/// The interaction point types Create registers, in its registry order (`getPrimaryType` takes
/// the first that fits a block): the ones whose blocks the port has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Basin,
    Belt,
    BlazeBurner,
    Chute,
    Crafter,
    CrushingWheels,
    Deployer,
    Depot,
    Funnel,
    Millstone,
    Saw,
}

impl Kind {
    pub const ALL: [Kind; 11] = [
        Kind::Basin,
        Kind::Belt,
        Kind::BlazeBurner,
        Kind::Chute,
        Kind::Crafter,
        Kind::CrushingWheels,
        Kind::Deployer,
        Kind::Depot,
        Kind::Funnel,
        Kind::Millstone,
        Kind::Saw,
    ];

    pub fn index(self) -> u8 {
        self as u8
    }

    pub fn from_index(i: u8) -> Option<Kind> {
        Kind::ALL.get(usize::from(i)).copied()
    }

    /// `DepositOnlyArmInteractionPoint`: nothing is taken from these.
    pub fn deposit_only(self) -> bool {
        matches!(
            self,
            Kind::BlazeBurner | Kind::CrushingWheels | Kind::Funnel
        )
    }

    /// Whether the glue reports every change of what such a point holds (depots and ejectors,
    /// belts, chutes): an arm waiting on only these points sleeps until one changes.
    pub fn reported(self) -> bool {
        matches!(self, Kind::Depot | Kind::Belt | Kind::Chute)
    }
}

/// `ArmInteractionPoint.Mode`: TAKE (an input) or DEPOSIT (an output).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub pos: [i32; 3],
    pub kind: Kind,
    /// The facing that turns its interaction (a funnel's, a crafter's or a deployer's).
    pub facing: Option<Dir>,
}

fn normal(d: Dir) -> Vec3 {
    let o = d.offset();
    Vec3::new(o[0] as f32, o[1] as f32, o[2] as f32)
}

impl Point {
    /// `getInteractionPositionVector` and `getInteractionDirection` (the way the claw points).
    pub fn interaction(&self) -> (Vec3, Dir) {
        let p = Vec3::new(self.pos[0] as f32, self.pos[1] as f32, self.pos[2] as f32);
        let centre = p + Vec3::splat(0.5);
        match self.kind {
            // TopFaceArmInteractionPoint; CrushingWheelPoint.
            Kind::Chute | Kind::CrushingWheels => (p + Vec3::new(0.5, 1.0, 0.5), Dir::Down),
            // DepotPoint (depots, ejectors, saws) and BeltPoint.
            Kind::Depot | Kind::Belt | Kind::Saw => {
                (p + Vec3::new(0.5, 14.0 / 16.0, 0.5), Dir::Down)
            }
            // CrafterPoint: its facing's back, half a block out.
            Kind::Crafter => {
                let d = self.facing.unwrap_or(Dir::South).opposite();
                (centre + normal(d) * 0.5, d)
            }
            // DeployerPoint: its facing's back, 0.65 out.
            Kind::Deployer => {
                let d = self.facing.unwrap_or(Dir::Up).opposite();
                (centre + normal(d) * 0.65, d)
            }
            // FunnelPoint: 0.15 into the funnel, pointing into it.
            Kind::Funnel => match self.facing {
                Some(f) => (centre - normal(f) * 0.15, f.opposite()),
                None => (centre, Dir::Up),
            },
            Kind::Basin | Kind::BlazeBurner | Kind::Millstone => (centre, Dir::Down),
        }
    }
}

/// Catnip's `AngleHelper.getShortestAngleDiff` (float `%`, the sign of the dividend).
pub fn shortest_angle_diff(current: f32, target: f32) -> f32 {
    let (c, t) = (current % 360.0, target % 360.0);
    (((t - c) % 360.0 + 540.0) % 360.0) - 180.0
}

/// Catnip's `AngleHelper.angleLerp`.
pub fn angle_lerp(pct: f32, current: f32, target: f32) -> f32 {
    current + shortest_angle_diff(current, target) * pct
}

/// Catnip's `VecHelper.rotate(vec, deg, axis)` about x (0) or y (1).
fn rotate(v: [f64; 3], deg: f64, axis: u8) -> [f64; 3] {
    if deg == 0.0 {
        return v;
    }
    let angle = (deg / 180.0 * std::f64::consts::PI) as f32;
    let (sin, cos) = (f64::from(mth_sin(angle)), f64::from(mth_cos(angle)));
    let [x, y, z] = v;
    if axis == 0 {
        [x, y * cos - z * sin, z * cos + y * sin]
    } else {
        [x * cos + z * sin, y, z * cos - x * sin]
    }
}

/// `ArmAngleTarget`: the base's yaw and the three joints' angles that reach a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Angles {
    pub base: f32,
    pub lower: f32,
    pub upper: f32,
    pub head: f32,
}

/// `ArmAngleTarget.NO_TARGET`: folded up at rest.
pub const NO_TARGET: Angles = Angles {
    base: 0.0,
    lower: 135.0,
    upper: 45.0,
    head: 0.0,
};

/// `AngleHelper.deg`.
fn deg(rad: f64) -> f32 {
    if rad == 0.0 {
        return 0.0;
    }
    (rad * 180.0 / std::f64::consts::PI) as f32
}

impl Angles {
    /// `new ArmAngleTarget(armPos, pointTarget, clawFacing, ceiling)`: the arm's two segments
    /// (14 and 15 px) reach the point backed off half a block against the claw's facing, by the
    /// law of cosines; the head then points at the point itself.
    pub fn new(arm: [i32; 3], point: Vec3, claw: Dir, ceiling: bool) -> Angles {
        let target = [point.x, point.y, point.z].map(f64::from);
        let origin = [
            f64::from(arm[0]) + 0.5,
            f64::from(arm[1]) + 0.5 + if ceiling { -6.0 / 16.0 } else { 6.0 / 16.0 },
            f64::from(arm[2]) + 0.5,
        ];
        let back = claw.opposite().offset().map(|v| f64::from(v) * 0.5);
        let reach = [0, 1, 2].map(|i| target[i] + back[i]);
        let mut diff = [0, 1, 2].map(|i| reach[i] - origin[i]);
        let horizontal = (diff[0] * diff[0] + diff[2] * diff[2]).sqrt() as f32;
        let mut base = deg(diff[0].atan2(diff[2])) + 180.0;
        if ceiling {
            diff[1] = -diff[1];
            base = 180.0 - base;
        }
        let alpha_offset = deg(diff[1].atan2(f64::from(horizontal)));
        let a = 14.0f32 / 16.0;
        let b = 15.0f32 / 16.0;
        let (a2, b2) = (a * a, b * b);
        let length = ((diff[1] * diff[1]) as f32 + horizontal * horizontal)
            .sqrt()
            .clamp(1.0 / 8.0, a + b);
        let length2 = length * length;
        let alpha_ratio = (-b2 + a2 + length2) / (2.0 * a * length);
        let mut alpha = deg(f64::from(alpha_ratio).acos()) + alpha_offset;
        let beta_ratio = (-length2 + a2 + b2) / (2.0 * b * a);
        let mut beta = deg(f64::from(beta_ratio).acos());
        if alpha.is_nan() {
            alpha = 0.0;
        }
        if beta.is_nan() {
            beta = 0.0;
        }
        let mut head = [0.0, f64::from(b), 0.0];
        head = rotate(head, f64::from(beta + 180.0), 0);
        head[1] += f64::from(a);
        head = rotate(head, f64::from(alpha - 90.0), 0);
        head = rotate(head, f64::from(base), 1);
        if ceiling {
            head = rotate(head, 180.0, 0);
        }
        let head_pos = [0, 1, 2].map(|i| head[i] + origin[i]);
        let mut head_diff = [0, 1, 2].map(|i| target[i] - head_pos[i]);
        if ceiling {
            head_diff[1] = -head_diff[1];
        }
        let horizontal_head =
            (head_diff[0] * head_diff[0] + head_diff[2] * head_diff[2]).sqrt() as f32;
        let head_angle = (f64::from(alpha) + f64::from(beta) + 135.0
            - f64::from(deg(head_diff[1].atan2(f64::from(horizontal_head)))))
            as f32;
        Angles {
            base,
            lower: alpha,
            upper: beta,
            head: -head_angle,
        }
    }

    /// The angles of the arm at `point` (`getTargetAngles`).
    pub fn of(arm: [i32; 3], point: &Point, ceiling: bool) -> Angles {
        let (at, claw) = point.interaction();
        Angles::new(arm, at, claw, ceiling)
    }
}

/// `ArmBlockEntity.Phase`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    SearchInputs,
    MoveToInput,
    SearchOutputs,
    MoveToOutput,
    Dancing,
}

impl Phase {
    pub fn index(self) -> u8 {
        self as u8
    }
    pub fn from_index(i: u8) -> Phase {
        match i {
            1 => Phase::MoveToInput,
            2 => Phase::SearchOutputs,
            3 => Phase::MoveToOutput,
            4 => Phase::Dancing,
            _ => Phase::SearchInputs,
        }
    }
}

/// `ArmBlockEntity.SelectionMode` (round robin by default).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    #[default]
    RoundRobin,
    ForcedRoundRobin,
    PreferFirst,
}

/// A stack the arm holds or moves.
pub type Stack = (String, u8);

/// What an arm's tick asks of its points.
pub trait World {
    /// `isValid`: the block at the point is still of its type (`canCreatePoint`).
    fn valid(&mut self, p: &Point) -> bool;
    /// `getSlotCount`: the slots of its inventory (0 for deposit-only points).
    fn slots(&mut self, p: &Point) -> usize;
    /// `extract(slot, amount, simulate)`: up to `amount` from slot `slot`.
    fn extract(&mut self, p: &Point, slot: usize, amount: u8, simulate: bool) -> Option<Stack>;
    /// `insert(stack, simulate)`: the count of the stack left over.
    fn insert(&mut self, p: &Point, item: &str, count: u8, simulate: bool) -> u8;
    /// `keepAlive` while the arm reaches for an input (a belt holds its first item there).
    fn keep_alive(&mut self, p: &Point) {
        let _ = p;
    }
}

/// A mechanical arm's block entity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arm {
    pub inputs: Vec<Point>,
    pub outputs: Vec<Point>,
    pub phase: Phase,
    /// `chasedPointProgress` (1 at the target) and `chasedPointIndex` (-1 none).
    pub progress: f32,
    pub index: i32,
    pub held: Option<Stack>,
    pub selection: Selection,
    pub last_input: i32,
    pub last_output: i32,
    /// `redstoneLocked`.
    pub powered: bool,
    pub ceiling: bool,
    /// `lazyTickCounter`.
    pub lazy: i8,
    /// The pose the arm comes from (`previousTarget`, None for `NO_TARGET`) and its base's yaw
    /// (`previousBaseAngle`), as clients take them when the target changes.
    pub from: Option<Angles>,
    pub from_base: f32,
    /// The last search found nothing to do: it sleeps until woken.
    pub idle: bool,
}

impl Arm {
    pub fn new(ceiling: bool) -> Arm {
        Arm {
            progress: 0.0,
            index: 0,
            last_input: -1,
            last_output: -1,
            ceiling,
            lazy: LAZY_RATE as i8,
            ..Default::default()
        }
    }

    /// `getTargetedInteractionPoint`.
    pub fn target(&self) -> Option<&Point> {
        let i = usize::try_from(self.index).ok()?;
        match self.phase {
            Phase::MoveToInput => self.inputs.get(i),
            Phase::MoveToOutput => self.outputs.get(i),
            _ => None,
        }
    }

    /// The angles of the target (`NO_TARGET` when none) for an arm at `pos`.
    pub fn target_angles(&self, pos: [i32; 3]) -> Option<Angles> {
        self.target().map(|p| Angles::of(pos, p, self.ceiling))
    }

    /// Clients' `read` when the index or phase changes: the pose it leaves is the point it was
    /// at, and its base's yaw is that point's (else it keeps the yaw it had).
    fn retarget(&mut self, pos: [i32; 3], phase: Phase, index: i32) {
        let before = self.target_angles(pos);
        self.from = before;
        if let Some(a) = before {
            self.from_base = a.base;
        }
        self.phase = phase;
        self.index = index;
        self.progress = 0.0;
    }

    /// `tickMovementProgress` on the server: true when the target was reached this tick.
    fn tick_movement(&mut self, speed: f32) -> bool {
        let before = self.progress >= 1.0;
        self.progress += speed.abs().min(256.0) / 1024.0;
        if self.progress > 1.0 {
            self.progress = 1.0;
        }
        !before && self.progress >= 1.0
    }

    /// Whether ticking it would do anything: moving with speed, or a target reached that it
    /// still has to act on, or a search not yet found fruitless.
    pub fn busy(&self, speed: f32) -> bool {
        if self.progress < 1.0 {
            return speed != 0.0;
        }
        match self.phase {
            Phase::MoveToInput | Phase::MoveToOutput => true,
            _ => !self.idle,
        }
    }

    /// `ArmBlockEntity.tick` on the server for an arm at `pos` turning at `speed`. True when
    /// clients must see it again (`sendData`).
    pub fn tick(&mut self, pos: [i32; 3], speed: f32, world: &mut impl World) -> bool {
        let mut changed = false;
        // SmartBlockEntity.tick: the lazy tick first.
        let lazy = self.lazy <= 0;
        self.lazy -= 1;
        if lazy {
            self.lazy = LAZY_RATE as i8;
            changed |= self.lazy_tick(pos, world);
        }
        let reached = self.tick_movement(speed);
        if self.progress < 1.0 {
            if self.phase == Phase::MoveToInput
                && let Some(p) = self.target().copied()
            {
                world.keep_alive(&p);
            }
            return changed;
        }
        match self.phase {
            Phase::MoveToInput => changed |= self.collect(pos, world),
            Phase::MoveToOutput => changed |= self.deposit(pos, world),
            Phase::SearchInputs | Phase::Dancing => changed |= self.search_for_item(pos, world),
            Phase::SearchOutputs => {}
        }
        if reached {
            changed |= self.lazy_tick(pos, world);
        }
        changed
    }

    /// `lazyTick`: an arm at least half way to rest looks for where to put what it holds.
    fn lazy_tick(&mut self, pos: [i32; 3], world: &mut impl World) -> bool {
        if self.progress < 0.5 {
            return false;
        }
        if self.phase == Phase::SearchOutputs {
            return self.search_for_destination(pos, world);
        }
        false
    }

    /// `getDistributableAmount`: what slot `slot` gives that the outputs would take.
    fn distributable(&mut self, p: &Point, slot: usize, world: &mut impl World) -> u8 {
        let Some((item, count)) = world.extract(p, slot, 64, true) else {
            return 0;
        };
        let mut left = count;
        for k in 0..self.outputs.len() {
            let o = self.outputs[k];
            if world.valid(&o) {
                left = world.insert(&o, &item, left, true);
            }
            if left == 0 {
                break;
            }
        }
        count - left
    }

    /// `searchForItem`.
    fn search_for_item(&mut self, pos: [i32; 3], world: &mut impl World) -> bool {
        self.idle = true;
        if self.powered {
            return false;
        }
        let start = if self.selection == Selection::PreferFirst {
            0
        } else {
            self.last_input + 1
        };
        let len = self.inputs.len() as i32;
        let scan = if self.selection == Selection::ForcedRoundRobin {
            self.last_input + 2
        } else {
            len
        }
        .min(len);
        let mut found = false;
        'points: for i in start.max(0)..scan {
            let p = self.inputs[i as usize];
            if !world.valid(&p) {
                continue;
            }
            for slot in 0..world.slots(&p) {
                if self.distributable(&p, slot, world) == 0 {
                    continue;
                }
                self.select(pos, true, i);
                found = true;
                break 'points;
            }
        }
        if !found && self.selection == Selection::RoundRobin {
            self.last_input = -1;
        }
        if self.last_input == len - 1 {
            self.last_input = -1;
        }
        found
    }

    /// `searchForDestination`.
    fn search_for_destination(&mut self, pos: [i32; 3], world: &mut impl World) -> bool {
        self.idle = true;
        let Some((item, count)) = self.held.clone() else {
            return false;
        };
        let start = if self.selection == Selection::PreferFirst {
            0
        } else {
            self.last_output + 1
        };
        let len = self.outputs.len() as i32;
        let scan = if self.selection == Selection::ForcedRoundRobin {
            self.last_output + 2
        } else {
            len
        }
        .min(len);
        let mut found = false;
        for i in start.max(0)..scan {
            let p = self.outputs[i as usize];
            if !world.valid(&p) {
                continue;
            }
            if world.insert(&p, &item, count, true) == count {
                continue;
            }
            self.select(pos, false, i);
            found = true;
            break;
        }
        if !found && self.selection == Selection::RoundRobin {
            self.last_output = -1;
        }
        if self.last_output == len - 1 {
            self.last_output = -1;
        }
        found
    }

    /// `selectIndex`.
    fn select(&mut self, pos: [i32; 3], input: bool, index: i32) {
        let phase = if input {
            Phase::MoveToInput
        } else {
            Phase::MoveToOutput
        };
        self.retarget(pos, phase, index);
        self.idle = false;
        if input {
            self.last_input = index;
        } else {
            self.last_output = index;
        }
    }

    /// `depositItem`.
    fn deposit(&mut self, pos: [i32; 3], world: &mut impl World) -> bool {
        if let Some(p) = self.target().copied()
            && world.valid(&p)
            && let Some((item, count)) = self.held.take()
        {
            let left = world.insert(&p, &item, count, false);
            self.held = (left > 0).then_some((item, left));
        }
        let phase = if self.held.is_none() {
            Phase::SearchInputs
        } else {
            Phase::SearchOutputs
        };
        self.retarget(pos, phase, -1);
        self.idle = false;
        true
    }

    /// `collectItem`.
    fn collect(&mut self, pos: [i32; 3], world: &mut impl World) -> bool {
        if let Some(p) = self.target().copied()
            && world.valid(&p)
        {
            for slot in 0..world.slots(&p) {
                let amount = self.distributable(&p, slot, world);
                if amount == 0 {
                    continue;
                }
                self.held = world.extract(&p, slot, amount, false);
                self.retarget(pos, Phase::SearchOutputs, -1);
                self.idle = false;
                return true;
            }
        }
        self.retarget(pos, Phase::SearchInputs, -1);
        self.idle = false;
        true
    }

    /// `redstoneUpdate`: a new signal locks or frees it; freed, it looks for items at once.
    pub fn set_powered(&mut self, pos: [i32; 3], powered: bool, world: &mut impl World) -> bool {
        if powered == self.powered {
            return false;
        }
        self.powered = powered;
        if !powered {
            self.search_for_item(pos, world);
        }
        true
    }

    /// `ArmBlock.useItemOn` with something held: it goes to the player and the arm looks for
    /// inputs again.
    pub fn take_held(&mut self) -> Option<Stack> {
        let held = self.held.take()?;
        self.phase = Phase::SearchInputs;
        self.idle = false;
        Some(held)
    }

    /// The points an arm at `pos` may use (`ArmInteractionPointHandler.flushSettings`): closer
    /// than the range, inputs not deposit-only.
    pub fn set_points(&mut self, pos: [i32; 3], inputs: Vec<Point>, outputs: Vec<Point>) {
        let near = |p: &Point| {
            let d = [0, 1, 2].map(|i| i64::from(p.pos[i] - pos[i]));
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < i64::from(RANGE * RANGE)
        };
        self.inputs = inputs
            .into_iter()
            .filter(|p| near(p) && !p.kind.deposit_only())
            .collect();
        self.outputs = outputs.into_iter().filter(near).collect();
        self.idle = false;
    }

    /// The points it reaches, inputs first.
    pub fn points(&self) -> impl Iterator<Item = &Point> {
        self.inputs.iter().chain(self.outputs.iter())
    }

    /// Whether a sleeping arm must still be woken by the lazy tick: one of the points it waits
    /// on does not report its changes.
    pub fn polls(&self) -> bool {
        let list = if self.held.is_some() {
            &self.outputs
        } else {
            &self.inputs
        };
        list.iter().any(|p| !p.kind.reported())
    }
}

/// The arms of a dimension: each arm, the awake ones (the only ones that tick), the arms each
/// point position wakes, the sleeping arms only a lazy tick wakes, and the arms clients must
/// see again.
#[derive(Clone, Debug, Default)]
pub struct Arms {
    pub arms: HashMap<[i32; 3], Arm>,
    pub running: Vec<[i32; 3]>,
    pub watch: HashMap<[i32; 3], Vec<[i32; 3]>>,
    pub pollers: Vec<[i32; 3]>,
    pub changes: Vec<[i32; 3]>,
    /// Arms saved with a column, waiting for their block.
    pub saved: HashMap<[i32; 3], Arm>,
}

impl Arms {
    /// An arm placed or loaded at `pos`.
    pub fn placed(&mut self, pos: [i32; 3], ceiling: bool) {
        if let Some(a) = self.arms.get_mut(&pos) {
            a.ceiling = ceiling;
            return;
        }
        let arm = match self.saved.remove(&pos) {
            Some(mut a) => {
                a.ceiling = ceiling;
                a
            }
            None => Arm::new(ceiling),
        };
        self.arms.insert(pos, arm);
        self.watch_points(pos);
        self.wake(pos);
        self.changes.push(pos);
    }

    /// An arm gone: what it held, to drop.
    pub fn removed(&mut self, pos: [i32; 3]) -> Option<Stack> {
        let arm = self.arms.remove(&pos)?;
        self.unwatch(pos, &arm);
        self.running.retain(|p| *p != pos);
        self.pollers.retain(|p| *p != pos);
        self.changes.push(pos);
        arm.held
    }

    fn unwatch(&mut self, pos: [i32; 3], arm: &Arm) {
        for p in arm.points() {
            if let Some(list) = self.watch.get_mut(&p.pos) {
                list.retain(|a| *a != pos);
                if list.is_empty() {
                    self.watch.remove(&p.pos);
                }
            }
        }
    }

    fn watch_points(&mut self, pos: [i32; 3]) {
        let Some(arm) = self.arms.get(&pos) else {
            return;
        };
        let points: Vec<[i32; 3]> = arm.points().map(|p| p.pos).collect();
        for p in points {
            let list = self.watch.entry(p).or_default();
            if !list.contains(&pos) {
                list.push(pos);
            }
        }
    }

    /// New points for the arm at `pos` (the debug command, standing in for the arm item).
    pub fn set_points(&mut self, pos: [i32; 3], inputs: Vec<Point>, outputs: Vec<Point>) {
        let Some(mut arm) = self.arms.remove(&pos) else {
            return;
        };
        self.unwatch(pos, &arm);
        arm.set_points(pos, inputs, outputs);
        self.arms.insert(pos, arm);
        self.watch_points(pos);
        self.wake(pos);
        self.changes.push(pos);
    }

    /// The arm at `pos` ticks again.
    pub fn wake(&mut self, pos: [i32; 3]) {
        if let Some(a) = self.arms.get_mut(&pos) {
            a.idle = false;
            if !self.running.contains(&pos) {
                self.running.push(pos);
            }
            self.pollers.retain(|p| *p != pos);
        }
    }

    /// What a point at `at` holds or takes changed: the arms using it wake. A miss is one
    /// lookup in an empty map when no arm is placed.
    pub fn touched(&mut self, at: [i32; 3]) {
        if self.watch.is_empty() {
            return;
        }
        let Some(list) = self.watch.get(&at) else {
            return;
        };
        for pos in list.clone() {
            self.wake(pos);
        }
    }

    /// Each tick: the lazy tick comes round for sleeping arms whose points don't report changes.
    pub fn poll(&mut self, game_time: u64) {
        if self.pollers.is_empty() || !game_time.is_multiple_of(u64::from(LAZY_RATE) + 1) {
            return;
        }
        for pos in std::mem::take(&mut self.pollers) {
            self.wake(pos);
        }
    }

    /// Ticks the awake arms (`speed_of` their kinetic speed); those that have nothing left to do
    /// sleep, those clients must see again are listed in `changes`.
    pub fn tick(&mut self, speed_of: impl Fn([i32; 3]) -> f32, world: &mut impl World) {
        if self.running.is_empty() {
            return;
        }
        let running = std::mem::take(&mut self.running);
        for pos in running {
            let Some(mut arm) = self.arms.remove(&pos) else {
                continue;
            };
            let speed = speed_of(pos);
            let changed = arm.tick(pos, speed, world);
            if changed {
                self.changes.push(pos);
            }
            let busy = arm.busy(speed);
            let polls = !busy && arm.idle && arm.polls();
            self.arms.insert(pos, arm);
            if busy {
                if !self.running.contains(&pos) {
                    self.running.push(pos);
                }
            } else if polls && !self.pollers.contains(&pos) {
                self.pollers.push(pos);
            }
        }
    }

    /// The arms of a column, to save.
    pub fn column(&self, cx: i32, cz: i32) -> Vec<([i32; 3], Arm)> {
        let mut v: Vec<([i32; 3], Arm)> = self
            .arms
            .iter()
            .filter(|(p, _)| (p[0] >> 4, p[2] >> 4) == (cx, cz))
            .map(|(p, a)| (*p, a.clone()))
            .collect();
        v.sort_by_key(|(p, _)| *p);
        v
    }

    /// A column left: its arms are forgotten (they were saved with it).
    pub fn column_unloaded(&mut self, cx: i32, cz: i32) {
        let gone: Vec<[i32; 3]> = self
            .arms
            .keys()
            .filter(|p| (p[0] >> 4, p[2] >> 4) == (cx, cz))
            .copied()
            .collect();
        for pos in gone {
            if let Some(arm) = self.arms.remove(&pos) {
                self.unwatch(pos, &arm);
            }
            self.running.retain(|p| *p != pos);
            self.pollers.retain(|p| *p != pos);
        }
        self.saved.retain(|p, _| (p[0] >> 4, p[2] >> 4) != (cx, cz));
    }
}

/// The arm's parts as `ArmRenderer` poses them, in block units with the base's yaw left out (the
/// GPU turns every part about the block's vertical axis by the yaw, negated on the ceiling):
/// base, lower body, upper body, claw, upper grip, lower grip, and the held item's pose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Poses {
    pub base: Mat4,
    pub lower: Mat4,
    pub upper: Mat4,
    pub claw: Mat4,
    pub upper_grip: Mat4,
    pub lower_grip: Mat4,
    pub item: Mat4,
}

/// The joints at a moment: Create's renderer values (`lowerArmAngle - 135`, `upperArmAngle -
/// 90`, `headAngle`).
pub fn poses(a: &Angles, ceiling: bool, held: Option<bool>) -> Poses {
    let rx = |d: f32| Mat4::from_rotation_x(d.to_radians());
    let t = |x: f32, y: f32, z: f32| Mat4::from_translation(Vec3::new(x, y, z));
    let mut m = t(0.5, 0.5, 0.5);
    if ceiling {
        m *= rx(180.0);
    }
    // transformBase (the yaw is the GPU's).
    m *= t(0.0, 4.0 / 16.0, 0.0);
    let base = m;
    // transformLowerArm(lower - 135): rotateX(lower).
    m *= t(0.0, 2.0 / 16.0, 0.0) * rx(a.lower);
    let lower = m;
    // transformUpperArm(upper - 90): rotateX(upper - 180).
    m *= t(0.0, 0.0, -14.0 / 16.0) * rx(a.upper - 90.0 - 90.0);
    let upper = m;
    // transformHead.
    m *= t(0.0, 0.0, -15.0 / 16.0) * rx(a.head - 45.0);
    let claw = m;
    let grip = |flip: f32| {
        let open = match held {
            Some(true) => 3.0 / 16.0,
            Some(false) => 5.0 / 64.0,
            None => 1.0 / 16.0,
        };
        m * t(0.0, -flip * open, -6.0 / 16.0)
    };
    let block_item = held == Some(true);
    let scale = if block_item { 0.5 } else { 0.625 };
    let item = m
        * rx(90.0)
        * t(
            0.0,
            if block_item {
                -9.0 / 16.0
            } else {
                -10.0 / 16.0
            },
            0.0,
        )
        * Mat4::from_scale(Vec3::splat(scale));
    Poses {
        base,
        lower,
        upper,
        claw,
        upper_grip: grip(-1.0),
        lower_grip: grip(1.0),
        item,
    }
}

/// The arm's joints and yaw at `progress` of a move from `from` (None: `NO_TARGET`) to `to`, its
/// base starting at `from_base` (`tickMovementProgress` on the client): the base turns the
/// whole way; the joints first fold to rest, then reach out.
pub fn at_progress(
    from: Option<Angles>,
    from_base: f32,
    to: Option<Angles>,
    progress: f32,
) -> Angles {
    let base = angle_lerp(progress, from_base, to.map_or(from_base, |t| t.base));
    let (prev, target) = if progress < 0.5 {
        (from.unwrap_or(NO_TARGET), NO_TARGET)
    } else {
        (NO_TARGET, to.unwrap_or(NO_TARGET))
    };
    let p = if progress == 1.0 {
        1.0
    } else {
        (progress % 0.5) * 2.0
    };
    let lerp = |a: f32, b: f32| a + (b - a) * p;
    Angles {
        base,
        lower: lerp(prev.lower, target.lower),
        upper: lerp(prev.upper, target.upper),
        head: angle_lerp(p, prev.head % 360.0, target.head % 360.0),
    }
}

/// The progress a tick adds at kinetic speed `speed`.
pub fn step(speed: f32) -> f32 {
    speed.abs().min(256.0) / 1024.0
}

/// The mod-message channel that brings arms to clients.
pub const CHANNEL: &str = "create:arm";

/// An arm as clients draw it: where it is, which way it hangs, its move (from, to, the base's
/// starting yaw, its progress when sent) and what it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub pos: [i32; 3],
    pub gone: bool,
    pub ceiling: bool,
    pub phase: Phase,
    pub progress: f32,
    pub from: Option<Angles>,
    pub to: Option<Angles>,
    pub from_base: f32,
    pub held: Option<String>,
}

impl Shown {
    /// What clients see of the arm at `pos`.
    pub fn of(pos: [i32; 3], arm: &Arm) -> Shown {
        Shown {
            pos,
            gone: false,
            ceiling: arm.ceiling,
            phase: arm.phase,
            progress: arm.progress,
            from: arm.from,
            to: arm.target_angles(pos),
            from_base: arm.from_base,
            held: arm.held.as_ref().map(|h| h.0.clone()),
        }
    }
}

fn put_angles(out: &mut Vec<u8>, a: Option<Angles>) {
    match a {
        Some(a) => {
            out.push(1);
            for v in [a.base, a.lower, a.upper, a.head] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        None => out.push(0),
    }
}

/// Arms as messages (each well under the 30,000-byte limit).
pub fn encode(all: &[Shown]) -> Vec<Vec<u8>> {
    all.chunks(64)
        .map(|chunk| {
            let mut out = Vec::new();
            for s in chunk {
                for c in s.pos {
                    out.extend_from_slice(&c.to_le_bytes());
                }
                out.push(u8::from(s.gone) | u8::from(s.ceiling) << 1);
                out.push(s.phase.index());
                out.extend_from_slice(&s.progress.to_le_bytes());
                out.extend_from_slice(&s.from_base.to_le_bytes());
                put_angles(&mut out, s.from);
                put_angles(&mut out, s.to);
                let held = s.held.as_deref().unwrap_or("");
                let n = held.len().min(255);
                out.push(n as u8);
                out.extend_from_slice(&held.as_bytes()[..n]);
            }
            out
        })
        .collect()
}

/// A message's arms; a malformed tail is dropped.
pub fn decode(mut d: &[u8]) -> Vec<Shown> {
    fn take<'a>(d: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
        if d.len() < n {
            return None;
        }
        let (a, b) = d.split_at(n);
        *d = b;
        Some(a)
    }
    fn f32_of(d: &mut &[u8]) -> Option<f32> {
        Some(f32::from_le_bytes(take(d, 4)?.try_into().ok()?))
    }
    fn angles(d: &mut &[u8]) -> Option<Option<Angles>> {
        if take(d, 1)?[0] == 0 {
            return Some(None);
        }
        Some(Some(Angles {
            base: f32_of(d)?,
            lower: f32_of(d)?,
            upper: f32_of(d)?,
            head: f32_of(d)?,
        }))
    }
    let mut out = Vec::new();
    let one = |d: &mut &[u8]| -> Option<Shown> {
        let p = take(d, 12)?;
        let int = |i: usize| i32::from_le_bytes(p[i..i + 4].try_into().unwrap());
        let pos = [int(0), int(4), int(8)];
        let flags = take(d, 1)?[0];
        let phase = Phase::from_index(take(d, 1)?[0]);
        let progress = f32_of(d)?;
        let from_base = f32_of(d)?;
        let from = angles(d)?;
        let to = angles(d)?;
        let n = usize::from(take(d, 1)?[0]);
        let held = String::from_utf8_lossy(take(d, n)?).into_owned();
        Some(Shown {
            pos,
            gone: flags & 1 != 0,
            ceiling: flags & 2 != 0,
            phase,
            progress,
            from,
            to,
            from_base,
            held: (!held.is_empty()).then_some(held),
        })
    };
    while !d.is_empty() {
        match one(&mut d) {
            Some(s) => out.push(s),
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Depots as a world: their stack by position (a depot takes into its one held slot while
    /// it holds the same item, up to 64).
    #[derive(Default)]
    struct Depots {
        stacks: HashMap<[i32; 3], Stack>,
        locked: Vec<[i32; 3]>,
    }

    impl World for Depots {
        fn valid(&mut self, _: &Point) -> bool {
            true
        }
        fn slots(&mut self, p: &Point) -> usize {
            if p.kind.deposit_only() { 0 } else { 1 }
        }
        fn extract(&mut self, p: &Point, _: usize, amount: u8, simulate: bool) -> Option<Stack> {
            let s = self.stacks.get_mut(&p.pos)?;
            let n = s.1.min(amount);
            let out = (s.0.clone(), n);
            if !simulate {
                s.1 -= n;
                if s.1 == 0 {
                    self.stacks.remove(&p.pos);
                }
            }
            Some(out)
        }
        fn insert(&mut self, p: &Point, item: &str, count: u8, simulate: bool) -> u8 {
            let have = self.stacks.get(&p.pos).cloned();
            if have.as_ref().is_some_and(|h| h.0 != item) {
                return count;
            }
            let room = 64 - have.as_ref().map_or(0, |h| h.1);
            let n = count.min(room);
            if !simulate && n > 0 {
                self.stacks
                    .insert(p.pos, (item.to_owned(), have.map_or(0, |h| h.1) + n));
            }
            count - n
        }
        fn keep_alive(&mut self, p: &Point) {
            self.locked.push(p.pos);
        }
    }

    fn depot(pos: [i32; 3]) -> Point {
        Point {
            pos,
            kind: Kind::Depot,
            facing: None,
        }
    }

    #[test]
    fn an_arm_carries_a_stack_from_input_to_output() {
        let pos = [0, 0, 0];
        let mut arm = Arm::new(false);
        arm.set_points(pos, vec![depot([2, 0, 0])], vec![depot([-2, 0, 0])]);
        let mut w = Depots::default();
        w.stacks
            .insert([2, 0, 0], ("minecraft:iron_ingot".into(), 16));
        // At 64 rpm the arm moves 1/16 a move per tick: 16 ticks to reach a point.
        let speed = 64.0;
        assert_eq!(step(speed), 1.0 / 16.0);
        // A fresh arm starts at progress 0: 16 ticks to settle, then it searches on the 16th.
        let mut ticks = 0;
        while arm.phase == Phase::SearchInputs {
            arm.tick(pos, speed, &mut w);
            ticks += 1;
        }
        assert_eq!(ticks, 16);
        assert_eq!((arm.phase, arm.index), (Phase::MoveToInput, 0));
        assert_eq!(arm.progress, 0.0);
        // Reaching for it keeps the belt item there; 16 ticks later it takes the whole stack.
        for _ in 0..15 {
            arm.tick(pos, speed, &mut w);
        }
        assert_eq!(w.locked.len(), 15);
        assert!(arm.held.is_none());
        arm.tick(pos, speed, &mut w);
        assert_eq!(arm.held, Some(("minecraft:iron_ingot".into(), 16)));
        assert_eq!((arm.phase, arm.index), (Phase::SearchOutputs, -1));
        assert!(!w.stacks.contains_key(&[2, 0, 0]));
        // It leaves the input's pose; the base keeps the input's yaw.
        let at_input = Angles::of(pos, &depot([2, 0, 0]), false);
        assert_eq!(arm.from, Some(at_input));
        assert_eq!(arm.from_base, at_input.base);
        // Folding back, it looks for an output at its lazy tick (every 11 ticks, from its 11th):
        // the 44th tick of its life, 12 into this move (progress 12/16, past half way), so it
        // turns to the output before reaching rest; 16 ticks later it puts the stack down.
        let mut ticks = 0;
        while arm.phase != Phase::MoveToOutput {
            arm.tick(pos, speed, &mut w);
            ticks += 1;
        }
        assert_eq!(ticks, 12);
        assert_eq!(arm.from, None, "it comes from rest");
        for _ in 0..16 {
            arm.tick(pos, speed, &mut w);
        }
        assert_eq!(arm.phase, Phase::SearchInputs);
        assert!(arm.held.is_none());
        assert_eq!(w.stacks[&[-2, 0, 0]], ("minecraft:iron_ingot".into(), 16));
        // Nothing left: the next search finds nothing and it sleeps.
        for _ in 0..16 {
            arm.tick(pos, speed, &mut w);
        }
        assert!(!arm.busy(speed));
    }

    #[test]
    fn round_robin_takes_inputs_in_turn() {
        let pos = [0, 0, 0];
        let mut arm = Arm::new(false);
        let inputs = vec![depot([1, 0, 0]), depot([0, 0, 1]), depot([-1, 0, 0])];
        arm.set_points(pos, inputs, vec![depot([0, 0, -2])]);
        let mut w = Depots::default();
        for p in [[1, 0, 0], [0, 0, 1], [-1, 0, 0]] {
            w.stacks.insert(p, ("minecraft:dirt".into(), 1));
        }
        arm.progress = 1.0;
        let mut order = Vec::new();
        for _ in 0..400 {
            let before = (arm.phase, arm.index);
            arm.tick(pos, 256.0, &mut w);
            if arm.phase == Phase::MoveToInput && before.0 != Phase::MoveToInput {
                order.push(arm.index);
            }
        }
        assert_eq!(order, [0, 1, 2]);
        assert_eq!(w.stacks[&[0, 0, -2]].1, 3);
        // Prefer first always starts at the first: with only the last filled it still finds it.
        let mut arm = Arm::new(false);
        arm.selection = Selection::PreferFirst;
        arm.set_points(
            pos,
            vec![depot([1, 0, 0]), depot([2, 0, 0])],
            vec![depot([0, 0, -2])],
        );
        arm.progress = 1.0;
        let mut w = Depots::default();
        w.stacks.insert([2, 0, 0], ("minecraft:dirt".into(), 1));
        arm.tick(pos, 256.0, &mut w);
        assert_eq!((arm.phase, arm.index), (Phase::MoveToInput, 1));
    }

    #[test]
    fn full_outputs_and_redstone_hold_it() {
        let pos = [0, 0, 0];
        let mut arm = Arm::new(false);
        arm.set_points(pos, vec![depot([2, 0, 0])], vec![depot([-2, 0, 0])]);
        arm.progress = 1.0;
        let mut w = Depots::default();
        w.stacks.insert([2, 0, 0], ("minecraft:dirt".into(), 4));
        // The output holds something else: nothing is distributable, the arm sleeps.
        w.stacks.insert([-2, 0, 0], ("minecraft:stone".into(), 1));
        arm.tick(pos, 32.0, &mut w);
        assert_eq!(arm.phase, Phase::SearchInputs);
        assert!(!arm.busy(32.0));
        // Room again; powered it still takes nothing, freed it searches at once.
        w.stacks.remove(&[-2, 0, 0]);
        assert!(arm.set_powered(pos, true, &mut w));
        arm.idle = false;
        arm.tick(pos, 32.0, &mut w);
        assert_eq!(arm.phase, Phase::SearchInputs);
        assert!(arm.set_powered(pos, false, &mut w));
        assert_eq!(arm.phase, Phase::MoveToInput);
        // Without speed it never gets there, and costs nothing.
        assert!(!arm.busy(0.0));
    }

    #[test]
    fn points_out_of_range_or_deposit_only_inputs_are_dropped() {
        let mut arm = Arm::new(false);
        let funnel = Point {
            pos: [1, 0, 0],
            kind: Kind::Funnel,
            facing: Some(Dir::East),
        };
        arm.set_points(
            [0, 0, 0],
            vec![depot([4, 3, 0]), depot([3, 3, 2]), funnel],
            vec![funnel, depot([5, 0, 0])],
        );
        // 4² + 3² = 25 is not closer than 5; 9 + 9 + 4 = 22 is.
        assert_eq!(arm.inputs, [depot([3, 3, 2])]);
        assert_eq!(arm.outputs, [funnel]);
    }

    #[test]
    fn angles_reach_down_to_a_depot_beside_it() {
        // A depot two blocks south of an arm on the floor: the base turns to face south
        // (atan2(0, 2) + 180 = 180), the joints reach out and down.
        let a = Angles::of([0, 0, 0], &depot([0, 0, 2]), false);
        assert!((a.base - 180.0).abs() < 1e-4, "{a:?}");
        assert!(a.lower < 135.0 && a.lower > 0.0, "{a:?}");
        assert!(a.upper > 45.0, "{a:?}");
        // East: atan2(2, 0) + 180 = 270.
        let e = Angles::of([0, 0, 0], &depot([2, 0, 0]), false);
        assert!((e.base - 270.0).abs() < 1e-4);
        // On the ceiling the yaw mirrors: 180 - 270 = -90.
        let c = Angles::of([0, 4, 0], &depot([2, 2, 0]), true);
        assert!((c.base + 90.0).abs() < 1e-4);
        // Too far to reach: the segments stretch out straight (beta 180).
        let far = Angles::new([0, 0, 0], Vec3::new(8.5, 0.5, 0.5), Dir::Down, false);
        assert!((far.upper - 180.0).abs() < 1e-3, "{far:?}");
    }

    #[test]
    fn a_move_folds_then_reaches() {
        let to = Angles {
            base: 90.0,
            lower: 60.0,
            upper: 100.0,
            head: -30.0,
        };
        // Half way: at rest, the base half turned.
        let half = at_progress(None, 0.0, Some(to), 0.5);
        assert_eq!((half.lower, half.upper), (NO_TARGET.lower, NO_TARGET.upper));
        assert_eq!(half.base, 45.0);
        let end = at_progress(None, 0.0, Some(to), 1.0);
        assert_eq!(end, to);
        // The base turns the short way round: 350 → 10 passes 0.
        let short = at_progress(None, 350.0, Some(Angles { base: 10.0, ..to }), 0.5);
        assert_eq!(short.base, 360.0);
        // Without a target the base stays.
        assert_eq!(at_progress(Some(to), 90.0, None, 0.3).base, 90.0);
        assert_eq!(shortest_angle_diff(-170.0, 170.0), -20.0);
    }

    #[test]
    fn poses_chain_the_joints() {
        let p = poses(&NO_TARGET, false, None);
        // The base sits 4 px above the centre.
        let b = p.base.transform_point3(Vec3::ZERO);
        assert!((b - Vec3::new(0.5, 0.75, 0.5)).length() < 1e-6);
        // Folded at rest, the claw is back near the base and above it.
        let claw = p.claw.transform_point3(Vec3::ZERO);
        assert!(claw.y > 0.75, "{claw}");
        // Hung from the ceiling, everything is mirrored below the centre.
        let c = poses(&NO_TARGET, true, None);
        assert!(c.base.transform_point3(Vec3::ZERO).y < 0.5);
        // Holding a block item opens the grips wider.
        let open = poses(&NO_TARGET, false, Some(true));
        let d = |q: &Poses| {
            (q.upper_grip.transform_point3(Vec3::ZERO) - q.lower_grip.transform_point3(Vec3::ZERO))
                .length()
        };
        assert!(d(&open) > d(&p));
    }

    #[test]
    fn messages_round_trip() {
        let s = Shown {
            pos: [1, -64, 3],
            gone: false,
            ceiling: true,
            phase: Phase::MoveToOutput,
            progress: 0.25,
            from: None,
            to: Some(NO_TARGET),
            from_base: 12.5,
            held: Some("minecraft:dirt".into()),
        };
        let gone = Shown {
            gone: true,
            held: None,
            ..s.clone()
        };
        let m = encode(&[s.clone(), gone.clone()]);
        assert_eq!(decode(&m[0]), [s, gone]);
    }

    #[test]
    fn idle_arms_sleep_until_a_point_changes() {
        let mut arms = Arms::default();
        let pos = [0, 0, 0];
        arms.placed(pos, false);
        arms.set_points(pos, vec![depot([2, 0, 0])], vec![depot([-2, 0, 0])]);
        let mut d = Depots::default();
        for _ in 0..20 {
            arms.tick(|_| 64.0, &mut d);
        }
        assert!(arms.running.is_empty(), "nothing to take: asleep");
        assert!(arms.pollers.is_empty(), "depots report their changes");
        // A stack arrives on the input depot: the arm wakes and takes it.
        d.stacks.insert([2, 0, 0], ("minecraft:dirt".into(), 2));
        arms.touched([1, 0, 0]);
        assert!(arms.running.is_empty(), "not one of its points");
        arms.touched([2, 0, 0]);
        assert_eq!(arms.running, [pos]);
        for _ in 0..20 {
            arms.tick(|_| 64.0, &mut d);
        }
        assert_eq!(arms.arms[&pos].held, Some(("minecraft:dirt".into(), 2)));
        // A millstone input is not reported: the arm is woken by its lazy tick instead.
        let mut a2 = Arms::default();
        a2.placed(pos, false);
        let mill = Point {
            pos: [0, 0, 2],
            kind: Kind::Millstone,
            facing: None,
        };
        a2.set_points(pos, vec![mill], vec![depot([0, 0, -2])]);
        let mut empty = Depots::default();
        for _ in 0..20 {
            a2.tick(|_| 64.0, &mut empty);
        }
        assert!(a2.running.is_empty());
        assert_eq!(a2.pollers, [pos]);
        a2.poll(10);
        assert!(a2.running.is_empty());
        a2.poll(11);
        assert_eq!(a2.running, [pos]);
        // Removed: what it held drops, nothing watches its points any more.
        assert_eq!(arms.removed(pos), Some(("minecraft:dirt".into(), 2)));
        assert!(arms.watch.is_empty());
    }
}
