//! Create's gantry (`GantryShaftBlock`, `GantryShaftBlockEntity`, `GantryCarriageBlockEntity`,
//! `GantryContraptionEntity`): a carriage on a line of gantry shafts carries the block in front
//! of it (and what holds on to that) along the shafts at their turning speed, and puts it all
//! back at the end of the line or when the shafts stop.

use crate::kinetics::Dir;

/// `GantryShaftBlock.Part`: where a shaft is in its line, along its facing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Start,
    Middle,
    End,
    Single,
}

impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::Start => "start",
            Part::Middle => "middle",
            Part::End => "end",
            Part::Single => "single",
        }
    }

    pub fn read(name: &str) -> Option<Part> {
        [Part::Start, Part::Middle, Part::End, Part::Single]
            .into_iter()
            .find(|p| p.name() == name)
    }

    /// `updateShape` settled: a shaft of the same facing ahead (along the facing) and behind.
    pub fn of(ahead: bool, behind: bool) -> Part {
        match (ahead, behind) {
            (true, true) => Part::Middle,
            (true, false) => Part::Start,
            (false, true) => Part::End,
            (false, false) => Part::Single,
        }
    }
}

/// `getPinionMovementSpeed`: blocks a tick along the shaft's facing, `convertToLinear(-speed)`
/// held to ±0.49.
pub fn pinion_speed(rpm: f32) -> f32 {
    (-rpm / 512.0).clamp(-0.49, 0.49)
}

/// `canAssembleOn`: a carriage may start or go on from this shaft.
pub fn can_assemble_on(part: Part, powered: bool, speed: f32) -> bool {
    if powered {
        return false;
    }
    match part {
        Part::End => speed < 0.0,
        Part::Middle => speed != 0.0,
        Part::Start => speed > 0.0,
        Part::Single => false,
    }
}

/// A moving gantry: where its carriage started, the shaft line's facing, how far along it has
/// gone and how fast, and its blocks (offsets from the start, states).
#[derive(Clone, Debug, PartialEq)]
pub struct Moving {
    pub start: [i32; 3],
    pub carriage_facing: Dir,
    pub shaft_facing: Dir,
    pub offset: f32,
    pub per_tick: f32,
    pub blocks: Vec<([i32; 3], u32)>,
}

/// The shaft under a moving carriage, as `checkPinionShaft` reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shaft {
    pub facing: Dir,
    pub part: Part,
    pub powered: bool,
    pub speed: f32,
}

/// What a tick of a moving gantry does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// Moves on at this many blocks a tick along the shaft facing.
    Move(f32),
    /// Stops and goes back into the world (`disassemble`).
    Stop,
}

impl Moving {
    /// The carriage's block now (`BlockPos.containing(anchor + .5)`).
    pub fn carriage(&self) -> [i32; 3] {
        let o = self.shaft_facing.offset();
        let d = self.offset.round() as i32;
        [
            self.start[0] + o[0] * d,
            self.start[1] + o[1] * d,
            self.start[2] + o[2] * d,
        ]
    }

    /// The shaft it rides on now.
    pub fn shaft_pos(&self) -> [i32; 3] {
        let c = self.carriage();
        let b = self.carriage_facing.opposite().offset();
        [c[0] + b[0], c[1] + b[1], c[2] + b[2]]
    }

    /// `checkPinionShaft`: no shaft, a powered or still one stops it; past the middle of a
    /// shaft it can't go on from, it stops there; else it moves at that shaft's speed.
    pub fn step(&self, shaft: Option<Shaft>) -> Step {
        let Some(s) = shaft else {
            return Step::Stop;
        };
        let v = pinion_speed(s.speed);
        if s.powered || v == 0.0 {
            return Step::Stop;
        }
        // Along the line's facing, in blocks from the start's centre.
        let sign = if s.facing == self.shaft_facing {
            1.0
        } else {
            -1.0
        };
        let v = v * sign;
        let current = self.offset + 0.5;
        let next = current + v;
        let crossing = (current.floor() + 0.5 < next) != (v < 0.0);
        if crossing && !can_assemble_on(s.part, s.powered, pinion_speed(s.speed)) {
            return Step::Stop;
        }
        Step::Move(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_carriage_runs_the_line_and_stops_on_its_last_shaft() {
        assert_eq!(Part::of(true, false), Part::Start);
        assert_eq!(Part::of(false, true), Part::End);
        // -64 rpm: 0.125 blocks a tick toward the shafts' facing.
        assert_eq!(pinion_speed(-64.0), 0.125);
        assert!(can_assemble_on(Part::Start, false, 0.125));
        assert!(!can_assemble_on(Part::End, false, 0.125));
        assert!(!can_assemble_on(Part::Middle, true, 0.125));
        // Shafts at offsets 0..=4 along east: start, three middles, end.
        let mut g = Moving {
            start: [0, 0, 0],
            carriage_facing: Dir::Up,
            shaft_facing: Dir::East,
            offset: 0.0,
            per_tick: 0.0,
            blocks: vec![],
        };
        let shaft_at = |i: i32| Shaft {
            facing: Dir::East,
            part: match i {
                0 => Part::Start,
                4 => Part::End,
                _ => Part::Middle,
            },
            powered: false,
            speed: -64.0,
        };
        let mut ticks = 0;
        loop {
            let i = g.carriage()[0];
            match g.step((0..=4).contains(&i).then(|| shaft_at(i))) {
                Step::Move(v) => g.offset += v,
                Step::Stop => break,
            }
            ticks += 1;
            assert!(ticks < 100);
        }
        // It ends on the last shaft, at its middle.
        assert_eq!(g.carriage(), [4, 0, 0]);
        assert!((g.offset - 4.0).abs() < 0.13, "{}", g.offset);
        assert_eq!(g.shaft_pos(), [4, -1, 0]);
    }
}
