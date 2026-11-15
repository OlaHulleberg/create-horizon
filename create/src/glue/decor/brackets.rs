//! Brackets on shafts, cogs and pipes (`BracketBlockItem`, `BracketedBlockEntityBehaviour`,
//! `IWrenchableWithBracket`): put on with a bracket item, taken off with the wrench, dropped when
//! the block changes; drawn by the block's kinetic block entity as the bracket block's model.

use super::super::kinetics;
use super::super::persist;
use super::super::server::ServerLevel;
use super::super::states;
use super::{
    BRACKET, block_of, bracketed, creative, girders, give, player_dimension, prop, sneaking,
    use_one, with,
};
use create_engine::Block;
use create_engine::decor::{self, BracketKind};
use create_engine::kinetics::{Axis, Dir};
use mod_api::{
    BlockEntityPart, BlockEntitySubmitEvent, BlockPart, ClientHost, HostSide, InteractionResult,
    ItemPart, PlayerRef, Registrar, ServerHost, Tag, UseItemOnEvent, UseOnEvent,
};

pub(super) fn register(r: &mut dyn Registrar) {
    for name in ["create:metal_bracket", "create:wooden_bracket"] {
        let item = r.item_key(name);
        r.item_part(item, BracketItem);
    }
    for block in [
        Block::Shaft,
        Block::Cogwheel,
        Block::LargeCogwheel,
        Block::FluidPipe,
    ] {
        r.block_part(states::key(block), Bracketed);
    }
    if let Some(kind) = kinetics::kind()
        && let Err(e) = r.block_entity_part(&kind, BracketEntity)
    {
        crate::create_log!("[create] the bracket drawing part was refused: {e:?}");
    }
}

/// `FluidPropagator.getStraightPipeAxis` of a fluid pipe's state: the axis of its only two open
/// sides when they face each other.
fn straight_pipe_axis(open: impl Fn(Dir) -> bool) -> Option<Axis> {
    let mut found = None;
    let mut connections = 0;
    for axis in [Axis::X, Axis::Y, Axis::Z] {
        let (a, b) = (
            open(Dir::from_axis(axis, false)),
            open(Dir::from_axis(axis, true)),
        );
        connections += usize::from(a) + usize::from(b);
        if a && b {
            if found.is_some() {
                return None;
            }
            found = Some(axis);
        }
    }
    (connections == 2).then_some(found).flatten()
}

/// The bracket items.
struct BracketItem;

#[mod_api::horizon_part]
impl ItemPart for BracketItem {
    /// `BracketBlockItem.useOn`: on a shaft, cog or pipe, the bracket for the clicked face (else
    /// the face toward the player) replaces any it had, which goes back to the player; the
    /// bracket's block is never placed.
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        let HostSide::Server(host) = host else {
            return Some(InteractionResult::Success);
        };
        let player = host.player(event.player)?;
        let dimension = player_dimension(&*host, player)?;
        let mut lv = ServerLevel::new(host, &dimension)?;
        let wooden = event.stack.item().id(&*lv.host) == "create:wooden_bracket";
        use_bracket(&mut lv, player, event, wooden);
        Some(InteractionResult::Success)
    }
}

fn use_bracket(lv: &mut ServerLevel<'_>, player: PlayerRef, event: &UseOnEvent<'_>, wooden: bool) {
    let pos = event.pos;
    let Some((block, info)) = block_of(lv.block(pos)).filter(|(b, _)| bracketed(*b)) else {
        // `behaviour == null`: FAIL, nothing is placed.
        return;
    };
    let (kind, target) = match block {
        Block::Shaft => (BracketKind::Shaft, info.axis),
        Block::FluidPipe => {
            let state = lv.block_state(pos);
            let host = &*lv.host;
            let open = |d: Dir| {
                state
                    .and_then(|s| prop(host, s, kinetics::dir_name(d)))
                    .as_deref()
                    == Some("true")
            };
            (BracketKind::Pipe, straight_pipe_axis(open))
        }
        _ => (BracketKind::Cog, info.axis),
    };
    let Some(clicked) = Dir::from_offset(event.face) else {
        return;
    };
    // `Direction.orderedByNearest(player)[0].getOpposite()`: the face toward the player.
    let [yaw, pitch] = player.entity().rotation(&*lv.host).unwrap_or([0.0; 2]);
    let toward = create_engine::place::ordered_by_nearest(yaw, pitch)[0].opposite();
    let Some(new) = decor::bracket_for_click(wooden, target, kind, clicked, toward) else {
        return;
    };
    let dim = lv.dim;
    let old = with(dim, |d| d.brackets.get(&pos).copied());
    if old == Some(new) {
        return;
    }
    let sound = if wooden {
        "minecraft:block.scaffolding.place"
    } else {
        "minecraft:block.netherite_block.place"
    };
    lv.block_sound(sound, pos, 0.75, 1.0);
    with(dim, |d| {
        d.brackets.insert(pos, new);
        d.shown.push(pos);
    });
    girders::around(dim, pos);
    persist::mark_pos_unsaved(dim, pos);
    if !creative(&*lv.host, player) {
        use_one(&mut *lv.host, player, event.hand, event.stack);
        if let Some(b) = old {
            give(&mut *lv.host, player, b.block());
        }
    }
}

/// `IWrenchableWithBracket.onWrenched`: a wrench takes the bracket off first, back to `player`
/// (not in creative). True when there was one.
fn wrench(lv: &mut ServerLevel<'_>, pos: [i32; 3], player: PlayerRef) -> bool {
    let dim = lv.dim;
    let Some(b) = with(dim, |d| d.brackets.remove(&pos)) else {
        return false;
    };
    removed_effect(lv, pos, b);
    with(dim, |d| d.shown.push(pos));
    girders::around(dim, pos);
    persist::mark_pos_unsaved(dim, pos);
    if !creative(&*lv.host, player) {
        give(&mut *lv.host, player, b.block());
    }
    true
}

/// `BracketedBlockEntityBehaviour.removeBracket`: the bracket's break particles and sound
/// (`levelEvent(PARTICLES_DESTROY_BLOCK, pos, Block.getId(bracket))`).
pub(super) fn removed_effect(lv: &mut ServerLevel<'_>, pos: [i32; 3], b: decor::Bracket) {
    if let Some(state) = lv.host.block_state_id(&shown_text(Some(b))) {
        lv.destroy_effect(pos, state);
    }
}

/// Shafts, cogs and pipes: the wrench's `onWrenched` reaches their bracket first.
struct Bracketed;

#[mod_api::horizon_part]
impl BlockPart for Bracketed {
    /// `WrenchItem.useOn` → `IWrenchableWithBracket.onWrenched`, not sneaking (a sneaking wrench
    /// picks the block up, its bracket dropping as it goes): the bracket comes off and the block
    /// stays as it is. Without a bracket the wrench does what it does to the block.
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let stack = click.stack?;
        if stack.item().id(&*host) != "create:wrench" {
            return None;
        }
        let player = host.player(click.player)?;
        if sneaking(&*host, player) {
            return None;
        }
        let dimension = click.dimension.key().id().to_owned();
        let mut lv = ServerLevel::new(host, &dimension)?;
        wrench(&mut lv, click.pos, player).then_some(InteractionResult::Success)
    }
}

/// The bracket a shaft's, cog's or pipe's block entity draws.
struct BracketEntity;

#[mod_api::horizon_part]
impl BlockPart for BracketEntity {}

#[mod_api::horizon_part]
impl BlockEntityPart for BracketEntity {
    /// `BracketedKineticBlockEntityRenderer`: the bracket block's model, still.
    fn block_entity_submit(&self, _host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some(Tag::String(text)) = event.entity.nbt.get(&BRACKET.into()) else {
            return;
        };
        let text = text.to_string_lossy();
        let (name, props) = states::split(&text);
        let block = match name {
            "create:metal_bracket" => Block::MetalBracket,
            "create:wooden_bracket" => Block::WoodenBracket,
            _ => return,
        };
        event.draw.block(states::key(block), &props);
        event.draw.keep();
    }
}

/// The text a bracket is shown as (`create:metal_bracket[axis_along_first=..,facing=..,type=..]`),
/// empty for none.
pub(super) fn shown_text(b: Option<decor::Bracket>) -> String {
    b.map(|b| {
        format!(
            "{}[axis_along_first={},facing={},type={}]",
            b.block(),
            b.along_first,
            kinetics::dir_name(b.facing),
            b.kind.name()
        )
    })
    .unwrap_or_default()
}

/// A shaft, cog or pipe riding a moving structure: its bracket as it was taken.
pub(super) fn draw_carried(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    BracketEntity.block_entity_submit(host, event);
}
