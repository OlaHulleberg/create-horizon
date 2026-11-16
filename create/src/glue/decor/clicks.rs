//! Clicks on Create's decorative blocks (`useItemOn` / `useWithoutItem`): placards, clipboards,
//! the desk bell, the creative crate's supply slot, the item hatch, a shaft on a girder and a
//! table cloth on a table cloth; and the left clicks that take a placard's item back or collect a
//! clipboard. Also the desk bell's signal.

use super::super::persist;
use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::{
    Timer, add, block_of, dir, flag, girders, player_dimension, sneaking, stack_of, use_one, with,
    with_props,
};
use create_engine::Block;
use create_engine::decor;
use create_engine::kinetics::Dir;
use mod_api::{
    BlockClick, BlockSignalEvent, Flow, Hand, HostBlockStates, HostTransaction, ITEMS,
    InteractionResult, IsSignalSourceEvent, ItemResource, ItemStack, LeftClickBlockEvent,
    LevelHost, NeighbourUpdate, PlayerRef, Registrar, ServerHost, Text, UseItemOnEvent,
    UseWithoutItemEvent,
};

pub(super) fn register(r: &mut dyn Registrar) {
    for block in [
        Block::Placard,
        Block::Clipboard,
        Block::DeskBell,
        Block::CreativeCrate,
        Block::ItemHatch,
        Block::MetalGirder,
    ] {
        r.block_part(states::key(block), DecorBlock);
    }
    for &block in Block::ALL.iter() {
        if block.name().ends_with("_table_cloth") {
            r.block_part(states::key(block), TableCloth);
        }
        if is_sail(block) {
            r.block_part(states::key(block), Sail);
        }
    }
}

/// `SailBlock`: the sail frame and the sails of every colour.
fn is_sail(block: Block) -> bool {
    block == Block::SailFrame || block.name().ends_with("_sail")
}

/// A click's player and level.
fn clicked<'a>(
    host: &'a mut dyn ServerHost,
    click: &BlockClick<'_>,
    dimension: &'a mut String,
) -> Option<(PlayerRef, ServerLevel<'a>)> {
    let player = host.player(click.player)?;
    *dimension = click.dimension.key().id().to_owned();
    Some((player, ServerLevel::new(host, dimension)?))
}

/// The hit's place in the block at `pos`.
fn local(click: &BlockClick<'_>) -> [f64; 3] {
    let (p, at) = (click.hit.point, click.pos);
    [0, 1, 2].map(|i| p[i] - f64::from(at[i]))
}

/// The placard, clipboard, desk bell, creative crate, item hatch and girder.
struct DecorBlock;

#[mod_api::horizon_part]
impl mod_api::BlockPart for DecorBlock {
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let mut dimension = String::new();
        let (player, mut lv) = clicked(host, click, &mut dimension)?;
        let (block, info) = block_of(states::id(event.state)?)?;
        let back = sneaking(&*lv.host, player);
        match block {
            // `PlacardBlock.useItemOn`: sneaking passes; else the item goes on, or matches.
            Block::Placard => {
                if back {
                    return Some(InteractionResult::TryWithEmptyHand);
                }
                Some(use_placard(&mut lv, player, click, info.powered))
            }
            // `ClipboardBlock.useItemOn`: `breakAndCollect` when sneaking; the pages' screen
            // otherwise (the clipboard port's).
            Block::Clipboard => {
                if back {
                    collect_clipboard(&mut lv, player, click.pos);
                } else if click.hand == Hand::Main {
                    // `useWithoutItem`: its pages' screen (the clipboard port's).
                    let stack = with(lv.dim, |d| d.clipboards.get(&click.pos).cloned())
                        .or_else(|| stack_of(&*lv.host, "create:clipboard", 1));
                    if let Some(stack) = stack {
                        crate::glue::display::clipboard::open(
                            lv.host,
                            player,
                            Some(click.pos),
                            &stack,
                        );
                    }
                }
                Some(InteractionResult::Success)
            }
            Block::CreativeCrate => use_crate(&mut lv, player, click),
            Block::ItemHatch => use_hatch(&mut lv, player, click, info.facing?, back),
            // `GirderBlock.useItemOn` with a shaft: an encased shaft.
            Block::MetalGirder => {
                let stack = click.stack?;
                // The placement helper (`GirderPlacementHelper`).
                if stack.item().id(&*lv.host) == "create:metal_girder" {
                    return girders::helper(&mut lv, player, click, stack);
                }
                if stack.item().id(&*lv.host) != "create:shaft" {
                    return None;
                }
                let clicked = Dir::from_offset(click.hit.face).unwrap_or(Dir::Up);
                girders::encase(&mut lv, click.pos, event.state, clicked);
                use_one(&mut *lv.host, player, click.hand, stack);
                Some(InteractionResult::Success)
            }
            _ => None,
        }
    }

    /// `DeskBellBlock.useWithoutItem`: its ding, `powered` for 20 ticks.
    fn use_without_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseWithoutItemEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let (block, _) = block_of(states::id(event.state)?)?;
        let mut dimension = String::new();
        let (_, mut lv) = clicked(host, click, &mut dimension)?;
        match block {
            Block::DeskBell => {
                ring_desk_bell(&mut lv, click.pos);
                Some(InteractionResult::Success)
            }
            // A sneaking click the placard or clipboard passed: the lever template must not flip.
            Block::Placard | Block::Clipboard => Some(InteractionResult::Pass),
            _ => None,
        }
    }

    /// `DeskBellBlock.getDirectSignal` / `getSignal`: 15 while pressed, strong only into the block
    /// it stands on.
    fn block_signal(&self, _level: &dyn LevelHost, event: &mut BlockSignalEvent) {
        let Some(id) = states::id(event.state) else {
            return;
        };
        let Some((Block::DeskBell, info)) = block_of(id) else {
            return;
        };
        // The hook names the face touching the reader; vanilla's direction points from the reader
        // to the block.
        let side = dir(event.face).opposite();
        event.signal = decor::desk_bell_signal(
            info.powered,
            info.facing.unwrap_or(Dir::Up),
            side,
            event.direct,
        );
    }

    /// `DeskBellBlock.isSignalSource`.
    fn is_signal_source(&self, _level: &dyn LevelHost, event: &mut IsSignalSourceEvent) {
        if states::id(event.state).and_then(block_of).map(|(b, _)| b) == Some(Block::DeskBell) {
            event.source = true;
        }
    }
}

/// `PlacardBlock.useItemOn`, not sneaking: the held item (one of it) goes on an empty placard; on
/// a full one, the same item pulses it powered with the confirm sound, another denies.
fn use_placard(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    click: &BlockClick<'_>,
    powered: bool,
) -> InteractionResult {
    let pos = click.pos;
    let dim = lv.dim;
    let held = click.stack;
    let shown = with(dim, |d| d.placards.get(&pos).cloned());
    let same = match (&held, &shown) {
        (Some(a), Some(b)) => a.is_stackable_with(b),
        _ => false,
    };
    let may_build = player.may_build(&*lv.host) == Some(true);
    match decor::use_placard(
        false,
        may_build,
        held.is_none(),
        shown.is_none(),
        powered,
        same,
    ) {
        decor::PlacardUse::Put => {
            let Some(held) = held else {
                return InteractionResult::Fail;
            };
            let mut one = held.clone();
            let _ = one.set_count(1);
            lv.block_sound("minecraft:entity.item_frame.add_item", pos, 1.0, 1.0);
            with(dim, |d| {
                d.placards.insert(pos, one);
                d.shown.push(pos);
            });
            use_one(&mut *lv.host, player, click.hand, held);
            persist::mark_pos_unsaved(dim, pos);
            InteractionResult::Success
        }
        decor::PlacardUse::Pulse => {
            // AllSoundEvents.CONFIRM: the note block's bell at .5, .8.
            lv.block_sound("minecraft:block.note_block.bell", pos, 0.5, 0.8);
            power(lv, pos, true);
            let due = lv.game_time() + i64::from(decor::PLACARD_PULSE);
            with(dim, |d| d.timers.push((pos, due, Timer::Placard)));
            InteractionResult::Success
        }
        // AllSoundEvents.DENY: the note block's bass at 1, .5.
        decor::PlacardUse::Deny => {
            lv.block_sound("minecraft:block.note_block.bass", pos, 1.0, 0.5);
            InteractionResult::Success
        }
        decor::PlacardUse::Fail => InteractionResult::Fail,
        decor::PlacardUse::Pass => InteractionResult::TryWithEmptyHand,
    }
}

/// The face a placard or desk bell hangs on: the block it powers strongly (`getConnectedDirection`
/// for a placard, the bell's facing).
fn support(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Dir> {
    let (block, info) = block_of(lv.block(pos))?;
    let facing = info.facing?;
    match block {
        Block::Placard => {
            let state = lv.block_state(pos)?;
            let face = super::prop(&*lv.host, state, "face").unwrap_or_default();
            Some(decor::connected_direction(&face, facing).opposite())
        }
        _ => Some(facing.opposite()),
    }
}

/// `powered` set (`UPDATE_ALL`), then the neighbours of the block it hangs on told
/// (`updateNeighbours`).
pub(super) fn power(lv: &mut ServerLevel<'_>, pos: [i32; 3], on: bool) {
    lv.set_property(pos, "powered", flag(on), update_all());
    let (Some(state), Some(side)) = (lv.block_state(pos), support(lv, pos)) else {
        return;
    };
    let _ = lv.level.update_neighbors(
        &mut *lv.host,
        add(pos, side),
        state,
        NeighbourUpdate::Neighbours,
    );
}

/// `DeskBellBlock.useWithoutItem`: its ding (`AllSoundEvents.DESK_BELL_USE`), `powered` for 20
/// ticks.
fn ring_desk_bell(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    lv.block_sound("create:desk_bell", pos, 1.0, 1.0);
    power(lv, pos, true);
    super::bell::rang(lv, pos);
    let due = lv.game_time() + i64::from(decor::DESK_BELL_TICKS);
    with(lv.dim, |d| {
        d.timers.retain(|t| !(t.0 == pos && t.2 == Timer::Bell));
        d.timers.push((pos, due, Timer::Bell));
    });
}

/// `ValueSettingsInputHandler` may use a value box: a player who may build, not sneaking, not
/// holding a clipboard.
fn may_use_value_box(lv: &ServerLevel<'_>, player: PlayerRef, click: &BlockClick<'_>) -> bool {
    player.may_build(&*lv.host) == Some(true)
        && !sneaking(&*lv.host, player)
        && click
            .stack
            .is_none_or(|s| s.item().id(&*lv.host) != "create:clipboard")
}

/// `FilteringBehaviour` on the creative crate's top: an item sets its supply, an empty hand
/// clears it.
fn use_crate(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    click: &BlockClick<'_>,
) -> Option<InteractionResult> {
    if !may_use_value_box(lv, player, click)
        || !create_engine::board::hits_box(decor::CRATE_SLOT, local(click))
    {
        return None;
    }
    let held = click.stack.map(|s| s.item().id(&*lv.host));
    if held.as_deref() == Some("create:wrench") {
        return None;
    }
    let pos = click.pos;
    with(lv.dim, |d| match held {
        Some(item) => d.supplies.insert(pos, item),
        None => d.supplies.remove(&pos),
    });
    persist::mark_pos_unsaved(lv.dim, pos);
    lv.block_sound("minecraft:entity.item_frame.add_item", pos, 0.25, 0.1);
    Some(InteractionResult::Success)
}

/// `ItemHatchBlock.useItemOn`: its filter slot takes the held item (`FilteringBehaviour`); else
/// the held stack (sneaking: everything outside the hotbar) goes into the inventory it faces,
/// as much as fits and the filter passes.
fn use_hatch(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    click: &BlockClick<'_>,
    facing: Dir,
    back: bool,
) -> Option<InteractionResult> {
    let held = click.stack.map(|s| s.item().id(&*lv.host));
    if back && held.as_deref() == Some("create:wrench") {
        return None;
    }
    let pos = click.pos;
    let dim = lv.dim;
    if !back
        && decor::hatch_filter_hit(facing, local(click))
        && held.as_deref() != Some("create:wrench")
    {
        // The slot takes the main hand's click (`ValueSettingsInputHandler` takes the event);
        // the client, not knowing it, also asks for the off hand, which leaves it as it is.
        if click.hand != Hand::Main {
            return Some(InteractionResult::Success);
        }
        // `FilteringBehaviour.onShortInteract`: a filter item leaves the hand (outside
        // creative), the one the slot held comes back.
        let filter = crate::glue::items::filters::take_filter(lv.host, player, click.hand);
        let old = with(dim, |d| {
            d.shown.push(pos);
            match filter {
                Some(f) => d.hatch_filters.insert(pos, f),
                None => d.hatch_filters.remove(&pos),
            }
        });
        persist::mark_pos_unsaved(dim, pos);
        crate::glue::items::filters::return_filter(lv, player, pos, old);
        return Some(InteractionResult::Success);
    }
    let target = add(pos, facing);
    let filter = with(dim, |d| d.hatch_filters.get(&pos).cloned());
    let selected = player.selected_slot(&*lv.host).unwrap_or(0);
    let slots: Vec<(u32, ItemStack)> = player.inventory_slots(&*lv.host).unwrap_or_default();
    let level = lv.level;
    let host = &mut *lv.host;
    // `getCapability(ItemHandler.BLOCK, pos, null)`: the inventory as a whole, from no side.
    let Some(inventory) = level.capability(host, &ITEMS, target, None) else {
        // No inventory there (`targetInv == null`).
        return Some(InteractionResult::Success);
    };
    let mut any = false;
    for slot in decor::hatch_slots(back, selected) {
        let Some((_, stack)) = slots.iter().find(|(i, _)| *i as usize == slot) else {
            continue;
        };
        let id = stack.item().id(&*host);
        // `canFitInsideContainerItems`: not shulker boxes, nor toolboxes
        // (`UncontainableBlockItem`).
        if id.ends_with("shulker_box")
            || (id.starts_with("create:") && id.ends_with("_toolbox"))
            || !crate::glue::items::filters::passes(&*host, filter.as_ref(), stack)
        {
            continue;
        }
        let count = stack.count();
        let moved: u32 = host
            .transaction(|host, tx| {
                let n = inventory.insert_any(host, tx, &ItemResource::of(stack), count);
                if n == 0 {
                    return Err(());
                }
                let rest = stack.clone().shrink(n);
                player
                    .set_inventory_slot(host, slot as u32, rest)
                    .map_err(|_| ())?;
                Ok(n)
            })
            .unwrap_or(0);
        any |= moved > 0;
    }
    if !any {
        return Some(InteractionResult::Success);
    }
    // AllSoundEvents.ITEM_HATCH: a barrel opening and a netherite block placed.
    lv.block_sound("minecraft:block.barrel.open", pos, 0.25, 1.4);
    lv.block_sound("minecraft:block.netherite_block.place", pos, 0.75, 1.15);
    lv.set_property(pos, "open", "true", update_all());
    let due = lv.game_time() + decor::HATCH_OPEN_TICKS;
    with(dim, |d| d.timers.push((pos, due, Timer::Hatch)));
    let key = if back {
        "create.item_hatch.deposit_inventory"
    } else {
        "create.item_hatch.deposit_item"
    };
    let _ = player.action_bar(&mut *lv.host, Text::translate(key, Vec::new()));
    Some(InteractionResult::Success)
}

/// `ClipboardBlock.breakAndCollect`: the clipboard leaves the world without drops and goes to
/// the player's hand (or inventory) with its pages.
fn collect_clipboard(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(stack) = with(dim, |d| d.clipboards.remove(&pos))
        .or_else(|| stack_of(&*lv.host, "create:clipboard", 1))
    else {
        return;
    };
    persist::mark_pos_unsaved(dim, pos);
    if let Some(air) = lv.host.block_state_id("minecraft:air") {
        lv.put(pos, air);
    }
    let host = &mut *lv.host;
    let empty_hand = host
        .living(player.entity())
        .is_some_and(|l| l.hand_item(&*host, Hand::Main).is_none());
    if empty_hand && let Some(living) = host.living(player.entity()) {
        let _ = living.set_hand_item(host, Hand::Main, Some(stack));
    } else {
        let _ = player.give_stack(host, stack);
    }
}

/// A left click: a placard gives its item back (`PlacardBlock.attack`, the click digging on), a
/// clipboard is collected.
pub(super) fn left_click(host: &mut dyn ServerHost, event: &mut LeftClickBlockEvent) -> Flow {
    let Some(id) = states::id(event.state) else {
        return Flow::Continue;
    };
    let Some((block, _)) = block_of(id) else {
        return Flow::Continue;
    };
    if !matches!(block, Block::Placard | Block::Clipboard) {
        return Flow::Continue;
    }
    let Some(player) = host.players().acting_player() else {
        return Flow::Continue;
    };
    let Some(dimension) = player_dimension(&*host, player) else {
        return Flow::Continue;
    };
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return Flow::Continue;
    };
    let pos = event.pos;
    let dim = lv.dim;
    if block == Block::Clipboard {
        collect_clipboard(&mut lv, player, pos);
        return Flow::Cancel;
    }
    let Some(stack) = with(dim, |d| d.placards.remove(&pos)) else {
        return Flow::Continue;
    };
    with(dim, |d| d.shown.push(pos));
    persist::mark_pos_unsaved(dim, pos);
    lv.block_sound("minecraft:entity.item_frame.remove_item", pos, 1.0, 1.0);
    // `player.getInventory().placeItemBackInInventory(heldItem)`.
    let _ = player.give_stack(&mut *lv.host, stack);
    Flow::Continue
}

/// Table cloths: `TableClothBlock.useItemOn`.
struct TableCloth;

#[mod_api::horizon_part]
impl mod_api::BlockPart for TableCloth {
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        if event.click.stack.is_none() {
            return Some(InteractionResult::TryWithEmptyHand);
        }
        use_cloth(host, event.click, event.state)
    }

    fn use_without_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseWithoutItemEvent<'_>,
    ) -> Option<InteractionResult> {
        use_cloth(host, event.click, event.state)
    }
}

/// Not from below, by a player who may build: a cloth item (not sneaking) goes on the nearest
/// free side of the clicked one (`PlacementHelper`); anything else is the cloth's own use
/// (`stock::shop`: its items and its shop).
fn use_cloth(
    host: &mut dyn ServerHost,
    click: &BlockClick<'_>,
    state: mod_api::BlockState,
) -> Option<InteractionResult> {
    // `TableClothBlockEntity.use` reads the main hand: the off hand's click (a client that
    // didn't take the main hand's asks for both) leaves the cloth alone.
    if click.hit.face == [0, -1, 0] || click.hand != Hand::Main {
        return None;
    }
    let mut dimension = String::new();
    let (player, mut lv) = clicked(host, click, &mut dimension)?;
    if player.may_build(&*lv.host) != Some(true) {
        return None;
    }
    let has_entity = super::prop(&*lv.host, state, "entity").as_deref() == Some("true");
    let Some(stack) = click.stack.filter(|s| {
        let id = s.item().id(&*lv.host);
        id.starts_with("create:") && id.ends_with("_table_cloth")
    }) else {
        return crate::glue::stock::shop::use_cloth(&mut lv, player, click, has_entity);
    };
    if sneaking(&*lv.host, player) {
        return None;
    }
    let item = stack.item();
    let Some(at) = cloth_target(&lv, click.pos, click.hit.point) else {
        return Some(InteractionResult::Success);
    };
    let Some(state) = lv
        .host
        .item_block(item)
        .and_then(|b| lv.host.block_state_default(b))
    else {
        return Some(InteractionResult::Success);
    };
    lv.put(at, state);
    lv.block_sound("minecraft:block.wool.place", at, 1.0, 0.8);
    use_one(&mut *lv.host, player, click.hand, stack);
    Some(InteractionResult::Success)
}

/// `TableClothBlock.PlacementHelper.getOffset`: the free side of the cloth at `pos` nearest the
/// hit `point`.
fn cloth_target(lv: &ServerLevel<'_>, pos: [i32; 3], point: [f64; 3]) -> Option<[i32; 3]> {
    let hit = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]) - 0.5);
    let facts = lv.host.block_states();
    decor::table_cloth_directions(hit)
        .into_iter()
        .map(|d| add(pos, d))
        .find(|p| {
            lv.block_state(*p)
                .is_some_and(|s| facts.replaceable(s) == Some(true))
        })
}

/// `SailBlock.PlacementHelper.getOffset`: the free side nearest the hit `point` of the sail at
/// `pos` facing `facing`, in its plane.
fn sail_target(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    point: [f64; 3],
    facing: Dir,
) -> Option<[i32; 3]> {
    let hit = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]) - 0.5);
    let facts = lv.host.block_states();
    create_engine::place::ordered_by_distance(hit, |d| d.axis() != facing.axis())
        .into_iter()
        .map(|d| add(pos, d))
        .find(|p| {
            lv.block_state(*p)
                .is_some_and(|s| facts.replaceable(s) == Some(true))
        })
}

/// The cloth, sail and girder helpers' targets for the indicator (`PlacementClient`).
pub(crate) fn helper_target(
    lv: &ServerLevel<'_>,
    player: PlayerRef,
    pos: [i32; 3],
    point: [f64; 3],
    item: &str,
) -> Option<crate::glue::interaction::HelperTarget> {
    let (block, info) = lv
        .block_state(pos)
        .and_then(states::id)
        .and_then(block_of)?;
    if item.starts_with("create:")
        && item.ends_with("_table_cloth")
        && super::cloth::is_cloth(block)
    {
        // `s -> s`: the cloth's own state.
        return cloth_target(lv, pos, point).map(|at| (at, Vec::new()));
    }
    if is_sail(block) && (item == "create:white_sail" || item == "create:sail_frame") {
        let facing = info.facing?;
        return sail_target(lv, pos, point, facing).map(|at| {
            (
                at,
                vec![(
                    "facing",
                    create_engine::copycat::dir_name(facing).to_owned(),
                )],
            )
        });
    }
    girders::helper_target(lv, player, pos, point, item)
}

/// Sails and sail frames: `SailBlock.useItemOn`.
struct Sail;

#[mod_api::horizon_part]
impl mod_api::BlockPart for Sail {
    /// Not sneaking, a sail or sail frame goes on the nearest free side of the clicked one in
    /// its plane, facing its way (`SailBlock.PlacementHelper`); shears make sails frames and a
    /// dye gives cloth sails its colour (`applyDye`).
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        let click = event.click;
        let stack = click.stack?;
        let item = stack.item();
        let id = item.id(&*host);
        let mut dimension = String::new();
        let (player, mut lv) = clicked(host, click, &mut dimension)?;
        let pos = click.pos;
        let (block, info) = lv
            .block_state(pos)
            .and_then(states::id)
            .and_then(block_of)?;
        let facing = info.facing?;
        let hit = local(click).map(|v| v - 0.5);
        let helped = id == "create:white_sail" || id == "create:sail_frame";
        if helped && !sneaking(&*lv.host, player) && player.may_build(&*lv.host) == Some(true) {
            let free = sail_target(&lv, pos, click.hit.point, facing);
            let facing_name = super::super::kinetics::dir_name(facing);
            let state = lv
                .host
                .item_block(item)
                .and_then(|b| lv.host.block_state_default(b))
                .and_then(|s| with_props(&*lv.host, s, &[("facing", facing_name)]));
            if let (Some(at), Some(state)) = (free, state) {
                lv.put(at, state);
                lv.place_sound(at);
                use_one(&mut *lv.host, player, click.hand, stack);
            }
            return Some(InteractionResult::Success);
        }
        let color = match id.as_str() {
            "minecraft:shears" => None,
            _ if block == Block::SailFrame => return None,
            other => Some(
                other
                    .strip_prefix("minecraft:")?
                    .strip_suffix("_dye")?
                    .to_owned(),
            ),
        };
        let pitch = match color {
            None => 1.0,
            Some(_) => {
                use mod_api::RandomSource;
                let r = lv
                    .level
                    .with_random(&mut *lv.host, |r| r.next_float())
                    .unwrap_or(0.5);
                1.1 - r * 0.2
            }
        };
        let sound = match color {
            None => "minecraft:entity.sheep.shear",
            Some(_) => "minecraft:item.dye.use",
        };
        lv.block_sound(sound, pos, 1.0, pitch);
        let name = match &color {
            None => "create:sail_frame".to_owned(),
            Some(c) => format!("create:{c}_sail"),
        };
        let facing_name = super::super::kinetics::dir_name(facing);
        let Some(new_state) = lv
            .host
            .block_state_id(&format!("{name}[facing={facing_name}]"))
        else {
            return Some(InteractionResult::Success);
        };
        let state = lv.block_state(pos)?;
        let targets = {
            let lv = &lv;
            create_engine::decor::sail_dye_targets(
                pos,
                facing.axis(),
                hit,
                state != new_state,
                color.is_some(),
                |p| {
                    let s = lv.block_state(p)?;
                    let (b, i) = states::id(s).and_then(block_of)?;
                    is_sail(b).then_some(create_engine::decor::SailSide {
                        frame: b == Block::SailFrame,
                        same_facing: i.facing == Some(facing),
                        same_state: s == state,
                    })
                },
            )
        };
        for at in targets {
            lv.put(at, new_state);
        }
        Some(InteractionResult::Success)
    }
}
