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
    }
}
