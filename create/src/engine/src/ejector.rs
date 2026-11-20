//! Create's weighted ejector (`EjectorBlockEntity`, `EntityLauncher`, `EjectorRenderer`): a depot
//! on a hinged plate that, once charged, throws what it holds along an arc to a target block, the
//! items flying a few ticks and landing in what is there (a depot, a belt) or as items at the
//! end of the arc. Its stacks are a depot's (the block entity's slots, `depot.rs`); this keeps
//! the launcher, the plate and the items in flight. As the depot behaviour it extends, the
//! shared transport walk ticks it with its block entity, whether or not anything moves.

use crate::kinetics::{Axis, Dir};
use crate::place::{mth_cos, mth_sin};

/// `maxEjectorDistance`.
pub const MAX_DISTANCE: i32 = 32;
/// `ejectorScanInterval`: ticks between the trajectory checks of one launched stack.
pub const SCAN_INTERVAL: u32 = 120;

/// Catnip's `AngleHelper.horizontalAngle`: south 0°, west 90°, north 180°, east 270°, the x
/// axis negated.
pub fn horizontal_angle(d: Dir) -> f32 {
    let y_rot = match d {
        Dir::South => 0.0,
        Dir::West => 90.0,
        Dir::North => 180.0,
        Dir::East => 270.0,
        _ => return 0.0,
    };
    if d.axis() == Axis::X { -y_rot } else { y_rot }
}

/// Catnip's `VecHelper.rotate(vec, deg, Axis.Y)`, with `Mth`'s sine table.
pub fn rotate_y(v: [f64; 3], deg: f64) -> [f64; 3] {
    if deg == 0.0 {
        return v;
    }
    let angle = (deg / 180.0 * std::f64::consts::PI) as f32;
    let (sin, cos) = (f64::from(mth_sin(angle)), f64::from(mth_cos(angle)));
    let [x, y, z] = v;
    [x * cos + z * sin, y, z * cos - x * sin]
}

/// `EntityLauncher`: the arc to a target `h` blocks ahead and `v` up, fitted to vanilla's item
/// drag (x × 0.91, y − 0.08 then × 0.98 a tick).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Launcher {
    pub h: i32,
    pub v: i32,
    y_motion: f64,
    x_motion: f64,
    total: f64,
}

impl Default for Launcher {
    /// `new EntityLauncher(1, 0)`.
    fn default() -> Launcher {
        Launcher::new(1, 0)
    }
}

impl Launcher {
    pub fn new(h: i32, v: i32) -> Launcher {
        let mut l = Launcher {
            h,
            v,
            y_motion: 0.0,
            x_motion: 0.0,
            total: 0.0,
        };
        l.recalculate();
        l
    }

    /// `clamp(max)`.
    pub fn clamp(&mut self, max: i32) {
        *self = Launcher::new(self.h.min(max), self.v.signum() * self.v.abs().min(max));
    }

    /// `recalculateTrajectory`.
    fn recalculate(&mut self) {
        let x_target = f64::from(self.h);
        let y_target = f64::from(self.v);
        let x_error = (-0.0007 * (x_target + 0.5).powi(2) + 0.484)
            - (y_target.min(5.0) / 5.0)
                * (0.076 * x_target - 0.0014 * x_target * x_target).min(1.0);
        let y_peak =
            (y_target + (x_target + 0.5) / 8.0).max(0.0) + if x_target <= 1.0 { 1.0 } else { 4.0 };
        self.y_motion = (2.0 * y_peak / 13.0).sqrt() + 0.015 * y_peak;
        let t_peak = (98.0 / (25.0 * self.y_motion + 98.0)).ln()
            / (2.0 * 7f64.ln() - 2.0 * 5f64.ln() - 2f64.ln());
        self.total =
            (y_peak - self.y_motion).sqrt() * 4.87 + 0.115 * (y_peak - self.y_motion) + t_peak;
        self.x_motion = (x_target - x_error + 0.5) / (-10.6033 * (-1.0 + 0.91f64.powf(self.total)));
    }

    pub fn total_ticks(&self) -> f64 {
        self.total
    }

    pub fn x(&self, t: f64) -> f32 {
        (self.x_motion * -10.6033 * (-1.0 + 0.91f64.powf(t))) as f32
    }

    pub fn y(&self, t: f64) -> f32 {
        let f = 0.98f64.powf(t);
        (self.y_motion * -49.4983 * f + 49.4983 * self.y_motion - 194.033 * f - 3.92 * t + 194.033)
            as f32
    }

    pub fn dx(&self, t: f64) -> f32 {
        (self.x_motion * 0.91f64.powf(t)) as f32
    }

    pub fn dy(&self, t: f64) -> f32 {
        let f = 0.98f64.powf(t);
        (self.y_motion * f + ((f - 1.0) / (0.98 - 1.0)) * -0.0784) as f32
    }

    /// `getGlobalPos(t, d, launcher)`: where a launched stack is `t` ticks after leaving the
    /// ejector at `at`, thrown away from `d` (the ejector's back).
    pub fn global_pos(&self, t: f64, d: Dir, at: [i32; 3]) -> [f64; 3] {
        let start = at.map(|c| f64::from(c) + 0.5);
        let (xt, yt) = (f64::from(self.x(t)), f64::from(self.y(t)));
        let progress = (t / self.total).clamp(0.0, 1.0);
        let correction = progress.powi(3);
        let v = [
            0.0,
            yt + (f64::from(self.v) - yt) * correction * 0.5,
            xt + (f64::from(self.h) - xt) * correction,
        ];
        let r = rotate_y(v, 180.0 + f64::from(horizontal_angle(d)));
        [r[0] + start[0], r[1] + start[1], r[2] + start[2]]
    }

    /// `applyMotion`: the motion an entity on the plate is thrown with (`facing` the ejector's).
    pub fn entity_motion(&self, facing: Dir) -> [f64; 3] {
        let v = rotate_y(
            [0.0, self.y_motion, self.x_motion],
            f64::from(horizontal_angle(facing)),
        );
        [v[0] * 0.91, v[1] * 0.98, v[2] * 0.91]
    }

    /// `getGlobalVelocity(t, d, launcher)`.
    pub fn global_velocity(&self, t: f64, d: Dir) -> [f64; 3] {
        rotate_y(
            [0.0, f64::from(self.dy(t)), f64::from(self.dx(t))],
            180.0 + f64::from(horizontal_angle(d)),
        )
    }
}

/// `EjectorBlockEntity.State`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    Charged,
    Launching,
    #[default]
    Retracting,
}

impl State {
    pub fn index(self) -> u8 {
        self as u8
    }
    pub fn from_index(i: u8) -> State {
        match i {
            0 => State::Charged,
            1 => State::Launching,
            _ => State::Retracting,
        }
    }
}

/// Catnip's `LerpedFloat` as the plate uses it: linear, chased exponentially while launching.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lid {
    pub value: f32,
    pub previous: f32,
    pub target: f32,
    pub speed: f32,
    /// Whether a chase function was set (`chase`): before that `tickChaser` only moves time on.
    pub chasing: bool,
}

impl Default for Lid {
    /// `LerpedFloat.linear().startWithValue(1)`.
    fn default() -> Lid {
        Lid {
            value: 1.0,
            previous: 1.0,
            target: 1.0,
            speed: 0.0,
            chasing: false,
        }
    }
}

/// `Mth.equal` for doubles.
fn equal(a: f64, b: f64) -> bool {
    (b - a).abs() < 1.0e-5
}

impl Lid {
    pub fn set(&mut self, v: f32) {
        self.previous = self.value;
        self.value = v;
    }
    /// `chase(target, speed, Chaser.EXP)`.
    fn chase(&mut self, target: f32, speed: f32) {
        self.target = target;
        self.speed = speed;
        self.chasing = true;
    }
    fn tick_chaser(&mut self) {
        self.previous = self.value;
        if !self.chasing {
            return;
        }
        if equal(f64::from(self.value), f64::from(self.target)) {
            self.value = self.target;
            return;
        }
        // Chaser.EXP: the gap times the speed (no cap).
        self.value = (f64::from(self.value)
            + (f64::from(self.target) - f64::from(self.value)) * f64::from(self.speed))
            as f32;
    }
    fn settled(&self) -> bool {
        equal(f64::from(self.previous), f64::from(self.value))
    }
}

/// A stack in flight (`launchedItems`): ticks since it left, the stack, and an id to track it by
/// (`trackedItem` is compared by identity).
#[derive(Clone, Debug, PartialEq)]
pub struct Flying {
    pub id: u32,
    pub time: i32,
    pub item: String,
    pub count: u8,
}

/// Where an obstacle stopped the tracked stack (`earlyTarget`): the point a quarter block out of
/// the face it hit, and the block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Early {
    pub at: [f64; 3],
    pub block: [i32; 3],
    pub time: f32,
}

/// A weighted ejector's block entity state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ejector {
    pub launcher: Launcher,
    pub state: State,
    pub lid: Lid,
    pub powered: bool,
    /// `launch`: launch at the start of the next tick.
    pub launch: bool,
    pub flying: Vec<Flying>,
    pub early: Option<Early>,
    pub scan_cooldown: u32,
    pub tracked: Option<u32>,
    pub next_id: u32,
    /// `maxStackSize`'s value: 0 throws whatever lies on it.
    pub max_stack: u8,
}

/// The depot on the ejector as its tick asks it.
pub struct Present {
    /// `getPresentStackSize`: the held stack and the outputs.
    pub count: u32,
    /// The held stack's `beltPosition` (None when it holds none).
    pub held_position: Option<f32>,
}

/// What an ejector's tick reaches in the world.
pub trait World {
    /// Its depot's contents.
    fn present(&mut self) -> Present;
    /// Whether the depot or belt at `target` would take none of the held stack
    /// (`handleInsertion` simulated); false when nothing there takes items.
    fn target_refuses(&mut self, target: [i32; 3]) -> bool;
    /// Takes everything off its depot to launch (`launchItems`): the held stack, then each
    /// output.
    fn take_all(&mut self) -> Vec<(String, u8)>;
    /// Puts a stack into what takes items at `target` from above (`handleInsertion`): the count
    /// left, or None when nothing there takes items.
    fn insert(&mut self, target: [i32; 3], item: &str, count: u8) -> Option<u8>;
    /// Drops what is left as an item at `at` moving at `motion`.
    fn drop_item(&mut self, at: [f64; 3], motion: [f64; 3], item: &str, count: u8);
    /// `Level.clip` with collider shapes from `from` to `to`: the point, block and face hit.
    fn clip(&mut self, from: [f64; 3], to: [f64; 3]) -> Option<([f64; 3], [i32; 3], Dir)>;
    /// Whether the block at `pos` is an extracting funnel (items fly through those).
    fn extracting_funnel(&mut self, pos: [i32; 3]) -> bool;
    /// A funnel on top facing the ejector's back (`getFunnelFacing(above) == facing.opposite`):
    /// None without one; else whether its belt funnel would take the whole held stack
    /// (`tryExportingToBeltFunnel(held, funnelFacing, true)` empty).
    fn funnel_on_top_takes(&mut self) -> Option<bool> {
        None
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

impl Ejector {
    /// `getWindUpSpeed` at kinetic speed `speed`.
    pub fn wind_up(&self, speed: f32) -> f32 {
        let (hd, vd) = (self.launcher.h, self.launcher.v);
        let speed_factor = speed.abs() / 256.0;
        let distance_factor = if hd == 0 && vd == 0 {
            1.0
        } else {
            ((hd * hd + vd * vd) as f32).sqrt()
        };
        speed_factor / distance_factor
    }

    /// `setTarget(h, v)`.
    pub fn set_target(&mut self, h: i32, v: i32) {
        self.launcher = Launcher::new(h.max(1), v);
        self.launcher.clamp(MAX_DISTANCE);
    }

    /// The block items are thrown at without an obstacle: `v` up and `max(1, h)` ahead.
    pub fn target(&self, pos: [i32; 3], facing: Dir) -> [i32; 3] {
        if let Some(e) = self.early {
            return e.block;
        }
        let o = facing.offset();
        let h = self.launcher.h.max(1);
        [
            pos[0] + o[0] * h,
            pos[1] + self.launcher.v,
            pos[2] + o[2] * h,
        ]
    }

    /// `getLaunchedItemLocation(time)` for an ejector at `pos` facing `facing`.
    pub fn location(&self, time: f32, pos: [i32; 3], facing: Dir) -> [f64; 3] {
        self.launcher
            .global_pos(f64::from(time), facing.opposite(), pos)
    }

    /// `getLaunchedItemMotion(time)`.
    pub fn motion(&self, time: f32, facing: Dir) -> [f64; 3] {
        self.launcher
            .global_velocity(f64::from(time), facing.opposite())
            .map(|v| v * 0.5)
    }

    /// `activate` (`ejectIfTriggered`, or something landing on it): it launches next tick.
    pub fn activate(&mut self) {
        self.launch = true;
    }

    /// `EjectorBlockEntity.tick` on the server for an ejector at `pos` facing `facing`, turning
    /// at `speed`. True when clients must see it again (`notifyUpdate` on a state change).
    pub fn tick(&mut self, pos: [i32; 3], facing: Dir, speed: f32, world: &mut impl World) -> bool {
        let prev = self.state;
        let total = 3f32.max(self.launcher.total_ticks() as f32);
        let mut changed = false;
        if self.scan_cooldown > 0 {
            self.scan_cooldown -= 1;
        }
        if self.launch {
            self.launch = false;
            changed |= self.activate_deferred(world);
        }
        let mut i = 0;
        while i < self.flying.len() {
            let time = self.flying[i].time;
            let hit = if Some(self.flying[i].id) == self.tracked {
                self.scan(time, pos, facing, world)
            } else {
                false
            };
            let max = match self.early {
                Some(e) => e.time.min(total),
                None => total,
            };
            if hit || time > max as i32 {
                let f = self.flying.remove(i);
                self.place_at_target(max, f, pos, facing, world);
                changed = true;
                continue;
            }
            self.flying[i].time += 1;
            i += 1;
        }
        if self.state == State::Launching {
            self.lid.chase(1.0, 0.8);
            self.lid.tick_chaser();
            if self.lid.value > 1.0 - 1.0 / 16.0 {
                self.state = State::Retracting;
                self.lid.set(1.0);
            }
        }
        if self.state == State::Charged {
            self.lid.set(0.0);
            self.lid.speed = 0.0;
            self.eject_if_triggered(pos, facing, world);
        }
        if self.state == State::Retracting {
            if self.lid.target == 1.0 && !self.lid.settled() {
                self.lid.tick_chaser();
            } else {
                self.lid.target = 0.0;
                self.lid.speed = 0.0;
                if self.lid.value == 0.0 {
                    self.state = State::Charged;
                    self.lid.set(0.0);
                }
                let v = (self.lid.value - self.wind_up(speed)).clamp(0.0, 1.0);
                self.lid.set(v);
            }
        }
        changed || self.state != prev
    }

    /// `activateDeferred`: what lies on it is thrown, the plate swings up.
    fn activate_deferred(&mut self, world: &mut impl World) -> bool {
        if self.state != State::Charged {
            return false;
        }
        for (item, count) in world.take_all() {
            self.add_launched(item, count);
        }
        self.lid.chase(1.0, 0.8);
        self.state = State::Launching;
        true
    }

    /// `addToLaunchedItems`: the first stack launched while no scan runs is tracked.
    fn add_launched(&mut self, item: String, count: u8) {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.tracked.is_none() && self.scan_cooldown == 0 {
            self.scan_cooldown = SCAN_INTERVAL;
            self.tracked = Some(id);
        }
        self.flying.push(Flying {
            id,
            time: 0,
            item,
            count,
        });
    }

    /// `ejectIfTriggered`: charged and not powered, with enough on it, its held stack arrived
    /// and the target able to take some of it.
    fn eject_if_triggered(&mut self, pos: [i32; 3], facing: Dir, world: &mut impl World) {
        if self.powered {
            return;
        }
        let present = world.present();
        if present.count == 0 || present.count < u32::from(self.max_stack) {
            return;
        }
        if present.held_position.is_some_and(|p| p < 0.49) {
            return;
        }
        // A funnel on top facing its back takes the held stack instead (`launchItems`); it
        // waits until all of it fits.
        if present.held_position.is_some() && world.funnel_on_top_takes() == Some(false) {
            return;
        }
        if present.held_position.is_some() && world.target_refuses(self.target(pos, facing)) {
            return;
        }
        self.activate();
    }

    /// `scanTrajectoryForObstacles(time)` for the tracked stack.
    fn scan(&mut self, time: i32, pos: [i32; 3], facing: Dir, world: &mut impl World) -> bool {
        if time <= 2 {
            return false;
        }
        let source = self.location(time as f32, pos, facing);
        let target = self.location((time + 1) as f32, pos, facing);
        let hit = world
            .clip(source, target)
            .filter(|(_, block, _)| !world.extracting_funnel(*block));
        let Some((at, block, face)) = hit else {
            if self.early.is_some_and(|e| e.time < (time + 1) as f32) {
                self.early = None;
            }
            return false;
        };
        let o = face.offset().map(|v| f64::from(v) * 0.25);
        self.early = Some(Early {
            at: [at[0] + o[0], at[1] + o[1], at[2] + o[2]],
            block,
            time: (f64::from(time) + distance(source, at) / distance(source, target)) as f32,
        });
        true
    }

    /// `placeItemAtTarget`: into what takes items at the target, the rest dropped where the arc
    /// ends (or where the obstacle was).
    fn place_at_target(
        &mut self,
        max: f32,
        f: Flying,
        pos: [i32; 3],
        facing: Dir,
        world: &mut impl World,
    ) {
        if self.tracked == Some(f.id) {
            self.tracked = None;
        }
        let left = world
            .insert(self.target(pos, facing), &f.item, f.count)
            .unwrap_or(f.count);
        if left == 0 {
            return;
        }
        let at = match self.early {
            Some(e) => e.at,
            None => self.location(max, pos, facing),
        };
        world.drop_item(at, self.motion(max, facing), &f.item, left);
    }

    /// The ejector's own stacks, dropped where they are when it breaks (`dropFlyingItems`).
    pub fn drop_flying(&mut self, pos: [i32; 3], facing: Dir) -> Vec<([f64; 3], [f64; 3], Flying)> {
        std::mem::take(&mut self.flying)
            .into_iter()
            .map(|f| {
                let t = f.time as f32;
                (self.location(t, pos, facing), self.motion(t, facing), f)
            })
            .collect()
    }

    /// The plate's angle at `t` ticks from now (`getLidProgress` × 70°) as the client runs it.
    pub fn lid_angle(&self) -> f32 {
        self.lid.value * 70.0
    }
}

/// The ejector's plate hinge (`EjectorRenderer.pivot`), in block units.
pub const PIVOT: [f32; 3] = [0.0, 11.25 / 16.0, 0.75 / 16.0];

/// How the plate of an ejector facing `facing` swings up by `angle` degrees as one world-axis
/// turn: the axis (0 x, 2 z), the sign of the turn about it, and the hinge's place in the block
/// (`applyLidAngle`: turned to its facing about the centre, then about x at the pivot).
pub fn lid_hinge(facing: Dir) -> (u8, f32, [f32; 3]) {
    let deg = 180.0 + horizontal_angle(facing);
    let turn = |v: [f64; 3]| rotate_y(v, f64::from(deg));
    let x = turn([1.0, 0.0, 0.0]);
    let p = turn([
        f64::from(PIVOT[0]) - 0.5,
        f64::from(PIVOT[1]) - 0.5,
        f64::from(PIVOT[2]) - 0.5,
    ]);
    let hinge = [p[0] as f32 + 0.5, p[1] as f32 + 0.5, p[2] as f32 + 0.5];
    // `rotateXDegrees(-angle)` about the turned x axis.
    if x[0].abs() > 0.5 {
        (0, -x[0].signum() as f32, hinge)
    } else {
        (2, -x[2].signum() as f32, hinge)
    }
}

/// `EjectorTargetHandler.getValidTargetDirection` and `flushSettings`: a target on a straight
/// line from the ejector (not straight above or below it), within `MAX_DISTANCE`; the facing it
/// turns to and the launcher's distances (`h`, `v`).
pub fn target_for(pos: [i32; 3], target: [i32; 3]) -> Option<(Dir, i32, i32)> {
    if pos[0] == target[0] && pos[2] == target[2] {
        return None;
    }
    let (dx, dz) = (target[0] - pos[0], target[2] - pos[2]);
    if dx.abs() > MAX_DISTANCE || dz.abs() > MAX_DISTANCE {
        return None;
    }
    let facing = if dx == 0 {
        if dz < 0 { Dir::North } else { Dir::South }
    } else if dz == 0 {
        if dx < 0 { Dir::West } else { Dir::East }
    } else {
        return None;
    };
    let (ddx, ddy, ddz) = (pos[0] - target[0], pos[1] - target[1], pos[2] - target[2]);
    Some((facing, (ddx + ddz).abs(), -ddy))
}

/// `Level.clip` with `ClipContext.Block.COLLIDER, Fluid.NONE`, as the ejector's obstacle scan
/// uses it: the first collider box the segment from `from` to `to` enters. `colliders` gives
/// the boxes of one cell in world units (empty for none); the nearest entry wins within a
/// cell (`VoxelShape.clip`), and cells are visited in the segment's order, so the first cell
/// with a hit is the hit. The cell the segment ends inside is visited too.
pub fn clip(
    from: [f64; 3],
    to: [f64; 3],
    mut colliders: impl FnMut([i32; 3]) -> Vec<[f64; 6]>,
) -> Option<([f64; 3], [i32; 3], Dir)> {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut cell = from.map(|v| v.floor() as i32);
    let end = to.map(|v| v.floor() as i32);
    let step = d.map(|v| if v > 0.0 { 1 } else { -1 });
    let mut t_max = [0usize, 1, 2].map(|a| {
        if d[a] == 0.0 {
            f64::INFINITY
        } else {
            let edge = f64::from(cell[a] + i32::from(step[a] > 0));
            (edge - from[a]) / d[a]
        }
    });
    let t_delta = d.map(|v| {
        if v == 0.0 {
            f64::INFINITY
        } else {
            1.0 / v.abs()
        }
    });
    for _ in 0..64 {
        // `VoxelShape.clip`: the nearest of the cell's boxes, not the first one stored.
        let mut nearest: Option<(f64, Dir)> = None;
        for b in colliders(cell) {
            if let Some((t, face)) = box_clip(from, d, &b)
                && nearest.is_none_or(|(best, _)| t < best)
            {
                nearest = Some((t, face));
            }
        }
        if let Some((t, face)) = nearest {
            let at = [0, 1, 2].map(|i| from[i] + d[i] * t);
            return Some((at, cell, face));
        }
        if cell == end {
            return None;
        }
        let a = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] {
            0
        } else if t_max[1] <= t_max[2] {
            1
        } else {
            2
        };
        let t = t_max[a];
        if t > 1.0 {
            return None;
        }
        cell[a] += step[a];
        t_max[a] += t_delta[a];
    }
    None
}

/// `AABB.clip(from, to)`: where the segment from `from` with offset `d` enters the box (world
/// units), and the face it enters through; None when it starts inside, misses the box, or
/// would only enter past its end. The entry is the last near plane crossed (the greatest per
/// axis), its axis's face the one hit; ties keep the earliest axis, as `getDirection`'s
/// insertion order (x, then y, then z) does. Axes within `1e-7` of zero never enter.
fn box_clip(from: [f64; 3], d: [f64; 3], b: &[f64; 6]) -> Option<(f64, Dir)> {
    const EPS: f64 = 1.0e-7;
    let mut entry = f64::NEG_INFINITY;
    let mut exit = f64::INFINITY;
    let mut face: Option<(usize, bool)> = None;
    for a in 0..3 {
        if d[a].abs() <= EPS {
            if from[a] < b[a] || from[a] > b[a + 3] {
                return None;
            }
            continue;
        }
        let (near, far, front) = if d[a] > 0.0 {
            ((b[a] - from[a]) / d[a], (b[a + 3] - from[a]) / d[a], false)
        } else {
            ((b[a + 3] - from[a]) / d[a], (b[a] - from[a]) / d[a], true)
        };
        if near > entry {
            entry = near;
            face = Some((a, front));
        }
        exit = exit.min(far);
    }
    // Inside already (every near plane behind), a miss, or an entry past the segment's end.
    if face.is_none() || entry < 0.0 || entry > exit || entry > 1.0 {
        return None;
    }
    let (a, front) = face.unwrap();
    Some((
        entry,
        match (a, front) {
            (0, false) => Dir::West,
            (0, true) => Dir::East,
            (1, false) => Dir::Down,
            (1, true) => Dir::Up,
            (_, false) => Dir::North,
            (_, true) => Dir::South,
        },
    ))
}

/// The mod-message channel that brings ejectors to clients.
pub const CHANNEL: &str = "create:ejector";

/// An ejector as clients draw it: its plate (state, lid, how fast it winds back), its launcher
/// and the stacks in flight (ticks flown, item), and where an obstacle stops them.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub pos: [i32; 3],
    pub gone: bool,
    pub state: State,
    pub lid: f32,
    pub wind_up: f32,
    pub h: i32,
    pub v: i32,
    pub early: Option<f32>,
    pub flying: Vec<(i32, String)>,
}

impl Shown {
    pub fn of(pos: [i32; 3], e: &Ejector, speed: f32) -> Shown {
        Shown {
            pos,
            gone: false,
            state: e.state,
            lid: e.lid.value,
            wind_up: e.wind_up(speed),
            h: e.launcher.h,
            v: e.launcher.v,
            early: e.early.map(|x| x.time),
            flying: e
                .flying
                .iter()
                .map(|f| (f.time, crate::arm::shown_name(&f.item).to_owned()))
                .collect(),
        }
    }

    /// When a stack flown `time` ticks stops being drawn (`EjectorRenderer`'s `maxTime`).
    pub fn max_time(&self) -> f32 {
        self.early
            .unwrap_or(Launcher::new(self.h, self.v).total_ticks() as f32)
    }
}

/// Ejectors as messages of at most 30,000 bytes.
pub fn encode(all: &[Shown]) -> Vec<Vec<u8>> {
    let mut messages: Vec<Vec<u8>> = Vec::new();
    for s in all {
        let mut one = Vec::new();
        for c in s.pos {
            one.extend_from_slice(&c.to_le_bytes());
        }
        one.push(u8::from(s.gone) | s.state.index() << 1);
        for v in [s.lid, s.wind_up, s.early.unwrap_or(f32::NAN)] {
            one.extend_from_slice(&v.to_le_bytes());
        }
        one.extend_from_slice(&(s.h as i16).to_le_bytes());
        one.extend_from_slice(&(s.v as i16).to_le_bytes());
        let n = s.flying.len().min(16);
        one.push(n as u8);
        for (t, item) in &s.flying[..n] {
            one.extend_from_slice(&(*t as i16).to_le_bytes());
            let item = crate::arm::shown_name(item);
            one.extend_from_slice(&(item.len() as u16).to_le_bytes());
            one.extend_from_slice(item.as_bytes());
        }
        // A stack's name runs to a kilobyte (`arm::shown_name`): whole ejectors to a message.
        match messages.last_mut() {
            Some(m) if m.len() + one.len() <= 30_000 => m.extend_from_slice(&one),
            _ => messages.push(one),
        }
    }
    messages
}

/// A message's ejectors; a malformed tail is dropped.
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
    fn i16_of(d: &mut &[u8]) -> Option<i32> {
        Some(i32::from(i16::from_le_bytes(take(d, 2)?.try_into().ok()?)))
    }
    let one = |d: &mut &[u8]| -> Option<Shown> {
        let p = take(d, 12)?;
        let int = |i: usize| i32::from_le_bytes(p[i..i + 4].try_into().unwrap());
        let pos = [int(0), int(4), int(8)];
        let flags = take(d, 1)?[0];
        let (lid, wind_up, early) = (f32_of(d)?, f32_of(d)?, f32_of(d)?);
        let (h, v) = (i16_of(d)?, i16_of(d)?);
        let n = take(d, 1)?[0];
        let mut flying = Vec::with_capacity(usize::from(n));
        for _ in 0..n {
            let t = i16_of(d)?;
            let len = usize::from(u16::from_le_bytes(take(d, 2)?.try_into().ok()?));
            flying.push((t, String::from_utf8_lossy(take(d, len)?).into_owned()));
        }
        Some(Shown {
            pos,
            gone: flags & 1 != 0,
            state: State::from_index(flags >> 1),
            lid,
            wind_up,
            h,
            v,
            early: (!early.is_nan()).then_some(early),
            flying,
        })
    };
    let mut out = Vec::new();
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

    /// A full-cube collider box for the cell `p`, in world units.
    fn cube_at(p: [i32; 3]) -> [f64; 6] {
        let [x, y, z] = p.map(f64::from);
        [x, y, z, x + 1.0, y + 1.0, z + 1.0]
    }

    #[test]
    fn messages_round_trip() {
        let s = Shown {
            pos: [3, -60, -9],
            gone: false,
            state: State::Launching,
            lid: 0.8,
            wind_up: 0.0625,
            h: 4,
            v: -2,
            early: Some(7.5),
            flying: vec![(3, "minecraft:dirt".into())],
        };
        let gone = Shown {
            gone: true,
            early: None,
            flying: Vec::new(),
            ..s.clone()
        };
        let m = encode(&[s.clone(), gone.clone()]);
        assert_eq!(decode(&m[0]), [s, gone]);
    }

    #[derive(Default)]
    struct Spot {
        held: Option<(String, u8, f32)>,
        target_full: bool,
        put: Vec<([i32; 3], String, u8)>,
        dropped: Vec<([f64; 3], String, u8)>,
        wall: Option<[i32; 3]>,
        takes: bool,
    }

    impl World for Spot {
        fn present(&mut self) -> Present {
            Present {
                count: self.held.as_ref().map_or(0, |h| u32::from(h.1)),
                held_position: self.held.as_ref().map(|h| h.2),
            }
        }
        fn target_refuses(&mut self, _: [i32; 3]) -> bool {
            self.target_full
        }
        fn take_all(&mut self) -> Vec<(String, u8)> {
            self.held.take().map(|h| (h.0, h.1)).into_iter().collect()
        }
        fn insert(&mut self, target: [i32; 3], item: &str, count: u8) -> Option<u8> {
            if !self.takes {
                return None;
            }
            self.put.push((target, item.to_owned(), count));
            Some(0)
        }
        fn drop_item(&mut self, at: [f64; 3], _: [f64; 3], item: &str, count: u8) {
            self.dropped.push((at, item.to_owned(), count));
        }
        fn clip(&mut self, from: [f64; 3], to: [f64; 3]) -> Option<([f64; 3], [i32; 3], Dir)> {
            let wall = self.wall?;
            clip(from, to, |p| {
                (p[0] == wall[0] && p[2] == wall[2] && (0..=wall[1]).contains(&p[1]))
                    .then(|| cube_at(p))
                    .into_iter()
                    .collect::<Vec<[f64; 6]>>()
            })
        }
        fn extracting_funnel(&mut self, _: [i32; 3]) -> bool {
            false
        }
    }

    #[test]
    fn launchers_reach_their_target() {
        // The arc ends on its target block: h ahead, v up, from the centre at half height.
        for (h, v) in [(1, 0), (4, 0), (8, 3), (3, -2), (16, 10)] {
            let l = Launcher::new(h, v);
            let end = l.global_pos(l.total_ticks(), Dir::North, [0, 0, 0]);
            // Thrown away from north: toward the south (+z).
            assert!(
                (end[2] - (0.5 + f64::from(h))).abs() < 1e-9,
                "{h} {v}: {end:?}"
            );
            assert!(
                (end[1] - (0.5 + f64::from(v) * 0.5 + f64::from(l.y(l.total_ticks())) * 0.5)).abs()
                    < 1e-9
            );
            assert!(end[0].abs() - 0.5 < 1e-6);
        }
        // Create's numbers for the default launcher (1, 0): a peak of 1.1875, about 9.6 ticks
        // in the air.
        let l = Launcher::default();
        assert!(
            (l.total_ticks() - 9.604).abs() < 0.01,
            "{}",
            l.total_ticks()
        );
        // Clamped to 32 either way.
        let mut far = Launcher::new(40, -50);
        far.clamp(MAX_DISTANCE);
        assert_eq!((far.h, far.v), (32, -32));
    }

    #[test]
    fn targets_lie_on_a_line_from_the_ejector() {
        assert_eq!(target_for([0, 0, 0], [0, 2, -5]), Some((Dir::North, 5, 2)));
        assert_eq!(target_for([0, 0, 0], [3, -1, 0]), Some((Dir::East, 3, -1)));
        assert_eq!(target_for([0, 0, 0], [0, 4, 0]), None);
        assert_eq!(target_for([0, 0, 0], [2, 0, 2]), None);
        assert_eq!(target_for([0, 0, 0], [40, 0, 0]), None);
    }

    #[test]
    fn a_charged_ejector_throws_and_winds_up_again() {
        let mut e = Ejector::default();
        e.set_target(4, 0);
        let pos = [0, 64, 0];
        let mut w = Spot {
            takes: true,
            ..Default::default()
        };
        // Placed: it winds up from open (lid 1) at 16 rpm: 16 / 256 / 4 a tick, 64 ticks, then
        // charged on the next.
        let speed = 16.0;
        assert_eq!(e.wind_up(speed), 1.0 / 64.0);
        let mut ticks = 0;
        while e.state != State::Charged {
            e.tick(pos, Dir::North, speed, &mut w);
            ticks += 1;
        }
        assert_eq!(ticks, 65);
        // A stack arrives but is still sliding: it waits.
        w.held = Some(("minecraft:iron_ingot".into(), 3, 0.3));
        e.tick(pos, Dir::North, speed, &mut w);
        assert!(!e.launch);
        // At the centre it triggers; next tick it launches and the plate swings up.
        w.held.as_mut().unwrap().2 = 0.5;
        e.tick(pos, Dir::North, speed, &mut w);
        assert!(e.launch);
        assert!(e.tick(pos, Dir::North, speed, &mut w));
        assert_eq!(e.state, State::Launching);
        assert_eq!(e.lid.value, 0.8);
        assert_eq!(e.flying.len(), 1);
        assert!(w.held.is_none());
        e.tick(pos, Dir::North, speed, &mut w);
        assert_eq!(e.state, State::Retracting);
        assert_eq!(e.lid.value, 1.0);
        // It lands after `total` ticks (int-compared) in the target 4 north of it.
        let total = e.launcher.total_ticks() as f32;
        let mut flown = 2;
        while !e.flying.is_empty() {
            e.tick(pos, Dir::North, speed, &mut w);
            flown += 1;
        }
        assert_eq!(flown, total.max(3.0) as i32 + 2);
        assert_eq!(w.put, [([0, 64, -4], "minecraft:iron_ingot".to_owned(), 3)]);
        // A target that takes nothing: the stack drops where the arc ends.
        let mut e = Ejector {
            state: State::Charged,
            lid: Lid {
                value: 0.0,
                previous: 0.0,
                target: 0.0,
                speed: 0.0,
                chasing: false,
            },
            ..Default::default()
        };
        let mut w = Spot {
            held: Some(("minecraft:dirt".into(), 1, 0.5)),
            ..Default::default()
        };
        for _ in 0..40 {
            e.tick(pos, Dir::East, speed, &mut w);
        }
        assert_eq!(w.dropped.len(), 1);
        let (at, _, n) = &w.dropped[0];
        assert_eq!(*n, 1);
        // One block east (default target), at the arc's end.
        assert!((at[0] - 1.5).abs() < 1e-6, "{at:?}");
        // Powered, it never throws.
        let mut e = Ejector {
            state: State::Charged,
            powered: true,
            ..Default::default()
        };
        let mut w = Spot {
            held: Some(("minecraft:dirt".into(), 1, 0.5)),
            ..Default::default()
        };
        e.tick(pos, Dir::East, speed, &mut w);
        assert!(!e.launch && w.held.is_some());
    }

    #[test]
    fn a_wall_in_the_arc_stops_the_tracked_stack() {
        let mut e = Ejector::default();
        e.set_target(6, 0);
        e.state = State::Charged;
        e.lid.value = 0.0;
        let pos = [0, 0, 0];
        let mut w = Spot {
            held: Some(("minecraft:stone".into(), 1, 0.5)),
            wall: Some([0, 10, -3]),
            ..Default::default()
        };
        for _ in 0..30 {
            e.tick(pos, Dir::North, 32.0, &mut w);
        }
        let early = e.early.expect("hit the wall");
        assert_eq!((early.block[0], early.block[2]), (0, -3));
        assert_eq!(w.dropped.len(), 1);
        // Dropped a quarter block out of the face it hit.
        assert!(w.dropped[0].0[2] > -2.0 + 0.2, "{:?}", w.dropped[0].0);
    }

    #[test]
    fn an_idle_ejector_winds_down_once_its_shaft_turns() {
        // e2-ejector-idle-wind-up: placed with the plate open (lid 1) it retracts at
        // `getWindUpSpeed` — 16 rpm over one block: 16/256 a tick. Java's tick 10 shows the
        // lid at 0.4375: nine ticks of winding once the motor turns.
        let mut e = Ejector::default();
        assert_eq!(e.state, State::Retracting);
        let pos = [0, 8, 0];
        let mut w = Spot::default();
        // Its shaft has not been set going yet: the plate holds where it is.
        e.tick(pos, Dir::East, 0.0, &mut w);
        assert_eq!(e.lid.value, 1.0);
        for _ in 0..9 {
            e.tick(pos, Dir::East, 16.0, &mut w);
        }
        assert_eq!(e.lid.value, 0.4375);
        // 16 ticks of winding bring it down (tick 17 sees 0 and charges it).
        for _ in 9..16 {
            e.tick(pos, Dir::East, 16.0, &mut w);
        }
        assert_eq!(e.lid.value, 0.0);
        e.tick(pos, Dir::East, 16.0, &mut w);
        assert_eq!(e.state, State::Charged);
    }

    #[test]
    fn a_throw_meets_the_depot_thirteen_pixels_up() {
        // e4-ejector-clip-whole-blocks: an ejector at [0, 8, 0] facing east throws at the
        // depot at [1, 8, 0] (`Level.clip` with `Block.COLLIDER`). The depot's collider is
        // `AllShapes.CASING_13PX`, so the arc meets its top at y 9.0625 and Java records
        // `earlyTargetTime` 9.304945. A whole-block scan instead meets y 9.0 at 8.516402.
        let mut world = DepotArc;
        let mut e = Ejector {
            state: State::Charged,
            ..Default::default()
        };
        let pos = [0, 8, 0];
        assert!(!e.scan(8, pos, Dir::East, &mut world));
        assert!(e.scan(9, pos, Dir::East, &mut world));
        let early = e.early.expect("the depot stopped the tracked stack");
        assert_eq!(early.block, [1, 8, 0]);
        assert!((early.at[1] - 9.0625).abs() < 1e-6, "{:?}", early.at);
        assert!((early.time - 9.304945).abs() < 1e-6, "{}", early.time);
    }

    /// The e4 scene's world: a depot ahead of the ejector, with the shapes Create gives it.
    struct DepotArc;

    impl World for DepotArc {
        fn present(&mut self) -> Present {
            Present {
                count: 0,
                held_position: None,
            }
        }
        fn target_refuses(&mut self, _: [i32; 3]) -> bool {
            false
        }
        fn take_all(&mut self) -> Vec<(String, u8)> {
            Vec::new()
        }
        fn insert(&mut self, _: [i32; 3], _: &str, count: u8) -> Option<u8> {
            Some(count)
        }
        fn drop_item(&mut self, _: [f64; 3], _: [f64; 3], _: &str, _: u8) {}
        fn clip(&mut self, from: [f64; 3], to: [f64; 3]) -> Option<([f64; 3], [i32; 3], Dir)> {
            let info = crate::parts::StateInfo::default();
            clip(from, to, |p| {
                (p == [1, 8, 0])
                    .then(|| crate::shapes::colliders(crate::Block::Depot, &info))
                    .flatten()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|mut b| {
                        for a in 0..3 {
                            b[a] += f64::from(p[a]);
                            b[a + 3] += f64::from(p[a]);
                        }
                        b
                    })
                    .collect::<Vec<[f64; 6]>>()
            })
        }
        fn extracting_funnel(&mut self, _: [i32; 3]) -> bool {
            false
        }
    }

    #[test]
    fn the_plate_turns_about_its_hinge() {
        // Facing north the plate is turned 0° (180 + 180): its hinge at the north edge, turning
        // the plate up about -x.
        let (axis, sign, hinge) = lid_hinge(Dir::North);
        assert_eq!(axis, 0);
        assert_eq!(sign, -1.0);
        assert!((hinge[2] - 0.75 / 16.0).abs() < 1e-6);
        // Facing east, about z, the hinge at the east edge.
        let (axis, _, hinge) = lid_hinge(Dir::East);
        assert_eq!(axis, 2);
        assert!(hinge[0] > 0.9);
    }

    #[test]
    fn rays_enter_the_first_collider_box_at_its_near_face() {
        // Boxes come per cell in world units; a full cube two cells east.
        let cube = |p: [i32; 3]| {
            (p == [2, 0, 0])
                .then(|| cube_at(p))
                .into_iter()
                .collect::<Vec<_>>()
        };
        let (at, block, face) = clip([0.5, 0.5, 0.5], [3.5, 0.5, 0.5], cube).unwrap();
        assert_eq!(block, [2, 0, 0]);
        assert_eq!(face, Dir::West);
        assert!((at[0] - 2.0).abs() < 1e-9);
        // e4-ejector-clip-whole-blocks: the depot ahead is `AllShapes.CASING_13PX`, 13 pixels
        // tall. A throw descending onto it from inside its column of cells meets the top at
        // 13/16, not the block's face: the ray from [1.05, 0.9, 0.5] to [1.25, 0.7, 0.5]
        // enters the box at y 13/16, a 0.4375 of the way along.
        let depot = |p: [i32; 3]| {
            (p == [1, 0, 0])
                .then(|| [1.0, 0.0, 0.0, 2.0, 13.0 / 16.0, 1.0])
                .into_iter()
                .collect::<Vec<[f64; 6]>>()
        };
        let (at, block, face) = clip([1.05, 0.9, 0.5], [1.25, 0.7, 0.5], depot).unwrap();
        assert_eq!(block, [1, 0, 0]);
        assert_eq!(face, Dir::Up);
        assert!((at[1] - 13.0 / 16.0).abs() < 1e-9);
        assert!((at[0] - (1.05 + 0.2 * 0.4375)).abs() < 1e-9);
        // One that crosses into the cell's side first enters by the side face: the slab it
        // crosses last is x, at the cell's west face, 0.5 along.
        let (at, _, face) = clip([0.9, 0.9, 0.5], [1.1, 0.7, 0.5], depot).unwrap();
        assert_eq!(face, Dir::West);
        assert!((at[0] - 1.0).abs() < 1e-9);
        assert!((at[1] - 0.8).abs() < 1e-9);
        // A ray that passes over the 13 pixels misses the depot entirely.
        assert!(clip([0.9, 0.95, 0.5], [1.1, 0.83, 0.5], depot).is_none());
        // Two boxes in one cell (a shape's boxes): the nearest entry wins, whatever order the
        // boxes come in (`VoxelShape.clip`).
        let two = clip([0.5, 0.5, 0.5], [1.5, 0.5, 0.5], |_| {
            vec![
                [1.2, 0.0, 0.0, 1.4, 1.0, 1.0],
                [1.0, 0.0, 0.0, 1.1, 1.0, 1.0],
            ]
        })
        .unwrap();
        assert!(
            (two.0[0] - 1.0).abs() < 1e-9 && two.2 == Dir::West,
            "{two:?}"
        );
        // One that starts inside a box does not hit it (vanilla `AABB.clip` skips it).
        let inside = clip([0.5, 0.5, 0.5], [3.5, 0.5, 0.5], |p| {
            (p == [0, 0, 0])
                .then(|| cube_at(p))
                .into_iter()
                .collect::<Vec<_>>()
        });
        assert!(inside.is_none());
        // The cell the segment ends inside is visited: a box before its end still stops it.
        let (at, _, _) = clip([1.6, 0.5, 0.5], [2.4, 0.5, 0.5], cube).unwrap();
        assert!((at[0] - 2.0).abs() < 1e-9);
    }
}
