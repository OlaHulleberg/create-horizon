//! What Create's packages read and write of a stack: a package's address and nine stacks of
//! contents (`PACKAGE_ADDRESS`, `PACKAGE_CONTENTS`) and its order data
//! (`PACKAGE_ORDER_DATA`), kept in `minecraft:custom_data`; a stack
//! as NBT for the chunk save (`ItemStack.CODEC`, the API's NBT escape); and the item ids the
//! engine's rules name items by.

use create_engine::package::{ADDRESS_KEY, CONTENTS_KEY, SLOTS};
use create_engine::stock::{Order, OrderData};
use mod_api::{Component, Compound, Host, Identifier, ItemKey, ItemStack, Tag};

/// `minecraft:custom_data` as a whole compound.
pub struct CustomData;

impl Component for CustomData {
    type Value = Compound;
    fn id(&self) -> &str {
        "minecraft:custom_data"
    }
    fn decode(&self, value: &Tag) -> Option<Compound> {
        match value {
            Tag::Compound(c) => Some(c.clone()),
            _ => None,
        }
    }
    fn encode(&self, value: Compound) -> Tag {
        Tag::Compound(value)
    }
}

fn custom(stack: &ItemStack) -> Option<Compound> {
    stack.get(&CustomData)
}

/// Sets (or, with `None`, clears) one key of the stack's custom data; the component goes when
/// it is left empty.
pub fn set_custom(stack: &mut ItemStack, key: &str, value: Option<Tag>) {
    let mut c = custom(stack).unwrap_or_default();
    match value {
        Some(v) => {
            c.insert(key.into(), v);
        }
        None => {
            c.remove(&key.into());
        }
    }
    if c.is_empty() {
        stack.remove(&CustomData);
    } else {
        // A compound is always a valid `custom_data`.
        let _ = stack.set(&CustomData, c);
    }
}

/// The item's id (`minecraft:stone`).
pub fn item_id(host: &dyn Host, stack: &ItemStack) -> String {
    stack.item().id(host)
}

/// The item `id` names, when the game has it.
pub fn item_key(host: &dyn Host, id: &str) -> Option<ItemKey> {
    host.item_key(&Identifier::parse(id).ok()?)
}

/// `count` of the item `id` names, without components.
pub fn new_stack(host: &dyn Host, id: &str, count: u32) -> Option<ItemStack> {
    ItemStack::new(item_key(host, id)?, count)
}

/// `getMaxStackSize` (64 when the game cannot say).
pub fn stack_limit(host: &dyn Host, stack: &ItemStack) -> u32 {
    host.max_stack_size(stack.item()).unwrap_or(64)
}

/// `stack` with `count` items (`copyWithCount`); none for 0.
pub fn with_count(stack: &ItemStack, count: u32) -> Option<ItemStack> {
    let mut s = stack.clone();
    s.set_count(count).ok()?;
    Some(s)
}

/// `PackageItem.getAddress`: "" when none.
pub fn address(stack: &ItemStack) -> String {
    match custom(stack)
        .as_ref()
        .and_then(|c| c.get(&ADDRESS_KEY.into()))
    {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    }
}

/// `clearAddress`, then `addAddress` when one is given.
pub fn set_address(stack: &mut ItemStack, address: Option<&str>) {
    set_custom(stack, ADDRESS_KEY, address.map(|a| Tag::String(a.into())));
}

/// `getContents`: the nine slots, each `{Slot, id, count, components}` as the game's item
/// container lists are kept.
pub fn contents(host: &dyn Host, stack: &ItemStack) -> Vec<Option<ItemStack>> {
    let mut slots = vec![None; SLOTS];
    let c = custom(stack);
    let Some(Tag::List(items)) = c.as_ref().and_then(|c| c.get(&CONTENTS_KEY.into())) else {
        return slots;
    };
    for t in items {
        let Tag::Compound(e) = t else { continue };
        let slot = match e.get(&"Slot".into()) {
            Some(Tag::Byte(v)) => *v as u8 as usize,
            Some(Tag::Int(v)) => *v as u8 as usize,
            _ => 0,
        };
        if let Some(cell) = slots.get_mut(slot) {
            *cell = ItemStack::from_nbt(host, e);
        }
    }
    slots
}

/// `PACKAGE_CONTENTS` set (`ItemHelper.containerContentsFromHandler`).
pub fn set_contents(host: &dyn Host, stack: &mut ItemStack, slots: &[Option<ItemStack>]) {
    let list = slots
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let mut c = s.as_ref()?.to_nbt(host)?;
            c.insert("Slot".into(), Tag::Byte(i as i8));
            Some(Tag::Compound(c))
        })
        .collect();
    set_custom(stack, CONTENTS_KEY, Some(Tag::List(list)));
}

/// `ItemStack.CODEC`, as a tag (`End` when the game cannot name the item).
pub fn to_nbt(host: &dyn Host, stack: &ItemStack) -> Tag {
    stack.to_nbt(host).map_or(Tag::End, Tag::Compound)
}

pub fn from_nbt(host: &dyn Host, tag: &Tag) -> Option<ItemStack> {
    match tag {
        Tag::Compound(c) => ItemStack::from_nbt(host, c),
        _ => None,
    }
}

/// `PACKAGE_ORDER_DATA` (`PackageOrderData.CODEC`), kept in `minecraft:custom_data`.
const ORDER_KEY: &str = "create:package_order_data";

fn int(c: &Compound, key: &str) -> i32 {
    match c.get(&key.into()) {
        Some(Tag::Int(v)) => *v,
        Some(Tag::Byte(v)) => i32::from(*v),
        _ => 0,
    }
}

/// `List<BigItemStack>` as `{entries: [{item_stack, count}]}` (`PackageOrder.CODEC`).
fn stacks_nbt(host: &dyn Host, stacks: &[(ItemStack, u32)]) -> Tag {
    Tag::compound([(
        "entries",
        Tag::List(
            stacks
                .iter()
                .map(|(s, n)| {
                    let one = with_count(s, 1).unwrap_or_else(|| s.clone());
                    Tag::compound([
                        ("item_stack", to_nbt(host, &one)),
                        ("count", Tag::Int(*n as i32)),
                    ])
                })
                .collect(),
        ),
    )])
}

fn stacks_from(host: &dyn Host, t: Option<&Tag>) -> Vec<(ItemStack, u32)> {
    let Some(Tag::Compound(c)) = t else {
        return Vec::new();
    };
    let Some(Tag::List(l)) = c.get(&"entries".into()) else {
        return Vec::new();
    };
    l.iter()
        .filter_map(|e| {
            let Tag::Compound(e) = e else { return None };
            let s = from_nbt(host, e.get(&"item_stack".into())?)?;
            Some((s, int(e, "count").max(0) as u32))
        })
        .collect()
}

/// `PackageOrderWithCrafts.CODEC`: `{ordered_stacks, ordered_crafts: [{pattern, count}]}`.
pub fn order_nbt(host: &dyn Host, o: &Order<ItemStack>) -> Tag {
    Tag::compound([
        ("ordered_stacks", stacks_nbt(host, &o.stacks)),
        (
            "ordered_crafts",
            Tag::List(
                o.crafts
                    .iter()
                    .map(|(p, n)| {
                        Tag::compound([
                            ("pattern", stacks_nbt(host, p)),
                            ("count", Tag::Int(*n as i32)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

pub fn order_from_nbt(host: &dyn Host, t: &Tag) -> Option<Order<ItemStack>> {
    let Tag::Compound(c) = t else { return None };
    // The legacy form (6.0.0 - 6.0.2): the stacks' list itself.
    if c.contains_key(&"entries".into()) {
        return Some(Order::simple(stacks_from(host, Some(t))));
    }
    let crafts = match c.get(&"ordered_crafts".into()) {
        Some(Tag::List(l)) => l
            .iter()
            .filter_map(|e| {
                let Tag::Compound(e) = e else { return None };
                Some((
                    stacks_from(host, e.get(&"pattern".into())),
                    int(e, "count").max(0) as u32,
                ))
            })
            .collect(),
        _ => Vec::new(),
    };
    Some(Order {
        stacks: stacks_from(host, c.get(&"ordered_stacks".into())),
        crafts,
    })
}

/// `PackageItem.getOrderId` and the rest of its order data; None when it has none.
pub fn order(host: &dyn Host, stack: &ItemStack) -> Option<OrderData<ItemStack>> {
    let custom = custom(stack)?;
    let Some(Tag::Compound(c)) = custom.get(&ORDER_KEY.into()) else {
        return None;
    };
    Some(OrderData {
        order_id: int(c, "order_id"),
        link_index: int(c, "link_index"),
        final_link: int(c, "is_final_link") != 0,
        fragment_index: int(c, "fragment_index"),
        is_final: int(c, "is_final") != 0,
        context: c
            .get(&"order_context".into())
            .and_then(|t| order_from_nbt(host, t)),
    })
}

/// `PackageItem.setOrder`.
pub fn set_order(host: &dyn Host, stack: &mut ItemStack, d: &OrderData<ItemStack>) {
    let mut c = vec![
        ("order_id", Tag::Int(d.order_id)),
        ("link_index", Tag::Int(d.link_index)),
        ("is_final_link", Tag::Byte(i8::from(d.final_link))),
        ("fragment_index", Tag::Int(d.fragment_index)),
        ("is_final", Tag::Byte(i8::from(d.is_final))),
    ];
    if let Some(o) = &d.context {
        c.push(("order_context", order_nbt(host, o)));
    }
    set_custom(stack, ORDER_KEY, Some(Tag::compound(c)));
}
