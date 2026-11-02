// The template test's stand-in for `mixins/create/package_body.rs`: the fixture's stacks carry
// no components, so a package keeps no address or contents on its stack here (the glue carries
// the address beside the stack), and stacks go to NBT as id and count.
use horizon_client::items::ItemStack;
use horizon_world::nbt::Tag;

pub(super) fn address(stack: &ItemStack) -> String {
    let _ = stack;
    String::new()
}

pub(super) fn set_address(stack: &mut ItemStack, address: Option<&str>) {
    let _ = (stack, address);
}

pub(super) fn contents(stack: &ItemStack) -> Vec<Option<ItemStack>> {
    let _ = stack;
    vec![None; create_engine::package::SLOTS]
}

pub(super) fn set_contents(stack: &mut ItemStack, slots: &[Option<ItemStack>]) {
    let _ = (stack, slots);
}

pub(super) fn to_nbt(stack: &ItemStack) -> Tag {
    let mut c = horizon_world::nbt::Compound::new();
    c.insert("id".into(), Tag::String((&*stack.item).into()));
    c.insert("count".into(), Tag::IntArray(vec![stack.count as i32]));
    Tag::Compound(c)
}

pub(super) fn from_nbt(tag: &Tag) -> Option<ItemStack> {
    let Tag::Compound(c) = tag else { return None };
    let Some(Tag::String(id)) = c.get(&"id".into()) else {
        return None;
    };
    let count = match c.get(&"count".into()) {
        Some(Tag::IntArray(v)) => v.first().copied().unwrap_or(1).max(1) as u32,
        _ => 1,
    };
    Some(ItemStack::new(&id.to_string_lossy(), count))
}

pub(super) fn snbt(stack: &ItemStack) -> String {
    format!("{{id:'{}',count:{}}}", stack.item, stack.count)
}

/// The fixture's stacks keep no order data; an order saved with a requester is its stacks.
pub(super) fn order(stack: &ItemStack) -> Option<create_engine::stock::OrderData<ItemStack>> {
    let _ = stack;
    None
}

pub(super) fn set_order(stack: &mut ItemStack, d: &create_engine::stock::OrderData<ItemStack>) {
    let _ = (stack, d);
}

pub(super) fn order_nbt(o: &create_engine::stock::Order<ItemStack>) -> Tag {
    Tag::List(
        o.stacks
            .iter()
            .map(|(s, n)| {
                let mut c = horizon_world::nbt::Compound::new();
                c.insert("item_stack".into(), to_nbt(s));
                c.insert("count".into(), Tag::IntArray(vec![*n as i32]));
                Tag::Compound(c)
            })
            .collect(),
    )
}

pub(super) fn order_from_nbt(t: &Tag) -> Option<create_engine::stock::Order<ItemStack>> {
    let Tag::List(l) = t else { return None };
    let stacks = l
        .iter()
        .filter_map(|e| {
            let Tag::Compound(c) = e else { return None };
            let s = from_nbt(c.get(&"item_stack".into())?)?;
            let n = match c.get(&"count".into()) {
                Some(Tag::IntArray(v)) => v.first().copied().unwrap_or(0).max(0) as u32,
                _ => 0,
            };
            Some((s, n))
        })
        .collect();
    Some(create_engine::stock::Order::simple(stacks))
}
