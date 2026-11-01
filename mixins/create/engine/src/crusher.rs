//! Create's crushing wheels (`CrushingWheelBlock`, `CrushingWheelControllerBlock`,
//! `CrushingWheelControllerBlockEntity`): two wheels turning against each other two blocks apart
//! keep a controller between them, which catches an item falling in, holds it while it sinks, takes
//! it in and crushes it (Create's crushing recipes, else milling), then lets the results out.

use crate::kinetics::{Axis, Dir};
use crate::processing::{Book, Rng, roll};

/// `updateControllers` from the wheel at `pos`'s side `side`: what the controller between it and
/// the wheel two blocks away should be. None: no controller (no wheel there, or one on another
/// axis); `Some((false, Down))`: one that doesn't crush (the wheels don't turn against each other).
pub fn controller(
    axis: Axis,
    side: Dir,
    speed: f32,
    other: Option<(Axis, f32)>,
) -> Option<(bool, Dir)> {
    if side.axis() == axis {
        return None;
    }
    let (other_axis, other_speed) = other?;
    if other_axis != axis {
        return None;
    }
    if (speed > 0.0) == (other_speed > 0.0) || speed == 0.0 || other_speed == 0.0 {
        return Some((false, Dir::Down));
    }
    // The wheel's axis crossed with the side's, signed by the wheel's turn and the side.
    let unit = |a: Axis| match a {
        Axis::X => [1, 0, 0],
        Axis::Y => [0, 1, 0],
        Axis::Z => [0, 0, 1],
    };
    let (w, s) = (unit(axis), unit(side.axis()));
    let cross = [
        w[1] * s[2] - w[2] * s[1],
        w[2] * s[0] - w[0] * s[2],
        w[0] * s[1] - w[1] * s[0],
    ];
    let sign = speed.signum() as i32 * if side.positive() { 1 } else { -1 };
    let v = cross.map(|c| c * sign);
    Some((true, Dir::from_offset(v).unwrap_or(Dir::Down)))
}

/// `updateSpeed`: the controller crushes at a wheel's speed / 50.
pub fn crushing_speed(wheel_speed: f32) -> f32 {
    (wheel_speed / 50.0).abs()
}

/// A controller's state: the item entity it holds, else what it took in and made.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Crusher {
    pub entity: Option<u32>,
    /// Slot 0 (the input), then up to 31 results.
    pub slots: Vec<(String, u8)>,
    pub remaining: f32,
    pub applied: bool,
}

impl Crusher {
    /// `isOccupied`.
    pub fn occupied(&self) -> bool {
        self.entity.is_some() || !self.slots.is_empty()
    }
}

/// `findRecipe`: crushing, else milling.
pub fn recipe<'a>(
    crushing: &'a Book,
    milling: &'a Book,
    item: &str,
) -> Option<&'a crate::processing::Recipe> {
    crushing
        .find_single(item)
        .map(|i| crushing.get(i))
        .or_else(|| milling.find_single(item).map(|i| milling.get(i)))
}

/// `intakeItem` + `itemInserted`: the stack goes into slot 0; its recipe's time, else 100.
pub fn intake(c: &mut Crusher, item: &str, count: u8, time: Option<i32>) {
    c.entity = None;
    c.slots = vec![(item.to_owned(), count)];
    c.remaining = time.map_or(100.0, |t| t as f32);
    c.applied = false;
}

/// What a tick of a controller without an entity does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
    /// Apply the recipe now (`remainingTime < 20`).
    Apply,
    /// Let the results out.
    Output,
}

/// The inventory's part of `tick`: the time runs down faster at speed, slower for big stacks
/// until the recipe is applied (by log2 of the count).
pub fn tick_inventory(c: &mut Crusher, crushing_speed: f32) -> Step {
    if c.slots.is_empty() || crushing_speed == 0.0 {
        return Step::Wait;
    }
    let speed = crushing_speed * 4.0;
    let count = c.slots.first().map_or(1, |s| s.1);
    let divisor = if c.applied {
        1.0
    } else {
        (f32::from(count)).ln() / 2f32.ln()
    };
    c.remaining -= (speed / divisor).clamp(0.25, 20.0);
    if c.remaining < 20.0 && !c.applied {
        c.applied = true;
        return Step::Apply;
    }
    if c.remaining > 0.0 {
        return Step::Wait;
    }
    c.remaining = 0.0;
    Step::Output
}

/// `applyRecipe`: each item of the input rolls the recipe's results, merged into stacks of at most
/// `max`; no recipe and the input is gone.
pub fn apply(
    c: &mut Crusher,
    recipe: Option<&crate::processing::Recipe>,
    max: impl Fn(&str) -> u8,
    rng: &mut Rng,
) {
    let Some((_, count)) = c.slots.first().cloned() else {
        return;
    };
    let mut list: Vec<(String, u8)> = Vec::new();
    if let Some(r) = recipe {
        for _ in 0..count {
            for (item, mut n) in roll(r, rng) {
                // ItemHelper.addToList: onto stacks of the same item first.
                for (have, m) in list.iter_mut() {
                    if n > 0 && *have == item {
                        let moved = n.min(max(have).saturating_sub(*m));
                        *m += moved;
                        n -= moved;
                    }
                }
                if n > 0 {
                    list.push((item, n));
                }
            }
        }
    }
    list.truncate(31);
    c.slots = list;
}

/// `tick` for the held entity: its motion (toward the middle, sinking at up to half a block a
/// tick along the facing), and whether it is far enough in to take (a quarter past the middle).
pub fn hold(
    controller: [i32; 3],
    facing: Dir,
    crushing_speed: f32,
    at: [f64; 3],
) -> ([f64; 3], bool) {
    let centre = controller.map(|c| f64::from(c) + 0.5);
    let offset = if facing.positive() { 1.0 } else { -1.0 };
    let speed = f64::from(crushing_speed * 4.0);
    let movement = (-speed / 4.0).max(-0.5) * -offset;
    let (xm, zm) = ((centre[0] - at[0]) / 2.0, (centre[2] - at[2]) / 2.0);
    let a = match facing.axis() {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    };
    let motion = [
        if a == 0 { movement } else { xm },
        if a == 1 { movement } else { 0.0 },
        if a == 2 { movement } else { zm },
    ];
    let taken = at[a] * -offset < (centre[a] - 0.25) * -offset;
    (motion, taken)
}

/// `BypassCrushingWheel`: an item at or past the controller's output face (where its results are
/// let out) isn't caught again.
pub fn bypassed(controller: [i32; 3], facing: Dir, at: [f64; 3]) -> bool {
    let o = facing.offset();
    (0..3).any(|a| o[a] != 0 && (at[a] - (f64::from(controller[a]) + 0.5)) * f64::from(o[a]) >= 0.5)
}

/// Where and how fast the results leave (`outPos`, `outSpeed`).
pub fn output(controller: [i32; 3], facing: Dir) -> ([f64; 3], [f64; 3]) {
    let o = facing.offset();
    let pos = [0, 1, 2].map(|a| f64::from(controller[a]) + 0.5 + 0.55 * f64::from(o[a]));
    let speed = match facing {
        Dir::Up => [0.0, 0.5, 0.0],
        Dir::Down => [0.0; 3],
        _ => o.map(|v| 0.25 * f64::from(v)),
    };
    (pos, speed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::{Output, Recipe};

    #[test]
    fn wheels_turning_against_each_other_crush_downward() {
        // Two z-axis wheels at x 0 and x 2 (Create: z × x = +y, signed by the turn and the side).
        assert_eq!(
            controller(Axis::Z, Dir::East, -16.0, Some((Axis::Z, 16.0))),
            Some((true, Dir::Down))
        );
        // Seen from the other wheel, the same direction.
        assert_eq!(
            controller(Axis::Z, Dir::West, 16.0, Some((Axis::Z, -16.0))),
            Some((true, Dir::Down))
        );
        // Turned the other way, they throw items up.
        assert_eq!(
            controller(Axis::Z, Dir::East, 16.0, Some((Axis::Z, -16.0))),
            Some((true, Dir::Up))
        );
        assert_eq!(
            controller(Axis::Z, Dir::East, 16.0, Some((Axis::Z, 16.0))),
            Some((false, Dir::Down))
        );
        assert_eq!(
            controller(Axis::Z, Dir::East, 16.0, Some((Axis::X, -16.0))),
            None
        );
        assert_eq!(
            controller(Axis::Z, Dir::South, 16.0, Some((Axis::Z, -16.0))),
            None
        );
    }

    #[test]
    fn a_stack_is_crushed_then_let_out() {
        let ore = Recipe {
            id: "create:crushing/iron_ore".into(),
            ingredients: vec![vec!["minecraft:iron_ore".into()]],
            results: vec![Output {
                item: "create:crushed_raw_iron".into(),
                count: 1,
                chance: 1.0,
            }],
            time: 250,
        };
        let mut c = Crusher::default();
        intake(&mut c, "minecraft:iron_ore", 2, Some(250));
        // 64 rpm: crushing speed 1.28, 5.12 a tick over log2(2) = 1.
        let mut ticks = 0;
        while tick_inventory(&mut c, crushing_speed(64.0)) != Step::Apply {
            ticks += 1;
        }
        assert_eq!(ticks, 44, "250 - 45 × 5.12 < 20");
        apply(&mut c, Some(&ore), |_| 64, &mut Rng::new(1));
        assert_eq!(c.slots, vec![("create:crushed_raw_iron".to_owned(), 2)]);
        while tick_inventory(&mut c, crushing_speed(64.0)) != Step::Output {}
        // A single item divides by log2(1) = 0: the fastest, 20 a tick.
        let mut one = Crusher::default();
        intake(&mut one, "minecraft:stone", 1, None);
        tick_inventory(&mut one, 0.32);
        assert_eq!(one.remaining, 80.0);
    }

    #[test]
    fn a_falling_item_is_held_and_taken() {
        let (v, taken) = hold([0, 0, 0], Dir::Down, 1.28, [0.7, 0.6, 0.5]);
        assert_eq!(v, [-0.09999999999999998, -0.5, 0.0]);
        assert!(!taken);
        assert!(hold([0, 0, 0], Dir::Down, 1.28, [0.5, 0.2, 0.5]).1);
        assert_eq!(
            output([0, 0, 0], Dir::Down),
            ([0.5, -0.050000000000000044, 0.5], [0.0; 3])
        );
    }
}
