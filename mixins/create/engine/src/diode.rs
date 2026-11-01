//! Create's brass diodes (`BrassDiodeBlock` on vanilla's `DiodeBlock`, and the pulse repeater's,
//! extender's and timer's block entities). The block takes its input as a repeater does
//! (`powered`, two ticks after the back changes); its block entity counts `state` toward the
//! delay it is set to (`maxState`) and switches the output (`powering`), which an `inverted`
//! diode turns around.

use crate::kinetics::Dir;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Repeater,
    Extender,
    Timer,
}

impl Kind {
    /// `BrassDiodeBlockEntity.defaultValue` (2), `PulseTimerBlockEntity.defaultValue` (20).
    pub fn default_delay(self) -> i32 {
        match self {
            Kind::Timer => 20,
            Kind::Repeater | Kind::Extender => 2,
        }
    }
}

/// `maxState.between(2, 60 * 20 * 60)`: two ticks to an hour.
pub const MIN_DELAY: i32 = 2;
pub const MAX_DELAY: i32 = 60 * 20 * 60;

/// `BrassDiodeBlock.getDelay`: the block's own tick comes two ticks after its input changed.
pub const BLOCK_DELAY: i64 = 2;

/// `DiodeBlock.setPlacedBy`: a diode placed on a signal looks at it one tick later.
pub const PLACED_DELAY: i64 = 1;

/// A diode's block state: `facing` (toward its input), `powered` (its input), `powering` (its
/// block entity's output) and `inverted`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub facing: Dir,
    pub powered: bool,
    pub powering: bool,
    pub inverted: bool,
}

/// `DiodeBlock.checkTickOnNeighbor`: whether its input differs from `powered`, so a tick must
/// come.
pub fn needs_tick(powered: bool, input: bool) -> bool {
    powered != input
}

/// `DiodeBlock.tick`: the new `powered`, and whether another tick follows `BLOCK_DELAY` later
/// (on for one tick even when the input is already gone, then off).
pub fn block_tick(powered: bool, input: bool) -> (bool, bool) {
    if powered && !input {
        (false, false)
    } else if !powered {
        (true, !input)
    } else {
        (powered, false)
    }
}

/// `BrassDiodeBlock.getSignal` (and `getDirectSignal`, as for any diode): 15 toward the side it
/// faces while `powering` and `inverted` differ.
pub fn signal(s: &State, direction: Dir) -> i32 {
    if s.facing == direction && (s.powering ^ s.inverted) {
        15
    } else {
        0
    }
}

/// `BrassDiodeBlock.toggle`: the lever click's pitch, by `inverted` before the click.
pub fn toggle_pitch(inverted: bool) -> f32 {
    if inverted { 0.5 } else { 0.6 }
}

/// A diode's block entity (`BrassDiodeBlockEntity`): `state` and the delay (`maxState`), with
/// the block's `powered` and `powering` as it last was, which its tick reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Diode {
    pub kind: Kind,
    pub state: i32,
    pub max: i32,
    pub powered: bool,
    pub powering: bool,
}

impl Diode {
    pub fn new(kind: Kind) -> Diode {
        Diode {
            kind,
            state: 0,
            max: kind.default_delay(),
            powered: false,
            powering: false,
        }
    }

    /// `BrassDiodeBlockEntity.tick` -> `updateState` on the server: the new `powering` when the
    /// block must change (`setBlockAndUpdate`), else None.
    pub fn tick(&mut self) -> Option<bool> {
        let (powered, powering) = (self.powered, self.powering);
        let max = self.max;
        let at_min = self.state <= 0;
        let next = match self.kind {
            // PulseRepeaterBlockEntity: counts up while powered; on at `max - 1`, off at
            // `max + 1`, then waits for the input and output to be gone.
            Kind::Repeater => {
                if at_min && !powered {
                    return None;
                }
                if self.state > max + 1 {
                    if !powered && !powering {
                        self.state = 0;
                    }
                    return None;
                }
                self.state += 1;
                if self.state == max - 1 && !powering {
                    Some(true)
                } else if self.state == max + 1 && powering {
                    Some(false)
                } else {
                    None
                }
            }
            // PulseExtenderBlockEntity: on while powered, held at `max`; counts down after,
            // off at 1.
            Kind::Extender => {
                if at_min && !powered {
                    None
                } else if at_min || powered {
                    self.state = max;
                    (!powering).then_some(true)
                } else if self.state == 1 {
                    self.state = 0;
                    powering.then_some(false)
                } else {
                    self.state -= 1;
                    None
                }
            }
            // PulseTimerBlockEntity: counts to `max - 1` and around while unpowered, on for its
            // first two counts (the first one at a delay of 2).
            Kind::Timer => {
                if powered || self.state >= max - 1 {
                    self.state = 0;
                } else {
                    self.state += 1;
                }
                let should = !powered
                    && if max == 2 {
                        self.state == 0
                    } else {
                        self.state <= 1
                    };
                (powering != should).then_some(should)
            }
        };
        if let Some(p) = next {
            self.powering = p;
        }
        next
    }

    /// Whether its tick would change nothing: such a diode leaves the ticking list until its
    /// block or delay changes. An unpowered timer never idles.
    pub fn idle(&self) -> bool {
        let mut d = *self;
        d.tick().is_none() && d == *self
    }

    /// `ScrollValueBehaviour.setValue` within `between(2, 60 * 20 * 60)`, then
    /// `onMaxDelayChanged` (`state` within the new delay). Whether it changed.
    pub fn set_delay(&mut self, value: i32) -> bool {
        let value = value.clamp(MIN_DELAY, MAX_DELAY);
        if value == self.max {
            return false;
        }
        self.max = value;
        self.state = self.state.clamp(0, value);
        true
    }

    /// `BrassDiodeBlockEntity.getProgress`.
    pub fn progress(&self) -> f32 {
        let max = self.max.max(2);
        self.state.clamp(0, max) as f32 / max as f32
    }
}

/// `BrassDiodeBlockEntity.format`: ticks below 60, then seconds, then minutes.
pub fn format(value: i32) -> String {
    if value < 60 {
        format!("{value}t")
    } else if value < 20 * 60 {
        format!("{}s", value / 20)
    } else {
        format!("{}m", value / 20 / 60)
    }
}

/// `BrassDiodeScrollSlot.getLocalOffset`: the value box over the middle of the diode, 2.6 px up.
pub const SLOT: [f64; 3] = [0.5, 2.6 / 16.0, 0.5];

#[cfg(test)]
mod tests {
    use super::*;

    /// Ticks `d` with its input `powered` for `n` ticks; the output after each.
    fn run(d: &mut Diode, powered: bool, n: usize) -> Vec<bool> {
        d.powered = powered;
        (0..n)
            .map(|_| {
                d.tick();
                d.powering
            })
            .collect()
    }

    #[test]
    fn the_block_follows_its_input_as_a_repeater() {
        assert!(needs_tick(false, true));
        assert_eq!(block_tick(false, true), (true, false));
        assert_eq!(block_tick(true, false), (false, false));
        // A pulse gone by the tick: on for one more tick.
        assert_eq!(block_tick(false, false), (true, true));
        assert_eq!(BLOCK_DELAY, 2);
    }

    #[test]
    fn a_pulse_repeater_sends_two_ticks_after_its_delay() {
        let mut d = Diode::new(Kind::Repeater);
        assert!(d.idle());
        d.set_delay(5);
        // On at state 4 (max - 1), off at 6 (max + 1), however long the input stays.
        let out = run(&mut d, true, 10);
        assert_eq!(
            out,
            [
                false, false, false, true, true, false, false, false, false, false
            ]
        );
        assert_eq!(d.state, 7);
        assert!(d.idle(), "held past max + 1 while powered");
        // Released: back to 0, then idle.
        run(&mut d, false, 1);
        assert_eq!(d.state, 0);
        assert!(d.idle());
    }

    #[test]
    fn a_pulse_extender_holds_its_output_for_its_delay() {
        let mut d = Diode::new(Kind::Extender);
        d.set_delay(4);
        assert_eq!(run(&mut d, true, 3), [true, true, true]);
        assert_eq!(d.state, 4);
        assert!(d.idle(), "powered and held at max");
        // Released: 4 -> 3 -> 2 -> 1, off at 1.
        assert_eq!(run(&mut d, false, 4), [true, true, true, false]);
        assert_eq!(d.state, 0);
        assert!(d.idle());
    }

    #[test]
    fn a_pulse_timer_pulses_every_delay_while_unpowered() {
        let mut d = Diode::new(Kind::Timer);
        assert_eq!(d.max, 20);
        assert!(!d.idle());
        let out = run(&mut d, false, 40);
        // Counts 1..19 then 0: on at counts 0 and 1, two ticks in every 20.
        assert_eq!(out.iter().filter(|p| **p).count(), 4);
        assert_eq!(&out[..3], [true, false, false]);
        assert_eq!(&out[18..22], [false, true, true, false]);
        // Powered: off, stopped at 0, idle.
        assert_eq!(run(&mut d, true, 2), [false, false]);
        assert!(d.idle());
        // At a delay of 2 it is on every other tick.
        d.set_delay(2);
        assert_eq!(run(&mut d, false, 4), [false, true, false, true]);
    }

    #[test]
    fn its_delay_stays_in_range_and_cuts_its_count() {
        let mut d = Diode::new(Kind::Repeater);
        assert!(!d.set_delay(1), "2 already");
        assert!(d.set_delay(100_000));
        assert_eq!(d.max, MAX_DELAY);
        d.state = 500;
        d.set_delay(30);
        assert_eq!(d.state, 30);
        assert_eq!(d.progress(), 1.0);
        assert_eq!(format(40), "40t");
        assert_eq!(format(100), "5s");
        assert_eq!(format(72000), "60m");
    }

    #[test]
    fn an_inverted_diode_gives_a_signal_while_off() {
        let s = State {
            facing: Dir::North,
            powered: false,
            powering: false,
            inverted: true,
        };
        assert_eq!(signal(&s, Dir::North), 15);
        assert_eq!(signal(&s, Dir::South), 0);
        let on = State {
            powering: true,
            ..s
        };
        assert_eq!(signal(&on, Dir::North), 0);
        assert_eq!(
            signal(
                &State {
                    inverted: false,
                    ..on
                },
                Dir::North
            ),
            15
        );
        assert_eq!(toggle_pitch(false), 0.6);
        assert_eq!(toggle_pitch(true), 0.5);
    }
}
