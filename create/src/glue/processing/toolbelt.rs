//! Toolboxes worn as a tool belt (`ToolboxHandler`, `ToolboxHandlerClient`, `RadialToolboxMenu`,
//! `ToolboxEquipPacket`, `ToolboxDisposeAllPacket`, `ToolboxBlockEntity.tickPlayers`), and a
//! toolbox keeping its contents as an item (`ToolboxBlock.attack`, `getCloneItemStack`, its loot's
//! `copy_components` of `TOOLBOX_INVENTORY`, `setPlacedBy`).
//!
//! Holding the tool belt key (left Alt) opens the radial menu over the nearest eight toolboxes in
//! reach (10 blocks): with one, its eight compartments in a ring; with more, the toolboxes first
//! (right-click goes back to them). Choosing a compartment equips it in the selected hotbar slot:
//! what was there goes back to a toolbox or the inventory, and the slot is kept at half a stack
//! from the compartment while the toolbox is in reach (`tickPlayers`). The centre unequips the
//! slot (its items back to the toolbox), or out of reach stops tracking it; the right-hand button
//! returns the inventory to the toolbox (or every toolbox near). The hotbar shows which slots are
//! equipped, lit while their toolbox is in reach. The equipped slots are kept on the player
//! (`CreateToolboxData` in their persistent data) and sent to their client when they change.
//!
//! A compartment's item kind (`ToolboxInventory.filters`) is the first stack put into it, kept
//! with the toolbox's column (and on its item) once set, and what shares it is Create's
//! (`canItemsShareCompartment`: the same item and components, tools however worn, any belt
//! connectors). Each toolbox has its `UniqueId`, kept with it and on its item, by which the menu
//! orders the nearest eight. A left click picks a toolbox up with its contents into the
//! inventory; broken by a player, an explosion or a machine of the port's, its drop keeps them.
//! While a toolbox's menu is open, a slot it would refill or take from is let go instead
//! (`isOpenInContainer`), and out of creative the pick-block key equips an item a toolbox in reach
//! holds (`onPickItem`, from a block; the server's pick names no entity's item).
//!
//! A toolbox replaced by the game (`/setblock`, `/fill`) loses its contents, as Create's
//! (`onRemove` drops nothing), and a piston can't move it. Left out (no API for it): one the level
//! destroys with its drops (`/setblock .. destroy`, `Level.destroyBlock`) drops an empty toolbox,
//! where Create's loot copies its contents (`BlockDropsEvent` gives no read of the block entity;
//! `API_GAPS.md`).

use super::super::net;
use super::super::persist::mark_pos_unsaved;
use super::super::server::ServerLevel;
use super::super::world::level_index;
use create_engine::toolbox::{self as tb, STACKS_PER_COMPARTMENT};
use create_engine::{Block, dye};
use mod_api::{
    ClientHost, Compound, DrawContext, Flow, ItemStack, PlayerRef, ServerHost, Tag, Text,
};
use std::cell::RefCell;
use std::collections::HashMap;

pub(crate) const CHANNEL: &str = "create:toolbelt";
/// The player's persistent data key (`CreateToolboxData`).
const DATA: &str = "CreateToolboxData";
/// The toolbox item's custom data key for its contents (`TOOLBOX_INVENTORY`).
const ITEM_KEY: &str = "create:toolbox_inventory";
/// The toolbox's id, on its item (`TOOLBOX_UUID`) and with its column (`UniqueId`).
const UUID_KEY: &str = "create:toolbox_uuid";
const COMPARTMENTS: usize = 8;
const MOD: &str = "create";
pub(crate) const KEY: &str = "toolbelt";

/// Each compartment's kind (`ToolboxInventory.filters`): the first stack put into it, one of it,
/// as its item NBT.
type Filters = [Option<Compound>; COMPARTMENTS];

/// What the port keeps of a toolbox besides its slots: its compartments' kinds and its
/// `UniqueId` (`ToolboxBlockEntity.uniqueId`, by which the radial menu orders toolboxes).
#[derive(Clone, Default)]
struct Kept {
    filters: Filters,
    uuid: Option<u128>,
}

/// The toolboxes' item keys, requested at registration.
pub(crate) static TOOLBOX_ITEMS: std::sync::OnceLock<Vec<mod_api::ItemKey>> =
    std::sync::OnceLock::new();

thread_local! {
    /// Each toolbox's compartment kinds and id, by level and position.
    static FILTERS: RefCell<HashMap<(usize, [i32; 3]), Kept>> = RefCell::new(HashMap::new());
    /// Contents taken out of a toolbox about to be broken, for its drop (`block_drops`).
    static BREAKING: RefCell<HashMap<(usize, [i32; 3]), Compound>> = RefCell::new(HashMap::new());
}

pub(crate) fn clear() {
    FILTERS.with_borrow_mut(HashMap::clear);
    BREAKING.with_borrow_mut(HashMap::clear);
}

fn is_toolbox(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    lv.info(pos).is_some_and(|(b, _)| dye::is_toolbox(b))
}

fn stored(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Vec<Option<ItemStack>> {
    lv.slots(pos, tb::STORED)
        .unwrap_or_else(|| vec![None; tb::STORED])
}

/// `ToolboxInventory.canItemsShareCompartment`: two unstackable damageable items of one kind
/// (tools worn differently), any two belt connectors, else the same item with the same
/// components.
fn same_kind(host: &dyn mod_api::Host, a: &ItemStack, b: &ItemStack) -> bool {
    let unstackable_tool = |s: &ItemStack| {
        host.max_stack_size(s.item()).unwrap_or(64) <= 1
            && host.max_damage(s.item()).unwrap_or(0) > 0
    };
    if unstackable_tool(a) && unstackable_tool(b) {
        return a.item() == b.item();
    }
    let connector = |s: &ItemStack| s.item().id(host) == "create:belt_connector";
    if connector(a) && connector(b) {
        return true;
    }
    a.is_stackable_with(b)
}

/// A kind from a stack: one of it, as NBT.
fn kind_of(host: &dyn mod_api::Host, s: &ItemStack) -> Option<Compound> {
    super::super::packages::body::with_count(s, 1)?.to_nbt(host)
}

/// A saved kind: the stack's NBT, or (as saves before kinds kept components) an item id.
fn read_kind(t: &Tag) -> Option<Compound> {
    match t {
        Tag::Compound(c) if !c.is_empty() => Some(c.clone()),
        Tag::String(s) if !s.to_string_lossy().is_empty() => Some(Compound::from([
            (
                "id".into(),
                Tag::String(s.to_string_lossy().as_str().into()),
            ),
            ("count".into(), Tag::Int(1)),
        ])),
        _ => None,
    }
}

fn write_kinds(f: &Filters) -> Tag {
    Tag::List(
        f.iter()
            .map(|k| Tag::Compound(k.clone().unwrap_or_default()))
            .collect(),
    )
}

fn read_kinds(t: Option<&Tag>) -> Filters {
    let mut f: Filters = Default::default();
    if let Some(Tag::List(l)) = t {
        for (i, t) in l.iter().take(COMPARTMENTS).enumerate() {
            f[i] = read_kind(t);
        }
    }
    f
}

/// A `UUID` as Java's codec writes it: four ints, most significant first.
fn uuid_tag(u: u128) -> Tag {
    Tag::IntArray((0..4).map(|i| (u >> (96 - 32 * i)) as u32 as i32).collect())
}

fn read_uuid(t: Option<&Tag>) -> Option<u128> {
    let Some(Tag::IntArray(v)) = t else {
        return None;
    };
    (v.len() == 4).then(|| v.iter().fold(0u128, |a, w| a << 32 | u128::from(*w as u32)))
}

/// `UUID.compareTo`: the most significant half, then the least, each as a signed long.
fn uuid_order(u: u128) -> (i64, i64) {
    ((u >> 64) as u64 as i64, u as u64 as i64)
}

fn kept(dim: usize, pos: [i32; 3]) -> Kept {
    FILTERS.with_borrow(|m| m.get(&(dim, pos)).cloned().unwrap_or_default())
}

/// The toolbox's id, made (`UUID.randomUUID`, from the level's random) for one that has none.
fn uuid(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> u128 {
    if let Some(u) = kept(lv.dim, pos).uuid {
        return u;
    }
    use mod_api::RandomSource;
    let words = lv
        .level
        .with_random(&mut *lv.host, |r| {
            [r.next_int(), r.next_int(), r.next_int(), r.next_int()]
        })
        .unwrap_or([pos[0], pos[1], pos[2], lv.game_time() as i32]);
    // Version 4, variant 2, as `randomUUID`.
    let mut u = words
        .iter()
        .fold(0u128, |a, w| a << 32 | u128::from(*w as u32));
    u = (u & !(0xF << 76)) | (0x4 << 76);
    u = (u & !(0x3 << 62)) | (0x2 << 62);
    let dim = lv.dim;
    FILTERS.with_borrow_mut(|m| m.entry((dim, pos)).or_default().uuid = Some(u));
    mark_pos_unsaved(dim, pos);
    u
}

/// The toolbox's compartment kinds: the kept ones, and for a compartment without one, the
/// kind of what is in it now (`setStackInSlot` / `insertItem` setting the filter).
fn filters(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Filters {
    let dim = lv.dim;
    let mut f = kept(dim, pos).filters;
    let slots = stored(lv, pos);
    let mut changed = false;
    for (c, filter) in f.iter_mut().enumerate() {
        if filter.is_some() {
            continue;
        }
        let first = (0..STACKS_PER_COMPARTMENT)
            .find_map(|i| slots[c * STACKS_PER_COMPARTMENT + i].as_ref());
        if let Some(s) = first {
            *filter = kind_of(&*lv.host, s);
            changed = true;
        }
    }
    if changed {
        FILTERS.with_borrow_mut(|m| m.entry((dim, pos)).or_default().filters = f.clone());
        mark_pos_unsaved(dim, pos);
    }
    f
}

fn filter_stack(lv: &ServerLevel<'_>, f: &Option<Compound>) -> Option<ItemStack> {
    ItemStack::from_nbt(&*lv.host, f.as_ref()?)
}

fn max_stack(lv: &ServerLevel<'_>, s: &ItemStack) -> u32 {
    lv.host.max_stack_size(s.item()).unwrap_or(64).max(1)
}

/// `settle`: a compartment's stacks packed to the front, full ones first.
fn settle(lv: &mut ServerLevel<'_>, pos: [i32; 3], c: usize, mut stacks: Vec<Option<ItemStack>>) {
    let Some(sample) = stacks.iter().flatten().next().cloned() else {
        let changes: Vec<(usize, Option<ItemStack>)> = (0..STACKS_PER_COMPARTMENT)
            .map(|i| (c * STACKS_PER_COMPARTMENT + i, None))
            .collect();
        lv.set_slots(pos, &changes);
        return;
    };
    let max = max_stack(lv, &sample);
    if max == 1 {
        stacks.sort_by_key(Option::is_none);
    } else {
        let mut total: u32 = stacks.iter().flatten().map(ItemStack::count).sum();
        for s in stacks.iter_mut() {
            let n = total.min(max);
            total -= n;
            *s = (n > 0).then(|| {
                let mut x = sample.clone();
                let _ = x.set_count(n);
                x
            });
        }
    }
    let changes: Vec<(usize, Option<ItemStack>)> = stacks
        .into_iter()
        .enumerate()
        .map(|(i, s)| (c * STACKS_PER_COMPARTMENT + i, s))
        .collect();
    lv.set_slots(pos, &changes);
}

fn compartment(lv: &ServerLevel<'_>, pos: [i32; 3], c: usize) -> Vec<Option<ItemStack>> {
    let all = stored(lv, pos);
    all[c * STACKS_PER_COMPARTMENT..(c + 1) * STACKS_PER_COMPARTMENT].to_vec()
}

/// `takeFromCompartment`: up to `amount` from the back of compartment `c`.
fn take(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    c: usize,
    amount: u32,
    simulate: bool,
) -> Option<ItemStack> {
    if amount == 0 {
        return None;
    }
    let mut stacks = compartment(lv, pos, c);
    let mut remaining = amount;
    let mut last = None;
    for s in stacks.iter_mut().rev() {
        let Some(st) = s.as_mut() else { continue };
        let n = st.count().min(remaining);
        remaining -= n;
        last = Some(st.clone());
        if n == st.count() {
            *s = None;
        } else {
            let _ = st.set_count(st.count() - n);
        }
        if remaining == 0 {
            break;
        }
    }
    let got = amount - remaining;
    if got == 0 {
        return None;
    }
    if !simulate {
        settle(lv, pos, c, stacks);
    }
    let mut out = last?;
    let _ = out.set_count(got);
    Some(out)
}

/// `distributeToCompartment`: `stack` into compartment `c` (of its kind); what is left.
fn distribute(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    c: usize,
    stack: ItemStack,
    simulate: bool,
) -> Option<ItemStack> {
    let f = filters(lv, pos);
    let kind = filter_stack(lv, &f[c])?;
    if !same_kind(&*lv.host, &kind, &stack) {
        return Some(stack);
    }
    let max = max_stack(lv, &stack);
    let mut stacks = compartment(lv, pos, c);
    let mut left = stack.count();
    for s in stacks.iter_mut() {
        if left == 0 {
            break;
        }
        match s {
            None => {
                let n = left.min(max);
                let mut x = stack.clone();
                let _ = x.set_count(n);
                *s = Some(x);
                left -= n;
            }
            Some(st) if st.is_stackable_with(&stack) && st.count() < max => {
                let n = left.min(max - st.count());
                let _ = st.set_count(st.count() + n);
                left -= n;
            }
            _ => {}
        }
    }
    if !simulate && left != stack.count() {
        settle(lv, pos, c, stacks);
    }
    (left > 0).then(|| {
        let mut r = stack;
        let _ = r.set_count(left);
        r
    })
}

/// `inLimitedMode` + `insertItemStacked`: `stack` into the compartments of its kind (those with
/// one); what is left.
fn insert_limited(lv: &mut ServerLevel<'_>, pos: [i32; 3], stack: ItemStack) -> Option<ItemStack> {
    let f = filters(lv, pos);
    let mut left = Some(stack);
    for (c, kind) in f.iter().enumerate() {
        let Some(s) = left.take() else { break };
        if filter_stack(lv, kind).is_none_or(|k| !same_kind(&*lv.host, &k, &s)) {
            left = Some(s);
            continue;
        }
        left = distribute(lv, pos, c, s, false);
    }
    left
}

// ---------------------------------------------------------------------------------------------
// The equipped slots

/// `CreateToolboxData`: hotbar slot -> (toolbox, compartment).
fn equipped(host: &dyn ServerHost, player: PlayerRef) -> Vec<(usize, [i32; 3], usize)> {
    let Some(data) = player.persistent_data(host) else {
        return Vec::new();
    };
    let Some(Tag::Compound(c)) = data.get(&DATA.into()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for hotbar in 0..9usize {
        let Some(Tag::Compound(e)) = c.get(&hotbar.to_string().as_str().into()) else {
            continue;
        };
        let (Some(Tag::IntArray(p)), Some(Tag::Int(slot))) =
            (e.get(&"Pos".into()), e.get(&"Slot".into()))
        else {
            continue;
        };
        if p.len() == 3 {
            out.push((hotbar, [p[0], p[1], p[2]], (*slot).max(0) as usize));
        }
    }
    out
}

fn set_equipped(host: &mut dyn ServerHost, player: PlayerRef, list: &[(usize, [i32; 3], usize)]) {
    let mut data = player.persistent_data(&*host).unwrap_or_default();
    let mut c = Compound::new();
    for (hotbar, pos, slot) in list {
        let mut e = Compound::new();
        e.insert("Slot".into(), Tag::Int(*slot as i32));
        e.insert("Pos".into(), Tag::IntArray(pos.to_vec()));
        c.insert(hotbar.to_string().as_str().into(), Tag::Compound(e));
    }
    if c.is_empty() {
        data.remove(&DATA.into());
    } else {
        data.insert(DATA.into(), Tag::Compound(c));
    }
    let _ = player.set_persistent_data(host, data);
}

/// `syncData`: the equipped slots to the player's client.
fn sync(host: &mut dyn ServerHost, player: PlayerRef) {
    let list = equipped(&*host, player);
    let mut out = vec![0u8, list.len() as u8];
    for (hotbar, pos, slot) in list {
        out.push(hotbar as u8);
        for v in pos {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(slot as u8);
    }
    net::send_to_player(host, player, CHANNEL, &out);
}

fn hotbar_stack(host: &dyn ServerHost, player: PlayerRef, slot: usize) -> Option<ItemStack> {
    player
        .inventory_slots(host)
        .unwrap_or_default()
        .into_iter()
        .find(|(s, _)| *s as usize == slot)
        .map(|(_, st)| st)
}

fn position(host: &dyn ServerHost, player: PlayerRef) -> [f64; 3] {
    player.entity().position(host).unwrap_or([0.0; 3])
}

fn level_of<'a>(
    host: &'a mut dyn ServerHost,
    player: PlayerRef,
    dimension: &'a mut String,
) -> Option<ServerLevel<'a>> {
    *dimension = player.dimension(&*host)?.to_string();
    ServerLevel::new(host, dimension)
}

/// `ToolboxHandler.unequip`: the hotbar slot stops being equipped; unless `keep` (or out of
/// reach), what it holds goes back into its compartment.
fn unequip(lv: &mut ServerLevel<'_>, player: PlayerRef, hotbar: usize, keep: bool) {
    let mut list = equipped(&*lv.host, player);
    let Some(i) = list.iter().position(|e| e.0 == hotbar) else {
        return;
    };
    let (_, pos, slot) = list.remove(i);
    let in_range = tb::in_range(position(&*lv.host, player), pos);
    if !keep
        && in_range
        && is_toolbox(lv, pos)
        && let Some(stack) = hotbar_stack(&*lv.host, player, hotbar)
    {
        let before = stack.count();
        let left = distribute(lv, pos, slot, stack, false);
        if left.as_ref().map_or(0, ItemStack::count) != before {
            let _ = player.set_inventory_slot(lv.host, hotbar as u32, left);
        }
    }
    set_equipped(lv.host, player, &list);
}

/// `ToolboxEquipPacket.handle`: equip compartment `slot` of the toolbox at `pos` in `hotbar`
/// (no toolbox: stop tracking; a slot out of 0..8: unequip only).
fn equip(
    host: &mut dyn ServerHost,
    player: PlayerRef,
    pos: Option<[i32; 3]>,
    slot: i32,
    hotbar: usize,
) {
    let mut dimension = String::new();
    let Some(mut lv) = level_of(host, player, &mut dimension) else {
        return;
    };
    let Some(pos) = pos else {
        unequip(&mut lv, player, hotbar, false);
        sync(lv.host, player);
        return;
    };
    if !tb::in_range(position(&*lv.host, player), pos) || !is_toolbox(&lv, pos) {
        return;
    }
    unequip(&mut lv, player, hotbar, false);
    if !(0..COMPARTMENTS as i32).contains(&slot) {
        sync(lv.host, player);
        return;
    }
    let slot = slot as usize;
    let f = filters(&lv, pos);
    if let Some(stack) = hotbar_stack(&*lv.host, player, hotbar)
        && filter_stack(&lv, &f[slot]).is_none_or(|k| !same_kind(&*lv.host, &k, &stack))
    {
        // Back into the toolbox, else the main inventory (`ItemReturnInvWrapper`: not the
        // hotbar).
        let before = stack.count();
        let mut left = insert_limited(&mut lv, pos, stack);
        if let Some(rest) = left.take() {
            left = return_to_inventory(&mut lv, player, rest);
        }
        if left.as_ref().map_or(0, ItemStack::count) != before {
            let _ = player.set_inventory_slot(lv.host, hotbar as u32, left);
        }
    }
    let mut list = equipped(&*lv.host, player);
    list.retain(|e| e.0 != hotbar);
    list.push((hotbar, pos, slot));
    list.sort();
    set_equipped(lv.host, player, &list);
    sync(lv.host, player);
}

/// `insertItemStacked` into the main inventory (slots 9..35): stacks of it first, then empty
/// slots; what is left.
fn return_to_inventory(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    stack: ItemStack,
) -> Option<ItemStack> {
    let inv = player.inventory_slots(&*lv.host).unwrap_or_default();
    let max = max_stack(lv, &stack);
    let mut left = stack.count();
    let mut writes: Vec<(u32, ItemStack)> = Vec::new();
    for (slot, st) in &inv {
        if left == 0
            || !(9..36).contains(slot)
            || !st.is_stackable_with(&stack)
            || st.count() >= max
        {
            continue;
        }
        let n = left.min(max - st.count());
        let mut x = st.clone();
        let _ = x.set_count(st.count() + n);
        writes.push((*slot, x));
        left -= n;
    }
    for slot in 9..36u32 {
        if left == 0 {
            break;
        }
        if inv.iter().any(|(s, _)| *s == slot) {
            continue;
        }
        let n = left.min(max);
        let mut x = stack.clone();
        let _ = x.set_count(n);
        writes.push((slot, x));
        left -= n;
    }
    for (slot, s) in writes {
        let _ = player.set_inventory_slot(lv.host, slot, Some(s));
    }
    (left > 0).then(|| {
        let mut r = stack;
        let _ = r.set_count(left);
        r
    })
}

/// `ToolboxDisposeAllPacket.handle`: the inventory goes into the toolbox at `pos`, its slots
/// equipped from it let go (their items kept).
fn dispose_all(host: &mut dyn ServerHost, player: PlayerRef, pos: [i32; 3]) {
    let mut dimension = String::new();
    let Some(mut lv) = level_of(host, player, &mut dimension) else {
        return;
    };
    if !tb::in_range(position(&*lv.host, player), pos) || !is_toolbox(&lv, pos) {
        return;
    }
    let mut changed = false;
    for i in 0..36usize {
        if equipped(&*lv.host, player)
            .iter()
            .any(|e| e.0 == i && e.1 == pos)
        {
            unequip(&mut lv, player, i, true);
            changed = true;
        }
        let Some(stack) = hotbar_stack(&*lv.host, player, i) else {
            continue;
        };
        let before = stack.count();
        let left = insert_limited(&mut lv, pos, stack);
        if left.as_ref().map_or(0, ItemStack::count) != before {
            let _ = player.set_inventory_slot(lv.host, i as u32, left);
        }
    }
    if changed {
        sync(lv.host, player);
    }
}

/// A player's tick: their equipped slots whose toolbox is gone stop (`entityTick`'s check), and
/// the others are kept at half a stack from their compartment while in reach (`tickPlayers`).
pub(crate) fn player_tick(host: &mut dyn ServerHost, player: PlayerRef) {
    let list = equipped(&*host, player);
    if list.is_empty() {
        return;
    }
    let mut dimension = String::new();
    let Some(mut lv) = level_of(host, player, &mut dimension) else {
        return;
    };
    let at = position(&*lv.host, player);
    let mut drop = Vec::new();
    let mut unequipped = Vec::new();
    for (hotbar, pos, slot) in list {
        if !lv.is_loaded(pos) {
            continue;
        }
        if !is_toolbox(&lv, pos) {
            drop.push(hotbar);
            continue;
        }
        let f = filters(&lv, pos);
        let kind = filter_stack(&lv, &f[slot]);
        let Some(kind) = kind else {
            drop.push(hotbar);
            continue;
        };
        if !tb::in_range(at, pos) {
            continue;
        }
        let stack = hotbar_stack(&*lv.host, player, hotbar);
        if stack
            .as_ref()
            .is_some_and(|s| !same_kind(&*lv.host, &kind, s))
        {
            drop.push(hotbar);
            continue;
        }
        let count = stack.as_ref().map_or(0, ItemStack::count);
        let target = tb::target_amount(max_stack(&lv, &kind));
        // `isOpenInContainer`: with this toolbox's menu open, a slot that would change is let go
        // instead (its items back in the toolbox when it would refill, kept when it would give).
        let open = count != target && is_open(&lv, player, pos);
        if count < target && open && take(&mut lv, pos, slot, target - count, true).is_some() {
            unequipped.push((hotbar, false));
            continue;
        }
        if count > target && open {
            let mut extra = stack.clone().unwrap_or(kind.clone());
            let _ = extra.set_count(count - target);
            let left = distribute(&mut lv, pos, slot, extra, true);
            if left.map_or(0, |l| l.count()) < count - target {
                unequipped.push((hotbar, true));
                continue;
            }
        }
        if count < target {
            if let Some(got) = take(&mut lv, pos, slot, target - count, false) {
                let mut s = stack.clone().unwrap_or(got.clone());
                let _ = s.set_count(count + got.count());
                let _ = player.set_inventory_slot(lv.host, hotbar as u32, Some(s));
            }
        } else if count > target
            && let Some(s) = stack
        {
            let mut extra = s.clone();
            let _ = extra.set_count(count - target);
            let left = distribute(&mut lv, pos, slot, extra, false);
            let deposited = count - target - left.map_or(0, |l| l.count());
            if deposited > 0 {
                let mut s = s;
                let _ = s.set_count(count - deposited);
                let _ = player.set_inventory_slot(lv.host, hotbar as u32, Some(s));
            }
        }
    }
    for (hotbar, keep) in &unequipped {
        unequip(&mut lv, player, *hotbar, *keep);
    }
    if !unequipped.is_empty() && drop.is_empty() {
        sync(lv.host, player);
    }
    if !drop.is_empty() {
        let list: Vec<_> = equipped(&*lv.host, player)
            .into_iter()
            .filter(|e| !drop.contains(&e.0))
            .collect();
        set_equipped(lv.host, player, &list);
        sync(lv.host, player);
    }
}

/// `ToolboxHandlerClient.onPickItem`, out of creative: the item the pick would give (the
/// block's own; an entity's picked item is not known here), found as one of a compartment's
/// (`ItemStack.matches`) in the nearest toolboxes, is equipped in the selected slot instead of the
/// game's pick.
pub(crate) fn pick_item(
    host: &mut dyn ServerHost,
    event: &mut mod_api::PlayerPickItemEvent<'_>,
) -> Flow {
    if event.from_entity {
        return Flow::Continue;
    }
    let Some(player) = host.player(event.player) else {
        return Flow::Continue;
    };
    if player.game_mode(&*host) == Some(mod_api::GameMode::Creative) {
        return Flow::Continue;
    }
    let hotbar = player.selected_slot(&*host).unwrap_or(0);
    let mut dimension = String::new();
    let Some(mut lv) = level_of(host, player, &mut dimension) else {
        return Flow::Continue;
    };
    let Some(state) = lv.block_state(event.pos) else {
        return Flow::Continue;
    };
    let Some(result) = lv
        .host
        .block_item(state.block())
        .and_then(|i| ItemStack::new(i, 1))
    else {
        return Flow::Continue;
    };
    let at = position(&*lv.host, player);
    for pos in nearest(&lv, at) {
        for comp in 0..COMPARTMENTS {
            let Some(one) = take(&mut lv, pos, comp, 1, true) else {
                continue;
            };
            if one.is_stackable_with(&result) {
                equip(lv.host, player, Some(pos), comp as i32, hotbar);
                return Flow::Cancel;
            }
        }
    }
    Flow::Continue
}

/// `ToolboxBlockEntity.isOpenInContainer`: the player's open menu holds the toolbox's at `pos`.
fn is_open(lv: &ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) -> bool {
    let host: &dyn mod_api::Host = &*lv.host;
    host.block_entity(lv.level, pos)
        .and_then(|e| e.viewers(host))
        .is_some_and(|v| v.contains(&player))
}

pub(crate) fn player_joined(host: &mut dyn ServerHost, player: PlayerRef) {
    if !equipped(&*host, player).is_empty() {
        sync(host, player);
    }
}

/// The toolboxes within reach of the player, nearest first, at most eight
/// (`ToolboxHandler.getNearest`).
fn nearest(lv: &ServerLevel<'_>, at: [f64; 3]) -> Vec<[i32; 3]> {
    let r = tb::RANGE.ceil() as i32 + 1;
    let c = at.map(|v| v.floor() as i32);
    let mut out = Vec::new();
    for x in c[0] - r..=c[0] + r {
        for y in c[1] - r..=c[1] + r {
            for z in c[2] - r..=c[2] + r {
                let p = [x, y, z];
                if tb::in_range(at, p) && is_toolbox(lv, p) {
                    out.push(p);
                }
            }
        }
    }
    out.sort_by(|a, b| tb::distance(at, *a).total_cmp(&tb::distance(at, *b)));
    out.truncate(8);
    out
}

/// What the menu shows of a toolbox: where, its item, and each compartment's kind and whether
/// it is empty.
fn put_box(lv: &ServerLevel<'_>, out: &mut Vec<u8>, pos: [i32; 3]) {
    for v in pos {
        out.extend_from_slice(&v.to_le_bytes());
    }
    put_str(out, &lv.block_name(pos).unwrap_or_default());
    let f = filters(lv, pos);
    let slots = stored(lv, pos);
    for (c, kind) in f.iter().enumerate() {
        let bytes = kind
            .clone()
            .and_then(|k| Tag::Compound(k).to_network_bytes().ok())
            .unwrap_or_default();
        out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&bytes);
        out.push(u8::from(slots[c * STACKS_PER_COMPARTMENT].is_none()));
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    out.push(b.len().min(255) as u8);
    out.extend_from_slice(&b[..b.len().min(255)]);
}

/// The tool belt key (`ToolboxHandlerClient.onKeyInput`, decided here where the toolboxes are):
/// the menu for the selected hotbar slot, if any.
fn open_menu(host: &mut dyn ServerHost, player: PlayerRef) {
    let selected = player.selected_slot(&*host).unwrap_or(0);
    let mut dimension = String::new();
    let Some(mut lv) = level_of(host, player, &mut dimension) else {
        return;
    };
    let at = position(&*lv.host, player);
    // `onKeyInput`: the nearest eight, ordered by their `UniqueId`.
    let mut boxes = nearest(&lv, at);
    boxes.sort_by_cached_key(|p| uuid_order(uuid(&mut lv, *p)));
    let current = equipped(&*lv.host, player)
        .into_iter()
        .find(|e| e.0 == selected);
    // State: 0 select box, 1 select item, 2 select item or unequip, 3 detach.
    let (state, chosen, prev) = match current {
        Some((_, pos, slot)) => {
            if tb::in_range(at, pos) && is_toolbox(&lv, pos) {
                (2u8, Some(pos), slot as u8)
            } else {
                (3, None, 0)
            }
        }
        None if boxes.is_empty() => return,
        None if boxes.len() == 1 => (1, Some(boxes[0]), 0),
        None => (0, None, 0),
    };
    let mut out = vec![1u8, state, prev, boxes.len() as u8];
    for p in &boxes {
        put_box(&lv, &mut out, *p);
    }
    out.push(u8::from(chosen.is_some()));
    if let Some(p) = chosen {
        put_box(&lv, &mut out, p);
    }
    net::send_to_player(lv.host, player, CHANNEL, &out);
}

fn read_pos(d: &[u8]) -> Option<[i32; 3]> {
    let v = |i: usize| {
        Some(i32::from_le_bytes(
            d.get(i * 4..i * 4 + 4)?.try_into().ok()?,
        ))
    };
    Some([v(0)?, v(1)?, v(2)?])
}

/// A client's message: 0 open the menu; 1 equip (has toolbox, x, y, z, compartment, hotbar);
/// 2 return the inventory to the toolbox at x, y, z.
pub(crate) fn client_message(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    match data.first() {
        Some(0) => open_menu(host, player),
        Some(1) if data.len() >= 16 => {
            let pos = (data[1] != 0).then(|| read_pos(&data[2..14])).flatten();
            equip(
                host,
                player,
                pos,
                i32::from(data[14] as i8),
                usize::from(data[15]),
            );
        }
        Some(2) if data.len() >= 13 => {
            if let Some(pos) = read_pos(&data[1..13]) {
                dispose_all(host, player, pos);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------
// The toolbox as an item

/// The toolbox's contents and compartment kinds as its item keeps them, the toolbox emptied (so
/// its removal spills nothing).
fn take_contents(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> Compound {
    let f = filters(lv, pos);
    let id = uuid(lv, pos);
    let slots = stored(lv, pos);
    let mut items = Vec::new();
    let mut clear = Vec::new();
    for (i, s) in slots.into_iter().enumerate() {
        let Some(s) = s else { continue };
        if let Some(mut nbt) = s.to_nbt(&*lv.host) {
            nbt.insert("Slot".into(), Tag::Byte(i as i8));
            items.push(Tag::Compound(nbt));
        }
        clear.push((i, None));
    }
    lv.set_slots(pos, &clear);
    FILTERS.with_borrow_mut(|m| m.remove(&(lv.dim, pos)));
    let mut c = Compound::new();
    c.insert("Items".into(), Tag::List(items));
    c.insert("Compartments".into(), write_kinds(&f));
    c.insert(UUID_KEY.into(), uuid_tag(id));
    c
}

/// The toolbox item with `contents` (`TOOLBOX_INVENTORY`, none kept for an empty toolbox) and
/// its id (`TOOLBOX_UUID`, always: `getCloneItemStack` and the loot's `copy_components`).
fn with_contents(stack: &mut ItemStack, contents: &Compound) {
    let mut contents = contents.clone();
    let id = contents.remove(&UUID_KEY.into());
    let empty = matches!(contents.get(&"Items".into()), Some(Tag::List(l)) if l.is_empty())
        && read_kinds(contents.get(&"Compartments".into()))
            .iter()
            .all(Option::is_none);
    let mut custom = match stack
        .components()
        .and_then(|c| c.as_nbt().get(&"minecraft:custom_data".into()))
    {
        Some(Tag::Compound(c)) => c.clone(),
        _ => Compound::new(),
    };
    if !empty {
        custom.insert(ITEM_KEY.into(), Tag::Compound(contents));
    }
    if let Some(id) = id {
        custom.insert(UUID_KEY.into(), id);
    }
    if custom.is_empty() {
        return;
    }
    let patch = Compound::from([("minecraft:custom_data".into(), Tag::Compound(custom))]);
    let _ = stack.apply_components(&patch);
}

/// Before a toolbox at `pos` is removed by a player, an explosion or the port's own break: its
/// contents go with its drop.
pub(crate) fn before_removal(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    if !is_toolbox(lv, pos) {
        return;
    }
    let c = take_contents(lv, pos);
    BREAKING.with_borrow_mut(|b| b.insert((lv.dim, pos), c));
}

/// The broken toolbox's drop takes what it held (`copy_components` of `TOOLBOX_INVENTORY`).
pub(crate) fn block_drops(
    level: &dyn mod_api::LevelHost,
    event: &mut mod_api::BlockDropsEvent<'_>,
) {
    let Some(dim) = level_index(event.dimension.key().id()) else {
        return;
    };
    let pos = [event.pos.x, event.pos.y, event.pos.z];
    let Some(c) = BREAKING.with_borrow_mut(|b| b.remove(&(dim, pos))) else {
        return;
    };
    let _ = level;
    let items = TOOLBOX_ITEMS.get().map_or(&[][..], Vec::as_slice);
    if let Some(drop) = event
        .drops
        .iter_mut()
        .find(|d| items.contains(&d.stack.item()))
    {
        with_contents(&mut drop.stack, &c);
    }
}

/// Drops collected rather than spawned (a drill on a moving structure): a toolbox among them
/// takes what it held.
pub(crate) fn attach(dim: usize, pos: [i32; 3], drops: &mut [ItemStack]) {
    let Some(c) = BREAKING.with_borrow_mut(|b| b.remove(&(dim, pos))) else {
        return;
    };
    let items = TOOLBOX_ITEMS.get().map_or(&[][..], Vec::as_slice);
    if let Some(d) = drops.iter_mut().find(|d| items.contains(&d.item())) {
        with_contents(d, &c);
    }
}

/// Contents taken for a drop that never came (a creative break) are gone with the toolbox.
pub(crate) fn level_tick() {
    BREAKING.with_borrow_mut(HashMap::clear);
}

/// `ToolboxBlock.setPlacedBy`: a toolbox placed from an item with contents takes them.
pub(crate) fn placed(lv: &mut ServerLevel<'_>, pos: [i32; 3], stack: &ItemStack) {
    let Some(Tag::Compound(custom)) = stack
        .components()
        .and_then(|c| c.as_nbt().get(&"minecraft:custom_data".into()))
    else {
        return;
    };
    if let Some(u) = read_uuid(custom.get(&UUID_KEY.into())) {
        FILTERS.with_borrow_mut(|m| m.entry((lv.dim, pos)).or_default().uuid = Some(u));
        mark_pos_unsaved(lv.dim, pos);
    }
    let Some(Tag::Compound(c)) = custom.get(&ITEM_KEY.into()) else {
        return;
    };
    let mut changes = Vec::new();
    if let Some(Tag::List(items)) = c.get(&"Items".into()) {
        for t in items {
            let Tag::Compound(nbt) = t else { continue };
            let Some(Tag::Byte(slot)) = nbt.get(&"Slot".into()) else {
                continue;
            };
            let slot = *slot as usize;
            if slot < tb::STORED
                && let Some(s) = ItemStack::from_nbt(&*lv.host, nbt)
            {
                changes.push((slot, Some(s)));
            }
        }
    }
    lv.set_slots(pos, &changes);
    let f = read_kinds(c.get(&"Compartments".into()));
    FILTERS.with_borrow_mut(|m| m.entry((lv.dim, pos)).or_default().filters = f);
    mark_pos_unsaved(lv.dim, pos);
}

/// `ToolboxBlock.attack`: a player's left click picks the toolbox up into their inventory with
/// what it holds; the slots equipped from it let go, their items back in it.
pub(crate) fn left_click(host: &mut dyn ServerHost, pos: [i32; 3]) -> Flow {
    let Some(player) = host.players().acting_player() else {
        return Flow::Continue;
    };
    let mut dimension = String::new();
    let Some(mut lv) = level_of(host, player, &mut dimension) else {
        return Flow::Continue;
    };
    let Some((block, _)) = lv.info(pos) else {
        return Flow::Continue;
    };
    if !dye::is_toolbox(block) {
        return Flow::Continue;
    }
    // `unequipTracked`, for the players of this level.
    for p in lv.level.online_players(&*lv.host).unwrap_or_default() {
        let slots: Vec<usize> = equipped(&*lv.host, p)
            .into_iter()
            .filter(|e| e.1 == pos)
            .map(|e| e.0)
            .collect();
        if slots.is_empty() {
            continue;
        }
        for s in slots {
            unequip(&mut lv, p, s, false);
        }
        sync(lv.host, p);
    }
    let Some(item) = super::stacks::stack(&*lv.host, &format!("create:{}", block.name()), 1) else {
        return Flow::Continue;
    };
    let contents = take_contents(&mut lv, pos);
    let mut item = item;
    with_contents(&mut item, &contents);
    lv.destroy_block(pos, false);
    if !is_toolbox(&lv, pos) {
        lv.give(player, item);
    }
    Flow::Cancel
}

// ---------------------------------------------------------------------------------------------
// Saving the compartment kinds with their column

pub(crate) fn save(dim: usize, chunk: [i32; 2], data: &mut Compound) {
    let list: Vec<Tag> = FILTERS.with_borrow(|m| {
        m.iter()
            .filter(|((d, p), _)| *d == dim && [p[0] >> 4, p[2] >> 4] == chunk)
            .map(|((_, p), k)| {
                let mut c = Compound::new();
                c.insert("Pos".into(), Tag::IntArray(p.to_vec()));
                c.insert("Compartments".into(), write_kinds(&k.filters));
                if let Some(u) = k.uuid {
                    c.insert("UniqueId".into(), uuid_tag(u));
                }
                Tag::Compound(c)
            })
            .collect()
    });
    if !list.is_empty() {
        data.insert("toolboxes".into(), Tag::List(list));
    }
}

pub(crate) fn load(dim: usize, data: &Compound) {
    let Some(Tag::List(list)) = data.get(&"toolboxes".into()) else {
        return;
    };
    FILTERS.with_borrow_mut(|m| {
        for t in list {
            let Tag::Compound(c) = t else { continue };
            let Some(Tag::IntArray(p)) = c.get(&"Pos".into()) else {
                continue;
            };
            if p.len() != 3 {
                continue;
            }
            let kept = Kept {
                filters: read_kinds(c.get(&"Compartments".into())),
                uuid: read_uuid(c.get(&"UniqueId".into())),
            };
            m.insert((dim, [p[0], p[1], p[2]]), kept);
        }
    });
}

/// The toolbox at `pos` is gone: what was kept of it too.
pub(crate) fn removed(dim: usize, pos: [i32; 3]) {
    if FILTERS.with_borrow_mut(|m| m.remove(&(dim, pos))).is_some() {
        mark_pos_unsaved(dim, pos);
    }
}

pub(crate) fn chunk_unload(dim: usize, chunk: [i32; 2]) {
    FILTERS.with_borrow_mut(|m| {
        m.retain(|(d, p), _| *d != dim || [p[0] >> 4, p[2] >> 4] != chunk);
    });
}

// ---------------------------------------------------------------------------------------------
// The client

#[derive(Clone)]
struct BoxView {
    pos: [i32; 3],
    item: Option<ItemStack>,
    name: String,
    /// Each compartment's kind (as a stack) and whether it is empty.
    compartments: Vec<(Option<ItemStack>, bool)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    SelectBox,
    SelectItem,
    SelectItemUnequip,
    Detach,
}

struct Menu {
    state: State,
    boxes: Vec<BoxView>,
    selected: Option<BoxView>,
    ticks_open: u32,
    hovered: Option<i32>,
    scroll_mode: bool,
    scroll_slot: i32,
}

thread_local! {
    /// The local player's equipped slots: hotbar slot, toolbox.
    static EQUIPPED: RefCell<Vec<(u8, [i32; 3])>> = const { RefCell::new(Vec::new()) };
    static MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
    /// `ToolboxHandlerClient.COOLDOWN`.
    static COOLDOWN: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
    /// Messages for the server, sent at the next tick.
    static OUT: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
    static CLOSE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn client_reset() {
    EQUIPPED.with_borrow_mut(Vec::clear);
    MENU.with_borrow_mut(|m| *m = None);
    OUT.with_borrow_mut(Vec::clear);
}

fn read_str(d: &mut &[u8]) -> Option<String> {
    let n = usize::from(*d.first()?);
    let s = String::from_utf8(d.get(1..1 + n)?.to_vec()).ok()?;
    *d = &d[1 + n..];
    Some(s)
}

fn read_box(host: &dyn ClientHost, d: &mut &[u8]) -> Option<BoxView> {
    let pos = read_pos(d)?;
    *d = d.get(12..)?;
    let block = read_str(d)?;
    let stack = |id: &str| super::stacks::stack(host, id, 1);
    let mut compartments = Vec::new();
    for _ in 0..COMPARTMENTS {
        let n = usize::from(u16::from_le_bytes([*d.first()?, *d.get(1)?]));
        let bytes = d.get(2..2 + n)?;
        let kind = match Tag::from_network_bytes(bytes, None) {
            Ok((Tag::Compound(c), _)) if n > 0 => ItemStack::from_nbt(host, &c),
            _ => None,
        };
        *d = d.get(2 + n..)?;
        let empty = *d.first()? != 0;
        *d = &d[1..];
        compartments.push((kind, empty));
    }
    let item = stack(&block);
    let name = item
        .as_ref()
        .map(|s| host.hover_name(s))
        .unwrap_or_default();
    Some(BoxView {
        pos,
        item,
        name,
        compartments,
    })
}

pub(crate) fn server_message(host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    if channel != CHANNEL {
        return false;
    }
    match data.first() {
        Some(0) => {
            let n = usize::from(*data.get(1).unwrap_or(&0));
            let mut list = Vec::new();
            for i in 0..n {
                let at = 2 + i * 14;
                let Some(e) = data.get(at..at + 14) else {
                    break;
                };
                if let Some(p) = read_pos(&e[1..13]) {
                    list.push((e[0], p));
                }
            }
            EQUIPPED.with_borrow_mut(|e| *e = list);
        }
        Some(1) => {
            let decoded = (|| {
                let mut d = &data[1..];
                let state = match *d.first()? {
                    0 => State::SelectBox,
                    1 => State::SelectItem,
                    2 => State::SelectItemUnequip,
                    _ => State::Detach,
                };
                let prev = i32::from(*d.get(1)?);
                let n = usize::from(*d.get(2)?);
                d = &d[3..];
                let mut boxes = Vec::new();
                for _ in 0..n {
                    boxes.push(read_box(&*host, &mut d)?);
                }
                let has = *d.first()? != 0;
                d = &d[1..];
                let selected = if has {
                    Some(read_box(&*host, &mut d)?)
                } else {
                    None
                };
                Some(Menu {
                    state,
                    boxes,
                    selected,
                    ticks_open: 0,
                    hovered: None,
                    scroll_mode: false,
                    scroll_slot: prev,
                })
            })();
            if let Some(menu) = decoded {
                MENU.with_borrow_mut(|m| *m = Some(menu));
                if !host.mod_screen_open(MOD) {
                    MENU.with_borrow_mut(|m| *m = None);
                }
            }
        }
        _ => {}
    }
    true
}

fn selected_hotbar(host: &dyn ClientHost) -> u8 {
    host.local_player()
        .and_then(|p| p.selected_slot(host))
        .unwrap_or(0) as u8
}

/// `ToolboxHandlerClient.clientTick` and `onKeyInput`: the key asks the server for the menu.
pub(crate) fn client_tick(host: &mut dyn ClientHost) {
    if COOLDOWN.get() > 0 && !host.is_down(MOD, KEY) {
        COOLDOWN.set(COOLDOWN.get() - 1);
    }
    if MENU.with_borrow(Option::is_some) {
        MENU.with_borrow_mut(|m| {
            if let Some(m) = m.as_mut() {
                m.ticks_open += 1;
            }
        });
        // The screen gone some other way.
        if host.mod_screen() != Some(MOD) {
            MENU.with_borrow_mut(|m| *m = None);
        }
        // `keyReleased` of the tool belt key on Alt (a modifier, which the window reports while
        // held): let go, the menu closes. Another key closes it on its release
        // (`screen_key_released`).
        let bound = host
            .mod_key(MOD, KEY)
            .map_or_else(|| "key.keyboard.left.alt".to_owned(), |(k, _)| k);
        if bound.ends_with(".alt") && !host.modifiers().alt() && MENU.with_borrow(Option::is_some) {
            HOTBAR.set(selected_hotbar(&*host));
            close_menu();
        }
    } else if host.consume_click(MOD, KEY)
        && COOLDOWN.get() == 0
        && host.playing()
        && host
            .local_player()
            .and_then(|p| p.game_mode(&*host))
            .is_none_or(|m| m != mod_api::GameMode::Spectator)
    {
        OUT.with_borrow_mut(|o| o.push(vec![0]));
    }
    if CLOSE.replace(false) && host.mod_screen() == Some(MOD) {
        host.mod_screen_close(MOD);
    }
    for m in OUT.with_borrow_mut(std::mem::take) {
        net::send_to_server(host, CHANNEL, &m);
    }
}

fn equip_message(pos: Option<[i32; 3]>, slot: i32, hotbar: u8) -> Vec<u8> {
    let mut out = vec![1u8, u8::from(pos.is_some())];
    for v in pos.unwrap_or_default() {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.push(slot as i8 as u8);
    out.push(hotbar);
    out
}

fn dispose_message(pos: [i32; 3]) -> Vec<u8> {
    let mut out = vec![2u8];
    for v in pos {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn chosen(m: &Menu) -> Option<i32> {
    if m.scroll_mode {
        Some(m.scroll_slot)
    } else {
        m.hovered
    }
}

/// `RadialToolboxMenu.removed`: what was chosen goes to the server.
fn close_menu() {
    let Some(m) = MENU.with_borrow_mut(Option::take) else {
        return;
    };
    CLOSE.set(true);
    let hotbar = HOTBAR.get();
    let selected = chosen(&m);
    let mut out = Vec::new();
    if selected == Some(tb::DEPOSIT) {
        match m.state {
            State::Detach => {}
            State::SelectBox => out.extend(m.boxes.iter().map(|b| dispose_message(b.pos))),
            _ => {
                if let Some(b) = &m.selected {
                    out.push(dispose_message(b.pos));
                }
            }
        }
    } else if m.state == State::Detach {
        if selected == Some(tb::UNEQUIP) {
            out.push(equip_message(None, tb::UNEQUIP, hotbar));
        }
    } else if m.state != State::SelectBox
        && let Some(b) = &m.selected
    {
        if selected == Some(tb::UNEQUIP) {
            out.push(equip_message(Some(b.pos), tb::UNEQUIP, hotbar));
        }
        if let Some(s) = selected.filter(|s| *s >= 0)
            && let Some((Some(_), false)) = b.compartments.get(s as usize)
        {
            out.push(equip_message(Some(b.pos), s, hotbar));
        }
    }
    OUT.with_borrow_mut(|o| o.extend(out));
}

thread_local! {
    /// The selected hotbar slot when the menu was last drawn.
    static HOTBAR: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
    /// The screen's size when the menu was last drawn.
    static SIZE: std::cell::Cell<[f32; 2]> = const { std::cell::Cell::new([0.0; 2]) };
}

// The widgets (`AllGuiTextures` in `widgets.png`) and icons (`AllIcons`).
const SLOT: [f32; 4] = [0.0, 68.0, 22.0, 22.0];
const SLOT_HIGHLIGHT: [f32; 4] = [27.0, 67.0, 24.0, 24.0];
const EMPTY_SLOT: [f32; 4] = [27.0, 98.0, 22.0, 22.0];
const INACTIVE_SLOT: [f32; 4] = [52.0, 98.0, 22.0, 22.0];
const HOTBAR_OFF: [f32; 4] = [0.0, 130.0, 20.0, 24.0];
const HOTBAR_ON: [f32; 4] = [20.0, 130.0, 20.0, 24.0];
const SELECTED_OFF: [f32; 4] = [0.0, 155.0, 22.0, 22.0];
const SELECTED_ON: [f32; 4] = [22.0, 155.0, 22.0, 22.0];
const I_TOOLBOX: [f32; 2] = [11.0 * 16.0, 3.0 * 16.0];
const I_DISABLE: [f32; 2] = [8.0 * 16.0, 10.0 * 16.0];
const I_FLIP: [f32; 2] = [8.0 * 16.0, 4.0 * 16.0];
const I_REFRESH: [f32; 2] = [3.0 * 16.0, 16.0];

fn widget(draw: &mut dyn DrawContext, src: [f32; 4], at: [f32; 2]) {
    draw.sprite(
        "create:gui/widgets",
        src,
        [at[0], at[1], src[2], src[3]],
        None,
    );
}

fn icon(draw: &mut dyn DrawContext, i: [f32; 2], at: [f32; 2]) {
    draw.sprite(
        "create:gui/icons",
        [i[0], i[1], 16.0, 16.0],
        [at[0], at[1], 16.0, 16.0],
        None,
    );
}

fn tr(host: &dyn ClientHost, key: &str) -> String {
    host.translate(key).unwrap_or_else(|| key.to_owned())
}

/// `RadialToolboxMenu.renderWindow` (and its background), drawn over the mod's screen.
pub(crate) fn screen_render_post(
    host: &dyn ClientHost,
    event: &mut mod_api::ScreenRenderPostEvent<'_>,
) {
    if host.mod_screen() != Some(MOD) {
        return;
    }
    HOTBAR.set(selected_hotbar(host));
    let mouse = event.screen.mouse;
    let partial = event.screen.partial_tick;
    MENU.with_borrow_mut(|m| {
        if let Some(m) = m.as_mut() {
            draw_menu(host, event.draw, m, mouse, partial);
        }
    });
}

/// `renderWindow`'s hovered choice for the mouse `d` from the middle: a ring slot, the centre
/// (unequip) near the middle, the detach button above it, or the deposit button right of it.
fn hover(m: &mut Menu, d: [f32; 2]) {
    let distance = d[0] * d[0] + d[1] * d[1];
    m.hovered = tb::hovered(d).map(|s| s as i32);
    if m.scroll_mode && distance > 150.0 {
        m.scroll_mode = false;
    }
    if m.state == State::SelectItemUnequip && distance <= 150.0 {
        m.hovered = Some(tb::UNEQUIP);
    }
    if m.state == State::Detach {
        if d[0] > -20.0 && d[0] < 20.0 && d[1] > -80.0 && d[1] < -20.0 {
            m.hovered = Some(tb::UNEQUIP);
        }
    } else if d[0] > 60.0 && d[0] < 100.0 && d[1] > -20.0 && d[1] < 20.0 {
        m.hovered = Some(tb::DEPOSIT);
    }
}

fn draw_menu(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    m: &mut Menu,
    mouse: [f32; 2],
    partial: f32,
) {
    let size = draw.size();
    SIZE.set(size);
    let t = m.ticks_open as f32 + partial;
    // `renderBackground`: `BACKGROUND_COLOR` (0x50101010 over 0x50101010), fading in over a second.
    let alpha = (0x50 as f32 * (t / 20.0).min(1.0)) as u32;
    draw.fill([0.0, 0.0, size[0], size[1]], (alpha << 24) | 0x10_1010);
    let fade = (t / 10.0).clamp(1.0 / 512.0, 1.0);
    let ease = 10.0 * (1.0 - fade) * (1.0 - fade);
    let c = [(size[0] / 2.0).floor(), (size[1] / 2.0).floor()];
    let d = [mouse[0] - c[0], mouse[1] - c[1]];
    hover(m, d);
    let centre = m.state == State::SelectItemUnequip;
    let mut tip: Option<Text> = None;
    let gold = |t: Text| {
        let mut style = mod_api::text::TextStyle::default();
        style.color = Some("gold".to_owned());
        t.with_style(style)
    };
    if m.state == State::Detach {
        tip = Some(Text::literal(tr(host, "create.toolbox.outOfRange")));
        widget(draw, INACTIVE_SLOT, [c[0] - 12.0, c[1] - 12.0]);
        if let Some(s) = super::stacks::stack(host, "create:brown_toolbox", 1) {
            draw.item_stack(&s, [c[0] - 9.0, c[1] - 9.0], 16.0);
        }
        let y = c[1] - 40.0 + ease;
        widget(draw, SLOT, [c[0] - 12.0, y - 12.0]);
        icon(draw, I_DISABLE, [c[0] - 9.5, y - 8.5]);
        if !m.scroll_mode && m.hovered == Some(tb::UNEQUIP) {
            widget(draw, SLOT_HIGHLIGHT, [c[0] - 13.0, y - 13.0]);
            tip = Some(gold(Text::literal(tr(host, "create.toolbox.detach"))));
        }
    } else {
        let x = c[0] + 80.0 - 5.0 * (1.0 - fade) * (1.0 - fade);
        widget(draw, SLOT, [x - 12.0, c[1] - 12.0]);
        icon(draw, I_TOOLBOX, [x - 9.5, c[1] - 8.5]);
        if !m.scroll_mode && m.hovered == Some(tb::DEPOSIT) {
            widget(draw, SLOT_HIGHLIGHT, [x - 13.0, c[1] - 13.0]);
            let key = if m.state == State::SelectBox {
                "create.toolbox.depositAll"
            } else {
                "create.toolbox.depositBox"
            };
            tip = Some(gold(Text::literal(tr(host, key))));
        }
        let pick = chosen(m);
        for slot in 0..COMPARTMENTS {
            let o = tb::ring_offset(slot, 40.0 - ease);
            let at = [c[0] + o[0] - 12.0, c[1] + o[1] - 12.0];
            match m.state {
                State::SelectItem | State::SelectItemUnequip => {
                    let Some(b) = &m.selected else { continue };
                    match &b.compartments[slot] {
                        (Some(kind), empty) => {
                            widget(draw, if *empty { INACTIVE_SLOT } else { SLOT }, at);
                            draw.item_stack(kind, [at[0] + 3.0, at[1] + 3.0], 16.0);
                            if pick == Some(slot as i32) && !*empty {
                                widget(draw, SLOT_HIGHLIGHT, [at[0] - 1.0, at[1] - 1.0]);
                                tip = Some(Text::literal(host.hover_name(kind)));
                            }
                        }
                        (None, _) => widget(draw, EMPTY_SLOT, at),
                    }
                }
                State::SelectBox => {
                    if let Some(b) = m.boxes.get(slot) {
                        widget(draw, SLOT, at);
                        if let Some(s) = &b.item {
                            draw.item_stack(s, [at[0] + 3.0, at[1] + 3.0], 16.0);
                        }
                        if pick == Some(slot as i32) {
                            widget(draw, SLOT_HIGHLIGHT, [at[0] - 1.0, at[1] - 1.0]);
                            tip = Some(Text::literal(b.name.clone()));
                        }
                    } else {
                        widget(draw, EMPTY_SLOT, at);
                    }
                }
                State::Detach => {}
            }
        }
        if centre {
            widget(draw, SLOT, [c[0] - 12.0, c[1] - 12.0]);
            icon(
                draw,
                if m.scroll_mode { I_REFRESH } else { I_FLIP },
                [c[0] - 9.0, c[1] - 9.0],
            );
            if !m.scroll_mode && m.hovered == Some(tb::UNEQUIP) {
                widget(draw, SLOT_HIGHLIGHT, [c[0] - 13.0, c[1] - 13.0]);
                let held = host
                    .local_player()
                    .and_then(|p| host.living(p.entity()))
                    .and_then(|l| l.hand_item(host, mod_api::Hand::Main))
                    .map(|s| host.hover_name(&s))
                    .unwrap_or_default();
                let line = tr(host, "create.toolbox.unequip").replace("%1$s", &held);
                tip = Some(gold(Text::literal(line)));
            }
        }
    }
    if let Some(tip) = tip {
        let a = (fade * 255.0) as u32;
        if a > 8 {
            let w = host.text_width(&tip).unwrap_or(0) as f32;
            let _ = draw.gui_paint(&[mod_api::GuiDraw::Text {
                text: tip,
                x: (c[0] - (w / 2.0).round()) as i32,
                y: (size[1] - 68.0 - 4.0) as i32,
                color: (a.min(255) << 24) | 0xFF_FFFF,
                shadow: false,
            }]);
        }
    }
}

/// `RadialToolboxMenu.mouseClicked`.
pub(crate) fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut mod_api::ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if host.mod_screen() != Some(MOD) || MENU.with_borrow(Option::is_none) {
        return Flow::Continue;
    }
    let button = event.button;
    let size = SIZE.get();
    let mouse = event.screen.mouse;
    let d = [
        mouse[0] - (size[0] / 2.0).floor(),
        mouse[1] - (size[1] / 2.0).floor(),
    ];
    let mut close = false;
    MENU.with_borrow_mut(|m| {
        let Some(m) = m.as_mut() else { return };
        // Where the mouse is now (a click may come before the frame that moved it).
        if !m.scroll_mode {
            hover(m, d);
        }
        let selected = chosen(m);
        if button == 0 {
            if selected == Some(tb::DEPOSIT) {
                close = true;
            } else if m.state == State::SelectBox
                && let Some(s) = selected.filter(|s| *s >= 0 && (*s as usize) < m.boxes.len())
            {
                m.state = State::SelectItem;
                m.selected = Some(m.boxes[s as usize].clone());
            } else if m.state != State::SelectBox
                && (selected == Some(tb::UNEQUIP) || selected.is_some_and(|s| s >= 0))
            {
                close = true;
            }
        } else if button == 1 {
            if m.state == State::SelectItem && m.boxes.len() > 1 {
                m.state = State::SelectBox;
            } else if m.state == State::SelectItemUnequip && selected == Some(tb::UNEQUIP) {
                if m.boxes.len() > 1 {
                    if let Some(b) = &m.selected {
                        let msg = equip_message(Some(b.pos), tb::UNEQUIP, selected_hotbar(host));
                        OUT.with_borrow_mut(|o| o.push(msg));
                    }
                    m.state = State::SelectBox;
                } else {
                    close = true;
                }
            }
        }
    });
    if close {
        HOTBAR.set(selected_hotbar(host));
        close_menu();
        COOLDOWN.set(2);
    }
    Flow::Cancel
}

/// `RadialToolboxMenu.mouseScrolled`: near the middle the wheel steps round the ring, skipping
/// empty compartments (and missing toolboxes).
pub(crate) fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut mod_api::ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if host.mod_screen() != Some(MOD) || MENU.with_borrow(Option::is_none) {
        return Flow::Continue;
    }
    let size = SIZE.get();
    let mouse = event.screen.mouse;
    let d = [mouse[0] - size[0] / 2.0, mouse[1] - size[1] / 2.0];
    if d[0] * d[0] + d[1] * d[1] > 150.0 {
        return Flow::Continue;
    }
    let dy = event.scroll[1];
    MENU.with_borrow_mut(|m| {
        let Some(m) = m.as_mut() else { return };
        m.scroll_mode = true;
        m.scroll_slot = ((m.scroll_slot - dy as i32) + 8).rem_euclid(8);
        for _ in 0..10 {
            let ok = match m.state {
                State::SelectItem | State::SelectItemUnequip => {
                    m.selected.as_ref().is_some_and(|b| {
                        matches!(
                            b.compartments.get(m.scroll_slot as usize),
                            Some((Some(_), false))
                        )
                    })
                }
                State::SelectBox => (m.scroll_slot as usize) < m.boxes.len(),
                State::Detach => true,
            };
            if ok {
                break;
            }
            m.scroll_slot = (m.scroll_slot - dy.signum() as i32 + 8).rem_euclid(8);
        }
    });
    Flow::Cancel
}

/// `RadialToolboxMenu.keyPressed`: hotbar keys 1 to 8 pick that slot of the ring.
pub(crate) fn screen_key_pressed(
    host: &dyn ClientHost,
    event: &mut mod_api::ScreenKeyPressedPreEvent<'_>,
) -> Flow {
    if host.mod_screen() != Some(MOD) || MENU.with_borrow(Option::is_none) {
        return Flow::Continue;
    }
    // The hotbar keys at their default binding, 1 to 8.
    let Some(i) = (1..=8usize).find(|n| event.key == format!("key.keyboard.{n}")) else {
        return Flow::Continue;
    };
    let i = i - 1;
    let mut picked = false;
    MENU.with_borrow_mut(|m| {
        let Some(m) = m.as_mut() else { return };
        let ok = match m.state {
            State::SelectItem | State::SelectItemUnequip => m
                .selected
                .as_ref()
                .is_some_and(|b| matches!(b.compartments.get(i), Some((Some(_), false)))),
            State::SelectBox => i < m.boxes.len(),
            State::Detach => true,
        };
        if ok {
            m.scroll_mode = true;
            m.scroll_slot = i as i32;
            picked = true;
        }
    });
    if picked {
        let mut ev = mod_api::ScreenMouseButtonPressedPreEvent {
            screen: event.screen,
            button: 0,
            modifiers: 0,
            cancelled: false,
        };
        screen_mouse_pressed(host, &mut ev);
        return Flow::Cancel;
    }
    Flow::Continue
}

/// `RadialToolboxMenu.keyReleased`: letting go of the tool belt key closes the menu.
pub(crate) fn screen_key_released(
    host: &dyn ClientHost,
    event: &mut mod_api::ScreenKeyReleasedPreEvent<'_>,
) -> Flow {
    if host.mod_screen() != Some(MOD) || MENU.with_borrow(Option::is_none) {
        return Flow::Continue;
    }
    let bound = host
        .mod_key(MOD, KEY)
        .map_or_else(|| "key.keyboard.left.alt".to_owned(), |(k, _)| k);
    if event.key != bound {
        return Flow::Continue;
    }
    HOTBAR.set(selected_hotbar(host));
    close_menu();
    Flow::Cancel
}

/// The screen closed some other way (Escape): it acts as `removed`.
pub(crate) fn screen_closing(host: &dyn ClientHost, event: &mut mod_api::ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if MENU.with_borrow(Option::is_some) {
        HOTBAR.set(selected_hotbar(host));
        close_menu();
        CLOSE.set(false);
    }
}

/// `ToolboxHandlerClient.renderOverlay`: each equipped hotbar slot framed, lit while its toolbox
/// is in reach.
pub(crate) fn render_gui_post(host: &dyn ClientHost, event: &mut mod_api::RenderGuiPostEvent<'_>) {
    let list = EQUIPPED.with_borrow(Clone::clone);
    if list.is_empty() {
        return;
    }
    let Some(player) = host.local_player() else {
        return;
    };
    if player.game_mode(host) == Some(mod_api::GameMode::Spectator) {
        return;
    }
    let at = player.entity().position(host).unwrap_or([0.0; 3]);
    let selected = selected_hotbar(host);
    let draw = &mut *event.draw;
    let size = draw.size();
    let x = (size[0] / 2.0).floor() - 90.0;
    let y = size[1] - 23.0;
    for (slot, pos) in list {
        let sel = slot == selected;
        let offset = if sel { 1.0 } else { 0.0 };
        let near = tb::in_range(at, pos);
        let src = match (near, sel) {
            (true, true) => SELECTED_ON,
            (true, false) => HOTBAR_ON,
            (false, true) => SELECTED_OFF,
            (false, false) => HOTBAR_OFF,
        };
        widget(draw, src, [x + 20.0 * f32::from(slot) - offset, y + offset]);
    }
}

/// `ToolboxInventory.isItemValid` in a toolbox's menu: a compartment with a kind takes only
/// that kind, and no stack that can't go inside an item (a
/// shulker box, `canFitInsideContainerItems`), as `canItemsShareCompartment` compares them. The
/// kind is the kept one on the server, else what is in the compartment (all the client's
/// prediction knows).
struct CompartmentRules;

#[mod_api::horizon_part]
impl mod_api::MenuPart for CompartmentRules {
    fn menu_slot_rules(
        &self,
        host: &dyn mod_api::Host,
        event: &mut mod_api::MenuSlotRulesEvent<'_>,
    ) {
        let (Some(pos), mod_api::MenuContainer::Main) = (event.context.pos, event.container) else {
            return;
        };
        if event.slot >= tb::STORED {
            return;
        }
        let player = event.context.player.and_then(|u| host.player(u));
        let level = player
            .and_then(|p| p.dimension(host))
            .map(|d| d.to_string())
            .and_then(|d| Some((level_index(&d)?, host.level(&d)?)));
        let Some((dim, level)) = level else {
            return;
        };
        // By name, which the client's prediction can read too (it has no Create world).
        let toolbox = level.block_state(host, pos).is_some_and(|s| {
            let name = s.block().id(host);
            name.starts_with("create:") && name.ends_with("_toolbox")
        });
        if !toolbox {
            return;
        }
        let id = event.stack.item().id(host);
        if id.ends_with("shulker_box") {
            event.may_place = false;
            return;
        }
        let c = event.slot / STACKS_PER_COMPARTMENT;
        let kept = FILTERS.with_borrow(|m| m.get(&(dim, pos)).and_then(|k| k.filters[c].clone()));
        let kind = kept
            .and_then(|f| ItemStack::from_nbt(host, &f))
            .or_else(|| {
                (0..STACKS_PER_COMPARTMENT)
                    .find_map(|i| {
                        event
                            .context
                            .slot(event.container, c * STACKS_PER_COMPARTMENT + i)
                    })
                    .cloned()
            });
        if kind.is_some_and(|k| !same_kind(host, &k, event.stack)) {
            event.may_place = false;
        }
    }
}

/// The tool belt key: left Alt (`AllKeys.TOOLBELT`).
pub(crate) fn register(r: &mut dyn mod_api::Registrar) {
    r.menu_part(tb::MENU, CompartmentRules);
    let mut k = mod_api::KeyRegistration::new(KEY, "key.keyboard.left.alt");
    k.label = "create.keyinfo.toolbelt".into();
    r.register_key_mapping(k);
}

/// Whether `state` is a toolbox (for the glue's block hooks).
pub(crate) fn toolbox_block(b: Block) -> bool {
    dye::is_toolbox(b)
}
