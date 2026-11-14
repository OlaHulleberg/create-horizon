//! Filters (`FilterItemStack`, `create-engine`'s `filter`): what a filter slot's stack is, a
//! stack tested against it, a filter taken from a hand into a slot and given back
//! (`FilteringBehaviour.onShortInteract`), the filter items of broken blocks dropped
//! (`FilteringBehaviour.destroy`), and `/create_debug filter` standing in for the filters'
//! screens. A filter's settings are kept in the stack's custom data under Create's component
//! names, its list as `ItemContainerContents` (`Slot` and the stack's own NBT).
//!
//! Left out: the attributes that read a recipe book (`washable`, `hauntable`, `crushable`,
//! `smeltable`, `smokable`, `blastable`), which are the processing port's (its recipes), and
//! `max_enchanted`, which needs each enchantment's maximum level (the host answers it on the
//! client only, `ItemsClientHost::enchantments`): those test false.

use super::super::server::ServerLevel;
use super::super::world::with_world;
use super::{creative, custom, give_back, held, set_custom, set_held, set_slot, slot_stack};
use create_engine::filter::{
    self, ATTRIBUTES_KEY, Attribute, BLACKLIST_KEY, Filter, ITEMS_KEY, LIST_SLOTS, MODE_KEY, Mode,
    Passes, Probe, RESPECT_NBT_KEY, Spec,
};
use mod_api::{
    CommandArgumentValue, CommandSource, Compound, Hand, Host, Identifier, ItemStack, PlayerRef,
    RunModCommandEvent, ServerHost, SoundSource, Tag,
};
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
fn component(host: &dyn Host, stack: &ItemStack, id: &str) -> Option<Tag> {
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
fn plain_name(tag: Option<Tag>) -> Option<String> {
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
            // The recipe attributes and `max_enchanted`: see the module's doc.
            _ => false,
        }
    }
}

/// `FilteringBehaviour.test(stack)`: anything with no filter.
pub(crate) fn passes(host: &dyn Host, filter: Option<&Filter>, stack: &ItemStack) -> bool {
    filter.is_none_or(|f| f.test(&Looked::new(host, stack)))
}

/// The same for an item without components (on a belt, a block's item).
pub(crate) fn passes_item(host: &dyn Host, filter: Option<&Filter>, item: &str) -> bool {
    let Some(f) = filter else {
        return true;
    };
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

/// The attribute filter's screen: its mode and attributes (`ItemAttributeEntry`'s codec:
/// `{attribute: {type, value}, inverted}`).
fn set_attributes(stack: &mut ItemStack, mode: Mode, tests: &[(Attribute, bool)]) {
    let list = tests
        .iter()
        .map(|(a, inverted)| {
            let mut attribute = Compound::new();
            attribute.insert("type".into(), Tag::String(a.id().as_str().into()));
            match a {
                Attribute::BookCopy(g) => {
                    attribute.insert("value".into(), Tag::Int(*g));
                }
                _ => {
                    if let Some(v) = a.value() {
                        attribute.insert("value".into(), Tag::String(v.as_str().into()));
                    }
                }
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
