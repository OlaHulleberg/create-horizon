//! `getStateForPlacement` of Create's directional blocks: a block placed from an item turns to
//! the facing Create gives it, from the shafts around it, the player's view and sneaking
//! (`create_engine::place`): item vaults, nixie tubes, gauges, and the blocks with a facing or
//! an axis. Blocks whose placement reads or writes another subsystem's state (display links and
//! boards, steam engines, controller rails, redstone contacts, smart observers and threshold
//! switches, fluid valves, pipes, hose pulleys and whistles) are placed by those subsystems.

use super::super::states;
use super::super::world::{level_index, with_world};
use create_engine::Block;
use create_engine::kinetics::{Axis, Dir};
use create_engine::parts::StateInfo;
use mod_api::{BlockPart, BlockState, LevelHost, Registrar, StateForPlacementEvent};

/// The blocks other subsystems place.
fn placed_elsewhere(block: Block) -> bool {
    matches!(
        block,
        Block::DisplayLink
            | Block::DisplayBoard
            | Block::SteamEngine
            | Block::ControllerRail
            | Block::RedstoneContact
            | Block::ContentObserver
            | Block::StockpileSwitch
            | Block::FluidValve
            | Block::SmartFluidPipe
            | Block::HosePulley
            | Block::SteamWhistle
            | Block::FluidPipe
    )
}

/// Attaches the placement part to every Create block it places.
pub fn register(r: &mut dyn Registrar) {
    for &block in Block::ALL.iter() {
        if !placed_elsewhere(block) {
            r.block_part(states::key(block), Placement);
        }
    }
}

fn axis_name(a: Axis) -> &'static str {
    match a {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

fn dir_of(d: mod_api::Direction) -> Dir {
    Dir::ALL[d as usize]
}

struct Placement;

#[mod_api::horizon_part]
impl BlockPart for Placement {
    fn state_for_placement(
        &self,
        level: &dyn LevelHost,
        event: &mut StateForPlacementEvent<'_>,
    ) -> Option<Option<BlockState>> {
        let id = states::id(event.state)?;
        let table = |s: u32| -> Option<(Block, StateInfo)> {
            with_world(|w| w.table.get(s).map(|(b, i)| (b, *i))).flatten()
        };
        let (block, info) = table(id)?;
        let dim = level_index(event.dimension.key().id())?;
        let c = event.context;
        let pos = [c.pos.x, c.pos.y, c.pos.z];
        let clicked_face = dir_of(c.face);
        let ctx = create_engine::place::Context {
            yaw: c.yaw,
            pitch: c.pitch,
            sneaking: c.sneaking,
            clicked_face,
        };
        let set = |s: BlockState, k: &str, v: &str| level.state_with_property(s, k, v).unwrap_or(s);
        // `SimpleWaterloggedBlock`: placed into water, it holds it (the template's own answer,
        // which this one replaces).
        let water = level
            .block_state_id("minecraft:water")
            .and_then(|w| level.state_fluid_state(w))
            .is_some_and(|w| event.fluid.source && event.fluid.fluid == w.fluid);
        let default = if water {
            set(event.state, "waterlogged", "true")
        } else {
            event.state
        };
        let beside = |side: Dir| {
            let o = side.offset();
            [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]
        };
        if block == Block::ItemVault {
            // ItemVaultBlock.getStateForPlacement: the axis of the vault it was placed against
            // unless sneaking, else the player's horizontal facing's.
            let on = beside(clicked_face.opposite());
            let preferred = level
                .block_state(on)
                .and_then(states::id)
                .and_then(table)
                .filter(|(b, _)| *b == Block::ItemVault)
                .and_then(|(_, i)| i.axis);
            let axis = match preferred {
                Some(a) if !c.sneaking => a,
                _ => create_engine::place::horizontal_direction(c.yaw).axis(),
            };
            return Some(Some(set(default, "axis", axis_name(axis))));
        }
        if create_engine::dye::is_nixie_tube(block) {
            // NixieTubeBlock.getStateForPlacement: its face from the clicked face, its facing
            // from the player.
            let (face, facing) = create_engine::place::nixie(ctx);
            let s = set(default, "double_face", face.name());
            return Some(Some(set(
                s,
                "facing",
                super::super::kinetics::dir_name(facing),
            )));
        }
        if create_engine::world::gauge_kind(block).is_some() {
            // GaugeBlock.getStateForPlacement: the block it was placed on (behind the clicked
            // face) carries its shaft on through it.
            let on = beside(clicked_face.opposite());
            let look = create_engine::place::ordered_by_nearest(c.yaw, c.pitch)[0];
            let horizontal = create_engine::place::horizontal_direction(c.yaw);
            let (facing, along) = with_world(|w| {
                create_engine::gauge::placement(
                    clicked_face,
                    look,
                    horizontal,
                    w.has_shaft_towards(dim, on, clicked_face),
                    |side| w.has_shaft_towards(dim, beside(side), side.opposite()),
                )
            })?;
            let s = set(default, "facing", super::super::kinetics::dir_name(facing));
            return Some(Some(set(
                s,
                "axis_along_first",
                if along { "true" } else { "false" },
            )));
        }
        let mut state = default;
        let mut changed = water;
        let facing = with_world(|w| {
            create_engine::place::facing(block, ctx, |side| {
                w.has_shaft_towards(dim, beside(side), side.opposite())
            })
        })
        .flatten();
        if let Some(f) = facing
            && info.facing != Some(f)
        {
            state = set(state, "facing", super::super::kinetics::dir_name(f));
            changed = true;
        }
        // Create's axis blocks: chain drives, the speed controller, the sequenced gearshift.
        let axis = with_world(|w| {
            let above = w.large_cog_axis(dim, [pos[0], pos[1] + 1, pos[2]]);
            create_engine::place::axis(
                block,
                ctx,
                |side| w.has_shaft_towards(dim, beside(side), side.opposite()),
                above,
            )
        })
        .flatten();
        if let Some((a, vertical)) = axis
            && (info.axis != Some(a) || info.vertical != vertical)
        {
            state = set(state, "axis", axis_name(a));
            if block == Block::SequencedGearshift {
                state = set(state, "vertical", if vertical { "true" } else { "false" });
            }
            changed = true;
        }
        changed.then_some(Some(state))
    }
}
