//! Encased chain drives and adjustable chain gearshifts, after Create's `ChainDriveBlock`,
//! `ChainGearshiftBlock` and `ChainGearshiftBlockEntity`: how a drive joins the drives beside it
//! into a chain (`updateShape`: its `part` and the axis the chain runs along), and the speed an
//! adjustable gearshift gives its chain for the redstone signal it gets.
//!
//! Create runs `updateShape` inside the block change that causes it: the level tells the drives
//! beside a changed block at once, and a drive that changes tells its own neighbours in turn.
//! [`Shapes`] carries those notifications from the block change to the host that writes the
//! states, and runs them in the level's order.

use std::collections::{HashMap, VecDeque};

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
    /// `defaultBlockState().setValue(AXIS, axis)`: alone (`part=none`), the chain's axis the
    /// first other one (`axis_along_first=true`, the running binary's default).
    pub fn placed(axis: Axis) -> Drive {
        Drive {
            axis,
            part: Part::None,
            along_first: true,
        }
    }

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

    /// `ChainDriveBlock.updateAfterWrenched`: the wrenched drive starts over from the default
    /// state on its new `axis` and is shaped by the drives beside it.
    pub fn update_after_wrenched(axis: Axis, neighbour: impl Fn(Dir) -> Option<Drive>) -> Drive {
        Drive::placed(axis).shaped(neighbour)
    }
}

/// The order the level tells a changed block's neighbours their shape may have changed in
/// (vanilla's `BlockBehaviour.UPDATE_SHAPE_ORDER`).
pub const UPDATE_SHAPE_ORDER: [Dir; 6] = [
    Dir::West,
    Dir::East,
    Dir::North,
    Dir::South,
    Dir::Down,
    Dir::Up,
];

/// The level's budget for nested shape updates (`Level.setBlock`'s default recursion of 512).
const SHAPE_RECURSION: u32 = 512;

/// A shape update a drive is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Update {
    /// `getStateForPlacement`: a drive placed is shaped by the drives beside it.
    Placed,
    /// `updateShape(face, …)`: the block on `face` changed.
    Neighbour(Dir),
}

/// The chain-drive shape updates of the level's block changes (`Level.setBlock`'s
/// `updateNeighbourShapes`, and `getStateForPlacement`), in the order the changes were made.
/// [`Shapes::run`] carries them out as the level does inside each change: every drive whose
/// state changes tells its neighbours in turn, depth first, so a chain re-forms completely
/// (`ChainDriveBlock.updateShape`).
#[derive(Debug, Default)]
pub struct Shapes {
    pending: VecDeque<([i32; 3], Update)>,
}

fn offset(pos: [i32; 3], d: Dir) -> [i32; 3] {
    let o = d.offset();
    [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]
}

impl Shapes {
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// A chain drive was placed at `pos`: its placement shape (`getStateForPlacement`).
    pub fn placed(&mut self, pos: [i32; 3]) {
        self.pending.push_back((pos, Update::Placed));
    }

    /// The block at `pos` changed: each drive beside it (`is_drive`) is told, in
    /// [`UPDATE_SHAPE_ORDER`] (`updateNeighbourShapes`).
    pub fn changed(&mut self, pos: [i32; 3], is_drive: impl Fn([i32; 3]) -> bool) {
        for d in UPDATE_SHAPE_ORDER {
            let at = offset(pos, d);
            if is_drive(at) {
                self.pending
                    .push_back((at, Update::Neighbour(d.opposite())));
            }
        }
    }

    /// Carry out the updates due, reading drives from `world` (None: no chain drive there)
    /// overlaid with what the updates made of them. Returns each drive that changed, in the
    /// order they first changed, with its final state.
    pub fn run(&mut self, world: impl Fn([i32; 3]) -> Option<Drive>) -> Vec<([i32; 3], Drive)> {
        let mut run = Run {
            world: &world,
            now: HashMap::new(),
            changed: Vec::new(),
        };
        while let Some((pos, update)) = self.pending.pop_front() {
            run.update(pos, update, SHAPE_RECURSION);
        }
        run.changed
            .into_iter()
            .map(|pos| (pos, run.now[&pos]))
            .filter(|&(pos, drive)| world(pos) != Some(drive))
            .collect()
    }
}

/// One [`Shapes::run`]: the drives changed so far.
struct Run<'a> {
    world: &'a dyn Fn([i32; 3]) -> Option<Drive>,
    now: HashMap<[i32; 3], Drive>,
    changed: Vec<[i32; 3]>,
}

impl Run<'_> {
    fn drive(&self, pos: [i32; 3]) -> Option<Drive> {
        self.now.get(&pos).copied().or_else(|| (self.world)(pos))
    }

    fn update(&mut self, pos: [i32; 3], update: Update, budget: u32) {
        let Some(now) = self.drive(pos) else {
            return;
        };
        let next = match update {
            Update::Placed => now.shaped(|d| self.drive(offset(pos, d))),
            Update::Neighbour(face) => now.update_shape(face, self.drive(offset(pos, face))),
        };
        if next == now {
            return;
        }
        if self.now.insert(pos, next).is_none() {
            self.changed.push(pos);
        }
        // `getStateForPlacement` precedes the change, whose own notifications follow it in
        // the queue; a state `updateShape` changes is set at once (`updateOrDestroy`) and
        // tells its neighbours before the next of the first change's are told.
        if let Update::Neighbour(_) = update
            && budget > 0
        {
            for d in UPDATE_SHAPE_ORDER {
                self.update(offset(pos, d), Update::Neighbour(d.opposite()), budget - 1);
            }
        }
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
    fn isolated_chain_placement_keeps_java_default_coordinate() {
        // ChainDriveBlock registers only PART; the running Java 6.0.11 binary
        // reports axis=y, axis_along_first=true, part=none for both drives.
        for declaration in [
            include_str!("../../../data/create/sunrise/block/encased_chain_drive.json"),
            include_str!("../../../data/create/sunrise/block/adjustable_chain_gearshift.json"),
        ] {
            assert!(declaration.contains("\"axis_along_first\": \"true\""));
        }
        let lone = drive(Axis::Y, Part::None, true);
        assert_eq!(lone.shaped(|_| None), lone);
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
    fn a_wrenched_drive_starts_over_from_its_default_state() {
        // ChainDriveBlock.updateAfterWrenched, as Java 6.0.11 shows it (create-horizon#97): a
        // lone z drive turned onto x beside an x gearshift to its north ends a chain along z.
        let gearshift = drive(Axis::X, Part::None, true);
        let wrenched =
            Drive::update_after_wrenched(Axis::X, |d| (d == Dir::North).then_some(gearshift));
        assert_eq!(wrenched, drive(Axis::X, Part::End, false));
        // Its old part and chain are not kept: alone, it is the default on its new axis.
        assert_eq!(
            Drive::update_after_wrenched(Axis::Y, |_| None),
            drive(Axis::Y, Part::None, true)
        );
    }

    #[test]
    fn a_change_re_forms_the_chain_through_every_drive_it_changes() {
        // Two drives chained along z; a third above the south one. The north one goes: the
        // south one is alone, so the one above joins it, so it joins the one above.
        let north = [0, 0, -1];
        let (here, above) = ([0, 0, 0], [0, 1, 0]);
        let world = |p: [i32; 3]| match p {
            [0, 0, 0] => Some(drive(Axis::X, Part::End, false)),
            [0, 1, 0] => Some(drive(Axis::X, Part::None, true)),
            _ => None,
        };
        let mut shapes = Shapes::default();
        shapes.changed(north, |p| world(p).is_some());
        assert!(!shapes.is_empty());
        assert_eq!(
            shapes.run(world),
            [
                (here, drive(Axis::X, Part::Start, true)),
                (above, drive(Axis::X, Part::End, true)),
            ]
        );
        assert!(shapes.is_empty());
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
