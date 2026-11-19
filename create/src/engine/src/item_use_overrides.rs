//! Create's `ItemUseOverrides` (`foundation/block/ItemUseOverrides.java`): the blocks whose own
//! use takes a click over the held item. `onRegister(ItemUseOverrides::addBlock)` registers the
//! hand crank (`AllBlocks.HAND_CRANK`), every valve handle (`BuilderTransformers.valveHandle`)
//! and the analog lever (`AllBlocks.ANALOG_LEVER`); on `PlayerInteractEvent.RightClickBlock`,
//! which fires before the game hands a sneaking player's click to the item, the class invokes
//! the block's own use (`BlockHelper.invokeUse`) and cancels with its result — unless the hand
//! holds the wrench, whose own use wins. The glue consults this from its `right_click_block`.

use crate::Block;
use crate::dye;

/// `AllItems.WRENCH`.
const WRENCH: &str = "create:wrench";

/// `OVERRIDES`, the blocks registered with `addBlock`: the hand crank, every valve handle and
/// the analog lever.
pub fn contains(block: Block) -> bool {
    matches!(block, Block::HandCrank | Block::AnalogLever) || dye::is_valve_handle(block)
}

/// Whether `ItemUseOverrides.onBlockActivated` invokes the block's own use for a click on
/// `block` with `held` in the clicking hand: every override block whatever it holds, but never
/// the wrench (`if (AllItems.WRENCH.isIn(event.getItemStack())) return;`).
pub fn invokes_use(block: Block, held: Option<&str>) -> bool {
    held != Some(WRENCH) && contains(block)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AllBlocks` registers the hand crank (`AllBlocks.java`, `HAND_CRANK`), every valve
    /// handle (`BuilderTransformers.valveHandle`: the copper one and all sixteen colours) and
    /// the analog lever (`ANALOG_LEVER`) — no other block.
    #[test]
    fn the_overrides_are_the_crank_the_valve_handles_and_the_lever() {
        let mut names: Vec<&str> = Block::ALL
            .iter()
            .filter(|b| contains(**b))
            .map(|b| b.name())
            .collect();
        names.sort_unstable();
        let mut expected: Vec<&str> = [Block::CopperValveHandle]
            .into_iter()
            .chain(dye::VALVE_HANDLES)
            .map(|b| b.name())
            .chain(["analog_lever", "hand_crank"])
            .collect();
        expected.sort_unstable();
        assert_eq!(names, expected);
    }

    /// `onBlockActivated` hands the click to the block's use whatever the hand holds but the
    /// wrench: the crank turns for a sneaking extendo grip (create-horizon issue #96's case,
    /// where Java answers `Crank { in_use: 9, backwards: true }`), the valve handles and the
    /// lever the same, and the wrench keeps its own use on every one of them.
    #[test]
    fn the_blocks_use_wins_over_any_held_item_but_the_wrench() {
        for held in [None, Some("create:extendo_grip"), Some("minecraft:stick")] {
            assert!(invokes_use(Block::HandCrank, held));
            assert!(invokes_use(Block::CopperValveHandle, held));
            assert!(invokes_use(dye::VALVE_HANDLES[0], held));
            assert!(invokes_use(Block::AnalogLever, held));
        }
        for block in [
            Block::HandCrank,
            Block::CopperValveHandle,
            dye::VALVE_HANDLES[3],
            Block::AnalogLever,
        ] {
            assert!(!invokes_use(block, Some("create:wrench")));
        }
        // No other block's use is invoked: those clicks keep the game's own dispatch.
        for held in [None, Some("create:extendo_grip"), Some("create:wrench")] {
            assert!(!invokes_use(Block::Shaft, held));
            assert!(!invokes_use(Block::CreativeMotor, held));
            assert!(!invokes_use(Block::RedstoneLink, held));
        }
    }
}
