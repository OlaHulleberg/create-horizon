//! Toolboxes (`ToolboxBlockEntity`, `ToolboxInventory`): eight compartments of four stacks
//! (`STACKS_PER_COMPARTMENT`), 32 slots, kept as a container a player opens and hoppers use.
//! Horizon's mod menus are vanilla slot grids: the container has 36 slots for a four-row grid, the
//! four past Create's 32 are hidden from the menu and from hoppers, and the screen shows each
//! compartment as a column of its four stacks.

/// `ToolboxInventory.STACKS_PER_COMPARTMENT`.
pub const STACKS_PER_COMPARTMENT: usize = 4;
/// Create's slots: 8 compartments.
pub const STORED: usize = 8 * STACKS_PER_COMPARTMENT;
/// The container's slots: a `minecraft:generic_9x4` menu's.
pub const SLOTS: usize = 36;
/// The menu the container opens.
pub const MENU: &str = "minecraft:generic_9x4";

/// The screen: vanilla's chest screen of four rows (`ChestScreen`: 176 wide, 114 + 18 × rows
/// high, the player's inventory from y 103).
pub const SCREEN_WIDTH: f32 = 176.0;
pub const SCREEN_HEIGHT: f32 = 186.0;
pub const INVENTORY_Y: f32 = 103.0;
pub const INVENTORY_X: f32 = 8.0;

/// Where the screen shows container slot `slot`: compartment `slot / 4` is a column (the eight
/// centred in the nine of a chest row), its stacks top to bottom; the slots past Create's 32
/// are hidden.
pub fn slot_position(slot: usize) -> Option<[f32; 2]> {
    if slot >= STORED {
        return None;
    }
    let (compartment, stack) = (slot / STACKS_PER_COMPARTMENT, slot % STACKS_PER_COMPARTMENT);
    Some([17.0 + compartment as f32 * 18.0, 18.0 + stack as f32 * 18.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compartments_are_columns_of_four_stacks() {
        assert_eq!(slot_position(0), Some([17.0, 18.0]));
        assert_eq!(slot_position(3), Some([17.0, 72.0]));
        assert_eq!(slot_position(4), Some([35.0, 18.0]));
        assert_eq!(slot_position(31), Some([143.0, 72.0]));
        assert_eq!(slot_position(32), None);
        assert_eq!(slot_position(35), None);
        // The last column ends inside the screen's 176 px (7 px border).
        assert!(slot_position(31).unwrap()[0] + 16.0 <= SCREEN_WIDTH - 7.0);
    }
}
