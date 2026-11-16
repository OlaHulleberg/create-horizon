//! `getStateForPlacement` of Create's directional blocks: a block placed from an item turns to
//! the facing Create gives it, from the shafts around it, the player's view and sneaking
//! (`create_engine::place`): item vaults, nixie tubes, gauges, and the blocks with a facing or
//! an axis. Blocks whose placement reads or writes another subsystem's state (display links and
//! boards, steam engines, controller rails, redstone contacts, smart observers and threshold
//! switches, fluid valves, pipes, pumps, hose pulleys and whistles) are placed by those subsystems.

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
            | Block::MechanicalPump
    )
}

/// Attaches the placement part to every Create block it places.
pub fn register(r: &mut dyn Registrar) {
    for &block in Block::ALL.iter() {
        if !placed_elsewhere(block) {
            r.block_part(states::key(block), Placement);
        }
    }
    let vault = r.item_key("create:item_vault");
    r.item_part(vault, VaultItem);
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

/// `ProperWaterloggedBlock.withWater` at placement: a water source where the block goes
/// (`getFluidState(pos).getType() == Fluids.WATER`).
pub(crate) fn in_water(level: &dyn LevelHost, event: &StateForPlacementEvent<'_>) -> bool {
    level
        .block_state_id("minecraft:water")
        .and_then(|w| level.state_fluid_state(w))
        .is_some_and(|w| event.fluid.source && event.fluid.fluid == w.fluid)
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
        let (block, _) = table(id)?;
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
        let water = in_water(level, event);
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
        // Create's logistics blocks take no facing from their templates' placement.
        let flag = |b: bool| if b { "true" } else { "false" };
        match block {
            // FunnelBlock.getStateForPlacement: extracting unless sneaking; facing away from the
            // nearest way the player looks (`getNearestLookingDirections`: into the clicked
            // block first) whose block behind is no funnel.
            Block::AndesiteFunnel | Block::BrassFunnel => {
                let funnel = |p: [i32; 3]| {
                    level
                        .block_state(p)
                        .and_then(states::id)
                        .and_then(table)
                        .is_some_and(|(b, _)| super::funnel::is_funnel(b))
                };
                let facing =
                    create_engine::place::nearest_looking_directions(ctx, c.replace_clicked)
                        .into_iter()
                        .map(Dir::opposite)
                        .find(|f| !funnel(beside(f.opposite())))
                        .unwrap_or(Dir::Up);
                let s = set(default, "facing", super::super::kinetics::dir_name(facing));
                let s = set(s, "extracting", flag(!c.sneaking));
                return Some(Some(s));
            }
            // ChuteBlock.getStateForPlacement: down, or, placed against a block's side while not
            // sneaking, facing that side (diagonal) when a chute is there for it to lead into
            // (`updateChuteState`).
            Block::Chute => {
                let facing_at = |p: [i32; 3]| {
                    let (b, i) = level.block_state(p).and_then(states::id).and_then(table)?;
                    match b {
                        Block::Chute => Some(i.facing.unwrap_or(Dir::Down)),
                        Block::SmartChute => Some(Dir::Down),
                        _ => None,
                    }
                };
                let facing = if clicked_face.axis() != Axis::Y && !c.sneaking {
                    clicked_face
                } else {
                    Dir::Down
                };
                let above = [pos[0], pos[1] + 1, pos[2]];
                let funnel_down = level
                    .block_state(above)
                    .and_then(states::id)
                    .and_then(table)
                    .is_some_and(|(b, i)| {
                        matches!(b, Block::AndesiteFunnel | Block::BrassFunnel)
                            && i.facing == Some(Dir::Down)
                    });
                let (f, shape) = create_engine::chute::update_state(
                    pos,
                    facing,
                    create_engine::chute::Shape::Normal,
                    facing_at,
                    funnel_down,
                );
                let s = set(default, "facing", super::super::kinetics::dir_name(f));
                return Some(Some(set(s, "shape", shape.name())));
            }
            // HorizontalKineticBlock.getStateForPlacement: facing the player.
            Block::WeightedEjector => {
                let f = create_engine::place::horizontal_direction(c.yaw).opposite();
                return Some(Some(set(
                    default,
                    "facing",
                    super::super::kinetics::dir_name(f),
                )));
            }
            // ArmBlock.getStateForPlacement: hanging when placed against a block's underside.
            Block::MechanicalArm => {
                return Some(Some(set(
                    default,
                    "ceiling",
                    flag(clicked_face == Dir::Down),
                )));
            }
            _ => {}
        }
        placed_state(block, default, water, (dim, pos), ctx, &set, &|p| {
            level.block_state(p).and_then(states::id).and_then(table)
        })
    }
}

/// The rest of `getStateForPlacement` for Create's kinetic blocks (their facing, axis and the
/// properties that follow them) placed at `pos` in level `dim` in `ctx`, from `default` (in
/// water when `water`): None leaves the template's state, `Some(None)` refuses the spot.
/// `info_at` reads the blocks around. Also what `processing::operator` places an assembly
/// operator with, two above what it works on.
pub(crate) fn placed_state(
    block: Block,
    default: BlockState,
    water: bool,
    (dim, pos): (usize, [i32; 3]),
    ctx: create_engine::place::Context,
    set: &dyn Fn(BlockState, &str, &str) -> BlockState,
    info_at: &dyn Fn([i32; 3]) -> Option<(Block, StateInfo)>,
) -> Option<Option<BlockState>> {
    let clicked_face = ctx.clicked_face;
    let flag = |b: bool| if b { "true" } else { "false" };
    let beside = |side: Dir| {
        let o = side.offset();
        [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]]
    };
    let mut state = default;
    let mut changed = water;
    let facing = with_world(|w| {
        create_engine::place::facing(block, ctx, |side| {
            w.has_shaft_towards(dim, beside(side), side.opposite())
        })
    })
    .flatten();
    // Set even when it is the default state's: a template's own placement (the basin's
    // barrel) would otherwise turn it.
    if let Some(f) = facing {
        state = set(state, "facing", super::super::kinetics::dir_name(f));
        changed = true;
    }
    if let Some(f) = facing
        && matches!(block, Block::Deployer | Block::MechanicalSaw)
    {
        let along = with_world(|w| {
            create_engine::place::along_first(f, ctx, |side| {
                w.has_shaft_towards(dim, beside(side), side.opposite())
            })
        })
        .unwrap_or(false);
        state = set(state, "axis_along_first", flag(along));
        if block == Block::MechanicalSaw {
            state = set(
                state,
                "flipped",
                flag(create_engine::place::saw_flipped(f, ctx)),
            );
        }
        changed = true;
    }
    if block == Block::MechanicalCrafter {
        // Placed against another crafter, it takes that one's facing.
        let against = info_at(beside(clicked_face.opposite()))
            .filter(|(b, _)| *b == Block::MechanicalCrafter)
            .and_then(|(_, i)| i.facing);
        let (f, pointing) = create_engine::place::crafter(ctx, against);
        state = set(state, "facing", super::super::kinetics::dir_name(f));
        state = set(state, "pointing", pointing);
        changed = true;
    }
    // Create's axis blocks: shafts, cogs, casings, gearboxes, chain drives, the speed
    // controller, the sequenced gearshift.
    let axis = with_world(|w| {
        let below = [pos[0], pos[1] - 1, pos[2]];
        let near = create_engine::place::Nearby {
            large_cog_above: w.large_cog_axis(dim, [pos[0], pos[1] + 1, pos[2]]),
            small_cog_clicked: w
                .cog_at(dim, beside(clicked_face.opposite()))
                .filter(|(large, _)| !large)
                .map(|(_, a)| a),
            controller_below: w
                .state_at(dim, below)
                .and_then(|s| w.table.get(s))
                .filter(|(b, _)| *b == Block::RotationSpeedController)
                .and_then(|(_, i)| i.axis),
        };
        create_engine::place::axis(
            block,
            ctx,
            |side| w.has_shaft_towards(dim, beside(side), side.opposite()),
            near,
        )
    })
    .flatten();
    // CogWheelBlock.canSurvive (`isValidCogwheelPosition`): no large cog beside it in its
    // plane, nor a small one beside a large one.
    if let (Block::Cogwheel | Block::LargeCogwheel, Some((a, _))) = (block, axis) {
        let large = block == Block::LargeCogwheel;
        if with_world(|w| w.valid_cog_position(dim, pos, large, a)) == Some(false) {
            return Some(None);
        }
    }
    // Set even when it is the default state's: a template's own placement (a pillar's, from
    // the clicked face) would otherwise turn an upright gearbox or shaft.
    if let Some((a, vertical)) = axis {
        state = set(state, "axis", axis_name(a));
        if block == Block::SequencedGearshift {
            state = set(state, "vertical", if vertical { "true" } else { "false" });
        }
        changed = true;
    }
    changed.then_some(Some(state))
}

/// `ItemVaultItem.tryMultiPlace`: a vault placed by a player (not sneaking) on the end face of a
/// vault wider than one fills the rest of that cross-section with vaults, when it is free and
/// the player has the vaults (creative: always). Done in the level's next tick, as the
/// placement is not finished yet (its stack not used up).
pub struct VaultItem;

/// Vault layers to fill: level, the player, the hand holding the vaults, the axis, the
/// positions.
type Layer = (
    usize,
    mod_api::PlayerRef,
    mod_api::Hand,
    Axis,
    Vec<[i32; 3]>,
);

thread_local! {
    static LAYERS: std::cell::RefCell<Vec<Layer>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[mod_api::horizon_part]
impl mod_api::ItemPart for VaultItem {
    fn on_block_placed_by(
        &self,
        host: &mut dyn mod_api::ServerHost,
        event: &mut mod_api::OnBlockPlacedByEvent<'_>,
    ) {
        use super::super::items as player_items;
        let Some(player) = event.placer else {
            return;
        };
        if player_items::sneaking(&*host, player) {
            return;
        }
        let Some(dim) = player
            .dimension(&*host)
            .and_then(|d| level_index(&d.to_string()))
        else {
            return;
        };
        let face = match event.face {
            mod_api::BlockFace::Up => Dir::Up,
            mod_api::BlockFace::Down => Dir::Down,
            mod_api::BlockFace::North => Dir::North,
            mod_api::BlockFace::South => Dir::South,
            mod_api::BlockFace::West => Dir::West,
            mod_api::BlockFace::East => Dir::East,
            _ => return,
        };
        let pos = event.pos;
        let o = face.opposite().offset();
        let on = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
        let vault = with_world(|w| {
            let v = &w.dims[dim].vaults;
            let c = v.controller_of(on)?;
            let part = v.parts.get(&c)?;
            Some((c, part.axis, part.width, part.length))
        })
        .flatten();
        let Some((c, axis, width, length)) = vault else {
            return;
        };
        if width == 1 || face.axis() != axis {
            return;
        }
        // The layer past either end of the vault, where the block went.
        let along = Dir::from_axis(axis, true).offset();
        let start = if face == Dir::from_axis(axis, false) {
            [c[0] - along[0], c[1] - along[1], c[2] - along[2]]
        } else {
            [
                c[0] + along[0] * length,
                c[1] + along[1] * length,
                c[2] + along[2] * length,
            ]
        };
        if create_engine::kinetics::Axis::of(start, axis)
            != create_engine::kinetics::Axis::of(pos, axis)
        {
            return;
        }
        let Some(level) = player.level(&*host) else {
            return;
        };
        let mut free = Vec::new();
        for x in 0..width {
            for z in 0..width {
                let p = create_engine::vault::offset(start, axis, 0, x, z);
                let Some(state) = level.block_state(&*host, p) else {
                    return;
                };
                let is_vault = states::id(state).is_some_and(|id| {
                    with_world(|w| w.table.get(id).map(|(b, _)| b))
                        .flatten()
                        .is_some_and(|b| b == Block::ItemVault)
                });
                if is_vault || p == pos {
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
        // The stack still holds the vault just placed (it is used up after this).
        let key = event.stack.item();
        let hand = [mod_api::Hand::Main, mod_api::Hand::Off]
            .into_iter()
            .find(|h| player_items::held(&*host, player, *h).is_some_and(|s| s.item() == key));
        let Some(hand) = hand else {
            return;
        };
        if !player_items::creative(&*host, player) && event.stack.count() < free.len() as u32 + 1 {
            return;
        }
        LAYERS.with_borrow_mut(|l| l.push((dim, player, hand, axis, free)));
    }
}

/// The vault layers asked for in this level: each free place takes a vault of the layer's
/// axis, the player's stack one each outside creative.
pub fn tick_vault_layers(lv: &mut super::super::server::ServerLevel<'_>) {
    use super::super::items as player_items;
    let dim = lv.dim;
    let layers: Vec<Layer> = LAYERS.with_borrow_mut(|l| {
        if l.is_empty() {
            return Vec::new();
        }
        let (here, rest): (Vec<_>, Vec<_>) = l.drain(..).partition(|e| e.0 == dim);
        *l = rest;
        here
    });
    for (_, player, hand, axis, free) in layers {
        let Some(key) = lv.host.block_key("create:item_vault") else {
            continue;
        };
        let constraint = [mod_api::StateConstraint::new("axis", axis_name(axis))];
        let Ok(state) = BlockState::of(
            &mod_api::HostBlockStates::block_states(&*lv.host),
            key,
            &constraint,
        ) else {
            continue;
        };
        let creative = player_items::creative(&*lv.host, player);
        for p in free {
            let replaceable = lv
                .block_state(p)
                .and_then(|s| mod_api::HostBlockStates::block_states(&*lv.host).replaceable(s))
                .unwrap_or(false);
            if !replaceable {
                continue;
            }
            if !creative {
                let has = player_items::held(&*lv.host, player, hand)
                    .is_some_and(|s| s.item().id(&*lv.host) == "create:item_vault");
                if !has {
                    break;
                }
                player_items::shrink_held(lv.host, player, hand, 1);
            }
            lv.set_state(p, state, super::super::server::update_all());
        }
    }
}
