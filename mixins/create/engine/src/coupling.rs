//! Create's minecart couplings (`MinecartController`'s couplings, `CouplingHandler`,
//! `CouplingPhysics`, `MinecartSim2020`, `CapabilityMinecartController.handleKilledMinecart`):
//! which carts are linked, by their UUIDs (kept in the save), the rules for linking two, and
//! the motion that keeps linked carts at their length.

use crate::mounted::{RailShape, normalize, project};
use std::collections::HashMap;

/// `kinetics.maxCartCouplingLength`.
pub const MAX_LENGTH: i32 = 32;

/// `MinecartController.CouplingData`: the cart that leads it (`main`), the one it pulls, its
/// length and whether a contraption made it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    pub main: u128,
    pub connected: u128,
    pub length: f32,
    pub contraption: bool,
}

impl Link {
    fn flip(&mut self) {
        std::mem::swap(&mut self.main, &mut self.connected);
    }
}

/// A cart's couplings: the one it leads (`couplings.get(true)`) and the one it is pulled by.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Controller {
    pub leading: Option<Link>,
    pub connected: Option<Link>,
}

impl Controller {
    pub fn get(&self, main: bool) -> Option<Link> {
        if main { self.leading } else { self.connected }
    }
    fn slot(&mut self, main: bool) -> &mut Option<Link> {
        if main {
            &mut self.leading
        } else {
            &mut self.connected
        }
    }
    pub fn is_fully_coupled(&self) -> bool {
        self.leading.is_some() && self.connected.is_some()
    }
    /// `getCoupledCart(asMain)`.
    pub fn coupled_cart(&self, main: bool) -> Option<u128> {
        self.get(main)
            .map(|l| if main { l.connected } else { l.main })
    }
    pub fn is_empty(&self) -> bool {
        self.leading.is_none() && self.connected.is_none()
    }
}

/// Why `tryToCoupleCarts` refused (`minecart_coupling.*`), or None for a silent refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    TooFar,
    TwoCouplingsMax,
    Unloaded,
    NoLoops,
    /// Already coupled to each other, or a contraption's carts closer than 2: no message.
    Silent,
}

impl Refused {
    /// The status line Create shows (`CouplingHandler.status`).
    pub fn message(self) -> Option<&'static str> {
        Some(match self {
            Refused::TooFar => "Minecarts are too far apart",
            Refused::TwoCouplingsMax => "Minecarts cannot have more than two couplings each",
            Refused::Unloaded => "Parts of your train seem to be in unloaded chunks",
            Refused::NoLoops => "Couplings cannot form a loop",
            Refused::Silent => return None,
        })
    }
}

/// Create's "removed" status.
pub const REMOVED: &str = "Removed all couplings from minecart";

/// Every cart's couplings, by UUID.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Couplings {
    pub carts: HashMap<u128, Controller>,
}

impl Couplings {
    pub fn is_empty(&self) -> bool {
        self.carts.is_empty()
    }

    pub fn get(&self, cart: u128) -> Controller {
        self.carts.get(&cart).copied().unwrap_or_default()
    }

    fn set(&mut self, cart: u128, c: Controller) {
        if c.is_empty() {
            self.carts.remove(&cart);
        } else {
            self.carts.insert(cart, c);
        }
    }

    /// `CouplingHandler.getNextInCouplingChain`: Ok(None) at the chain's end, Err for a cart
    /// that isn't loaded.
    fn next(
        &self,
        cart: u128,
        forward: bool,
        loaded: &impl Fn(u128) -> bool,
    ) -> Result<Option<u128>, ()> {
        match self.get(cart).coupled_cart(forward) {
            None => Ok(None),
            Some(n) if loaded(n) => Ok(Some(n)),
            Some(_) => Err(()),
        }
    }

    /// `CouplingHandler.tryToCoupleCarts` for carts `a` and `b` `distance` apart (`(int)`
    /// rounded down): by a player (`contraption` false: closer than 2 counts as 2) or by an
    /// assembler. `loaded` says whether a cart is loaded. On success `a` leads `b`, an existing
    /// chain turned around first where needed (`prepareForCoupling`).
    pub fn try_couple(
        &mut self,
        a: u128,
        b: u128,
        distance: f64,
        contraption: bool,
        loaded: impl Fn(u128) -> bool,
    ) -> Result<(), Refused> {
        let mut distance = distance as i32;
        if distance < 2 {
            if contraption {
                return Err(Refused::Silent);
            }
            distance = 2;
        }
        if distance > MAX_LENGTH {
            return Err(Refused::TooFar);
        }
        if !loaded(a) || !loaded(b) {
            return Err(Refused::Unloaded);
        }
        let (ca, cb) = (self.get(a), self.get(b));
        if ca.is_fully_coupled() || cb.is_fully_coupled() {
            return Err(Refused::TwoCouplingsMax);
        }
        if ca.coupled_cart(true) == Some(b) || cb.coupled_cart(true) == Some(a) {
            return Err(Refused::Silent);
        }
        for start in [a, b] {
            let forward = self.get(start).leading.is_some();
            let mut current = start;
            for _ in 0..1000 {
                match self.next(current, forward, &loaded) {
                    Err(()) => return Err(Refused::Unloaded),
                    Ok(None) => break,
                    Ok(Some(n)) if n == b => return Err(Refused::NoLoops),
                    Ok(Some(n)) => current = n,
                }
            }
        }
        self.prepare(a, true, &loaded);
        self.prepare(b, false, &loaded);
        let link = Link {
            main: a,
            connected: b,
            length: distance as f32,
            contraption,
        };
        let mut ca = self.get(a);
        ca.leading = Some(link);
        self.set(a, ca);
        let mut cb = self.get(b);
        cb.connected = Some(link);
        self.set(b, cb);
        Ok(())
    }

    /// `prepareForCoupling`: a cart that already has a coupling on the side the new one takes
    /// turns its whole chain around.
    fn prepare(&mut self, cart: u128, leading: bool, loaded: &impl Fn(u128) -> bool) {
        let c = self.get(cart);
        if !(leading && c.leading.is_some() || !leading && c.connected.is_some()) {
            return;
        }
        let forward = c.leading.is_some();
        let mut flip = vec![cart];
        let mut current = cart;
        for _ in 0..1000 {
            match self.next(current, forward, loaded) {
                Ok(Some(n)) => {
                    flip.push(n);
                    current = n;
                }
                _ => break,
            }
        }
        for id in flip {
            let mut c = self.get(id);
            for l in [&mut c.leading, &mut c.connected].into_iter().flatten() {
                l.flip();
            }
            std::mem::swap(&mut c.leading, &mut c.connected);
            self.set(id, c);
        }
    }

    /// `MinecartController.removeConnection(main)` on `cart`.
    pub fn remove_connection(&mut self, cart: u128, main: bool) {
        let mut c = self.get(cart);
        *c.slot(main) = None;
        self.set(cart, c);
    }

    /// `MinecartController.decouple` (a wrench on a cart): both of its couplings go, from both
    /// ends. The coupling items a player gets back (`onWrenchInteractOnMinecart`: those not
    /// made by a contraption), or None when the cart had none.
    pub fn decouple(&mut self, cart: u128) -> Option<u32> {
        let c = self.get(cart);
        let links = u32::from(c.leading.is_some()) + u32::from(c.connected.is_some());
        if links == 0 {
            return None;
        }
        let mut items = links;
        for main in [true, false] {
            let Some(l) = c.get(main) else { continue };
            if l.contraption {
                items -= 1;
            }
            let other = if main { l.connected } else { l.main };
            self.remove_connection(cart, main);
            self.remove_connection(other, !main);
        }
        Some(items)
    }

    /// `handleKilledMinecart`: a cart that died lets go of its neighbours, each of which
    /// drops a coupling item halfway between them unless a contraption made it. The
    /// neighbours that drop one.
    pub fn killed(&mut self, cart: u128) -> Vec<u128> {
        let c = self.get(cart);
        let mut drops = Vec::new();
        for forward in [true, false] {
            let Some(l) = c.get(forward) else { continue };
            let next = if forward { l.connected } else { l.main };
            self.remove_connection(next, !forward);
            if !l.contraption {
                drops.push(next);
            }
        }
        self.carts.remove(&cart);
        drops
    }

    /// `forEachLoadedCoupling`: each coupling once, from the cart leading it: (main,
    /// connected, length).
    pub fn pairs(&self) -> Vec<(u128, u128, f32)> {
        let mut out: Vec<(u128, u128, f32)> = self
            .carts
            .iter()
            .filter_map(|(id, c)| c.leading.map(|l| (*id, l.connected, l.length)))
            .collect();
        out.sort_by_key(|p| (p.0, p.1));
        out
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

/// `VecHelper.clamp`.
pub fn clamp(v: [f64; 3], max: f64) -> [f64; 3] {
    if dot(v, v) > max * max {
        normalize(v).map(|c| c * max)
    } else {
        v
    }
}

/// `VecHelper.intersectSphere`.
fn intersect_sphere(
    origin: [f64; 3],
    dir: [f64; 3],
    centre: [f64; 3],
    radius: f64,
) -> Option<[f64; 3]> {
    if dir == [0.0; 3] {
        return None;
    }
    let dir = if dot(dir, dir) != 1.0 {
        normalize(dir)
    } else {
        dir
    };
    let diff = sub(origin, centre);
    let along = dot(dir, diff);
    let delta = along * along - (dot(diff, diff) - radius * radius);
    if delta < 0.0 {
        return None;
    }
    let t = -along + delta.sqrt();
    Some(add(origin, dir.map(|c| c * t)))
}

/// `CouplingPhysics.followLinkOnRail`: where the cart at `cart` goes along its rail `axis` to
/// shorten its link (`link`, to the other cart) by `reduce`.
pub fn follow_link_on_rail(
    link: [f64; 3],
    cart: [f64; 3],
    reduce: f64,
    axis: [f64; 3],
) -> [f64; 3] {
    let d = dot(axis, link);
    if d.is_nan() || d == 0.0 || reduce == 0.0 {
        return cart;
    }
    let axis = axis.map(|c| c * -d.signum());
    let centre = add(cart, link);
    let radius = len(link) - reduce;
    match intersect_sphere(cart, axis, centre, radius) {
        Some(p) => p,
        None => add(cart, project(link, axis)),
    }
}

/// One coupled cart as the physics reads it: where it is, its motion, whether motion may be
/// added (`MinecartSim2020.canAddMotion`), its top speed and the rail its next position is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub pos: [f64; 3],
    pub motion: [f64; 3],
    pub can_add_motion: bool,
    pub max_speed: f64,
    pub rail: Option<RailShape>,
}

/// `MinecartSim2020.predictNextPositionOf`.
pub fn predict(pos: [f64; 3], motion: [f64; 3]) -> [f64; 3] {
    add(pos, clamp(motion, 1.0))
}

/// `CouplingPhysics.softCollisionStep`: each cart's motion (clamped to a block a tick), changed
/// so that their next positions are `length` apart, each doing half (all of it when only one
/// may move), along its rail. `bodies[k].rail` is the rail under its next position. None when
/// nothing changes (the next positions are already `length` apart).
pub fn soft_step(bodies: [Body; 2], length: f64) -> Option<[[f64; 3]; 2]> {
    let mut motions = bodies.map(|b| clamp(b.motion, 1.0));
    let next = [0, 1].map(|k| add(bodies[k].pos, motions[k]));
    let stress = (length - len(sub(next[0], next[1]))) as f32;
    if (stress as f64).abs() < 1.0e-5 {
        return None;
    }
    for k in 0..2 {
        let other = 1 - k;
        let link = sub(next[other], next[k]);
        let mut magnitude = -f64::from(stress) / 2.0;
        if bodies[k].can_add_motion != bodies[other].can_add_motion {
            magnitude = if bodies[k].can_add_motion {
                magnitude * 2.0
            } else {
                0.0
            };
        }
        if !bodies[k].can_add_motion {
            continue;
        }
        let correction = match bodies[k].rail {
            Some(shape) => sub(
                follow_link_on_rail(link, next[k], magnitude, shape.vec()),
                next[k],
            ),
            None => normalize(link).map(|c| c * magnitude),
        };
        let correction = clamp(correction, bodies[k].max_speed);
        motions[k] = add(motions[k], correction);
    }
    Some([0, 1].map(|k| clamp(motions[k], bodies[k].max_speed)))
}

/// `AbstractMinecart.getCurrentRailPosition().above()`: the rail block a cart at `pos` runs on
/// (the one below its own when that is a rail, else its own), and its shape; None off a rail.
pub fn rail_of(
    pos: [f64; 3],
    rail: &impl Fn([i32; 3]) -> Option<RailShape>,
) -> Option<([i32; 3], RailShape)> {
    let p = pos.map(|v| v.floor() as i32);
    let below = [p[0], p[1] - 1, p[2]];
    match rail(below) {
        Some(shape) => Some((below, shape)),
        None => rail(p).map(|shape| (p, shape)),
    }
}

/// `OldMinecartBehavior.getPos`: the point on the rail under `pos`, None off a rail.
pub fn rail_pos(pos: [f64; 3], rail: &impl Fn([i32; 3]) -> Option<RailShape>) -> Option<[f64; 3]> {
    let (b, shape) = rail_of(pos, rail)?;
    let (e0, e1) = shape.exits();
    let f = b.map(f64::from);
    let end = |e: [i32; 3]| {
        [
            f[0] + 0.5 + f64::from(e[0]) * 0.5,
            f[1] + 0.0625 + f64::from(e[1]) * 0.5,
            f[2] + 0.5 + f64::from(e[2]) * 0.5,
        ]
    };
    let (a, c) = (end(e0), end(e1));
    let (xd, yd, zd) = (c[0] - a[0], (c[1] - a[1]) * 2.0, c[2] - a[2]);
    let t = if xd == 0.0 {
        pos[2] - f[2]
    } else if zd == 0.0 {
        pos[0] - f[0]
    } else {
        ((pos[0] - a[0]) * xd + (pos[2] - a[2]) * zd) * 2.0
    };
    let mut out = [a[0] + xd * t, a[1] + yd * t, a[2] + zd * t];
    if yd < 0.0 {
        out[1] += 1.0;
    } else if yd > 0.0 {
        out[1] += 0.5;
    }
    Some(out)
}

/// `OldMinecartBehavior.getPosOffs`: the point on the rail `offs` along the rail's way from
/// `pos`, None off a rail.
pub fn rail_pos_offs(
    pos: [f64; 3],
    offs: f64,
    rail: &impl Fn([i32; 3]) -> Option<RailShape>,
) -> Option<[f64; 3]> {
    let (b, shape) = rail_of(pos, rail)?;
    let mut y = f64::from(b[1]) + if shape.ascending() { 1.0 } else { 0.0 };
    let (e0, e1) = shape.exits();
    let (xd, zd) = (f64::from(e1[0] - e0[0]), f64::from(e1[2] - e0[2]));
    let d = xd.hypot(zd);
    let (x, z) = (pos[0] + xd / d * offs, pos[2] + zd / d * offs);
    let at = |e: [i32; 3]| x.floor() as i32 - b[0] == e[0] && z.floor() as i32 - b[2] == e[2];
    if e0[1] != 0 && at(e0) {
        y += f64::from(e0[1]);
    } else if e1[1] != 0 && at(e1) {
        y += f64::from(e1[1]);
    }
    rail_pos([x, y, z], rail)
}

/// `AbstractMinecart.getSlopeAdjustment`.
const SLOPE: f64 = 0.0078125;

/// `MinecartSim2020.moveCartAlongTrack`: where a cart at `pos` on the rail `rail_block` of
/// `shape` ends up moved by `forced` along it (`moveMinecartOnRail`: by `scale` of it, 0.75 for
/// a cart with a passenger, each part held to `max_speed`), back on its rail (`getPos`). Its
/// motion is left as it was. Blocks in the way are not looked at (a cart on its rail has none).
pub fn move_along_track(
    pos: [f64; 3],
    forced: [f64; 3],
    rail_block: [i32; 3],
    shape: RailShape,
    scale: f64,
    max_speed: f64,
    rail: &impl Fn([i32; 3]) -> Option<RailShape>,
) -> [f64; 3] {
    if forced == [0.0; 3] {
        return pos;
    }
    let mut forced = forced;
    let before = rail_pos(pos, rail);
    let mut y = f64::from(rail_block[1]);
    match shape {
        RailShape::AscendingEast => forced[0] -= SLOPE,
        RailShape::AscendingWest => forced[0] += SLOPE,
        RailShape::AscendingNorth => forced[2] += SLOPE,
        RailShape::AscendingSouth => forced[2] -= SLOPE,
        _ => {}
    }
    if shape.ascending() {
        y += 1.0;
    }
    let (e0, e1) = shape.exits();
    let (fx, fz) = (f64::from(rail_block[0]), f64::from(rail_block[2]));
    let x0 = fx + 0.5 + f64::from(e0[0]) * 0.5;
    let z0 = fz + 0.5 + f64::from(e0[2]) * 0.5;
    let (xd, zd) = (
        f64::from(e1[0] - e0[0]) * 0.5,
        f64::from(e1[2] - e0[2]) * 0.5,
    );
    let t = if xd == 0.0 {
        pos[2] - fz
    } else if zd == 0.0 {
        pos[0] - fx
    } else {
        ((pos[0] - x0) * xd + (pos[2] - z0) * zd) * 2.0
    };
    let mut p = [
        x0 + xd * t + (scale * forced[0]).clamp(-max_speed, max_speed),
        y,
        z0 + zd * t + (scale * forced[2]).clamp(-max_speed, max_speed),
    ];
    let at = |e: [i32; 3], p: [f64; 3]| {
        p[0].floor() as i32 - rail_block[0] == e[0] && p[2].floor() as i32 - rail_block[2] == e[2]
    };
    if e0[1] != 0 && at(e0, p) {
        p[1] += f64::from(e0[1]);
    } else if e1[1] != 0 && at(e1, p) {
        p[1] += f64::from(e1[1]);
    }
    if let (Some(after), Some(_)) = (rail_pos(p, rail), before) {
        p[1] = after[1];
    }
    p
}

/// One coupled cart as the hard step reads it: where it is, whether motion may be added, its
/// top speed and whether it carries something (`isVehicle`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hard {
    pub pos: [f64; 3],
    pub can_add_motion: bool,
    pub max_speed: f64,
    pub vehicle: bool,
}

/// What the hard step did to a cart: its new position, and whether it was off a rail (moved
/// straight, its motion then kept at 0.95).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moved {
    pub pos: [f64; 3],
    pub off_rail: bool,
}

/// `CouplingPhysics.hardCollisionStep`: carts whose distance is off their `length` by an
/// eighth or more are moved back toward it along their rails, the first half the way, then
/// the other all of it, then the first again (a cart that can't add motion half as far), each
/// move at most `min(1.75, max_speed)`. Each cart's new place, or None where it stayed.
pub fn hard_step(
    bodies: [Hard; 2],
    length: f64,
    rail: &impl Fn([i32; 3]) -> Option<RailShape>,
) -> [Option<Moved>; 2] {
    // The second can't move and the first can: they swap.
    let order = if !bodies[1].can_add_motion && bodies[0].can_add_motion {
        [1, 0]
    } else {
        [0, 1]
    };
    let mut pos = bodies.map(|b| b.pos);
    let mut out = [None, None];
    let mut first = true;
    for k in [0, 1, 0] {
        let (i, j) = (order[k], order[1 - k]);
        let b = bodies[i];
        let stress = (length - len(sub(pos[j], pos[i]))) as f32;
        if stress.abs() < 1.0 / 8.0 {
            continue;
        }
        let link = sub(pos[j], pos[i]);
        let mut magnitude = if first { -stress / 2.0 } else { -stress };
        if !b.can_add_motion {
            magnitude /= 2.0;
        }
        let on = rail_of(pos[i], rail);
        let correction = match on {
            Some((_, shape)) => sub(
                follow_link_on_rail(link, pos[i], f64::from(magnitude), shape.vec()),
                pos[i],
            ),
            None => normalize(link).map(|c| c * f64::from(magnitude)),
        };
        let correction = clamp(correction, b.max_speed.min(1.75));
        let moved = match on {
            Some((block, shape)) => {
                let scale = if b.vehicle { 0.75 } else { 1.0 };
                move_along_track(pos[i], correction, block, shape, scale, b.max_speed, rail)
            }
            None => add(pos[i], correction),
        };
        pos[i] = moved;
        out[i] = Some(Moved {
            pos: moved,
            off_rail: on.is_none(),
        });
        first = false;
    }
    out
}

/// `OrientedContraptionEntity.updateOrientation` on a coupling: the yaw and pitch (degrees) of
/// a contraption spanning `main` (the coupling's leading cart) and `connected`, turned a half
/// turn when it rides the leading cart (`getCouplingId().equals(riding.getUUID())`).
pub fn coupled_orientation(main: [f64; 3], connected: [f64; 3], rides_main: bool) -> (f32, f32) {
    let d = sub(main, connected);
    let mut yaw = d[2].atan2(d[0]).to_degrees() as f32;
    let mut pitch = d[1].atan2(d[0].hypot(d[2])).to_degrees() as f32;
    if rides_main {
        pitch = -pitch;
        yaw += 180.0;
    }
    (yaw, pitch)
}

/// `Direction.fromYRot`: south 0, west 90, north 180, east 270 (as `y_rot`), the nearest.
pub fn dir_from_y_rot(yaw: f64) -> crate::kinetics::Dir {
    use crate::kinetics::Dir;
    match ((yaw / 90.0 + 0.5).floor() as i64).rem_euclid(4) {
        0 => Dir::South,
        1 => Dir::West,
        2 => Dir::North,
        _ => Dir::East,
    }
}

/// `CouplingRenderer.getSuitableCartEndpoint` (its 0.004-block jitter left out): where a
/// coupling meets the cart at `pos` (the rail under it, 3/8 + 1/8 up, 13/16 toward the
/// coupling's `centre`) and the cart's yaw there (`180 - yaw`, degrees, plus a half turn when
/// its back faces the centre: the attachment's turn).
pub fn endpoint(
    pos: [f64; 3],
    centre: [f64; 3],
    rail: &impl Fn([i32; 3]) -> Option<RailShape>,
) -> ([f64; 3], f32) {
    let on = rail_pos(pos, rail);
    let (mut front, mut back) = (pos, pos);
    let mut yaw = 0.0f32;
    let mut at = [pos[0], pos[1] + 0.375, pos[2]];
    if let Some(r) = on {
        front = rail_pos_offs(pos, 0.3, rail).unwrap_or(r);
        back = rail_pos_offs(pos, -0.3, rail).unwrap_or(r);
        at = [r[0], (front[1] + back[1]) / 2.0 + 0.375, r[2]];
        let d = normalize(sub(back, front));
        if d != [0.0; 3] {
            yaw = d[2].atan2(d[0]).to_degrees() as f32;
        }
    }
    let toward = if on.is_some() {
        normalize(sub(front, back))
    } else {
        [0.0; 3]
    };
    let dist = |p: [f64; 3]| len(sub(p, centre));
    let back_closer = dist(front) > dist(back);
    let offset = if back_closer {
        -13.0 / 16.0
    } else {
        13.0 / 16.0
    };
    let p = [
        at[0] + toward[0] * offset,
        at[1] + 2.0 / 16.0 + toward[1] * offset,
        at[2] + toward[2] * offset,
    ];
    let turn = 180.0 - yaw + if back_closer { 180.0 } else { 0.0 };
    (p, turn)
}

/// `CouplingRenderer.renderCoupling`'s connector between two endpoints: its yaw (about up,
/// degrees), pitch (about its own z) and length.
pub fn connector(a: [f64; 3], b: [f64; 3]) -> (f32, f32, f64) {
    let d = sub(b, a);
    let yaw = -(d[2].atan2(d[0]).to_degrees()) as f32;
    let pitch = d[1].atan2(d[0].hypot(d[2])).to_degrees() as f32;
    (yaw, pitch, len(d))
}

/// The mod message channel: couplings as clients draw them, server to client.
pub const CHANNEL: &str = "create:coupling";

/// A player's coupling as clients draw it (`CouplingRenderer`): keyed by its leading cart's
/// entity id, each end (`endpoint`) gliding at its cart's velocity (blocks a second) with its
/// attachment's turn, and the connector from the first end, turned `yaw` about up and `pitch`
/// about its own z, `length` long. Sent again when the glide drifts, as contraptions are.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawn {
    pub key: u32,
    pub ends: [[f64; 3]; 2],
    pub velocity: [[f32; 3]; 2],
    pub turns: [f32; 2],
    pub yaw: f32,
    pub pitch: f32,
    pub length: f32,
}

const DRAWN_BYTES: usize = 4 + 6 * 8 + 11 * 4;

impl Drawn {
    /// The key, the ends, then the velocities, turns, yaw, pitch and length. A message of the
    /// key alone clears it.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(DRAWN_BYTES);
        out.extend_from_slice(&self.key.to_le_bytes());
        for v in self.ends.iter().flatten() {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let singles = self
            .velocity
            .iter()
            .flatten()
            .copied()
            .chain(self.turns)
            .chain([self.yaw, self.pitch, self.length]);
        for v in singles {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// The coupling in a message, or the key of one to clear.
    pub fn decode(d: &[u8]) -> Result<Drawn, Option<u32>> {
        let key = d
            .get(0..4)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or(None)?;
        if d.len() < DRAWN_BYTES {
            return Err(Some(key));
        }
        let double = |k: usize| f64::from_le_bytes(d[k..k + 8].try_into().unwrap_or([0; 8]));
        let single = |k: usize| f32::from_le_bytes(d[k..k + 4].try_into().unwrap_or([0; 4]));
        let s = |i: usize| single(52 + 4 * i);
        Ok(Drawn {
            key,
            ends: [0, 1].map(|e| [0, 1, 2].map(|a| double(4 + 8 * (3 * e + a)))),
            velocity: [0, 1].map(|e| [0, 1, 2].map(|a| s(3 * e + a))),
            turns: [s(6), s(7)],
            yaw: s(8),
            pitch: s(9),
            length: s(10),
        })
    }

    /// Whether clients, gliding on from `self` for `ticks`, are off `now`: an end a 32nd of a
    /// block away, a turn half a degree, the length a 32nd; and every two seconds (a player
    /// who came near since hears of it).
    pub fn due(&self, now: &Drawn, ticks: i64) -> bool {
        let t = ticks as f64 / 20.0;
        let off = (0..2).any(|e| {
            let d = (0..3)
                .map(|a| {
                    let p = self.ends[e][a] + f64::from(self.velocity[e][a]) * t;
                    (p - now.ends[e][a]).powi(2)
                })
                .sum::<f64>();
            d.sqrt() > crate::mounted::DRIFT
        });
        let turned = |a: f32, b: f32| {
            crate::mounted::shortest_angle_diff(f64::from(a), f64::from(b)).abs()
                > crate::mounted::TURN_DRIFT
        };
        ticks >= 40
            || off
            || (0..2).any(|e| turned(self.turns[e], now.turns[e]))
            || turned(self.yaw, now.yaw)
            || turned(self.pitch, now.pitch)
            || (self.length - now.length).abs() > 1.0 / 32.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hard_step_pulls_carts_back_to_their_length() {
        let flat = |p: [i32; 3]| (p[1] == 64).then_some(RailShape::EastWest);
        let body = |x: f64| Hard {
            pos: [x, 64.0625, 0.5],
            can_add_motion: true,
            max_speed: 0.4,
            vehicle: false,
        };
        // 3 long, 3.5 apart: the first moves a quarter toward the second, the second the other
        // quarter, and then they are at their length.
        let [a, b] = hard_step([body(0.5), body(4.0)], 3.0, &flat);
        let (a, b) = (a.unwrap().pos, b.unwrap().pos);
        assert!((a[0] - 0.75).abs() < 1e-6, "{a:?}");
        assert!(
            (b[0] - 3.75).abs() < 1e-6 && (b[1] - 64.0625).abs() < 1e-9,
            "{b:?}"
        );
        // Within an eighth: nothing moves.
        assert_eq!(hard_step([body(0.5), body(3.6)], 3.0, &flat), [None, None]);
        // Off a rail: straight along the link.
        let none = |_: [i32; 3]| None;
        let [a, _] = hard_step([body(0.5), body(4.0)], 3.0, &none);
        assert!(a.unwrap().off_rail);
        // A cart with a passenger moves three quarters as far along its rail.
        let p = move_along_track(
            [0.5, 64.0625, 0.5],
            [0.4, 0.0, 0.0],
            [0, 64, 0],
            RailShape::EastWest,
            0.75,
            0.4,
            &flat,
        );
        assert!(
            (p[0] - 0.8).abs() < 1e-9 && (p[1] - 64.0625).abs() < 1e-9,
            "{p:?}"
        );
        assert_eq!(rail_pos([0.3, 64.5, 0.9], &flat), Some([0.3, 64.0625, 0.5]));
    }

    #[test]
    fn coupled_contraptions_face_along_their_carts() {
        // The second cart east of the first: assembled facing south (`fromYRot(0)`), and its
        // yaw a full turn from it, so it is drawn as assembled.
        assert_eq!(dir_from_y_rot(0.0), crate::kinetics::Dir::South);
        assert_eq!(dir_from_y_rot(-90.0), crate::kinetics::Dir::East);
        let (yaw, pitch) = coupled_orientation([0.0, 0.0, 0.0], [3.0, 0.0, 0.0], true);
        assert_eq!((yaw, pitch), (360.0, 0.0));
        // Round a corner: the second cart south-east of the first.
        let (yaw, _) = coupled_orientation([0.0, 0.0, 0.0], [2.0, 0.0, 2.0], true);
        assert!((yaw - 45.0).abs() < 1e-4, "{yaw}");
        // The coupling's ends 13/16 in front of each cart, toward each other.
        let flat = |p: [i32; 3]| (p[1] == 64).then_some(RailShape::EastWest);
        let (a, _) = endpoint([0.5, 64.0625, 0.5], [2.0, 64.0, 0.5], &flat);
        let (b, _) = endpoint([3.5, 64.0625, 0.5], [2.0, 64.0, 0.5], &flat);
        assert!(
            (a[0] - 1.3125).abs() < 1e-9 && (b[0] - 2.6875).abs() < 1e-9,
            "{a:?} {b:?}"
        );
        let (yaw, pitch, length) = connector(a, b);
        assert!(yaw.abs() < 1e-6 && pitch == 0.0 && (length - 1.375).abs() < 1e-9);
        let drawn = Drawn {
            key: 7,
            ends: [a, b],
            velocity: [[2.0, 0.0, 0.0]; 2],
            turns: [0.0, 180.0],
            yaw,
            pitch,
            length: length as f32,
        };
        assert_eq!(Drawn::decode(&drawn.encode()), Ok(drawn.clone()));
        assert_eq!(Drawn::decode(&7u32.to_le_bytes()), Err(Some(7)));
        // Gliding at 2 blocks a second: on time after 5 ticks, off when the carts stopped.
        let mut later = drawn.clone();
        later.ends = [a, b].map(|e| [e[0] + 0.5, e[1], e[2]]);
        assert!(!drawn.due(&later, 5));
        assert!(drawn.due(&drawn, 5) && drawn.due(&later, 40));
    }

    #[test]
    fn players_couple_carts_by_create_s_rules() {
        let mut c = Couplings::default();
        let loaded = |_| true;
        assert_eq!(
            c.try_couple(1, 2, 40.0, false, loaded),
            Err(Refused::TooFar)
        );
        // Closer than 2: a player's coupling is 2 long, an assembler's refused.
        assert_eq!(c.try_couple(1, 2, 1.0, true, loaded), Err(Refused::Silent));
        assert_eq!(c.try_couple(1, 2, 1.5, false, loaded), Ok(()));
        assert_eq!(c.get(1).leading.map(|l| l.length), Some(2.0));
        assert_eq!(c.get(2).coupled_cart(false), Some(1));
        // Twice: silently refused.
        assert_eq!(c.try_couple(1, 2, 3.0, false, loaded), Err(Refused::Silent));
        // 2 → 3, then 3 → 1 would close a loop.
        assert_eq!(c.try_couple(2, 3, 3.0, false, loaded), Ok(()));
        assert_eq!(
            c.try_couple(3, 1, 3.0, false, loaded),
            Err(Refused::NoLoops)
        );
        // A cart in an unloaded chunk.
        assert_eq!(
            c.try_couple(4, 5, 3.0, false, |id| id != 5),
            Err(Refused::Unloaded)
        );
        // 2 is fully coupled.
        assert_eq!(
            c.try_couple(2, 6, 3.0, false, loaded),
            Err(Refused::TwoCouplingsMax)
        );
        assert_eq!(c.pairs(), vec![(1, 2, 2.0), (2, 3, 3.0)]);
    }

    #[test]
    fn a_chain_turns_around_to_take_a_new_leader() {
        let mut c = Couplings::default();
        let loaded = |_| true;
        c.try_couple(1, 2, 3.0, false, loaded).unwrap();
        // 1 already leads: coupling 1 → 9 turns 1 → 2 into 2 → 1 first.
        c.try_couple(1, 9, 4.0, false, loaded).unwrap();
        assert_eq!(c.pairs(), vec![(1, 9, 4.0), (2, 1, 3.0)]);
        assert!(c.get(1).is_fully_coupled());
    }

    #[test]
    fn wrenches_and_deaths_break_couplings() {
        let mut c = Couplings::default();
        let loaded = |_| true;
        c.try_couple(1, 2, 3.0, false, loaded).unwrap();
        c.try_couple(2, 3, 3.0, true, loaded).unwrap();
        // The middle cart wrenched: both go, one item back (the other was a contraption's).
        assert_eq!(c.decouple(2), Some(1));
        assert!(c.is_empty());
        assert_eq!(c.decouple(2), None);
        c.try_couple(1, 2, 3.0, false, loaded).unwrap();
        c.try_couple(2, 3, 3.0, false, loaded).unwrap();
        assert_eq!(c.killed(2), vec![3, 1]);
        assert!(c.is_empty());
    }

    #[test]
    fn the_soft_step_keeps_carts_at_their_length() {
        let body = |x: f64, vx: f64| Body {
            pos: [x, 64.0, 0.5],
            motion: [vx, 0.0, 0.0],
            can_add_motion: true,
            max_speed: 0.4,
            rail: Some(RailShape::EastWest),
        };
        // Leader runs east at 0.3, follower still 3 blocks behind on a 3 long link: the
        // leader is held back and the follower pulled, each half.
        let [a, b] = soft_step([body(3.5, 0.3), body(0.5, 0.0)], 3.0).unwrap();
        assert!(
            (a[0] - 0.15).abs() < 1e-6 && (b[0] - 0.15).abs() < 1e-6,
            "{a:?} {b:?}"
        );
        let next = (3.5 + a[0]) - (0.5 + b[0]);
        assert!((next - 3.0).abs() < 1e-6);
        // At their length already: nothing.
        assert_eq!(soft_step([body(3.5, 0.2), body(0.5, 0.2)], 3.0), None);
        // A cart that can't move: the other does it all.
        let mut stuck = body(0.5, 0.0);
        stuck.can_add_motion = false;
        let [a, b] = soft_step([body(3.5, 0.3), stuck], 3.0).unwrap();
        assert!((a[0] - 0.0).abs() < 1e-6 && b == [0.0; 3], "{a:?} {b:?}");
        assert_eq!(
            follow_link_on_rail([3.0, 0.0, 0.0], [0.0; 3], 1.0, [1.0, 0.0, 0.0]),
            [1.0, 0.0, 0.0]
        );
    }
}
