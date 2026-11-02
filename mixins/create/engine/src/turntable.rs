//! Create's turntable (`TurntableBlock.entityInside`, `TurntableHandler`): what stands on a
//! turning turntable is turned with it. Mobs, items and other entities turn by 3/10 of its speed
//! a tick; a player turns on their own client (`TurntableHandler`, each frame) by 2/3 of it,
//! less away from the centre; a player whose middle is off the turntable (standing on its edge)
//! is carried round it.

use crate::ejector::rotate_y;

/// `entityInside`'s `getSpeed() * 3 / 10`: degrees a tick.
pub fn mob_turn(rpm: f32) -> f32 {
    rpm * 3.0 / 10.0
}

/// `entityInside`'s checks: on the ground, not going up, its feet at least halfway up the block.
pub fn stands_on(pos: [i32; 3], feet: [f64; 3], on_ground: bool, rising: f64) -> bool {
    on_ground && rising <= 0.0 && feet[1] >= f64::from(pos[1] as f32 + 0.5)
}

/// `entityInside` for a player whose middle is in another block: the motion that carries them
/// round the turntable's centre by the turn (`speed` = `mob_turn`, at most 16° a tick).
pub fn carry(pos: [i32; 3], feet: [f64; 3], speed: f32) -> [f64; 3] {
    let origin = pos.map(|v| f64::from(v) + 0.5);
    let offset = [0, 1, 2].map(|a| feet[a] - origin[a]);
    let turned = rotate_y(offset, f64::from(speed.clamp(-16.0, 16.0)));
    [0, 1, 2].map(|a| origin[a] + turned[a] - feet[a])
}

/// `TurntableHandler.gameRenderFrame` summed over a tick: how far the player standing on the
/// turntable at `pos` (its `getOnPos`) turns a tick. Each frame turns `speed * 2/3` times the
/// frame's length in ticks times the partial tick, which over a tick's frames averages a half;
/// beyond a quarter block from the centre it fades to nothing at half a block.
pub fn player_turn(rpm: f32, pos: [i32; 3], feet: [f64; 3]) -> f32 {
    let mut speed = rpm * (2.0 / 3.0) * 0.5;
    let origin = pos.map(|v| f64::from(v) + 0.5);
    let length = (0..3)
        .map(|a| (feet[a] - origin[a]).powi(2))
        .sum::<f64>()
        .sqrt();
    if length > 0.25 {
        speed *= ((0.5 - length) * 2.0).clamp(0.0, 1.0) as f32;
    }
    speed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobs_turn_three_tenths_of_its_speed_when_standing_on_it() {
        assert_eq!(mob_turn(32.0), 9.6);
        assert_eq!(mob_turn(-16.0), -4.8);
        let pos = [0, 64, 0];
        assert!(stands_on(pos, [0.5, 64.5, 0.5], true, 0.0));
        assert!(!stands_on(pos, [0.5, 64.5, 0.5], false, 0.0), "in the air");
        assert!(!stands_on(pos, [0.5, 64.5, 0.5], true, 0.1), "jumping");
        assert!(
            !stands_on(pos, [0.5, 64.2, 0.5], true, 0.0),
            "below its top half"
        );
    }

    #[test]
    fn players_turn_by_their_distance_from_the_centre() {
        let pos = [0, 64, 0];
        // At the centre: 2/3 of the speed, half of it on average over a tick's frames.
        assert!((player_turn(30.0, pos, [0.5, 64.5, 0.5]) - 10.0).abs() < 1e-5);
        // Within a quarter block: the same; past it `(1/2 - d) * 2` of it (a quarter at 0.375);
        // at half a block nothing.
        assert!((player_turn(30.0, pos, [0.7, 64.5, 0.5]) - 10.0).abs() < 1e-5);
        assert!((player_turn(30.0, pos, [0.875, 64.5, 0.5]) - 2.5).abs() < 1e-5);
        assert_eq!(player_turn(30.0, pos, [1.0, 64.5, 0.5]), 0.0);
    }

    #[test]
    fn players_on_the_edge_are_carried_round() {
        // A quarter turn (clamped to 16°) of a player 0.75 east of the centre: VecHelper.rotate
        // about y moves +x toward -z for a positive angle.
        let m = carry([0, 64, 0], [1.25, 64.5, 0.5], 16.0);
        let a = 16f64.to_radians();
        assert!((m[0] - (0.75 * a.cos() - 0.75)).abs() < 1e-3);
        assert!((m[2] - (-0.75 * a.sin())).abs() < 1e-3);
        assert_eq!(m[1], 0.0);
        assert_eq!(carry([0, 64, 0], [1.25, 64.5, 0.5], 40.0), m, "at most 16°");
    }
}
