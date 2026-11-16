//! `AssemblyOperatorBlockItem`, the item of the mechanical press, the mechanical mixer, the
//! spout and the deployer: placed on top of what it works on (a horizontal belt, a basin, a
//! depot or a weighted ejector), it goes two above it, leaving room for its head, whisk or
//! nozzle; with that spot taken nothing is placed. Elsewhere it places as a block item does.
//!
//! The placement two above is `BlockItem.place` with the click moved there
//! (`AssemblyOperatorUseContext`: the top face of that spot): the state Create gives the block
//! there (`logistics::placement::placed_state`), the spot free of entities, the placement told
//! to the subsystems (`entity_place`: the placer's ownership, the symmetry wand), the place
//! sound, one used up outside creative.

use super::super::items::{creative, obstructed, sneaking};
use super::super::server::ServerLevel;
use super::super::states;
use super::super::world::with_world;
use create_engine::Block;
use create_engine::belt::Slope;
use create_engine::kinetics::Dir;
use create_engine::parts::StateInfo;
use mod_api::{
    BlockState, ClientHost, EntityPlaceEvent, Flow, HostBlockStates, HostSide, InteractionResult,
    ItemPart, Registrar, ServerHost, UseOnEvent,
};

/// The blocks whose items are `AssemblyOperatorBlockItem`s (`AllBlocks`).
const OPERATORS: [Block; 4] = [
    Block::MechanicalPress,
    Block::MechanicalMixer,
    Block::Spout,
    Block::Deployer,
];

pub(super) fn register(r: &mut dyn Registrar) {
    for block in OPERATORS {
        let item = r.item_key(&format!("create:{}", block.name()));
        r.item_part(item, OperatorItem);
    }
}

/// `operatesOn`: a horizontal belt, a basin, a depot or a weighted ejector.
fn operates_on((block, info): (Block, StateInfo)) -> bool {
    match block {
        Block::Belt => info.belt.is_some_and(|s| s.slope == Slope::Horizontal),
        Block::Basin | Block::Depot | Block::WeightedEjector => true,
        _ => false,
    }
}

/// `place`'s test: the block placed on (behind the clicked face of the spot the item would go)
/// when it is clicked from above and operated on, and so the spot two above it.
/// `info_at` reads Create's blocks, `replaceable` whether a spot can be replaced.
fn moved_spot(
    event: &UseOnEvent<'_>,
    info_at: impl Fn([i32; 3]) -> Option<(Block, StateInfo)>,
    replaceable: impl Fn([i32; 3]) -> bool,
) -> Option<[i32; 3]> {
    if event.face != [0, 1, 0] {
        return None;
    }
    // `context.getClickedPos().relative(context.getClickedFace().getOpposite())`: the clicked
    // block, or the one under it when the clicked block is replaced.
    let p = event.pos;
    let on = if replaceable(p) {
        [p[0], p[1] - 1, p[2]]
    } else {
        p
    };
    info_at(on)
        .filter(|i| operates_on(*i))
        .map(|_| [on[0], on[1] + 2, on[2]])
}

struct OperatorItem;

#[mod_api::horizon_part]
impl ItemPart for OperatorItem {
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Server(host) => use_on(host, event),
            HostSide::Client(host) => predict(host, event),
        }
    }
}

/// The client's answer, as the server's will be, so it does not place the block where the
/// item would go elsewhere.
fn predict(host: &mut dyn ClientHost, event: &UseOnEvent<'_>) -> Option<InteractionResult> {
    let level = host.level(host.dimension())?;
    let facts = host.block_states();
    let state_at = |p: [i32; 3]| level.block_state(&*host, p);
    let info_at = |p: [i32; 3]| {
        let id = state_at(p).and_then(states::id)?;
        super::super::client_table(&*host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
    };
    let replaceable = |p: [i32; 3]| state_at(p).is_some_and(|s| facts.replaceable(s) == Some(true));
    let up = moved_spot(event, info_at, replaceable)?;
    Some(if replaceable(up) {
        InteractionResult::Success
    } else {
        InteractionResult::Fail
    })
}

fn use_on(host: &mut dyn ServerHost, event: &UseOnEvent<'_>) -> Option<InteractionResult> {
    let player = host.player(event.player)?;
    let dimension = player.dimension(&*host)?.to_string();
    let mut lv = ServerLevel::new(host, &dimension)?;
    let up = moved_spot(event, |p| lv.info(p), |p| lv.free(p))?;
    // `if (level.getBlockState(placedOnPos.above(2)).canBeReplaced()) .. else return FAIL`.
    if !lv.free(up) {
        return Some(InteractionResult::Fail);
    }
    let block = OPERATORS
        .into_iter()
        .find(|b| event.stack.item().id(&*lv.host) == format!("create:{}", b.name()))?;
    let dim = lv.dim;
    // `getPlacementState` with the click on the top face of the spot.
    let state = {
        let facts = lv.host.block_states();
        let default = lv.host.block_state_default(states::key(block))?;
        let set = |s: BlockState, k: &str, v: &str| s.with(&facts, k, v).unwrap_or(s);
        let water = lv
            .block_state(up)
            .and_then(|s| lv.host.block_state_string(s))
            .is_some_and(|t| t == "minecraft:water[level=0]");
        let default = if water {
            set(default, "waterlogged", "true")
        } else {
            default
        };
        let rotation = player.entity().rotation(&*lv.host).unwrap_or([0.0, 0.0]);
        let ctx = create_engine::place::Context {
            yaw: rotation[0],
            pitch: rotation[1],
            sneaking: sneaking(&*lv.host, player),
            clicked_face: Dir::Up,
        };
        let info_at = |p: [i32; 3]| {
            let id = lv.block(p);
            (id != 0)
                .then(|| with_world(|w| w.table.get(id).map(|(b, i)| (b, *i))).flatten())
                .flatten()
        };
        match super::super::logistics::placement::placed_state(
            block,
            default,
            water,
            (dim, up),
            ctx,
            &set,
            &info_at,
        ) {
            Some(None) => return Some(InteractionResult::Fail),
            Some(Some(s)) => s,
            None => default,
        }
    };
    // `BlockItem.canPlace`: no entity in the way.
    if obstructed(&lv, state, up) {
        return Some(InteractionResult::Fail);
    }
    // `BlockItem.placeBlock`, heard by the subsystems as the game's placement is.
    let below = lv.block_state([up[0], up[1] - 1, up[2]])?;
    let mut placed = EntityPlaceEvent {
        placer: Some(player.entity()),
        pos: mod_api::BlockPos::new(up[0], up[1], up[2]),
        state,
        placed_against: below,
        placement: mod_api::Placement::Single,
        cancelled: false,
    };
    for s in super::super::SUBSYSTEMS {
        if let Flow::Cancel = s.entity_place(&mut *lv.host, &mut placed) {
            return Some(InteractionResult::Fail);
        }
    }
    let level = lv.level;
    if player
        .place_block(&mut *lv.host, level, up, state, true)
        .is_err()
    {
        return Some(InteractionResult::Fail);
    }
    lv.place_sound(up);
    if !creative(&*lv.host, player)
        && let Some(living) = lv.host.living(player.entity())
    {
        let _ = living.set_hand_item(&mut *lv.host, event.hand, event.stack.clone().shrink(1));
    }
    Some(InteractionResult::Success)
}
