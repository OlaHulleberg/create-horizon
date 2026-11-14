//! Create's placement of its decorative blocks: `getStateForPlacement` where the placement
//! context is enough (desk bell, item hatch, cardboard, nozzle), and what follows a placement
//! from an item where the template's own placement runs first (placards and clipboards turned,
//! a clipboard keeping its pages, girders joining their neighbours, a large water wheel's room,
//! a peculiar bell looking under itself).

use super::super::persist;
use super::super::server::ServerLevel;
use super::super::states;
use super::{
    add, block_of, creative, dir, flag, girders, player_dimension, prop, with, with_props,
};
use create_engine::Block;
use create_engine::decor;
use create_engine::kinetics::{Axis, Dir};
use mod_api::{
    BlockFace, BlockState, HostBlockStates, ItemPart, ItemStack, LevelHost, OnBlockPlacedByEvent,
    PlayerRef, Registrar, ServerHost, StateForPlacementEvent, Text,
};

pub(super) fn register(r: &mut dyn Registrar) {
    for block in [
        Block::DeskBell,
        Block::ItemHatch,
        Block::CardboardBlock,
        Block::BoundCardboardBlock,
        Block::Nozzle,
    ] {
        r.block_part(states::key(block), Placement);
    }
    for block in [
        Block::Placard,
        Block::Clipboard,
        Block::MetalGirder,
        Block::LargeWaterWheel,
        Block::PeculiarBell,
    ] {
        let item = r.item_key(&format!("create:{}", block.name()));
        r.item_part(item, Placed);
    }
}

fn axis_name(a: Axis) -> &'static str {
    match a {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

fn props_on(
    level: &dyn LevelHost,
    state: BlockState,
    props: &[(&str, &str)],
) -> Option<BlockState> {
    props
        .iter()
        .try_fold(state, |s, (k, v)| level.state_with_property(s, k, v))
}

struct Placement;

#[mod_api::horizon_part]
impl mod_api::BlockPart for Placement {
    fn state_for_placement(
        &self,
        level: &dyn LevelHost,
        event: &mut StateForPlacementEvent<'_>,
    ) -> Option<Option<BlockState>> {
        let (block, _) = block_of(states::id(event.state)?)?;
        let c = event.context;
        let clicked = dir(c.face);
        let state = event.state;
        let placed = match block {
            // `DeskBellBlock.getStateForPlacement`: facing the clicked face.
            Block::DeskBell => props_on(
                level,
                state,
                &[("facing", super::super::kinetics::dir_name(clicked))],
            ),
            // `ItemHatchBlock.getStateForPlacement`: only on a side, facing into it.
            Block::ItemHatch => {
                if clicked.axis() == Axis::Y {
                    return Some(None);
                }
                props_on(
                    level,
                    state,
                    &[
                        (
                            "facing",
                            super::super::kinetics::dir_name(clicked.opposite()),
                        ),
                        ("open", "false"),
                    ],
                )
            }
            // `CardboardBlock.getStateForPlacement`: the player's horizontal axis.
            Block::CardboardBlock | Block::BoundCardboardBlock => {
                let axis = decor::horizontal_facing(c.yaw).axis();
                props_on(level, state, &[("axis", axis_name(axis))])
            }
            // `NozzleBlock.getStateForPlacement`: facing the clicked face; `canSurvive`: only on
            // the front of an encased fan facing the same way.
            Block::Nozzle => {
                let o = clicked.offset();
                let p = [c.pos.x - o[0], c.pos.y - o[1], c.pos.z - o[2]];
                let behind = level
                    .block_state(p)
                    .and_then(states::id)
                    .and_then(block_of)
                    .filter(|(b, _)| *b == Block::EncasedFan)
                    .and_then(|(_, i)| i.facing);
                if !create_engine::nozzle::survives(clicked, behind) {
                    return Some(None);
                }
                props_on(
                    level,
                    state,
                    &[("facing", super::super::kinetics::dir_name(clicked))],
                )
            }
            _ => return None,
        };
        placed.map(Some)
    }
}

fn dir_of(face: BlockFace) -> Dir {
    match face {
        BlockFace::Down => Dir::Down,
        BlockFace::Up => Dir::Up,
        BlockFace::North => Dir::North,
        BlockFace::South => Dir::South,
        BlockFace::West => Dir::West,
        _ => Dir::East,
    }
}

/// The items of the blocks whose placement Create finishes after the template's.
struct Placed;

#[mod_api::horizon_part]
impl ItemPart for Placed {
    fn on_block_placed_by(&self, host: &mut dyn ServerHost, event: &mut OnBlockPlacedByEvent<'_>) {
        let Some(id) = states::id(event.state) else {
            return;
        };
        let Some((block, info)) = block_of(id) else {
            return;
        };
        // The notice names no level: the placer's is the one (a dispenser's placement, with
        // no placer, is left as the template made it).
        let Some(dimension) = event.placer.and_then(|p| player_dimension(&*host, p)) else {
            return;
        };
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        let pos = event.pos;
        let clicked = dir_of(event.face);
        match block {
            Block::MetalGirder => girders::place(&mut lv, pos, clicked),
            Block::Placard | Block::Clipboard => {
                let placard = block == Block::Placard;
                let Some(f) = info.facing else {
                    return;
                };
                let face = prop(&*lv.host, event.state, "face").unwrap_or_default();
                let facing = decor::placed_facing(placard, &face, f);
                let written = flag(event.stack.components().is_some());
                let mut props = vec![("facing", super::super::kinetics::dir_name(facing))];
                if !placard {
                    props.push(("written", written));
                    let mut one = event.stack.clone();
                    let _ = one.set_count(1);
                    with(lv.dim, |d| d.clipboards.insert(pos, one));
                    persist::mark_pos_unsaved(lv.dim, pos);
                }
                if let Some(next) = with_props(&*lv.host, event.state, &props)
                    && next != event.state
                {
                    lv.put(pos, next);
                }
            }
            // `LargeWaterWheelBlock.getStateForPlacement`: the eight blocks around it in its
            // plane must be free.
            Block::LargeWaterWheel => {
                let Some(axis) = info.axis else {
                    return;
                };
                let fits = |lv: &ServerLevel<'_>, at: [i32; 3]| {
                    let facts = lv.host.block_states();
                    let free = |p: [i32; 3]| {
                        lv.block_state(p)
                            .is_some_and(|s| facts.replaceable(s) == Some(true))
                    };
                    let behind = add(at, Dir::from_axis(axis, false));
                    create_engine::waterwheel::plane(axis)
                        .into_iter()
                        .all(|o| free([at[0] + o[0], at[1] + o[1], at[2] + o[2]]))
                        && block_of(lv.block(behind))
                            .is_none_or(|(b, _)| b != Block::LargeWaterWheel)
                };
                if !fits(&lv, pos) {
                    // `LargeWaterWheelBlockItem.place`: refused against a face across its axis,
                    // it tries once more a block further out.
                    // (The first try leaves nothing behind.)
                    let further = add(pos, clicked);
                    if clicked.axis() != axis
                        && let Some(air) = lv.host.block_state_id("minecraft:air")
                        && lv.put(pos, air)
                        && lv
                            .block_state(further)
                            .is_some_and(|s| lv.host.block_states().replaceable(s) == Some(true))
                        && fits(&lv, further)
                    {
                        lv.put(further, event.state);
                        lv.place_sound(further);
                        return;
                    }
                    refuse(&mut lv, pos, event.placer, event.stack);
                    if let Some(p) = event.placer {
                        let line = Text::translate(
                            "create.large_water_wheel.not_enough_space",
                            Vec::new(),
                        );
                        let _ = p.action_bar(&mut *lv.host, line);
                    }
                }
            }
            Block::PeculiarBell => with(lv.dim, |d| d.bell_checks.push(pos)),
            _ => {}
        }
    }
}

/// `getStateForPlacement` answered null after the template placed the block: it goes and the
/// item comes back (the placement still uses one up).
fn refuse(lv: &mut ServerLevel<'_>, pos: [i32; 3], placer: Option<PlayerRef>, stack: &ItemStack) {
    if let Some(air) = lv.host.block_state_id("minecraft:air") {
        lv.put(pos, air);
    }
    let Some(player) = placer else {
        return;
    };
    if !creative(&*lv.host, player) {
        let mut one = stack.clone();
        let _ = one.set_count(1);
        let _ = player.give_stack(&mut *lv.host, one);
    }
}
