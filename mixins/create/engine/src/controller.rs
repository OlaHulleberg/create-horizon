//! Create's linked controller (`LinkedControllerItem`, `LinkedControllerClientHandler`,
//! `LinkedControllerServerHandler`, `LinkedControllerInputPacket`, `LinkedControllerBindPacket`,
//! `LinkedControllerStopLecternPacket`) and the lectern holding one (`LecternControllerBlock`,
//! `LecternControllerBlockEntity`). Six buttons, each bound to a redstone link frequency (two
//! items) kept on the controller; while active, held buttons transmit on their frequencies from
//! where the player stands, renewed every few ticks and dropped 30 ticks after the last word.

use crate::link::{Key, Links};

/// `ControlsUtil.getControls()`: up, down, left, right, jump and sneak, by their default keys
/// (W, S, A, D, space, left shift).
pub const KEYS: [&str; 6] = [
    "key.keyboard.w",
    "key.keyboard.s",
    "key.keyboard.a",
    "key.keyboard.d",
    "key.keyboard.space",
    "key.keyboard.left.shift",
];

/// `LinkedControllerServerHandler.TIMEOUT`: ticks a pressed frequency lasts without word.
pub const TIMEOUT: i32 = 30;
/// `LinkedControllerClientHandler.PACKET_RATE`: ticks between keep-alive packets.
pub const PACKET_RATE: u8 = 5;
/// The frequency items a controller holds (`getFrequencyItems`: two per button).
pub const SLOTS: usize = 12;
/// The mod message channel, both ways.
pub const CHANNEL: &str = "create:controller";
/// The stack's custom data key for its frequency items (`LINKED_CONTROLLER_ITEMS`).
pub const ITEMS_KEY: &str = "create:linked_controller_items";

/// The button a key is (`getControls().indexOf`), None for any other key.
pub fn button(key: &str) -> Option<u8> {
    KEYS.iter().position(|k| *k == key).map(|i| i as u8)
}

/// `LinkedControllerItem.toFrequency`: button `b`'s frequency from the controller's 12 items
/// ("" for an empty slot).
pub fn frequency(items: &[String], b: u8) -> Key {
    let at = |i: usize| items.get(i).cloned().unwrap_or_default();
    (at(usize::from(b) * 2), at(usize::from(b) * 2 + 1))
}

/// `LinkedControllerBindPacket.handleItem`: button `b` takes the link's frequency.
pub fn bind(items: &mut Vec<String>, b: u8, key: &Key) {
    items.resize(SLOTS, String::new());
    let i = usize::from(b) * 2;
    if i + 1 < SLOTS {
        items[i] = key.0.clone();
        items[i + 1] = key.1.clone();
    }
}

/// Buttons as a set (bit `b` for button `b`).
pub fn buttons(mask: u8) -> impl Iterator<Item = u8> {
    (0..KEYS.len() as u8).filter(move |b| mask & (1 << b) != 0)
}

/// What a client sends its server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToServer {
    /// `LinkedControllerInputPacket`: buttons pressed (or released), from a lectern or the hand.
    Input {
        buttons: u8,
        press: bool,
        lectern: Option<[i32; 3]>,
    },
    /// `LinkedControllerBindPacket`: the button bound to the link at `link`.
    Bind { button: u8, link: [i32; 3] },
    /// `LinkedControllerStopLecternPacket`.
    StopLectern([i32; 3]),
}

/// What a server tells a client: the controller's use toggles it (`LinkedControllerItem.use`,
/// on the client in Create), a click on a link starts binding, a lectern's user changed
/// (`LecternControllerBlockEntity.tryToggleActive`, from its synced `user`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToClient {
    Toggle,
    BindMode([i32; 3]),
    LecternOn([i32; 3]),
    LecternOff([i32; 3]),
}

fn put_pos(out: &mut Vec<u8>, p: [i32; 3]) {
    for v in p {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn pos_at(d: &[u8], at: usize) -> Option<[i32; 3]> {
    let b = d.get(at..at + 12)?;
    let i = |k: usize| i32::from_le_bytes([b[k], b[k + 1], b[k + 2], b[k + 3]]);
    Some([i(0), i(4), i(8)])
}

impl ToServer {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16);
        match *self {
            ToServer::Input {
                buttons,
                press,
                lectern,
            } => {
                out.extend_from_slice(&[0, buttons, u8::from(press), u8::from(lectern.is_some())]);
                if let Some(p) = lectern {
                    put_pos(&mut out, p);
                }
            }
            ToServer::Bind { button, link } => {
                out.extend_from_slice(&[1, button]);
                put_pos(&mut out, link);
            }
            ToServer::StopLectern(p) => {
                out.push(2);
                put_pos(&mut out, p);
            }
        }
        out
    }

    /// None for anything malformed (the player's own bytes).
    pub fn decode(d: &[u8]) -> Option<ToServer> {
        let all = (1u8 << KEYS.len()) - 1;
        match *d.first()? {
            0 => {
                let (buttons, press, has) = (*d.get(1)?, *d.get(2)? != 0, *d.get(3)? != 0);
                let lectern = if has { Some(pos_at(d, 4)?) } else { None };
                Some(ToServer::Input {
                    buttons: buttons & all,
                    press,
                    lectern,
                })
            }
            1 => {
                let button = *d.get(1)?;
                (usize::from(button) < KEYS.len()).then_some(())?;
                Some(ToServer::Bind {
                    button,
                    link: pos_at(d, 2)?,
                })
            }
            2 => Some(ToServer::StopLectern(pos_at(d, 1)?)),
            _ => None,
        }
    }
}

impl ToClient {
    pub fn encode(&self) -> Vec<u8> {
        let (kind, pos) = match *self {
            ToClient::Toggle => return vec![0],
            ToClient::BindMode(p) => (1, p),
            ToClient::LecternOn(p) => (2, p),
            ToClient::LecternOff(p) => (3, p),
        };
        let mut out = vec![kind];
        put_pos(&mut out, pos);
        out
    }

    pub fn decode(d: &[u8]) -> Option<ToClient> {
        match *d.first()? {
            0 => Some(ToClient::Toggle),
            1 => Some(ToClient::BindMode(pos_at(d, 1)?)),
            2 => Some(ToClient::LecternOn(pos_at(d, 1)?)),
            3 => Some(ToClient::LecternOff(pos_at(d, 1)?)),
            _ => None,
        }
    }
}

/// `LinkedControllerClientHandler.Mode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Idle,
    Active,
    /// Binding the link at the position.
    Bind([i32; 3]),
}

/// The client's controller (`LinkedControllerClientHandler`'s statics).
#[derive(Clone, Debug, Default)]
pub struct Client {
    pub mode: Mode,
    /// `lecternPos`: the lectern used, None for the controller in hand.
    pub lectern: Option<[i32; 3]>,
    /// `currentlyPressed`.
    pub pressed: u8,
    cooldown: u8,
}

impl Client {
    /// Whether the key hook should look at keys (any mode but idle).
    pub fn active(&self) -> bool {
        self.mode != Mode::Idle
    }

    /// `toggle`: idle to active (the controller in hand), anything else to idle.
    pub fn toggle(&mut self) -> Vec<ToServer> {
        if self.mode == Mode::Idle {
            self.mode = Mode::Active;
            self.lectern = None;
            Vec::new()
        } else {
            self.mode = Mode::Idle;
            self.reset()
        }
    }

    /// `toggleBindMode`.
    pub fn toggle_bind(&mut self, link: [i32; 3]) -> Vec<ToServer> {
        if self.mode == Mode::Idle {
            self.mode = Mode::Bind(link);
            Vec::new()
        } else {
            self.mode = Mode::Idle;
            self.reset()
        }
    }

    /// `activateInLectern`.
    pub fn activate_in_lectern(&mut self, at: [i32; 3]) {
        if self.mode == Mode::Idle {
            self.mode = Mode::Active;
            self.lectern = Some(at);
        }
    }

    /// `deactivateInLectern`.
    pub fn deactivate_in_lectern(&mut self) -> Vec<ToServer> {
        if self.mode == Mode::Active && self.lectern.is_some() {
            self.mode = Mode::Idle;
            return self.reset();
        }
        Vec::new()
    }

    /// Back to idle (`MODE = IDLE; onReset()`).
    pub fn stop(&mut self) -> Vec<ToServer> {
        self.mode = Mode::Idle;
        self.reset()
    }

    /// `onReset`: a lectern in use is left, held buttons are released.
    fn reset(&mut self) -> Vec<ToServer> {
        let mut out = Vec::new();
        self.cooldown = 0;
        if let Some(at) = self.lectern.take() {
            out.push(ToServer::StopLectern(at));
        }
        if self.pressed != 0 {
            out.push(ToServer::Input {
                buttons: self.pressed,
                press: false,
                lectern: None,
            });
        }
        self.pressed = 0;
        out
    }

    /// `tick` once the hand, lectern and screen checks passed: `held` are the buttons whose keys
    /// are down now (`ControlsUtil.isActuallyPressed`).
    /// The packets to send, and whether any button went down or up (Create's click sound, with
    /// its pitch: .75 for a press, .5 for a release).
    pub fn tick(&mut self, held: u8) -> (Vec<ToServer>, Option<f32>) {
        let mut out = Vec::new();
        let mut click = None;
        if self.mode == Mode::Idle {
            return (out, click);
        }
        self.cooldown = self.cooldown.saturating_sub(1);
        let new = held & !self.pressed;
        let released = self.pressed & !held;
        match self.mode {
            Mode::Active => {
                if released != 0 {
                    out.push(ToServer::Input {
                        buttons: released,
                        press: false,
                        lectern: self.lectern,
                    });
                    click = Some(0.5);
                }
                if new != 0 {
                    out.push(ToServer::Input {
                        buttons: new,
                        press: true,
                        lectern: self.lectern,
                    });
                    self.cooldown = PACKET_RATE;
                    click = Some(0.75);
                }
                if self.cooldown == 0 && held != 0 {
                    out.push(ToServer::Input {
                        buttons: held,
                        press: true,
                        lectern: self.lectern,
                    });
                    self.cooldown = PACKET_RATE;
                }
            }
            Mode::Bind(link) => {
                if let Some(b) = buttons(new).next() {
                    out.push(ToServer::Bind { button: b, link });
                    self.mode = Mode::Idle;
                }
            }
            Mode::Idle => {}
        }
        self.pressed = held;
        (out, click)
    }
}

/// `ManualFrequencyEntry`: a frequency a player holds down, as a transmitter with no block.
#[derive(Clone, Debug)]
pub struct Held {
    pub player: u128,
    pub key: Key,
    pub id: u64,
    /// Ticks left (`getFirst`); alive while above 0.
    pub timeout: i32,
}

/// `LinkedControllerServerHandler.receivedInputs` of a level.
#[derive(Clone, Debug, Default)]
pub struct Server {
    pub held: Vec<Held>,
    next: u64,
}

impl Server {
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// `receivePressed`: the player's buttons' frequencies `keys`, pressed (renewed at `pos`, or
    /// joining the network there) or released (dead at the next tick). The receivers whose
    /// signal changed.
    pub fn receive(
        &mut self,
        links: &mut Links,
        player: u128,
        pos: [i32; 3],
        keys: &[Key],
        pressed: bool,
    ) -> Vec<[i32; 3]> {
        let mut changed = Vec::new();
        for key in keys {
            if let Some(h) = self
                .held
                .iter_mut()
                .find(|h| h.player == player && h.key == *key)
            {
                if pressed {
                    h.timeout = TIMEOUT;
                    links.move_virtual(h.id, pos);
                    links.set_virtual_strength(h.id, 15);
                } else {
                    h.timeout = 0;
                    links.set_virtual_strength(h.id, 0);
                }
                continue;
            }
            if !pressed {
                continue;
            }
            self.next += 1;
            let id = self.next;
            self.held.push(Held {
                player,
                key: key.clone(),
                id,
                timeout: TIMEOUT,
            });
            changed.extend(links.add_virtual(id, pos, key.clone()));
        }
        changed
    }

    /// `tick`: each entry counts down; dead ones leave their network. The receivers whose
    /// signal changed.
    pub fn tick(&mut self, links: &mut Links) -> Vec<[i32; 3]> {
        let mut changed = Vec::new();
        self.held.retain_mut(|h| {
            h.timeout -= 1;
            if h.timeout > 0 {
                return true;
            }
            changed.extend(links.remove_virtual(h.id));
            false
        });
        changed
    }
}

/// `LecternControllerBlockEntity.playerInRange`: the eyes within 0.4 of the block interaction
/// range (4.5) of the block's centre.
pub fn in_lectern_range(eyes: [f64; 3], pos: [i32; 3]) -> bool {
    let reach = 0.4 * 4.5;
    let d: f64 = (0..3)
        .map(|i| (eyes[i] - (f64::from(pos[i]) + 0.5)).powi(2))
        .sum();
    d < reach * reach
}

/// A lectern holding a controller (`LecternControllerBlockEntity`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lectern {
    /// The controller's frequency items (`controllerData`).
    pub items: Vec<String>,
    /// `user`.
    pub user: Option<u128>,
    /// The tick its user last left (`deactivatedThisTick`).
    pub stopped: Option<i64>,
}

impl Lectern {
    /// `tryStartUsing`: not the tick its user left, no user, the player using no other
    /// lectern, in range.
    pub fn try_start(&mut self, player: u128, busy: bool, in_range: bool, now: i64) -> bool {
        if self.stopped == Some(now) || self.user.is_some() || busy || !in_range {
            return false;
        }
        self.user = Some(player);
        true
    }

    /// `stopUsing`.
    pub fn stop(&mut self, now: i64) -> Option<u128> {
        let user = self.user.take()?;
        self.stopped = Some(now);
        Some(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::Link;

    fn key(a: &str) -> Key {
        (a.to_owned(), String::new())
    }

    #[test]
    fn buttons_map_to_their_frequency_pairs() {
        assert_eq!(button("key.keyboard.w"), Some(0));
        assert_eq!(button("key.keyboard.space"), Some(4));
        assert_eq!(button("key.keyboard.left.shift"), Some(5));
        assert_eq!(button("key.keyboard.e"), None);
        let mut items = Vec::new();
        bind(
            &mut items,
            2,
            &("minecraft:red_dye".into(), "minecraft:stone".into()),
        );
        assert_eq!(items.len(), SLOTS);
        assert_eq!(
            frequency(&items, 2),
            ("minecraft:red_dye".into(), "minecraft:stone".into())
        );
        assert_eq!(frequency(&items, 0), (String::new(), String::new()));
        assert_eq!(buttons(0b100101).collect::<Vec<_>>(), [0, 2, 5]);
    }

    #[test]
    fn packets_round_trip() {
        for p in [
            ToServer::Input {
                buttons: 0b11,
                press: true,
                lectern: None,
            },
            ToServer::Input {
                buttons: 1,
                press: false,
                lectern: Some([1, -2, 3]),
            },
            ToServer::Bind {
                button: 5,
                link: [7, 8, -9],
            },
            ToServer::StopLectern([4, 5, 6]),
        ] {
            assert_eq!(ToServer::decode(&p.encode()), Some(p));
        }
        assert_eq!(
            ToServer::decode(&[1, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            None
        );
        assert_eq!(ToServer::decode(&[0, 1]), None);
        for m in [
            ToClient::Toggle,
            ToClient::BindMode([1, 2, 3]),
            ToClient::LecternOn([-1, 0, 1]),
            ToClient::LecternOff([5, 5, 5]),
        ] {
            assert_eq!(ToClient::decode(&m.encode()), Some(m));
        }
    }

    #[test]
    fn client_sends_presses_keepalives_and_releases() {
        let mut c = Client::default();
        assert!(c.toggle().is_empty());
        assert!(c.active());
        let (out, click) = c.tick(1);
        assert_eq!(
            out,
            [ToServer::Input {
                buttons: 1,
                press: true,
                lectern: None
            }]
        );
        assert_eq!(click, Some(0.75));
        // Held: nothing for four ticks, then a keep-alive.
        for _ in 0..4 {
            assert!(c.tick(1).0.is_empty());
        }
        assert_eq!(c.tick(1).0.len(), 1);
        let (out, click) = c.tick(0);
        assert_eq!(
            out,
            [ToServer::Input {
                buttons: 1,
                press: false,
                lectern: None
            }]
        );
        assert_eq!(click, Some(0.5));
        // Toggled off while space is held: released.
        c.tick(1 << 4);
        assert_eq!(
            c.toggle(),
            [ToServer::Input {
                buttons: 1 << 4,
                press: false,
                lectern: None
            }]
        );
        assert!(!c.active());
    }

    #[test]
    fn client_binds_the_first_pressed_button_and_leaves_lecterns() {
        let mut c = Client::default();
        c.toggle_bind([1, 2, 3]);
        assert_eq!(
            c.tick(1 << 3).0,
            [ToServer::Bind {
                button: 3,
                link: [1, 2, 3]
            }]
        );
        assert_eq!(c.mode, Mode::Idle);
        let mut c = Client::default();
        c.activate_in_lectern([9, 9, 9]);
        assert_eq!(
            c.tick(1 << 2).0,
            [ToServer::Input {
                buttons: 1 << 2,
                press: true,
                lectern: Some([9, 9, 9])
            }]
        );
        assert_eq!(
            c.deactivate_in_lectern(),
            [
                ToServer::StopLectern([9, 9, 9]),
                ToServer::Input {
                    buttons: 1 << 2,
                    press: false,
                    lectern: None
                }
            ]
        );
    }

    #[test]
    fn held_frequencies_power_receivers_until_released_or_timed_out() {
        let mut links = Links::default();
        let rx = [10, 0, 0];
        links.add(
            rx,
            Link {
                key: key("minecraft:red_dye"),
                receiver: true,
                ..Link::default()
            },
        );
        let mut s = Server::default();
        let changed = s.receive(&mut links, 7, [0, 0, 0], &[key("minecraft:red_dye")], true);
        assert_eq!(changed, [rx]);
        assert_eq!(links.received(rx), 15);
        // Renewed by keep-alives: still on after twice the timeout.
        for _ in 0..TIMEOUT * 2 {
            if s.held[0].timeout < 5 {
                s.receive(&mut links, 7, [1, 0, 0], &[key("minecraft:red_dye")], true);
            }
            assert!(s.tick(&mut links).is_empty());
        }
        assert_eq!(links.received(rx), 15);
        // Released: off at the next tick.
        s.receive(&mut links, 7, [1, 0, 0], &[key("minecraft:red_dye")], false);
        assert_eq!(links.received(rx), 15);
        assert_eq!(s.tick(&mut links), [rx]);
        assert_eq!(links.received(rx), 0);
        assert!(s.is_empty());
        // Pressed and never renewed: off after the timeout.
        s.receive(&mut links, 7, [0, 0, 0], &[key("minecraft:red_dye")], true);
        assert_eq!(links.received(rx), 15);
        for i in 1..TIMEOUT {
            assert!(s.tick(&mut links).is_empty(), "tick {i}");
        }
        assert_eq!(s.tick(&mut links), [rx]);
        assert_eq!(links.received(rx), 0);
        // Out of range, or another frequency: nothing.
        s.receive(
            &mut links,
            7,
            [300, 0, 0],
            &[key("minecraft:red_dye")],
            true,
        );
        s.receive(&mut links, 7, [0, 0, 0], &[key("minecraft:stone")], true);
        assert_eq!(links.received(rx), 0);
    }

    #[test]
    fn a_lectern_has_one_user_at_a_time() {
        let mut l = Lectern::default();
        assert!(!l.try_start(1, false, false, 0), "out of range");
        assert!(!l.try_start(1, true, true, 0), "using another lectern");
        assert!(l.try_start(1, false, true, 0));
        assert!(!l.try_start(2, false, true, 0));
        assert_eq!(l.stop(5), Some(1));
        assert!(!l.try_start(2, false, true, 5), "not the tick it was left");
        assert!(l.try_start(2, false, true, 6));
        assert!(in_lectern_range([0.5, 1.6, 0.5], [0, 0, 0]));
        assert!(!in_lectern_range([0.5, 1.6, 3.0], [0, 0, 0]));
    }
}
