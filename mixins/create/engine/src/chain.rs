//! Encased chain drives and adjustable chain gearshifts, after Create's `ChainDriveBlock`,
//! `ChainGearshiftBlock` and `ChainGearshiftBlockEntity`: how a drive joins the drives beside it
//! into a chain (`updateShape`: its `part` and the axis the chain runs along), and the speed an
//! adjustable gearshift gives its chain for the redstone signal it gets.

use crate::kinetics::{Axis, Dir};

/// `ChainDriveBlock.Part`: where a drive sits in its chain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Part {
    Start,
    Middle,
    End,
    #[default]
    None,
}

impl Part {
    pub fn read(v: &str) -> Option<Part> {
        Some(match v {
            "start" => Part::Start,
            "middle" => Part::Middle,
            "end" => Part::End,
            "none" => Part::None,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Part::Start => "start",
            Part::Middle => "middle",
            Part::End => "end",
            Part::None => "none",
        }
    }
}

/// A drive's state as its chain rules read it: `AXIS`, `PART` and
/// `CONNECTED_ALONG_FIRST_COORDINATE` (`axis_along_first`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drive {
    pub axis: Axis,
    pub part: Part,
    pub along_first: bool,
}

/// `ChainDriveBlock.getConnectionAxis`: the first of the other two axes (y for x, x otherwise),
/// or the second (y for z, z otherwise).
pub fn connection_axis(axis: Axis, along_first: bool) -> Axis {
    match (along_first, axis) {
        (true, Axis::X) => Axis::Y,
        (true, _) => Axis::X,
        (false, Axis::Z) => Axis::Y,
        (false, _) => Axis::Z,
    }
}

impl Drive {
    pub fn connection_axis(self) -> Axis {
        connection_axis(self.axis, self.along_first)
    }

    /// Whether the chain goes on toward the positive (`START`, `MIDDLE`) and the negative
    /// (`MIDDLE`, `END`) side of its axis (`areBlocksConnected`).
    pub fn goes_on(self) -> (bool, bool) {
        (
            matches!(self.part, Part::Start | Part::Middle),
            matches!(self.part, Part::Middle | Part::End),
        )
    }

    /// `ChainDriveBlock.updateShape`: this drive after its neighbour on `face` became
    /// `neighbour` (None: not a chain drive).
    pub fn update_shape(self, face: Dir, neighbour: Option<Drive>) -> Drive {
        let mut part = self.part;
        let mut along_first = self.along_first;
        let face_axis = face.axis();
        let facing_along_first = if self.axis == Axis::X {
            face_axis == Axis::Y
        } else {
            face_axis == Axis::X
        };
        let positive = face.positive();
        if self.axis == face_axis {
            return self;
        }
        let Some(other) = neighbour else {
            if facing_along_first != along_first || part == Part::None {
                return self;
            }
            if part == Part::Middle {
                let part = if positive { Part::End } else { Part::Start };
                return Drive { part, ..self };
            }
            if (part == Part::Start) == positive {
                return Drive {
                    part: Part::None,
                    ..self
                };
            }
            return self;
        };
        if other.axis == face_axis {
            return self;
        }
        if other.part != Part::None && other.connection_axis() != face_axis {
            return self;
        }
        if part == Part::None {
            part = if positive { Part::Start } else { Part::End };
            along_first = facing_along_first;
        } else if self.connection_axis() != face_axis {
            return self;
        }
        if (part == Part::Start) != positive {
            part = Part::Middle;
        }
        Drive {
            part,
            along_first,
            ..self
        }
    }

    /// `getStateForPlacement` / `updateAfterWrenched`: `updateShape` toward each side off its
    /// axis in turn, with what is there.
    pub fn shaped(self, neighbour: impl Fn(Dir) -> Option<Drive>) -> Drive {
        Dir::ALL
            .into_iter()
            .filter(|d| d.axis() != self.axis)
            .fold(self, |s, d| s.update_shape(d, neighbour(d)))
    }
}

/// `ChainGearshiftBlockEntity.getModifierForSignal`: 1 without a signal, else
/// `1 + (signal + 1) / 16` (up to 2 at 15).
pub fn modifier_for_signal(signal: i32) -> f32 {
    if signal == 0 {
        1.0
    } else {
        1.0 + (signal + 1) as f32 / 16.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(axis: Axis, part: Part, along_first: bool) -> Drive {
        Drive {
            axis,
            part,
            along_first,
        }
    }

    #[test]
    fn the_gearshift_s_modifier_follows_the_signal() {
        assert_eq!(modifier_for_signal(0), 1.0);
        assert_eq!(modifier_for_signal(1), 1.125);
        assert_eq!(modifier_for_signal(7), 1.5);
        assert_eq!(modifier_for_signal(15), 2.0);
    }

    #[test]
    fn the_connection_axis_is_one_of_the_other_two() {
        assert_eq!(connection_axis(Axis::X, true), Axis::Y);
        assert_eq!(connection_axis(Axis::X, false), Axis::Z);
        assert_eq!(connection_axis(Axis::Y, true), Axis::X);
        assert_eq!(connection_axis(Axis::Y, false), Axis::Z);
        assert_eq!(connection_axis(Axis::Z, true), Axis::X);
        assert_eq!(connection_axis(Axis::Z, false), Axis::Y);
    }

    #[test]
    fn drives_side_by_side_form_a_chain() {
        // Three x-axis drives in a row along z, placed north to south.
        let lone = drive(Axis::X, Part::None, false);
        let a = lone.shaped(|_| None);
        assert_eq!(a, lone);
        // The second, south of the first, joins it: the first starts the chain toward the
        // south, the second ends it.
        let b = lone.shaped(|d| (d == Dir::North).then_some(a));
        assert_eq!(b, drive(Axis::X, Part::End, false));
        let a = a.update_shape(Dir::South, Some(b));
        assert_eq!(a, drive(Axis::X, Part::Start, false));
        // A third: the second is in the middle.
        let c = lone.shaped(|d| (d == Dir::North).then_some(b));
        assert_eq!(c, drive(Axis::X, Part::End, false));
        let b = b.update_shape(Dir::South, Some(c));
        assert_eq!(b, drive(Axis::X, Part::Middle, false));
        // The third goes: the middle one ends the chain; then the first goes too: alone again.
        let b = b.update_shape(Dir::South, None);
        assert_eq!(b, drive(Axis::X, Part::End, false));
        let b = b.update_shape(Dir::North, None);
        assert_eq!(b, lone);
    }

    #[test]
    fn a_chain_runs_along_the_first_axis_when_joined_that_way() {
        // An x-axis drive with one above: the chain runs along y (the first other axis).
        let lone = drive(Axis::X, Part::None, false);
        let above = lone;
        let s = lone.shaped(|d| (d == Dir::Up).then_some(above));
        assert_eq!(s, drive(Axis::X, Part::Start, true));
        assert_eq!(s.connection_axis(), Axis::Y);
        // A z-axis drive beside it does not join (its axis is the face's).
        let z = drive(Axis::Z, Part::None, false);
        assert_eq!(lone.shaped(|d| (d == Dir::South).then_some(z)), lone);
        // A drive already chained along y beside it on z does not join either.
        let chained = drive(Axis::X, Part::Start, true);
        assert_eq!(lone.shaped(|d| (d == Dir::South).then_some(chained)), lone);
        // Losing a neighbour across the chain changes nothing.
        let m = drive(Axis::X, Part::Middle, true);
        assert_eq!(m.update_shape(Dir::North, None), m);
    }
}
