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
    if slot >= STORED || slot % STACKS_PER_COMPARTMENT != 0 {
        return None;
    }
    let compartment = slot / STACKS_PER_COMPARTMENT;
    Some([X[compartment], Y[compartment]])
}

#[cfg(test)]
mod tests {
    use super::*;

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
