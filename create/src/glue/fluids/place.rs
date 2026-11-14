//! `getStateForPlacement` of Create's fluid blocks placed from an item: fluid pipes straight
//! toward the player's view when nothing is around (`FluidPipeBlock`), pumps turned to a pipe
//! beside them (`PumpBlock`), valves, smart pipes and hose pulleys by the shafts and pipes
//! around them, and steam engines and whistles on a tank (refused without one).
//! Read from the server's world; a client predicts the template's state and is corrected.

use super::super::states;
use super::super::world::{level_index, with_world};
use crate::glue::kinetics::dir_name;
use create_engine::Block;
use create_engine::fluids::{self, Cell, Gate};
use create_engine::kinetics::{Axis, Dir};
use create_engine::parts::StateInfo;
use create_engine::place::{horizontal_direction, ordered_by_nearest};
use create_engine::steam::Face;
use mod_api::{BlockPart, BlockState, LevelHost, StateForPlacementEvent};

/// The blocks this module places.
pub(super) const PLACED: [Block; 7] = [
    Block::FluidPipe,
    Block::MechanicalPump,
    Block::FluidValve,
    Block::SmartFluidPipe,
    Block::HosePulley,
    Block::SteamWhistle,
    Block::SteamEngine,
];

pub(super) struct Placement;

fn flag(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

/// The Create block at `p` as the server's world numbers it.
fn create_block(level: &dyn LevelHost, p: [i32; 3]) -> Option<(Block, StateInfo, BlockState)> {
    let state = level.block_state(p)?;
    let id = states::id(state)?;
    let (b, i) = with_world(|w| w.table.get(id).map(|(b, i)| (b, *i))).flatten()?;
    Some((b, i, state))
}

/// A pipe state's open sides, read from its properties.
fn sides(level: &dyn LevelHost, state: BlockState) -> u8 {
    let text = level.state_string(state).unwrap_or_default();
    let (_, props) = states::split(&text);
    Dir::ALL
        .into_iter()
        .filter(|d| props.contains(&(dir_name(*d), "true")))
        .fold(0, |m, d| m | d.bit())
}

/// What the block at `p` is to a pipe (`FluidPipeBlock.canConnectTo`), as the pipes see it
/// (`pipes::pipe_cell`); anything a pipe doesn't join is solid here.
fn cell(level: &dyn LevelHost, p: [i32; 3]) -> Cell {
    let Some((block, info, state)) = create_block(level, p) else {
        return Cell::Solid;
    };
    let axis = |a: Option<Axis>| a.map_or(Cell::Solid, |a| Cell::Axis(a, Gate::Open));
    match block {
        Block::FluidPipe => Cell::Pipe(sides(level, state)),
        Block::EncasedFluidPipe => Cell::Encased(sides(level, state)),
        Block::MechanicalPump => info.facing.map_or(Cell::Solid, Cell::Pump),
        Block::FluidTank
        | Block::CreativeFluidTank
        | Block::Spout
        | Block::ItemDrain
        | Block::Basin => Cell::Tank,
        Block::GlassFluidPipe => axis(info.axis),
        Block::FluidValve => axis(
            info.facing
                .map(|f| fluids::valve_pipe_axis(f, info.along_first)),
        ),
        Block::SmartFluidPipe => axis(
            info.facing
                .map(|f| fluids::smart_pipe_axis(info.face == Some(Face::Wall), f)),
        ),
        Block::HosePulley => info.facing.map_or(Cell::Solid, |f| {
            Cell::Port(create_engine::hose::pipe_side(f))
        }),
        _ => Cell::Solid,
    }
}

fn offset(p: [i32; 3], d: Dir) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

fn set(level: &dyn LevelHost, state: BlockState, props: &[(&str, &str)]) -> BlockState {
    props.iter().fold(state, |s, (k, v)| {
        level.state_with_property(s, k, v).unwrap_or(s)
    })
}

#[mod_api::horizon_part]
impl BlockPart for Placement {
    fn state_for_placement(
        &self,
        level: &dyn LevelHost,
        event: &mut StateForPlacementEvent<'_>,
    ) -> Option<Option<BlockState>> {
        let id = states::id(event.state)?;
        let (block, _) = with_world(|w| w.table.get(id).map(|(b, i)| (b, *i))).flatten()?;
        let dim = level_index(event.dimension.key().id())?;
        let c = event.context;
        let pos = [c.pos.x, c.pos.y, c.pos.z];
        let looking = ordered_by_nearest(c.yaw, c.pitch);
        let nearest = looking[0];
        let horizontal = horizontal_direction(c.yaw);
        let clicked = Dir::ALL[c.face as usize];
        let joins = |d: Dir| fluids::connects(cell(level, offset(pos, d)), d);
        let shaft = |d: Dir| {
            with_world(|w| w.has_shaft_towards(dim, offset(pos, d), d.opposite())).unwrap_or(false)
        };
        let state = event.state;
        let placed = match block {
            // FluidPipeBlock.getStateForPlacement: `updateBlockState` preferring the view.
            Block::FluidPipe => {
                let s = fluids::pipe_sides(0, |d| cell(level, offset(pos, d)), nearest);
                let props: Vec<(&str, &str)> = Dir::ALL
                    .into_iter()
                    .map(|d| (dir_name(d), flag(s & d.bit() != 0)))
                    .collect();
                set(level, state, &props)
            }
            // PumpBlock.getStateForPlacement: DirectionalKineticBlock's facing, turned to the
            // pipe beside it nearest that facing unless it is on the facing's axis or the player
            // sneaks.
            Block::MechanicalPump => {
                let target = if c.sneaking {
                    nearest
                } else {
                    nearest.opposite()
                };
                let facing = match create_engine::place::preferred(false, shaft) {
                    Some(p) if !c.sneaking => p.opposite(),
                    _ => target,
                };
                let mut best: Option<(Dir, i32)> = None;
                for d in Dir::ALL {
                    if !joins(d) {
                        continue;
                    }
                    let (a, b) = (d.offset(), target.offset());
                    let dist = (0..3).map(|i| (a[i] - b[i]).pow(2)).sum::<i32>();
                    if best.is_some_and(|(_, bd)| dist > bd) {
                        continue;
                    }
                    best = Some((d, dist));
                }
                let facing = match best {
                    Some((d, _)) if d.axis() != target.axis() && !c.sneaking => d,
                    _ => facing,
                };
                set(level, state, &[("facing", dir_name(facing))])
            }
            // FluidValveBlock (DirectionalAxisKineticBlock) placed among shafts and pipes.
            Block::FluidValve => {
                let (facing, along) =
                    fluids::valve_placement(nearest, c.sneaking, horizontal, shaft, joins);
                set(
                    level,
                    state,
                    &[
                        ("facing", dir_name(facing)),
                        ("axis_along_first", flag(along)),
                    ],
                )
            }
            // SmartFluidPipeBlock: on the face looked at, then turned to the pipes around it.
            Block::SmartFluidPipe => {
                let (mut face, mut facing) = match nearest {
                    Dir::Up => (Face::Ceiling, horizontal),
                    Dir::Down => (Face::Floor, horizontal),
                    d => (Face::Wall, d.opposite()),
                };
                if let Some((f, d)) = fluids::smart_pipe_placement(joins, looking, face, facing) {
                    (face, facing) = (f, d);
                }
                set(
                    level,
                    state,
                    &[("face", face.name()), ("facing", dir_name(facing))],
                )
            }
            // HosePulleyBlock.getStateForPlacement.
            Block::HosePulley => {
                let facing = create_engine::hose::placement(shaft, joins, horizontal);
                set(level, state, &[("facing", dir_name(facing))])
            }
            // WhistleBlock.getStateForPlacement: on the tank it is attached to, or not at all.
            Block::SteamWhistle => {
                let (wall, facing) = create_engine::whistle::placement(clicked, horizontal);
                let tank = offset(pos, create_engine::whistle::attached(wall, facing));
                let on_tank = create_block(level, tank).is_some_and(|(b, _, _)| {
                    matches!(b, Block::FluidTank | Block::CreativeFluidTank)
                });
                if !on_tank {
                    return Some(None);
                }
                set(
                    level,
                    state,
                    &[("wall", flag(wall)), ("facing", dir_name(facing))],
                )
            }
            // SteamEngineBlock: on the tank nearest the view, or not at all (`canSurvive`).
            Block::SteamEngine => {
                let tank = |d: Dir| {
                    create_block(level, offset(pos, d))
                        .is_some_and(|(b, _, _)| b == Block::FluidTank)
                };
                let Some((face, facing)) =
                    create_engine::steam::placement(looking, horizontal, tank)
                else {
                    return Some(None);
                };
                set(
                    level,
                    state,
                    &[("face", face.name()), ("facing", dir_name(facing))],
                )
            }
            _ => return None,
        };
        (placed != state).then_some(Some(placed))
    }
}
