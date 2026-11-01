//! Create's redstone link (`RedstoneLinkBlock`, `RedstoneLinkBlockEntity`, `LinkBehaviour`,
//! `RedstoneLinkNetworkHandler`, `RedstoneLinkFrequencySlot`): links whose two frequency items
//! match form a network; a receiver gives the strongest signal its network's transmitters within
//! 256 blocks take in. Networks are looked at only when a link joins, leaves, retunes or its input
//! changes.

use crate::kinetics::Dir;
use std::collections::HashMap;

/// `linkRange`.
pub const RANGE: i32 = 256;

/// A network's key (`Couple<Frequency>`): the first and last slot's item, "" for none. Create
/// also tells dyed items apart by colour; items here are told apart by id.
pub type Key = (String, String);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Link {
    pub key: Key,
    pub receiver: bool,
    /// `transmittedSignal` (a transmitter's input), `receivedSignal` (a receiver's network).
    pub transmit: i32,
    pub received: i32,
}

/// Every loaded link of a level, and its networks.
#[derive(Clone, Debug, Default)]
pub struct Links {
    pub links: HashMap<[i32; 3], Link>,
    nets: HashMap<Key, Vec<[i32; 3]>>,
}

/// `Vec3i.closerThan(other, linkRange)`.
pub fn within_range(a: [i32; 3], b: [i32; 3]) -> bool {
    let d = |i: usize| f64::from(a[i] - b[i]);
    d(0) * d(0) + d(1) * d(1) + d(2) * d(2) < f64::from(RANGE) * f64::from(RANGE)
}

impl Links {
    /// `addToNetwork` (`LinkBehaviour.initialize`): the receivers whose signal changed.
    pub fn add(&mut self, pos: [i32; 3], link: Link) -> Vec<[i32; 3]> {
        let key = link.key.clone();
        if let Some(old) = self.links.insert(pos, link) {
            self.leave(pos, &old.key);
        }
        let net = self.nets.entry(key.clone()).or_default();
        if !net.contains(&pos) {
            net.push(pos);
        }
        self.update(pos, &key)
    }

    /// `removeFromNetwork` (`LinkBehaviour.unload`, the block gone): the receivers whose signal
    /// changed.
    pub fn remove(&mut self, pos: [i32; 3]) -> Vec<[i32; 3]> {
        let Some(old) = self.links.remove(&pos) else {
            return Vec::new();
        };
        if self.leave(pos, &old.key) {
            return Vec::new();
        }
        self.update(pos, &old.key)
    }

    /// Takes `pos` out of its network's list; true when the network is gone.
    fn leave(&mut self, pos: [i32; 3], key: &Key) -> bool {
        let Some(net) = self.nets.get_mut(key) else {
            return true;
        };
        net.retain(|p| *p != pos);
        if net.is_empty() {
            self.nets.remove(key);
            return true;
        }
        false
    }

    /// `RedstoneLinkBlockEntity.transmit` and `notifySignalChange`.
    pub fn transmit(&mut self, pos: [i32; 3], strength: i32) -> Vec<[i32; 3]> {
        let Some(l) = self.links.get_mut(&pos) else {
            return Vec::new();
        };
        l.transmit = strength;
        let key = l.key.clone();
        self.update(pos, &key)
    }

    /// `LinkBehaviour.setFrequency`: `item` ("" for an empty hand) in the first or last slot; the
    /// link leaves its network and joins the new one when it changed.
    pub fn set_frequency(&mut self, pos: [i32; 3], first: bool, item: &str) -> Vec<[i32; 3]> {
        let Some(l) = self.links.get(&pos).cloned() else {
            return Vec::new();
        };
        let mut key = l.key.clone();
        let slot = if first { &mut key.0 } else { &mut key.1 };
        if slot == item {
            return Vec::new();
        }
        *slot = item.to_owned();
        let mut changed = self.remove(pos);
        changed.extend(self.add(pos, Link { key, ..l }));
        changed.sort();
        changed.dedup();
        changed
    }

    /// The block's `receiver` changed (`RedstoneLinkBlockEntity.tick`): the old behaviour leaves
    /// the network, the new one, with the same frequencies, joins it.
    pub fn set_receiver(&mut self, pos: [i32; 3], receiver: bool) -> Vec<[i32; 3]> {
        let Some(l) = self.links.get(&pos).cloned() else {
            return Vec::new();
        };
        if l.receiver == receiver {
            return Vec::new();
        }
        let mut changed = self.remove(pos);
        changed.extend(self.add(pos, Link { receiver, ..l }));
        changed.sort();
        changed.dedup();
        changed
    }

    /// `updateNetworkOf(actor)`: the strongest transmitter within range of the actor, given to
    /// the actor (if it listens) and every receiver within its range. The receivers whose signal
    /// changed.
    fn update(&mut self, actor: [i32; 3], key: &Key) -> Vec<[i32; 3]> {
        let Some(net) = self.nets.get(key) else {
            return Vec::new();
        };
        let mut power = 0;
        for p in net {
            if !within_range(actor, *p) {
                continue;
            }
            if power < 15
                && let Some(l) = self.links.get(p)
                && !l.receiver
            {
                power = power.max(l.transmit);
            }
        }
        let mut changed = Vec::new();
        for p in net {
            if !within_range(actor, *p) {
                continue;
            }
            if let Some(l) = self.links.get_mut(p)
                && l.receiver
                && l.received != power
            {
                l.received = power;
                changed.push(*p);
            }
        }
        changed
    }

    /// A receiver's signal (`getReceivedSignal`), 0 for anything else.
    pub fn received(&self, pos: [i32; 3]) -> i32 {
        self.links
            .get(&pos)
            .filter(|l| l.receiver)
            .map_or(0, |l| l.received)
    }
}

/// `RedstoneLinkBlock.getSignal` / `getDirectSignal` of a receiver facing `facing` holding
/// `received`; `side` is vanilla's direction (from the reader toward the link).
pub fn signal(receiver: bool, facing: Dir, received: i32, side: Dir, direct: bool) -> i32 {
    if !receiver || direct && side != facing {
        return 0;
    }
    received
}

/// `RedstoneLinkFrequencySlot.getLocalOffset`: where a slot sits in the block, in block units.
pub fn slot_offset(facing: Dir, first: bool) -> [f64; 3] {
    let v = |x: f64, y: f64, z: f64| [x / 16.0, y / 16.0, z / 16.0];
    if !matches!(facing, Dir::Up | Dir::Down) {
        let mut l = v(8.0, 5.5, 3.01);
        if first {
            l[1] += 5.0 / 16.0;
        }
        // `rotateHorizontally`: about the block's centre by `horizontalAngle(facing)` (`toYRot`,
        // negated on the x axis: south 0, west -90, north 180, east -270), `VecHelper.rotate`
        // about Y.
        let angle: f64 = match facing {
            Dir::South => 0.0,
            Dir::West => -90.0,
            Dir::North => 180.0,
            _ => -270.0,
        };
        let (s, c) = angle.to_radians().sin_cos();
        let (x, z) = (l[0] - 0.5, l[2] - 0.5);
        return [x * c + z * s + 0.5, l[1], z * c - x * s + 0.5];
    }
    let mut l = v(8.0, 3.01, 5.5);
    if first {
        l[2] += 5.0 / 16.0;
    }
    if facing == Dir::Down {
        // About X by 180 through the centre.
        l = [l[0], 1.0 - l[1], 1.0 - l[2]];
    }
    l
}

/// `ValueBoxTransform.Dual.testHit`: within `scale / 3.5` (scale .4975) of the slot.
pub fn hit_slot(facing: Dir, local: [f64; 3]) -> Option<bool> {
    // LinkHandler tests the second slot first.
    [false, true].into_iter().find(|&first| {
        let o = slot_offset(facing, first);
        let d = (0..3)
            .map(|i| (local[i] - o[i]).powi(2))
            .sum::<f64>()
            .sqrt();
        d < 0.4975 / 3.5
    })
}

/// The mod message channel for links' frequencies, server to client.
pub const CHANNEL: &str = "create:link";

/// A link's frequencies as clients get them: none, both "", hides its items.
pub type Shown = ([i32; 3], Dir, Key);

/// `[x, y, z, facing]` then each item as a byte length and its id, per link.
pub fn encode(links: &[Shown]) -> Vec<u8> {
    let mut out = Vec::new();
    for (pos, facing, key) in links {
        for v in pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(*facing as u8);
        for item in [&key.0, &key.1] {
            let b = &item.as_bytes()[..item.len().min(255)];
            out.push(b.len() as u8);
            out.extend_from_slice(b);
        }
    }
    out
}

pub fn decode(mut data: &[u8]) -> Vec<Shown> {
    let mut out = Vec::new();
    let take = |n: usize, data: &mut &[u8]| -> Option<Vec<u8>> {
        let (a, b) = (data.get(..n)?.to_vec(), data.get(n..)?);
        *data = b;
        Some(a)
    };
    while !data.is_empty() {
        let Some(head) = take(13, &mut data) else {
            break;
        };
        let int = |i: usize| i32::from_le_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
        let Some(&facing) = Dir::ALL.get(usize::from(head[12])) else {
            break;
        };
        let mut items = [String::new(), String::new()];
        for item in &mut items {
            let Some(n) = take(1, &mut data) else {
                return out;
            };
            let Some(b) = take(usize::from(n[0]), &mut data) else {
                return out;
            };
            *item = String::from_utf8_lossy(&b).into_owned();
        }
        let [a, b] = items;
        out.push(([int(0), int(4), int(8)], facing, (a, b)));
    }
    out
}

/// `LinkRenderer.renderOnBlockEntity` for one slot: `ValueBoxTransform.transform` (the slot's
/// offset, `RedstoneLinkFrequencySlot.rotate`, its .4975 scale), then
/// `ValueBoxRenderer.renderItemIntoValueBox` (a flat item at half size pushed .15 back, a block
/// item whole, each 1/64 larger). The pose the item's `FIXED` model is drawn with, in the block.
pub fn slot_pose(facing: Dir, first: bool, block_item: bool) -> glam::Mat4 {
    use glam::{Mat4, Vec3};
    let o = slot_offset(facing, first);
    let y_rot: f32 = match facing {
        Dir::Up | Dir::Down => 0.0,
        Dir::South => 180.0,
        Dir::West => 90.0,
        Dir::North => 360.0,
        Dir::East => -90.0,
    };
    let x_rot: f32 = match facing {
        Dir::Up => 90.0,
        Dir::Down => 270.0,
        _ => 0.0,
    };
    let scale = if block_item { 1.0 } else { 0.5 } + 1.0 / 64.0;
    let z = if block_item { 0.0 } else { -0.15 };
    Mat4::from_translation(Vec3::new(o[0] as f32, o[1] as f32, o[2] as f32))
        * Mat4::from_rotation_y(y_rot.to_radians())
        * Mat4::from_rotation_x(x_rot.to_radians())
        * Mat4::from_scale(Vec3::splat(0.4975 * scale))
        * Mat4::from_translation(Vec3::new(0.0, 0.0, z))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(first: &str, receiver: bool) -> Link {
        Link {
            key: (first.into(), String::new()),
            receiver,
            ..Link::default()
        }
    }

    #[test]
    fn a_transmitter_powers_receivers_on_its_frequency_within_range() {
        let mut l = Links::default();
        assert!(l.add([0, 0, 0], link("minecraft:dirt", false)).is_empty());
        l.add([10, 0, 0], link("minecraft:dirt", true));
        l.add([0, 0, 300], link("minecraft:dirt", true));
        l.add([5, 0, 0], link("minecraft:stone", true));
        assert_eq!(l.transmit([0, 0, 0], 9), vec![[10, 0, 0]]);
        assert_eq!(l.received([10, 0, 0]), 9);
        assert_eq!(l.received([0, 0, 300]), 0, "out of range");
        assert_eq!(l.received([5, 0, 0]), 0, "another frequency");
        // Retuned to dirt, it hears the transmitter.
        assert_eq!(
            l.set_frequency([5, 0, 0], true, "minecraft:dirt"),
            vec![[5, 0, 0]]
        );
        assert_eq!(l.received([5, 0, 0]), 9);
        // The transmitter gone, its receivers go dark.
        let mut gone = l.remove([0, 0, 0]);
        gone.sort();
        assert_eq!(gone, vec![[5, 0, 0], [10, 0, 0]]);
        assert_eq!(l.received([10, 0, 0]), 0);
    }

    #[test]
    fn the_strongest_transmitter_wins_and_a_toggled_link_changes_sides() {
        let mut l = Links::default();
        l.add([0, 0, 0], link("a", false));
        l.add([1, 0, 0], link("a", false));
        l.add([2, 0, 0], link("a", true));
        l.transmit([0, 0, 0], 4);
        l.transmit([1, 0, 0], 12);
        assert_eq!(l.received([2, 0, 0]), 12);
        l.transmit([1, 0, 0], 0);
        assert_eq!(l.received([2, 0, 0]), 4);
        // The 4 transmitter becomes a receiver: nothing transmits.
        let changed = l.set_receiver([0, 0, 0], true);
        assert_eq!(changed, vec![[2, 0, 0]]);
        assert_eq!((l.received([0, 0, 0]), l.received([2, 0, 0])), (0, 0));
    }

    #[test]
    fn signals_and_slots_sit_where_create_puts_them() {
        // A receiver facing north (on the block south of it) gives its signal to every side, and
        // its direct signal only into that block.
        assert_eq!(signal(true, Dir::North, 7, Dir::East, false), 7);
        assert_eq!(signal(true, Dir::North, 7, Dir::North, true), 7);
        assert_eq!(signal(true, Dir::North, 7, Dir::South, true), 0);
        assert_eq!(signal(false, Dir::North, 7, Dir::North, false), 0);
        // Facing south: slots at z 3.01/16, the first 5 px above the second.
        let a = slot_offset(Dir::South, true);
        let b = slot_offset(Dir::South, false);
        assert!((a[1] - 10.5 / 16.0).abs() < 1e-9 && (b[1] - 5.5 / 16.0).abs() < 1e-9);
        assert!((a[2] - 3.01 / 16.0).abs() < 1e-9 && (a[0] - 0.5).abs() < 1e-9);
        // Facing north they are on the other side; facing up the first is further south.
        assert!((slot_offset(Dir::North, true)[2] - (1.0 - 3.01 / 16.0)).abs() < 1e-9);
        // On the block west of it (facing east) the tablet is at low x, and the slots with it.
        assert!((slot_offset(Dir::East, true)[0] - 3.01 / 16.0).abs() < 1e-9);
        assert!((slot_offset(Dir::West, true)[0] - (1.0 - 3.01 / 16.0)).abs() < 1e-9);
        let up = slot_offset(Dir::Up, true);
        assert!((up[1] - 3.01 / 16.0).abs() < 1e-9 && (up[2] - 10.5 / 16.0).abs() < 1e-9);
        assert_eq!(hit_slot(Dir::South, a), Some(true));
        assert_eq!(hit_slot(Dir::South, b), Some(false));
        assert_eq!(hit_slot(Dir::South, [0.5, 0.5, 0.9]), None);
    }

    #[test]
    fn frequencies_round_trip_and_items_face_out_of_the_tablet() {
        let shown = vec![
            (
                [1, -2, 3],
                Dir::East,
                ("minecraft:dirt".into(), String::new()),
            ),
            ([4, 5, 6], Dir::Up, (String::new(), "create:wrench".into())),
        ];
        assert_eq!(decode(&encode(&shown)), shown);
        // An item's flat side (z in its model) lies along the tablet's facing.
        let front = |f: Dir| {
            slot_pose(f, true, false)
                .transform_vector3(glam::Vec3::Z)
                .normalize()
        };
        let s = front(Dir::South);
        let u = front(Dir::Up);
        assert!(s.z.abs() > 0.99 && u.y.abs() > 0.99, "{s} {u}");
    }
}
