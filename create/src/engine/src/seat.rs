//! Create's seats (`SeatBlock`, `SeatEntity`): a click seats the player on an invisible seat
//! entity at the block, a mob that lands on one sits down (`updateEntityAfterFallOn`), and the
//! seat goes, letting its rider off, once it has none or its block is gone (`SeatEntity.tick`).
//! The seat's passenger point is above its anchor; the dyes are `dye::SEATS`.

/// `SeatEntity.build`: `sized(0.25f, 0.35f)`; riders sit on its top (`getPassengerRidingPosition`:
/// its height above its position).
pub const HEIGHT: f32 = 0.35;
/// `SeatEntity.positionRider`: an extra sixteenth of a block above the ride point.
pub const PASSENGER_HEIGHT: f64 = HEIGHT as f64 + 1.0 / 16.0;

/// `SeatEntity.getCustomEntitySeatOffset`: how much higher some mobs sit, by their type
/// (`instanceof`: a cave spider is a spider, a magma cube a slime).
pub fn custom_offset(kind: &str) -> f64 {
    let k = kind.strip_prefix("minecraft:").unwrap_or(kind);
    match k {
        "slime" | "magma_cube" => 0.0,
        "parrot" | "cat" => f64::from(1.0f32 / 12.0),
        "skeleton" => f64::from(1.0f32 / 8.0),
        "wolf" => f64::from(1.0f32 / 16.0),
        "frog" => f64::from(1.5f32 / 16.0),
        "spider" | "cave_spider" => 1.0 / 8.0,
        _ => 0.0,
    }
}

/// The bottom centre of the seat block, plus the rider-specific `positionRider` offset.
/// The general entity contributes the constant part via its passenger attachment.
pub fn seat_anchor(pos: [i32; 3], kind: &str) -> [f64; 3] {
    [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + custom_offset(kind),
        f64::from(pos[2]) + 0.5,
    ]
}

/// Where a rider gets off (`getDismountLocationForPassenger`): the top of the seat's box, which
/// `setPos` centres on its position, plus half a block.
pub fn dismount_at(pos: [i32; 3]) -> [f64; 3] {
    [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + f64::from(HEIGHT) / 2.0 + f64::from(0.5f32),
        f64::from(pos[2]) + 0.5,
    ]
}

/// `SeatBlock.canBePickedUp` for an entity of `kind` that is living: not a shulker nor a player
/// (`seatHostileMobs` is on by default, so hostile mobs sit too). Create's `IGNORE_SEAT` tag
/// (`#c:capturing_not_supported`) has no reader here.
/// `SeatBlock.fallOn`: `super.fallOn(.., fallDistance * 0.5F)`, the damage of a fall of half
/// `distance` blocks, from `damage`, the game's for the whole of it
/// (`LivingEntity.calculateFallDamage`: `ceil((distance - safeFallDistance) * multiplier)`).
/// The safe distance is vanilla's 3 when `damage` is what that gives, else the one `damage`
/// implies at a multiplier of 1 (jump boost raises it). No damage stays none (immune, creative).
pub fn fall_damage(distance: f64, damage: f32) -> f32 {
    if damage <= 0.0 {
        return 0.0;
    }
    let safe = if (distance - 3.0).ceil() as f32 == damage {
        3.0
    } else {
        distance - f64::from(damage)
    };
    (distance * 0.5 - safe).ceil().max(0.0) as f32
}

pub fn can_sit(kind: &str) -> bool {
    !matches!(kind, "minecraft:shulker" | "minecraft:player")
}

/// `SeatBlock.updateEntityAfterFallOn` for a living entity of `kind` landing (`on_ground`) on
/// the seat at `pos`, its feet at `feet`, not riding anything: it sits when it may, the block it
/// stands in is that seat and no seat is there yet.
pub fn sits_down(
    pos: [i32; 3],
    kind: &str,
    feet: [f64; 3],
    on_ground: bool,
    riding: bool,
    occupied: bool,
) -> bool {
    on_ground && !riding && !occupied && can_sit(kind) && feet.map(|v| v.floor() as i32) == pos
}

/// SeatBlock.fallOn passes half the fall distance to the ordinary fall calculation.
pub fn fall_distance(distance: f64) -> f64 {
    distance * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seat_counts_half_the_fall() {
        // 10 blocks: 7 on the ground, 5 blocks' 2 on a seat; 5 blocks: 2, then none.
        assert_eq!(fall_damage(10.0, 7.0), 2.0);
        assert_eq!(fall_damage(5.0, 2.0), 0.0);
        assert_eq!(fall_damage(20.5, 18.0), 8.0);
        // Jump boost II (safe distance 5): 12 blocks give 7, 6 blocks give 1.
        assert_eq!(fall_damage(12.0, 7.0), 1.0);
        assert_eq!(fall_damage(30.0, 0.0), 0.0);
    }

    #[test]
    fn riders_sit_where_create_s_seat_puts_them() {
        // A player (vehicle attachment 0.6) on a seat at y 64: feet at 64 + 1/16 + 0.35 - 0.6.
        let s = seat_anchor([3, 64, -2], "minecraft:player");
        assert_eq!((s[0], s[2]), (3.5, -1.5));
        assert_eq!(s[1], 64.0);
        assert_eq!(
            s[1] + PASSENGER_HEIGHT - 0.6,
            64.0 + 0.0625 + f64::from(0.35_f32) - 0.6
        );
        // A skeleton sits an eighth higher, a cave spider as a spider, a pig as anyone.
        let k = seat_anchor([0, 0, 0], "minecraft:skeleton")[1];
        assert_eq!(k, 0.125);
        assert_eq!(custom_offset("minecraft:cave_spider"), 0.125);
        assert_eq!(custom_offset("minecraft:pig"), 0.0);
        assert!((custom_offset("minecraft:frog") - 0.09375).abs() < 1e-9);
        // Getting off: the seat's half-height box top plus half a block.
        assert!((dismount_at([0, 10, 0])[1] - 10.675).abs() < 1e-6);
    }

    #[test]
    fn mobs_landing_on_a_free_seat_sit_down() {
        let pos = [5, 64, 5];
        let feet = [5.4, 64.5, 5.7];
        assert!(sits_down(pos, "minecraft:pig", feet, true, false, false));
        assert!(sits_down(pos, "minecraft:zombie", feet, true, false, false));
        // Not a shulker or a player; not in the air, riding, on a taken seat or beside it.
        assert!(!sits_down(
            pos,
            "minecraft:shulker",
            feet,
            true,
            false,
            false
        ));
        assert!(!sits_down(
            pos,
            "minecraft:player",
            feet,
            true,
            false,
            false
        ));
        assert!(!sits_down(pos, "minecraft:pig", feet, false, false, false));
        assert!(!sits_down(pos, "minecraft:pig", feet, true, true, false));
        assert!(!sits_down(pos, "minecraft:pig", feet, true, false, true));
        assert!(!sits_down(
            pos,
            "minecraft:pig",
            [6.1, 64.5, 5.7],
            true,
            false,
            false
        ));
    }
}
