//! Table cloths (`TableClothBlockEntity`, `TableClothBlock`, `TableClothRenderer`) and their
//! shops (`ShoppingListItem`, `StockTickerInteractionHandler.interactWithShop`).
//!
//! A cloth gets its block entity (`entity=true`) once something goes on it: up to four items set
//! down by hand (the last taken back by an empty hand), turned to the player who put them. A
//! cloth programmed at a stock keeper's screen (`encodeRequester`: the request in hand is kept
//! by the cloth item, with its ticker's place) and placed becomes its owner's shop: it shows what
//! it sells and a price tag (`TableClothFilteringBehaviour`, its value box: the owner sets the
//! payment item by a click and its amount on a board of 1 to 100). A click on a shop with an
//! empty hand or a shopping list adds one purchase to the list (`useShop`: as much as the
//! ticker's stock allows), and the list handed to the keeper pays from the player's inventory
//! into the ticker's `receivedPayments` and sends the order to the list's address. A broken shop
//! drops its cloth programmed again; a broken cloth drops what lay on it.
//!
//! Each level's cloths are kept here by position and saved with their column (`create_table_cloths`); what clients draw is in the cloth's block entity data
//! (`TableCloth`). The clipboard copies and pastes the price (`Filtering`, the payment item).

use super::super::items;
use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::{Key, body, keeper, network_at, network_stock, with};
use create_engine::Block;
use create_engine::kinetics::Dir;
use create_engine::stock::{self as st, Order, RequestType, Summary};
use mod_api::{
    BlockEntityPart, BlockEntitySubmitEvent, BlockEntityUpdateTagEvent, BlockPart, ClientHost,
    Compound, Hand, Host, InteractionResult, ItemDisplayContext, ItemStack, PlayerRef, SoundSource,
    SupportType, Tag, Text,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// The cloths' block entity type.
pub const TYPE: &str = "create:table_cloth";

/// The cloth's state in its block entity's data, for its clients.
pub const DATA: &str = "TableCloth";

/// The key of a column's cloths in its saved data.
pub(super) const SAVED: &str = "create_table_cloths";

/// The shopping list's components (`AllDataComponents.SHOPPING_LIST`, `SHOPPING_LIST_ADDRESS`),
/// in its custom data.
const LIST: &str = "create:shopping_list";
const LIST_ADDRESS: &str = "create:shopping_list_address";

/// `AutoRequestData`'s ticker place and dimension on a programmed cloth (beside the requester's
/// `EncodedRequest` and `EncodedAddress`).
pub(super) const TARGET_POS: &str = "TargetPos";
pub(super) const TARGET_DIM: &str = "TargetDim";

/// A shop (`requestData` and `owner`).
#[derive(Clone, Debug)]
struct Shop {
    order: Vec<(Key, u32)>,
    address: String,
    /// The ticker it sells from (`targetOffset` from the cloth, kept as a place).
    ticker: [i32; 3],
    dim: String,
    owner: u128,
    /// What its cloth drops carrying (`AutoRequestData.writeToItem`), made when it is known.
    item_data: Tag,
}

/// `AutoRequestData` as a programmed cloth item carries it: the order, its address, the
/// ticker's place and dimension.
fn item_data(
    host: &dyn Host,
    order: &[(Key, u32)],
    address: &str,
    ticker: [i32; 3],
    dim: &str,
) -> Tag {
    Tag::compound([
        ("EncodedRequest", order_tag(host, order)),
        ("EncodedAddress", Tag::String(address.into())),
        (TARGET_POS, Tag::IntArray(ticker.to_vec())),
        (TARGET_DIM, Tag::String(dim.into())),
    ])
}

/// A cloth with its block entity.
#[derive(Clone, Debug)]
struct Cloth {
    /// `manuallyAddedItems`.
    items: Vec<ItemStack>,
    facing: Dir,
    shop: Option<Shop>,
    /// `priceTag`: the payment item and its amount (`count`, at least 1).
    price: Option<ItemStack>,
    price_count: u32,
    /// `sideOccluded`: another cloth or a full face beside it, so the tag lies on top.
    occluded: bool,
    /// What its ticker's network holds of each item of its order, for its clients' overlay
    /// (`getStockLevelForTrade` on a client reads the ticker's last snapshot).
    stock: Vec<i32>,
}

impl Default for Cloth {
    fn default() -> Self {
        Cloth {
            items: Vec::new(),
            facing: Dir::South,
            shop: None,
            price: None,
            price_count: 1,
            occluded: false,
            stock: Vec::new(),
        }
    }
}

thread_local! {
    static CLOTHS: RefCell<HashMap<usize, HashMap<[i32; 3], Cloth>>> =
        RefCell::new(HashMap::new());
}

fn cloths<R>(dim: usize, f: impl FnOnce(&mut HashMap<[i32; 3], Cloth>) -> R) -> R {
    CLOTHS.with_borrow_mut(|c| f(c.entry(dim).or_default()))
}

pub(super) fn reset() {
    CLOTHS.with_borrow_mut(HashMap::clear);
    CHANGED.with_borrow_mut(Vec::clear);
}

thread_local! {
    /// Cloths placed, changed or removed, looked at in their level's tick.
    static CHANGED: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
}

/// A block at `pos` was or became a cloth.
pub(super) fn changed(dim: usize, pos: [i32; 3]) {
    CHANGED.with_borrow_mut(|c| c.push((dim, pos)));
}

/// Whether `b` is one of the table cloths.
pub(crate) fn is_cloth(b: Block) -> bool {
    b.name().ends_with("_table_cloth")
}

fn uuid_ints(id: u128) -> Tag {
    Tag::IntArray(st::network_ints(id).to_vec())
}

fn green(text: Text, color: &str) -> Text {
    text.with_style(keeper::style(color, false))
}

fn status(host: &mut dyn mod_api::ServerHost, player: PlayerRef, key: &str, color: Option<&str>) {
    let mut t = Text::translate(key, Vec::new());
    if let Some(c) = color {
        t = green(t, c);
    }
    let _ = player.action_bar(host, t);
}

fn sound(lv: &mut ServerLevel<'_>, id: &str, at: [f64; 3], volume: f32, pitch: f32) {
    let _ = lv
        .level
        .play_sound(&mut *lv.host, id, at, SoundSource::Blocks, volume, pitch);
}

fn centre(p: [i32; 3]) -> [f64; 3] {
    p.map(|v| f64::from(v) + 0.5)
}

/// `nextFloat()` of the level's random.
pub(super) fn random_spread(lv: &mut ServerLevel<'_>) -> f32 {
    use mod_api::RandomSource;
    lv.level
        .with_random(&mut *lv.host, |r| r.next_float())
        .unwrap_or(0.5)
}

// ---------------------------------------------------------------------------------------------
// Saving and showing

fn order_tag(host: &dyn Host, order: &[(Key, u32)]) -> Tag {
    let o = Order::simple(order.iter().map(|(k, n)| (k.0.clone(), *n)).collect());
    body::order_nbt(host, &o)
}

fn order_of(host: &dyn Host, t: Option<&Tag>) -> Vec<(Key, u32)> {
    t.and_then(|t| body::order_from_nbt(host, t))
        .map(|o| o.stacks.iter().map(|(s, n)| (Key::of(s), *n)).collect())
        .unwrap_or_default()
}

fn ints(t: Option<&Tag>) -> Vec<i32> {
    match t {
        Some(Tag::IntArray(v)) => v.clone(),
        _ => Vec::new(),
    }
}

fn int(t: Option<&Tag>) -> Option<i32> {
    match t {
        Some(Tag::Int(v)) => Some(*v),
        _ => None,
    }
}

fn stack_tag(host: &dyn Host, s: &ItemStack) -> Option<Tag> {
    s.to_nbt(host).map(Tag::Compound)
}

fn stack_of(host: &dyn Host, t: Option<&Tag>) -> Option<ItemStack> {
    match t {
        Some(Tag::Compound(c)) if !c.is_empty() => ItemStack::from_nbt(host, c),
        _ => None,
    }
}

/// `Direction.get2DDataValue`: south 0, west 1, north 2, east 3.
fn data_2d(d: Dir) -> i32 {
    match d {
        Dir::West => 1,
        Dir::North => 2,
        Dir::East => 3,
        _ => 0,
    }
}

fn from_2d(v: i32) -> Dir {
    [Dir::South, Dir::West, Dir::North, Dir::East][v.rem_euclid(4) as usize]
}

/// `TableClothBlockEntity.write`: its items, facing, shop and price, and whether its tag lies on
/// top (for clients).
fn cloth_tag(host: &dyn Host, c: &Cloth) -> Compound {
    let mut out = Compound::new();
    out.insert(
        "Items".into(),
        Tag::List(c.items.iter().filter_map(|s| stack_tag(host, s)).collect()),
    );
    out.insert("Facing".into(), Tag::Int(data_2d(c.facing)));
    if let Some(s) = &c.shop {
        out.insert(
            "Shop".into(),
            Tag::compound([
                ("Order", order_tag(host, &s.order)),
                ("Address", Tag::String(s.address.as_str().into())),
                ("Ticker", Tag::IntArray(s.ticker.to_vec())),
                ("Dim", Tag::String(s.dim.as_str().into())),
                ("Owner", uuid_ints(s.owner)),
            ]),
        );
    }
    if let Some(t) = c.price.as_ref().and_then(|p| stack_tag(host, p)) {
        out.insert("Price".into(), t);
    }
    out.insert("PriceCount".into(), Tag::Int(c.price_count as i32));
    out.insert("Occluded".into(), Tag::Byte(i8::from(c.occluded)));
    out.insert("Stock".into(), Tag::IntArray(c.stock.clone()));
    out
}

fn cloth_of(host: &dyn Host, c: &Compound) -> Cloth {
    let items = match c.get(&"Items".into()) {
        Some(Tag::List(l)) => l.iter().filter_map(|t| stack_of(host, Some(t))).collect(),
        _ => Vec::new(),
    };
    let shop = match c.get(&"Shop".into()) {
        Some(Tag::Compound(s)) => {
            let t = ints(s.get(&"Ticker".into()));
            let order = order_of(host, s.get(&"Order".into()));
            let address = super::text(s.get(&"Address".into()));
            let ticker = [
                t.first().copied().unwrap_or(0),
                t.get(1).copied().unwrap_or(0),
                t.get(2).copied().unwrap_or(0),
            ];
            let dim = super::text(s.get(&"Dim".into()));
            Some(Shop {
                item_data: item_data(host, &order, &address, ticker, &dim),
                order,
                address,
                ticker,
                dim,
                owner: st::network_from_ints(&ints(s.get(&"Owner".into()))).unwrap_or(0),
            })
        }
        _ => None,
    };
    Cloth {
        items,
        facing: from_2d(int(c.get(&"Facing".into())).unwrap_or(0)),
        shop,
        price: stack_of(host, c.get(&"Price".into())),
        price_count: int(c.get(&"PriceCount".into())).unwrap_or(1).max(1) as u32,
        occluded: matches!(c.get(&"Occluded".into()), Some(Tag::Byte(b)) if *b != 0),
        stock: ints(c.get(&"Stock".into())),
    }
}

/// The cloths of column `chunk`, as `TableCloths` (`Pos` and the cloth's data each).
pub(super) fn save(lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
    let host: &dyn Host = &*lv.host;
    let mut list: Vec<([i32; 3], Compound)> = cloths(lv.dim, |m| {
        m.iter()
            .filter(|(p, _)| [p[0] >> 4, p[2] >> 4] == chunk)
            .map(|(p, c)| (*p, cloth_tag(host, c)))
            .collect()
    });
    if list.is_empty() {
        data.remove(&SAVED.into());
        return;
    }
    list.sort_by_key(|(p, _)| *p);
    let tags = list
        .into_iter()
        .map(|(p, c)| {
            let mut c = c;
            c.insert("Pos".into(), Tag::IntArray(p.to_vec()));
            Tag::Compound(c)
        })
        .collect();
    data.insert(SAVED.into(), Tag::List(tags));
}

pub(super) fn load(lv: &mut ServerLevel<'_>, data: &Compound) {
    let Some(Tag::List(l)) = data.get(&SAVED.into()) else {
        return;
    };
    let host: &dyn Host = &*lv.host;
    let loaded: Vec<([i32; 3], Cloth)> = l
        .iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else { return None };
            let p = ints(c.get(&"Pos".into()));
            (p.len() == 3).then(|| ([p[0], p[1], p[2]], cloth_of(host, c)))
        })
        .collect();
    cloths(lv.dim, |m| m.extend(loaded));
}

pub(super) fn unload(dim: usize, chunk: [i32; 2]) {
    cloths(dim, |m| m.retain(|p, _| [p[0] >> 4, p[2] >> 4] != chunk));
}

fn mark(lv: &ServerLevel<'_>, pos: [i32; 3]) {
    super::super::persist::mark_pos_unsaved(lv.dim, pos);
}

/// The cloth's state into its block entity, for its clients.
fn show(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let Some(c) = cloths(lv.dim, |m| m.get(&pos).cloned()) else {
        return;
    };
    // TableClothBlockEntity.write rewrites its items and request and omits a cleared owner.
    let cloth = cloth_tag(&*lv.host, &c);
    lv.write_entity_field(pos, DATA, cloth);
    mark(lv, pos);
}

/// `HAS_BE` (`entity`) on the cloth at `pos`.
fn set_has_entity(lv: &mut ServerLevel<'_>, pos: [i32; 3], on: bool) {
    lv.set_property(
        pos,
        "entity",
        if on { "true" } else { "false" },
        update_all(),
    );
}

/// The cloth at `pos` became another block (or is gone): what lay on it drops
/// (`TableClothBlockEntity.destroy`); one that lost its block entity (`entity=false`) forgets it.
pub(super) fn block_moved(lv: &mut ServerLevel<'_>, pos: [i32; 3], is: Option<Block>) {
    let keeps = is.is_some_and(is_cloth)
        && lv
            .block_state(pos)
            .and_then(|s| lv.host.block_state_string(s))
            .is_some_and(|t| t.contains("entity=true"));
    if keeps {
        return;
    }
    let Some(c) = cloths(lv.dim, |m| m.remove(&pos)) else {
        return;
    };
    mark(lv, pos);
    if is.is_some_and(is_cloth) {
        return;
    }
    let at = pos.map(f64::from);
    for s in c.items {
        let _ = lv
            .level
            .spawn_item(&mut *lv.host, at[0] + 0.5, at[1] + 0.5, at[2] + 0.5, s, 10);
    }
}

/// `lazyTick`: whether the tag lies on top (`sideOccluded`): another cloth or a full face of
/// the block below the one the cloth faces.
fn occluded(lv: &ServerLevel<'_>, pos: [i32; 3], facing: Dir) -> bool {
    let o = facing.offset();
    let rel = [pos[0] + o[0], pos[1], pos[2] + o[2]];
    if super::block_at(lv, rel).is_some_and(is_cloth) {
        return true;
    }
    let below = [rel[0], rel[1] - 1, rel[2]];
    lv.level.block_face_sturdy(
        &*lv.host,
        below,
        super::super::decor::face_of(facing.opposite()),
        SupportType::Full,
    ) == Some(true)
}

/// Cloths changed since the last tick; then the shops' `lazyTick` (every 10 ticks): their tags
/// follow what is beside them.
pub(super) fn level_tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mine: Vec<[i32; 3]> = CHANGED.with_borrow_mut(|c| {
        if c.is_empty() {
            return Vec::new();
        }
        let (m, rest): (Vec<_>, Vec<_>) = std::mem::take(c).into_iter().partition(|x| x.0 == dim);
        *c = rest;
        m.into_iter().map(|x| x.1).collect()
    });
    for pos in mine {
        let is = super::block_at(lv, pos);
        block_moved(lv, pos, is);
    }
    if lv.game_time() % 10 != 0 {
        return;
    }
    let shops: Vec<([i32; 3], Dir, bool)> = cloths(lv.dim, |m| {
        m.iter()
            .filter(|(_, c)| c.shop.is_some())
            .map(|(p, c)| (*p, c.facing, c.occluded))
            .collect()
    });
    for (pos, facing, was) in shops {
        if !lv.is_loaded(pos) {
            continue;
        }
        let now = occluded(lv, pos, facing);
        let stock = order_stock(lv, pos);
        let changed = cloths(lv.dim, |m| {
            let c = m.get_mut(&pos)?;
            let changed = c.occluded != now || c.stock != stock;
            c.occluded = now;
            c.stock = stock;
            Some(changed)
        });
        if changed == Some(true) {
            show(lv, pos);
        }
        let _ = was;
    }
}

// ---------------------------------------------------------------------------------------------
// Using a cloth

/// `TableClothFilterSlot.getLocalOffset`: on top by the edge it faces (`sideOccluded`), else
/// hanging down its side.
pub(crate) fn price_slot(facing: Dir, occluded: bool) -> [f64; 3] {
    let v = if occluded {
        [8.0, 0.75, 15.25]
    } else {
        [12.0, -2.75, 16.75]
    }
    .map(|x| x / 16.0);
    // `rotateCentered(v, -toYRot, Y)`: toYRot south 0, west 90, north 180, east 270.
    let deg: f64 = match facing {
        Dir::West => -90.0,
        Dir::North => -180.0,
        Dir::East => -270.0,
        _ => 0.0,
    };
    let (s, c) = deg.to_radians().sin_cos();
    let (x, z) = (v[0] - 0.5, v[2] - 0.5);
    [x * c + z * s + 0.5, v[1], z * c - x * s + 0.5]
}

/// `ValueBoxTransform.testHit`: within a quarter block of the slot.
pub(crate) fn hits_price(facing: Dir, occluded: bool, local: [f64; 3]) -> bool {
    let s = price_slot(facing, occluded);
    let d2: f64 = (0..3).map(|i| (local[i] - s[i]).powi(2)).sum();
    d2.sqrt() < 0.25
}

/// `TableClothFilteringBehaviour.mayInteract`: no owner, or its owner.
fn may_price(c: &Cloth, player: u128) -> bool {
    c.shop.as_ref().is_none_or(|s| s.owner == player)
}

/// `TableClothBlock.useItemOn` after the placement helper: an empty hand or sneaking does
/// nothing to a cloth without block entity; else it gets one, and the click is its
/// (`TableClothBlockEntity.use`). A click on a shop's price tag is the tag's (its value box).
pub(crate) fn use_cloth(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    click: &mod_api::BlockClick<'_>,
    has_entity: bool,
) -> Option<InteractionResult> {
    let pos = click.pos;
    let held = items::held(&*lv.host, player, Hand::Main);
    let sneaking = items::sneaking(&*lv.host, player);
    if (sneaking || held.is_none()) && !has_entity {
        return None;
    }
    let local = [0, 1, 2].map(|i| click.hit.point[i] - f64::from(pos[i]));
    let existing = cloths(lv.dim, |m| m.get(&pos).cloned());
    if let Some(c) = &existing
        && c.shop.is_some()
        && hits_price(c.facing, c.occluded, local)
        && may_price(c, player.uuid())
        && !sneaking
    {
        // The value box's short click (`interaction::values`) sets the price.
        return Some(InteractionResult::Success);
    }
    if !has_entity {
        set_has_entity(lv, pos, true);
    }
    cloths(lv.dim, |m| {
        m.entry(pos).or_default();
    });
    let c = cloths(lv.dim, |m| m.get(&pos).cloned()).unwrap_or_default();
    if c.shop.is_some() {
        use_shop(lv, player, pos, &c);
        return Some(InteractionResult::Success);
    }
    let Some(held) = held else {
        // The last item set down comes back to the hand.
        let mut c = c;
        let Some(last) = c.items.pop() else {
            return Some(InteractionResult::Success);
        };
        items::set_held(lv.host, player, Hand::Main, Some(last));
        sound(
            lv,
            "minecraft:entity.item_frame.remove_item",
            centre(pos),
            0.5,
            1.0,
        );
        if c.items.is_empty() {
            cloths(lv.dim, |m| m.remove(&pos));
            set_has_entity(lv, pos, false);
            mark(lv, pos);
        } else {
            cloths(lv.dim, |m| m.insert(pos, c));
            show(lv, pos);
        }
        return Some(InteractionResult::Success);
    };
    if c.items.len() >= 4 {
        return Some(InteractionResult::Success);
    }
    sound(
        lv,
        "minecraft:entity.item_frame.add_item",
        centre(pos),
        0.5,
        1.0,
    );
    let one = body::with_count(&held, 1).unwrap_or_else(|| held.clone());
    let yaw = player.entity().rotation(&*lv.host).map_or(0.0, |r| r[0]);
    let facing = create_engine::place::horizontal_direction(yaw).opposite();
    cloths(lv.dim, |m| {
        let c = m.entry(pos).or_default();
        c.items.push(one);
        c.facing = facing;
    });
    // `heldItem.shrink(1)`, in creative too.
    items::set_held(lv.host, player, Hand::Main, held.shrink(1));
    show(lv, pos);
    Some(InteractionResult::Success)
}

// ---------------------------------------------------------------------------------------------
// Shops

/// `ShoppingList`: purchases by cloth, the shop's owner and its ticker's network.
#[derive(Clone, Debug, Default, PartialEq)]
struct List {
    purchases: Vec<([i32; 3], u32)>,
    owner: u128,
    network: st::Network,
}

impl List {
    fn of(stack: &ItemStack) -> Option<List> {
        let Some(Tag::Compound(c)) = items::custom(stack).get(&LIST.into()).cloned() else {
            return None;
        };
        let purchases = match c.get(&"purchases".into()) {
            Some(Tag::List(l)) => l
                .iter()
                .filter_map(|t| {
                    let Tag::Compound(e) = t else { return None };
                    let p = ints(e.get(&"pos".into()));
                    (p.len() == 3).then(|| {
                        (
                            [p[0], p[1], p[2]],
                            int(e.get(&"amount".into())).unwrap_or(0).max(0) as u32,
                        )
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        Some(List {
            purchases,
            owner: st::network_from_ints(&ints(c.get(&"shop_owner".into())))?,
            network: st::network_from_ints(&ints(c.get(&"shop_network".into())))?,
        })
    }

    fn tag(&self) -> Tag {
        Tag::compound([
            (
                "purchases",
                Tag::List(
                    self.purchases
                        .iter()
                        .map(|(p, n)| {
                            Tag::compound([
                                ("pos", Tag::IntArray(p.to_vec())),
                                ("amount", Tag::Int(*n as i32)),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("shop_owner", uuid_ints(self.owner)),
            ("shop_network", uuid_ints(self.network)),
        ])
    }

    fn purchases(&self, cloth: [i32; 3]) -> u32 {
        self.purchases
            .iter()
            .find(|e| e.0 == cloth)
            .map_or(0, |e| e.1)
    }

    /// `bakeEntries`: what the purchases order and what they cost, the cloth at `ignore` left
    /// out.
    fn bake(&self, dim: usize, ignore: Option<[i32; 3]>) -> (Summary<Key>, Summary<Key>) {
        let (mut out, mut cost) = (Summary::default(), Summary::default());
        for (p, n) in &self.purchases {
            if Some(*p) == ignore {
                continue;
            }
            let Some(c) = cloths(dim, |m| m.get(p).cloned()) else {
                continue;
            };
            let Some(s) = &c.shop else { continue };
            if let Some(price) = &c.price {
                cost.add(&Key::of(price), (c.price_count * n) as i32);
            }
            for (k, count) in &s.order {
                out.add(k, (count * n) as i32);
            }
        }
        (out, cost)
    }
}

/// `ShoppingListItem.saveList`.
fn list_stack(host: &dyn Host, list: &List, address: &str) -> Option<ItemStack> {
    let mut s = body::new_stack(host, "create:shopping_list", 1)?;
    items::set_custom(
        &mut s,
        &[
            (LIST, Some(list.tag())),
            (LIST_ADDRESS, Some(Tag::String(address.into()))),
        ],
    );
    Some(s)
}

fn is_list(host: &dyn Host, s: &ItemStack) -> bool {
    s.item().id(host) == "create:shopping_list"
}

/// The ticker the shop sells from, with a keeper present (`isKeeperPresent`): its network.
fn ticker_network(lv: &ServerLevel<'_>, shop: &Shop) -> Option<st::Network> {
    if super::block_at(lv, shop.ticker) != Some(Block::StockTicker) {
        return None;
    }
    if !keeper::keeper_present(lv, shop.ticker) {
        return None;
    }
    network_at(lv.dim, shop.ticker)
}

/// What the shop at `pos`'s ticker's network holds of each item of its order.
fn order_stock(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> Vec<i32> {
    let Some(shop) = cloths(lv.dim, |m| m.get(&pos).and_then(|c| c.shop.clone())) else {
        return Vec::new();
    };
    if super::block_at(lv, shop.ticker) != Some(Block::StockTicker) {
        return Vec::new();
    }
    let Some(net) = network_at(lv.dim, shop.ticker) else {
        return Vec::new();
    };
    let dim = lv.dim;
    let stock = network_stock(lv, dim, net, false);
    shop.order.iter().map(|(k, _)| stock.count_of(k)).collect()
}

/// `getStockLevelForTrade`: how many times the shop's order is in stock, less what `other`
/// already buys of it.
fn stock_level(lv: &mut ServerLevel<'_>, pos: [i32; 3], other: Option<&List>) -> u32 {
    let Some(c) = cloths(lv.dim, |m| m.get(&pos).cloned()) else {
        return 0;
    };
    let Some(shop) = &c.shop else { return 0 };
    if super::block_at(lv, shop.ticker) != Some(Block::StockTicker) {
        return 0;
    }
    let Some(net) = network_at(lv.dim, shop.ticker) else {
        return 0;
    };
    let dim = lv.dim;
    let stock = network_stock(lv, dim, net, false);
    let modifier = other.map(|l| l.bake(dim, Some(pos)).0).unwrap_or_default();
    let mut smallest = i32::MAX;
    for (k, n) in &shop.order {
        if *n > 0 {
            smallest = smallest.min((stock.count_of(k) - modifier.count_of(k)) / *n as i32);
        }
    }
    smallest.max(0) as u32
}

/// `TableClothBlockEntity.useShop`.
fn use_shop(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3], c: &Cloth) {
    let Some(shop) = c.shop.clone() else { return };
    let host = &*lv.host;
    let in_hand = items::held(host, player, Hand::Main);
    let mut prev: Option<ItemStack> = None;
    let mut onto = false;
    // Other lists leave the hotbar.
    let slots = player.inventory_slots(host).unwrap_or_default();
    for (i, s) in slots {
        if i >= 9 || !is_list(&*lv.host, &s) {
            continue;
        }
        prev = Some(s);
        onto = true;
        let _ = player.set_inventory_slot(&mut *lv.host, i, None);
    }
    if let Some(h) = in_hand.as_ref().filter(|h| is_list(&*lv.host, h)) {
        prev = Some(h.clone());
        onto = true;
    }
    let deny = |lv: &mut ServerLevel<'_>| sound(lv, "create:deny", centre(pos), 0.5, 1.0);
    if in_hand.is_some() && !onto {
        status(
            lv.host,
            player,
            "create.stock_keeper.shopping_list_empty_hand",
            None,
        );
        deny(lv);
        return;
    }
    if c.price.is_none() {
        status(lv.host, player, "create.stock_keeper.no_price_set", None);
        deny(lv);
        return;
    }
    let network = ticker_network(lv, &shop);
    let prev_list = prev.as_ref().and_then(List::of);
    let level = stock_level(lv, pos, prev_list.as_ref());
    let Some(network) = network else {
        status(
            lv.host,
            player,
            "create.stock_keeper.keeper_missing",
            Some("red"),
        );
        deny(lv);
        return;
    };
    let give = |lv: &mut ServerLevel<'_>, s: ItemStack| {
        if items::held(&*lv.host, player, Hand::Main).is_none() {
            items::set_held(lv.host, player, Hand::Main, Some(s));
        } else {
            items::give_back(lv.host, player, s);
        }
    };
    if level == 0 {
        status(
            lv.host,
            player,
            "create.stock_keeper.out_of_stock",
            Some("red"),
        );
        deny(lv);
        if let Some(p) = prev {
            give(lv, p);
        }
        return;
    }
    let mut list = List {
        purchases: Vec::new(),
        owner: shop.owner,
        network,
    };
    if onto {
        match prev_list {
            Some(p) if p.owner == shop.owner && p.network == network => list = p,
            _ => onto = false,
        }
    }
    if list.purchases(pos) >= level {
        for e in &mut list.purchases {
            if e.0 == pos {
                e.1 = e.1.min(level);
            }
        }
        status(
            lv.host,
            player,
            "create.stock_keeper.limited_stock",
            Some("red"),
        );
    } else {
        sound(lv, "create:confirm_2", centre(pos), 0.5, 1.0);
        match list.purchases.iter_mut().find(|e| e.0 == pos) {
            Some(e) => e.1 += 1,
            None => list.purchases.push((pos, 1)),
        }
        if !onto {
            let t = Text::translate("create.stock_keeper.use_list_to_add_purchases", Vec::new())
                .with_style(keeper::style("#EEEEEE", false));
            let _ = player.action_bar(lv.host, t);
            sound(lv, "minecraft:item.book.page_turn", centre(pos), 1.0, 1.5);
        }
    }
    if let Some(s) = list_stack(&*lv.host, &list, &shop.address) {
        give(lv, s);
    }
}

/// `StockTickerInteractionHandler.interactWithShop`: the shopping list in hand, given to the
/// keeper of the ticker at `ticker`: the order in stock, room for the payment, the payment
/// taken from the player's inventory into the ticker, the order sent to the list's address.
pub(super) fn checkout(lv: &mut ServerLevel<'_>, player: PlayerRef, ticker: [i32; 3]) {
    let Some(stack) = items::held(&*lv.host, player, Hand::Main) else {
        return;
    };
    let Some(list) = List::of(&stack) else { return };
    let dim = lv.dim;
    let at = player
        .entity()
        .position(&*lv.host)
        .map_or(centre(ticker), |p| p.map(|v| v.floor() + 0.5));
    let deny = |lv: &mut ServerLevel<'_>| sound(lv, "create:deny", at, 1.0, 1.0);
    if network_at(dim, ticker) != Some(list.network) {
        deny(lv);
        status(
            lv.host,
            player,
            "create.stock_keeper.wrong_network",
            Some("red"),
        );
        return;
    }
    let (order, payment) = list.bake(dim, None);
    // Must be up to date.
    let stock = network_stock(lv, dim, list.network, true);
    let wanted: Vec<(Key, u32)> = order
        .by_count()
        .into_iter()
        .map(|(k, n)| (k, n.max(0) as u32))
        .collect();
    if wanted.iter().any(|(k, n)| stock.count_of(k) < *n as i32) {
        deny(lv);
        status(
            lv.host,
            player,
            "create.stock_keeper.stock_level_too_low",
            Some("red"),
        );
        return;
    }
    // Room in the ticker's 27 slots.
    let host = &*lv.host;
    let mut occupied: i32 = payment
        .by_count()
        .iter()
        .map(|(k, n)| {
            let max = host.max_stack_size(k.0.item()).unwrap_or(64).max(1) as i32;
            (n + max - 1) / max
        })
        .sum();
    let used = with(dim, |n| {
        n.tickers.get(&ticker).map_or(0, |t| t.payments.len())
    });
    occupied -= 27 - used as i32;
    if occupied > 0 {
        deny(lv);
        status(
            lv.host,
            player,
            "create.stock_keeper.cash_register_full",
            Some("red"),
        );
        return;
    }
    // Pay: first see that the inventory holds it all, then take it.
    let slots: Vec<(u32, ItemStack)> = player
        .inventory_slots(&*lv.host)
        .unwrap_or_default()
        .into_iter()
        .filter(|(i, _)| *i < 36)
        .collect();
    for simulate in [true, false] {
        let mut tally = payment.clone();
        let mut taken: Vec<ItemStack> = Vec::new();
        for (i, s) in &slots {
            let k = Key::of(s);
            let left = tally.count_of(&k);
            if left <= 0 {
                continue;
            }
            let n = (s.count() as i32).min(left);
            tally.add(&k, -n);
            if simulate {
                continue;
            }
            let _ = player.set_inventory_slot(&mut *lv.host, *i, s.clone().shrink(n as u32));
            if let Some(t) = body::with_count(s, n as u32) {
                taken.push(t);
            }
        }
        if simulate && tally.total() != 0 {
            deny(lv);
            status(
                lv.host,
                player,
                "create.stock_keeper.too_broke",
                Some("red"),
            );
            return;
        }
        if !simulate {
            pay_into(lv, ticker, taken);
        }
    }
    let address = super::text(items::custom(&stack).get(&LIST_ADDRESS.into()));
    let order = Order::simple(wanted);
    super::broadcast(
        lv,
        dim,
        list.network,
        RequestType::Player,
        &order,
        None,
        &address,
    );
    items::set_held(lv.host, player, Hand::Main, None);
    if !order.is_empty() {
        sound(lv, "create:stock_ticker_trade", centre(ticker), 1.0, 1.0);
    }
}

/// `ItemHandlerHelper.insertItemStacked` into the ticker's `receivedPayments`.
fn pay_into(lv: &mut ServerLevel<'_>, ticker: [i32; 3], taken: Vec<ItemStack>) {
    let host = &*lv.host;
    let maxes: Vec<u32> = taken
        .iter()
        .map(|s| host.max_stack_size(s.item()).unwrap_or(64).clamp(1, 64))
        .collect();
    with(lv.dim, |n| {
        let Some(t) = n.tickers.get_mut(&ticker) else {
            return;
        };
        for (s, max) in taken.into_iter().zip(maxes) {
            let mut left = s.count();
            for p in t.payments.iter_mut() {
                if left == 0 {
                    break;
                }
                if !p.is_stackable_with(&s) {
                    continue;
                }
                let room = max.saturating_sub(p.count());
                let put = room.min(left);
                if put > 0 {
                    let _ = p.set_count(p.count() + put);
                    left -= put;
                }
            }
            while left > 0 && t.payments.len() < 27 {
                let put = left.min(max);
                if let Some(x) = body::with_count(&s, put) {
                    t.payments.push(x);
                }
                left -= put;
            }
        }
        n.unsaved.push((ticker[0] >> 4, ticker[2] >> 4));
    });
}

/// `ShoppingListItem.use` / `useOn`: sneaking with it in the main hand throws it away.
pub(super) fn discard_list(
    host: &mut dyn mod_api::ServerHost,
    player: PlayerRef,
    hand: Hand,
) -> bool {
    if hand != Hand::Main || !items::sneaking(&*host, player) {
        return false;
    }
    items::set_held(host, player, hand, None);
    status(
        host,
        player,
        "create.table_cloth.shopping_list_discarded",
        None,
    );
    let at = player.entity().position(&*host).unwrap_or([0.0; 3]);
    let _ = player.play_sound(
        host,
        "minecraft:item.book.page_turn",
        SoundSource::Players,
        1.0,
        1.0,
    );
    let _ = at;
    true
}

// ---------------------------------------------------------------------------------------------
// Programmed cloths

/// `TableClothBlock.setPlacedBy` with `AutoRequestData.readFromItem`: a programmed cloth
/// placed by a player becomes their shop, facing them; whether its ticker is near enough and in
/// this dimension is said.
pub(super) fn placed(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    pos: [i32; 3],
    stack: &ItemStack,
) {
    let custom = items::custom(stack);
    let Some(Tag::Compound(c)) = custom.get(&super::AUTO_REQUEST.into()) else {
        return;
    };
    let host: &dyn Host = &*lv.host;
    let order = order_of(host, c.get(&"EncodedRequest".into()));
    let address = super::text(c.get(&"EncodedAddress".into()));
    let t = ints(c.get(&TARGET_POS.into()));
    if t.len() != 3 {
        return;
    }
    let ticker = [t[0], t[1], t[2]];
    let dim = super::text(c.get(&TARGET_DIM.into()));
    let d2: i64 = (0..3).map(|i| i64::from(ticker[i] - pos[i]).pow(2)).sum();
    let valid = d2 < 128 * 128 && dim == lv.dimension;
    let t = Text::translate(
        if valid {
            "create.redstone_requester.keeper_connected"
        } else {
            "create.redstone_requester.keeper_too_far_away"
        },
        Vec::new(),
    )
    .with_style(keeper::style(if valid { "white" } else { "red" }, false));
    let _ = player.action_bar(lv.host, t);
    let yaw = player.entity().rotation(&*lv.host).map_or(0.0, |r| r[0]);
    let facing = create_engine::place::horizontal_direction(yaw).opposite();
    let occluded = occluded(lv, pos, facing);
    set_has_entity(lv, pos, true);
    let host: &dyn Host = &*lv.host;
    cloths(lv.dim, |m| {
        m.insert(
            pos,
            Cloth {
                facing,
                occluded,
                shop: Some(Shop {
                    item_data: item_data(host, &order, &address, ticker, &dim),
                    order,
                    address,
                    ticker,
                    dim,
                    owner: player.uuid(),
                }),
                ..Cloth::default()
            },
        );
    });
    show(lv, pos);
    crate::glue::advancements::award(lv.host, player, "table_cloth_shop");
}

/// `TableClothBlock.getDrops`: a shop's cloth drops programmed again
/// (`AutoRequestData.writeToItem`), glinting.
pub(super) fn drops(dim: usize, pos: [i32; 3], drops: &mut [mod_api::ItemDrop]) {
    let Some(shop) = cloths(dim, |m| m.get(&pos).and_then(|c| c.shop.clone())) else {
        return;
    };
    // A cloth drops itself alone (what lay on it drops when it goes).
    let Some(d) = drops.first_mut() else { return };
    items::set_custom(&mut d.stack, &[(super::AUTO_REQUEST, Some(shop.item_data))]);
    let _ = d.stack.set(&super::Glint, true);
}

// ---------------------------------------------------------------------------------------------
// The price tag

/// The price of the shop at `pos` as the clipboard copies it (`Filtering`).
pub(crate) fn price_of(dim: usize, pos: [i32; 3]) -> Option<ItemStack> {
    cloths(dim, |m| m.get(&pos).and_then(|c| c.price.clone()))
}

/// `TableClothFilteringBehaviour.setFilter` (the predicate: no filter item, no shopping list;
/// the amount kept): whether it took it.
fn set_price(lv: &mut ServerLevel<'_>, pos: [i32; 3], stack: Option<ItemStack>) -> bool {
    let host = &*lv.host;
    if let Some(s) = &stack {
        let id = s.item().id(host);
        if create_engine::filter::FilterItem::of(&id).is_some() || is_list(host, s) {
            return false;
        }
    }
    let one = stack.map(|s| body::with_count(&s, 1).unwrap_or(s));
    let changed = cloths(lv.dim, |m| {
        let c = m.get_mut(&pos)?;
        c.price = one;
        Some(())
    });
    if changed.is_some() {
        show(lv, pos);
    }
    changed.is_some()
}

/// The amount of the price of the shop at `pos`.
pub(crate) fn price_count(dim: usize, pos: [i32; 3]) -> Option<i32> {
    cloths(dim, |m| m.get(&pos).map(|c| c.price_count as i32))
}

/// Whether `player` may paste a price onto the cloth at `pos`: a shop, and its owner.
pub(crate) fn may_paste(dim: usize, pos: [i32; 3], player: u128) -> bool {
    cloths(dim, |m| {
        m.get(&pos)
            .is_some_and(|c| c.shop.is_some() && may_price(c, player))
    })
}

/// The clipboard's paste of a price (`readFromClipboard`).
pub(crate) fn paste_price(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    stack: Option<ItemStack>,
) -> bool {
    set_price(lv, pos, stack)
}

/// The price tag's value box (`interaction::values`): a short click sets the payment item from
/// the main hand (`FilteringBehaviour.onShortInteract`, the owner only: the item frame's sound,
/// or Create's message and deny for an item it refuses), a pick on its board the amount (1 to
/// 100). The amount after.
pub(crate) fn value_settings(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: PlayerRef,
    pick: Option<create_engine::board::Settings>,
    short: bool,
) -> Option<i32> {
    let c = cloths(lv.dim, |m| m.get(&pos).cloned())?;
    c.shop.as_ref()?;
    if !may_price(&c, player.uuid()) {
        return Some(c.price_count as i32);
    }
    if short {
        let held = items::held(&*lv.host, player, Hand::Main);
        let id = held.as_ref().map(|s| s.item().id(&*lv.host));
        if !matches!(
            id.as_deref(),
            Some("create:wrench" | "create:mechanical_arm")
        ) {
            if set_price(lv, pos, held) {
                sound(
                    lv,
                    "minecraft:entity.item_frame.add_item",
                    centre(pos),
                    0.25,
                    0.1,
                );
            } else {
                let t = Text::translate("create.logistics.filter.invalid_item", Vec::new());
                let _ = player.action_bar(lv.host, t);
                let at = player.entity().position(&*lv.host).unwrap_or(centre(pos));
                sound(lv, "create:deny", at, 1.0, 1.0);
            }
        }
    }
    if let Some(p) = pick {
        let count = p.value.clamp(1, 100) as u32;
        if count != c.price_count {
            cloths(lv.dim, |m| {
                if let Some(c) = m.get_mut(&pos) {
                    c.price_count = count;
                }
            });
            show(lv, pos);
            // `playFeedbackSound`.
            sound(lv, "minecraft:block.note_block.hat", centre(pos), 0.25, 2.0);
        }
    }
    cloths(lv.dim, |m| m.get(&pos).map(|c| c.price_count as i32))
}

// ---------------------------------------------------------------------------------------------
// Clients

/// The cloth at `pos` as its block entity's data tells this client.
pub(crate) fn client_cloth(
    host: &dyn ClientHost,
    pos: [i32; 3],
) -> Option<(Dir, bool, bool, u128)> {
    let level = host.level(host.dimension())?;
    let data = host.block_entity(level, pos)?.data(host)?;
    let Some(Tag::Compound(c)) = data.get(&DATA.into()) else {
        return None;
    };
    let shop = match c.get(&"Shop".into()) {
        Some(Tag::Compound(s)) => Some(st::network_from_ints(&ints(s.get(&"Owner".into())))?),
        _ => None,
    };
    Some((
        from_2d(int(c.get(&"Facing".into())).unwrap_or(0)),
        matches!(c.get(&"Occluded".into()), Some(Tag::Byte(b)) if *b != 0),
        shop.is_some(),
        shop.unwrap_or(0),
    ))
}

/// Whether a click on the cloth at `pos` (`local` in it) lands on its price tag's value box, for
/// this client's player (`targetsPriceTag`).
pub(crate) fn client_price_hit(host: &dyn ClientHost, pos: [i32; 3], local: [f64; 3]) -> bool {
    let Some((facing, occluded, shop, owner)) = client_cloth(host, pos) else {
        return false;
    };
    let me = host.local_player().map(|p| p.uuid()).unwrap_or(0);
    shop && (owner == 0 || owner == me) && hits_price(facing, occluded, local)
}

/// The amount the price tag of the cloth at `pos` shows to this client.
pub(crate) fn client_price_count(host: &dyn ClientHost, pos: [i32; 3]) -> Option<i32> {
    let level = host.level(host.dimension())?;
    let data = host.block_entity(level, pos)?.data(host)?;
    let Some(Tag::Compound(c)) = data.get(&DATA.into()) else {
        return None;
    };
    Some(int(c.get(&"PriceCount".into())).unwrap_or(1))
}

/// The cloths' block entity: its data to clients, and `TableClothRenderer`.
pub(super) struct ClothEntity;

#[mod_api::horizon_part]
impl BlockPart for ClothEntity {}

#[mod_api::horizon_part]
impl BlockEntityPart for ClothEntity {
    fn block_entity_update_tag(
        &self,
        event: &mut BlockEntityUpdateTagEvent<'_>,
    ) -> Option<Compound> {
        // The shop and its items, and the sides its neighbours cull for the corners
        // (`decor::cloth`, drawn by its part on this type).
        let shown: Compound = [DATA, crate::glue::decor::cloth::CULL]
            .into_iter()
            .filter_map(|k| Some((k.into(), event.entity.nbt.get(&k.into())?.clone())))
            .collect();
        (!shown.is_empty()).then_some(shown)
    }

    /// `TableClothRenderer`: a shop's price tag (on top or down its side), and its items (the
    /// shop's first four of its order, else those set down): one in the middle, more round it,
    /// 5 px out; flat items turned to face the way the cloth does. The tag's payment item and
    /// amount sit on its value box (`FilteringRenderer`).
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some(Tag::Compound(c)) = event.entity.nbt.get(&DATA.into()) else {
            return;
        };
        let cloth = cloth_of(host, c);
        let draw = &mut *event.draw;
        let rot = 180.0
            - match cloth.facing {
                Dir::West => 90.0,
                Dir::North => 180.0,
                Dir::East => 270.0,
                _ => 0.0,
            };
        let centre = [0.5f32; 3];
        let stacks: Vec<ItemStack> = match &cloth.shop {
            Some(s) => s.order.iter().take(4).map(|(k, _)| k.0.clone()).collect(),
            None => cloth.items.clone(),
        };
        if cloth.shop.is_some() {
            let name = if cloth.occluded {
                "part_table_cloth_price_top"
            } else {
                "part_table_cloth_price_side"
            };
            if let Some(tag) = states::partial(name) {
                draw.push();
                draw.rotate_about([0.0, 1.0, 0.0], rot, centre);
                draw.block(tag, &[("facing", "up")]);
                draw.pop();
            }
            if let Some(price) = &cloth.price {
                draw_price(host, draw, &cloth, price);
            }
        }
        draw.push();
        draw.rotate_about([0.0, 1.0, 0.0], rot, centre);
        let n = stacks.len();
        for (i, s) in stacks.iter().enumerate() {
            draw.push();
            draw.translate([0.5, 3.0 / 16.0, 0.5]);
            if n > 1 {
                let a = i as f32 * (360.0 / n as f32) + 45.0;
                draw.rotate_about([0.0, 1.0, 0.0], a, [0.0; 3]);
                draw.translate([0.0, if i % 2 == 0 { -0.005 } else { 0.0 }, 5.0 / 16.0]);
                draw.rotate_about([0.0, 1.0, 0.0], -a, [0.0; 3]);
            }
            // `isGui3d`: an item that places a block stands for a 3D model.
            let block_item = host.item_block(s.item()).is_some();
            if !block_item {
                draw.rotate_about([0.0, 1.0, 0.0], -rot + 180.0, [0.0; 3]);
            }
            depot_item(draw, s, block_item);
            draw.pop();
        }
        draw.pop();
        draw.keep();
    }
}

/// `DepotRenderer.renderItem` of one item at rest (`renderUpright`): a block item standing at
/// half size, a flat one lying at half size.
fn depot_item(draw: &mut dyn mod_api::BlockEntityDrawContext, s: &ItemStack, block_item: bool) {
    draw.push();
    if block_item {
        draw.translate([0.0, 0.25 * 0.5 - 1.0 / 64.0, 0.0]);
    } else {
        draw.translate([0.0, 1.0 / 32.0, 0.0]);
        draw.rotate_about([1.0, 0.0, 0.0], 90.0, [0.0; 3]);
    }
    draw.scale([0.5; 3]);
    draw.item(s, ItemDisplayContext::Fixed, None, 0);
    draw.pop();
}

/// `FilteringRenderer` on the price tag's box (`TableClothFilterSlot.rotate`: turned to the
/// cloth's facing, tipped flat on top): the payment item at the box's scale.
fn draw_price(
    host: &dyn ClientHost,
    draw: &mut dyn mod_api::BlockEntityDrawContext,
    cloth: &Cloth,
    price: &ItemStack,
) {
    let slot = price_slot(cloth.facing, cloth.occluded);
    let yrot = 180.0
        - match cloth.facing {
            Dir::West => 90.0,
            Dir::North => 180.0,
            Dir::East => 270.0,
            _ => 0.0,
        };
    let solid = host.item_block(price.item()).is_some();
    let scale = 0.5 * (if solid { 1.0 } else { 0.5 } + 1.0 / 64.0);
    draw.push();
    draw.translate(slot.map(|v| v as f32));
    draw.rotate_about([0.0, 1.0, 0.0], yrot, [0.0; 3]);
    if cloth.occluded {
        draw.rotate_about([1.0, 0.0, 0.0], 90.0, [0.0; 3]);
    }
    draw.scale([scale; 3]);
    if !solid {
        draw.translate([0.0, 0.0, -0.15]);
    }
    draw.item(price, ItemDisplayContext::Fixed, None, 0);
    draw.pop();
}

/// The cloths' block entity type, on every table cloth.
pub(super) fn register(r: &mut dyn mod_api::Registrar) {
    let names: Vec<String> = Block::ALL
        .iter()
        .filter(|b| is_cloth(**b))
        .map(|b| format!("create:{}", b.name()))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let kind = r.block_entity_type(TYPE).blocks(&refs).build();
    if let Err(e) = r.block_entity_part(&kind, ClothEntity) {
        crate::create_log!("[create] the table cloth block entity's part was refused: {e:?}");
    }
    for n in &names {
        let key = r.item_key(n);
        r.item_part(key, ClothItem);
    }
    let list = r.item_key("create:shopping_list");
    r.item_part(list, ShoppingListPart);
}

/// The cloth items: a programmed one placed becomes a shop (`setPlacedBy`); its tooltip.
struct ClothItem;

#[mod_api::horizon_part]
impl mod_api::ItemPart for ClothItem {
    fn on_block_placed_by(
        &self,
        host: &mut dyn mod_api::ServerHost,
        event: &mut mod_api::OnBlockPlacedByEvent<'_>,
    ) {
        let Some(player) = event.placer else { return };
        let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
            return;
        };
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        placed(&mut lv, player, event.pos, event.stack);
    }

    /// `TableClothBlockItem.appendHoverText`: a programmed cloth says so and lists its order.
    fn tooltip(&self, host: &dyn ClientHost, event: &mut mod_api::TooltipEvent<'_>) {
        let custom = items::custom(event.stack);
        let Some(Tag::Compound(c)) = custom.get(&super::AUTO_REQUEST.into()) else {
            return;
        };
        let line = |key: &str| Text::translate(key, Vec::new());
        event
            .lines
            .push((line("create.table_cloth.shop_configured"), 0xFFAA00));
        for (k, n) in order_of(host, c.get(&"EncodedRequest".into())) {
            let name = host.hover_name(&k.0);
            event
                .lines
                .push((Text::literal(format!("{name} x{n}")), 0xAAAAAA));
        }
        event
            .lines
            .push((line("create.logistically_linked.tooltip_clear"), 0x555555));
    }
}

/// The shopping list: thrown away when used sneaking; its tooltip lists what it orders and
/// costs.
struct ShoppingListPart;

#[mod_api::horizon_part]
impl mod_api::ItemPart for ShoppingListPart {
    fn use_item(
        &self,
        host: mod_api::HostSide,
        event: &mut mod_api::UseItemEvent<'_>,
    ) -> Option<InteractionResult> {
        match host {
            mod_api::HostSide::Server(host) => {
                let player = host.player(event.player)?;
                discard_list(host, player, event.hand).then_some(InteractionResult::Success)
            }
            mod_api::HostSide::Client(host) => {
                let player = host.player(event.player)?;
                (event.hand == Hand::Main && items::sneaking(&*host, player))
                    .then_some(InteractionResult::Success)
            }
        }
    }

    fn use_on(
        &self,
        host: mod_api::HostSide,
        event: &mut mod_api::UseOnEvent<'_>,
    ) -> Option<InteractionResult> {
        match host {
            mod_api::HostSide::Server(host) => {
                let player = host.player(event.player)?;
                discard_list(host, player, event.hand).then_some(InteractionResult::Success)
            }
            mod_api::HostSide::Client(host) => {
                let player = host.player(event.player)?;
                (event.hand == Hand::Main && items::sneaking(&*host, player))
                    .then_some(InteractionResult::Success)
            }
        }
    }

    /// `ShoppingListItem.appendHoverText`: the order (grey) and its total cost (gold and
    /// yellow), read from the cloths this client sees; then the hints.
    fn tooltip(&self, host: &dyn ClientHost, event: &mut mod_api::TooltipEvent<'_>) {
        if let Some(list) = List::of(event.stack) {
            let (order, cost) = client_bake(host, &list);
            for (cost_side, entries) in [(false, order), (true, cost)] {
                if cost_side {
                    event.lines.push((Text::literal(""), 0xFFFFFF));
                }
                let colour = if cost_side { 0xFFFF55 } else { 0xAAAAAA };
                if entries.len() == 1 {
                    let (s, n) = &entries[0];
                    let head = if cost_side {
                        host.translate("create.table_cloth.total_cost")
                            .unwrap_or_else(|| "Total cost: ".into())
                    } else {
                        String::new()
                    };
                    let line = format!("{head}{} x{n}", host.hover_name(s));
                    event.lines.push((Text::literal(line), colour));
                } else {
                    if cost_side {
                        event.lines.push((
                            Text::translate("create.table_cloth.total_cost", Vec::new()),
                            0xFFAA00,
                        ));
                    }
                    for (s, n) in entries {
                        let line = format!("{} x{n}", host.hover_name(&s));
                        event.lines.push((Text::literal(line), colour));
                    }
                }
            }
        }
        event.lines.push((
            Text::translate("create.table_cloth.hand_to_shop_keeper", Vec::new()),
            0xAAAAAA,
        ));
        event.lines.push((
            Text::translate("create.table_cloth.sneak_click_discard", Vec::new()),
            0x555555,
        ));
    }
}

/// Stacks and how many of each, most first.
type Counted = Vec<(ItemStack, i32)>;

/// `bakeEntries` on a client, from the cloths' block entity data: the order and the cost, most
/// first.
fn client_bake(host: &dyn ClientHost, list: &List) -> (Counted, Counted) {
    let (mut out, mut cost): (Summary<Key>, Summary<Key>) = Default::default();
    let Some(level) = host.level(host.dimension()) else {
        return (Vec::new(), Vec::new());
    };
    for (p, n) in &list.purchases {
        let Some(data) = host.block_entity(level, *p).and_then(|e| e.data(host)) else {
            continue;
        };
        let Some(Tag::Compound(c)) = data.get(&DATA.into()) else {
            continue;
        };
        let cloth = cloth_of(host, c);
        let Some(shop) = cloth.shop else { continue };
        if let Some(price) = &cloth.price {
            cost.add(&Key::of(price), (cloth.price_count * n) as i32);
        }
        for (k, count) in &shop.order {
            out.add(k, (count * n) as i32);
        }
    }
    let flat = |s: Summary<Key>| s.by_count().into_iter().map(|(k, n)| (k.0, n)).collect();
    (flat(out), flat(cost))
}

// ---------------------------------------------------------------------------------------------
// The overlay

/// `TableClothOverlayRenderer.tick` and `BlueprintOverlayRenderer.displayClothShop` /
/// `renderOverlay`: looking at a shop (not with a clipboard, not at its price tag), the HUD
/// shows the price (lit while it is set and stock is left beyond the purchases on the list in
/// hand), an arrow (red while the shop is unusable) and what one purchase gives, over the
/// trade backdrop, with the list's purchases of it.
pub(super) fn render_overlay(host: &dyn ClientHost, draw: &mut dyn mod_api::DrawContext) {
    if host.mod_screen().is_some() || host.screen_title().is_some() {
        return;
    }
    let target = host.crosshair_target();
    if let Some(mod_api::CrosshairTarget::Entity { entity, .. }) = target {
        checkout_overlay(host, draw, entity);
        return;
    }
    let Some(mod_api::CrosshairTarget::Block { pos, hit, .. }) = target else {
        return;
    };
    let pos = [pos.x, pos.y, pos.z];
    let Some(player) = host.local_player() else {
        return;
    };
    if player.game_mode(host) == Some(mod_api::GameMode::Spectator) {
        return;
    }
    let Some(level) = host.level(host.dimension()) else {
        return;
    };
    let Some(data) = host.block_entity(level, pos).and_then(|e| e.data(host)) else {
        return;
    };
    let Some(Tag::Compound(c)) = data.get(&DATA.into()) else {
        return;
    };
    let cloth = cloth_of(host, c);
    let Some(shop) = &cloth.shop else { return };
    let held = items::held(host, player, Hand::Main);
    if held
        .as_ref()
        .is_some_and(|s| s.item().id(host) == "create:clipboard")
    {
        return;
    }
    let local = [0, 1, 2].map(|i| hit.point[i] - f64::from(pos[i]));
    if client_price_hit(host, pos, local) {
        return;
    }
    let list = held.as_ref().and_then(List::of);
    let purchases = list.as_ref().map_or(0, |l| l.purchases(pos));
    // `getStockLevelForTrade` with the list's other purchases taken off.
    let modifier = list.as_ref().map(|l| {
        let mut other = l.clone();
        other.purchases.retain(|e| e.0 != pos);
        client_bake(host, &other).0
    });
    let mut level_left = i32::MAX;
    for (i, (k, n)) in shop.order.iter().enumerate() {
        if *n == 0 {
            continue;
        }
        let have = cloth.stock.get(i).copied().unwrap_or(0);
        let taken: i32 = modifier
            .iter()
            .flatten()
            .filter(|(s, _)| Key::of(s) == *k)
            .map(|(_, n)| *n)
            .sum();
        level_left = level_left.min((have - taken) / *n as i32);
    }
    let stock_level = level_left.max(0) as u32;
    let price = cloth
        .price
        .as_ref()
        .and_then(|p| body::with_count(p, cloth.price_count));
    let invalid = price.is_none() || stock_level == 0;
    let size = draw.size();
    let w = 21.0 * (1 + shop.order.len()) as f32 + 30.0;
    let mut x = ((size[0] - w) / 2.0).floor();
    let y = size[1] - 100.0;
    // `TooltipRenderUtil.renderTooltipBackground` in 0x55000000.
    draw.fill([x - 2.0, y + 1.0, w + 4.0, 19.0], 0x5500_0000);
    let mid = (size[0] / 2.0).floor();
    draw.sprite(
        "create:gui/widgets",
        [128.0, 98.0, 96.0, 46.0],
        [mid - 48.0, y - 19.0, 96.0, 46.0],
        None,
    );
    if purchases > 0
        && let Some(list_item) = body::new_stack(host, "create:shopping_list", 1)
    {
        draw.item_stack(&list_item, [mid + 20.0, y - 20.0], 16.0);
        draw.text(
            &format!("x{purchases}"),
            [mid + 36.0, y - 16.0],
            0xFFEE_EEEE,
            true,
        );
    }
    // The price.
    let affordable = price.is_some() && stock_level > purchases;
    let slot = |active: bool| {
        if active {
            [0.0, 46.0, 22.0, 22.0]
        } else {
            [0.0, 68.0, 22.0, 22.0]
        }
    };
    draw.sprite(
        "create:gui/widgets",
        slot(affordable),
        [x, y, 22.0, 22.0],
        None,
    );
    if let Some(p) = &price {
        draw.gui_draw_item_with(p, [x + 3.0, y + 3.0], None);
    }
    x += 21.0 + 5.0;
    // The arrow.
    if invalid {
        draw.sprite(
            "create:gui/widgets",
            [52.0, 51.0, 20.0, 15.0],
            [x, y + 4.0, 20.0, 15.0],
            None,
        );
    } else {
        draw.sprite(
            "create:gui/widgets",
            [24.0, 51.0, 20.0, 12.0],
            [x, y + 4.0, 20.0, 12.0],
            None,
        );
    }
    x += 25.0;
    // What it gives.
    let mut results = Vec::new();
    for (k, n) in &shop.order {
        let lit = !invalid && stock_level > purchases;
        draw.sprite("create:gui/widgets", slot(lit), [x, y, 22.0, 22.0], None);
        let stack = body::with_count(&k.0, *n);
        if let Some(s) = &stack {
            draw.gui_draw_item_with(s, [x + 3.0, y + 3.0], None);
        }
        results.push(stack);
        x += 21.0;
    }
    // Each result's tooltip in turn, 40 GUI ticks each, at the screen's corner. As Create's:
    // the turn counts only results with more than their name, but is matched against every
    // result's index.
    let tips: Vec<Vec<Text>> = results
        .iter()
        .map(|s| {
            s.as_ref()
                .map(|s| items::tooltip_lines(host, s))
                .unwrap_or_default()
        })
        .collect();
    let cycle = tips.iter().filter(|t| t.len() > 1).count() as u32;
    if cycle > 0 {
        let turn = (GUI_TICKS.with(Cell::get) / 40) % cycle;
        for (i, lines) in tips.iter().enumerate() {
            if lines.len() > 1 && turn == i as u32 {
                let _ = draw.gui_tooltip(lines, size);
            }
        }
    }
}

thread_local! {
    /// `Gui.tickCount`: the client ticks run while the game is not paused.
    static GUI_TICKS: Cell<u32> = const { Cell::new(0) };
}

/// A client tick: `Gui.tick` counts it unless the game is paused.
pub(super) fn tick(host: &dyn ClientHost) {
    if !host.is_paused() {
        GUI_TICKS.with(|t| t.set(t.get().wrapping_add(1)));
    }
}

/// Whether `keeper` rides something (`getVehicle() != null`) as its client sees it. A client
/// can't ask an entity's vehicle (`EntityRef::vehicle` answers on the server only) nor list the
/// seat's stand-in (a display, which the client's entity search leaves out), so a keeper counts
/// as seated where a seat puts its rider: at the centre of a seat block, no higher than the
/// stand-in (`seat::seat_at`; a rider hangs its own vehicle attachment below it), where a mob
/// standing on the seat would be half a block up.
fn seated(host: &dyn ClientHost, keeper: mod_api::EntityRef) -> bool {
    if keeper.vehicle(host).is_some() {
        return true;
    }
    let (Some(at), Some(level)) = (keeper.position(host), host.level(host.dimension())) else {
        return false;
    };
    let kind = keeper.kind(host).map(|k| k.id(host)).unwrap_or_default();
    let floor = at.map(|v| v.floor() as i32);
    [floor, [floor[0], floor[1] + 1, floor[2]]]
        .into_iter()
        .any(|p| {
            let seat = level
                .block_state(host, p)
                .and_then(states::id)
                .and_then(|id| {
                    super::super::client_table(host, |t| t.get(id).map(|(b, _)| b)).flatten()
                })
                .is_some_and(create_engine::dye::is_seat);
            let s = create_engine::seat::seat_at(p, &kind);
            seat && (at[0] - s[0]).abs() < 1e-3
                && (at[2] - s[2]).abs() < 1e-3
                && at[1] <= s[1] + 1e-3
                && at[1] > s[1] - 1.0
        })
}

/// `TableClothOverlayRenderer.tick` on a seated keeper with a shopping list in hand, and
/// `BlueprintOverlayRenderer.displayShoppingList`: what the list costs (each lit while the
/// inventory holds it, else its count in gold) and what it orders. The keeper is a riding mob
/// with one stock ticker beside it or a block above that (`getStockTickerPosition`).
fn checkout_overlay(
    host: &dyn ClientHost,
    draw: &mut dyn mod_api::DrawContext,
    keeper: mod_api::EntityRef,
) {
    let Some(player) = host.local_player() else {
        return;
    };
    let Some(held) = items::held(host, player, Hand::Main) else {
        return;
    };
    let Some(list) = List::of(&held) else { return };
    if !seated(host, keeper)
        || keeper.kind(host).map(|k| k.id(host)).as_deref() == Some("create:package")
    {
        return;
    }
    let Some(at) = keeper.block_position(host) else {
        return;
    };
    let Some(level) = host.level(host.dimension()) else {
        return;
    };
    let mut tickers = Vec::new();
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        for dy in [0, 1] {
            let p = [at[0] + dx, at[1] + dy, at[2] + dz];
            if level
                .block_state(host, p)
                .is_some_and(|s| s.block() == states::key(Block::StockTicker))
            {
                tickers.push(p);
            }
        }
    }
    let [ticker] = tickers[..] else { return };
    // `tickerBE.behaviour.freqId.equals(list.shopNetwork())`: the keeper serves the list's shop.
    let network = host
        .block_entity(level, ticker)
        .and_then(|e| e.data(host))
        .and_then(|d| {
            crate::glue::kinetics::synced(&d, super::TICKER_NETWORK)
                .map(|t| st::network_from_ints(&ints(Some(t))))
        })
        .flatten();
    if network != Some(list.network) {
        return;
    }
    let (order, cost) = client_bake(host, &list);
    let inventory: Vec<ItemStack> = player
        .inventory_slots(host)
        .unwrap_or_default()
        .into_iter()
        .filter(|(i, _)| *i < 36)
        .map(|(_, s)| s)
        .collect();
    let size = draw.size();
    let w = 21.0 * (cost.len() + order.len()) as f32 + 30.0;
    let mut x = ((size[0] - w) / 2.0).floor();
    let y = size[1] - 100.0;
    draw.fill([x - 2.0, y + 1.0, w + 4.0, 19.0], 0x5500_0000);
    let mid = (size[0] / 2.0).floor();
    draw.sprite(
        "create:gui/widgets",
        [128.0, 98.0, 96.0, 46.0],
        [mid - 48.0, y - 19.0, 96.0, 46.0],
        None,
    );
    let slot = |active: bool| {
        if active {
            [0.0, 46.0, 22.0, 22.0]
        } else {
            [0.0, 68.0, 22.0, 22.0]
        }
    };
    for (s, n) in &cost {
        // `canAfford`: the same item and components, enough of it in the inventory.
        let have: i32 = inventory
            .iter()
            .filter(|i| i.is_stackable_with(s))
            .map(|i| i.count() as i32)
            .sum();
        let afford = have >= *n;
        draw.sprite("create:gui/widgets", slot(afford), [x, y, 22.0, 22.0], None);
        if let Some(st) = body::with_count(s, (*n).max(1) as u32) {
            let count = (!afford).then(|| format!("\u{a7}6{n}"));
            draw.gui_draw_item_with(&st, [x + 3.0, y + 3.0], count.as_deref());
        }
        x += 21.0;
    }
    x += 5.0;
    let invalid = cost.is_empty();
    if invalid {
        draw.sprite(
            "create:gui/widgets",
            [52.0, 51.0, 20.0, 15.0],
            [x, y + 4.0, 20.0, 15.0],
            None,
        );
    } else {
        draw.sprite(
            "create:gui/widgets",
            [24.0, 51.0, 20.0, 12.0],
            [x, y + 4.0, 20.0, 12.0],
            None,
        );
    }
    x += 25.0;
    for (s, n) in &order {
        draw.sprite(
            "create:gui/widgets",
            slot(!invalid),
            [x, y, 22.0, 22.0],
            None,
        );
        if let Some(st) = body::with_count(s, (*n).max(1) as u32) {
            draw.gui_draw_item_with(&st, [x + 3.0, y + 3.0], None);
        }
        x += 21.0;
    }
}
