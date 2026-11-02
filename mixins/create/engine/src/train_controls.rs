//! Riding and driving Create's trains: a carriage's blocks in the world as it is posed
//! (`AbstractContraptionEntity.toGlobalVector`, `toLocalVector`), the click that finds one of
//! them (`ContraptionHandlerClient.rayTraceContraption`), where a carriage's seats put their
//! riders (`getPassengerPosition`), and its train controls (`ControlsHandler`,
//! `ControlsServerHandler`, `ControlsInputPacket`, `ControlsStopControllingPacket`,
//! `ControlsInteractionBehaviour`): the client sends the six keys it holds while a player
//! controls a carriage, the server keeps each pressed for 30 ticks after the last word and
//! drives the train by them (`CarriageContraptionEntity.control`, in `trains::Railway::control`).

use crate::bezier::Vec3;
use crate::controller::{KEYS, buttons};

/// The mod message channel, both ways.
pub const CHANNEL: &str = "create:train_controls";
/// `ControlsHandler.PACKET_RATE`: ticks between keep-alive packets.
pub const PACKET_RATE: u8 = 5;
/// `ControlsServerHandler.TIMEOUT`: ticks a pressed key lasts without word.
pub const TIMEOUT: u8 = 30;
/// `CarriageContraptionEntity.control`: `canInteractWithBlock(player, controls, 8)`.
pub const CONTROL_RANGE: f64 = 8.0;
/// `ControlsInputPacket.handle`: `canInteractWithBlock(player, controlsPos, 16)`.
pub const INPUT_RANGE: f64 = 16.0;
/// The keys (`ControlsUtil.getControls` indices): forward, back, left, right, jump, sneak.
pub const FORWARD: u8 = 0;
pub const BACK: u8 = 1;
pub const LEFT: u8 = 2;
pub const RIGHT: u8 = 3;
pub const JUMP: u8 = 4;
pub const SNEAK: u8 = 5;
/// A player's vehicle attachment (`Player`'s `EntityAttachment.VEHICLE`).
pub const PLAYER_ATTACHMENT: f64 = 0.6;

/// The key a control is (`getControls().indexOf`), None for any other key.
pub fn control(key: &str) -> Option<u8> {
    crate::controller::button(key)
}

/// A carriage as posed now: its pivot (its leading bogey's anchor, on the track), its yaw from
/// the way the train faced when assembled (counterclockwise looking down) and its pitch (its
/// front climbing), turning its blocks as clients draw them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub pivot: Vec3,
    pub yaw: f64,
    pub pitch: f64,
    pub heading: [i32; 2],
}

impl Frame {
    /// The frame of a carriage whose pivot is `pivot` and front `front`, of a train assembled
    /// facing `heading`.
    pub fn new(pivot: Vec3, front: Vec3, heading: [i32; 2]) -> Frame {
        let (yaw, pitch) = crate::trains::yaw_pitch(front, heading);
        Frame {
            pivot,
            yaw,
            pitch,
            heading,
        }
    }

    /// Turned about up by the yaw (as `contraption::rotate` turns quarters).
    fn turn(&self, v: Vec3, sign: f64) -> Vec3 {
        let (s, c) = (sign * self.yaw).to_radians().sin_cos();
        [v[0] * c + v[2] * s, v[1], v[2] * c - v[0] * s]
    }

    /// Tilted by the pitch about the train's side (`heading` × up), as clients tilt the model.
    fn tilt(&self, v: Vec3, sign: f64) -> Vec3 {
        let k = [-f64::from(self.heading[1]), 0.0, f64::from(self.heading[0])];
        let (s, c) = (sign * self.pitch).to_radians().sin_cos();
        let cross = [
            k[1] * v[2] - k[2] * v[1],
            k[2] * v[0] - k[0] * v[2],
            k[0] * v[1] - k[1] * v[0],
        ];
        let dot = k[0] * v[0] + k[1] * v[1] + k[2] * v[2];
        std::array::from_fn(|i| v[i] * c + cross[i] * s + k[i] * dot * (1.0 - c))
    }

    /// `toGlobalVector`: a point of the carriage (in its blocks' space, block `o` spanning
    /// `o` to `o + 1`, from its leading bogey's block) in the world. The bogey's block stands a
    /// block over the track, its centre over the pivot.
    pub fn to_world(&self, local: Vec3) -> Vec3 {
        let rel = [local[0] - 0.5, local[1] + 1.0, local[2] - 0.5];
        let w = self.turn(self.tilt(rel, 1.0), 1.0);
        std::array::from_fn(|i| self.pivot[i] + w[i])
    }

    /// `toLocalVector`: a point of the world in the carriage's blocks' space.
    pub fn to_local(&self, world: Vec3) -> Vec3 {
        let d = std::array::from_fn(|i| world[i] - self.pivot[i]);
        let rel = self.tilt(self.turn(d, -1.0), -1.0);
        [rel[0] + 0.5, rel[1] - 1.0, rel[2] + 0.5]
    }

    /// Where a seat of the carriage at block `o` holds its rider's vehicle attachment
    /// (`getPassengerPosition` with `positionRider`: the rider stands its attachment below the
    /// seat's bottom centre, their own seat offset and Create's eighth cancelling).
    pub fn seat(&self, o: [i32; 3]) -> Vec3 {
        self.to_world([
            f64::from(o[0]) + 0.5,
            f64::from(o[1]),
            f64::from(o[2]) + 0.5,
        ])
    }

    /// Where a player getting off the seat at block `o` is put (`removePassenger`'s
    /// `ContraptionDismountLocation`, `getPassengerPosition`): an eighth over their riding
    /// place.
    pub fn dismount(&self, o: [i32; 3]) -> Vec3 {
        self.to_world([
            f64::from(o[0]) + 0.5,
            f64::from(o[1]) + 0.125 - PLAYER_ATTACHMENT,
            f64::from(o[2]) + 0.5,
        ])
    }

    /// `canInteractWithBlock(player, localPos, distance)` (`Player.canInteractWithBlock`): the
    /// block of the world holding block `o`'s centre is within `reach + distance` of the eyes.
    pub fn within(&self, o: [i32; 3], eye: Vec3, reach: f64, distance: f64) -> bool {
        let c = self.to_world(o.map(|v| f64::from(v) + 0.5));
        let d2: f64 = (0..3)
            .map(|i| {
                let lo = c[i].floor();
                let d = (lo - eye[i]).max(eye[i] - (lo + 1.0)).max(0.0);
                d * d
            })
            .sum();
        let r = reach + distance;
        d2 < r * r
    }
}

/// `rayTraceContraption`: the block of a carriage (posed by `frame`; its blocks' offsets and
/// states) the segment from `origin` to `target` first hits, by each block's outline boxes
/// (`shape`, in block units), with how far along (0 to 1) it is hit.
pub fn ray_trace<'a>(
    frame: &Frame,
    blocks: impl IntoIterator<Item = &'a ([i32; 3], u32)>,
    origin: Vec3,
    target: Vec3,
    shape: impl Fn(u32) -> &'a [[f64; 6]],
) -> Option<([i32; 3], f64)> {
    let a = frame.to_local(origin);
    let b = frame.to_local(target);
    let d: Vec3 = std::array::from_fn(|i| b[i] - a[i]);
    let mut best: Option<([i32; 3], f64)> = None;
    for (o, state) in blocks {
        for bx in shape(*state) {
            let (mut lo, mut hi) = (0.0f64, 1.0f64);
            let mut hit = true;
            for i in 0..3 {
                let min = f64::from(o[i]) + bx[i];
                let max = f64::from(o[i]) + bx[i + 3];
                if d[i].abs() < 1.0e-12 {
                    if a[i] < min || a[i] > max {
                        hit = false;
                        break;
                    }
                    continue;
                }
                let (t0, t1) = ((min - a[i]) / d[i], (max - a[i]) / d[i]);
                lo = lo.max(t0.min(t1));
                hi = hi.min(t0.max(t1));
                if lo > hi {
                    hit = false;
                    break;
                }
            }
            if hit && best.is_none_or(|(_, t)| lo < t) {
                best = Some((*o, lo));
            }
        }
    }
    best
}

/// A carriage's block: its train, its carriage and the block's offset in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    pub train: u32,
    pub car: u16,
    pub local: [i32; 3],
}

fn put_target(out: &mut Vec<u8>, t: &Target) {
    out.extend_from_slice(&t.train.to_le_bytes());
    out.extend_from_slice(&t.car.to_le_bytes());
    for v in t.local {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn get_target(d: &[u8]) -> Option<Target> {
    let d = d.get(..18)?;
    let int = |i: usize| i32::from_le_bytes(d[i..i + 4].try_into().unwrap_or_default());
    Some(Target {
        train: u32::from_le_bytes(d[0..4].try_into().ok()?),
        car: u16::from_le_bytes(d[4..6].try_into().ok()?),
        local: [int(6), int(10), int(14)],
    })
}

/// What a client sends its server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToServer {
    /// `ContraptionInteractionPacket`: a use click on a carriage's block, the hand holding a
    /// wrench or not.
    Click { target: Target, wrench: bool },
    /// `ControlsInputPacket`: keys (bit per control) pressed or released, or the controls let
    /// go (`stopControlling`).
    Input {
        target: Target,
        keys: u8,
        press: bool,
        stop: bool,
    },
}

impl ToServer {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(21);
        match self {
            ToServer::Click { target, wrench } => {
                out.push(0);
                put_target(&mut out, target);
                out.push(u8::from(*wrench));
            }
            ToServer::Input {
                target,
                keys,
                press,
                stop,
            } => {
                out.push(1);
                put_target(&mut out, target);
                out.push(*keys);
                out.push(u8::from(*press) | (u8::from(*stop) << 1));
            }
        }
        out
    }

    pub fn decode(d: &[u8]) -> Option<ToServer> {
        let target = get_target(d.get(1..)?)?;
        match (d.first()?, d.len()) {
            (0, 20) => Some(ToServer::Click {
                target,
                wrench: d[19] != 0,
            }),
            (1, 21) => Some(ToServer::Input {
                target,
                keys: d[19] & 0x3F,
                press: d[20] & 1 != 0,
                stop: d[20] & 2 != 0,
            }),
            _ => None,
        }
    }
}

/// What a server tells a client: it controls a carriage's controls now
/// (`ControlsHandler.startControlling`), or no more (`ControlsStopControllingPacket`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToClient {
    Start(Target),
    Stop,
}

impl ToClient {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            ToClient::Start(t) => {
                let mut out = vec![0];
                put_target(&mut out, t);
                out
            }
            ToClient::Stop => vec![1],
        }
    }

    pub fn decode(d: &[u8]) -> Option<ToClient> {
        match (d.first()?, d.len()) {
            (0, 19) => Some(ToClient::Start(get_target(&d[1..])?)),
            (1, 1) => Some(ToClient::Stop),
            _ => None,
        }
    }
}

/// The client's hold on a carriage's controls (`ControlsHandler`): the controls, the keys it
/// held last tick, and the ticks to the next keep-alive.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Driver {
    pub controls: Option<Target>,
    pub pressed: u8,
    cooldown: u8,
}

impl Driver {
    /// `startControlling`.
    pub fn start(&mut self, target: Target) {
        *self = Driver {
            controls: Some(target),
            ..Driver::default()
        };
    }

    /// `stopControlling`: the keys still held are released to the server.
    pub fn stop(&mut self) -> Vec<ToServer> {
        let out = match self.controls {
            Some(target) if self.pressed != 0 => vec![ToServer::Input {
                target,
                keys: self.pressed,
                press: false,
                stop: false,
            }],
            _ => Vec::new(),
        };
        *self = Driver::default();
        out
    }

    /// `ControlsHandler.tick` with the keys held down now (bit per control) and whether escape
    /// is: escape lets the controls go; released keys go up, newly pressed ones down, and every
    /// `PACKET_RATE` ticks the held ones again (an empty word too, as Create's keep-alive).
    pub fn tick(&mut self, down: u8, escape: bool) -> Vec<ToServer> {
        let Some(target) = self.controls else {
            return Vec::new();
        };
        self.cooldown = self.cooldown.saturating_sub(1);
        if escape {
            let mut out = self.stop();
            out.push(ToServer::Input {
                target,
                keys: 0,
                press: false,
                stop: true,
            });
            return out;
        }
        let mut out = Vec::new();
        let released = self.pressed & !down;
        let new = down & !self.pressed;
        if released != 0 {
            out.push(ToServer::Input {
                target,
                keys: released,
                press: false,
                stop: false,
            });
        }
        if new != 0 {
            out.push(ToServer::Input {
                target,
                keys: new,
                press: true,
                stop: false,
            });
            self.cooldown = PACKET_RATE;
        }
        if self.cooldown == 0 {
            out.push(ToServer::Input {
                target,
                keys: down,
                press: true,
                stop: false,
            });
            self.cooldown = PACKET_RATE;
        }
        self.pressed = down;
        out
    }
}

/// A player's keys on a carriage's controls on the server (`ControlsServerHandler`'s
/// `ControlsContext`): the controls and each pressed key's ticks left.
#[derive(Clone, Debug, PartialEq)]
pub struct Context {
    pub controls: Target,
    pub keys: [u8; KEYS.len()],
}

impl Context {
    /// The keys still held (bit per control).
    pub fn held(&self) -> u8 {
        (0..KEYS.len())
            .filter(|&k| self.keys[k] > 0)
            .fold(0, |m, k| m | (1 << k))
    }
}

/// `ControlsServerHandler.receivedInputs`: every player's keys, by UUID.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    pub players: Vec<(u128, Context)>,
}

impl Inputs {
    pub fn is_empty(&self) -> bool {
        self.players.is_empty()
    }

    /// `receivePressed`: keys `keys` pressed (kept alive for `TIMEOUT` ticks) or released;
    /// controls of another carriage start over.
    pub fn receive(&mut self, player: u128, controls: Target, keys: u8, press: bool) {
        let at = self.players.iter().position(|(p, _)| *p == player);
        let at = match at {
            Some(i) if self.players[i].1.controls == controls => i,
            other => {
                if let Some(i) = other {
                    self.players.remove(i);
                }
                self.players.push((
                    player,
                    Context {
                        controls,
                        keys: [0; KEYS.len()],
                    },
                ));
                self.players.len() - 1
            }
        };
        let ctx = &mut self.players[at].1;
        for k in buttons(keys) {
            let k = usize::from(k);
            if press {
                ctx.keys[k] = TIMEOUT;
            } else {
                ctx.keys[k] = 0;
            }
        }
    }

    /// `ControlsServerHandler.tick`, first half: every key counts down (gone at 0).
    pub fn count_down(&mut self) {
        for (_, ctx) in &mut self.players {
            for k in &mut ctx.keys {
                *k = k.saturating_sub(1);
            }
        }
    }

    /// The second half: contexts with no key left go (after this tick's `control`).
    pub fn drop_released(&mut self) {
        self.players.retain(|(_, c)| c.held() != 0);
    }
}

/// What the train tells its driver (`TrainPromptPacket`, `displayClientMessage`): nothing, a
/// station's name as the train arrives or departs, the station ahead it may approach, the
/// progress of an approach (30 bars, `progress` of them still to go), or the approach prompt
/// taken away.
#[derive(Clone, Debug, PartialEq)]
pub enum Prompt {
    None,
    Arrived(String),
    Departing(String),
    Approach(String),
    Progress(u8),
    Clear,
}

/// What `control` keeps per carriage between ticks: whether the approach prompt is shown
/// (`stationMessage`) and how far the approached station was (`navDistanceTotal`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Driving {
    pub station_message: bool,
    pub nav_total: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1.0e-9)
    }

    #[test]
    fn carriage_blocks_sit_where_clients_draw_them() {
        // Level and as assembled: block (0, 0, 0) (the bogey's) is centred a block and a half
        // over the pivot; a block two ahead along z is two further.
        let f = Frame::new([10.5, 64.0, 3.5], [0.0, 0.0, 1.0], [0, 1]);
        assert!(close(f.to_world([0.5, 0.5, 0.5]), [10.5, 65.5, 3.5]));
        assert!(close(f.to_world([0.5, 0.5, 2.5]), [10.5, 65.5, 5.5]));
        // Turned a quarter: as `contraption::rotate` turns the blocks when it disassembles.
        let f = Frame::new([10.5, 64.0, 3.5], [1.0, 0.0, 0.0], [0, 1]);
        let r =
            crate::contraption::rotate([0, 0, 2], f.yaw.round() as i32, crate::kinetics::Axis::Y);
        let w = f.to_world([0.5, 0.5, 2.5]);
        assert!(close(
            w,
            [10.5 + f64::from(r[0]), 65.5, 3.5 + f64::from(r[2])]
        ));
        assert!(close(w, [12.5, 65.5, 3.5]));
        // Pitched up a slope: the front climbs.
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let f = Frame::new([0.5, 64.0, 0.5], [0.0, s, s], [0, 1]);
        assert!((f.pitch - 45.0).abs() < 1.0e-9);
        assert!(f.to_world([0.5, -1.0, 2.5])[1] > 64.0 + 1.0);
        // To the world and back.
        let f = Frame::new([3.25, 70.0, -8.5], [0.6, 0.3, -0.74], [-1, 0]);
        for p in [[0.5, 0.5, 0.5], [2.0, -1.0, 7.25], [-3.0, 4.0, 0.0]] {
            assert!(close(f.to_local(f.to_world(p)), p));
        }
    }

    #[test]
    fn seats_hold_riders_where_create_puts_them() {
        // A player on a carriage seat at (0, 0, 1) stands its attachment below the seat's
        // bottom centre; getting off, an eighth over that.
        let f = Frame::new([0.5, 64.0, 0.5], [0.0, 0.0, 1.0], [0, 1]);
        assert!(close(f.seat([0, 0, 1]), [0.5, 65.0, 1.5]));
        assert!(close(f.dismount([0, 0, 1]), [0.5, 65.0 + 0.125 - 0.6, 1.5]));
    }

    #[test]
    fn a_click_finds_the_nearest_block_by_its_shape() {
        let f = Frame::new([0.5, 64.0, 0.5], [0.0, 0.0, 1.0], [0, 1]);
        // A seat (half a block) at (0, 0, 1) and a full block at (0, 0, 3), looked at along z
        // at the seat's middle height and above it.
        let blocks = [([0, 0, 1], 7u32), ([0, 0, 3], 1u32)];
        let half = [[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]];
        let full = [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
        let shape = |s: u32| -> &[[f64; 6]] { if s == 7 { &half } else { &full } };
        let low = f.to_world([0.5, 0.25, -1.0]);
        let far = f.to_world([0.5, 0.25, 5.0]);
        let (o, t) = ray_trace(&f, &blocks, low, far, shape).unwrap();
        assert_eq!(o, [0, 0, 1]);
        assert!((t - 2.0 / 6.0).abs() < 1.0e-9);
        let high = f.to_world([0.5, 0.75, -1.0]);
        let far = f.to_world([0.5, 0.75, 5.0]);
        assert_eq!(
            ray_trace(&f, &blocks, high, far, shape).unwrap().0,
            [0, 0, 3]
        );
        // Short of it: nothing.
        let near = f.to_world([0.5, 0.25, 0.5]);
        assert!(ray_trace(&f, &blocks, low, near, shape).is_none());
    }

    #[test]
    fn the_controls_reach_eight_past_the_player_s_own() {
        let f = Frame::new([0.5, 64.0, 0.5], [0.0, 0.0, 1.0], [0, 1]);
        // The controls at (0, 0, 0): its world block is (0, 65, 0).
        assert!(f.within([0, 0, 0], [0.5, 66.6, 12.9], 4.5, CONTROL_RANGE));
        assert!(!f.within([0, 0, 0], [0.5, 66.6, 13.6], 4.5, CONTROL_RANGE));
    }

    #[test]
    fn messages_round_trip() {
        let target = Target {
            train: 3,
            car: 1,
            local: [-2, 1, 5],
        };
        for m in [
            ToServer::Click {
                target,
                wrench: true,
            },
            ToServer::Input {
                target,
                keys: 0b100101,
                press: true,
                stop: false,
            },
            ToServer::Input {
                target,
                keys: 0,
                press: false,
                stop: true,
            },
        ] {
            assert_eq!(ToServer::decode(&m.encode()), Some(m));
        }
        for m in [ToClient::Start(target), ToClient::Stop] {
            assert_eq!(ToClient::decode(&m.encode()), Some(m));
        }
        assert_eq!(ToServer::decode(&[1, 2, 3]), None);
        assert_eq!(control("key.keyboard.space"), Some(JUMP));
        assert_eq!(control("key.keyboard.left.shift"), Some(SNEAK));
    }

    #[test]
    fn the_client_sends_its_keys_as_create_does() {
        let target = Target {
            train: 1,
            car: 0,
            local: [0, 1, 0],
        };
        let mut d = Driver::default();
        assert!(d.tick(1, false).is_empty(), "nothing while not controlling");
        d.start(target);
        // W goes down: the new key, then nothing for 5 ticks, then the keep-alive.
        let input = |keys, press| ToServer::Input {
            target,
            keys,
            press,
            stop: false,
        };
        assert_eq!(d.tick(1, false), [input(1, true)]);
        for _ in 0..4 {
            assert!(d.tick(1, false).is_empty());
        }
        assert_eq!(d.tick(1, false), [input(1, true)]);
        // W up, A down in one tick.
        assert_eq!(d.tick(4, false), [input(1, false), input(4, true)]);
        // Escape: the held key goes up and the controls are let go.
        let out = d.tick(4, true);
        assert_eq!(
            out,
            [
                input(4, false),
                ToServer::Input {
                    target,
                    keys: 0,
                    press: false,
                    stop: true
                }
            ]
        );
        assert_eq!(d.controls, None);
    }

    #[test]
    fn the_server_keeps_keys_for_thirty_ticks() {
        let target = Target {
            train: 1,
            car: 0,
            local: [0, 1, 0],
        };
        let mut s = Inputs::default();
        s.receive(7, target, 1 << FORWARD, true);
        for _ in 0..29 {
            s.count_down();
            assert_eq!(s.players[0].1.held(), 1);
            s.drop_released();
        }
        s.count_down();
        assert_eq!(s.players[0].1.held(), 0);
        s.drop_released();
        assert!(s.is_empty());
        // Released by word; another carriage's controls start over.
        s.receive(7, target, 0b11, true);
        s.receive(7, target, 1, false);
        assert_eq!(s.players[0].1.held(), 2);
        let other = Target { car: 1, ..target };
        s.receive(7, other, 1 << JUMP, true);
        assert_eq!(s.players.len(), 1);
        assert_eq!(s.players[0].1.controls, other);
        assert_eq!(s.players[0].1.held(), 1 << JUMP);
    }
}
