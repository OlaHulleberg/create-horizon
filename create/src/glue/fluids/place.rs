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
        let text = level
            .block_state(p)
            .and_then(|s| level.state_string(s))
            .unwrap_or_default();
        let (name, props) = states::split(&text);
        return if fluids::vanilla_target(name, props.iter().any(|(k, _)| *k == "honey_level")) {
            Cell::VanillaTarget
        } else {
            Cell::Solid
        };
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
        // `withWater` (pipes and pumps are `ProperWaterloggedBlock`s).
        let water = crate::glue::logistics::placement::in_water(level, event);
        let placed = match block {
            // FluidPipeBlock.getStateForPlacement: `updateBlockState` preferring the view.
            Block::FluidPipe => {
                let s = fluids::pipe_sides(0, |d| cell(level, offset(pos, d)), nearest);
                let mut props: Vec<(&str, &str)> = Dir::ALL
                    .into_iter()
                    .map(|d| (dir_name(d), flag(s & d.bit() != 0)))
                    .collect();
                props.push(("waterlogged", flag(water)));
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
                set(
                    level,
                    state,
                    &[("facing", dir_name(facing)), ("waterlogged", flag(water))],
                )
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

/// `FluidTankItem.tryMultiPlace`: a tank placed by a player (not sneaking) on the top or bottom
/// of a tank wider than one fills the rest of that layer with tanks, when the layer is free
/// and the player has the tanks (creative: always). Done in the level's next tick, as the
/// placement is not finished yet (its stack not used up).
pub(super) struct TankItem;

/// Tank layers to fill: level, the player, the hand holding the tanks, the positions.
type Layer = (usize, mod_api::PlayerRef, mod_api::Hand, Vec<[i32; 3]>);

thread_local! {
    static LAYERS: std::cell::RefCell<Vec<Layer>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(super) fn reset() {
    LAYERS.with_borrow_mut(Vec::clear);
}

#[mod_api::horizon_part]
impl mod_api::ItemPart for TankItem {
    fn on_block_placed_by(
        &self,
        host: &mut dyn mod_api::ServerHost,
        event: &mut mod_api::OnBlockPlacedByEvent<'_>,
    ) {
        use super::super::items;
        let Some(player) = event.placer else {
            return;
        };
        // The accepted placement can be followed immediately by a save. Publish
        // FluidTankBlockEntity.write state, including pending updateConnectivity,
        // before any optional layer placement is queued.
        if let Some(dimension) = player.dimension(&*host) {
            let dimension = dimension.to_string();
            if let Some(mut lv) = super::super::server::ServerLevel::new(host, &dimension) {
                super::super::persist::publish(&mut lv);
            }
        }
        if items::sneaking(&*host, player) {
            return;
        }
        let up = match event.face {
            mod_api::BlockFace::Up => true,
            mod_api::BlockFace::Down => false,
            _ => return,
        };
        let Some(dim) = player
            .dimension(&*host)
            .and_then(|d| level_index(&d.to_string()))
        else {
            return;
        };
        let pos = event.pos;
        let on = [pos[0], pos[1] + if up { -1 } else { 1 }, pos[2]];
        let Some((c, m)) = with_world(|w| w.tank(dim, on)).flatten() else {
            return;
        };
        if m.width == 1 {
            return;
        }
        let start_y = if up {
            c[1] + i32::from(m.height)
        } else {
            c[1] - 1
        };
        if start_y != pos[1] {
            return;
        }
        let Some(level) = player.level(&*host) else {
            return;
        };
        let mut free = Vec::new();
        for x in 0..i32::from(m.width) {
            for z in 0..i32::from(m.width) {
                let p = [c[0] + x, start_y, c[2] + z];
                if p == pos {
                    continue;
                }
                let Some(state) = level.block_state(&*host, p) else {
                    return;
                };
                let tank = states::id(state).is_some_and(|id| {
                    with_world(|w| w.table.get(id).map(|(b, _)| b))
                        .flatten()
                        .is_some_and(|b| b == Block::FluidTank)
                });
                if tank {
                    continue;
                }
                if mod_api::HostBlockStates::block_states(&*host).replaceable(state) != Some(true) {
                    return;
                }
                free.push(p);
            }
        }
        if free.is_empty() {
            return;
        }
        // The stack still holds the tank just placed (it is used up after this).
        let tank_key = event.stack.item();
        let hand = [mod_api::Hand::Main, mod_api::Hand::Off]
            .into_iter()
            .find(|h| items::held(&*host, player, *h).is_some_and(|s| s.item() == tank_key));
        let Some(hand) = hand else {
            return;
        };
        if !items::creative(&*host, player) && event.stack.count() < free.len() as u32 + 1 {
            return;
        }
        LAYERS.with_borrow_mut(|l| l.push((dim, player, hand, free)));
    }
}

/// The tank layers asked for in this level: each free place takes a tank (`super.place` with
/// the tank sound silenced), the player's stack one each outside creative.
pub(super) fn tick_layers(lv: &mut super::ServerLevel<'_>) {
    use super::super::items;
    let dim = lv.dim;
    let layers: Vec<Layer> = LAYERS.with_borrow_mut(|l| {
        if l.is_empty() {
            return Vec::new();
        }
        let (here, rest): (Vec<_>, Vec<_>) = l.drain(..).partition(|e| e.0 == dim);
        *l = rest;
        here
    });
    for (_, player, hand, free) in layers {
        let Some(state) = super::state_with(lv, "create:fluid_tank", &[]) else {
            continue;
        };
        let creative = items::creative(&*lv.host, player);
        for p in free {
            if !super::replaceable(lv, p) {
                continue;
            }
            if !creative {
                let has = items::held(&*lv.host, player, hand).is_some_and(|s| {
                    super::machines::item_name(&*lv.host, &s) == "create:fluid_tank"
                });
                if !has {
                    break;
                }
                items::shrink_held(lv.host, player, hand, 1);
            }
            lv.set_state(p, state, super::all());
        }
    }
}
