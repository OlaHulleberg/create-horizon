//! What comparators read from Create's blocks (`Block.hasAnalogOutputSignal` and
//! `getAnalogOutputSignal` in each block class, `ItemHelper.calcRedstoneFromInventory`,
//! `ComparatorUtil`, `SharedDepotBlockMethods.getComparatorInputOverride`). The glue gathers a
//! block's state (its slots, its tank, its air) and these give Create's number for it.

use crate::Block;
pub use crate::equipment::fraction_to_redstone_level;

/// The blocks of this port whose Create class answers `hasAnalogOutputSignal` with true:
/// `ItemVaultBlock`, `ToolboxBlock`, `DepotBlock`, `EjectorBlock`, `BasinBlock`,
/// `FrogportBlock`, `PostboxBlock`, `PackagerBlock`, `ItemDrainBlock`, `SpoutBlock`,
/// `FluidTankBlock`, `BacktankBlock`, `GaugeBlock`, `SequencedGearshiftBlock`,
/// `BlazeBurnerBlock`, `LitBlazeBurnerBlock`, `SignalBlock` and `StationBlock`. (Create's
/// portable storage interfaces, controller rail, redstone requester, creative fluid tank and
/// lectern controller have one too, and are not ported.)
pub fn has_analog_output(b: Block) -> bool {
    matches!(
        b,
        Block::ItemVault
            | Block::Depot
            | Block::WeightedEjector
            | Block::Basin
            | Block::PackageFrogport
            | Block::Packager
            | Block::ItemDrain
            | Block::Spout
            | Block::FluidTank
            | Block::CopperBacktank
            | Block::NetheriteBacktank
            | Block::Speedometer
            | Block::Stressometer
            | Block::SequencedGearshift
            | Block::BlazeBurner
            | Block::LitBlazeBurner
            | Block::TrackSignal
            | Block::TrackStation
    ) || crate::dye::is_toolbox(b)
        || crate::dye::is_postbox(b)
}

/// `ItemHelper.calcRedstoneFromInventory` over a handler's slots, each empty or `(count, the
/// stack's max stack size)`, every slot limited to `slot_limit` (an `ItemStackHandler`'s 64, a
/// `SmartInventory`'s stack size). A slot limited to 0 doesn't count; none left reads 0. Float
/// arithmetic as Create's.
pub fn inventory(slots: impl IntoIterator<Item = Option<(u32, u32)>>, slot_limit: u32) -> i32 {
    let mut found = 0;
    let mut f = 0.0f32;
    let mut total = 0;
    for s in slots {
        total += 1;
        if slot_limit == 0 {
            total -= 1;
            continue;
        }
        if let Some((count, max)) = s
            && count > 0
        {
            f += count as f32 / slot_limit.min(max) as f32;
            found += 1;
        }
    }
    if total == 0 {
        return 0;
    }
    f /= total as f32;
    (f * 14.0).floor() as i32 + i32::from(found > 0)
}

/// `SharedDepotBlockMethods.getComparatorInputOverride`: the depot's items (held and outputs,
/// `getPresentStackSize`) over its `maxStackSize` (0 counts as 64): the held stack's max stack
/// size, 64 when it holds none; an ejector's stack-size setting.
pub fn depot(present: u32, max_stack: u32) -> i32 {
    let max = if max_stack == 0 { 64 } else { max_stack };
    let f = present as f32 / max as f32;
    ((f * 14.0).floor() as i32 + i32::from(f > 0.0)).clamp(0, 15)
}

/// `ComparatorUtil.levelOfSmartFluidTank` (item drain, spout): the primary tank's amount over
/// its capacity, as a double.
pub fn smart_tank(amount: u32, capacity: u32) -> i32 {
    fraction_to_redstone_level(f64::from(amount) / f64::from(capacity))
}

/// `FluidTankBlock.getAnalogOutputSignal`: the controller's `getFillState` (a float: the whole
/// tank's amount over its capacity).
pub fn fluid_tank(amount: u32, capacity: u32) -> i32 {
    if capacity == 0 {
        return 0;
    }
    fraction_to_redstone_level(f64::from(amount as f32 / capacity as f32))
}

/// `BlazeBurnerBlock.getAnalogOutputSignal`: `max(0, HEAT_LEVEL.ordinal() - 1)` (none and
/// smouldering 0, fading 1, kindled 2, seething 3).
pub fn blaze_burner(heat: crate::fan::Heat) -> i32 {
    (heat as i32 - 1).max(0)
}

/// `LitBlazeBurnerBlock.getAnalogOutputSignal`: 1 for a regular flame, 2 for a soul flame.
pub fn lit_blaze_burner(soul: bool) -> i32 {
    if soul { 2 } else { 1 }
}

/// `PackagerBlock`, `StationBlock`, `SignalBlock`: 15 or nothing (a box held or animating, a
/// train present, the signal red).
pub fn full(on: bool) -> i32 {
    if on { 15 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventories_read_as_create_counts_them() {
        // A vault block: 20 slots of 64.
        let empty = vec![None; 20];
        assert_eq!(inventory(empty.clone(), 64), 0);
        // One item anywhere: 1 (floor(1/64/20 * 14) + 1).
        let mut one = empty.clone();
        one[7] = Some((1, 64));
        assert_eq!(inventory(one, 64), 1);
        // Half full: floor(0.5 * 14) + 1 = 8.
        let half: Vec<_> = (0..20).map(|i| (i < 10).then_some((64, 64))).collect();
        assert_eq!(inventory(half, 64), 8);
        // Full: 15.
        assert_eq!(inventory(vec![Some((64, 64)); 20], 64), 15);
        // Ender pearls count against their own max stack size (16).
        let pearls: Vec<_> = (0..20).map(|i| (i == 0).then_some((16, 16))).collect();
        assert_eq!(inventory(pearls, 64), 1);
        // A two-block vault of 40 slots with 20 full stacks: 8, as one block of 20 with 10.
        let two: Vec<_> = (0..40).map(|i| (i % 2 == 0).then_some((64, 64))).collect();
        assert_eq!(inventory(two, 64), 8);
        // A toolbox's 32 slots holding 4 stacks of 64 and 2 swords: floor(6/32 * 14) + 1 = 3.
        let tools: Vec<_> = (0..32)
            .map(|i| match i {
                0..4 => Some((64, 64)),
                4..6 => Some((1, 1)),
                _ => None,
            })
            .collect();
        assert_eq!(inventory(tools, 64), 3);
        // A basin's 9 inputs (stack size 64): 3 iron ingots read 1, 9 stacks 15.
        let mut basin = vec![None; 9];
        basin[0] = Some((3, 64));
        assert_eq!(inventory(basin, 64), 1);
        assert_eq!(inventory(vec![Some((64, 64)); 9], 64), 15);
        // A frogport's 18 slots of packages (max stack size 1): 9 packages read 8.
        let port: Vec<_> = (0..18).map(|i| (i < 9).then_some((1, 1))).collect();
        assert_eq!(inventory(port, 64), 8);
        // No slot counts: 0.
        assert_eq!(inventory(vec![Some((5, 64))], 0), 0);
        assert_eq!(inventory(Vec::new(), 64), 0);
    }

    #[test]
    fn depots_read_their_items_over_the_held_stack_size() {
        assert_eq!(depot(0, 64), 0);
        assert_eq!(depot(1, 64), 1);
        // 32 of 64: floor(7) + 1.
        assert_eq!(depot(32, 64), 8);
        assert_eq!(depot(64, 64), 15);
        // Held 16 ender pearls (max 16): full.
        assert_eq!(depot(16, 16), 15);
        // Outputs past the held stack's size can't read more than 15.
        assert_eq!(depot(200, 64), 15);
        // An ejector set to stacks of 8 holding 4: floor(7) + 1; its 0 setting counts as 64.
        assert_eq!(depot(4, 8), 8);
        assert_eq!(depot(4, 0), depot(4, 64));
    }

    #[test]
    fn fluids_read_their_fill() {
        // A spout (1000 mB): empty 0, a drop 1, half 8, full 15.
        assert_eq!(smart_tank(0, 1000), 0);
        assert_eq!(smart_tank(1, 1000), 1);
        assert_eq!(smart_tank(500, 1000), 8);
        assert_eq!(smart_tank(1000, 1000), 15);
        // An item drain (1500 mB) holding a bucket: floor(2/3 * 14 + 1) = 10.
        assert_eq!(smart_tank(1000, 1500), 10);
        // A one-block tank (8000 mB) with a bucket: floor(14/8 + 1) = 2; a 3x3x2 tank (144
        // buckets) half full: 8.
        assert_eq!(fluid_tank(1000, 8000), 2);
        assert_eq!(fluid_tank(72_000, 144_000), 8);
        assert_eq!(fluid_tank(8000, 8000), 15);
        assert_eq!(fluid_tank(0, 8000), 0);
        assert_eq!(fluid_tank(0, 0), 0);
    }

    #[test]
    fn block_states_read_as_create_maps_them() {
        use crate::fan::Heat;
        assert_eq!(blaze_burner(Heat::None), 0);
        assert_eq!(blaze_burner(Heat::Smouldering), 0);
        assert_eq!(blaze_burner(Heat::Fading), 1);
        assert_eq!(blaze_burner(Heat::Kindled), 2);
        assert_eq!(blaze_burner(Heat::Seething), 3);
        assert_eq!((lit_blaze_burner(false), lit_blaze_burner(true)), (1, 2));
        assert_eq!((full(true), full(false)), (15, 0));
        assert!(has_analog_output(Block::ItemVault));
        assert!(has_analog_output(Block::RedToolbox) && has_analog_output(Block::WhitePostbox));
        assert!(!has_analog_output(Block::Shaft) && !has_analog_output(Block::Chute));
    }
}
