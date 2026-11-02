//! Create's filters (`FilterItemStack`, `ListFilterItemStack`, `AttributeFilterItemStack`,
//! `PackageFilterItemStack`, `FilterItem.testDirect`, `AllItemAttributeTypes`): what a filter
//! slot holds, compiled once from the stack put there into a typed matcher, then tested against
//! items without reading the stack again.
//!
//! The filter items keep their settings as Create's data components do, in the stack's
//! `minecraft:custom_data` under Create's component names (Horizon adds no component types):
//! `create:filter_items` (the list, as `ItemContainerContents`), `create:filter_items_respect_nbt`,
//! `create:filter_items_blacklist`, `create:attribute_filter_whitelist_mode`,
//! `create:attribute_filter_matched_attributes` and `create:package_address`. The glue reads
//! them into a `Spec`; `Filter::compile` turns that into the matcher.

use crate::tank::Fluid;
use std::any::Any;
use std::rc::Rc;

pub const FILTER: &str = "create:filter";
pub const ATTRIBUTE_FILTER: &str = "create:attribute_filter";
pub const PACKAGE_FILTER: &str = "create:package_filter";
/// The custom data keys (Create's component ids).
pub const ITEMS_KEY: &str = "create:filter_items";
pub const RESPECT_NBT_KEY: &str = "create:filter_items_respect_nbt";
pub const BLACKLIST_KEY: &str = "create:filter_items_blacklist";
pub const MODE_KEY: &str = "create:attribute_filter_whitelist_mode";
pub const ATTRIBUTES_KEY: &str = "create:attribute_filter_matched_attributes";
/// `ListFilterItem.getFilterItemHandler`: 18 slots.
pub const LIST_SLOTS: usize = 18;

/// Which of Create's filter items an item is (`instanceof FilterItem`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterItem {
    List,
    Attribute,
    Package,
}

impl FilterItem {
    pub fn of(item: &str) -> Option<FilterItem> {
        match item {
            FILTER => Some(FilterItem::List),
            ATTRIBUTE_FILTER => Some(FilterItem::Attribute),
            PACKAGE_FILTER => Some(FilterItem::Package),
            _ => None,
        }
    }
}

/// `AttributeFilterWhitelistMode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    WhitelistDisj,
    WhitelistConj,
    Blacklist,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::WhitelistDisj, Mode::WhitelistConj, Mode::Blacklist];
    /// `getSerializedName` (`Lang.asId(name())`).
    pub fn name(self) -> &'static str {
        match self {
            Mode::WhitelistDisj => "whitelist_disj",
            Mode::WhitelistConj => "whitelist_conj",
            Mode::Blacklist => "blacklist",
        }
    }
    pub fn parse(s: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.name() == s)
    }
}

/// The item attributes of `AllItemAttributeTypes` the port can evaluate. Left out:
/// `fluid_container` (an item fluid handler capability, gap 4), `in_item_group` (creative tabs),
/// `shulker_fill_level`, `has_color` and `has_fluid` (their readers of dyes, banners and fluid
/// handlers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attribute {
    Placeable,
    Consumable,
    Enchanted,
    MaxEnchanted,
    Renamed,
    Damaged,
    BadlyDamaged,
    NotStackable,
    Equipable,
    FurnaceFuel,
    Washable,
    Hauntable,
    Crushable,
    Smeltable,
    Smokable,
    Blastable,
    Compostable,
    /// `InTagAttribute`: an item tag, without its `#`.
    InTag(String),
    /// `AddedByAttribute`: a mod id (the item id's namespace, `getCreatorModId`).
    AddedBy(String),
    /// `EnchantAttribute`: an enchantment id.
    HasEnchant(String),
    /// `ItemNameAttribute`: the custom name's text.
    HasName(String),
    /// `BookAuthorAttribute`.
    BookAuthor(String),
    /// `BookCopyAttribute`: the generation (0 original ... 3 tattered).
    BookCopy(i32),
}

const SINGLETONS: [(&str, Attribute); 17] = [
    ("placeable", Attribute::Placeable),
    ("consumable", Attribute::Consumable),
    ("enchanted", Attribute::Enchanted),
    ("max_enchanted", Attribute::MaxEnchanted),
    ("renamed", Attribute::Renamed),
    ("damaged", Attribute::Damaged),
    ("badly_damaged", Attribute::BadlyDamaged),
    ("not_stackable", Attribute::NotStackable),
    ("equipable", Attribute::Equipable),
    ("furnace_fuel", Attribute::FurnaceFuel),
    ("washable", Attribute::Washable),
    ("hauntable", Attribute::Hauntable),
    ("crushable", Attribute::Crushable),
    ("smeltable", Attribute::Smeltable),
    ("smokable", Attribute::Smokable),
    ("blastable", Attribute::Blastable),
    ("compostable", Attribute::Compostable),
];

/// Why an attribute was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Create has it, the port can't evaluate it.
    NotPorted(String),
    /// Create has no such attribute type, or its value is missing or bad.
    Unknown(String),
}

impl Attribute {
    /// The attribute type's id (`create:in_tag`).
    pub fn id(&self) -> String {
        let name = match self {
            Attribute::InTag(_) => "in_tag",
            Attribute::AddedBy(_) => "added_by",
            Attribute::HasEnchant(_) => "has_enchant",
            Attribute::HasName(_) => "has_name",
            Attribute::BookAuthor(_) => "book_author",
            Attribute::BookCopy(_) => "book_copy",
            s => SINGLETONS
                .iter()
                .find(|(_, a)| a == s)
                .map_or("", |(n, _)| n),
        };
        format!("create:{name}")
    }

    /// The codec's `value` field (none for a singleton).
    pub fn value(&self) -> Option<String> {
        match self {
            Attribute::InTag(v)
            | Attribute::AddedBy(v)
            | Attribute::HasEnchant(v)
            | Attribute::HasName(v)
            | Attribute::BookAuthor(v) => Some(v.clone()),
            Attribute::BookCopy(g) => Some(g.to_string()),
            _ => None,
        }
    }

    /// From the dispatch codec's `type` (with or without `create:`) and `value`.
    pub fn parse(kind: &str, value: Option<&str>) -> Result<Attribute, Refused> {
        let name = kind.strip_prefix("create:").unwrap_or(kind);
        if let Some((_, a)) = SINGLETONS.iter().find(|(n, _)| *n == name) {
            return Ok(a.clone());
        }
        let text = || {
            value
                .map(str::to_owned)
                .ok_or_else(|| Refused::Unknown(format!("{name} needs a value")))
        };
        Ok(match name {
            "in_tag" => Attribute::InTag(text()?.trim_start_matches('#').to_owned()),
            "added_by" => Attribute::AddedBy(text()?),
            "has_enchant" => Attribute::HasEnchant(text()?),
            "has_name" => Attribute::HasName(text()?),
            "book_author" => Attribute::BookAuthor(text()?),
            "book_copy" => match text()?.parse::<i32>() {
                Ok(g) if g >= 0 => Attribute::BookCopy(g),
                _ => return Err(Refused::Unknown("book_copy needs a generation".into())),
            },
            "fluid_container" | "in_item_group" | "shulker_fill_level" | "has_color"
            | "has_fluid" => return Err(Refused::NotPorted(name.to_owned())),
            _ => return Err(Refused::Unknown(name.to_owned())),
        })
    }
}

/// `BADLY_DAMAGED`: damaged past three quarters of its durability.
pub fn badly_damaged(damage: u32, max_damage: u32) -> bool {
    damage > 0 && max_damage > 0 && damage as f32 / max_damage as f32 > 3.0 / 4.0
}

/// `ItemStack.isStackable` negated (`NOT_STACKABLE`): one to a stack, or a damaged damageable
/// item.
pub fn not_stackable(max_stack: u32, max_damage: u32, damage: u32) -> bool {
    !(max_stack > 1 && (max_damage == 0 || damage == 0))
}

/// What a filter asks of a stack; the glue answers from the stack and the game's tables.
pub trait Probe {
    fn item(&self) -> &str;
    /// The stack's components as one canonical text (`isSameItemSameComponents`), asked only
    /// when the items match and a list filter respects them.
    fn components(&self) -> String {
        String::new()
    }
    /// A package's address (`PackageItem.getAddress`), "" when none.
    fn address(&self) -> String {
        String::new()
    }
    /// `ItemAttribute.appliesTo`.
    fn attribute(&self, attribute: &Attribute) -> bool {
        let _ = attribute;
        false
    }
}

/// An item with no components (an item on a belt, a block's item).
impl Probe for str {
    fn item(&self) -> &str {
        self
    }
}

/// What a filter slot's stack is, as the glue reads it from the stack.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spec {
    pub item: String,
    /// The stack's components as canonical text (empty when it has none:
    /// `isComponentsPatchEmpty`).
    pub components: String,
    /// `create:filter_items` is present (`filter.has(FILTER_ITEMS)`); its stacks (empty slots
    /// left out).
    pub has_list: bool,
    pub list: Vec<Spec>,
    pub respect_nbt: bool,
    pub blacklist: bool,
    /// `ATTRIBUTE_FILTER_WHITELIST_MODE` as saved (None: absent).
    pub mode: Option<String>,
    /// `ATTRIBUTE_FILTER_MATCHED_ATTRIBUTES` (None: absent): each (type, value, inverted).
    pub attributes: Option<Vec<(String, Option<String>, bool)>>,
    pub address: String,
}

impl Spec {
    /// A plain item.
    pub fn item(item: &str) -> Spec {
        Spec {
            item: item.to_owned(),
            ..Spec::default()
        }
    }
}

/// The compiled test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `FilterItemStack`: any other stack (a blank filter item too, which passes only itself).
    Item {
        item: Box<str>,
        components: Box<str>,
    },
    /// `ListFilterItemStack`.
    List {
        entries: Vec<Filter>,
        respect_nbt: bool,
        blacklist: bool,
    },
    /// `AttributeFilterItemStack`.
    Attributes {
        mode: Mode,
        tests: Vec<(Attribute, bool)>,
    },
    /// `PackageFilterItemStack`.
    Package { address: Box<str> },
}

/// What a filter slot holds: the compiled test and the stack it came from (the glue's own
/// stack, kept for saving and for giving it back; the engine never looks inside it).
#[derive(Clone)]
pub struct Filter {
    pub kind: Kind,
    pub source: Option<Rc<dyn Any>>,
}

impl std::fmt::Debug for Filter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.kind.fmt(f)
    }
}

impl PartialEq for Filter {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Eq for Filter {}

impl From<&str> for Filter {
    fn from(item: &str) -> Filter {
        Filter::item(item)
    }
}

impl Filter {
    /// A plain item id (a filter saved before filter items, or set from a plain stack).
    pub fn item(item: &str) -> Filter {
        Filter::compile(&Spec::item(item))
    }

    /// `FilterItemStack.of`: a filter item with components becomes its typed filter; anything
    /// else (a blank filter item included) tests its item.
    pub fn compile(spec: &Spec) -> Filter {
        let typed = if spec.components.is_empty() {
            None
        } else {
            FilterItem::of(&spec.item)
        };
        let kind = match typed {
            None => Kind::Item {
                item: spec.item.as_str().into(),
                components: spec.components.as_str().into(),
            },
            Some(FilterItem::List) => Kind::List {
                entries: spec
                    .list
                    .iter()
                    .filter(|s| !s.item.is_empty() && s.item != "minecraft:air")
                    .map(Filter::compile)
                    .collect(),
                respect_nbt: spec.has_list && spec.respect_nbt,
                blacklist: spec.has_list && spec.blacklist,
            },
            Some(FilterItem::Attribute) => Kind::Attributes {
                mode: spec
                    .mode
                    .as_deref()
                    .and_then(Mode::parse)
                    .unwrap_or_default(),
                // An attribute the codec can't read decodes to null and is left out.
                tests: spec
                    .attributes
                    .iter()
                    .flatten()
                    .filter_map(|(t, v, inv)| Some((Attribute::parse(t, v.as_deref()).ok()?, *inv)))
                    .collect(),
            },
            Some(FilterItem::Package) => Kind::Package {
                address: spec.address.as_str().into(),
            },
        };
        Filter { kind, source: None }
    }

    /// The same filter keeping the stack it came from.
    pub fn with_source(mut self, source: Rc<dyn Any>) -> Filter {
        self.source = Some(source);
        self
    }

    /// The filter stack's own stack, if the glue kept it.
    pub fn source<T: 'static>(&self) -> Option<&T> {
        self.source.as_deref()?.downcast_ref()
    }

    /// `isFilterItem`: a filter item (given back when replaced or broken).
    pub fn is_filter_item(&self) -> bool {
        match &self.kind {
            Kind::Item { item, .. } => FilterItem::of(item).is_some(),
            _ => true,
        }
    }

    /// A line for debug output: a plain item's id, or the filter's settings
    /// (`create:filter[deny: minecraft:dirt]`).
    pub fn describe(&self) -> String {
        match &self.kind {
            Kind::Item { item, .. } => item.to_string(),
            Kind::List {
                entries,
                respect_nbt,
                blacklist,
            } => format!(
                "{FILTER}[{}{}: {}]",
                if *blacklist { "deny" } else { "allow" },
                if *respect_nbt { ", nbt" } else { "" },
                entries
                    .iter()
                    .map(Filter::describe)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Kind::Attributes { mode, tests } => format!(
                "{ATTRIBUTE_FILTER}[{}: {}]",
                mode.name(),
                tests
                    .iter()
                    .map(|(a, inv)| format!(
                        "{}{}{}",
                        if *inv { "!" } else { "" },
                        a.id(),
                        a.value().map(|v| format!("={v}")).unwrap_or_default()
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Kind::Package { address } => format!("{PACKAGE_FILTER}[-> {address}]"),
        }
    }

    /// `FilterItemStack.test(level, stack)`.
    pub fn test<P: Probe + ?Sized>(&self, stack: &P) -> bool {
        self.test_nbt(stack, false)
    }

    /// `FilterItemStack.test(level, stack, matchNBT)`.
    pub fn test_nbt<P: Probe + ?Sized>(&self, stack: &P, match_nbt: bool) -> bool {
        match &self.kind {
            Kind::Item { item, components } => test_direct(item, components, stack, match_nbt),
            Kind::List {
                entries,
                respect_nbt,
                blacklist,
            } => {
                for e in entries {
                    if e.test_nbt(stack, *respect_nbt) {
                        return !blacklist;
                    }
                }
                *blacklist
            }
            Kind::Attributes { mode, tests } => {
                for (attribute, inverted) in tests {
                    let matches = stack.attribute(attribute) != *inverted;
                    match (matches, mode) {
                        (true, Mode::Blacklist) => return false,
                        (true, Mode::WhitelistDisj) => return true,
                        (false, Mode::WhitelistConj) => return false,
                        _ => {}
                    }
                }
                !matches!(mode, Mode::WhitelistDisj)
            }
            Kind::Package { address } => {
                crate::package::is_package(stack.item())
                    && crate::package::match_address(&stack.address(), address)
            }
        }
    }

    /// `FilterItemStack.test(level, FluidStack)`: the fluid the filter item empties into (a
    /// bucket's); a list tests its entries; attribute and package filters pass no fluid.
    pub fn test_fluid(&self, fluid: Fluid) -> bool {
        match &self.kind {
            Kind::Item { item, .. } => Fluid::of_bucket(item) == Some(fluid),
            Kind::List {
                entries, blacklist, ..
            } => {
                for e in entries {
                    if e.test_fluid(fluid) {
                        return !blacklist;
                    }
                }
                *blacklist
            }
            Kind::Attributes { .. } | Kind::Package { .. } => false,
        }
    }
}

/// `FilterItem.testDirect`: with components, packages compare their data, any other stack its
/// item and components; without, any package passes a package and any other its item.
fn test_direct<P: Probe + ?Sized>(
    item: &str,
    components: &str,
    stack: &P,
    match_nbt: bool,
) -> bool {
    use crate::package::is_package;
    let other = stack.item();
    if is_package(item) && is_package(other) {
        return !match_nbt || (item == other && components == stack.components());
    }
    if item != other {
        return false;
    }
    !match_nbt || components == stack.components()
}

/// A detector's filter test: any item (and fluid) with no filter.
pub trait Passes {
    fn filtered(&self) -> bool;
    fn item(&self, item: &str) -> bool;
    fn fluid(&self, fluid: Fluid) -> bool;
}

impl<T: Passes + ?Sized> Passes for &T {
    fn filtered(&self) -> bool {
        (**self).filtered()
    }
    fn item(&self, item: &str) -> bool {
        (**self).item(item)
    }
    fn fluid(&self, fluid: Fluid) -> bool {
        (**self).fluid(fluid)
    }
}

impl Passes for Option<&Filter> {
    fn filtered(&self) -> bool {
        self.is_some()
    }
    fn item(&self, item: &str) -> bool {
        self.is_none_or(|f| f.test(item))
    }
    fn fluid(&self, fluid: Fluid) -> bool {
        self.is_none_or(|f| f.test_fluid(fluid))
    }
}

/// A plain item id as the filter.
impl Passes for Option<&str> {
    fn filtered(&self) -> bool {
        self.is_some()
    }
    fn item(&self, item: &str) -> bool {
        self.is_none_or(|f| f == item)
    }
    fn fluid(&self, fluid: Fluid) -> bool {
        self.is_none_or(|f| Fluid::of_bucket(f) == Some(fluid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stack as the tests see it.
    struct S<'a> {
        item: &'a str,
        components: &'a str,
        address: &'a str,
        attributes: Vec<Attribute>,
    }

    fn s(item: &str) -> S<'_> {
        S {
            item,
            components: "",
            address: "",
            attributes: Vec::new(),
        }
    }

    impl Probe for S<'_> {
        fn item(&self) -> &str {
            self.item
        }
        fn components(&self) -> String {
            self.components.to_owned()
        }
        fn address(&self) -> String {
            self.address.to_owned()
        }
        fn attribute(&self, a: &Attribute) -> bool {
            self.attributes.contains(a)
        }
    }

    fn list(items: &[Spec], respect_nbt: bool, blacklist: bool) -> Filter {
        Filter::compile(&Spec {
            item: FILTER.into(),
            components: "{filter_items}".into(),
            has_list: true,
            list: items.to_vec(),
            respect_nbt,
            blacklist,
            ..Spec::default()
        })
    }

    fn attributes(mode: Option<&str>, tests: &[(&str, Option<&str>, bool)]) -> Filter {
        Filter::compile(&Spec {
            item: ATTRIBUTE_FILTER.into(),
            components: "{attributes}".into(),
            mode: mode.map(str::to_owned),
            attributes: Some(
                tests
                    .iter()
                    .map(|(t, v, i)| ((*t).to_owned(), v.map(str::to_owned), *i))
                    .collect(),
            ),
            ..Spec::default()
        })
    }

    #[test]
    fn plain_items_pass_their_item_and_blank_filters_only_themselves() {
        let iron = Filter::item("minecraft:iron_ingot");
        assert!(iron.test("minecraft:iron_ingot"));
        assert!(!iron.test("minecraft:dirt"));
        // A plain stack ignores components even when it has some.
        let named = S {
            components: "{custom_name:'x'}",
            ..s("minecraft:iron_ingot")
        };
        assert!(iron.test(&named));
        assert!(!iron.is_filter_item());
        // A blank list filter (no components) is a plain item: it passes filters only.
        let blank = Filter::item(FILTER);
        assert!(blank.is_filter_item());
        assert!(blank.test(FILTER));
        assert!(!blank.test("minecraft:dirt"));
        // Any package passes a package filter set to a package; with components, only the same.
        let package = Filter::item("create:cardboard_package_10x12");
        assert!(package.test("create:cardboard_package_12x12"));
        assert!(!package.test_nbt(&s("create:cardboard_package_12x12"), true));
        // Fluids: a bucket's.
        let water = Filter::item("minecraft:water_bucket");
        assert!(water.test_fluid(Fluid::Water));
        assert!(!water.test_fluid(Fluid::Lava));
        assert!(!iron.test_fluid(Fluid::Water));
    }

    #[test]
    fn list_filters_allow_or_deny_their_items_and_may_respect_components() {
        let allow = list(
            &[
                Spec::item("minecraft:iron_ingot"),
                Spec::item("minecraft:gold_ingot"),
            ],
            false,
            false,
        );
        assert!(allow.test("minecraft:gold_ingot"));
        assert!(!allow.test("minecraft:dirt"));
        let deny = list(&[Spec::item("minecraft:iron_ingot")], false, true);
        assert!(!deny.test("minecraft:iron_ingot"));
        assert!(deny.test("minecraft:dirt"));
        // Respecting components: the same item with other components fails.
        let named = Spec {
            components: "{custom_name:'a'}".into(),
            ..Spec::item("minecraft:iron_ingot")
        };
        let nbt = list(&[named], true, false);
        let a = S {
            components: "{custom_name:'a'}",
            ..s("minecraft:iron_ingot")
        };
        assert!(nbt.test(&a));
        assert!(!nbt.test("minecraft:iron_ingot"));
        // Not respecting them, the item alone counts.
        let loose = list(
            &[Spec {
                components: "{custom_name:'a'}".into(),
                ..Spec::item("minecraft:iron_ingot")
            }],
            false,
            false,
        );
        assert!(loose.test("minecraft:iron_ingot"));
        // Flags without a list (`has(FILTER_ITEMS)` false): an allow list of nothing.
        let flags_only = Filter::compile(&Spec {
            item: FILTER.into(),
            components: "{blacklist}".into(),
            blacklist: true,
            ..Spec::default()
        });
        assert!(!flags_only.test("minecraft:dirt"));
        // A list in a list is a filter itself.
        let inner = Spec {
            item: FILTER.into(),
            components: "{filter_items}".into(),
            has_list: true,
            list: vec![Spec::item("minecraft:dirt")],
            ..Spec::default()
        };
        let outer = list(&[inner], false, false);
        assert!(outer.test("minecraft:dirt"));
        assert!(!outer.test("minecraft:stone"));
        // Fluids through the list's buckets, a deny list passing the others.
        let buckets = list(&[Spec::item("minecraft:water_bucket")], false, true);
        assert!(!buckets.test_fluid(Fluid::Water));
        assert!(buckets.test_fluid(Fluid::Lava));
    }

    #[test]
    fn attribute_filters_follow_their_mode() {
        let mut sword = s("minecraft:iron_sword");
        sword.attributes = vec![Attribute::Damaged, Attribute::NotStackable];
        let mut dirt = s("minecraft:dirt");
        dirt.attributes = vec![Attribute::Placeable];
        let tests = [("create:damaged", None, false), ("placeable", None, false)];
        // Any of them.
        let any = attributes(None, &tests);
        assert!(any.test(&sword) && any.test(&dirt));
        assert!(!any.test(&s("minecraft:stick")));
        // All of them.
        let all = attributes(Some("whitelist_conj"), &tests);
        assert!(!all.test(&sword) && !all.test(&dirt));
        let mut both = s("minecraft:x");
        both.attributes = vec![Attribute::Damaged, Attribute::Placeable];
        assert!(all.test(&both));
        // None of them.
        let none = attributes(Some("blacklist"), &tests);
        assert!(!none.test(&sword) && none.test(&s("minecraft:stick")));
        // Inverted: "is not damaged".
        let not_damaged = attributes(None, &[("damaged", None, true)]);
        assert!(!not_damaged.test(&sword) && not_damaged.test(&dirt));
        // No attributes: an allow-any list passes nothing, the others everything.
        assert!(!attributes(None, &[]).test(&dirt));
        assert!(attributes(Some("whitelist_conj"), &[]).test(&dirt));
        assert!(attributes(Some("blacklist"), &[]).test(&dirt));
        // Valued attributes, and ones the port can't read left out (decoded as null).
        let tag = attributes(
            None,
            &[
                ("in_tag", Some("#minecraft:logs"), false),
                ("has_color", Some("red"), false),
            ],
        );
        assert_eq!(
            tag.kind,
            Kind::Attributes {
                mode: Mode::WhitelistDisj,
                tests: vec![(Attribute::InTag("minecraft:logs".into()), false)]
            }
        );
        // Fluids never pass an attribute filter.
        assert!(!any.test_fluid(Fluid::Water));
    }

    #[test]
    fn package_filters_match_addresses_by_glob() {
        let f = Filter::compile(&Spec {
            item: PACKAGE_FILTER.into(),
            components: "{address}".into(),
            address: "Base*".into(),
            ..Spec::default()
        });
        let package = S {
            address: "Base North",
            ..s("create:cardboard_package_12x12")
        };
        assert!(f.test(&package));
        let other = S {
            address: "Farm",
            ..s("create:cardboard_package_12x12")
        };
        assert!(!f.test(&other));
        // Not a package: never.
        assert!(!f.test(&S {
            address: "Base",
            ..s("minecraft:chest")
        }));
        assert!(!f.test_fluid(Fluid::Water));
    }

    #[test]
    fn attributes_parse_by_create_s_type_ids() {
        assert_eq!(
            Attribute::parse("create:placeable", None),
            Ok(Attribute::Placeable)
        );
        assert_eq!(
            Attribute::parse("book_copy", Some("2")),
            Ok(Attribute::BookCopy(2))
        );
        assert!(matches!(
            Attribute::parse("in_item_group", Some("x")),
            Err(Refused::NotPorted(_))
        ));
        assert!(matches!(
            Attribute::parse("nope", None),
            Err(Refused::Unknown(_))
        ));
        assert!(Attribute::parse("in_tag", None).is_err());
        assert_eq!(
            Attribute::InTag("minecraft:logs".into()).id(),
            "create:in_tag"
        );
        assert_eq!(Attribute::Smokable.id(), "create:smokable");
        assert_eq!(Attribute::Smokable.value(), None);
        assert!(badly_damaged(200, 250) && !badly_damaged(150, 250) && !badly_damaged(0, 0));
        assert!(not_stackable(1, 0, 0));
        assert!(not_stackable(64, 100, 3));
        assert!(!not_stackable(64, 100, 0) && !not_stackable(64, 0, 0));
        assert_eq!(Mode::parse("blacklist"), Some(Mode::Blacklist));
    }

    #[test]
    fn a_filter_keeps_its_stack_for_the_glue() {
        let f = Filter::item("minecraft:dirt").with_source(Rc::new(7u32));
        assert_eq!(f.source::<u32>(), Some(&7));
        assert_eq!(f.source::<i32>(), None);
        assert_eq!(f, Filter::item("minecraft:dirt"));
    }
}
