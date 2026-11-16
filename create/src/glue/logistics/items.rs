//! Stacks as Create's logistics move them: the engine names an item by its id and a count, the
//! game holds `ItemStack`s in block entity slots and in other mods' and vanilla's containers,
//! reached through the `ITEMS` capability in transactions.

use super::super::contraptions::actors;
use super::super::server::ServerLevel;
use create_engine::kinetics::Dir;
use create_engine::potion::Contents;
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

/// `stack` as the engine's belts, arms and ejectors name it (`create_engine::arm::item_id`):
/// its item id; a potion with its whole contents (`tank::potion_item`, as spouts and drains
/// name it); and any other stack with components (a package's address and contents, a name, an
/// enchantment, a potion's other components) followed by `#` and the hex of its item NBT, so
/// that moving it keeps them all.
pub fn key_of(host: &dyn Host, stack: &ItemStack) -> String {
    let id = id_of(host, stack);
    if !has_components(stack) {
        return id;
    }
    let potion = potion_name(host, stack);
    if potion.is_some() && only_potion_contents(stack) {
        return potion.unwrap_or(id);
    }
    let Some(mut nbt) = with_count(stack, 1).and_then(|s| s.to_nbt(host)) else {
        return id;
    };
    nbt.remove(&"count".into());
    let Ok(bytes) = mod_api::Tag::Compound(nbt).to_network_bytes() else {
        return id;
    };
    let mut key = potion.unwrap_or(id);
    key.push('#');
    for b in bytes {
        key.push_str(&format!("{b:02x}"));
    }
    key
}

/// `count` of item `id` (or of a stack named by `key_of`, its components kept; or a potion
/// named with its contents, `tank::potion_item`, as a spout fills one on a belt); None for an
/// unknown item or a zero count.
pub fn stack_of(host: &dyn Host, id: &str, count: u32) -> Option<ItemStack> {
    if let Some((_, hex)) = id.split_once('#') {
        let bytes: Option<Vec<u8>> = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok())
            .collect();
        let (tag, _) = mod_api::Tag::from_network_bytes(&bytes?, None).ok()?;
        let mod_api::Tag::Compound(nbt) = tag else {
            return None;
        };
        let stack = ItemStack::from_nbt(host, &nbt)?;
        return with_count(&stack, count);
    }
    let (id, contents) = match create_engine::tank::split_potion_item(id) {
        Some((id, c)) => (id, Some(Contents::decode(c)?)),
        None => (id, None),
    };
    let key = host.item_key(&Identifier::parse(id).ok()?)?;
    let mut stack = ItemStack::new(key, count)?;
    // `PotionFluidHandler.fillBottle`: the bottle holds the fluid's contents (none: empty).
    if let Some(tag) = contents.as_ref().and_then(contents_tag) {
        let patch = mod_api::Compound::from([(POTION_CONTENTS.into(), tag)]);
        let _ = stack.set_components(patch);
    }
    Some(stack)
}

/// The item component a potion's contents are in.
pub const POTION_CONTENTS: &str = "minecraft:potion_contents";

/// A potion bottle as spouts, drains and tanks name it (`tank::potion_item`): its item and its
/// whole `potion_contents` (`PotionFluidHandler.getFluidFromPotionItem` reads nothing else);
/// None for any other item.
pub fn potion_name(host: &dyn Host, stack: &ItemStack) -> Option<String> {
    let id = id_of(host, stack);
    create_engine::tank::Bottle::of_item(&id)?;
    let contents = stack
        .components()
        .and_then(|c| c.as_nbt().get(&POTION_CONTENTS.into()).map(contents_of_tag))
        .unwrap_or_default();
    Some(create_engine::tank::potion_item(&id, &contents.encode()))
}

/// Whether a stack's only component is its `potion_contents` (undamaged).
fn only_potion_contents(stack: &ItemStack) -> bool {
    !stack.is_damaged()
        && stack.components().is_some_and(|c| {
            let nbt = c.as_nbt();
            nbt.len() == 1 && nbt.get(&POTION_CONTENTS.into()).is_some()
        })
}

/// `PotionContents` as its codec reads `tag`: a lone potion's id, or a compound of `potion`,
/// `custom_color`, `custom_name` and `custom_effects` (kept whole; their ids, amplifiers and
/// `show_particles` read for the colour).
pub fn contents_of_tag(tag: &mod_api::Tag) -> Contents {
    use mod_api::Tag;
    let text = |t: Option<&Tag>| match t {
        Some(Tag::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    };
    let int = |t: Option<&Tag>| match t {
        Some(Tag::Byte(v)) => Some(i32::from(*v)),
        Some(Tag::Short(v)) => Some(i32::from(*v)),
        Some(Tag::Int(v)) => Some(*v),
        Some(Tag::Long(v)) => Some(*v as i32),
        _ => None,
    };
    let c = match tag {
        Tag::String(s) => return Contents::of(&s.to_string_lossy()),
        Tag::Compound(c) => c,
        _ => return Contents::default(),
    };
    let mut out = Contents {
        potion: text(c.get(&"potion".into())),
        custom_color: int(c.get(&"custom_color".into())),
        custom_name: text(c.get(&"custom_name".into())),
        ..Contents::default()
    };
    if let Some(list @ Tag::List(effects)) = c.get(&"custom_effects".into())
        && !effects.is_empty()
    {
        out.custom_effects = list.to_network_bytes().unwrap_or_default();
        out.effects = effects
            .iter()
            .filter_map(|e| {
                let Tag::Compound(e) = e else { return None };
                Some(create_engine::potion::Effect {
                    id: text(e.get(&"id".into()))?,
                    amplifier: int(e.get(&"amplifier".into())).unwrap_or(0),
                    visible: int(e.get(&"show_particles".into())).unwrap_or(1) != 0,
                })
            })
            .collect();
    }
    out
}

/// The `potion_contents` tag of `c`; None for `PotionContents.EMPTY` (the component left out,
/// `PotionFluid.addPotionToFluidStack`).
pub fn contents_tag(c: &Contents) -> Option<mod_api::Tag> {
    use mod_api::Tag;
    if c.is_empty() {
        return None;
    }
    let mut out = mod_api::Compound::new();
    if let Some(p) = &c.potion {
        out.insert("potion".into(), Tag::String(p.as_str().into()));
    }
    if let Some(color) = c.custom_color {
        out.insert("custom_color".into(), Tag::Int(color));
    }
    if !c.custom_effects.is_empty()
        && let Ok((effects, _)) = Tag::from_network_bytes(&c.custom_effects, None)
    {
        out.insert("custom_effects".into(), effects);
    }
    if let Some(n) = &c.custom_name {
        out.insert("custom_name".into(), Tag::String(n.as_str().into()));
    }
    Some(Tag::Compound(out))
}

/// The item id of a stack's name (`key_of`), or of a potion named with its contents
/// (`tank::potion_item`).
fn base(id: &str) -> &str {
    let id = create_engine::arm::item_id(id);
    create_engine::tank::split_potion_item(id).map_or(id, |(item, _)| item)
}

/// Whether item `id` is in item tag `tag`.
fn in_tag(host: &dyn Host, id: &str, tag: &str) -> bool {
    let (Ok(item), Ok(tag)) = (Identifier::parse(base(id)), Identifier::parse(tag)) else {
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

/// `BeltHelper.isItemUpright`: items of `create:upright_on_belt` and items holding fluid
/// (`FluidHandler.ITEM`) stand, unless also in `create:not_upright_on_belt`.
pub fn upright(host: &dyn Host, id: &str) -> bool {
    (holds_fluid(id) || in_tag(host, id, "create:upright_on_belt"))
        && !in_tag(host, id, "create:not_upright_on_belt")
}

/// Whether an item carries a fluid of its own (`Capabilities.FluidHandler.ITEM` in Create): the
/// empty bucket and the bucket of each fluid the port's tanks know (`tank::Fluid::of_bucket`),
/// as the item filter's `fluid_container` reads it. Bottles and potions are Create's
/// `PotionFluidHandler`'s, not an item's own fluid; they stand by `create:upright_on_belt`.
fn holds_fluid(id: &str) -> bool {
    let id = base(id);
    id == "minecraft:bucket" || create_engine::tank::Fluid::of_bucket(id).is_some()
}

/// A stack's limit (at most 64).
pub fn stack_limit(host: &dyn Host, id: &str) -> u8 {
    Identifier::parse(base(id))
        .ok()
        .and_then(|i| host.item_key(&i))
        .and_then(|k| host.max_stack_size(k))
        .unwrap_or(64)
        .clamp(1, 64) as u8
}

/// Whether `id` draws as a block (`BakedModel.isGui3d`). The mod API does not say; a block's
/// item stands in for it.
pub fn is_block_item(host: &dyn Host, id: &str) -> bool {
    Identifier::parse(base(id))
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

/// Up to `amount` of the first item `test` passes from `from` (`ItemHelper.extract`), or with
/// `exact` exactly `amount` of one item or nothing (`ExtractionCountMode.EXACTLY`): gathered
/// from its indices in order, in its own transaction.
pub fn extract_matching(
    lv: &mut ServerLevel<'_>,
    from: &Items,
    test: impl Fn(&mut dyn ServerHost, &ItemResource) -> bool,
    amount: u32,
    exact: bool,
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
    let plan = create_engine::chute::extract(&keys, |_| true, exact, amount);
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

/// Up to `max` of the first item `test` passes the container at `pos` gives through `side`
/// (exactly `max` or nothing with `exact`); None when there is no container or nothing to
/// take. A connected portable storage interface gives from its contraption's storage
/// (contraptions' `psi_extract`, up to `max`).
pub fn take_from(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    side: Direction,
    test: impl Fn(&ItemStack) -> bool,
    max: u32,
    exact: bool,
) -> Option<ItemStack> {
    if let Ok(taken) = actors::psi_extract(lv, pos, &test, max) {
        return taken;
    }
    let from = items_at(lv, pos, Some(side))?;
    extract_matching(lv, &from, |_, r| test(r.as_stack()), max, exact)
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
        .filter(|e| e.kind(&*lv.host).map(|k| k.id(&*lv.host)).as_deref() == Some("minecraft:item"))
        .collect()
}

/// Gives `stack` to player `player` (`Inventory.placeItemBackInInventory`: what does not fit
/// drops at them).
pub fn give(host: &mut dyn ServerHost, player: u128, stack: ItemStack) {
    if let Some(p) = host.player(player) {
        let _ = p.give_stack(host, stack);
    }
}
