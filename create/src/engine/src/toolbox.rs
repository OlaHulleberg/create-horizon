//! Toolboxes (`ToolboxBlockEntity`, `ToolboxInventory`): eight compartments of four stacks
//! (`STACKS_PER_COMPARTMENT`), 32 slots, kept as a container a player opens and hoppers use.
//! Horizon's mod menus are vanilla slot grids: the container has 36 slots for a four-row grid, the
//! four past Create's 32 are hidden from the menu and from hoppers, and the screen shows each
//! compartment by its first stack, as Create's does (its other stacks are filled by hoppers and
//! the screen's deposit button, not by clicks: a slot holds one stack, not a compartment's 256).

/// `ToolboxInventory.STACKS_PER_COMPARTMENT`.
pub const STACKS_PER_COMPARTMENT: usize = 4;
/// Create's slots: 8 compartments.
pub const STORED: usize = 8 * STACKS_PER_COMPARTMENT;
/// The container's slots: a `minecraft:generic_9x4` menu's.
pub const SLOTS: usize = 36;
/// The menu the container opens.
pub const MENU: &str = "minecraft:generic_9x4";

/// The screen (`ToolboxScreen`): the toolbox's background (`AllGuiTextures.TOOLBOX`, 188 x 171)
/// 30 px in from the window's left, the player's inventory (`PLAYER_INVENTORY`, 176 x 108)
/// overlapping its last 24 px, the player's slots from (8, 165) (`addPlayerSlots(8, 165)`).
pub const SCREEN_WIDTH: f32 = 30.0 + BACKGROUND[0];
pub const SCREEN_HEIGHT: f32 = BACKGROUND[1] + 108.0 - 24.0;
pub const INVENTORY_Y: f32 = 165.0;
pub const INVENTORY_X: f32 = 8.0;
/// `AllGuiTextures.TOOLBOX`'s size.
pub const BACKGROUND: [f32; 2] = [188.0, 171.0];

/// Where the screen shows container slot `slot` (`ToolboxMenu.addSlots`): each compartment's
/// first stack is its one visible slot, round the toolbox's picture; its other three stacks and
/// the slots past Create's 32 are hidden (Create's sit at -10000).
pub fn slot_position(slot: usize) -> Option<[f32; 2]> {
    const X: [f32; 8] = [79.0, 112.0, 145.0, 151.0, 145.0, 112.0, 79.0, 73.0];
    const Y: [f32; 8] = [37.0, 31.0, 37.0, 70.0, 103.0, 109.0, 103.0, 70.0];
    if slot >= STORED || !slot.is_multiple_of(STACKS_PER_COMPARTMENT) {
        return None;
    }
    let compartment = slot / STACKS_PER_COMPARTMENT;
    Some([X[compartment], Y[compartment]])
}

/// `CEquipment.toolboxRange`: how far a player reaches toolboxes from (10 blocks).
pub const RANGE: f64 = 10.0;
/// `ToolboxHandler.validationTimer`: equipped slots are checked against their toolbox this often.
pub const VALIDATION: u64 = 20;
/// `RadialToolboxMenu`'s centre (unequip) and deposit choices.
pub const UNEQUIP: i32 = -5;
pub const DEPOSIT: i32 = -7;

/// `ToolboxHandler.distance`: the squared distance from a player at `at` to the toolbox at
/// `pos` (its middle, at its bottom).
pub fn distance(at: [f64; 3], pos: [i32; 3]) -> f64 {
    let d = [
        at[0] - (f64::from(pos[0]) + 0.5),
        at[1] - f64::from(pos[1]),
        at[2] - (f64::from(pos[2]) + 0.5),
    ];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// `ToolboxHandler.withinRange`.
pub fn in_range(at: [f64; 3], pos: [i32; 3]) -> bool {
    distance(at, pos) < RANGE * RANGE
}

/// `tickPlayers`: an equipped hotbar slot is kept at half its stack size (rounded up).
pub fn target_amount(max_stack: u32) -> u32 {
    max_stack.div_ceil(2)
}

/// `RadialToolboxMenu.renderWindow`: the ring slot under the mouse, `d` from the screen's
/// middle (none within 5 pixels or 100 and more away). Slot 0 is up and to the left of the
/// top, going clockwise.
pub fn hovered(d: [f32; 2]) -> Option<usize> {
    let distance = d[0] * d[0] + d[1] * d[1];
    if !(distance > 25.0 && distance < 10000.0) {
        return None;
    }
    let deg = d[1].atan2(d[0]).to_degrees();
    let v = (deg + 360.0 + 180.0 - 22.5).floor() as i32;
    Some((v.rem_euclid(360) / 45) as usize)
}

/// Where ring slot `slot` sits from the middle (`rotateZDegrees(slot * 45 - 45)`, 40 out).
pub fn ring_offset(slot: usize, out: f32) -> [f32; 2] {
    let a = ((slot as f32) * 45.0 - 45.0).to_radians();
    // (0, -out) turned by `a`.
    [out * a.sin(), -out * a.cos()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_counts_clockwise_from_the_top_left() {
        // Up and left (-45 degrees from the top) is slot 0, the top 1, the right 3.
        assert_eq!(hovered([-30.0, -30.0]), Some(0));
        assert_eq!(hovered([0.0, -40.0]), Some(1));
        assert_eq!(hovered([40.0, 0.0]), Some(3));
        assert_eq!(hovered([0.0, 40.0]), Some(5));
        assert_eq!(hovered([-40.0, 0.0]), Some(7));
        assert_eq!(hovered([1.0, 1.0]), None);
        assert_eq!(hovered([200.0, 0.0]), None);
        for slot in 0..8 {
            let o = ring_offset(slot, 40.0);
            assert_eq!(hovered(o), Some(slot), "slot {slot} at {o:?}");
        }
        assert_eq!(target_amount(64), 32);
        assert_eq!(target_amount(1), 1);
        assert_eq!(target_amount(16), 8);
        assert!(in_range([0.5, 0.0, 9.0], [0, 0, 0]) && !in_range([0.5, 0.0, 10.6], [0, 0, 0]));
    }

    #[test]
    fn each_compartment_shows_its_first_stack_round_the_toolbox() {
        // `ToolboxMenu`: x = 79, y = 37, then clockwise from the top left.
        assert_eq!(slot_position(0), Some([79.0, 37.0]));
        assert_eq!(slot_position(4), Some([112.0, 31.0]));
        assert_eq!(slot_position(12), Some([151.0, 70.0]));
        assert_eq!(slot_position(28), Some([73.0, 70.0]));
        assert_eq!(slot_position(1), None);
        assert_eq!(slot_position(31), None);
        assert_eq!(slot_position(32), None);
        assert_eq!((SCREEN_WIDTH, SCREEN_HEIGHT), (218.0, 255.0));
    }
}
