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

/// `CuckooClockRenderer`'s door angle at animation tick `value`: from closed to 135 degrees in
/// the 5 ticks before each phase (every 15 from 30 to 60, every 3 for the surprise), held a
/// third of the step, back to closed in the next 5.
pub fn door_angle(kind: Animation, value: f32) -> f32 {
    let step = if kind == Animation::Surprise { 3 } else { 15 };
    let lerp = |t: f32, a: f32, b: f32| a + t * (b - a);
    let mut angle = 0.0;
    let mut phase = 30;
    while phase <= 60 {
        let local = value - phase as f32;
        let third = (step / 3) as f32;
        if local < -third {
        } else if local < 0.0 {
            angle = lerp((value - (phase - 5) as f32) / 5.0, 0.0, 135.0);
        } else if local < third {
            angle = 135.0;
        } else if local < 2.0 * third {
            angle = lerp((value - (phase + 5) as f32) / 5.0, 135.0, 0.0);
        }
        phase += step;
    }
    angle
}

/// How far out the figure stands with the doors at `angle` (`-(angle / 135) / 2 + 10 / 16`).
pub fn figure_offset(angle: f32) -> f32 {
    -(angle / 135.0) / 2.0 + 10.0 / 16.0
}

/// A part drawn turned about its block's centre on y: its model's pose (the clock's facing), the
/// angle about y and the offset that puts its pivot where Create's renderer has it.
pub struct YPose {
    pub pose: glam::Mat4,
    pub angle: f32,
    pub offset: [f32; 3],
}

/// The clock's turn (`rotateCentered(horizontalAngle(facing.getCounterClockWise()), UP)`).
fn clock_turn(facing: Dir) -> (glam::Mat4, f32) {
    use glam::{Mat4, Vec3};
    let ry = crate::ejector::horizontal_angle(counter_clockwise(facing)).to_radians();
    let c = Vec3::splat(0.5);
    (
        Mat4::from_translation(c) * Mat4::from_rotation_y(ry) * Mat4::from_translation(-c),
        ry,
    )
}

/// `rotateDoor`: the door turns `angle` (the left one the other way) about its hinge, (2, y, 6) or
/// (2, y, 10) px in the clock's turned frame. Turning about the hinge is turning about the
/// centre and a shift, constant for an angle: `M(c) - c`.
pub fn door_pose(facing: Dir, left: bool, angle: f32) -> YPose {
    use glam::{Mat4, Vec3};
    let (turn, _) = clock_turn(facing);
    let a = if left { -angle } else { angle };
    let p = Vec3::new(2.0 / 16.0, 0.0, if left { 6.0 } else { 10.0 } / 16.0);
    let m = turn
        * Mat4::from_translation(p)
        * Mat4::from_rotation_y(a.to_radians())
        * Mat4::from_translation(-p);
    let c = Vec3::splat(0.5);
    let o = m.transform_point3(c) - c;
    YPose {
        pose: turn,
        angle: a,
        offset: o.to_array(),
    }
}

/// The figure (`rotateCentered` to the facing, then `translate(offset, 0, 0)`): the facing's
/// pose and the shift along the turned x.
pub fn figure_pose(facing: Dir, offset: f32) -> YPose {
    let (turn, ry) = clock_turn(facing);
    let o = glam::Mat4::from_rotation_y(ry).transform_vector3(glam::Vec3::new(offset, 0.0, 0.0));
    YPose {
        pose: turn,
        angle: 0.0,
        offset: o.to_array(),
    }
}

/// `CuckooClockBlockEntity.Animation`: what comes out of the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Animation {
    Pig,
    Creeper,
    /// The mysterious clock's (`CuckooClockBlock.containsSurprise`): it blows up.
    Surprise,
}

impl Animation {
    pub fn index(self) -> u8 {
        match self {
            Animation::Pig => 0,
            Animation::Creeper => 1,
            Animation::Surprise => 2,
        }
    }
    pub fn from_index(i: u8) -> Option<Animation> {
        Some(match i {
            0 => Animation::Pig,
            1 => Animation::Creeper,
            2 => Animation::Surprise,
            _ => return None,
        })
    }
}

/// `tick` on the server with no animation running: the pig at 12:00 to 12:04, the creeper at
/// 18:32 to 18:35; a mysterious clock's is always the surprise (`startAnimation`).
pub fn due(hours: i32, minutes: i32, mysterious: bool) -> Option<Animation> {
    let a = if hours == 12 && minutes < 5 {
        Animation::Pig
    } else if hours == 18 && minutes < 36 && minutes > 31 {
        Animation::Creeper
    } else {
        return None;
    };
    Some(if mysterious { Animation::Surprise } else { a })
}

/// One server tick of a running animation (`animationProgress` from 0): it ends once past
/// 100; true when the surprise goes off now (its progress reaching 50).
pub fn step(kind: Animation, progress: &mut u32) -> (bool, bool) {
    let value = *progress;
    *progress = value + 1;
    let running = value <= 100;
    (running, kind == Animation::Surprise && *progress == 50)
}

/// A client's sound at animation tick `value` (`tick` on the client: the chimes, the doors
/// opening and closing, the pig or creeper and the surprise's fuse): id, volume, pitch.
pub fn sounds(kind: Animation, value: u32) -> Vec<(&'static str, f32, f32)> {
    let mut out = Vec::new();
    if value == 1 {
        out.push(("minecraft:block.note_block.chime", 2.0, 0.5));
    }
    if value == 21 {
        out.push(("minecraft:block.note_block.chime", 2.0, 0.793_701));
    }
    let surprise = kind == Animation::Surprise;
    if value == 40 && surprise {
        out.push(("minecraft:entity.tnt.primed", 1.0, 1.0));
    }
    let step = if surprise { 3 } else { 15 };
    let mut phase = 30;
    while phase <= 60 {
        if value == phase - step / 3 {
            out.push(("minecraft:block.chest.open", 1.0 / 16.0, 2.0));
        }
        if value == phase {
            out.push(if kind == Animation::Pig {
                ("minecraft:entity.pig.ambient", 0.25, 1.0)
            } else {
                ("minecraft:entity.creeper.hurt", 0.25, 3.0)
            });
        }
        if value == phase + step / 3 {
            out.push(("minecraft:block.chest.close", 1.0 / 16.0, 2.0));
        }
        phase += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doors_open_before_each_phase_and_turn_about_their_hinges() {
        assert_eq!(door_angle(Animation::Pig, 10.0), 0.0);
        assert_eq!(door_angle(Animation::Pig, 27.5), 67.5);
        assert_eq!(door_angle(Animation::Pig, 30.0), 135.0);
        assert_eq!(door_angle(Animation::Pig, 37.5), 67.5);
        // Closed between phases, opening again for 45.
        assert_eq!(door_angle(Animation::Pig, 40.0), 0.0);
        assert_eq!(door_angle(Animation::Pig, 41.0), 27.0);
        assert_eq!(figure_offset(135.0), 10.0 / 16.0 - 0.5);
        // The GPU turns (pose * v - c) about y by `angle` and adds `offset`: that is Create's
        // transform for every vertex, the hinge staying where it is.
        for facing in [Dir::North, Dir::East, Dir::South, Dir::West] {
            for left in [true, false] {
                let d = door_pose(facing, left, 135.0);
                let (turn, _) = clock_turn(facing);
                let hinge = glam::Vec3::new(2.0 / 16.0, 0.3, if left { 6.0 } else { 10.0 } / 16.0);
                let c = glam::Vec3::splat(0.5);
                let gpu = |v: glam::Vec3| {
                    glam::Mat4::from_rotation_y(d.angle.to_radians())
                        .transform_vector3(d.pose.transform_point3(v) - c)
                        + c
                        + glam::Vec3::from(d.offset)
                };
                let want = turn.transform_point3(hinge);
                assert!((gpu(hinge) - want).length() < 1e-5, "{facing:?} {left}");
            }
        }
    }

    #[test]
    fn the_pig_comes_at_noon_the_creeper_at_dusk_and_the_surprise_goes_off_at_50() {
        assert_eq!(due(12, 0, false), Some(Animation::Pig));
        assert_eq!(due(12, 5, false), None);
        assert_eq!(due(18, 32, false), Some(Animation::Creeper));
        assert_eq!(due(18, 31, false), None);
        assert_eq!(due(18, 36, false), None);
        assert_eq!(due(12, 2, true), Some(Animation::Surprise));
        assert_eq!(due(9, 0, true), None);
        let (mut p, mut boom, mut ticks) = (0, Vec::new(), 0);
        loop {
            let (running, now) = step(Animation::Surprise, &mut p);
            if now {
                boom.push(p);
            }
            if !running {
                break;
            }
            ticks += 1;
        }
        assert_eq!((boom, ticks), (vec![50], 101));
        // Two chimes, and the pig every 15 from 30 to 60 (30, 45, 60).
        let all: Vec<_> = (0..=101).flat_map(|v| sounds(Animation::Pig, v)).collect();
        assert_eq!(all.iter().filter(|s| s.0.ends_with("chime")).count(), 2);
        assert_eq!(
            all.iter().filter(|s| s.0.ends_with("pig.ambient")).count(),
            3
        );
        let surprise: Vec<_> = (0..=101)
            .flat_map(|v| sounds(Animation::Surprise, v))
            .collect();
        assert_eq!(
            surprise
                .iter()
                .filter(|s| s.0.ends_with("tnt.primed"))
                .count(),
            1
        );
        assert_eq!(
            surprise
                .iter()
                .filter(|s| s.0.ends_with("creeper.hurt"))
                .count(),
            11
        );
    }

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
