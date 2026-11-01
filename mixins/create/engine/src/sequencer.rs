//! The sequenced gearshift, after Create's `SequencedGearshiftBlockEntity`, `Instruction`,
//! `SequencerInstructions` and `InstructionSpeedModifiers`: a list of instructions (turn so many
//! degrees or metres, wait, wait for a pulse, end) run one after the other while it is powered,
//! each turning the far half at its speed modifier for as many ticks as it needs.
//!
//! The block's `state` (0 idle, else the running instruction's index + 1) is what the game shows
//! and what a comparator reads; the rest lives here, ticking only while an instruction runs.

use crate::kinetics::convert_to_angular;

/// `SequencerInstructions`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    TurnAngle,
    TurnDistance,
    Delay,
    Await,
    End,
}

impl Kind {
    /// `maxValue`, `shiftStep` and `defaultValue` of the instruction's parameter (none for
    /// await and end).
    pub fn value_range(self) -> Option<(i32, i32, i32)> {
        match self {
            Kind::TurnAngle => Some((360, 45, 90)),
            Kind::TurnDistance => Some((128, 5, 5)),
            Kind::Delay => Some((600, 20, 10)),
            Kind::Await | Kind::End => None,
        }
    }
    /// `needsPropagation`: the instructions that turn.
    pub fn turns(self) -> bool {
        matches!(self, Kind::TurnAngle | Kind::TurnDistance)
    }
}

/// `InstructionSpeedModifiers`: `>>` 2, `->` 1, `<-` -1, `<<` -2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    ForwardFast,
    Forward,
    Back,
    BackFast,
}

impl Speed {
    pub fn value(self) -> i32 {
        match self {
            Speed::ForwardFast => 2,
            Speed::Forward => 1,
            Speed::Back => -1,
            Speed::BackFast => -2,
        }
    }
    /// `getByModifier`: forward for any other value.
    pub fn by_modifier(m: i32) -> Speed {
        match m {
            2 => Speed::ForwardFast,
            -1 => Speed::Back,
            -2 => Speed::BackFast,
            _ => Speed::Forward,
        }
    }
}

/// `KineticBlockEntity.convertToLinear`: RPM as metres per tick.
pub fn convert_to_linear(rpm: f32) -> f32 {
    rpm / 512.0
}

/// `Instruction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub kind: Kind,
    pub speed: Speed,
    pub value: i32,
}

impl Instruction {
    /// `new Instruction(kind, value)`: forward.
    pub fn new(kind: Kind, value: i32) -> Instruction {
        Instruction {
            kind,
            speed: Speed::Forward,
            value,
        }
    }

    /// `getDuration`: ticks the instruction runs from `progress` at `speed` RPM; turns always
    /// overshoot by 2 ticks (the target stops them), await is -1 (until a pulse).
    pub fn duration(&self, progress: f32, speed: f32) -> i32 {
        let speed = (speed * self.speed.value() as f32).abs();
        let target = f64::from(self.value) - f64::from(progress);
        match self.kind {
            Kind::TurnAngle => {
                let per_tick = f64::from(convert_to_angular(speed));
                // Java's int: at 0 RPM, MAX_VALUE + 2 wraps.
                ((target / per_tick).ceil() as i32).wrapping_add(2)
            }
            Kind::TurnDistance => {
                let per_tick = f64::from(convert_to_linear(speed));
                ((target / per_tick).ceil() as i32).wrapping_add(2)
            }
            Kind::Delay => target as i32,
            Kind::Await => -1,
            Kind::End => 0,
        }
    }

    /// `getTickProgress`: degrees, metres or ticks done per tick at `speed`.
    pub fn tick_progress(&self, speed: f32) -> f32 {
        match self.kind {
            Kind::TurnAngle => convert_to_angular(speed),
            Kind::TurnDistance => convert_to_linear(speed),
            Kind::Delay => 1.0,
            Kind::Await | Kind::End => 0.0,
        }
    }

    /// `getSpeedModifier`: what the far half turns at while it runs (0 while waiting).
    pub fn speed_modifier(&self) -> i32 {
        if self.kind.turns() {
            self.speed.value()
        } else {
            0
        }
    }
}

/// `Instruction.createDefault`: turn 90°, then end.
pub fn default_instructions() -> Vec<Instruction> {
    vec![
        Instruction::new(Kind::TurnAngle, 90),
        Instruction::new(Kind::End, 1),
    ]
}

/// A sequenced gearshift's block entity state.
#[derive(Clone, Debug, PartialEq)]
pub struct Sequencer {
    pub instructions: Vec<Instruction>,
    /// `currentInstruction`: -1 idle.
    pub current: i32,
    /// `currentInstructionDuration`: -1 for none (or waiting for a pulse).
    pub duration: i32,
    /// `currentInstructionProgress`.
    pub progress: f32,
    pub timer: i32,
    /// `poweredPreviously`.
    pub powered_before: bool,
    /// The speed its block entity turns at (the half turned), as its tick reads it.
    pub speed: f32,
}

impl Default for Sequencer {
    fn default() -> Self {
        Sequencer {
            instructions: default_instructions(),
            current: -1,
            duration: -1,
            progress: 0.0,
            timer: 0,
            powered_before: false,
            speed: 0.0,
        }
    }
}

/// What a step asks of the block's `state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// An instruction started: its index + 1.
    Run(u8),
    /// Idle and unpowered: 0.
    Idle,
    /// The sequence ended (`run` past its end): 0 unless the block is still powered.
    Ended,
}

pub type State = Option<Step>;

impl Sequencer {
    pub fn is_idle(&self) -> bool {
        self.current == -1
    }

    fn instruction(&self, index: i32) -> Option<Instruction> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.instructions.get(i))
            .copied()
    }

    /// `getModifier`: the far half's speed relative to the near one's (0 idle).
    pub fn modifier(&self) -> i32 {
        if self.is_idle() {
            return 0;
        }
        self.instruction(self.current)
            .map_or(0, |i| i.speed_modifier())
    }

    /// `run`: starts instruction `index`, or ends (`END`, or past the list).
    pub fn run(&mut self, index: i32) -> State {
        let Some(instruction) = self.instruction(index).filter(|i| i.kind != Kind::End) else {
            self.current = -1;
            self.duration = -1;
            self.progress = 0.0;
            self.timer = 0;
            return Some(Step::Ended);
        };
        self.duration = instruction.duration(0.0, self.speed);
        self.current = index;
        self.progress = 0.0;
        self.timer = 0;
        Some(Step::Run((index + 1).clamp(0, 5) as u8))
    }

    /// `tick`: the running instruction counts its ticks and progress, then the next one starts.
    pub fn tick(&mut self) -> State {
        if self.is_idle() || self.duration < 0 {
            return None;
        }
        if self.timer < self.duration {
            self.timer += 1;
            if let Some(i) = self.instruction(self.current) {
                self.progress += i.tick_progress(self.speed);
            }
            return None;
        }
        self.run(self.current + 1)
    }

    /// Whether `tick` has anything to do: an instruction runs that is not waiting for a pulse.
    pub fn ticks(&self) -> bool {
        !self.is_idle() && self.duration >= 0
    }

    /// `onSpeedChanged`: the running instruction's time is worked out again for the new speed;
    /// stopping ends the sequence.
    pub fn speed_changed(&mut self, speed: f32) -> State {
        let previous = self.speed;
        self.speed = speed;
        if self.is_idle() || previous.abs() == speed.abs() {
            return None;
        }
        let instruction = self.instruction(self.current)?;
        let state = if speed == 0.0 { self.run(-1) } else { None };
        self.duration = instruction.duration(self.progress, speed);
        self.timer = 0;
        state
    }

    /// `onRedstoneUpdate` (the block's scheduled tick after a neighbour changed): a rising edge
    /// moves past an await; powered while idle starts the sequence if it turns, unpowered while
    /// idle shows `state` 0. `running` is whether the block's `state` is not 0.
    pub fn redstone(&mut self, powered: bool, running: bool) -> State {
        if !self.powered_before
            && powered
            && let Some(state) = self.rising_flank()
        {
            self.powered_before = powered;
            return Some(state);
        }
        self.powered_before = powered;
        if !self.is_idle() || powered == running {
            return None;
        }
        if !powered {
            return Some(Step::Idle);
        }
        if self.speed == 0.0 {
            return None;
        }
        self.run(0)
    }

    /// `risingFlank`: an await goes on to the next instruction.
    fn rising_flank(&mut self) -> State {
        let instruction = self.instruction(self.current)?;
        self.powered_before = true;
        if instruction.kind == Kind::Await {
            return self.run(self.current + 1);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_create_s() {
        // 90° at 16 RPM: 4.8° a tick, ceil(18.75) + 2.
        let turn = Instruction::new(Kind::TurnAngle, 90);
        assert_eq!(turn.duration(0.0, 16.0), 21);
        // Twice as fast with `>>`, the same backwards.
        let fast = Instruction {
            speed: Speed::ForwardFast,
            ..turn
        };
        assert_eq!(fast.duration(0.0, -16.0), 12);
        // 5 m at 32 RPM: 1/16 m a tick, 80 + 2.
        let distance = Instruction::new(Kind::TurnDistance, 5);
        assert_eq!(distance.duration(0.0, 32.0), 82);
        assert_eq!(Instruction::new(Kind::Delay, 10).duration(3.0, 0.0), 7);
        assert_eq!(Instruction::new(Kind::Await, 1).duration(0.0, 16.0), -1);
        assert_eq!(Instruction::new(Kind::End, 1).duration(0.0, 16.0), 0);
        assert_eq!(turn.tick_progress(16.0), 4.8);
        assert_eq!(distance.tick_progress(32.0), 0.0625);
        assert_eq!(turn.speed_modifier(), 1);
        assert_eq!(Instruction::new(Kind::Delay, 10).speed_modifier(), 0);
        assert_eq!(Speed::by_modifier(-2), Speed::BackFast);
        assert_eq!(Speed::by_modifier(5), Speed::Forward);
        assert_eq!(Kind::TurnAngle.value_range(), Some((360, 45, 90)));
    }

    #[test]
    fn a_pulse_runs_the_default_sequence_once() {
        let mut s = Sequencer {
            speed: 16.0,
            ..Sequencer::default()
        };
        assert_eq!(s.modifier(), 0);
        // Powered: the 90° turn starts (state 1), the far half turns.
        assert_eq!(s.redstone(true, false), Some(Step::Run(1)));
        assert_eq!(s.modifier(), 1);
        assert_eq!(s.duration, 21);
        // 21 ticks of 4.8°, then the end (the state stays 1 while still powered).
        for _ in 0..21 {
            assert_eq!(s.tick(), None);
        }
        assert!((s.progress - 100.8).abs() < 1e-3);
        assert_eq!(s.tick(), Some(Step::Ended));
        assert!(s.is_idle());
        assert_eq!(s.modifier(), 0);
        // Power off: state 0; on again: it runs again.
        assert_eq!(s.redstone(false, true), Some(Step::Idle));
        assert_eq!(s.redstone(true, false), Some(Step::Run(1)));
    }

    #[test]
    fn it_ends_unpowered_and_waits_for_pulses() {
        let mut s = Sequencer {
            speed: 32.0,
            instructions: vec![
                Instruction::new(Kind::Delay, 2),
                Instruction::new(Kind::Await, 1),
                Instruction {
                    speed: Speed::Back,
                    ..Instruction::new(Kind::TurnAngle, 45)
                },
            ],
            ..Sequencer::default()
        };
        assert_eq!(s.redstone(true, false), Some(Step::Run(1)));
        assert_eq!(s.modifier(), 0, "a delay holds the far half");
        s.tick();
        s.tick();
        assert_eq!(s.tick(), Some(Step::Run(2)), "the await");
        assert!(!s.ticks(), "waiting costs nothing per tick");
        assert_eq!(s.tick(), None);
        // Still powered: no rising edge. Off, then on again: the turn backwards.
        assert_eq!(s.redstone(true, true), None);
        assert_eq!(s.redstone(false, true), None);
        assert_eq!(s.redstone(true, true), Some(Step::Run(3)));
        assert_eq!(s.modifier(), -1);
        // 45° at 32 RPM: 9.6° a tick, ceil(4.69) + 2 = 7; then past the list it ends.
        assert_eq!(s.duration, 7);
        for _ in 0..7 {
            s.tick();
        }
        assert_eq!(s.tick(), Some(Step::Ended));
        assert!(s.is_idle());
    }

    #[test]
    fn speed_changes_retime_or_stop_it() {
        let mut s = Sequencer {
            speed: 16.0,
            ..Sequencer::default()
        };
        s.redstone(true, false);
        for _ in 0..10 {
            s.tick();
        }
        // 48° done; at 32 RPM the 42° left take ceil(4.375) + 2.
        assert_eq!(s.speed_changed(32.0), None);
        assert_eq!(s.duration, 7);
        assert_eq!(s.timer, 0);
        // The same speed the other way: nothing changes.
        assert_eq!(s.speed_changed(-32.0), None);
        assert_eq!(s.duration, 7);
        // Stopped: it ends.
        assert_eq!(s.speed_changed(0.0), Some(Step::Ended));
        assert!(s.is_idle());
        // Not turning, power does not start it.
        assert_eq!(s.redstone(true, false), None);
    }
}
