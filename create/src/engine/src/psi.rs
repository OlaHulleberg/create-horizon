//! Create's portable storage interfaces (`PortableStorageInterfaceBlockEntity`,
//! `PortableItemInterfaceBlockEntity`, `PortableFluidInterfaceBlockEntity`,
//! `PortableStorageInterfaceMovement`): an interface riding a contraption stops the contraption
//! at a stationary interface facing it; while they are connected the stationary one is the
//! contraption's storage to whatever reaches it (funnels, chutes...), each transfer holding the
//! connection for `TIMEOUT` ticks more; once none came for that long the contraption goes on.
//! A stationary interface ticks only while connected, connecting or letting go.

use crate::kinetics::Dir;

/// `PortableStorageInterfaceBlockEntity.ANIMATION`.
pub const ANIMATION: i32 = 4;
/// `getTransferTimeout`: `CLogistics.psiTimeout`'s default.
pub const TIMEOUT: i32 = 60;
/// `getActiveAreaOffset`: how far ahead of its block's centre a moving interface looks.
pub const REACH: f64 = 1.85;

/// A stationary interface (`transferTimer`, `distance`, `powered`, `connectedEntity`,
/// `keepAlive`); the contraption by its key.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stationary {
    pub timer: i32,
    pub distance: f32,
    pub powered: bool,
    pub connected: Option<[i32; 3]>,
    pub keep_alive: i32,
}

impl Stationary {
    /// `startTransferringTo`.
    pub fn start_transferring_to(&mut self, contraption: [i32; 3], distance: f32) {
        if self.connected == Some(contraption) {
            return;
        }
        self.distance = distance.min(2.0);
        self.connected = Some(contraption);
        // `startConnecting`.
        self.timer = 6 + ANIMATION * 2;
    }

    /// `stopTransferring`.
    pub fn stop_transferring(&mut self) {
        self.connected = None;
    }

    /// `isConnected`.
    pub fn is_connected(&self) -> bool {
        self.timer >= ANIMATION && self.timer <= TIMEOUT + ANIMATION
    }

    /// `canTransfer`: the contraption it is connected to, while connected (none: nothing).
    pub fn can_transfer(&self) -> Option<[i32; 3]> {
        self.connected.filter(|_| self.is_connected())
    }

    /// `onContentTransferred`.
    pub fn on_content_transferred(&mut self) {
        self.timer = TIMEOUT + ANIMATION;
    }

    /// `neighbourChanged` with the block's signal now: true when contraptions near it should
    /// look for it again (it lost its power); power lets the contraption go.
    pub fn neighbour_changed(&mut self, powered: bool) -> bool {
        if powered == self.powered {
            return false;
        }
        self.powered = powered;
        if powered {
            self.stop_transferring();
        }
        !powered
    }

    /// `tick` on the server.
    pub fn tick(&mut self) {
        if self.keep_alive > 0 {
            self.keep_alive -= 1;
            if self.keep_alive == 0 {
                self.stop_transferring();
                self.timer = ANIMATION - 1;
                return;
            }
        }
        self.timer = self.timer.min(ANIMATION * 2 + TIMEOUT);
        let can_decrement = self.timer > ANIMATION || self.timer > 0 && self.keep_alive == 0;
        if can_decrement {
            self.timer -= 1;
            if self.timer <= 0 || self.powered {
                self.stop_transferring();
            }
        }
    }

    /// Nothing left to tick for.
    pub fn idle(&self) -> bool {
        self.timer <= 0 && self.keep_alive == 0 && self.connected.is_none()
    }
}

/// What a moving interface reads of the world, and does to the stationary ones.
pub trait World {
    /// `getStationaryInterfaceAt`: an interface of the same kind at `pos` facing `facing`'s
    /// opposite, not powered.
    fn interface_at(&self, pos: [i32; 3], facing: Dir) -> bool;
    /// The stationary interface at `pos`.
    fn stationary(&mut self, pos: [i32; 3]) -> Option<&mut Stationary>;
}

/// A moving interface (`MovementContext` with `WorkingPos`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Moving {
    pub working: Option<[i32; 3]>,
}

fn centre(p: [i32; 3]) -> [f64; 3] {
    p.map(|v| f64::from(v) + 0.5)
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

impl Moving {
    /// `visitNewPosition` → `findInterface` (a translating contraption: its facing is its
    /// block's): a stationary interface at `pos` or the block beyond it, facing back; it starts
    /// connecting to `contraption` at the distance between their faces.
    pub fn visit(
        &mut self,
        world: &mut impl World,
        contraption: [i32; 3],
        pos: [i32; 3],
        position: [f64; 3],
        facing: Dir,
    ) {
        let o = facing.offset();
        let found = (0..2)
            .map(|i| [pos[0] + o[0] * i, pos[1] + o[1] * i, pos[2] + o[2] * i])
            .find(|p| world.interface_at(*p, facing));
        let Some(at) = found else {
            self.working = None;
            return;
        };
        self.working = Some(at);
        // `VecHelper.project(diff, facing)`: along the facing only.
        let c = centre(at);
        let along = (0..3)
            .map(|i| (c[i] - position[i]) * f64::from(o[i]))
            .sum::<f64>()
            .abs();
        let d = (along + REACH - 1.0) as f32;
        if let Some(s) = world.stationary(at) {
            s.start_transferring_to(contraption, d);
        }
    }

    /// `tick` on the server: the contraption stalls once it has come closest to the stationary
    /// interface, which it keeps alive; it moves on once that one's timer has run down to
    /// `ANIMATION`. Returns the actor's `stall`.
    pub fn tick(
        &mut self,
        world: &mut impl World,
        contraption: [i32; 3],
        position: [f64; 3],
        motion: [f64; 3],
        facing: Dir,
        mut stall: bool,
    ) -> bool {
        let Some(at) = self.working else {
            // `cancelStall` → `reset`.
            return false;
        };
        let target = centre(at);
        let next = [0, 1, 2].map(|i| position[i] + motion[i]);
        if !stall && distance(position, target) < distance(target, next) {
            stall = true;
        }
        if !world.interface_at(at, facing) {
            self.working = None;
            return false;
        }
        let Some(s) = world.stationary(at) else {
            self.working = None;
            return false;
        };
        if s.connected.is_none() {
            let d = s.distance;
            s.start_transferring_to(contraption, d);
        }
        let timer_below = s.timer <= ANIMATION;
        s.keep_alive = 2;
        if stall && timer_below {
            stall = false;
        }
        stall
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Interfaces(HashMap<[i32; 3], (Dir, Stationary)>);

    impl World for Interfaces {
        fn interface_at(&self, pos: [i32; 3], facing: Dir) -> bool {
            self.0
                .get(&pos)
                .is_some_and(|(f, s)| *f == facing.opposite() && !s.powered)
        }
        fn stationary(&mut self, pos: [i32; 3]) -> Option<&mut Stationary> {
            self.0.get_mut(&pos).map(|e| &mut e.1)
        }
    }

    #[test]
    fn a_stationary_interface_connects_holds_and_lets_go() {
        let mut s = Stationary::default();
        s.start_transferring_to([0; 3], 1.85);
        assert_eq!(s.timer, 14);
        assert!(!s.is_connected() || s.timer <= TIMEOUT + ANIMATION);
        // Kept alive, it runs down to ANIMATION and holds there.
        for _ in 0..20 {
            s.keep_alive = 2;
            s.tick();
        }
        assert_eq!(s.timer, ANIMATION);
        assert_eq!(s.can_transfer(), Some([0; 3]));
        // A transfer holds the connection TIMEOUT ticks more.
        s.on_content_transferred();
        assert_eq!(s.timer, 64);
        let mut ticks = 0;
        while s.timer > ANIMATION {
            s.keep_alive = 2;
            s.tick();
            ticks += 1;
        }
        assert_eq!(ticks, TIMEOUT);
        // Not kept alive (the last keep-alive already counted down once): it lets go on the next
        // tick and draws back.
        assert_eq!(s.keep_alive, 1);
        s.tick();
        assert_eq!((s.connected, s.timer), (None, ANIMATION - 1));
        for _ in 0..3 {
            s.tick();
        }
        assert!(s.idle());
        // Powered: it lets go at once.
        s.start_transferring_to([1; 3], 3.0);
        assert_eq!(s.distance, 2.0);
        assert!(!s.neighbour_changed(true));
        assert_eq!(s.connected, None);
        assert!(s.neighbour_changed(false));
    }

    #[test]
    fn a_moving_interface_stops_at_the_closest_point_and_waits_out_the_timeout() {
        // A stationary interface at x 4 facing west; the moving one faces east, its block at
        // offset `x` along x, reaching 1.85 ahead.
        let mut world = Interfaces(HashMap::from([(
            [4, 0, 0],
            (Dir::West, Stationary::default()),
        )]));
        let mut m = Moving::default();
        let key = [9, 9, 9];
        let speed = 0.125;
        let mut x = 0.0f64;
        let mut stall = false;
        let mut grid = None;
        let mut stalled_at = None;
        let mut released_at = None;
        let mut transferred_at = None;
        for t in 0..400 {
            if !stall {
                x += speed;
            }
            let position = [x + 0.5 + REACH, 0.5, 0.5];
            let g = position.map(|v| v.floor() as i32);
            if grid != Some(g) {
                grid = Some(g);
                m.visit(&mut world, key, g, position, Dir::East);
            }
            let was = stall;
            stall = m.tick(
                &mut world,
                key,
                position,
                [speed, 0.0, 0.0],
                Dir::East,
                stall,
            );
            if stall && !was && stalled_at.is_none() {
                stalled_at = Some((t, x));
            }
            if was && !stall && released_at.is_none() {
                released_at = Some(t);
            }
            let s = &mut world.0.get_mut(&[4, 0, 0]).unwrap().1;
            // A funnel takes an item as soon as it may.
            if transferred_at.is_none() && s.can_transfer() == Some(key) {
                s.on_content_transferred();
                transferred_at = Some(t);
            }
            // The stationary one ticks after the contraption.
            s.tick();
        }
        let (_, stall_x) = stalled_at.unwrap();
        // It stops where its reach is closest to the stationary interface's centre: a block
        // between the two.
        assert_eq!(stall_x, 2.125);
        // The transfer holds it until the timer has run the 60 ticks down to ANIMATION.
        let (from, released) = (transferred_at.unwrap(), released_at.unwrap());
        assert_eq!(released - from, TIMEOUT as usize);
        assert_eq!(world.0[&[4, 0, 0]].1.connected, None, "let go once it left");
    }
}
