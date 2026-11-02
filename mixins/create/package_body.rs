// What Create's packages read and write of a stack beyond the hook helpers: a package's address
// and nine stacks of contents (`PACKAGE_ADDRESS`, `PACKAGE_CONTENTS`), kept in
// `minecraft:custom_data` since Horizon adds no component types; stacks to and from NBT for the
// chunk save; a stack as SNBT for the `summon` of a dropped one. Compiled against Horizon by
// `horizon.py check`; the template test swaps it for `tools/fixture/package_body.rs`.
use create_engine::package::{ADDRESS_KEY, CONTENTS_KEY, SLOTS};
use create_engine::stock::{Order, OrderData};
use horizon_client::items::ItemStack;
use horizon_world::nbt::{Compound, Tag};

const CUSTOM_DATA: &str = "minecraft:custom_data";

fn custom(stack: &ItemStack) -> Option<&Compound> {
    match stack.data().and_then(|d| d.other(CUSTOM_DATA)) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    }
}

fn set_custom(stack: &mut ItemStack, key: &str, value: Option<Tag>) {
    let mut c = custom(stack).cloned().unwrap_or_default();
    match value {
        Some(v) => {
            c.insert(key.into(), v);
        }
        None => {
            c.remove(&horizon_world::nbt::JavaString::from(key));
        }
    }
    let data = stack.data_mut();
    data.set_other(CUSTOM_DATA, (!c.is_empty()).then_some(Tag::Compound(c)));
    stack.tidy();
}

/// `PackageItem.getAddress`: "" when none.
pub(super) fn address(stack: &ItemStack) -> String {
    match custom(stack).and_then(|c| c.get(&ADDRESS_KEY.into())) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    }
}

/// `clearAddress`, then `addAddress` when one is given.
pub(super) fn set_address(stack: &mut ItemStack, address: Option<&str>) {
    set_custom(stack, ADDRESS_KEY, address.map(|a| Tag::String(a.into())));
}

/// `getContents`: the nine slots.
pub(super) fn contents(stack: &ItemStack) -> Vec<Option<ItemStack>> {
    horizon_items::item_nbt::decode_items(
        custom(stack).and_then(|c| c.get(&CONTENTS_KEY.into())),
        SLOTS,
    )
}

/// `PACKAGE_CONTENTS` set (`ItemHelper.containerContentsFromHandler`).
pub(super) fn set_contents(stack: &mut ItemStack, slots: &[Option<ItemStack>]) {
    set_custom(
        stack,
        CONTENTS_KEY,
        Some(horizon_items::item_nbt::encode_items(slots)),
    );
}

/// `ItemStack.CODEC`.
pub(super) fn to_nbt(stack: &ItemStack) -> Tag {
    horizon_items::item_nbt::item_to_nbt(stack)
}

pub(super) fn from_nbt(tag: &Tag) -> Option<ItemStack> {
    horizon_items::item_nbt::item_from_nbt(tag)
}

/// The stack as SNBT, for `summon minecraft:item ... {Item:...}`.
pub(super) fn snbt(stack: &ItemStack) -> String {
    horizon_world::snbt_text::to_snbt(&to_nbt(stack))
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
fn stacks_nbt(stacks: &[(ItemStack, u32)]) -> Tag {
    Tag::compound([(
        "entries",
        Tag::List(
            stacks
                .iter()
                .map(|(s, n)| {
                    Tag::compound([
                        (
                            "item_stack",
                            to_nbt(&ItemStack {
                                count: 1,
                                ..s.clone()
                            }),
                        ),
                        ("count", Tag::Int(*n as i32)),
                    ])
                })
                .collect(),
        ),
    )])
}

fn stacks_from(t: Option<&Tag>) -> Vec<(ItemStack, u32)> {
    let Some(Tag::Compound(c)) = t else {
        return Vec::new();
    };
    let Some(Tag::List(l)) = c.get(&"entries".into()) else {
        return Vec::new();
    };
    l.iter()
        .filter_map(|e| {
            let Tag::Compound(e) = e else { return None };
            let s = from_nbt(e.get(&"item_stack".into())?)?;
            Some((s, int(e, "count").max(0) as u32))
        })
        .collect()
}

/// `PackageOrderWithCrafts.CODEC`: `{ordered_stacks, ordered_crafts: [{pattern, count}]}`.
pub(super) fn order_nbt(o: &Order<ItemStack>) -> Tag {
    Tag::compound([
        ("ordered_stacks", stacks_nbt(&o.stacks)),
        (
            "ordered_crafts",
            Tag::List(
                o.crafts
                    .iter()
                    .map(|(p, n)| {
                        Tag::compound([("pattern", stacks_nbt(p)), ("count", Tag::Int(*n as i32))])
                    })
                    .collect(),
            ),
        ),
    ])
}

pub(super) fn order_from_nbt(t: &Tag) -> Option<Order<ItemStack>> {
    let Tag::Compound(c) = t else { return None };
    // The legacy form (6.0.0 - 6.0.2): the stacks' list itself.
    if c.contains_key(&"entries".into()) {
        return Some(Order::simple(stacks_from(Some(t))));
    }
    let crafts = match c.get(&"ordered_crafts".into()) {
        Some(Tag::List(l)) => l
            .iter()
            .filter_map(|e| {
                let Tag::Compound(e) = e else { return None };
                Some((
                    stacks_from(e.get(&"pattern".into())),
                    int(e, "count").max(0) as u32,
                ))
            })
            .collect(),
        _ => Vec::new(),
    };
    Some(Order {
        stacks: stacks_from(c.get(&"ordered_stacks".into())),
        crafts,
    })
}

/// `PackageItem.getOrderId` and the rest of its order data; None when it has none.
pub(super) fn order(stack: &ItemStack) -> Option<OrderData<ItemStack>> {
    let Some(Tag::Compound(c)) = custom(stack)?.get(&ORDER_KEY.into()) else {
        return None;
    };
    Some(OrderData {
        order_id: int(c, "order_id"),
        link_index: int(c, "link_index"),
        final_link: int(c, "is_final_link") != 0,
        fragment_index: int(c, "fragment_index"),
        is_final: int(c, "is_final") != 0,
        context: c.get(&"order_context".into()).and_then(order_from_nbt),
    })
}

/// `PackageItem.setOrder`.
pub(super) fn set_order(stack: &mut ItemStack, d: &OrderData<ItemStack>) {
    let mut c = vec![
        ("order_id", Tag::Int(d.order_id)),
        ("link_index", Tag::Int(d.link_index)),
        ("is_final_link", Tag::Byte(i8::from(d.final_link))),
        ("fragment_index", Tag::Int(d.fragment_index)),
        ("is_final", Tag::Byte(i8::from(d.is_final))),
    ];
    if let Some(o) = &d.context {
        c.push(("order_context", order_nbt(o)));
    }
    set_custom(stack, ORDER_KEY, Some(Tag::compound(c)));
}
