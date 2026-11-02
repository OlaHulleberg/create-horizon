//! Create's dyed block families (`DyedBlockList`: valve handles, toolboxes, postboxes, nixie
//! tubes) and vanilla's `DyeColor.getColor` for the item a player recolours one with.

use crate::Block;

/// `DyeColor`, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DyeColor {
    White,
    Orange,
    Magenta,
    LightBlue,
    Yellow,
    Lime,
    Pink,
    Gray,
    LightGray,
    Cyan,
    Purple,
    Blue,
    Brown,
    Green,
    Red,
    Black,
}

impl DyeColor {
    pub const ALL: [DyeColor; 16] = [
        DyeColor::White,
        DyeColor::Orange,
        DyeColor::Magenta,
        DyeColor::LightBlue,
        DyeColor::Yellow,
        DyeColor::Lime,
        DyeColor::Pink,
        DyeColor::Gray,
        DyeColor::LightGray,
        DyeColor::Cyan,
        DyeColor::Purple,
        DyeColor::Blue,
        DyeColor::Brown,
        DyeColor::Green,
        DyeColor::Red,
        DyeColor::Black,
    ];
    const NAMES: [&'static str; 16] = [
        "white",
        "orange",
        "magenta",
        "light_blue",
        "yellow",
        "lime",
        "pink",
        "gray",
        "light_gray",
        "cyan",
        "purple",
        "blue",
        "brown",
        "green",
        "red",
        "black",
    ];

    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    /// `DyeColor.getColor(stack)` for a vanilla dye (`DyeItem`): `minecraft:<colour>_dye`.
    /// NeoForge also reads the `c:dyes/<colour>` tags; items only those tags name (another
    /// mod's dyes) are not seen here.
    pub fn of_item(item: &str) -> Option<DyeColor> {
        let colour = item.strip_prefix("minecraft:")?.strip_suffix("_dye")?;
        Self::NAMES
            .iter()
            .position(|n| *n == colour)
            .map(|i| Self::ALL[i])
    }
}

/// `AllBlocks.DYED_VALVE_HANDLES`, by colour (the copper one has none).
pub const VALVE_HANDLES: [Block; 16] = [
    Block::WhiteValveHandle,
    Block::OrangeValveHandle,
    Block::MagentaValveHandle,
    Block::LightBlueValveHandle,
    Block::YellowValveHandle,
    Block::LimeValveHandle,
    Block::PinkValveHandle,
    Block::GrayValveHandle,
    Block::LightGrayValveHandle,
    Block::CyanValveHandle,
    Block::PurpleValveHandle,
    Block::BlueValveHandle,
    Block::BrownValveHandle,
    Block::GreenValveHandle,
    Block::RedValveHandle,
    Block::BlackValveHandle,
];

/// `AllBlocks.TOOLBOXES`, by colour.
pub const TOOLBOXES: [Block; 16] = [
    Block::WhiteToolbox,
    Block::OrangeToolbox,
    Block::MagentaToolbox,
    Block::LightBlueToolbox,
    Block::YellowToolbox,
    Block::LimeToolbox,
    Block::PinkToolbox,
    Block::GrayToolbox,
    Block::LightGrayToolbox,
    Block::CyanToolbox,
    Block::PurpleToolbox,
    Block::BlueToolbox,
    Block::BrownToolbox,
    Block::GreenToolbox,
    Block::RedToolbox,
    Block::BlackToolbox,
];

/// `AllBlocks.PACKAGE_POSTBOXES`, by colour.
pub const POSTBOXES: [Block; 16] = [
    Block::WhitePostbox,
    Block::OrangePostbox,
    Block::MagentaPostbox,
    Block::LightBluePostbox,
    Block::YellowPostbox,
    Block::LimePostbox,
    Block::PinkPostbox,
    Block::GrayPostbox,
    Block::LightGrayPostbox,
    Block::CyanPostbox,
    Block::PurplePostbox,
    Block::BluePostbox,
    Block::BrownPostbox,
    Block::GreenPostbox,
    Block::RedPostbox,
    Block::BlackPostbox,
];

/// `AllBlocks.NIXIE_TUBES`, by colour (orange is `ORANGE_NIXIE_TUBE`, `nixie_tube`).
pub const NIXIE_TUBES: [Block; 16] = [
    Block::WhiteNixieTube,
    Block::NixieTube,
    Block::MagentaNixieTube,
    Block::LightBlueNixieTube,
    Block::YellowNixieTube,
    Block::LimeNixieTube,
    Block::PinkNixieTube,
    Block::GrayNixieTube,
    Block::LightGrayNixieTube,
    Block::CyanNixieTube,
    Block::PurpleNixieTube,
    Block::BlueNixieTube,
    Block::BrownNixieTube,
    Block::GreenNixieTube,
    Block::RedNixieTube,
    Block::BlackNixieTube,
];

/// The colour of `block` in `family`.
fn colour_in(family: &[Block; 16], block: Block) -> Option<DyeColor> {
    family
        .iter()
        .position(|b| *b == block)
        .map(|i| DyeColor::ALL[i])
}

pub fn is_valve_handle(block: Block) -> bool {
    block == Block::CopperValveHandle || VALVE_HANDLES.contains(&block)
}

pub fn is_toolbox(block: Block) -> bool {
    TOOLBOXES.contains(&block)
}

pub fn is_postbox(block: Block) -> bool {
    POSTBOXES.contains(&block)
}

pub fn is_nixie_tube(block: Block) -> bool {
    NIXIE_TUBES.contains(&block)
}

/// Create's sliding doors (`SlidingDoorBlock`).
pub fn is_sliding_door(block: Block) -> bool {
    matches!(
        block,
        Block::AndesiteDoor
            | Block::BrassDoor
            | Block::CopperDoor
            | Block::TrainDoor
            | Block::FramedGlassDoor
    )
}

/// The block a dye of `colour` makes of `block` (`ValveHandleBlock.clicked`,
/// `ToolboxBlock.useItemOn`): the same family in that colour, or None when `block` already has it
/// or has no dyed family here. A copper valve handle has no colour, so any dye recolours it.
pub fn recoloured(block: Block, colour: DyeColor) -> Option<Block> {
    let (family, now) = if is_valve_handle(block) {
        (&VALVE_HANDLES, colour_in(&VALVE_HANDLES, block))
    } else if is_toolbox(block) {
        (&TOOLBOXES, colour_in(&TOOLBOXES, block))
    } else {
        return None;
    };
    (now != Some(colour)).then(|| family[colour as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dyes_recolour_valve_handles_and_toolboxes() {
        assert_eq!(
            DyeColor::of_item("minecraft:light_blue_dye"),
            Some(DyeColor::LightBlue)
        );
        assert_eq!(DyeColor::of_item("minecraft:blue_wool"), None);
        assert_eq!(DyeColor::of_item("create:blue_dye"), None);
        // ValveHandleBlock.clicked: `color != this.color`; copper has none.
        assert_eq!(
            recoloured(Block::CopperValveHandle, DyeColor::Red),
            Some(Block::RedValveHandle)
        );
        assert_eq!(recoloured(Block::RedValveHandle, DyeColor::Red), None);
        assert_eq!(
            recoloured(Block::RedValveHandle, DyeColor::Black),
            Some(Block::BlackValveHandle)
        );
        assert_eq!(
            recoloured(Block::BrownToolbox, DyeColor::LightGray),
            Some(Block::LightGrayToolbox)
        );
        assert_eq!(recoloured(Block::Shaft, DyeColor::Red), None);
        // Every family's blocks have their own colour's name.
        for c in DyeColor::ALL {
            assert_eq!(
                VALVE_HANDLES[c as usize].name(),
                format!("{}_valve_handle", c.name())
            );
            assert_eq!(
                TOOLBOXES[c as usize].name(),
                format!("{}_toolbox", c.name())
            );
            assert_eq!(
                POSTBOXES[c as usize].name(),
                format!("{}_postbox", c.name())
            );
            let nixie = if c == DyeColor::Orange {
                "nixie_tube".to_owned()
            } else {
                format!("{}_nixie_tube", c.name())
            };
            assert_eq!(NIXIE_TUBES[c as usize].name(), nixie);
        }
        assert!(is_sliding_door(Block::TrainDoor));
        assert!(!is_sliding_door(Block::TrainTrapdoor));
    }
}
