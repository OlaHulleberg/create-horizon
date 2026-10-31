//! Create's mechanical press (`PressingBehaviour`, `BeltPressingCallbacks`,
//! `MechanicalPressBlockEntity`): a 240-tick cycle whose middle presses what lies on the depot
//! two blocks below. A press runs only while it presses; a depot under it asks it only when its
//! stack arrives, changes, or the press finishes or starts turning.

use crate::depot::Held;

/// `PressingBehaviour.CYCLE`.
pub const CYCLE: i32 = 240;

/// `PressingBehaviour.Mode`: where it presses, and how far its head goes down (blocks).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    World,
    Belt,
    Basin,
}

impl Mode {
    pub fn head_offset(self) -> f32 {
        match self {
            Mode::World => 1.0,
            Mode::Belt => 19.0 / 16.0,
            Mode::Basin => 22.0 / 16.0,
        }
    }
    pub fn index(self) -> u8 {
        self as u8
    }
    pub fn from_index(i: u8) -> Mode {
        match i {
            1 => Mode::Belt,
            2 => Mode::Basin,
            _ => Mode::World,
        }
    }
}

/// The press's state: `running`, `runningTicks`, `mode`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Press {
    pub running: bool,
    pub ticks: i32,
    pub mode: Mode,
}

/// What a tick of a running press did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    /// Nothing to tell.
    Moving,
    /// The head is down (`runningTicks == CYCLE / 2`): what lies below is pressed now.
    Down,
    /// The cycle ended (`onPressingCompleted`).
    Done,
}

/// `PressingBehaviour.getRunningTickSpeed`: 1 to 60 cycle ticks a game tick, by speed.
pub fn tick_speed(speed: f32) -> i32 {
    if speed == 0.0 {
        return 0;
    }
    let t = (speed.abs() / 512.0).clamp(0.0, 1.0);
    (1.0 + t * 59.0) as i32
}

/// `PressingBehaviour.getRenderedHeadOffset` as a fraction of the mode's depth.
pub fn head_offset(ticks: f32) -> f32 {
    let ticks = ticks.abs();
    if ticks < (CYCLE * 2 / 3) as f32 {
        return ((ticks / CYCLE as f32 * 2.0).powi(3)).clamp(0.0, 1.0);
    }
    ((CYCLE as f32 - ticks) / CYCLE as f32 * 3.0).clamp(0.0, 1.0)
}

impl Press {
    /// `PressingBehaviour.start`.
    pub fn start(&mut self, mode: Mode) {
        *self = Press {
            running: true,
            ticks: 0,
            mode,
        };
    }

    /// `PressingBehaviour.tick` on the server for a running press turning at `speed`.
    pub fn tick(&mut self, speed: f32) -> Tick {
        if !self.running {
            return Tick::Moving;
        }
        let down = self.ticks == CYCLE / 2 && speed != 0.0;
        if self.ticks > CYCLE {
            self.running = false;
            return Tick::Done;
        }
        let before = self.ticks;
        self.ticks += tick_speed(speed);
        if before < CYCLE / 2 && self.ticks >= CYCLE / 2 {
            self.ticks = CYCLE / 2;
        }
        if down { Tick::Down } else { Tick::Moving }
    }
}

/// What a depot does with its held stack under a press this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do until something changes (it stops asking).
    Idle,
    /// Keep asking every tick.
    Wait,
    /// The press starts its cycle on it (`start(Mode.BELT)`); keep asking.
    Start,
    /// Press it now (`whenItemHeld` at the head's lowest point); keep asking.
    Process,
}

/// `DepotBehaviour.tick` after the stack arrived, with `BeltPressingCallbacks`:
/// `onItemReceived` for an unlocked stack, `whenItemHeld` for a locked one. `blocked` is
/// `BeltProcessingBehaviour.isBlocked` (something solid between depot and press); `fits` says
/// whether a pressing recipe takes the stack.
pub fn depot_step(held: &mut Held, press: &Press, speed: f32, blocked: bool, fits: bool) -> Step {
    if !held.locked && blocked {
        return Step::Idle;
    }
    if held.locked {
        if speed == 0.0 {
            held.locked = false;
            return Step::Idle;
        }
        if !press.running {
            // PASS: unlocked; next tick it is received again (and may start another cycle).
            held.locked = false;
            return Step::Wait;
        }
        if press.ticks != CYCLE / 2 {
            return Step::Wait;
        }
        return Step::Process;
    }
    if speed == 0.0 {
        return Step::Idle;
    }
    if press.running {
        held.locked = true;
        return Step::Wait;
    }
    if !fits {
        return Step::Idle;
    }
    held.locked = true;
    Step::Start
}

/// The mod-message channel that brings presses' cycles to clients.
pub const CHANNEL: &str = "create:press";

const RECORD: usize = 12 + 1 + 4 + 1;

/// Presses' cycles as records of 18 bytes: position, running, cycle ticks, mode; at most
/// 30,000 bytes a message.
pub fn encode(all: &[([i32; 3], Press)]) -> Vec<Vec<u8>> {
    all.chunks(30_000 / RECORD)
        .map(|chunk| {
            let mut out = Vec::with_capacity(chunk.len() * RECORD);
            for (pos, p) in chunk {
                for v in pos {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                out.push(u8::from(p.running));
                out.extend_from_slice(&p.ticks.to_le_bytes());
                out.push(p.mode.index());
            }
            out
        })
        .collect()
}

pub fn decode(data: &[u8]) -> Vec<([i32; 3], Press)> {
    let i = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
    data.chunks_exact(RECORD)
        .map(|r| {
            (
                [i(&r[0..4]), i(&r[4..8]), i(&r[8..12])],
                Press {
                    running: r[12] != 0,
                    ticks: i(&r[13..17]),
                    mode: Mode::from_index(r[17]),
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_speed_follows_create_s_lerp() {
        assert_eq!(
            [0.0, 1.0, 16.0, 32.0, 256.0, 512.0, -64.0].map(tick_speed),
            [0, 1, 2, 4, 30, 60, 8]
        );
    }

    #[test]
    fn the_head_goes_down_cubically_and_back_linearly() {
        assert_eq!(head_offset(0.0), 0.0);
        assert_eq!(head_offset(60.0), 0.125);
        assert_eq!(head_offset(120.0), 1.0);
        assert_eq!(head_offset(159.0), 1.0);
        assert_eq!(head_offset(200.0), 0.5);
        assert_eq!(head_offset(240.0), 0.0);
    }

    /// A depot holding two iron ingots under a press at 32 rpm (4 cycle ticks a tick): the
    /// stack is held, pressed once at the head's lowest point, then the rest starts again.
    #[test]
    fn a_depot_stack_is_pressed_one_at_a_time() {
        let mut held = Held::new(0, false);
        let mut press = Press::default();
        let speed = 32.0;
        assert_eq!(
            depot_step(&mut held, &press, speed, false, true),
            Step::Start
        );
        press.start(Mode::Belt);
        assert!(held.locked);
        let mut pressed_at = None;
        for tick in 1..200 {
            if depot_step(&mut held, &press, speed, false, true) == Step::Process {
                pressed_at.get_or_insert(tick);
            }
            if press.tick(speed) == Tick::Done {
                // 0 → 120 in 30 ticks, the 120 tick, 124 → 244 in 31 more, then done.
                assert_eq!((pressed_at, tick), (Some(31), 62));
                break;
            }
        }
        // The cycle ended: the locked stack is let go, then received again, and starts again.
        assert_eq!(
            depot_step(&mut held, &press, speed, false, true),
            Step::Wait
        );
        assert!(!held.locked);
        assert_eq!(
            depot_step(&mut held, &press, speed, false, true),
            Step::Start
        );
        // Nothing to press, or a block in the way, or no speed: it stops asking.
        let mut other = Held::new(0, false);
        assert_eq!(
            depot_step(&mut other, &Press::default(), speed, false, false),
            Step::Idle
        );
        assert_eq!(
            depot_step(&mut other, &Press::default(), speed, true, true),
            Step::Idle
        );
        assert_eq!(
            depot_step(&mut other, &Press::default(), 0.0, false, true),
            Step::Idle
        );
    }

    #[test]
    fn cycles_round_trip() {
        let all = vec![(
            [-3, 64, 9],
            Press {
                running: true,
                ticks: 120,
                mode: Mode::Belt,
            },
        )];
        assert_eq!(decode(&encode(&all)[0]), all);
    }
}
