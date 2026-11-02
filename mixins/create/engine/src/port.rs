//! Create's package ports (`PackagePortBlockEntity`, `FrogportBlockEntity`, `PostboxBlockEntity`,
//! `PackagePortAutomationInventoryWrapper`, `PackagePortTarget.ChainConveyorFrogportTarget`) and
//! a dimension's package logistics: packagers, ports and chain conveyors together.
//!
//! A port keeps 18 packages and an address filter. A frogport sends packages that are not for
//! it (its own, or from the inventory below) onto its chain conveyor, and catches those that
//! match its filter, handing them down to the inventory below. Postboxes hold packages for
//! trains, which are not ported: they keep what automation and the menu put in. Nothing here
//! ticks for an idle port: a frogport looks at its inventories when something changes (a
//! package caught or put in, the inventory below or the packager below, its chain's speed) and
//! again every 10 ticks only while something it could send is held back.

use crate::chain_conveyor::{Chains, Event as ChainEvent, Parcel};
use crate::package::match_address;
use crate::packager::Packager;
use std::collections::HashMap;

/// `PackagePortBlockEntity.inventory`: 18 slots.
pub const SLOTS: usize = 18;
/// The frogport's `lazyTick` period while it waits on its target.
pub const RETRY: u64 = 10;

/// A frogport's chain conveyor (`ChainConveyorFrogportTarget`): the conveyor, where on it
/// (degrees round the wheel, or blocks along the chain toward `connection`), and whether it was
/// chosen while the chain ran backwards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub conveyor: [i32; 3],
    pub chain_pos: f32,
    pub connection: Option<[i32; 3]>,
    pub flipped: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Port<P> {
    pub frog: bool,
    /// `addressFilter`.
    pub filter: String,
    /// `acceptsPackages`.
    pub accepts: bool,
    pub target: Option<Target>,
    pub items: Vec<Option<Parcel<P>>>,
    /// `animationProgress` (linear, chasing 1 at 0.1 a tick while animating).
    progress: f32,
    animating: bool,
    /// `currentlyDepositing`.
    pub deposit: bool,
    pub animated: Option<Parcel<P>>,
    /// `failedLastExport`.
    pub failed_export: bool,
    /// `passiveYaw`.
    pub yaw: f32,
}

impl<P> Port<P> {
    pub fn new(frog: bool) -> Self {
        Port {
            frog,
            filter: String::new(),
            accepts: true,
            target: None,
            items: (0..SLOTS).map(|_| None).collect(),
            progress: 0.0,
            animating: false,
            deposit: false,
            animated: None,
            failed_export: false,
            yaw: 0.0,
        }
    }

    /// `getFilterString`.
    pub fn filter_string(&self) -> Option<&str> {
        self.accepts.then_some(self.filter.as_str())
    }

    /// `isAnimationInProgress`.
    pub fn is_animating(&self) -> bool {
        self.animating
    }

    /// Whether it holds a package not for it (one `tryPullingFrom(inventory)` would send).
    pub fn has_outgoing(&self) -> bool {
        let filter = self.filter_string();
        self.items
            .iter()
            .flatten()
            .any(|b| filter.is_none_or(|f| !match_address(&b.address, f)))
    }

    /// Its slots' stacks.
    pub fn stacks(&self) -> Vec<Option<P>>
    where
        P: Clone,
    {
        let slot = |s: &Option<Parcel<P>>| s.as_ref().map(|b| b.stack.clone());
        self.items.iter().map(slot).collect()
    }

    /// `isBackedUp`.
    pub fn backed_up(&self) -> bool {
        self.items.iter().all(Option::is_some)
    }

    /// `PackagePortAutomationInventoryWrapper.insertItem`: only packages, and not those for
    /// this port (a frogport sends them on).
    pub fn automation_takes(&self, address: &str) -> bool {
        !self
            .filter_string()
            .is_some_and(|f| match_address(address, f))
    }

    /// `PackagePortAutomationInventoryWrapper.extractItem`: only packages for this port.
    pub fn automation_gives(&self, address: &str) -> bool {
        self.filter_string()
            .is_some_and(|f| match_address(address, f))
    }

    /// `ItemHandlerHelper.insertItem` into the first empty slot; the package back when full.
    pub fn insert(&mut self, p: Parcel<P>) -> Option<Parcel<P>> {
        match self.items.iter_mut().find(|s| s.is_none()) {
            Some(s) => {
                *s = Some(p);
                None
            }
            None => Some(p),
        }
    }

    /// `startAnimation`: the frog catches (`deposit` false) or sends a package.
    pub fn start(&mut self, p: Parcel<P>, deposit: bool) {
        self.progress = 0.0;
        self.animating = true;
        self.animated = Some(p);
        self.deposit = deposit;
    }

    /// The animation's progress, 0 to 1 (for clients).
    pub fn progress(&self) -> f32 {
        self.progress
    }

    /// One tick of the animation (`FrogportBlockEntity.tick`, the server's part).
    pub fn tick(&mut self) -> PortStep<P> {
        if !self.animating {
            return PortStep::Idle;
        }
        // LerpedFloat.tickChaser with Chaser.LINEAR at 0.1 toward 1.
        if (f64::from(self.progress) - 1.0).abs() < 1.0e-5 {
            self.progress = 1.0;
        } else {
            let c = f64::from(self.progress);
            self.progress = (c + (1.0 - c).clamp(-0.1f32 as f64, 0.1f32 as f64)) as f32;
        }
        let mut step = PortStep::Moving;
        if self.deposit && self.progress > 0.5 && self.animated.is_some() {
            step = PortStep::Send(self.animated.take().unwrap());
        }
        if self.progress < 1.0 {
            return step;
        }
        self.progress = 0.0;
        self.animating = false;
        match self.animated.take() {
            Some(p) if !self.deposit => PortStep::Caught(p),
            _ => PortStep::Done,
        }
    }
}

/// What a port's tick did.
#[derive(Clone, Debug, PartialEq)]
pub enum PortStep<P> {
    Idle,
    Moving,
    /// Halfway through sending: the package goes to the target (`export`).
    Send(Parcel<P>),
    /// A catch ended: the package goes into the port.
    Caught(Parcel<P>),
    /// The animation ended.
    Done,
}

/// A dimension's packagers, ports and chain conveyors.
#[derive(Clone, Debug)]
pub struct Packaging<P> {
    pub packagers: HashMap<[i32; 3], Packager<Parcel<P>>>,
    pub ports: HashMap<[i32; 3], Port<P>>,
    pub chains: Chains<P>,
    /// Packagers that tick (animating, cooling down, powered).
    pub active: Vec<[i32; 3]>,
    /// Ports animating.
    animating: Vec<[i32; 3]>,
    /// Frogports to look at their inventories now.
    pub wake: Vec<[i32; 3]>,
    /// Frogports to look again at a later tick (held back by their target).
    retry: Vec<([i32; 3], u64)>,
    /// Packagers a neighbour changed beside (their power).
    pub redstone: Vec<[i32; 3]>,
    /// Columns whose saved state changed.
    pub unsaved: Vec<(i32, i32)>,
}

impl<P> Default for Packaging<P> {
    fn default() -> Self {
        Packaging {
            packagers: HashMap::new(),
            ports: HashMap::new(),
            chains: Chains::default(),
            active: Vec::new(),
            animating: Vec::new(),
            wake: Vec::new(),
            retry: Vec::new(),
            redstone: Vec::new(),
            unsaved: Vec::new(),
        }
    }
}

/// What a dimension's tick leaves to the game.
#[derive(Clone, Debug, PartialEq)]
pub enum Out<P> {
    /// A packager's lazy tick while powered: read its power, pack if still powered.
    Recheck([i32; 3]),
    /// A packager's animation ended (its box is out, or its tray back in).
    PackagerDone([i32; 3]),
    /// A package falls where it is.
    Drop([f64; 3], Parcel<P>),
}

impl<P: Clone> Packaging<P> {
    pub fn touch(&mut self, pos: [i32; 3]) {
        self.unsaved.push((pos[0] >> 4, pos[2] >> 4));
    }

    /// The packager at `pos`, made when missing.
    pub fn packager(&mut self, pos: [i32; 3]) -> &mut Packager<Parcel<P>> {
        self.packagers.entry(pos).or_default()
    }

    /// The packager at `pos` needs ticks now.
    pub fn activate(&mut self, pos: [i32; 3]) {
        if !self.active.contains(&pos) {
            self.active.push(pos);
        }
        self.touch(pos);
    }

    /// The port at `pos`, made when missing.
    pub fn port(&mut self, pos: [i32; 3], frog: bool) -> &mut Port<P> {
        self.ports.entry(pos).or_insert_with(|| Port::new(frog))
    }

    /// A package goes to the port's animation (catching or sending).
    pub fn start(&mut self, pos: [i32; 3], p: Parcel<P>, deposit: bool) {
        let Some(port) = self.ports.get_mut(&pos) else {
            return;
        };
        port.start(p, deposit);
        if !self.animating.contains(&pos) {
            self.animating.push(pos);
        }
        self.touch(pos);
    }

    pub fn wake(&mut self, pos: [i32; 3]) {
        if self.ports.get(&pos).is_some_and(|p| p.frog) && !self.wake.contains(&pos) {
            self.wake.push(pos);
        }
    }

    /// The frogport at `pos` looks again `RETRY` ticks after `now`.
    pub fn retry(&mut self, pos: [i32; 3], now: u64) {
        if !self.retry.iter().any(|(p, _)| *p == pos) {
            self.retry.push((pos, now + RETRY));
        }
    }

    /// Whether the frogport may send now (`tryPullingFromOwnAndAdjacentInventories`' guards):
    /// not animating, its target turning and with room.
    pub fn can_send(&self, pos: [i32; 3]) -> bool {
        let Some(p) = self.ports.get(&pos) else {
            return false;
        };
        let Some(t) = p.target else { return false };
        p.frog && !p.animating && self.chains.can_accept(t.conveyor, t.connection)
    }

    /// `tryPullingFrom(inventory)`: the first of its own packages not for it starts on its way.
    pub fn send_own(&mut self, pos: [i32; 3]) -> bool {
        let Some(port) = self.ports.get_mut(&pos) else {
            return false;
        };
        let filter = port.filter_string().map(str::to_owned);
        let Some(slot) = port.items.iter().position(|s| {
            s.as_ref().is_some_and(|p| {
                filter
                    .as_deref()
                    .is_none_or(|f| !match_address(&p.address, f))
            })
        }) else {
            return false;
        };
        let p = port.items[slot].take().unwrap();
        self.start(pos, p, true);
        true
    }

    /// `ChainConveyorFrogportTarget.register` of the frogport at `pos` (and `deregister` from
    /// where it was): a chain that runs the other way now has it jump to the conveyor at the
    /// chain's other end.
    pub fn register(&mut self, pos: [i32; 3]) {
        let Some(port) = self.ports.get(&pos) else {
            return;
        };
        let Some(mut t) = port.target else { return };
        let filter = port.filter_string().map(str::to_owned);
        self.chains.deregister_port(pos, t.conveyor);
        let Some(c) = self.chains.conveyors.get(&t.conveyor) else {
            return;
        };
        if let Some(k) = t.connection
            && (c.speed < 0.0) != t.flipped
        {
            let other = [
                t.conveyor[0] + k[0],
                t.conveyor[1] + k[1],
                t.conveyor[2] + k[2],
            ];
            if !self.chains.conveyors.contains_key(&other) {
                return;
            }
            let length = crate::chain_conveyor::connection_stats(t.conveyor, k, c.reversed).length;
            t = Target {
                conveyor: other,
                chain_pos: length - t.chain_pos,
                connection: Some([-k[0], -k[1], -k[2]]),
                flipped: !t.flipped,
            };
            self.ports.get_mut(&pos).unwrap().target = Some(t);
            self.touch(pos);
        }
        if let Some(f) = filter {
            self.chains
                .register_port(pos, t.conveyor, t.chain_pos, t.connection, f);
        }
    }

    /// The port at `pos` is gone or unloaded: off its conveyor. Its packages, when it was
    /// broken.
    pub fn remove_port(&mut self, pos: [i32; 3]) -> Vec<Parcel<P>> {
        let Some(p) = self.ports.remove(&pos) else {
            return Vec::new();
        };
        if let Some(t) = p.target {
            self.chains.deregister_port(pos, t.conveyor);
        }
        self.animating.retain(|x| *x != pos);
        self.wake.retain(|x| *x != pos);
        self.retry.retain(|(x, _)| *x != pos);
        p.items.into_iter().flatten().chain(p.animated).collect()
    }

    /// The ports whose target is the conveyor at `conveyor` register again (its speed or
    /// chains changed) and look at their inventories.
    pub fn conveyor_changed(&mut self, conveyor: [i32; 3]) {
        let ports: Vec<[i32; 3]> = self
            .ports
            .iter()
            .filter(|(_, p)| {
                p.target.is_some_and(|t| {
                    t.conveyor == conveyor
                        || t.connection.is_some_and(|k| {
                            [
                                t.conveyor[0] + k[0],
                                t.conveyor[1] + k[1],
                                t.conveyor[2] + k[2],
                            ] == conveyor
                        })
                })
            })
            .map(|(p, _)| *p)
            .collect();
        for p in ports {
            self.register(p);
            self.wake(p);
        }
    }

    /// The frogports due to look again at tick `now`.
    pub fn due(&mut self, now: u64) -> Vec<[i32; 3]> {
        let mut out = std::mem::take(&mut self.wake);
        self.retry.retain(|(p, at)| {
            if *at <= now {
                if !out.contains(p) {
                    out.push(*p);
                }
                false
            } else {
                true
            }
        });
        out
    }

    /// Whether this dimension has nothing to tick.
    pub fn is_idle(&self) -> bool {
        self.active.is_empty()
            && self.animating.is_empty()
            && self.wake.is_empty()
            && self.retry.is_empty()
            && self.redstone.is_empty()
            && self.chains.is_idle()
    }

    /// One tick: packagers, the chains (catching into frogports), then the frogports'
    /// animations (sending onto the chains, catching into their slots).
    pub fn tick(&mut self, out: &mut Vec<Out<P>>) {
        let mut i = 0;
        while i < self.active.len() {
            let pos = self.active[i];
            let Some(p) = self.packagers.get_mut(&pos) else {
                self.active.swap_remove(i);
                continue;
            };
            let step = p.tick();
            if step.recheck {
                out.push(Out::Recheck(pos));
            }
            if step.done {
                out.push(Out::PackagerDone(pos));
                // `wakeTheFrogs`: a frogport on top may take the box.
                self.wake.push([pos[0], pos[1] + 1, pos[2]]);
                self.unsaved.push((pos[0] >> 4, pos[2] >> 4));
            }
            if p.active() {
                i += 1;
            } else {
                self.active.swap_remove(i);
            }
        }
        self.wake.retain(|w| self.ports.contains_key(w));
        let mut events = Vec::new();
        let ports = &self.ports;
        self.chains.tick(
            |at| {
                ports
                    .get(&at)
                    .is_some_and(|p| p.frog && !p.animating && !p.backed_up())
            },
            &mut events,
        );
        for e in events {
            match e {
                ChainEvent::Catch(at, parcel) => self.start(at, parcel, false),
                ChainEvent::Anticipate(_) => {}
                ChainEvent::Drop(v, parcel) => out.push(Out::Drop(v, parcel)),
            }
        }
        let mut i = 0;
        while i < self.animating.len() {
            let pos = self.animating[i];
            let Some(port) = self.ports.get_mut(&pos) else {
                self.animating.swap_remove(i);
                continue;
            };
            let target = port.target;
            let step = port.tick();
            let finished = !port.animating;
            match step {
                PortStep::Send(parcel) => {
                    let sent = match target {
                        Some(t) => {
                            self.chains
                                .insert(t.conveyor, t.chain_pos, t.connection, parcel)
                        }
                        None => Err(parcel),
                    };
                    if let Err(parcel) = sent {
                        out.push(Out::Drop(drop_at(pos), parcel));
                    }
                }
                PortStep::Caught(parcel) => {
                    if let Some(parcel) = self.ports.get_mut(&pos).unwrap().insert(parcel) {
                        out.push(Out::Drop(drop_at(pos), parcel));
                    }
                }
                _ => {}
            }
            if finished {
                self.animating.swap_remove(i);
                self.touch(pos);
                self.wake(pos);
            } else {
                i += 1;
            }
        }
    }
}

/// `Block.popResource` at a port: its centre.
fn drop_at(pos: [i32; 3]) -> [f64; 3] {
    pos.map(|v| f64::from(v) + 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parcel(address: &str) -> Parcel<u32> {
        Parcel {
            stack: 1,
            address: address.into(),
        }
    }

    #[test]
    fn the_frog_sends_at_half_and_catches_at_the_end() {
        let mut p = Port::<u32>::new(true);
        p.start(parcel("a"), true);
        let mut sent_at = None;
        for t in 1..=10 {
            match p.tick() {
                PortStep::Send(_) => sent_at = Some(t),
                PortStep::Done => assert_eq!(t, 10),
                _ => {}
            }
        }
        // 0.1 a tick: past 0.5 on the sixth.
        assert_eq!(sent_at, Some(6));
        assert!(!p.is_animating());
        p.start(parcel("a"), false);
        let mut caught = None;
        for t in 1..=10 {
            if let PortStep::Caught(_) = p.tick() {
                caught = Some(t);
            }
        }
        assert_eq!(caught, Some(10));
    }

    #[test]
    fn automation_rules_follow_the_filter() {
        let mut p = Port::<u32>::new(true);
        p.filter = "Home".into();
        assert!(p.automation_takes("Mine"), "outgoing");
        assert!(!p.automation_takes("Home"));
        assert!(p.automation_gives("Home"), "arrived");
        assert!(!p.automation_gives("Mine"));
        p.accepts = false;
        assert!(p.automation_takes("Home"));
        assert!(!p.automation_gives("Home"));
        for _ in 0..SLOTS {
            assert!(p.insert(parcel("x")).is_none());
        }
        assert!(p.backed_up());
        assert!(p.insert(parcel("x")).is_some());
    }

    #[test]
    fn two_frogports_on_a_chain_deliver_by_address() {
        let mut pk = Packaging::<u32>::default();
        for x in [0, 4] {
            pk.chains.add([x, 0, 0], vec![]);
            pk.chains.set_speed([x, 0, 0], 64.0);
        }
        pk.chains.connect([0, 0, 0], [4, 0, 0]);
        // A frogport at each end: "Base" beside the first, "Mine" beside the second.
        for (pos, conveyor, filter) in [
            ([0, 0, 2], [0, 0, 0], "Base"),
            ([4, 0, 2], [4, 0, 0], "Mine"),
        ] {
            let p = pk.port(pos, true);
            p.filter = filter.into();
            p.target = Some(Target {
                conveyor,
                chain_pos: 0.0,
                connection: None,
                flipped: false,
            });
            pk.register(pos);
        }
        // A package for the mine goes into the base's frogport: it is sent.
        assert!(
            pk.ports
                .get_mut(&[0, 0, 2])
                .unwrap()
                .insert(parcel("Mine"))
                .is_none()
        );
        assert!(pk.can_send([0, 0, 2]));
        assert!(pk.send_own([0, 0, 2]));
        let mut out = Vec::new();
        for _ in 0..400 {
            pk.tick(&mut out);
        }
        assert!(out.is_empty(), "{out:?}");
        let mine = &pk.ports[&[4, 0, 2]];
        assert_eq!(mine.items.iter().flatten().count(), 1, "caught at the mine");
        assert!(pk.ports[&[0, 0, 2]].items.iter().all(Option::is_none));
        // A package for the base stays put in the base (its own address).
        pk.ports.get_mut(&[0, 0, 2]).unwrap().insert(parcel("Base"));
        assert!(!pk.send_own([0, 0, 2]));
        // Nothing left to do: idle.
        pk.due(1000);
        assert!(pk.is_idle());
    }

    #[test]
    fn a_reversed_chain_moves_the_port_to_the_other_end() {
        let mut pk = Packaging::<u32>::default();
        for x in [0, 4] {
            pk.chains.add([x, 0, 0], vec![]);
            pk.chains.set_speed([x, 0, 0], 32.0);
        }
        pk.chains.connect([0, 0, 0], [4, 0, 0]);
        let p = pk.port([2, 0, 1], true);
        p.target = Some(Target {
            conveyor: [0, 0, 0],
            chain_pos: 1.0,
            connection: Some([4, 0, 0]),
            flipped: false,
        });
        pk.register([2, 0, 1]);
        assert_eq!(pk.chains.conveyors[&[0, 0, 0]].travel_ports.len(), 1);
        pk.chains.set_speed([0, 0, 0], -32.0);
        pk.conveyor_changed([0, 0, 0]);
        let t = pk.ports[&[2, 0, 1]].target.unwrap();
        assert_eq!(t.conveyor, [4, 0, 0]);
        assert_eq!(t.connection, Some([-4, 0, 0]));
        assert!(t.flipped);
        assert!(pk.chains.conveyors[&[0, 0, 0]].travel_ports.is_empty());
        assert_eq!(pk.chains.conveyors[&[4, 0, 0]].travel_ports.len(), 1);
    }
}
