//! Smart chutes' filters as the engine tests them (`create_engine::filter`): a filter slot's
//! stack read as Create's `FilterItemStack.of` reads it, from Create's settings in the stack's
//! `minecraft:custom_data` (Horizon adds no component types), and a stack tested against it.

use super::items::id_of;
use create_engine::filter::{
    ATTRIBUTES_KEY, BLACKLIST_KEY, Filter, FilterItem, ITEMS_KEY, LIST_SLOTS, MODE_KEY, Probe,
    RESPECT_NBT_KEY, Spec,
};
use mod_api::{Compound, Host, ItemStack, Tag};
use std::rc::Rc;

const CUSTOM_DATA: &str = "minecraft:custom_data";
/// `PackageItem`'s address, in custom data.
const ADDRESS_KEY: &str = "create:package_address";

fn custom(stack: &ItemStack) -> Option<&Compound> {
    match stack.components()?.as_nbt().get(&CUSTOM_DATA.into()) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    }
}

fn get<'a>(c: Option<&'a Compound>, key: &str) -> Option<&'a Tag> {
    c?.get(&key.into())
}

fn flag(t: Option<&Tag>) -> bool {
    matches!(t, Some(Tag::Byte(b)) if *b != 0)
}

fn text(t: Option<&Tag>) -> Option<String> {
    match t {
        Some(Tag::String(s)) => Some(s.to_string_lossy()),
        Some(Tag::Int(i)) => Some(i.to_string()),
        _ => None,
    }
}

/// The stack's components as one text (`isSameItemSameComponents`), "" when it has none.
fn components(stack: &ItemStack) -> String {
    let mut c = stack
        .components()
        .map(|p| p.as_nbt().clone())
        .unwrap_or_default();
    if let Some(d) = stack.damage_component() {
        c.insert("minecraft:damage".into(), Tag::Int(d as i32));
    }
    if c.is_empty() {
        String::new()
    } else {
        format!("{c:?}")
    }
}

/// What a filter slot's stack is (`FilterItemStack.of` reads it).
fn spec(host: &dyn Host, stack: &ItemStack) -> Spec {
    let c = custom(stack);
    let list = get(c, ITEMS_KEY);
    let items: Vec<ItemStack> = match list {
        Some(Tag::List(l)) => l
            .iter()
            .take(LIST_SLOTS)
            .filter_map(|t| match t {
                Tag::Compound(e) => ItemStack::from_nbt(host, e),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    Spec {
        item: id_of(host, stack),
        components: components(stack),
        has_list: list.is_some(),
        list: items.iter().map(|s| spec(host, s)).collect(),
        respect_nbt: flag(get(c, RESPECT_NBT_KEY)),
        blacklist: flag(get(c, BLACKLIST_KEY)),
        mode: text(get(c, MODE_KEY)),
        attributes: match get(c, ATTRIBUTES_KEY) {
            Some(Tag::List(l)) => Some(
                l.iter()
                    .filter_map(|e| {
                        let Tag::Compound(e) = e else { return None };
                        let Some(Tag::Compound(a)) = e.get(&"attribute".into()) else {
                            return None;
                        };
                        let kind = text(a.get(&"type".into()))?;
                        let value = text(a.get(&"value".into()));
                        Some((kind, value, flag(e.get(&"inverted".into()))))
                    })
                    .collect(),
            ),
            _ => None,
        },
        address: address(stack),
    }
}

fn address(stack: &ItemStack) -> String {
    text(get(custom(stack), ADDRESS_KEY)).unwrap_or_default()
}

/// `FilterItemStack.of` on a copy of `stack` (a filter item one of it): the compiled filter,
/// keeping the stack for its save and its return.
pub fn filter_of(host: &dyn Host, stack: &ItemStack) -> Filter {
    let mut s = stack.clone();
    if FilterItem::of(&id_of(host, &s)).is_some() {
        let _ = s.set_count(1);
    }
    Filter::compile(&spec(host, &s)).with_source(Rc::new(s))
}

/// The stack a filter came from (its item alone for one saved before filter items).
pub fn filter_stack(host: &dyn Host, f: &Filter) -> Option<ItemStack> {
    if let Some(s) = f.source::<ItemStack>() {
        return Some(s.clone());
    }
    match &f.kind {
        create_engine::filter::Kind::Item { item, .. } => super::items::stack_of(host, item, 1),
        _ => super::items::stack_of(host, create_engine::filter::FILTER, 1),
    }
}

/// A filter as saved with its column: the stack's NBT.
pub fn filter_to_nbt(host: &dyn Host, f: &Filter) -> Tag {
    filter_stack(host, f)
        .and_then(|s| s.to_nbt(host))
        .map_or(Tag::String("".into()), Tag::Compound)
}

/// What `filter_to_nbt` wrote, or an item id saved before filter items.
pub fn filter_from_nbt(host: &dyn Host, tag: &Tag) -> Option<Filter> {
    match tag {
        Tag::String(s) => {
            let item = s.to_string_lossy();
            (!item.is_empty()).then(|| Filter::item(&item))
        }
        Tag::Compound(c) => ItemStack::from_nbt(host, c).map(|s| filter_of(host, &s)),
        _ => None,
    }
}

/// A stack as a filter sees it. Item attributes (`ItemAttribute.appliesTo`) read the processing
/// recipes, which logistics does not hold: none applies.
struct Looked<'a> {
    item: String,
    stack: &'a ItemStack,
}

impl Probe for Looked<'_> {
    fn item(&self) -> &str {
        &self.item
    }
    fn components(&self) -> String {
        components(self.stack)
    }
    fn address(&self) -> String {
        address(self.stack)
    }
}

/// Whether `stack` passes `filter` (none: anything does).
pub fn passes(host: &dyn Host, filter: Option<&Filter>, stack: &ItemStack) -> bool {
    filter.is_none_or(|f| {
        f.test(&Looked {
            item: id_of(host, stack),
            stack,
        })
    })
}
