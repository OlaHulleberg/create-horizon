//! Create's cuckoo clock (`CuckooClockBlock`, `CuckooClockBlockEntity`, `CuckooClockRenderer`):
//! a kinetic block whose hands show the day time while it turns. The hands are parts set where
//! the time puts them, drawn again only when the minute shown changes.

use crate::kinetics::{Axis, Dir};

/// `CuckooClockBlockEntity.tick`: the hours and minutes shown at `day_time` (a dimension that
/// isn't `natural` runs 24 times faster).
pub fn time(day_time: i64, natural: bool) -> (i32, i32) {
    let day = (day_time.wrapping_mul(if natural { 1 } else { 24 }) % 24000) as i32;
    let hours = (day / 1000 + 6) % 24;
    let minutes = (day % 1000) * 60 / 1000;
    (hours, minutes)
}

/// `moveHands`: the hour and minute hands' angles in degrees (`360 / 12 * (hours % 12)`,
/// `360 / 60 * minutes`).
pub fn hand_angles(hours: i32, minutes: i32) -> (f32, f32) {
    ((30 * (hours % 12)) as f32, (6 * minutes) as f32)
}

/// `Direction.getCounterClockWise` about y.
pub fn counter_clockwise(d: Dir) -> Dir {
    match d {
        Dir::North => Dir::West,
        Dir::West => Dir::South,
        Dir::South => Dir::East,
        Dir::East => Dir::North,
        d => d,
    }
}

/// How a hand is drawn as a part turning about its block's centre: the model's pose (turned to
/// the clock's facing as `rotateHand` does, then up by 2 px so its pivot (2, 6, 8) px lies on
/// the centre's line), the axis it turns about, the sign of an angle on that axis, and the
/// offset (back down 2 px) the part sits at.
pub struct HandPose {
    pub pose: glam::Mat4,
    pub axis: Axis,
    pub sign: f32,
    pub offset: [f32; 3],
}

/// `CuckooClockRenderer.rotateHand` for a clock facing `facing`.
pub fn hand_pose(facing: Dir) -> HandPose {
    use glam::{Mat4, Vec3};
    let ry = crate::ejector::horizontal_angle(counter_clockwise(facing)).to_radians();
    let centre = Vec3::splat(0.5);
    let turn = Mat4::from_translation(centre)
        * Mat4::from_rotation_y(ry)
        * Mat4::from_translation(-centre);
    let lift = 0.5 - 6.0 / 16.0;
    // The hand turns about the model's x axis (EAST), wherever the clock's turn takes it.
    let x = Mat4::from_rotation_y(ry).transform_vector3(Vec3::X);
    let (axis, along) = if x.x.abs() > 0.5 {
        (Axis::X, x.x)
    } else {
        (Axis::Z, x.z)
    };
    HandPose {
        pose: Mat4::from_translation(Vec3::new(0.0, lift, 0.0)) * turn,
        axis,
        sign: along.signum(),
        offset: [0.0, -lift, 0.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_shown_follows_create_s_clock() {
        // Day time 0 is 6:00; 6000 is noon; 18000 midnight.
        assert_eq!(time(0, true), (6, 0));
        assert_eq!(time(6000, true), (12, 0));
        assert_eq!(time(6500, true), (12, 30));
        assert_eq!(time(18000, true), (0, 0));
        assert_eq!(time(24000 + 1016, true), (7, 0));
        assert_eq!(time(24000 + 1017, true), (7, 1));
        // Not natural: 24 times as fast.
        assert_eq!(time(250, false), (12, 0));
        assert_eq!(hand_angles(12, 0), (0.0, 0.0));
        assert_eq!(hand_angles(15, 45), (90.0, 270.0));
    }

    #[test]
    fn hands_turn_about_the_clock_s_facing_axis_through_their_pivot() {
        for facing in [Dir::North, Dir::South, Dir::East, Dir::West] {
            let h = hand_pose(facing);
            assert_eq!(h.axis, facing.axis());
            // The pivot lands on the centre's line, and the offset puts it back.
            let local = glam::Vec3::new(2.0 / 16.0, 6.0 / 16.0, 0.5);
            let p = h.pose.transform_point3(local);
            let across = match h.axis {
                Axis::X => [p.y, p.z],
                _ => [p.x, p.y],
            };
            assert!((across[0] - 0.5).abs() < 1e-5 && (across[1] - 0.5).abs() < 1e-5);
            assert!((p.y + h.offset[1] - 6.0 / 16.0).abs() < 1e-5);
        }
        // Facing north, the model's x (EAST) turns to +z: angles keep their sign on z.
        let n = hand_pose(Dir::North);
        assert_eq!((n.axis, n.sign), (Axis::Z, 1.0));
        let s = hand_pose(Dir::South);
        assert_eq!((s.axis, s.sign), (Axis::Z, -1.0));
    }

    #[test]
    fn a_clock_is_placed_with_its_back_to_a_shaft_else_facing_the_player() {
        use crate::place::{Context, facing};
        // Yaw 0 looks south: the clock faces north, toward the player.
        let ctx = Context {
            yaw: 0.0,
            pitch: 0.0,
            sneaking: false,
            clicked_face: Dir::Up,
        };
        assert_eq!(
            facing(crate::Block::CuckooClock, ctx, |_| false),
            Some(Dir::North)
        );
        assert_eq!(
            facing(crate::Block::MysteriousCuckooClock, ctx, |d| d == Dir::East),
            Some(Dir::West)
        );
    }
}
