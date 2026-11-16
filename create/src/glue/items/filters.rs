//! Filters (`FilterItemStack`, `create-engine`'s `filter`): what a filter slot's stack is, a
//! stack tested against it, a filter taken from a hand into a slot and given back
//! (`FilteringBehaviour.onShortInteract`), the filter items of broken blocks dropped
//! (`FilteringBehaviour.destroy`), and the list and attribute filters' screens opened from the
//! hand (`FilterItem.use`, drawn by `filter_screen`) and what they chose written back
//! (`FilterScreenPacket`, the menus' `saveData`). A filter's settings are kept in the stack's
//! custom data under Create's component names, its list as `ItemContainerContents` (`Slot` and
//! the stack's own NBT). `/create_debug filter` sets a filter in a slot or hand for tests, and
//! sets a package filter's address too (its screen is the stock glue's, `stock::package_filter`).
//!
//! The attributes that read a recipe book (`washable`, `hauntable`, `crushable`, `smeltable`,
//! `smokable`, `blastable`) ask processing's recipes.
//!
//! The item tags `in_tag` offers for a reference item (`ItemStack.getTags`) are the tags it is in
//! among those the server's data defines (`tags/item`, listed by a data reload listener).
//!
//! `fluid_container` and `has_fluid` know the buckets (the items the game gives a fluid
//! handler), `has_color` dyes, fireworks and colour-named items, and
//! `shulker_fill_level` a shulker box's `container`. Left out: `in_item_group`, as no API reads
//! the creative tabs (`API_GAPS.md`).
//!
//! `max_enchanted` reads each enchantment's maximum level from the server's data
//! (`enchantment/*.json`'s `max_level`, listed by a data reload listener), as the host answers
//! `Enchantment.getMaxLevel` on the client only.

use super::super::net;
use super::super::processing::recipes::{self, Recipes};
use super::super::server::ServerLevel;
use super::super::world::with_world;
use super::filter_screen;
use super::{creative, custom, give_back, held, set_custom, set_held, set_slot, slot_stack};
use create_engine::filter::{
    self, ATTRIBUTES_KEY, Attribute, BLACKLIST_KEY, Filter, ITEMS_KEY, LIST_SLOTS, MODE_KEY, Mode,
    Passes, Probe, RESPECT_NBT_KEY, Spec,
};
use mod_api::{
    CommandArgumentValue, CommandSource, Compound, Hand, Host, HostSide, Identifier,
    InteractionResult, ItemPart, ItemStack, PlayerRef, RunModCommandEvent, ServerHost, SoundSource,
    Tag, UseItemEvent, UseOnEvent,
};
use std::collections::BTreeMap;
use std::rc::Rc;

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

/// The stack's components as one canonical text (`isSameItemSameComponents`): its patch and
/// damage, "" when it has neither.
pub(crate) fn components(stack: &ItemStack) -> String {
    match (stack.components(), stack.damage_component()) {
        (None, None) => String::new(),
        (patch, damage) => format!("{:?}|{damage:?}", patch.map(|p| p.as_nbt())),
    }
}

/// `PackageItem.getAddress`: "" when none.
pub(crate) fn address(stack: &ItemStack) -> String {
    text(custom(stack).get(&create_engine::package::ADDRESS_KEY.into())).unwrap_or_default()
}

/// The stacks of an `ItemContainerContents` list, by slot.
fn decode_items(host: &dyn Host, tag: Option<&Tag>, size: usize) -> Vec<Option<ItemStack>> {
    let mut slots = vec![None; size];
    if let Some(Tag::List(items)) = tag {
        for it in items {
            let Tag::Compound(c) = it else {
                continue;
            };
            let slot = match c.get(&"Slot".into()) {
                Some(Tag::Byte(b)) => *b as u8 as usize,
                Some(Tag::Int(i)) => *i as usize,
                _ => 0,
            };
            if let Some(cell) = slots.get_mut(slot) {
                *cell = ItemStack::from_nbt(host, c);
            }
        }
    }
    slots
}

/// The `ItemContainerContents` list of `slots`.
fn encode_items(host: &dyn Host, slots: &[Option<ItemStack>]) -> Tag {
    Tag::List(
        slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let mut c = s.as_ref()?.to_nbt(host)?;
                c.insert("Slot".into(), Tag::Byte(i as i8));
                Some(Tag::Compound(c))
            })
            .collect(),
    )
}

/// What a filter slot's stack is (`FilterItemStack.of` reads it).
pub(crate) fn spec(host: &dyn Host, stack: &ItemStack) -> Spec {
    let c = custom(stack);
    let list = c.get(&ITEMS_KEY.into());
    Spec {
        item: stack.item().id(host),
        components: components(stack),
        has_list: list.is_some(),
        list: decode_items(host, list, LIST_SLOTS)
            .into_iter()
            .flatten()
            .map(|s| spec(host, &s))
            .collect(),
        respect_nbt: flag(c.get(&RESPECT_NBT_KEY.into())),
        blacklist: flag(c.get(&BLACKLIST_KEY.into())),
        mode: text(c.get(&MODE_KEY.into())),
        attributes: match c.get(&ATTRIBUTES_KEY.into()) {
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
        fluid: crate::glue::logistics::items::potion_name(host, stack)
            .and_then(|n| create_engine::tank::potion_fluid(&n)),
    }
}

/// `FilterItemStack.of` on a copy of `stack` (a filter item one of it): the compiled filter,
/// keeping the stack for its save and its return.
pub(crate) fn filter_of(host: &dyn Host, stack: &ItemStack) -> Filter {
    let mut s = stack.clone();
    if filter::FilterItem::of(&s.item().id(host)).is_some() {
        let _ = s.set_count(1);
    }
    Filter::compile(&spec(host, &s)).with_source(Rc::new(s))
}

/// The stack a filter came from (its item alone for one saved before filter items).
pub(crate) fn filter_stack(host: &dyn Host, f: &Filter) -> Option<ItemStack> {
    if let Some(s) = f.source::<ItemStack>() {
        return Some(s.clone());
    }
    let id = match &f.kind {
        filter::Kind::Item { item, .. } => item.to_string(),
        _ => filter::FILTER.to_owned(),
    };
    let key = host.item_key(&Identifier::parse(&id).ok()?)?;
    Some(ItemStack::of(key))
}

/// A filter as saved: the stack's NBT.
pub(crate) fn filter_to_nbt(host: &dyn Host, f: &Filter) -> Option<Tag> {
    Some(Tag::Compound(filter_stack(host, f)?.to_nbt(host)?))
}

/// What `filter_to_nbt` wrote, or an item id saved before filter items.
pub(crate) fn filter_from_nbt(host: &dyn Host, tag: &Tag) -> Option<Filter> {
    match tag {
        Tag::String(s) => {
            let item = s.to_string_lossy();
            (!item.is_empty()).then(|| Filter::item(&item))
        }
        Tag::Compound(c) => ItemStack::from_nbt(host, c).map(|s| filter_of(host, &s)),
        _ => None,
    }
}

/// The value of component `id` the stack has: its own, else its item's default.
pub(crate) fn component(host: &dyn Host, stack: &ItemStack, id: &str) -> Option<Tag> {
    if let Some(patch) = stack.components().map(|c| c.as_nbt()) {
        if patch.contains_key(&format!("!{id}").as_str().into()) {
            return None;
        }
        if let Some(t) = patch.get(&id.into()) {
            return Some(t.clone());
        }
    }
    host.item_default_component(stack.item(), id)
}

/// The enchantment ids of an `ItemEnchantments` component (`{levels: {..}}` or the map).
/// `FLUID_CONTAINER`: the item has a fluid handler (`Capabilities.FluidHandler.ITEM`): the
/// buckets, empty or of a fluid (vanilla's, milk and Create's honey and chocolate).
fn fluid_container(id: &str) -> bool {
    id == "minecraft:bucket" || create_engine::tank::Fluid::of_bucket(id).is_some()
}

/// `FluidContentsAttribute.extractFluids`: the fluid an item's handler holds, by id.
fn held_fluid(id: &str) -> Option<String> {
    create_engine::tank::Fluid::of_bucket(id).map(|f| f.id().to_owned())
}

/// `DyeColor.byFireworkColor`'s colours: the colour each dye gives a firework star, a vanilla fact
/// the game shows; as arismoko/horizon (MIT) has them (`THIRD_PARTY_NOTICES.md`).
const FIREWORK_COLORS: [(&str, i32); 16] = [
    ("white", 0xF0F0F0),
    ("orange", 0xEB8844),
    ("magenta", 0xC354CD),
    ("light_blue", 0x6689D3),
    ("yellow", 0xDECF2A),
    ("lime", 0x41CD34),
    ("pink", 0xD88198),
    ("gray", 0x434343),
    ("light_gray", 0xABABAB),
    ("cyan", 0x287697),
    ("purple", 0x7B2FBE),
    ("blue", 0x253192),
    ("brown", 0x51301A),
    ("green", 0x3B511A),
    ("red", 0xB3312C),
    ("black", 0x1E1B1B),
];

/// `ColorAttribute.findMatchingDyeColors`: a dye's colour (`DyeColor.getColor`: a vanilla dye,
/// `minecraft:<colour>_dye`); else a firework's explosions' colours and fade colours, and the
/// colours the item's id starts with (`red_wool`).
fn colors_of(host: &dyn Host, stack: &ItemStack, id: &str) -> Vec<String> {
    let path = id.split_once(':').map_or(id, |s| s.1);
    for (c, _) in FIREWORK_COLORS {
        if id == format!("minecraft:{c}_dye") {
            return vec![c.to_owned()];
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut add = |c: &str| {
        if !out.iter().any(|o| o == c) {
            out.push(c.to_owned());
        }
    };
    if (id == "minecraft:firework_rocket" || id == "minecraft:firework_star")
        && let Some(Tag::Compound(f)) = component(host, stack, "minecraft:fireworks")
        && let Some(Tag::List(explosions)) = f.get(&"explosions".into())
    {
        for e in explosions {
            let Tag::Compound(e) = e else { continue };
            for key in ["colors", "fade_colors"] {
                let rgbs: Vec<i32> = match e.get(&key.into()) {
                    Some(Tag::IntArray(v)) => v.clone(),
                    Some(Tag::List(l)) => l
                        .iter()
                        .filter_map(|t| match t {
                            Tag::Int(i) => Some(*i),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                for rgb in rgbs {
                    if let Some((c, _)) = FIREWORK_COLORS.iter().find(|(_, f)| *f == rgb) {
                        add(c);
                    }
                }
            }
        }
    }
    for (c, _) in FIREWORK_COLORS {
        if path.starts_with(&format!("{c}_")) {
            add(c);
        }
    }
    out
}

/// `ShulkerLevels.canApply`'s count for a shulker box: none for another item or one with a loot
/// table, else its container's slots (`getSlots`, to its last stack), `i32::MAX` for 27 full
/// stacks.
fn shulker_level(host: &dyn Host, stack: &ItemStack, id: &str) -> Option<i32> {
    if !id.ends_with("shulker_box") {
        return None;
    }
    if component(host, stack, "minecraft:container_loot").is_some() {
        return None;
    }
    let Some(Tag::List(slots)) = component(host, stack, "minecraft:container") else {
        return Some(0);
    };
    let mut filled: Vec<(i32, Option<ItemStack>)> = Vec::new();
    for t in slots {
        let Tag::Compound(c) = t else { continue };
        let slot = match c.get(&"slot".into()) {
            Some(Tag::Int(i)) => *i,
            Some(Tag::Byte(b)) => i32::from(*b),
            _ => continue,
        };
        let item = match c.get(&"item".into()) {
            Some(Tag::Compound(i)) => ItemStack::from_nbt(host, i),
            _ => None,
        };
        filled.push((slot, item));
    }
    let size = filled.iter().map(|(s, _)| s + 1).max().unwrap_or(0);
    if size < 27 {
        return Some(size);
    }
    let full = (0..27).all(|i| {
        filled.iter().any(|(s, st)| {
            *s == i
                && st
                    .as_ref()
                    .is_some_and(|st| st.count() == host.max_stack_size(st.item()).unwrap_or(64))
        })
    });
    Some(if full { i32::MAX } else { size })
}

fn shulker_level_holds(level: &str, size: i32) -> bool {
    match level {
        "empty" => size == 0,
        "partial" => size > 0 && size < i32::MAX,
        _ => size == i32::MAX,
    }
}

/// The enchantments and their levels in an `enchantments` component.
fn enchantment_levels(tag: Option<Tag>) -> Vec<(String, i32)> {
    let Some(Tag::Compound(c)) = tag else {
        return Vec::new();
    };
    let map = match c.get(&"levels".into()) {
        Some(Tag::Compound(l)) => l.clone(),
        _ => c,
    };
    map.iter()
        .filter_map(|(k, v)| {
            let level = match v {
                Tag::Int(i) => *i,
                Tag::Byte(b) => i32::from(*b),
                Tag::Short(s) => i32::from(*s),
                _ => return None,
            };
            Some((k.to_string_lossy(), level))
        })
        .collect()
}

fn enchantment_ids(tag: Option<Tag>) -> Vec<String> {
    let Some(Tag::Compound(c)) = tag else {
        return Vec::new();
    };
    let map = match c.get(&"levels".into()) {
        Some(Tag::Compound(l)) => l.clone(),
        _ => c,
    };
    map.keys().map(|k| k.to_string_lossy()).collect()
}

/// The plain text of a `custom_name` component.
pub(crate) fn plain_name(tag: Option<Tag>) -> Option<String> {
    match tag? {
        Tag::String(s) => Some(s.to_string_lossy()),
        Tag::Compound(c) => text(c.get(&"text".into())),
        _ => None,
    }
}

/// A stack as a filter sees it.
pub(crate) struct Looked<'a> {
    pub(crate) host: &'a dyn Host,
    pub(crate) stack: &'a ItemStack,
    /// The stack's item id.
    pub(crate) id: String,
}

impl<'a> Looked<'a> {
    pub(crate) fn new(host: &'a dyn Host, stack: &'a ItemStack) -> Looked<'a> {
        Looked {
            host,
            stack,
            id: stack.item().id(host),
        }
    }
}

impl Probe for Looked<'_> {
    fn item(&self) -> &str {
        &self.id
    }
    fn components(&self) -> String {
        components(self.stack)
    }
    fn address(&self) -> String {
        address(self.stack)
    }
    /// The item facts an attribute reads (`AllItemAttributeTypes`' singletons and the valued
    /// ones on the stack).
    fn attribute(&self, a: &Attribute) -> bool {
        let (host, stack) = (self.host, self.stack);
        let item = stack.item();
        let max_damage = host.max_damage(item).unwrap_or(0);
        let written = || match component(host, stack, "minecraft:written_book_content") {
            Some(Tag::Compound(c)) => Some(c),
            _ => None,
        };
        match a {
            Attribute::Placeable => host.item_block(item).is_some(),
            Attribute::Consumable => component(host, stack, "minecraft:food").is_some(),
            Attribute::Enchanted => {
                !enchantment_ids(component(host, stack, "minecraft:enchantments")).is_empty()
            }
            Attribute::Renamed => stack
                .components()
                .is_some_and(|c| c.as_nbt().contains_key(&"minecraft:custom_name".into())),
            Attribute::Damaged => max_damage > 0 && stack.damage() > 0,
            Attribute::BadlyDamaged => filter::badly_damaged(stack.damage(), max_damage),
            Attribute::NotStackable => filter::not_stackable(
                host.max_stack_size(item).unwrap_or(64),
                max_damage,
                stack.damage(),
            ),
            Attribute::Equipable => match component(host, stack, "minecraft:equippable") {
                Some(Tag::Compound(c)) => {
                    text(c.get(&"slot".into())).is_some_and(|s| s != "mainhand" && s != "offhand")
                }
                _ => false,
            },
            Attribute::FurnaceFuel => host.item_burn_time(item).is_some_and(|t| t > 0),
            Attribute::Compostable => host.item_compost_chance(item).is_some(),
            Attribute::InTag(tag) => Identifier::parse(tag)
                .ok()
                .and_then(|id| host.tags().key(&id))
                .and_then(|k| host.tags().is_in_tag(item, k))
                .unwrap_or(false),
            Attribute::AddedBy(m) => self.id.split_once(':').map_or("minecraft", |s| s.0) == m,
            // `getEnchantmentsForCrafting`: a book's stored ones.
            Attribute::HasEnchant(e) => {
                let which = if self.id == "minecraft:enchanted_book" {
                    "minecraft:stored_enchantments"
                } else {
                    "minecraft:enchantments"
                };
                enchantment_ids(component(host, stack, which)).contains(e)
            }
            Attribute::HasName(n) => {
                plain_name(component(host, stack, "minecraft:custom_name")).as_deref()
                    == Some(n.as_str())
            }
            Attribute::BookAuthor(author) => {
                written()
                    .and_then(|w| text(w.get(&"author".into())))
                    .unwrap_or_default()
                    == *author
            }
            Attribute::BookCopy(g) => {
                let generation = written().map_or(-1, |w| match w.get(&"generation".into()) {
                    Some(Tag::Int(i)) => *i,
                    _ => 0,
                });
                generation == *g
            }
            // The attributes that read a recipe book: processing's recipes (none read yet, or
            // off the server thread: false).
            Attribute::Washable => recipe(&self.id, |r| &r.splashing),
            Attribute::Hauntable => recipe(&self.id, |r| &r.haunting),
            Attribute::Crushable => {
                recipe(&self.id, |r| &r.crushing) || recipe(&self.id, |r| &r.milling)
            }
            Attribute::Smeltable => recipe(&self.id, |r| &r.smelting),
            Attribute::Smokable => recipe(&self.id, |r| &r.smoking),
            Attribute::Blastable => recipe(&self.id, |r| &r.blasting),
            Attribute::FluidContainer => fluid_container(&self.id),
            Attribute::HasFluid(f) => held_fluid(&self.id).is_some_and(|h| h == *f),
            Attribute::HasColor(c) => colors_of(host, stack, &self.id).contains(c),
            Attribute::ShulkerFillLevel(l) => shulker_level(host, stack, &self.id)
                .is_some_and(|size| shulker_level_holds(l, size)),
            // `maxEnchanted`: an enchantment (`getTagEnchantments`) at its maximum level or past it.
            Attribute::MaxEnchanted => {
                let max = ENCHANTMENT_MAX
                    .lock()
                    .map(|m| m.clone())
                    .unwrap_or_default();
                enchantment_levels(component(host, stack, "minecraft:enchantments"))
                    .iter()
                    .any(|(id, level)| max.get(id).is_some_and(|m| *m <= *level))
            }
        }
    }
}

/// Whether a recipe of the book `pick` takes `item` alone (`RecipeManager.getRecipeFor` with
/// the item's single-slot input, read from Create's recipe store).
fn recipe(item: &str, pick: fn(&Recipes) -> &create_engine::processing::Book) -> bool {
    recipes::with(|r| pick(r).find_single(item).is_some()) == Some(true)
}

/// `FilteringBehaviour.test(stack)`: anything with no filter.
pub(crate) fn passes(host: &dyn Host, filter: Option<&Filter>, stack: &ItemStack) -> bool {
    filter.is_none_or(|f| f.test(&Looked::new(host, stack)))
}

/// The same for an item as a belt names it (`logistics::items::key_of`: a plain id, or one
/// with its components).
pub(crate) fn passes_item(host: &dyn Host, filter: Option<&Filter>, item: &str) -> bool {
    let Some(f) = filter else {
        return true;
    };
    // A stack with components is tested as itself.
    if item.contains(['#', '|']) {
        return match crate::glue::logistics::items::stack_of(host, item, 1) {
            Some(stack) => passes(host, filter, &stack),
            None => f.test(item),
        };
    }
    // A plain item filter needs no stack.
    if matches!(f.kind, filter::Kind::Item { .. }) {
        return f.test(item);
    }
    let Some(stack) = Identifier::parse(item)
        .ok()
        .and_then(|id| host.item_key(&id))
        .map(ItemStack::of)
    else {
        return f.test(item);
    };
    f.test(&Looked::new(host, &stack))
}

/// A detector's filter with the host to read attributes.
pub(crate) struct Detector<'a> {
    pub(crate) host: &'a dyn Host,
    pub(crate) filter: Option<&'a Filter>,
}

impl Passes for Detector<'_> {
    fn filtered(&self) -> bool {
        self.filter.is_some()
    }
    fn item(&self, item: &str) -> bool {
        passes_item(self.host, self.filter, item)
    }
    fn fluid(&self, fluid: create_engine::tank::Fluid) -> bool {
        self.filter.is_none_or(|f| f.test_fluid(fluid))
    }
}

/// `FilteringBehaviour.onShortInteract`'s hand: what the slot is set to (None: the hand is
/// empty), a filter item taken from the hand outside creative.
pub(crate) fn take_filter(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    hand: Hand,
) -> Option<Filter> {
    let stack = held(&*host, player, hand)?;
    let f = filter_of(&*host, &stack);
    if f.is_filter_item() && !creative(&*host, player) {
        set_held(host, player, hand, stack.shrink(1));
    }
    Some(f)
}

/// The filter the slot held before, given back when it was a filter item (in creative only
/// when the inventory has none like it), and the frame sound.
pub(crate) fn return_filter(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    pos: [i32; 3],
    old: Option<Filter>,
) {
    if let Some(stack) = old
        .filter(Filter::is_filter_item)
        .and_then(|f| filter_stack(&*lv.host, &f))
    {
        let has = player
            .inventory_slots(&*lv.host)
            .unwrap_or_default()
            .iter()
            .any(|(_, s)| s.is_stackable_with(&stack));
        if !creative(&*lv.host, player) || !has {
            give_back(lv.host, player, stack);
        }
    }
    let center = pos.map(|v| f64::from(v) + 0.5);
    lv.level.play_sound(
        &mut *lv.host,
        "minecraft:entity.item_frame.add_item",
        center,
        SoundSource::Blocks,
        0.25,
        0.1,
    );
}

/// `FilteringBehaviour.destroy`: filter items of broken blocks drop at their centre.
pub(super) fn level_tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let dropped = with_world(|w| {
        let d = &mut w.dims[dim];
        if d.dropped_filters.is_empty() && d.detectors.dropped.is_empty() {
            return Vec::new();
        }
        let mut v = std::mem::take(&mut d.dropped_filters);
        v.append(&mut d.detectors.dropped);
        v
    })
    .unwrap_or_default();
    for (pos, f) in dropped {
        let at = pos.map(|v| f64::from(v) + 0.5);
        if let Some(stack) = filter_stack(&*lv.host, &f) {
            let _ = lv.level.spawn_item_with_motion(
                &mut *lv.host,
                at[0],
                at[1],
                at[2],
                stack,
                [0.0; 3],
                10,
            );
        }
    }
}

pub(super) fn register_commands(commands: &mut Vec<mod_api::Command>) {
    // The filters' screens: a list (allow or deny, respecting components), attributes (any,
    // all or none of them) or a package address on the filter in a slot or hand.
    commands.push(mod_api::Command::op("create_debug filter <args:text>", 2));
}

/// `create_debug filter <slot> list <allow|deny> [nbt] <item>...`,
/// `... attribute <any|all|none> <[!]type[=value]>...`, `... address <text>`.
pub(super) fn command(host: &mut dyn ServerHost, event: &RunModCommandEvent<'_>) -> String {
    const USAGE: &str = "usage: create_debug filter <hand|offhand|slot> list <allow|deny> [nbt] \
                         <item>... | attribute <any|all|none> <[!]attribute[=value]>... | address <text>";
    let CommandSource::Player { player, .. } = event.source else {
        return "filter: only a player holds a filter".into();
    };
    let player = *player;
    let rest = match event.command.argument("args") {
        Some(CommandArgumentValue::String(s)) => s.clone(),
        _ => return USAGE.into(),
    };
    let words: Vec<&str> = rest.split_whitespace().collect();
    let (Some(slot), Some(kind)) = (words.first(), words.get(1)) else {
        return USAGE.into();
    };
    let args = &words[2.min(words.len())..];
    let Some((at, stack)) = slot_stack(&*host, player, slot) else {
        return USAGE.into();
    };
    let Some(mut stack) = stack else {
        return format!("filter: nothing in {slot}");
    };
    let want = match *kind {
        "list" => filter::FILTER,
        "attribute" => filter::ATTRIBUTE_FILTER,
        "address" => filter::PACKAGE_FILTER,
        _ => return USAGE.into(),
    };
    let id = stack.item().id(&*host);
    if id != want {
        return format!("filter: {slot} holds {id}, not {want}");
    }
    let line = match *kind {
        "list" => {
            let deny = match args.first() {
                Some(&"allow") => false,
                Some(&"deny") => true,
                _ => return USAGE.into(),
            };
            let nbt = args.get(1) == Some(&"nbt");
            let mut items = Vec::new();
            for i in &args[if nbt { 2 } else { 1 }.min(args.len())..] {
                let id = if i.contains(':') {
                    (*i).to_owned()
                } else {
                    format!("minecraft:{i}")
                };
                let Some(key) = Identifier::parse(&id)
                    .ok()
                    .and_then(|id| host.item_key(&id))
                else {
                    return format!("filter: no item {id}");
                };
                items.push(Some(ItemStack::of(key)));
            }
            if items.len() > LIST_SLOTS {
                return format!("filter: at most {LIST_SLOTS} items");
            }
            let list = encode_items(&*host, &items);
            set_custom(
                &mut stack,
                &[
                    (ITEMS_KEY, Some(list)),
                    (RESPECT_NBT_KEY, Some(Tag::Byte(i8::from(nbt)))),
                    (BLACKLIST_KEY, Some(Tag::Byte(i8::from(deny)))),
                ],
            );
            format!(
                "filter: {} list of {} items{}",
                if deny { "deny" } else { "allow" },
                items.len(),
                if nbt { ", respecting components" } else { "" }
            )
        }
        "attribute" => {
            let mode = match args.first() {
                Some(&"any") => Mode::WhitelistDisj,
                Some(&"all") => Mode::WhitelistConj,
                Some(&"none") => Mode::Blacklist,
                _ => return USAGE.into(),
            };
            let mut tests = Vec::new();
            for a in &args[1.min(args.len())..] {
                let (inverted, a) = match a.strip_prefix('!') {
                    Some(a) => (true, a),
                    None => (false, *a),
                };
                let (kind, value) = match a.split_once('=') {
                    Some((k, v)) => (k, Some(v)),
                    None => (a, None),
                };
                match Attribute::parse(kind, value) {
                    Ok(at) => tests.push((at, inverted)),
                    Err(filter::Refused::NotPorted(n)) => {
                        return format!("filter: the {n} attribute is not ported");
                    }
                    Err(filter::Refused::Unknown(n)) => return format!("filter: {n}?"),
                }
            }
            set_attributes(&mut stack, mode, &tests);
            format!("filter: {} of {} attributes", mode.name(), tests.len())
        }
        _ => {
            let address = args.join(" ");
            let value = (!address.is_empty()).then(|| Tag::String(address.as_str().into()));
            set_custom(&mut stack, &[(create_engine::package::ADDRESS_KEY, value)]);
            format!("filter: address \"{address}\"")
        }
    };
    set_slot(host, player, at, stack);
    line
}

/// An attribute filter's mode and attributes (`ItemAttributeEntry`'s codec:
/// `{attribute: {type, value}, inverted}`).
fn set_attributes(stack: &mut ItemStack, mode: Mode, tests: &[(Attribute, bool)]) {
    let raw: Vec<(String, Option<String>, bool)> = tests
        .iter()
        .map(|(a, inverted)| (a.id(), a.value(), *inverted))
        .collect();
    write_attributes(stack, mode, &raw);
}

/// `set_attributes` of saved type ids and values (a `book_copy`'s generation an int).
fn write_attributes(stack: &mut ItemStack, mode: Mode, tests: &[(String, Option<String>, bool)]) {
    let list = tests
        .iter()
        .map(|(kind, value, inverted)| {
            let mut attribute = Compound::new();
            attribute.insert("type".into(), Tag::String(kind.as_str().into()));
            if let Some(v) = value {
                let v = match (kind.as_str(), v.parse::<i32>()) {
                    ("create:book_copy", Ok(g)) => Tag::Int(g),
                    _ => Tag::String(v.as_str().into()),
                };
                attribute.insert("value".into(), v);
            }
            let mut e = Compound::new();
            e.insert("attribute".into(), Tag::Compound(attribute));
            e.insert("inverted".into(), Tag::Byte(i8::from(*inverted)));
            Tag::Compound(e)
        })
        .collect();
    set_custom(
        stack,
        &[
            (MODE_KEY, Some(Tag::String(mode.name().into()))),
            (ATTRIBUTES_KEY, Some(Tag::List(list))),
        ],
    );
}

// ---------------------------------------------------------------------------------------------
// The screens (`FilterItem.use`, `FilterScreenPacket`, the menus' `saveData`)

/// The list and attribute filters' items: right-clicked with the main hand, not sneaking, they
/// open their screen, in the air or on a block that took no click of its own
/// (`FilterItem.use` / `useOn`; a block's filter slot takes the click before the item).
pub(super) struct FilterPart;

#[mod_api::horizon_part]
impl ItemPart for FilterPart {
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        used(host, event.player, event.hand)
    }

    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        used(host, event.player, event.hand)
    }
}

/// `FilterItem.use`: success (and the screen, on the server) for the main hand when not
/// sneaking; a pass otherwise.
fn used(host: HostSide, player: u128, hand: Hand) -> Option<InteractionResult> {
    if hand != Hand::Main {
        return None;
    }
    match host {
        HostSide::Server(host) => {
            let player = host.player(player)?;
            if super::sneaking(&*host, player) {
                return None;
            }
            open_screen(host, player);
        }
        HostSide::Client(host) => {
            let player = host.local_player()?;
            if super::sneaking(&*host, player) {
                return None;
            }
        }
    }
    Some(InteractionResult::Success)
}

/// What the server keeps of a screen it opened: the filter's item, the inventory sent, the
/// filter's own list and the attributes it had.
struct Session {
    item: mod_api::ItemKey,
    attribute: bool,
    inventory: Vec<(u32, ItemStack)>,
    list: Vec<Option<ItemStack>>,
    had: Vec<(String, Option<String>)>,
}

thread_local! {
    static SESSIONS: std::cell::RefCell<std::collections::HashMap<u128, Session>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// `player.openMenu(filter)`: the screen, with the held filter's settings and a copy of the
/// inventory.
fn open_screen(host: &mut dyn ServerHost, player: PlayerRef) {
    let Some(stack) = held(&*host, player, Hand::Main) else {
        return;
    };
    let attribute = match filter::FilterItem::of(&stack.item().id(&*host)) {
        Some(filter::FilterItem::List) => false,
        Some(filter::FilterItem::Attribute) => true,
        _ => return,
    };
    let h: &dyn Host = &*host;
    let inventory: Vec<(u32, ItemStack)> = player
        .inventory_slots(h)
        .unwrap_or_default()
        .into_iter()
        .filter(|(slot, _)| *slot < 36)
        .collect();
    let c = custom(&stack);
    let list = if attribute {
        Vec::new()
    } else {
        decode_items(h, c.get(&ITEMS_KEY.into()), LIST_SLOTS)
    };
    let s = spec(h, &stack);
    let had: Vec<(String, Option<String>, bool)> = s.attributes.clone().unwrap_or_default();
    let opened = filter_screen::Opened {
        attribute,
        selected: player.selected_slot(h).unwrap_or(0) as u8,
        inventory: inventory
            .iter()
            .filter_map(|(slot, st)| {
                let attrs = if attribute {
                    attributes_of(h, st)
                } else {
                    Vec::new()
                };
                Some((*slot as u8, filter_screen::stack_bytes(h, st)?, attrs))
            })
            .collect(),
        list: list
            .iter()
            .enumerate()
            .filter_map(|(i, st)| Some((i as u8, filter_screen::stack_bytes(h, st.as_ref()?)?)))
            .collect(),
        deny: s.blacklist,
        respect: s.respect_nbt,
        mode: s
            .mode
            .as_deref()
            .and_then(Mode::parse)
            .and_then(|m| Mode::ALL.iter().position(|a| *a == m))
            .unwrap_or(0) as u8,
        chosen: had
            .iter()
            .map(|(kind, value, inverted)| (attr_line(kind, value.clone()), *inverted))
            .collect(),
    };
    SESSIONS.with_borrow_mut(|m| {
        m.insert(
            player.uuid(),
            Session {
                item: stack.item(),
                attribute,
                inventory,
                list,
                had: had.into_iter().map(|(k, v, _)| (k, v)).collect(),
            },
        )
    });
    net::send_to_player(host, player, filter_screen::CHANNEL, &opened.encode());
}

/// How an attribute reads (`getTranslationKey`, `getTranslationParameters`).
fn attr_line(kind: &str, value: Option<String>) -> filter_screen::AttrLine {
    let name = kind.strip_prefix("create:").unwrap_or(kind);
    let (key, arg, arg_key) = match name {
        "in_tag" => (
            name.to_owned(),
            value
                .as_ref()
                .map(|v| format!("#{}", v.trim_start_matches('#'))),
            false,
        ),
        // `ModInfo.getDisplayName`, else the id capitalised.
        "added_by" => (
            name.to_owned(),
            value.as_deref().map(|m| match m {
                "minecraft" => "Minecraft".to_owned(),
                m => {
                    let mut c = m.chars();
                    c.next()
                        .map(|f| f.to_uppercase().chain(c).collect())
                        .unwrap_or_default()
                }
            }),
            false,
        ),
        "has_enchant" => (
            name.to_owned(),
            value.as_deref().map(|e| {
                let (ns, path) = e.split_once(':').unwrap_or(("minecraft", e));
                format!("enchantment.{ns}.{path}")
            }),
            true,
        ),
        "shulker_fill_level" => (
            "shulker_level".to_owned(),
            value
                .as_deref()
                .map(|v| format!("create.item_attributes.shulker_level.{v}")),
            true,
        ),
        "has_color" => (
            "color".to_owned(),
            value.as_deref().map(|c| format!("color.minecraft.{c}")),
            true,
        ),
        // `FluidType.getDescription`.
        "has_fluid" => (
            name.to_owned(),
            value.as_deref().map(|f| {
                create_engine::tank::Fluid::ALL
                    .into_iter()
                    .find(|x| x.id() == f)
                    .map_or_else(|| f.to_owned(), |x| x.name())
            }),
            false,
        ),
        "book_copy" => (
            match value.as_deref().and_then(|v| v.parse::<i32>().ok()) {
                Some(0) => "book_copy_original",
                Some(1) => "book_copy_first",
                Some(2) => "book_copy_second",
                _ => "book_copy_tattered",
            }
            .to_owned(),
            None,
            false,
        ),
        _ => (name.to_owned(), value.clone(), false),
    };
    filter_screen::AttrLine {
        kind: kind.to_owned(),
        value,
        key,
        arg,
        arg_key,
    }
}

/// The item tags `stack`'s item is in (`ItemStack.getTags`), among those the server's data
/// defines: what the stock keeper's `#` search looks through.
pub(crate) fn item_tags(host: &dyn Host, stack: &ItemStack) -> Vec<String> {
    let tags = ITEM_TAGS.lock().map(|t| t.clone()).unwrap_or_default();
    tags.into_iter()
        .filter(|tag| {
            Identifier::parse(tag)
                .ok()
                .and_then(|id| host.tags().key(&id))
                .and_then(|k| host.tags().is_in_tag(stack.item(), k))
                .unwrap_or(false)
        })
        .collect()
}

/// Every item tag the server's data defines, in resource order (`tags/item`), as the last data
/// reload found them.
static ITEM_TAGS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Lists the item tags of each data load, for `in_tag`.
struct ItemTagNames;

impl mod_api::ServerReloadListener for ItemTagNames {
    fn prepare(
        &self,
        context: &mod_api::ServerReloadContext,
    ) -> Result<Box<dyn mod_api::ReloadApply>, mod_api::ReloadResourceError> {
        let names: Vec<String> = context.list().map(ToString::to_string).collect();
        Ok(<dyn mod_api::ReloadApply>::new(move || {
            if let Ok(mut tags) = ITEM_TAGS.lock() {
                *tags = names;
            }
        }))
    }
}

/// Each enchantment's maximum level (`Enchantment.getMaxLevel`), by id, as the last data reload
/// defined it.
static ENCHANTMENT_MAX: std::sync::Mutex<BTreeMap<String, i32>> =
    std::sync::Mutex::new(BTreeMap::new());

/// Reads the enchantments' `max_level` with each data load.
struct EnchantmentLevels;

impl mod_api::ServerReloadListener for EnchantmentLevels {
    fn prepare(
        &self,
        context: &mod_api::ServerReloadContext,
    ) -> Result<Box<dyn mod_api::ReloadApply>, mod_api::ReloadResourceError> {
        let levels: BTreeMap<String, i32> = context
            .list()
            .filter_map(|id| {
                let text = std::str::from_utf8(context.read(id)?).ok()?;
                Some((id.to_string(), max_level(text)?))
            })
            .collect();
        Ok(<dyn mod_api::ReloadApply>::new(move || {
            if let Ok(mut m) = ENCHANTMENT_MAX.lock() {
                *m = levels;
            }
        }))
    }
}

/// An enchantment definition's `max_level` (a top-level number of the JSON object).
fn max_level(json: &str) -> Option<i32> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let bytes = json.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
        } else {
            match c {
                b'{' | b'[' => depth += 1,
                b'}' | b']' => depth -= 1,
                b'"' if depth == 1 && json[i..].starts_with("\"max_level\"") => {
                    let rest = json[i + "\"max_level\"".len()..].trim_start();
                    let rest = rest.strip_prefix(':')?.trim_start();
                    let end = rest
                        .find(|c: char| !(c.is_ascii_digit() || c == '-'))
                        .unwrap_or(rest.len());
                    return rest[..end].parse().ok();
                }
                b'"' => in_string = true,
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Registers the item tag listing and the enchantments' levels with each data load.
pub(crate) fn add_reload_listeners(listeners: &mut dyn mod_api::ServerReloadListeners) {
    if let (Ok(id), Ok(converter)) = (
        Identifier::parse("create:item_tag_names"),
        mod_api::FileToIdConverter::json("tags/item"),
    ) {
        let _ = listeners.add_listener(id, converter, std::sync::Arc::new(ItemTagNames));
    }
    if let (Ok(id), Ok(converter)) = (
        Identifier::parse("create:enchantment_levels"),
        mod_api::FileToIdConverter::json("enchantment"),
    ) {
        let _ = listeners.add_listener(id, converter, std::sync::Arc::new(EnchantmentLevels));
    }
}

/// `ItemAttribute.getAllAttributes` of each type, in Create's registration order (the types
/// this port reads).
fn attributes_of(host: &dyn Host, stack: &ItemStack) -> Vec<filter_screen::AttrLine> {
    const SINGLETONS: [&str; 18] = [
        "placeable",
        "consumable",
        "fluid_container",
        "enchanted",
        "max_enchanted",
        "renamed",
        "damaged",
        "badly_damaged",
        "not_stackable",
        "equipable",
        "furnace_fuel",
        "washable",
        "hauntable",
        "crushable",
        "smeltable",
        "smokable",
        "blastable",
        "compostable",
    ];
    let looked = Looked::new(host, stack);
    let mut out: Vec<filter_screen::AttrLine> = SINGLETONS
        .iter()
        .filter(|n| Attribute::parse(n, None).is_ok_and(|a| looked.attribute(&a)))
        .map(|n| attr_line(&format!("create:{n}"), None))
        .collect();
    // `InTagAttribute.Type.getAllAttributes`: the tags the item is in.
    let tags = ITEM_TAGS.lock().map(|t| t.clone()).unwrap_or_default();
    for tag in tags {
        let inside = Identifier::parse(&tag)
            .ok()
            .and_then(|id| host.tags().key(&id))
            .and_then(|k| host.tags().is_in_tag(stack.item(), k))
            .unwrap_or(false);
        if inside {
            out.push(attr_line("create:in_tag", Some(tag)));
        }
    }
    let id = looked.id.clone();
    out.push(attr_line(
        "create:added_by",
        Some(id.split_once(':').map_or("minecraft", |s| s.0).to_owned()),
    ));
    let which = if id == "minecraft:enchanted_book" {
        "minecraft:stored_enchantments"
    } else {
        "minecraft:enchantments"
    };
    for e in enchantment_ids(component(host, stack, which)) {
        out.push(attr_line("create:has_enchant", Some(e)));
    }
    if let Some(size) = shulker_level(host, stack, &id) {
        for l in ["empty", "partial", "full"] {
            if shulker_level_holds(l, size) {
                out.push(attr_line("create:shulker_fill_level", Some(l.to_owned())));
            }
        }
    }
    for c in colors_of(host, stack, &id) {
        out.push(attr_line("create:has_color", Some(c)));
    }
    if let Some(f) = held_fluid(&id) {
        out.push(attr_line("create:has_fluid", Some(f)));
    }
    let renamed = stack
        .components()
        .is_some_and(|c| c.as_nbt().contains_key(&"minecraft:custom_name".into()));
    if renamed
        && let Some(name) =
            plain_name(component(host, stack, "minecraft:custom_name")).filter(|n| !n.is_empty())
    {
        out.push(attr_line("create:has_name", Some(name)));
    }
    if let Some(Tag::Compound(w)) = component(host, stack, "minecraft:written_book_content") {
        if let Some(author) = text(w.get(&"author".into())).filter(|a| !a.is_empty()) {
            out.push(attr_line("create:book_author", Some(author)));
        }
        let generation = match w.get(&"generation".into()) {
            Some(Tag::Int(i)) => *i,
            _ => 0,
        };
        out.push(attr_line("create:book_copy", Some(generation.to_string())));
    }
    out
}

/// What the screen chose, written into the held filter (`FilterScreenPacket` and `saveData`):
/// only while the main hand still holds the filter it opened for (`stillValid`).
pub(super) fn client_message(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let Some(chosen) = filter_screen::Chosen::decode(data) else {
        return;
    };
    let Some(session) = SESSIONS.with_borrow_mut(|m| m.remove(&player.uuid())) else {
        return;
    };
    let Some(mut stack) = held(&*host, player, Hand::Main) else {
        return;
    };
    if stack.item() != session.item || chosen.attribute != session.attribute {
        return;
    }
    if session.attribute {
        let mode = Mode::ALL
            .get(usize::from(chosen.mode))
            .copied()
            .unwrap_or(Mode::WhitelistDisj);
        // The attributes offered (or kept from before): nothing else is written.
        let tests: Vec<(String, Option<String>, bool)> = chosen
            .attributes
            .into_iter()
            .filter(|(kind, value, _)| {
                session.had.iter().any(|(k, v)| k == kind && v == value)
                    || Attribute::parse(kind, value.as_deref()).is_ok()
            })
            .collect();
        write_attributes(&mut stack, mode, &tests);
    } else {
        let h: &dyn Host = &*host;
        let slots: Vec<Option<ItemStack>> = (0..LIST_SLOTS)
            .map(|i| {
                let mut s = match chosen.list.get(i).copied().flatten()? {
                    filter_screen::Ghost::Inventory(slot) => session
                        .inventory
                        .iter()
                        .find(|(s, _)| *s == u32::from(slot))
                        .map(|(_, st)| st.clone()),
                    filter_screen::Ghost::Own(j) => {
                        session.list.get(usize::from(j)).cloned().flatten()
                    }
                }?;
                let _ = s.set_count(1);
                Some(s)
            })
            .collect();
        let list = slots
            .iter()
            .any(Option::is_some)
            .then(|| encode_items(h, &slots));
        set_custom(
            &mut stack,
            &[
                (ITEMS_KEY, list),
                (RESPECT_NBT_KEY, Some(Tag::Byte(i8::from(chosen.respect)))),
                (BLACKLIST_KEY, Some(Tag::Byte(i8::from(chosen.deny)))),
            ],
        );
    }
    set_held(host, player, Hand::Main, Some(stack));
}

#[cfg(test)]
mod enchantment_tests {
    #[test]
    fn an_enchantments_max_level_is_its_top_level_field() {
        let sharpness = r##"{"description":{"translate":"enchantment.minecraft.sharpness"},
            "effects":{"minecraft:damage":[{"effect":{"type":"minecraft:add",
            "value":{"type":"minecraft:linear","base":1.0,"per_level_above_first":0.5}}}]},
            "exclusive_set":"#minecraft:exclusive_set/damage","max_level":5,"weight":10}"##;
        assert_eq!(super::max_level(sharpness), Some(5));
        // A nested `max_level` is not the enchantment's.
        assert_eq!(
            super::max_level(r##"{"a":{"max_level":3},"max_level": 1}"##),
            Some(1)
        );
        assert_eq!(super::max_level(r##"{"text":"\"max_level\": 9"}"##), None);
    }
}
