//! Stacks as Create's logistics move them: the engine names an item by its id and a count, the
//! game holds `ItemStack`s in block entity slots and in other mods' and vanilla's containers,
//! reached through the `ITEMS` capability in transactions.

use super::super::contraptions::actors;
use super::super::server::ServerLevel;
use create_engine::kinetics::Dir;
use mod_api::{
    BlockEntityRef, Capability, Direction, EntityRef, Host, HostTransaction, ITEMS, Identifier,
    ItemResource, ItemStack, ResourceHandler, ServerHost,
};

/// An `ITEMS` handler.
pub type Items = Capability<dyn ResourceHandler<ItemResource>>;

/// The game's face `d`.
pub fn face(d: Dir) -> Direction {
    Direction::ALL[d as usize]
}

/// The id of `stack`'s item (`minecraft:stone`).
pub fn id_of(host: &dyn Host, stack: &ItemStack) -> String {
    stack.item().id(host)
}

/// `count` of item `id`, without components; None for an unknown item or a zero count.
pub fn stack_of(host: &dyn Host, id: &str, count: u32) -> Option<ItemStack> {
    let key = host.item_key(&Identifier::parse(id).ok()?)?;
    ItemStack::new(key, count)
}

/// Whether item `id` is in item tag `tag`.
fn in_tag(host: &dyn Host, id: &str, tag: &str) -> bool {
    let (Ok(item), Ok(tag)) = (Identifier::parse(id), Identifier::parse(tag)) else {
        return false;
    };
    let Some(item) = host.item_key(&item) else {
        return false;
    };
    let tags = host.tags();
    tags.key(&tag)
        .and_then(|t| tags.is_in_tag(item, t))
        .unwrap_or(false)
}

/// `BeltHelper.isItemUpright`: items of `create:upright_on_belt` stand, unless also in
/// `create:not_upright_on_belt`. (Fluid-holding items stand in Create too: not yet.)
pub fn upright(host: &dyn Host, id: &str) -> bool {
    in_tag(host, id, "create:upright_on_belt") && !in_tag(host, id, "create:not_upright_on_belt")
}

/// A stack's limit (at most 64).
pub fn stack_limit(host: &dyn Host, id: &str) -> u8 {
    Identifier::parse(id)
        .ok()
        .and_then(|i| host.item_key(&i))
        .and_then(|k| host.max_stack_size(k))
        .unwrap_or(64)
        .clamp(1, 64) as u8
}

/// Whether `id` draws as a block (`BakedModel.isGui3d`). The mod API does not say; a block's
/// item stands in for it.
pub fn is_block_item(host: &dyn Host, id: &str) -> bool {
    Identifier::parse(id)
        .ok()
        .and_then(|i| host.item_key(&i))
        .is_some_and(|k| host.item_block(k).is_some())
}

/// Whether a stack carries components (or damage), which the engine's belts and arms do not keep.
pub fn has_components(stack: &ItemStack) -> bool {
    stack.components().is_some() || stack.is_damaged()
}

/// `TransportedItemStack`'s `R.nextInt(360)`: Create's shared random; any angle will do.
pub fn random_angle(lv: &ServerLevel<'_>, pos: [i32; 3]) -> i32 {
    let mut x = (lv.game_time() as u64)
        ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (pos[2] as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (pos[1] as u64) << 40;
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x % 360) as i32
}

/// The block entity at `pos`.
pub fn entity(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<BlockEntityRef> {
    lv.host.block_entity(lv.level, pos)
}

/// Slot `slot` of the block entity at `pos`.
pub fn slot(lv: &ServerLevel<'_>, pos: [i32; 3], slot: u32) -> Option<ItemStack> {
    entity(lv, pos)?.item_slot(&*lv.host, slot).ok().flatten()
}

/// The first `n` slots of the block entity at `pos`; None without one.
pub fn slots(lv: &ServerLevel<'_>, pos: [i32; 3], n: u32) -> Option<Vec<Option<ItemStack>>> {
    let e = entity(lv, pos)?;
    Some(
        (0..n)
            .map(|i| e.item_slot(&*lv.host, i).ok().flatten())
            .collect(),
    )
}

/// Sets slots of the block entity at `pos`, in one transaction. False without one.
pub fn set_slots(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    set: impl IntoIterator<Item = (u32, Option<ItemStack>)>,
) -> bool {
    let Some(e) = entity(lv, pos) else {
        return false;
    };
    lv.host
        .transaction(|host, tx| {
            for (i, s) in set {
                e.set_item_slot(host, tx, i, s).map_err(|_| ())?;
            }
            Ok::<(), ()>(())
        })
        .is_ok()
}

/// Sets one slot.
pub fn set_slot(lv: &mut ServerLevel<'_>, pos: [i32; 3], i: u32, stack: Option<ItemStack>) -> bool {
    set_slots(lv, pos, [(i, stack)])
}

/// `count` of `stack` (its components kept).
pub fn with_count(stack: &ItemStack, count: u32) -> Option<ItemStack> {
    let mut s = stack.clone();
    s.set_count(count).ok()?;
    Some(s)
}

/// The `ITEMS` handler of the block at `pos` reached through `side`.
pub fn items_at(lv: &mut ServerLevel<'_>, pos: [i32; 3], side: Option<Direction>) -> Option<Items> {
    lv.level.capability(&mut *lv.host, &ITEMS, pos, side)
}

/// `ItemHandlerHelper.insertItemStacked` into `to`, in `tx`: onto the same items first, then
/// into empty indices. How much went in.
fn insert_stacked(
    host: &mut dyn ServerHost,
    tx: &mod_api::Transaction,
    to: &Items,
    resource: &ItemResource,
    count: u32,
    limit: u32,
) -> u32 {
    let size = to.size(host);
    let view: Vec<Option<(bool, u32)>> = (0..size)
        .map(|i| {
            to.resource(host, i)
                .map(|r| (r == *resource, to.amount(host, i)))
        })
        .collect();
    let mut went = 0;
    for (i, n) in create_engine::vault::insert_stacked(&view, count, limit) {
        went += to.insert(host, tx, i, resource, n);
    }
    // An index the plan counted on refused: the rest goes wherever it goes.
    if went < count {
        went += to.insert_any(host, tx, resource, count - went);
    }
    went.min(count)
}

/// Puts `stack` into the container at `pos` through its side `side`
/// (`ItemHandlerHelper.insertItemStacked`, as Create's chutes and funnels insert). What did not
/// fit; all of it when nothing there takes items. A portable storage interface puts it into its
/// contraption's storage while connected (`InterfaceItemHandler`, contraptions' `psi_insert`:
/// that storage is no journaled container).
pub fn insert_item(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    side: Direction,
    stack: ItemStack,
) -> Option<ItemStack> {
    let stack = match actors::psi_insert(lv, pos, stack) {
        Ok(left) => return left,
        Err(stack) => stack,
    };
    let Some(to) = items_at(lv, pos, Some(side)) else {
        return Some(stack);
    };
    let resource = ItemResource::of(&stack);
    let count = stack.count();
    let limit = u32::from(stack_limit(&*lv.host, &id_of(&*lv.host, &stack)));
    let went = lv
        .host
        .transaction(|host, tx| {
            Ok::<u32, ()>(insert_stacked(host, tx, &to, &resource, count, limit))
        })
        .unwrap_or(0);
    with_count(&stack, count - went.min(count))
}

/// How much of `stack` `insert_item` would put into the container at `pos` (nothing changes);
/// None when there is no container.
pub fn insert_room(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    side: Direction,
    stack: &ItemStack,
) -> Option<u32> {
    if let Some(room) = actors::psi_room(lv, pos, stack) {
        return Some(room);
    }
    let to = items_at(lv, pos, Some(side))?;
    let resource = ItemResource::of(stack);
    let limit = u32::from(stack_limit(&*lv.host, &id_of(&*lv.host, stack)));
    let count = stack.count();
    // Tried, then aborted: the answer comes out in the error.
    let room = lv.host.transaction(|host, tx| {
        Err::<(), u32>(insert_stacked(host, tx, &to, &resource, count, limit))
    });
    Some(room.err().unwrap_or(0))
}

/// Up to `amount` of the first item `test` passes from `from` (`ItemHelper.extract`, up to):
/// gathered from its indices in order, in its own transaction.
pub fn extract_matching(
    lv: &mut ServerLevel<'_>,
    from: &Items,
    test: impl Fn(&mut dyn ServerHost, &ItemResource) -> bool,
    amount: u32,
) -> Option<ItemStack> {
    let host = &mut *lv.host;
    let size = from.size(host);
    let mut keys: Vec<Option<(ItemResource, u32, u32)>> = Vec::with_capacity(size);
    for i in 0..size {
        let key = from.resource(host, i).and_then(|r| {
            if !test(host, &r) {
                return None;
            }
            let n = from.amount(host, i);
            let id = r.item().id(&*host);
            let max = u32::from(stack_limit(&*host, &id));
            (n > 0).then_some((r, n, max))
        });
        keys.push(key);
    }
    let plan = create_engine::chute::extract(&keys, |_| true, false, amount);
    let &(first, _) = plan.first()?;
    let resource = keys[first].as_ref()?.0.clone();
    let taken = host
        .transaction(|host, tx| {
            let mut n = 0;
            for &(i, want) in &plan {
                n += from.extract(host, tx, i, &resource, want);
            }
            if n == 0 { Err(()) } else { Ok(n) }
        })
        .ok()?;
    resource.to_stack(taken)
}

/// Up to `max` of the first item `test` passes the container at `pos` gives through `side`;
/// None when there is no container or nothing to take. A connected portable storage interface
/// gives from its contraption's storage (contraptions' `psi_extract`).
pub fn take_from(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    side: Direction,
    test: impl Fn(&ItemStack) -> bool,
    max: u32,
) -> Option<ItemStack> {
    if let Ok(taken) = actors::psi_extract(lv, pos, &test, max) {
        return taken;
    }
    let from = items_at(lv, pos, Some(side))?;
    extract_matching(lv, &from, |_, r| test(r.as_stack()), max)
}

/// An item entity of `stack` at `at`, moving `motion` blocks a tick, picked up after
/// `pickup_delay` ticks (`new ItemEntity(..)`; Create's drops wait 10).
pub fn drop_stack(
    lv: &mut ServerLevel<'_>,
    at: [f64; 3],
    motion: [f64; 3],
    stack: ItemStack,
    pickup_delay: u32,
) {
    let _ = lv.level.spawn_item_with_motion(
        &mut *lv.host,
        at[0],
        at[1],
        at[2],
        stack,
        motion,
        pickup_delay,
    );
}

/// The same for `count` of item `id`.
pub fn drop_item(
    lv: &mut ServerLevel<'_>,
    at: [f64; 3],
    motion: [f64; 3],
    id: &str,
    count: u32,
    pickup_delay: u32,
) {
    if let Some(stack) = stack_of(&*lv.host, id, count) {
        drop_stack(lv, at, motion, stack, pickup_delay);
    }
}

/// The item entities in the box `min`..`max`.
pub fn items_in(lv: &ServerLevel<'_>, min: [f64; 3], max: [f64; 3]) -> Vec<EntityRef> {
    lv.level
        .entities_in(&*lv.host, min, max)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.kind(&*lv.host).as_deref() == Some("minecraft:item"))
        .collect()
}

/// Gives `stack` to player `player` (`Inventory.placeItemBackInInventory`: what does not fit
/// drops at them).
pub fn give(host: &mut dyn ServerHost, player: u128, stack: ItemStack) {
    if let Some(p) = host.player(player) {
        let _ = p.give_stack(host, stack);
    }
}
