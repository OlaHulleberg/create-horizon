// What Create's handheld items read and write of a stack and a player beyond the hook helpers:
// the filters', the wand of symmetry's, the worldshaper's and the schedule's settings in
// `minecraft:custom_data` under Create's component names (Horizon adds no component types), a
// stack's components as text, the item facts the attribute filter reads, and a stack given back
// to the player. Compiled against Horizon by `horizon.py check`; the template test swaps it for
// `tools/fixture/items_body.rs`.
use super::Game;
use create_engine::filter::{
    ATTRIBUTES_KEY, Attribute, BLACKLIST_KEY, ITEMS_KEY, LIST_SLOTS, MODE_KEY, Mode,
    RESPECT_NBT_KEY, Spec,
};
use create_engine::symmetry::{ENABLE_KEY, Kind, MIRROR_KEY, Wand};
use create_engine::worldshaper as ws;
use horizon_client::items::ItemStack;
use horizon_world::nbt::{Compound, JavaString, Tag};

const CUSTOM_DATA: &str = "minecraft:custom_data";
const SCHEDULE_KEY: &str = "create:train_schedule";

fn custom(stack: &ItemStack) -> Option<&Compound> {
    match stack.data().and_then(|d| d.other(CUSTOM_DATA)) {
        Some(Tag::Compound(c)) => Some(c),
        _ => None,
    }
}

fn get<'a>(c: Option<&'a Compound>, key: &str) -> Option<&'a Tag> {
    c?.get(&JavaString::from(key))
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

fn int(t: Option<&Tag>) -> Option<i32> {
    match t {
        Some(Tag::Int(i)) => Some(*i),
        _ => None,
    }
}

fn set_custom(stack: &mut ItemStack, entries: Vec<(&str, Option<Tag>)>) {
    let mut c = custom(stack).cloned().unwrap_or_default();
    for (key, value) in entries {
        match value {
            Some(v) => {
                c.insert(key.into(), v);
            }
            None => {
                c.remove(&JavaString::from(key));
            }
        }
    }
    let data = stack.data_mut();
    data.set_other(CUSTOM_DATA, (!c.is_empty()).then_some(Tag::Compound(c)));
    stack.tidy();
}

/// The stack's components as SNBT (`isSameItemSameComponents`), "" when it has none.
pub(super) fn components(stack: &ItemStack) -> String {
    match horizon_items::item_nbt::item_to_nbt(stack).get("components") {
        Some(t) => horizon_world::snbt_text::to_snbt(t),
        None => String::new(),
    }
}

/// What a filter slot's stack is (`FilterItemStack.of` reads it).
pub(super) fn spec(stack: &ItemStack) -> Spec {
    let c = custom(stack);
    let list = get(c, ITEMS_KEY);
    Spec {
        item: stack.item.to_string(),
        components: components(stack),
        has_list: list.is_some(),
        list: horizon_items::item_nbt::decode_items(list, LIST_SLOTS)
            .into_iter()
            .flatten()
            .map(|s| spec(&s))
            .collect(),
        respect_nbt: flag(get(c, RESPECT_NBT_KEY)),
        blacklist: flag(get(c, BLACKLIST_KEY)),
        mode: text(get(c, MODE_KEY)),
        attributes: match get(c, ATTRIBUTES_KEY) {
            Some(Tag::List(l)) => Some(
                l.iter()
                    .filter_map(|e| {
                        let Tag::Compound(e) = e else { return None };
                        let a = match e.get(&JavaString::from("attribute")) {
                            Some(Tag::Compound(a)) => a,
                            _ => return None,
                        };
                        let kind = text(a.get(&JavaString::from("type")))?;
                        let value = text(a.get(&JavaString::from("value")));
                        Some((kind, value, flag(e.get(&JavaString::from("inverted")))))
                    })
                    .collect(),
            ),
            _ => None,
        },
        address: super::create_package_body::address(stack),
    }
}

/// The list filter's screen: its items (one each), allow or deny, respecting components.
pub(super) fn set_list(stack: &mut ItemStack, items: &[ItemStack], respect_nbt: bool, deny: bool) {
    let mut slots: Vec<Option<ItemStack>> = items.iter().cloned().map(Some).collect();
    slots.truncate(LIST_SLOTS);
    set_custom(
        stack,
        vec![
            (
                ITEMS_KEY,
                Some(horizon_items::item_nbt::encode_items(&slots)),
            ),
            (RESPECT_NBT_KEY, Some(Tag::Byte(i8::from(respect_nbt)))),
            (BLACKLIST_KEY, Some(Tag::Byte(i8::from(deny)))),
        ],
    );
}

/// The attribute filter's screen: its mode and attributes (`ItemAttributeEntry`'s codec:
/// `{attribute: {type, value}, inverted}`).
pub(super) fn set_attributes(stack: &mut ItemStack, mode: Mode, tests: &[(Attribute, bool)]) {
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
        vec![
            (MODE_KEY, Some(Tag::String(mode.name().into()))),
            (ATTRIBUTES_KEY, Some(Tag::List(list))),
        ],
    );
}

/// The item facts an attribute reads (`AllItemAttributeTypes`' singletons and the valued ones
/// on the stack). Recipe attributes are the glue's.
pub(super) fn attribute(game: &Game<'_>, stack: &ItemStack, a: &Attribute) -> bool {
    let items = game.items();
    let info = items.get(&stack.item);
    let data = stack.data();
    match a {
        Attribute::Placeable => info.is_some_and(|i| i.block.is_some()),
        Attribute::Consumable => info.is_some_and(|i| i.food.is_some()),
        Attribute::Enchanted => !stack.enchantments().is_empty(),
        Attribute::MaxEnchanted => {
            let reg = &items.enchantments;
            stack
                .enchantments()
                .iter()
                .any(|(e, l)| reg.get(e).is_some_and(|e| e.max_level <= *l))
        }
        Attribute::Renamed => data.is_some_and(|d| d.custom_name.is_some()),
        Attribute::Damaged => info.is_some_and(|i| i.max_damage > 0) && stack.damage > 0,
        Attribute::BadlyDamaged => {
            create_engine::filter::badly_damaged(stack.damage, info.map_or(0, |i| i.max_damage))
        }
        Attribute::NotStackable => info.is_none_or(|i| {
            create_engine::filter::not_stackable(i.max_stack, i.max_damage, stack.damage)
        }),
        Attribute::Equipable => info
            .and_then(|i| i.equip_slot.as_deref())
            .is_some_and(|s| s != "mainhand" && s != "offhand"),
        Attribute::FurnaceFuel => game.res.providers().burn_time(items, stack, false) > 0,
        Attribute::Compostable => info.is_some_and(|i| i.compostable.is_some()),
        Attribute::InTag(tag) => items
            .item_tags
            .get(tag.as_str())
            .is_some_and(|t| t.contains(&*stack.item)),
        Attribute::AddedBy(m) => stack.item.split_once(':').map_or("minecraft", |s| s.0) == m,
        // `getEnchantmentsForCrafting`: a book's stored ones.
        Attribute::HasEnchant(e) => {
            let list = if &*stack.item == "minecraft:enchanted_book" {
                data.map(|d| d.stored_enchantments.as_slice())
                    .unwrap_or_default()
            } else {
                stack.enchantments()
            };
            list.iter().any(|(id, _)| **id == **e)
        }
        Attribute::HasName(n) => data
            .and_then(|d| d.custom_name_plain())
            .is_some_and(|name| name == *n),
        Attribute::BookAuthor(author) => {
            data.and_then(|d| d.written.as_ref())
                .map_or("", |w| w.author.as_str())
                == author
        }
        Attribute::BookCopy(g) => {
            data.and_then(|d| d.written.as_ref())
                .map_or(-1, |w| i32::from(w.generation))
                == *g
        }
        // The recipe attributes are answered by the glue.
        _ => false,
    }
}

/// The wand's mirror and its `SYMMETRY_WAND_ENABLE` (`checkComponents`: an empty mirror at the
/// origin, disabled, when it has none).
pub(super) fn wand(stack: &ItemStack) -> Wand {
    let c = custom(stack);
    let Some(Tag::Compound(m)) = get(c, MIRROR_KEY) else {
        return Wand::blank();
    };
    let position = match m.get(&JavaString::from("position")) {
        Some(Tag::List(v)) if v.len() == 3 => {
            let f = |t: &Tag| match t {
                Tag::Double(b) => f64::from_bits(*b),
                _ => 0.0,
            };
            [f(&v[0]), f(&v[1]), f(&v[2])]
        }
        _ => [0.0; 3],
    };
    let mut w = Wand {
        kind: Kind::parse(&text(m.get(&JavaString::from("type"))).unwrap_or_default()),
        orientation: 0,
        position,
        mirror_enable: flag(m.get(&JavaString::from("enable"))),
        enabled: flag(get(c, ENABLE_KEY)),
    };
    w.set_orientation(
        int(m.get(&JavaString::from("orientation_index")))
            .unwrap_or(0)
            .max(0) as usize,
    );
    w
}

pub(super) fn set_wand(stack: &mut ItemStack, w: &Wand) {
    let mut m = Compound::new();
    m.insert("orientation_index".into(), Tag::Int(w.orientation as i32));
    m.insert(
        "position".into(),
        Tag::List(
            w.position
                .iter()
                .map(|v| Tag::Double(v.to_bits()))
                .collect(),
        ),
    );
    m.insert("type".into(), Tag::String(w.kind.name().into()));
    m.insert("enable".into(), Tag::Byte(i8::from(w.mirror_enable)));
    set_custom(
        stack,
        vec![
            (MIRROR_KEY, Some(Tag::Compound(m))),
            (ENABLE_KEY, Some(Tag::Byte(i8::from(w.enabled)))),
        ],
    );
}

/// The worldshaper's settings and the block it uses (`SHAPER_BLOCK_USED` as a state's text).
pub(super) fn shaper(stack: &ItemStack) -> (ws::Settings, Option<String>) {
    let c = custom(stack);
    let name = |k: &str| text(get(c, k)).unwrap_or_default();
    let settings = ws::Settings {
        brush: ws::Brush::parse(&name(ws::BRUSH_KEY)).unwrap_or_default(),
        params: match get(c, ws::PARAMS_KEY) {
            Some(Tag::IntArray(v)) if v.len() == 3 => Some([v[0], v[1], v[2]]),
            _ => None,
        },
        tool: ws::Tool::parse(&name(ws::TOOL_KEY)).unwrap_or_default(),
        placement: ws::Placement::parse(&name(ws::PLACEMENT_KEY)).unwrap_or_default(),
        pattern: ws::Pattern::parse(&name(ws::PATTERN_KEY)).unwrap_or_default(),
    };
    (settings, text(get(c, ws::BLOCK_KEY)))
}

/// `WorldshaperItem.configureSettings` (and the block, when given: `trySelect`).
pub(super) fn set_shaper(stack: &mut ItemStack, s: &ws::Settings, block: Option<&str>) {
    let mut entries = vec![
        (ws::BRUSH_KEY, Some(Tag::String(s.brush.name().into()))),
        (ws::PARAMS_KEY, s.params.map(|p| Tag::IntArray(p.to_vec()))),
        (ws::TOOL_KEY, Some(Tag::String(s.tool.name().into()))),
        (
            ws::PLACEMENT_KEY,
            Some(Tag::String(s.placement.name().into())),
        ),
        (ws::PATTERN_KEY, Some(Tag::String(s.pattern.name().into()))),
    ];
    if let Some(b) = block {
        entries.push((ws::BLOCK_KEY, Some(Tag::String(b.into()))));
    }
    set_custom(stack, entries);
}

/// The schedule written on a schedule item (`TRAIN_SCHEDULE`, `Schedule.write`): its stations
/// and seconds to wait, None when it has none.
pub(super) fn schedule(stack: &ItemStack) -> Option<Vec<(String, i32)>> {
    let Some(Tag::Compound(s)) = get(custom(stack), SCHEDULE_KEY) else {
        return None;
    };
    let Some(Tag::List(entries)) = s.get(&JavaString::from("Entries")) else {
        return Some(Vec::new());
    };
    let field = |c: &Compound, k: &str| c.get(&JavaString::from(k)).cloned();
    Some(
        entries
            .iter()
            .filter_map(|e| {
                let Tag::Compound(e) = e else { return None };
                let Some(Tag::Compound(i)) = field(e, "Instruction") else {
                    return None;
                };
                let Some(Tag::Compound(d)) = field(&i, "Data") else {
                    return None;
                };
                let station = text(d.get(&JavaString::from("Text")))?;
                // The first condition's delay (seconds), 5 by default.
                let wait = match field(e, "Conditions") {
                    Some(Tag::List(outer)) => outer.iter().find_map(|o| match o {
                        Tag::List(inner) => inner.iter().find_map(|c| match c {
                            Tag::Compound(c) => match c.get(&JavaString::from("Data")) {
                                Some(Tag::Compound(d)) => int(d.get(&JavaString::from("Value"))),
                                _ => None,
                            },
                            _ => None,
                        }),
                        _ => None,
                    }),
                    _ => None,
                };
                Some((station, wait.unwrap_or(5)))
            })
            .collect(),
    )
}

/// The schedule's screen: `create:destination` entries each waiting `create:delay` seconds
/// (`TimeUnit.SECONDS`), cyclic.
pub(super) fn set_schedule(stack: &mut ItemStack, entries: &[(String, i32)]) {
    let list = entries
        .iter()
        .map(|(station, wait)| {
            let instruction = Tag::compound([
                ("Id", Tag::String("create:destination".into())),
                (
                    "Data",
                    Tag::compound([("Text", Tag::String(station.as_str().into()))]),
                ),
            ]);
            let delay = Tag::compound([
                ("Id", Tag::String("create:delay".into())),
                (
                    "Data",
                    Tag::compound([("Value", Tag::Int(*wait)), ("TimeUnit", Tag::Int(1))]),
                ),
            ]);
            Tag::compound([
                ("Instruction", instruction),
                ("Conditions", Tag::List(vec![Tag::List(vec![delay])])),
            ])
        })
        .collect();
    let schedule = Tag::compound([("Entries", Tag::List(list)), ("Cyclic", Tag::Byte(1))]);
    set_custom(stack, vec![(SCHEDULE_KEY, Some(schedule))]);
}

/// The belt connector's first shaft (`BELT_FIRST_SHAFT`, a `BlockPos`: three ints).
pub(super) fn belt_first_shaft(stack: &ItemStack) -> Option<[i32; 3]> {
    match get(
        custom(stack),
        create_engine::belt_connector::FIRST_SHAFT_KEY,
    ) {
        Some(Tag::IntArray(v)) if v.len() == 3 => Some([v[0], v[1], v[2]]),
        _ => None,
    }
}

/// `set` (or `remove`, for None) of `BELT_FIRST_SHAFT`.
pub(super) fn set_belt_first_shaft(stack: &mut ItemStack, pos: Option<[i32; 3]>) {
    set_custom(
        stack,
        vec![(
            create_engine::belt_connector::FIRST_SHAFT_KEY,
            pos.map(|p| Tag::IntArray(p.to_vec())),
        )],
    );
}

/// A linked controller's frequency items (`LINKED_CONTROLLER_ITEMS`, two a button), by id; ""
/// for an empty slot.
pub(super) fn controller_items(stack: &ItemStack) -> Vec<String> {
    match get(custom(stack), create_engine::controller::ITEMS_KEY) {
        Some(Tag::List(l)) => l
            .iter()
            .map(|t| text(Some(t)).unwrap_or_default())
            .collect(),
        _ => Vec::new(),
    }
}

/// `set(LINKED_CONTROLLER_ITEMS)`: none when every slot is empty.
pub(super) fn set_controller_items(stack: &mut ItemStack, items: &[String]) {
    let value = items.iter().any(|i| !i.is_empty()).then(|| {
        Tag::List(
            items
                .iter()
                .map(|i| Tag::String(i.as_str().into()))
                .collect(),
        )
    });
    set_custom(stack, vec![(create_engine::controller::ITEMS_KEY, value)]);
}

/// `Inventory.placeItemBackInInventory`: into the inventory, the rest dropped at the player.
pub(super) fn give_back(game: &mut Game<'_>, stack: ItemStack) {
    if let Some(rest) = game.give(stack)
        && let Some(p) = game.native_acting_player()
    {
        game.spawn_item(rest, p.position, [0.0; 3], 0);
    }
}
