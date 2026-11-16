//! Create's metal ladders (`MetalLadderBlock`): one hangs from a ladder of the same facing above
//! it where it has no wall behind (`canSurvive`), drops when neither holds it (`updateShape`, on
//! any neighbour's change), and a metal ladder item clicked on one extends the column up or
//! down as the player looks (`PlacementHelper`, within `placementAssistRange`, 4 more with an
//! extendo grip in hand).

use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::{add, block_of, dir_named, prop, use_one, with_props};
use create_engine::Block;
use create_engine::kinetics::Dir;
use mod_api::{
    BlockClick, BlockState, CanSurviveEvent, Hand, InteractionResult, ItemStack, LevelHost,
    PlayerRef, Registrar, ServerHost, SupportType, UseItemOnEvent,
};

/// Create's metal ladders.
const LADDERS: [Block; 3] = [
    Block::AndesiteLadder,
    Block::BrassLadder,
    Block::CopperLadder,
];

pub(super) fn register(r: &mut dyn Registrar) {
    for block in LADDERS {
        r.block_part(states::key(block), Ladder);
    }
}

/// Whether engine state `id` is a metal ladder.
pub(super) fn is_ladder(id: u32) -> bool {
    block_of(id).is_some_and(|(b, _)| LADDERS.contains(&b))
}

/// `facing=` of a state's text.
fn facing_in(text: &str) -> Option<Dir> {
    let (_, props) = states::split(text);
    props
        .iter()
        .find(|(k, _)| *k == "facing")
        .and_then(|(_, v)| dir_named(v))
}

struct Ladder;

#[mod_api::horizon_part]
impl mod_api::BlockPart for Ladder {
    /// `MetalLadderBlock.canSurvive`: the same ladder facing the same way above holds it; else
    /// the ladder's own rule (a sturdy wall behind).
    fn can_survive(&self, level: &dyn LevelHost, event: &mut CanSurviveEvent) -> Option<bool> {
        let text = level.state_string(event.state)?;
        let facing = facing_in(&text)?;
        let above = level.block_state([event.pos.x, event.pos.y + 1, event.pos.z])?;
        let up = level.state_string(above)?;
        (above.block() == event.state.block() && facing_in(&up) == Some(facing)).then_some(true)
    }

    /// `MetalLadderBlock.useItemOn`: the placement helper, not sneaking.
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let stack = click.stack?;
        let player = host.player(click.player)?;
        if super::sneaking(&*host, player) || player.may_build(&*host) != Some(true) {
            return None;
        }
        let dimension = click.dimension.key().id().to_owned();
        let mut lv = ServerLevel::new(host, &dimension)?;
        let held = lv.host.item_block(stack.item())?;
        let base = lv.host.block_state_default(held)?;
        if !states::id(base).is_some_and(is_ladder) {
            return None;
        }
        extend(&mut lv, player, click, stack, event.state, base)
    }
}

/// Whether the ladder `state` holds at `pos`: a sturdy wall behind (`LadderBlock.canSurvive`), or
/// the same ladder facing the same way above.
fn holds(lv: &ServerLevel<'_>, pos: [i32; 3], state: BlockState, facing: Dir) -> bool {
    let behind = add(pos, facing.opposite());
    let wall =
        lv.level
            .block_face_sturdy(&*lv.host, behind, super::face_of(facing), SupportType::Full)
            == Some(true);
    let above = [pos[0], pos[1] + 1, pos[2]];
    wall || lv.block_state(above).is_some_and(|up| {
        up.block() == state.block()
            && prop(&*lv.host, up, "facing").and_then(|f| dir_named(&f)) == Some(facing)
    })
}

/// Whether the player holds an extendo grip (`singleRangeAttributeModifier`).
pub(super) fn gripping(lv: &ServerLevel<'_>, player: PlayerRef) -> bool {
    let host = &*lv.host;
    host.living(player.entity()).is_some_and(|l| {
        [Hand::Main, Hand::Off].into_iter().any(|h| {
            l.hand_item(host, h)
                .is_some_and(|s| s.item().id(host) == "create:extendo_grip")
        })
    })
}

/// `MetalLadderBlock.PlacementHelper.getOffset` and `placeInWorld`: past the ladders above (the
/// player looking up) or below the clicked one, the held ladder with the clicked one's facing.
fn extend(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    click: &BlockClick<'_>,
    stack: &ItemStack,
    state: BlockState,
    base: BlockState,
) -> Option<InteractionResult> {
    let (at, facing) = target(lv, player, click.pos, state)?;
    let water = lv
        .block_state(at)
        .and_then(|s| lv.state_name(s))
        .is_some_and(|n| n == "minecraft:water");
    let next = with_props(
        &*lv.host,
        base,
        &[
            ("facing", super::super::kinetics::dir_name(facing)),
            ("waterlogged", if water { "true" } else { "false" }),
        ],
    )?;
    lv.set_state(at, next, update_all());
    lv.place_sound(at);
    use_one(&mut *lv.host, player, click.hand, stack);
    Some(InteractionResult::Success)
}

/// `MetalLadderBlock.PlacementHelper.getOffset`: where a ladder goes past the line from the
/// clicked one at `pos` (`state`), and its facing.
fn target(
    lv: &ServerLevel<'_>,
    player: PlayerRef,
    pos: [i32; 3],
    state: BlockState,
) -> Option<([i32; 3], Dir)> {
    let pitch = player.entity().rotation(&*lv.host).map_or(0.0, |r| r[1]);
    let dir = if pitch < 0.0 { Dir::Up } else { Dir::Down };
    let mut range = create_engine::place::PLACEMENT_ASSIST_RANGE;
    if gripping(lv, player) {
        range += 4;
    }
    // `attachedLadders`: any ladder (`LadderBlock`) in a line from the clicked one.
    let mut ladders = 0;
    let mut at = add(pos, dir);
    while lv.block_state(at).is_some_and(|s| {
        is_ladder(lv.block(at)) || lv.state_name(s).as_deref() == Some("minecraft:ladder")
    }) {
        ladders += 1;
        at = add(at, dir);
    }
    if ladders >= range {
        return None;
    }
    let facing = prop(&*lv.host, state, "facing").and_then(|f| dir_named(&f))?;
    // `state.canSurvive(world, newPos)`: the clicked ladder's rule at the new place.
    if !holds(lv, at, state, facing) || !lv.free(at) {
        return None;
    }
    Some((at, facing))
}

/// The ladder helper's target for the indicator: a metal ladder item on a metal ladder
/// (`matchesItem`: the held block is one, `matchesState`: the clicked one is).
pub(crate) fn helper_target(
    lv: &ServerLevel<'_>,
    player: PlayerRef,
    pos: [i32; 3],
    item: mod_api::ItemKey,
) -> Option<crate::glue::interaction::HelperTarget> {
    let held = lv.host.item_block(item)?;
    let base = lv.host.block_state_default(held)?;
    if !states::id(base).is_some_and(is_ladder) || !is_ladder(lv.block(pos)) {
        return None;
    }
    let state = lv.block_state(pos)?;
    target(lv, player, pos, state).map(|(at, facing)| {
        let facing = create_engine::copycat::dir_name(facing).to_owned();
        (at, vec![("facing", facing)])
    })
}

/// A ladder whose neighbour changed drops when nothing holds it (`updateShape`).
pub(super) fn check(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    if !is_ladder(lv.block(pos)) {
        return;
    }
    let Some(state) = lv.block_state(pos) else {
        return;
    };
    let Some(facing) = prop(&*lv.host, state, "facing").and_then(|f| dir_named(&f)) else {
        return;
    };
    if !holds(lv, pos, state, facing) {
        lv.destroy_block(pos, true);
    }
}
