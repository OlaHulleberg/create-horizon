//! Create's sliding doors (`SlidingDoorBlock`, `SlidingDoorBlockEntity`): the iron door's
//! template places them, keeps their halves together and follows redstone; these are Create's own
//! rules on top: a double door opens and closes as one, `visible` turns off while the door is open
//! or sliding, and on again once its panel has slid shut.

use crate::kinetics::Dir;
use crate::parts::StateInfo;

/// A sliding door's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Door {
    pub facing: Dir,
    pub upper: bool,
    pub hinge_right: bool,
    pub open: bool,
    pub visible: bool,
}

impl Door {
    pub fn of(info: &StateInfo) -> Option<Door> {
        Some(Door {
            facing: info.facing?,
            upper: info.upper,
            hinge_right: info.hinge_right,
            open: info.open,
            visible: info.visible,
        })
    }

    /// Where the other door of a double door would stand: `pos.relative(hinge == LEFT ?
    /// facing.getClockWise() : facing.getCounterClockWise())`.
    pub fn partner(&self, pos: [i32; 3]) -> [i32; 3] {
        let side = if self.hinge_right {
            counter_clockwise(self.facing)
        } else {
            clockwise(self.facing)
        };
        let o = side.offset();
        [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]
    }

    /// The other half's position.
    pub fn other_half(&self, pos: [i32; 3]) -> [i32; 3] {
        [pos[0], pos[1] + if self.upper { -1 } else { 1 }, pos[2]]
    }

    /// `SlidingDoorBlock.isDoubleDoor(this, hinge, facing, other)`, `other` being the same block:
    /// the other hinge, the same facing and half, and not (yet) as open as this.
    pub fn is_double(&self, other: &Door) -> bool {
        other.hinge_right != self.hinge_right
            && other.facing == self.facing
            && other.open != self.open
            && other.upper == self.upper
    }
}

fn clockwise(d: Dir) -> Dir {
    match d {
        Dir::North => Dir::East,
        Dir::East => Dir::South,
        Dir::South => Dir::West,
        Dir::West => Dir::North,
        other => other,
    }
}

fn counter_clockwise(d: Dir) -> Dir {
    match d {
        Dir::North => Dir::West,
        Dir::West => Dir::South,
        Dir::South => Dir::East,
        Dir::East => Dir::North,
        other => other,
    }
}

/// The panel's slide (`SlidingDoorBlockEntity.animation`): catnip's `LerpedFloat.linear()`,
/// chased at 0.15 a tick toward 1 when open, 0 when closed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slide {
    value: f32,
    previous: f32,
    target: f32,
    chasing: bool,
}

/// `Mth.equal` for doubles.
fn equal(a: f64, b: f64) -> bool {
    (b - a).abs() < 1.0e-5
}

impl Slide {
    /// A block entity made or loaded with its door (`startWithValue(isOpen ? 1 : 0)`).
    pub fn at_rest(open: bool) -> Slide {
        let v = if open { 1.0 } else { 0.0 };
        Slide {
            value: v,
            previous: v,
            target: v,
            chasing: false,
        }
    }

    /// `LerpedFloat.settled`.
    pub fn settled(&self) -> bool {
        equal(f64::from(self.previous), f64::from(self.value))
            && (!self.chasing || equal(f64::from(self.value), f64::from(self.target)))
    }

    /// Whether the panel is still and where its door wants it: nothing more to tick.
    pub fn at_target(&self) -> bool {
        self.settled() && self.chasing
    }

    /// One server tick of `SlidingDoorBlockEntity.tick` for a door `open` and `visible` now:
    /// true when the closed panel has just come to rest while hidden (`showBlockModel`: the
    /// door turns visible, with an iron door's closing sound at half volume).
    pub fn tick(&mut self, open: bool, visible: bool) -> bool {
        let was_settled = self.settled();
        // chase(open ? 1 : 0, .15f, Chaser.LINEAR)
        self.target = if open { 1.0 } else { 0.0 };
        self.chasing = true;
        // tickChaser
        self.previous = self.value;
        if equal(f64::from(self.value), f64::from(self.target)) {
            self.value = self.target;
        } else {
            let (c, t, s) = (
                f64::from(self.value),
                f64::from(self.target),
                f64::from(0.15f32),
            );
            self.value = (c + (t - c).clamp(-s, s)) as f32;
        }
        !open && !was_settled && self.settled() && !visible
    }

    pub fn value(&self) -> f32 {
        self.value
    }
}

/// `SlidingDoorRenderer`'s slide of a sliding (not folding) door at `value` (0 closed, 1 open):
/// along `facing.getClockWise()` (the other way for a left hinge) by `value² × 13/16`, and out
/// along its facing by up to 1/32 over the first tenth of the way.
pub fn slide_offset(facing: Dir, hinge_right: bool, value: f32) -> [f32; 3] {
    let along = if hinge_right {
        clockwise(facing)
    } else {
        counter_clockwise(facing)
    };
    let out = (value * 10.0).clamp(0.0, 1.0) / 32.0;
    let slide = value * value * 13.0 / 16.0;
    let (a, f) = (along.offset(), facing.offset());
    [0, 1, 2].map(|i| a[i] as f32 * slide + f[i] as f32 * out)
}

/// One leaf of a folding door (`SlidingDoorRenderer`, `isFoldingDoor`) at `value`: the steps
/// of its pose in order, as the renderer applies them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FoldStep {
    Translate([f32; 3]),
    /// Degrees about y through `pivot` (blocks).
    TurnY {
        degrees: f32,
        pivot: [f32; 3],
    },
}

/// The pose of a folding door's leaf: `left` the first of the two partials drawn (the left
/// leaf, or the right one for a right hinge, `partials.get(left ^ flip)`), and whether the left
/// partial model is the one to draw.
pub fn fold_pose(facing: Dir, hinge_right: bool, left: bool, value: f32) -> (bool, Vec<FoldStep>) {
    use FoldStep::*;
    let flip = hinge_right;
    let f = if flip { -1.0 } else { 1.0 };
    let out = (value * 10.0).clamp(0.0, 1.0) / 32.0;
    let o = facing.offset();
    let mut steps = vec![
        Translate([0.0, -1.0 / 512.0, 0.0]),
        Translate([o[0] as f32 * out, o[1] as f32 * out, o[2] as f32 * out]),
        TurnY {
            degrees: crate::decor::horizontal_angle(clockwise(facing)),
            pivot: [0.5; 3],
        },
    ];
    if flip {
        steps.push(Translate([0.0, 0.0, 1.0]));
    }
    steps.push(TurnY {
        degrees: 91.0 * f * value * value,
        pivot: [0.0; 3],
    });
    if !left {
        steps.push(Translate([0.0, 0.0, f / 2.0]));
        steps.push(TurnY {
            degrees: -181.0 * f * value * value,
            pivot: [0.0; 3],
        });
    }
    if flip {
        steps.push(Translate([0.0, 0.0, -0.5]));
    }
    (left ^ flip, steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn door(facing: Dir, hinge_right: bool, open: bool) -> Door {
        Door {
            facing,
            upper: false,
            hinge_right,
            open,
            visible: !open,
        }
    }

    #[test]
    fn a_sliding_panel_slides_toward_its_hinge_side_and_out() {
        // Facing north with the hinge right: it slides east (clockwise), 13/16 when open, and
        // 1/32 north.
        let open = slide_offset(Dir::North, true, 1.0);
        assert_eq!(open, [13.0 / 16.0, 0.0, -1.0 / 32.0]);
        // A left hinge slides it west; half way it has gone a quarter of the way.
        let half = slide_offset(Dir::North, false, 0.5);
        assert_eq!(half, [-13.0 / 64.0, 0.0, -1.0 / 32.0]);
        assert_eq!(slide_offset(Dir::East, true, 0.0), [0.0; 3]);
    }

    #[test]
    fn folding_leaves_turn_against_each_other() {
        let (first, steps) = fold_pose(Dir::South, false, true, 1.0);
        assert!(first);
        assert!(steps.contains(&FoldStep::TurnY {
            degrees: 91.0,
            pivot: [0.0; 3]
        }));
        let (model_left, steps) = fold_pose(Dir::South, true, false, 1.0);
        assert!(model_left, "a right hinge draws the left leaf second");
        assert!(steps.contains(&FoldStep::TurnY {
            degrees: 181.0,
            pivot: [0.0; 3]
        }));
    }

    #[test]
    fn double_doors_stand_on_the_hinge_side_away() {
        // Facing north, hinge left: the other door is to the east (clockwise).
        let d = door(Dir::North, false, true);
        assert_eq!(d.partner([0, 64, 0]), [1, 64, 0]);
        // Its partner (hinge right, to the east) points back west.
        let p = door(Dir::North, true, false);
        assert_eq!(p.partner([1, 64, 0]), [0, 64, 0]);
        assert!(d.is_double(&p), "opened, its closed partner follows");
        assert!(!d.is_double(&door(Dir::North, true, true)), "already open");
        assert!(!d.is_double(&door(Dir::North, false, false)), "same hinge");
        assert!(!d.is_double(&door(Dir::South, true, false)), "other facing");
        let upper = Door { upper: true, ..p };
        assert!(!d.is_double(&upper), "other half");
        assert_eq!(upper.other_half([1, 65, 0]), [1, 64, 0]);
    }

    #[test]
    fn a_closed_door_shows_its_model_once_the_panel_has_slid_shut() {
        // Fully open, then closed: 0.15 a tick takes 7 ticks to reach 0; it is settled (and
        // shown) on the 8th.
        let mut s = Slide::at_rest(true);
        assert!(!s.tick(true, false), "open and still");
        let mut shown = None;
        for t in 1..=12 {
            if s.tick(false, false) {
                shown = Some(t);
                break;
            }
        }
        assert_eq!(shown, Some(8));
        assert_eq!(s.value(), 0.0);
        assert!(s.at_target());
        // Opened then closed before it slid all the way: it shows when back at 0.
        let mut s = Slide::at_rest(false);
        for _ in 0..3 {
            assert!(!s.tick(true, false));
        }
        let mut t = 0;
        while !s.tick(false, false) {
            t += 1;
            assert!(t < 20);
        }
        assert_eq!(t + 1, 4, "0.45 back to 0 in 3 ticks, settled on the 4th");
        // A door that never opened (closed and visible) does nothing.
        let mut s = Slide::at_rest(false);
        assert!(!s.tick(false, true));
        assert!(s.at_target());
    }
}
