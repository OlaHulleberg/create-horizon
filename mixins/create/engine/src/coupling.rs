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

#[cfg(test)]
mod tests {
    use super::*;

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
