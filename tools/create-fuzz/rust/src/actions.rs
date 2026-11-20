//! Version 3 inputs use the same player interaction primitive on both adapters.
use create_engine::kinetics::Dir;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    GameMode {
        creative: bool,
    },
    Reload,
    Item {
        input: crate::transport::Input,
    },
    /// Another block's item access through the block's item handler on `side` (None: the
    /// unsided one): one insert of `insert`, or one extract of up to `extract` from `slot`.
    Capability {
        side: Option<Face>,
        slot: u8,
        insert: Option<crate::transport::Stack>,
        extract: Option<u8>,
        simulate: bool,
    },
    Use {
        item: String,
        face: Face,
        /// Surface hit relative to the clicked block; face is its normal.
        /// Partial-block surfaces may lie inside the unit cell.
        hit: [f64; 3],
        yaw: f32,
        pitch: f32,
        sneaking: bool,
    },
    SpeedSetting {
        row: i32,
        value: i32,
    },
    ValveSetting {
        row: i32,
        value: i32,
    },
    /// A value board pick on the block's `ScrollValueBehaviour` (`ValueSettingsPacket`): a
    /// brass tunnel's selection mode (row 0, the option's ordinal), an ejector's stack size.
    ScrollSetting {
        row: i32,
        value: i32,
    },
    /// The client's `ValueSettingsPacket` for the block's `FilteringBehaviour` while its amount
    /// board shows: the release of a short click on the filter slot holding `interact`'s item
    /// (`onShortInteract`), or a board pick (row 0 "up to", row 1 exactly; the count).
    FilterSetting {
        row: i32,
        value: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interact: Option<FilterInteract>,
    },
    Program {
        instructions: Vec<crate::program::Instruction>,
    },
    ChainLink {
        target: crate::scenario::Pos,
        connect: bool,
    },
    Remove,
    Signal {
        strength: u8,
    },
}

/// The held item and clicked face of a short click's release on a filter slot.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FilterInteract {
    pub item: String,
    pub face: Face,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Face {
    Down,
    Up,
    North,
    South,
    West,
    East,
}
impl Face {
    pub fn dir(self) -> Dir {
        match self {
            Self::Down => Dir::Down,
            Self::Up => Dir::Up,
            Self::North => Dir::North,
            Self::South => Dir::South,
            Self::West => Dir::West,
            Self::East => Dir::East,
        }
    }
    pub fn point(self, u: f64, v: f64) -> [f64; 3] {
        match self {
            Self::Down => [u, 0., v],
            Self::Up => [u, 1., v],
            Self::North => [u, v, 0.],
            Self::South => [u, v, 1.],
            Self::West => [0., u, v],
            Self::East => [1., u, v],
        }
    }
}
pub const BLOCKS: &[&str] = &[
    "create:copper_casing",
    "create:creative_fluid_tank",
    "create:smart_fluid_pipe",
    "create:fluid_valve",
    "create:spout",
    "create:item_drain",
    "create:depot",
    "create:fluid_pipe",
    "create:mechanical_pump",
    "create:fluid_tank",
    "create:rotation_speed_controller",
    "create:encased_chain_drive",
    "create:adjustable_chain_gearshift",
    "create:shaft",
    "create:hand_crank",
    "create:cogwheel",
    "create:large_cogwheel",
    "create:creative_motor",
    "create:clutch",
    "create:gearshift",
    "create:sequenced_gearshift",
    "create:gearbox",
    "create:vertical_gearbox",
    "create:speedometer",
    "create:stressometer",
    "create:flywheel",
    "create:steam_engine",
    "create:water_wheel",
    "create:large_water_wheel",
];
pub const ITEMS: &[&str] = &[
    "minecraft:milk_bucket",
    "create:honey_bucket",
    "create:chocolate_bucket",
    "minecraft:glass_bottle",
    "minecraft:honey_bottle",
    "create:builders_tea",
    "minecraft:bucket",
    "minecraft:water_bucket",
    "minecraft:lava_bucket",
    "create:copper_casing",
    "create:belt_connector",
    "minecraft:air",
    "create:wrench",
    "create:andesite_casing",
    "create:brass_casing",
    "create:metal_bracket",
    "create:wooden_bracket",
    "minecraft:oak_planks",
    "minecraft:spruce_planks",
    "minecraft:stone",
    "create:blaze_cake",
    "create:creative_blaze_cake",
    "create:andesite_tunnel",
    "create:brass_tunnel",
    // Held on a filter slot.
    "minecraft:iron_ingot",
    "minecraft:gold_ingot",
    "minecraft:diamond",
    "create:extendo_grip",
    "minecraft:stick",
    "create:metal_girder",
];
impl Action {
    pub fn validate(&self) {
        match self {
            Self::Item { input } => input.validate(),
            Self::Capability {
                slot,
                insert,
                extract,
                ..
            } => {
                assert!(*slot < 9, "capability slot outside a depot's");
                assert!(
                    insert.is_some() != extract.is_some_and(|n| n > 0),
                    "a capability access inserts or extracts"
                );
                if let Some(s) = insert {
                    s.validate();
                }
            }
            Self::ValveSetting { row, value } => {
                assert!((0..=1).contains(row) && (0..=180).contains(value));
            }
            Self::ScrollSetting { row, value } => {
                assert!((0..=1).contains(row) && (0..=64).contains(value));
            }
            Self::FilterSetting {
                row,
                value,
                interact,
            } => {
                assert!((0..=1).contains(row) && (0..=64).contains(value));
                if let Some(i) = interact {
                    assert!(
                        ITEMS.contains(&i.item.as_str()) || i.item == "minecraft:air",
                        "unsupported filter item {}",
                        i.item
                    );
                }
            }
            Self::SpeedSetting { row, value } => {
                assert!((0..=1).contains(row) && (0..=256).contains(value));
            }
            Self::Signal { strength } => assert!(*strength <= 15, "signal outside 0..15"),
            Self::Use {
                item,
                hit,
                yaw,
                pitch,
                ..
            } => {
                assert!(
                    BLOCKS.contains(&item.as_str())
                        || ITEMS.contains(&item.as_str())
                        || create_engine::dye::DyeColor::of_item(item).is_some(),
                    "unsupported use item {item}"
                );
                assert!(yaw.is_finite() && pitch.is_finite() && pitch.abs() <= 90.);
                assert!(
                    hit.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
                    "hit outside clicked block"
                );
            }
            Self::Program { instructions } => {
                assert!(!instructions.is_empty() && instructions.len() <= 5);
                for (i, instruction) in instructions.iter().enumerate() {
                    instruction.validate();
                    assert!(
                        instruction.instruction != crate::program::Kind::End
                            || i + 1 == instructions.len()
                    );
                }
            }
            Self::ChainLink { target, .. } => {
                assert!(
                    target[0].abs() <= 23 && target[2].abs() <= 23 && (1..=23).contains(&target[1])
                );
            }
            Self::Remove | Self::Reload | Self::GameMode { .. } => {}
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn input_schema_and_roundtrip() {
        assert!(serde_json::from_str::<super::Action>(r#"{"kind":"wrench","face":"up"}"#).is_err());
        assert!(
            serde_json::from_str::<super::Action>(r#"{"kind":"signal","strength":7,"extra":true}"#)
                .is_err()
        );
        for seed in (3..128).step_by(4) {
            let sc = crate::generate::generate(seed);
            let copy = serde_json::from_str(&serde_json::to_string(&sc).unwrap()).unwrap();
            assert_eq!(sc, copy);
            crate::catalogue::scenario(&copy);
            if !crate::fluid_world::enabled(&copy) {
                crate::engine::run(&copy);
            }
        }
    }
    #[test]
    fn wrench_uses_switching_block_state_before_commit() {
        let sc=serde_json::from_str(r#"{"version": 3, "ticks": 160, "checkpoints": [69, 71, 75, 77, 79], "blocks": [{"pos": [0, 8, 0], "state": "create:creative_motor[facing=east]", "motor": 32}, {"pos": [1, 8, 0], "state": "create:gearshift[axis=x,powered=false]"}, {"pos": [2, 8, 0], "state": "create:shaft[axis=x,waterlogged=false]"}], "events": [{"tick": 70, "pos": [2, 8, 0], "action": {"kind": "use", "item": "create:wrench", "face": "up", "hit": [0.5, 1.0, 0.5], "yaw": 0.0, "pitch": 0.0, "sneaking": false}}]}"#).unwrap();
        let snap = crate::engine::run(&sc);
        assert_eq!(
            snap.checkpoints[&69].networks.as_ref().unwrap().sizes["0,8,0"],
            3
        );
        assert_eq!(
            snap.checkpoints[&75].networks.as_ref().unwrap().sizes["0,8,0"],
            2
        );
        assert_eq!(
            snap.checkpoints[&75].networks.as_ref().unwrap().sizes["1,8,0"],
            2
        );
    }
    #[test]
    fn shaft_interaction_history() {
        let snap = crate::engine::run(&crate::generate::generate(3));
        assert!(snap.checkpoints[&73].blocks["2,8,0"].bracket.is_some());
        assert!(snap.checkpoints[&75].blocks["2,8,0"].bracket.is_none());
        assert!(
            snap.checkpoints[&77].blocks["2,8,0"]
                .state
                .contains("encased_shaft")
        );
        assert!(
            snap.checkpoints[&79].blocks["2,8,0"]
                .state
                .starts_with("create:shaft[")
        );
    }
}
