//! Valve handles (`ValveHandleBlock`, `ValveHandleBlockEntity`): a hand crank a player turns by
//! an angle at a time instead of for as long as they hold it.

use crate::kinetics::convert_to_angular;
use crate::parts::CRANK_RPM;

/// `angleInput.setValue(45)`: the angle a use turns it by, negative counter-clockwise.
pub const DEFAULT_ANGLE: i32 = 45;

/// `cooldown = 4` after each use.
pub const COOLDOWN: u8 = 4;

/// `activate`'s `inUse`: the ticks the angle takes at the copper valve handle's speed, rounded
/// up, and two more ("always overshoot, target will stop early").
pub fn use_ticks(angle: i32) -> u8 {
    let target = f64::from(angle.unsigned_abs());
    let degrees_per_tick = f64::from(convert_to_angular(CRANK_RPM));
    ((target / degrees_per_tick).ceil() as u8).saturating_add(2)
}

/// `activate`'s `targetAngle` from the server's `startAngle` (0 there: its `independentAngle` is
/// only the client's): a quarter turn, or a half one past 135°, the way the angle is signed.
/// The handle is drawn turning from 0 to it while in use and left there.
pub fn target_angle(angle: i32) -> i32 {
    let turn = if angle.unsigned_abs() > 135 { 180 } else { 90 };
    turn * angle.signum()
}

/// The angle a valve handle is drawn at, in degrees about its facing's positive axis
/// (`getIndependentAngle`): from 0 toward `target` over `total` ticks, `elapsed` (with the partial
/// tick) of them gone; flipped when used backwards and for a facing toward an axis' negative end.
pub fn drawn_angle(target: i32, total: u8, elapsed: f32, back: bool, positive_facing: bool) -> f32 {
    let t = if total == 0 {
        1.0
    } else {
        (elapsed / f32::from(total)).clamp(0.0, 1.0)
    };
    let back = if back { -1.0 } else { 1.0 };
    let step = if positive_facing { 1.0 } else { -1.0 };
    target as f32 * t * back * step
}

/// `ValveHandleBlockEntity.clockwise`: a negative angle, flipped when used sneaking.
pub fn clockwise(angle: i32, backwards: bool) -> bool {
    (angle < 0) ^ backwards
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_use_turns_for_its_angle_and_two_ticks_more() {
        // 45° at 9.6° a tick: 4.69, so 5 ticks, and 2 more.
        assert_eq!(use_ticks(DEFAULT_ANGLE), 7);
        assert_eq!(use_ticks(-90), 12);
        assert_eq!(use_ticks(180), 21);
        assert!(!clockwise(45, false));
        assert!(clockwise(45, true));
        assert!(clockwise(-45, false));
        assert_eq!(target_angle(45), 90);
        assert_eq!(target_angle(-136), -180);
        assert_eq!(target_angle(135), 90);
        assert_eq!(drawn_angle(90, 7, 3.5, false, true), 45.0);
        assert_eq!(drawn_angle(90, 7, 9.0, true, false), 90.0);
        assert_eq!(drawn_angle(-180, 21, 0.0, false, true), 0.0);
    }
}

/// `HandCrankBlockEntity.write/read` and `ValveHandleBlockEntity.write/read`.
/// Cooldown is deliberately absent: Java initializes it to zero on load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavedHandle {
    pub in_use: u8,
    pub backwards: bool,
    pub angle: i32,
    pub total_use_ticks: u8,
    pub start_angle: i32,
    pub target_angle: i32,
}

/// Live `HandCrankBlockEntity` / `ValveHandleBlockEntity` use state.
#[derive(Clone, Copy, Default)]
pub(crate) struct Handle {
    pub saved: SavedHandle,
    pub cooldown: u8,
}

impl Default for SavedHandle {
    fn default() -> Self {
        Self {
            in_use: 0,
            backwards: false,
            angle: DEFAULT_ANGLE,
            total_use_ticks: 0,
            start_angle: 0,
            target_angle: 0,
        }
    }
}
