//! A player's clicks on Create's fluid blocks (their `useItemOn`): items emptied into and
//! filled from fluid tanks (creative players; anyone on a creative tank) and item drains
//! (`FluidHelper.tryEmptyItemIntoBE`, `tryFillItemFromBE`), copper casing on a pipe
//! (`EncasableBlock.tryEncase`), a smart pipe's filter, and a whistle growing its stack
//! (`WhistleBlock.useItemOn`). The client predicts the same answer, so a bucket the block took
//! is not also used on the world (`right_click_block`).

use super::super::items;
use super::super::processing::recipes;
use super::super::server::ServerLevel;
use super::super::states;
use super::super::world::with_world;
use super::pipes::pipe_sides_of;
use super::{flag, info_at, state_with};
use crate::glue::kinetics::dir_name;
use create_engine::Block;
use create_engine::drain::Emptying;
use create_engine::kinetics::Dir;
use create_engine::tank::Fluid;
use mod_api::{
    BlockPart, ClientHost, Flow, Hand, HostSide, InteractionResult,
    ItemAfterVerticalCollisionEvent, ItemStack, PlayerRef, RightClickBlockEvent, ServerHost,
    SoundSource, UseItemOnEvent,
};

/// The blocks whose clicks are this module's.
pub(super) const CLICKED: [Block; 8] = [
    Block::FluidTank,
    Block::CreativeFluidTank,
    Block::ItemDrain,
    Block::FluidPipe,
    Block::GlassFluidPipe,
    Block::SmartFluidPipe,
    Block::SteamWhistle,
    Block::SteamWhistleExtension,
];

/// The part on `CLICKED`.
pub(super) struct Clicks;

#[mod_api::horizon_part]
impl BlockPart for Clicks {
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let player = host.player(click.player)?;
        let dimension = click.dimension.key().id().to_owned();
        let mut lv = ServerLevel::new(host, &dimension)?;
        let (block, _) = info_at(&lv, click.pos)?;
        let held = click.stack.filter(|s| s.count() > 0).cloned();
        let used = match block {
            Block::FluidTank | Block::CreativeFluidTank => {
                use_tank(&mut lv, click.pos, block, player, held.as_ref()?)
            }
            Block::ItemDrain => use_drain(&mut lv, click.pos, player, click.hand, held.as_ref()),
            Block::FluidPipe | Block::GlassFluidPipe => {
                let held = held.as_ref()?;
                (held.item().id(&*lv.host) == "create:copper_casing")
                    .then(|| encase(&mut lv, click.pos, block))
                    .is_some()
            }
            Block::SmartFluidPipe => {
                use_smart_pipe(&mut lv, click.pos, player, held.as_ref(), click.hit.point)
            }
            Block::SteamWhistle | Block::SteamWhistleExtension => {
                let held = held.as_ref()?;
                held.item().id(&*lv.host) == "create:steam_whistle"
                    && super::plumbing::grow_whistle_at(&mut lv, click.pos, block)
            }
            _ => false,
        };
        used.then_some(InteractionResult::Success)
    }

    /// An item landing on a drain (`ItemDrainBlock.updateEntityAfterFallOn`).
    fn item_after_vertical_collision(
        &self,
        host: HostSide,
        event: &mut ItemAfterVerticalCollisionEvent<'_>,
    ) -> Flow {
        let HostSide::Server(host) = host else {
            return Flow::Continue;
        };
        if event.client {
            return Flow::Continue;
        }
        let Some(mut lv) = ServerLevel::new(host, event.dimension) else {
            return Flow::Continue;
        };
        if info_at(&lv, event.block_pos).map(|(b, _)| b) == Some(Block::ItemDrain) {
            super::machines::drain_landed(&mut lv, event.block_pos, event.stack);
        }
        Flow::Continue
    }
}

/// What emptying `item` gives (`GenericItemEmptying.emptyItem`): a `create:emptying` recipe's,
/// else a filled bucket's.
pub(crate) fn emptying(item: &str) -> Option<Emptying> {
    let recipes = recipes::with(|r| r.emptying.clone()).unwrap_or_default();
    create_engine::drain::contents(item, &recipes)
}

/// How much of `fluid` filling `item` takes and what it becomes
/// (`GenericItemFilling.getRequiredAmountForItem`, `fillItem`).
pub(crate) fn filling(item: &str, fluid: Fluid) -> Option<(u32, String)> {
    let recipes = recipes::with(|r| r.filling.clone()).unwrap_or_default();
    create_engine::spout::filling(item, fluid, &recipes)
}

/// A stack of `count` of item `id`.
fn stack(lv: &ServerLevel<'_>, id: &str, count: u32) -> Option<ItemStack> {
    super::machines::stack_of(&*lv.host, id, count)
}

/// `FluidHelper.getEmptySound` / `getFillSound` of `fluid`, played at the block
/// (`FluidTankBlock.useItemOn`: 0.5, 1).
fn bucket_sound(lv: &mut ServerLevel<'_>, pos: [i32; 3], fluid: Fluid, fill: bool) {
    let sound = match (fill, fluid == Fluid::Lava) {
        (true, false) => "minecraft:item.bucket.fill",
        (true, true) => "minecraft:item.bucket.fill_lava",
        (false, false) => "minecraft:item.bucket.empty",
        (false, true) => "minecraft:item.bucket.empty_lava",
    };
    let at = pos.map(|v| f64::from(v) + 0.5);
    lv.level
        .play_sound(&mut *lv.host, sound, at, SoundSource::Blocks, 0.5, 1.0);
}

/// `FluidTankBlock.useItemOn`: only creative players, or anyone on a creative tank; an item is
/// emptied into the tank whole, else filled from it; an item that could be either is used up
/// even when neither happened, anything else passes. A survival player's item is swapped for
/// what is left of it (a creative tank keeps the held item, giving a filled one); a creative
/// player's stays. A boiler takes water as supply and gives nothing (`BoilerFluidHandler`).
fn use_tank(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    block: Block,
    player: PlayerRef,
    held: &ItemStack,
) -> bool {
    let dim = lv.dim;
    let creative_tank = block == Block::CreativeFluidTank;
    let creative = items::creative(&*lv.host, player);
    if !creative && !creative_tank {
        return false;
    }
    let id = super::machines::item_name(&*lv.host, held);
    // ITEM_TO_TANK.
    if let Some(e) = emptying(&id) {
        let took = if creative_tank {
            with_world(|w| w.set_creative_fluid(dim, pos, Some(e.fluid)));
            true
        } else {
            with_world(|w| {
                if let Some(took) = w.fill_boiler(dim, pos, e.fluid, e.amount) {
                    return took == e.amount;
                }
                let Some((_, m)) = w.tank(dim, pos) else {
                    return false;
                };
                let mut t = m.tank;
                if t.fill_amount(e.fluid, e.amount, m.capacity()) != e.amount {
                    return false;
                }
                w.set_tank(dim, pos, t);
                true
            })
            .unwrap_or(false)
        };
        if took {
            bucket_sound(lv, pos, e.fluid, false);
            return true;
        }
    }
    // TANK_TO_ITEM.
    let contents = if creative_tank {
        with_world(|w| w.dims[dim].plumbing.creative_fluid(pos))
            .flatten()
            .flatten()
            .map(|f| (f, u32::MAX))
    } else {
        with_world(|w| {
            if w.is_boiler(dim, pos) {
                return None;
            }
            let (_, m) = w.tank(dim, pos)?;
            Some((m.tank.fluid?, m.tank.amount))
        })
        .flatten()
    };
    if let Some((fluid, have)) = contents
        && let Some((need, out)) = filling(&id, fluid)
        && need <= have
    {
        if creative_tank {
            with_world(|w| w.set_creative_fluid(dim, pos, None));
        } else {
            with_world(|w| w.drain_tank(dim, pos, need));
        }
        if !creative && let Some(out) = stack(lv, &out, 1) {
            items::give_back(lv.host, player, out);
        }
        bucket_sound(lv, pos, fluid, true);
        return true;
    }
    // Neither: an item that could hold a fluid is still used up.
    emptying(&id).is_some() || can_be_filled(&id)
}

/// `GenericItemFilling.canItemBeFilled`.
fn can_be_filled(item: &str) -> bool {
    recipes::with(|r| create_engine::spout::fillable(item, &r.filling))
        .unwrap_or(item == "minecraft:bucket")
}

/// `ItemDrainBlock.useItemOn`: a block item passes; an item is emptied into the drain's tank
/// whole when it fits (a survival player's swapped for what is left of it); one that could be
/// emptied is used up all the same; otherwise the drain gives its held stack back.
fn use_drain(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: PlayerRef,
    hand: Hand,
    held: Option<&ItemStack>,
) -> bool {
    let dim = lv.dim;
    let id = held.map(|s| super::machines::item_name(&*lv.host, s));
    if let Some(id) = &id
        && emptying(id).is_none()
        && is_block_item(&*lv.host, id)
    {
        return false;
    }
    if let (Some(held), Some(e)) = (held, id.as_deref().and_then(emptying)) {
        let took = with_world(|w| {
            let Some(d) = w.dims[dim].drains.get_mut(&pos) else {
                return false;
            };
            let room = d.fluid.is_none_or(|(f, n)| {
                f == e.fluid && n + e.amount <= create_engine::drain::CAPACITY
            });
            if !room {
                return false;
            }
            d.fluid = Some((e.fluid, d.fluid.map_or(0, |f| f.1) + e.amount));
            w.dims[dim].analog_checks.push(pos);
            true
        })
        .unwrap_or(false);
        if took {
            if !items::creative(&*lv.host, player) {
                let left = e.leftover.as_deref().and_then(|l| stack(lv, l, 1));
                if held.count() <= 1 {
                    items::set_held(lv.host, player, hand, left);
                } else {
                    items::shrink_held(lv.host, player, hand, 1);
                    if let Some(left) = left {
                        items::give_back(lv.host, player, left);
                    }
                }
            }
            return true;
        }
        return true;
    }
    super::machines::give_back_drained(lv, pos, player);
    true
}

/// `EncasableBlock.tryEncase` with copper casing: a pipe becomes an encased pipe open where it
/// was (a glass pipe along its axis, `AxisPipeBlock`), the casing kept; the casing's place sound
/// (`playEncaseSound`).
fn encase(lv: &mut ServerLevel<'_>, pos: [i32; 3], block: Block) {
    let Some(state) = lv.block_state(pos) else {
        return;
    };
    let sides = if block == Block::FluidPipe {
        pipe_sides_of(lv, state)
    } else {
        info_at(lv, pos).and_then(|(_, i)| i.axis).map_or(0, |a| {
            Dir::from_axis(a, true).bit() | Dir::from_axis(a, false).bit()
        })
    };
    let props: Vec<(&str, &str)> = Dir::ALL
        .into_iter()
        .map(|d| (dir_name(d), flag(sides & d.bit() != 0)))
        .collect();
    if let Some(next) = state_with(lv, "create:encased_fluid_pipe", &props) {
        super::pipes::replace_pipe_state(lv, pos, next);
    }
    let at = pos.map(|v| f64::from(v) + 0.5);
    lv.level.play_sound(
        &mut *lv.host,
        "minecraft:block.copper.place",
        at,
        SoundSource::Blocks,
        1.0,
        0.8,
    );
}

/// Whether a click at `point` (in the world) on the smart pipe at `pos` lands on its filter's
/// slot (`ValueSettingsInputHandler.onBlockActivated`: `testHit`).
fn on_filter_slot(info: &create_engine::parts::StateInfo, pos: [i32; 3], point: [f64; 3]) -> bool {
    let (Some(face), Some(facing)) = (info.face, info.facing) else {
        return false;
    };
    let local = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]));
    create_engine::fluids::smart_pipe_filter_hit(face, facing, local)
}

/// The smart pipe's filter slot (`FilteringBehaviour.onShortInteract`, `forFluids`): a click
/// on the slot with any item but a wrench or an arm sets the filter to it (its fluid, if it
/// holds one: none passes nothing), an empty hand clears it; the pipes look again
/// (`onFilterChanged`). A click elsewhere on the pipe, or by a player who may not use value
/// boxes (sneaking), passes.
fn use_smart_pipe(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: PlayerRef,
    held: Option<&ItemStack>,
    point: [f64; 3],
) -> bool {
    if !super::super::interaction::values::may_use_value_box(&*lv.host, player) {
        return false;
    }
    let Some((_, info)) = info_at(lv, pos) else {
        return false;
    };
    if !on_filter_slot(&info, pos, point) {
        return false;
    }
    let id = held.map(|s| super::machines::item_name(&*lv.host, s));
    if id
        .as_deref()
        .is_some_and(|i| matches!(i, "create:wrench" | "create:mechanical_arm"))
    {
        return false;
    }
    let filter = id.map(|item| {
        let fluid = emptying(&item).map(|e| e.fluid);
        create_engine::fluids::SmartFilter { item, fluid }
    });
    let dim = lv.dim;
    with_world(|w| {
        let d = &mut w.dims[dim];
        let changed = match filter {
            Some(f) => d.plumbing.filters.insert(pos, f.clone()) != Some(f),
            None => d.plumbing.filters.remove(&pos).is_some(),
        };
        if changed {
            d.fluid_transport
                .changed
                .push(create_engine::fluids::runtime::Change::Pipe(pos));
            d.plumbing.filter_changes.push(pos);
            d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        }
    });
    let at = pos.map(|v| f64::from(v) + 0.5);
    lv.level.play_sound(
        &mut *lv.host,
        "minecraft:entity.item_frame.add_item",
        at,
        SoundSource::Blocks,
        0.25,
        0.1,
    );
    true
}

/// The item in the smart pipe's filter slot at `pos`.
pub(crate) fn smart_pipe_filter(dim: usize, pos: [i32; 3]) -> Option<String> {
    with_world(|w| {
        w.dims[dim]
            .plumbing
            .filters
            .get(&pos)
            .map(|f| f.item.clone())
    })
    .flatten()
}

/// `FilteringBehaviour.setFilter` on the smart pipe at `pos` (`forFluids`: the item's fluid, if
/// it holds one); the pipes look again.
pub(crate) fn set_smart_pipe_filter(lv: &mut ServerLevel<'_>, pos: [i32; 3], item: Option<String>) {
    let filter = item.map(|item| {
        let fluid = emptying(&item).map(|e| e.fluid);
        create_engine::fluids::SmartFilter { item, fluid }
    });
    let dim = lv.dim;
    with_world(|w| {
        let d = &mut w.dims[dim];
        let changed = match filter {
            Some(f) => d.plumbing.filters.insert(pos, f.clone()) != Some(f),
            None => d.plumbing.filters.remove(&pos).is_some(),
        };
        if changed {
            d.fluid_transport
                .changed
                .push(create_engine::fluids::runtime::Change::Pipe(pos));
            d.plumbing.filter_changes.push(pos);
            d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        }
    });
}

/// The client's side of the clicks above (`right_click_block`): where the block's `useItemOn`
/// answers SUCCESS on the client too, the click ends there, so the held item (a bucket) is not
/// also used on the world.
pub(super) fn predict(host: &mut dyn ClientHost, event: &mut RightClickBlockEvent<'_>) -> Flow {
    let click = event.click;
    let Some(player) = host.player(click.player) else {
        return Flow::Continue;
    };
    let state = host
        .level(host.dimension())
        .and_then(|l| l.block_state(&*host, click.pos))
        .map_or(0, states::id_or_none);
    if state == 0 {
        return Flow::Continue;
    }
    let Some(block) = super::super::client_table(&*host, |t| t.get(state).map(|(b, _)| b))
        .flatten()
        .filter(|b| CLICKED.contains(b))
    else {
        return Flow::Continue;
    };
    // The block's own use is skipped while sneaking with something in either hand.
    let bypassed = items::sneaking(&*host, player)
        && Hand::BOTH
            .iter()
            .any(|&h| items::held(&*host, player, h).is_some());
    if bypassed {
        return Flow::Continue;
    }
    let id = click
        .stack
        .filter(|s| s.count() > 0)
        .map(|s| s.item().id(&*host));
    let fluid_item = id.as_deref().is_some_and(holds_fluid);
    let answers = match block {
        Block::CreativeFluidTank => fluid_item,
        Block::FluidTank => fluid_item && items::creative(&*host, player),
        Block::ItemDrain => fluid_item || !id.as_deref().is_some_and(|i| is_block_item(&*host, i)),
        Block::FluidPipe | Block::GlassFluidPipe => id.as_deref() == Some("create:copper_casing"),
        Block::SmartFluidPipe => {
            let info =
                super::super::client_table(&*host, |t| t.get(state).map(|(_, i)| *i)).flatten();
            info.is_some_and(|i| on_filter_slot(&i, click.pos, click.hit.point))
                && !id
                    .as_deref()
                    .is_some_and(|i| matches!(i, "create:wrench" | "create:mechanical_arm"))
                && super::super::interaction::values::may_use_value_box(&*host, player)
        }
        Block::SteamWhistle | Block::SteamWhistleExtension => {
            id.as_deref() == Some("create:steam_whistle")
        }
        _ => false,
    };
    if !answers {
        return Flow::Continue;
    }
    *event.cancellation_result = InteractionResult::Success;
    Flow::Cancel
}

/// Whether an item can hold a fluid as the client sees it (`canItemBeEmptied` or
/// `canItemBeFilled` without the recipes): buckets, bottles and potions.
fn holds_fluid(item: &str) -> bool {
    item == "minecraft:bucket"
        || item.ends_with("_bucket")
        || matches!(
            item,
            "minecraft:glass_bottle"
                | "minecraft:potion"
                | "minecraft:splash_potion"
                | "minecraft:lingering_potion"
                | "minecraft:honey_bottle"
                | "create:builders_tea"
        )
        || Fluid::of_bucket(item).is_some()
}

/// Whether item `id` places a block (`instanceof BlockItem`).
fn is_block_item(host: &dyn mod_api::Host, id: &str) -> bool {
    mod_api::Identifier::parse(id)
        .ok()
        .and_then(|i| host.item_key(&i))
        .is_some_and(|k| host.item_block(k).is_some())
}
