//! Create's funnels (`FunnelBlock`, `FunnelBlockEntity`) on an inventory: extracting, a stack
//! leaves the inventory behind every 8 ticks while nothing lies in front; inserting, items that
//! land on it go into the inventory. Belt funnels (`BeltFunnelBlock`,
//! `BeltFunnelInteractionHandler`): a funnel over a belt takes the items passing it into the
//! inventory behind it, or puts that inventory's items onto the belt.

use crate::kinetics::Dir;

/// `defaultExtractionTimer`.
pub const EXTRACTION_COOLDOWN: u32 = 8;

/// `FunnelBlockEntity.getAmountToExtract`, up to (no filter set): an andesite funnel's filter
/// is never active (`supportsFiltering`), so it moves one item at a time; a brass funnel's empty
/// filter allows a stack.
pub fn amount(brass: bool) -> u32 {
    if brass { 64 } else { 1 }
}

/// Where an extracted stack appears and how it moves (`activateExtractor`).
pub fn output(pos: [i32; 3], facing: Dir) -> ([f64; 3], [f64; 3]) {
    let o = facing.offset().map(f64::from);
    let vertical = facing.axis() == crate::kinetics::Axis::Y;
    let scale = match facing {
        Dir::Up => 0.15,
        Dir::Down => 0.5,
        _ => 0.25,
    };
    let mut at = [0, 1, 2].map(|a| f64::from(pos[a]) + 0.5 + o[a] * scale);
    if !vertical {
        at[1] -= 0.45;
    }
    let motion = if facing == Dir::Up {
        [0.0, 0.25, 0.0]
    } else {
        [0.0; 3]
    };
    (at, motion)
}

/// `BeltFunnelBlock.Shape`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BeltShape {
    #[default]
    Retracted,
    Extended,
    Pushing,
    Pulling,
}

impl BeltShape {
    pub fn read(v: &str) -> Option<BeltShape> {
        Some(match v {
            "retracted" => BeltShape::Retracted,
            "extended" => BeltShape::Extended,
            "pushing" => BeltShape::Pushing,
            "pulling" => BeltShape::Pulling,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            BeltShape::Retracted => "retracted",
            BeltShape::Extended => "extended",
            BeltShape::Pushing => "pushing",
            BeltShape::Pulling => "pulling",
        }
    }
}

/// `BeltFunnelBlock.getShapeForPosition`: across a belt below it (`belt`, its facing) it is
/// retracted, else perpendicular: pushing when extracting, pulling when not.
pub fn shape_for_position(belt: Option<Dir>, facing: Dir, extracting: bool) -> BeltShape {
    match belt {
        Some(b) if b.axis() == facing.axis() => BeltShape::Retracted,
        _ if extracting => BeltShape::Pushing,
        _ => BeltShape::Pulling,
    }
}

/// `BeltFunnelBlock.updateShape` on a valid belt: the shape its place gives, except that a
/// wrenched state is kept (pulling over pushing, extended over retracted).
pub fn updated_shape(current: BeltShape, updated: BeltShape) -> BeltShape {
    match (current, updated) {
        (BeltShape::Pulling, BeltShape::Pushing) | (BeltShape::Extended, BeltShape::Retracted) => {
            current
        }
        _ => updated,
    }
}

/// `FunnelBlockEntity.Mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Invalid,
    Paused,
    Collect,
    PushingToBelt,
    TakingFromBelt,
    Extract,
}

/// `determineCurrentMode` for a belt funnel: pulling takes, pushing puts; otherwise by the way
/// the belt below moves (`movement`, None without a belt): with its facing it puts.
pub fn belt_mode(powered: bool, shape: BeltShape, movement: Option<Dir>, facing: Dir) -> Mode {
    if powered {
        return Mode::Paused;
    }
    match shape {
        BeltShape::Pulling => Mode::TakingFromBelt,
        BeltShape::Pushing => Mode::PushingToBelt,
        _ => match movement {
            Some(m) if m == facing => Mode::PushingToBelt,
            Some(_) => Mode::TakingFromBelt,
            None => Mode::Invalid,
        },
    }
}

/// A belt funnel as a belt sees it over one of its segments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OnBelt {
    pub facing: Dir,
    pub shape: BeltShape,
    pub powered: bool,
}

/// What `checkForFunnels` does at one segment for an item moving to `next`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Check {
    /// Not a funnel that takes from this belt: look at the next segment.
    Skip,
    /// The item has not reached the funnel's entry: stop looking (`return false`).
    Stop,
    /// It reached the entry at `entry`; a `blocking` funnel (facing against the belt) holds it
    /// there.
    Crossed { blocking: bool, entry: f32 },
}

/// `BeltFunnelInteractionHandler.checkForFunnels` for the funnel `f` over `segment`, the belt
/// moving toward `movement` (positive along its offsets or not).
pub fn check(f: &OnBelt, movement: Dir, positive: bool, segment: i32, next: f32) -> Check {
    let blocking = f.facing == movement.opposite();
    if f.facing == movement || f.shape == BeltShape::Pushing {
        return Check::Skip;
    }
    let mut entry = segment as f32 + 0.5;
    if f.shape == BeltShape::Extended {
        entry += 0.499 * if positive { -1.0 } else { 1.0 };
    }
    let crossed = (next > entry && positive) || (next < entry && !positive);
    if !crossed {
        return Check::Stop;
    }
    Check::Crossed { blocking, entry }
}

/// The segments `checkForFunnels` looks at, in order: from the item's own (clamped to the
/// belt) while the segment is not past `next`.
pub fn segments(
    position: f32,
    next: f32,
    positive: bool,
    length: i32,
) -> impl Iterator<Item = i32> {
    let first = (position.floor() as i32).clamp(0, (length - 1).max(0));
    let step = if positive { 1 } else { -1 };
    (0..).map(move |i| first + i * step).take_while(move |&s| {
        if positive {
            s as f32 <= next
        } else {
            (s + 1) as f32 >= next
        }
    })
}

/// Funnels by the game tick they look again at (`extractionCooldown` run out), so a tick with no
/// funnel due costs one look at the earliest. An entry whose time is no longer the funnel's (it
/// was woken, rescheduled or removed meanwhile) is stale and skipped.
#[derive(Clone, Debug, Default)]
pub struct Timers {
    heap: std::collections::BinaryHeap<std::cmp::Reverse<(u64, [i32; 3])>>,
}

impl Timers {
    /// `pos` is due at `tick`.
    pub fn at(&mut self, tick: u64, pos: [i32; 3]) {
        self.heap.push(std::cmp::Reverse((tick, pos)));
    }

    /// The funnels due by `now` whose time `current` still says is theirs, each once.
    pub fn due(
        &mut self,
        now: u64,
        mut current: impl FnMut([i32; 3], u64) -> bool,
    ) -> Vec<[i32; 3]> {
        let mut out = Vec::new();
        while let Some(std::cmp::Reverse((t, p))) = self.heap.peek().copied() {
            if t > now {
                break;
            }
            self.heap.pop();
            if current(p, t) && !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}

/// The inventory a funnel facing `facing` works on: the block behind it.
pub fn behind(pos: [i32; 3], facing: Dir) -> [i32; 3] {
    let o = facing.offset();
    [pos[0] - o[0], pos[1] - o[1], pos[2] - o[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stacks_come_out_in_front() {
        let (at, v) = output([0, 0, 0], Dir::East);
        assert_eq!(at, [0.75, 0.04999999999999999, 0.5]);
        assert_eq!(v, [0.0; 3]);
        assert_eq!(output([0, 0, 0], Dir::Up).1, [0.0, 0.25, 0.0]);
    }

    #[test]
    fn belt_funnel_shapes_follow_the_belt_and_keep_wrenched_states() {
        // Across a belt running north-south a funnel facing north is retracted; one facing east
        // sits beside it: pushing when it extracts, pulling when it inserts.
        assert_eq!(
            shape_for_position(Some(Dir::South), Dir::North, true),
            BeltShape::Retracted
        );
        assert_eq!(
            shape_for_position(Some(Dir::South), Dir::East, true),
            BeltShape::Pushing
        );
        assert_eq!(
            shape_for_position(None, Dir::East, false),
            BeltShape::Pulling
        );
        assert_eq!(
            updated_shape(BeltShape::Extended, BeltShape::Retracted),
            BeltShape::Extended
        );
        assert_eq!(
            updated_shape(BeltShape::Pulling, BeltShape::Pushing),
            BeltShape::Pulling
        );
        assert_eq!(
            updated_shape(BeltShape::Pushing, BeltShape::Retracted),
            BeltShape::Retracted
        );
        // Retracted over a belt moving the way it faces: it puts items on; against it, takes.
        assert_eq!(
            belt_mode(false, BeltShape::Retracted, Some(Dir::North), Dir::North),
            Mode::PushingToBelt
        );
        assert_eq!(
            belt_mode(false, BeltShape::Retracted, Some(Dir::South), Dir::North),
            Mode::TakingFromBelt
        );
        assert_eq!(
            belt_mode(true, BeltShape::Pulling, None, Dir::North),
            Mode::Paused
        );
        assert_eq!(amount(false), 1);
        assert_eq!(amount(true), 64);
    }

    #[test]
    fn items_cross_a_funnel_at_its_entry() {
        let f = OnBelt {
            facing: Dir::North,
            shape: BeltShape::Retracted,
            powered: false,
        };
        // A belt moving south (positive z): the funnel faces against it and blocks at 2.5.
        assert_eq!(check(&f, Dir::South, true, 2, 2.4), Check::Stop);
        assert_eq!(
            check(&f, Dir::South, true, 2, 2.6),
            Check::Crossed {
                blocking: true,
                entry: 2.5
            }
        );
        // Extended, the entry is nearly half a segment earlier.
        let e = OnBelt {
            shape: BeltShape::Extended,
            ..f
        };
        assert_eq!(
            check(&e, Dir::South, true, 2, 2.1),
            Check::Crossed {
                blocking: true,
                entry: 2.001
            }
        );
        // Facing the way the belt moves: not a taker.
        assert_eq!(check(&f, Dir::North, false, 2, 2.4), Check::Skip);
        // A pulling funnel beside the belt takes as items pass under it.
        let side = OnBelt {
            facing: Dir::East,
            shape: BeltShape::Pulling,
            powered: false,
        };
        assert_eq!(
            check(&side, Dir::South, true, 0, 0.7),
            Check::Crossed {
                blocking: false,
                entry: 0.5
            }
        );
        assert_eq!(segments(1.2, 2.6, true, 4).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(segments(2.7, 1.9, false, 4).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(segments(3.9, 4.0, true, 4).collect::<Vec<_>>(), [3, 4]);
    }

    #[test]
    fn timers_give_due_funnels_once() {
        let mut t = Timers::default();
        t.at(5, [0, 0, 0]);
        t.at(3, [1, 0, 0]);
        t.at(3, [1, 0, 0]);
        t.at(9, [2, 0, 0]);
        assert!(t.due(2, |_, _| true).is_empty());
        assert_eq!(t.due(5, |_, _| true), [[1, 0, 0], [0, 0, 0]]);
        // Stale: the funnel moved its time on.
        assert!(t.due(10, |_, tick| tick == 11).is_empty());
        assert!(t.is_empty());
    }
}
