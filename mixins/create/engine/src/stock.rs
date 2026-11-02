//! Create 6's stock logistics: logistics networks of stock links (`PackagerLinkBlockEntity`,
//! `LogisticallyLinkedBehaviour`), their stock (`InventorySummary`), orders
//! (`PackageOrder`, `PackageOrderWithCrafts`) split across the network's packagers
//! (`LogisticsManager.findPackagersForRequest`) and packed with their fragment data
//! (`PackagerBlockEntity.attemptToSend(requests)`, `PackageItem.setOrder`), re-packagers
//! putting fragmented orders back together (`PackageRepackageHelper`), redstone requesters
//! (`RedstoneRequesterBlockEntity`) and factory gauges (`FactoryPanelBehaviour`) with their
//! promises (`RequestPromiseQueue`).
//!
//! Plain rules over a stack key `K` (an item and its components, compared as
//! `ItemStack.isSameItemSameComponents` compares them); the glue reads inventories and makes the
//! boxes. Nothing here ticks: summaries are worked out when something asks, gauges are looked at
//! on their interval.

use crate::package::{Packed, SLOTS, Slot};
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

/// `BigItemStack.INF`: a creative crate's count.
pub const INF: i32 = 1_000_000_000;
/// `CLogistics.factoryGaugeTimer`'s default: ticks between a gauge's requests.
pub const GAUGE_INTERVAL: u32 = 100;
/// `TickBasedCache` lifetimes of `LogisticsManager.SUMMARIES` and `ACCURATE_SUMMARIES`.
pub const SUMMARY_TICKS: u64 = 20;
pub const ACCURATE_TICKS: u64 = 1;
/// `FactoryPanelBehaviour.addConnection`: at most nine panels feed one.
pub const MAX_INPUTS: usize = 9;
/// `LogisticsManager.performPackageRequests`: a packager packs at most 100 boxes per order.
pub const MAX_PACKAGES: usize = 100;

/// A logistics network's id (`freqId`, a UUID).
pub type Network = u128;

/// A network id from text: a UUID as Java writes it, else a fixed id made from the text (the
/// debug command names networks).
pub fn network_id(text: &str) -> Network {
    let hex: String = text.chars().filter(|c| *c != '-').collect();
    if text.len() == 36
        && hex.len() == 32
        && let Ok(v) = u128::from_str_radix(&hex, 16)
    {
        return v;
    }
    // FNV-1a, 128 bits.
    let mut h: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    for b in text.bytes() {
        h ^= u128::from(b);
        h = h.wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013b);
    }
    h
}

/// `UUID.toString`.
pub fn network_text(id: Network) -> String {
    let h = format!("{id:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// `UUID` as NBT's int array (`UUIDUtil`).
pub fn network_ints(id: Network) -> [i32; 4] {
    [
        (id >> 96) as u32 as i32,
        (id >> 64) as u32 as i32,
        (id >> 32) as u32 as i32,
        id as u32 as i32,
    ]
}

pub fn network_from_ints(v: &[i32]) -> Option<Network> {
    let v: [i32; 4] = v.try_into().ok()?;
    Some(v.iter().fold(0u128, |a, x| a << 32 | u128::from(*x as u32)))
}

/// `InventorySummary`: counts by stack, in the order first seen.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary<K> {
    entries: Vec<(K, i32)>,
    total: i32,
    /// `contributingLinks`.
    pub contributing_links: u32,
}

impl<K> Default for Summary<K> {
    fn default() -> Self {
        Summary {
            entries: Vec::new(),
            total: 0,
            contributing_links: 0,
        }
    }
}

impl<K: Clone + PartialEq> Summary<K> {
    /// `add(stack, count)` (a negative count takes away).
    pub fn add(&mut self, key: &K, count: i32) {
        if count == 0 {
            return;
        }
        if self.total < INF {
            self.total = self.total.saturating_add(count);
        }
        if let Some(e) = self.entries.iter_mut().find(|e| e.0 == *key) {
            if e.1 < INF {
                e.1 = e.1.saturating_add(count);
            }
            return;
        }
        self.entries.push((key.clone(), count));
    }

    /// `add(InventorySummary)`.
    pub fn add_all(&mut self, other: &Summary<K>) {
        for (k, n) in &other.entries {
            self.add(k, *n);
        }
        self.contributing_links += other.contributing_links;
    }

    /// `getCountOf`.
    pub fn count_of(&self, key: &K) -> i32 {
        self.entries.iter().find(|e| e.0 == *key).map_or(0, |e| e.1)
    }

    /// `erase`.
    pub fn erase(&mut self, key: &K) -> bool {
        let Some(i) = self.entries.iter().position(|e| e.0 == *key) else {
            return false;
        };
        self.total -= self.entries.remove(i).1;
        true
    }

    /// `getTotalOfMatching`.
    pub fn total_of_matching(&self, mut f: impl FnMut(&K) -> bool) -> i32 {
        self.entries.iter().filter(|e| f(&e.0)).map(|e| e.1).sum()
    }

    /// `getStacks`.
    pub fn stacks(&self) -> &[(K, i32)] {
        &self.entries
    }

    /// `getStacksByCount`: most first (a stable sort, as `List.sort`).
    pub fn by_count(&self) -> Vec<(K, i32)> {
        let mut v = self.entries.clone();
        v.sort_by_key(|e| std::cmp::Reverse(e.1));
        v
    }

    /// `getTotalCount`.
    pub fn total(&self) -> i32 {
        self.total
    }

    /// `isEmpty`: no stack was ever added (a count taken back to 0 stays).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// `PackageOrderWithCrafts`: the ordered stacks, and crafts (a pattern and how many).
#[derive(Clone, Debug, PartialEq)]
pub struct Order<K> {
    pub stacks: Vec<(K, u32)>,
    pub crafts: Vec<(Vec<(K, u32)>, u32)>,
}

impl<K> Default for Order<K> {
    fn default() -> Self {
        Order {
            stacks: Vec::new(),
            crafts: Vec::new(),
        }
    }
}

impl<K> Order<K> {
    /// `PackageOrderWithCrafts.simple`.
    pub fn simple(stacks: Vec<(K, u32)>) -> Self {
        Order {
            stacks,
            crafts: Vec::new(),
        }
    }

    /// `singleRecipe`.
    pub fn single_recipe(pattern: Vec<(K, u32)>) -> Self {
        Order {
            stacks: Vec::new(),
            crafts: vec![(pattern, 1)],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.stacks.is_empty()
    }
}

/// `PackageItem.PackageOrderData`: which order a box belongs to and where in it.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderData<K> {
    pub order_id: i32,
    pub link_index: i32,
    pub final_link: bool,
    pub fragment_index: i32,
    pub is_final: bool,
    pub context: Option<Order<K>>,
}

/// `LogisticallyLinkedBehaviour.RequestType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestType {
    Restock,
    Redstone,
    Player,
}

impl RequestType {
    /// `PackagerBlockEntity.isTooBusyFor`: queued boxes at which a packager refuses.
    pub fn busy_at(self) -> usize {
        match self {
            RequestType::Player => 50,
            RequestType::Redstone => 20,
            RequestType::Restock => 10,
        }
    }
}

/// `PackagingRequest`: `final_link` is shared by the requests of one link (`MutableBoolean`).
#[derive(Clone, Debug)]
pub struct Request<K> {
    pub item: K,
    pub count: u32,
    pub address: String,
    pub link_index: i32,
    pub final_link: Rc<Cell<bool>>,
    pub counter: Rc<Cell<i32>>,
    pub order_id: i32,
    pub context: Option<Order<K>>,
}

/// A link of the network as an order sees it (`LogisticallyLinkedBehaviour`, by priority): its
/// packager (None: none attached, it's a re-packager, or the link is at full power), the
/// inventory that packager reaches (`InventoryIdentifier`) and its stock (`getAvailableItems`).
#[derive(Clone, Debug)]
pub struct Source<K> {
    pub packager: Option<[i32; 3]>,
    pub inventory: Option<[i32; 3]>,
    pub stock: Summary<K>,
}

/// `LogisticsManager.findPackagersForRequest`: each ordered stack is taken from the links in
/// turn (those reaching one inventory share one, picked by `pick(n)` in `0..n`), each link's
/// requests numbered by the order it was first used in, the last link's last request marked
/// final; only the first box carries the order. Requests by packager in the order made.
pub fn find_packagers<K: Clone + PartialEq>(
    sources: &[Source<K>],
    order: &Order<K>,
    address: &str,
    order_id: i32,
    mut pick: impl FnMut(usize) -> usize,
) -> Vec<([i32; 3], Request<K>)> {
    let stacks: Vec<&(K, u32)> = order.stacks.iter().filter(|s| s.1 > 0).collect();
    // Links without an inventory first, then one of each inventory's.
    let mut available: Vec<usize> = Vec::new();
    let mut groups: Vec<([i32; 3], Vec<usize>)> = Vec::new();
    for (i, s) in sources.iter().enumerate() {
        match s.inventory {
            Some(id) => match groups.iter_mut().find(|g| g.0 == id) {
                Some(g) => g.1.push(i),
                None => groups.push((id, vec![i])),
            },
            None => available.push(i),
        }
    }
    for (_, g) in &groups {
        available.push(g[pick(g.len()).min(g.len() - 1)]);
    }
    let mut out: Vec<([i32; 3], Request<K>)> = Vec::new();
    let mut used: Vec<usize> = Vec::new();
    let mut final_tracker = Rc::new(Cell::new(false));
    let mut context = Some(order.clone());
    for (i, (item, count)) in stacks.iter().enumerate() {
        let mut remaining = *count;
        let final_entry = i == stacks.len() - 1;
        for &link in &available {
            let used_index = used.iter().position(|u| *u == link);
            let link_index = used_index.unwrap_or(used.len());
            let is_final_link = if used.len().checked_sub(1) == Some(link_index) {
                final_tracker.clone()
            } else {
                Rc::new(Cell::new(false))
            };
            let s = &sources[link];
            let Some(packager) = s.packager else { continue };
            let available_count = s.stock.count_of(item);
            if available_count <= 0 {
                continue;
            }
            let take = remaining.min(available_count as u32);
            out.push((
                packager,
                Request {
                    item: item.clone(),
                    count: take,
                    address: address.to_owned(),
                    link_index: link_index as i32,
                    final_link: is_final_link.clone(),
                    counter: Rc::new(Cell::new(0)),
                    order_id,
                    context: context.clone(),
                },
            ));
            if take > 0 && used_index.is_none() {
                context = None;
                used.push(link);
                final_tracker = is_final_link;
            }
            remaining -= take;
            if remaining > 0 {
                continue;
            }
            if final_entry {
                final_tracker.set(true);
            }
            break;
        }
    }
    out
}

/// The requests grouped by packager (`Multimap.asMap`), packagers in the order first named.
pub fn by_packager<K>(
    requests: Vec<([i32; 3], Request<K>)>,
) -> Vec<([i32; 3], VecDeque<Request<K>>)> {
    let mut out: Vec<([i32; 3], VecDeque<Request<K>>)> = Vec::new();
    for (p, r) in requests {
        match out.iter_mut().find(|g| g.0 == p) {
            Some(g) => g.1.push_back(r),
            None => out.push((p, VecDeque::from([r]))),
        }
    }
    out
}

/// One box packed for requests: what it takes and holds (as `package::pack`), its address and
/// its place in the order.
#[derive(Clone, Debug, PartialEq)]
pub struct Requested<K> {
    pub packed: Packed,
    pub address: String,
    pub order: OrderData<K>,
}

/// `PackagerBlockEntity.attemptToSend(queuedRequests)`: one box of the requests at the head of
/// `queue`, taken from `slots` (their counts go down by what it takes). A request done moves on
/// to the next while it goes to the same address in the same order. None when nothing of the
/// request was there (the request is dropped).
pub fn pack_requests<K: Clone + PartialEq>(
    slots: &mut [Option<Slot<K>>],
    queue: &mut VecDeque<Request<K>>,
) -> Option<Requested<K>> {
    let first = queue.front()?;
    let address = first.address.clone();
    let order_id = first.order_id;
    let link_index = first.link_index;
    let final_link = first.final_link.get();
    let fragment_index = first.counter.get();
    first.counter.set(fragment_index + 1);
    let mut context = first.context.clone();
    let mut final_package = false;
    let mut out = Packed::default();
    let mut any = false;
    'outer: for _ in 0..SLOTS {
        let mut continue_packing = true;
        while continue_packing {
            continue_packing = false;
            for i in 0..slots.len() {
                let Some(next) = queue.front_mut() else {
                    break 'outer;
                };
                let Some(s) = &slots[i] else { continue };
                let extracted = s.count.min(64).min(next.count);
                if extracted == 0 || s.key != next.item {
                    continue;
                }
                let (bulky, is_package) = (s.bulky, s.package);
                if bulky && any {
                    continue;
                }
                any = true;
                let view: Vec<Option<(bool, u32)>> = out
                    .contents
                    .iter()
                    .map(|c| {
                        c.map(|(from, n)| (slots[from].as_ref().is_some_and(|f| f.key == s.key), n))
                    })
                    .collect();
                let plan = crate::vault::insert_stacked(&view, extracted, s.max.clamp(1, 64));
                let mut transferred = 0;
                for (slot, n) in plan {
                    transferred += n;
                    match &mut out.contents[slot] {
                        Some((_, c)) => *c += n,
                        c => *c = Some((i, n)),
                    }
                }
                if let Some(s) = &mut slots[i] {
                    s.count -= transferred;
                }
                if transferred > 0 {
                    match out.taken.iter_mut().find(|t| t.0 == i) {
                        Some(t) => t.1 += transferred,
                        None => out.taken.push((i, transferred)),
                    }
                }
                if is_package {
                    out.package = Some(i);
                }
                next.count -= transferred;
                if next.count > 0 {
                    if bulky {
                        break 'outer;
                    }
                    continue;
                }
                final_package = true;
                let done = queue.pop_front().unwrap();
                let Some(next) = queue.front() else {
                    break 'outer;
                };
                if next.address != address || next.order_id != order_id {
                    break 'outer;
                }
                next.counter.set(done.counter.get());
                final_package = false;
                continue_packing = true;
                if next.context.is_some() {
                    context = next.context.clone();
                }
                if bulky {
                    break 'outer;
                }
                break;
            }
        }
    }
    if !any {
        queue.pop_front();
        return None;
    }
    Some(Requested {
        packed: out,
        address,
        order: OrderData {
            order_id,
            link_index,
            final_link,
            fragment_index,
            is_final: final_package,
            context,
        },
    })
}

/// A fragment a re-packager holds: its order data, address and contents.
#[derive(Clone, Debug, PartialEq)]
pub struct Fragment<K> {
    pub order: OrderData<K>,
    pub address: String,
    pub contents: Vec<(K, u32)>,
}

/// `PackageRepackageHelper.isOrderComplete`: every link's fragments up to its final one, links
/// up to the final link.
pub fn is_order_complete<K>(fragments: &[&OrderData<K>]) -> bool {
    let mut final_link_reached = false;
    'links: for link in 0..1000 {
        if final_link_reached {
            break;
        }
        'packages: for package in 0..1000 {
            for d in fragments {
                if d.link_index != link || d.fragment_index != package {
                    continue;
                }
                final_link_reached = d.final_link;
                if d.is_final {
                    continue 'links;
                }
                continue 'packages;
            }
            return false;
        }
    }
    true
}

/// A box on its way out of `repack`: its stacks, its order data if it has its own, how many.
type Exported<K> = (Vec<(K, u32)>, Option<OrderData<K>>, u32);

/// A gauge's inputs by network: each item (None: an input without one) and the total asked.
pub type Groups<K> = Vec<(Network, Vec<(Option<K>, u32)>)>;

/// A box a re-packager sends: its nine slots' stacks, its address and order, and how many of
/// it (`BigItemStack.count`).
#[derive(Clone, Debug, PartialEq)]
pub struct Repacked<K> {
    pub contents: Vec<(K, u32)>,
    pub address: String,
    pub order: OrderData<K>,
    pub count: u32,
}

/// `PackageRepackageHelper.repack`: an order's fragments made whole again, the ordered stacks
/// first in the order's order (crafts in boxes of their pattern), nine stacks a box of at most
/// `max(item)` each; boxes without order data are marked order `order_id`, the last one
/// carrying the order. `random` stands in for `r.nextInt()`.
pub fn repack<K: Clone + PartialEq>(
    fragments: &[Fragment<K>],
    order_id: i32,
    max: impl Fn(&K) -> u32,
    mut random: impl FnMut() -> i32,
) -> Vec<Repacked<K>> {
    let mut exporting: Vec<Exported<K>> = Vec::new();
    let mut address = String::new();
    let mut context: Option<Order<K>> = None;
    let mut summary = Summary::default();
    for f in fragments {
        address = f.address.clone();
        if let Some(c) = &f.order.context
            && !c.is_empty()
        {
            context = Some(c.clone());
        }
        for (k, n) in &f.contents {
            summary.add(k, *n as i32);
        }
    }
    let mut ordered: VecDeque<(K, i32)> = VecDeque::new();
    if let Some(ctx) = &context {
        // `repackBasedOnRecipes`.
        for (pattern, count) in &ctx.crafts {
            let mut boxes = 0;
            'crafts: for _ in 0..*count {
                for (k, _) in pattern {
                    if summary.count_of(k) <= 0 {
                        break 'crafts;
                    }
                    summary.add(k, -1);
                }
                boxes += 1;
            }
            let contents: Vec<(K, u32)> = pattern
                .iter()
                .take(SLOTS)
                .map(|(k, _)| (k.clone(), 1))
                .collect();
            let order = OrderData {
                order_id: random(),
                link_index: 0,
                final_link: true,
                fragment_index: 0,
                is_final: true,
                context: Some(Order::single_recipe(pattern.clone())),
            };
            exporting.push((contents, Some(order), boxes));
        }
        if exporting.is_empty() {
            ordered.extend(ctx.stacks.iter().map(|(k, n)| (k.clone(), *n as i32)));
        }
    }
    let mut all: Vec<(K, i32)> = summary.stacks().to_vec();
    let mut outputs: Vec<(K, u32)> = Vec::new();
    'repack: loop {
        all.retain(|e| e.1 != 0);
        if all.is_empty() {
            break;
        }
        let mut target = ordered.pop_front();
        'search: for entry in all.iter_mut() {
            let mut amount = entry.1;
            if amount == 0 {
                continue;
            }
            if let Some(t) = &target {
                amount = t.1;
                if t.0 != entry.0 {
                    continue;
                }
            }
            while amount > 0 {
                let removed = amount.min(max(&entry.0) as i32).min(entry.1);
                if removed <= 0 {
                    continue 'search;
                }
                outputs.push((entry.0.clone(), removed as u32));
                amount -= removed;
                if let Some(t) = &mut target {
                    t.1 = amount;
                }
                entry.1 -= removed;
            }
            continue 'repack;
        }
    }
    for chunk in outputs.chunks(SLOTS) {
        exporting.push((chunk.to_vec(), None, 1));
    }
    let n = exporting.len();
    exporting
        .into_iter()
        .enumerate()
        .map(|(i, (contents, order, count))| Repacked {
            contents,
            address: address.clone(),
            order: order.unwrap_or_else(|| OrderData {
                order_id,
                link_index: 0,
                final_link: true,
                fragment_index: 0,
                is_final: true,
                context: if i == n - 1 { context.clone() } else { None },
            }),
            count,
        })
        .collect()
}

/// `RequestPromiseQueue`: stacks on their way, with the tick each was promised (its age is the
/// time since: nothing ticks them).
#[derive(Clone, Debug, PartialEq)]
pub struct Promises<K> {
    pub list: Vec<(K, u32, u64)>,
}

impl<K> Default for Promises<K> {
    fn default() -> Self {
        Promises { list: Vec::new() }
    }
}

impl<K: PartialEq> Promises<K> {
    pub fn add(&mut self, key: K, count: u32, now: u64) {
        self.list.push((key, count, now));
    }

    /// `getTotalPromisedAndRemoveExpired` (`expiry` in ticks; None never expires).
    pub fn total(&mut self, key: &K, expiry: Option<u64>, now: u64) -> u32 {
        if let Some(e) = expiry {
            self.list
                .retain(|p| p.0 != *key || now.saturating_sub(p.2) < e);
        }
        self.list.iter().filter(|p| p.0 == *key).map(|p| p.1).sum()
    }

    /// `forceClear`.
    pub fn clear(&mut self, key: &K) {
        self.list.retain(|p| p.0 != *key);
    }

    /// `itemEnteredSystem`: arrivals pay off the oldest promises first.
    pub fn arrived(&mut self, key: &K, mut amount: u32) {
        let mut i = 0;
        while i < self.list.len() && amount > 0 {
            if self.list[i].0 != *key {
                i += 1;
                continue;
            }
            let take = amount.min(self.list[i].1);
            amount -= take;
            self.list[i].1 -= take;
            if self.list[i].1 == 0 {
                self.list.remove(i);
            } else {
                i += 1;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// `PackagerBlockEntity.submitNewArrivals`: what grew between two summaries of one inventory.
pub fn arrivals<K: Clone + PartialEq>(before: &Summary<K>, after: &Summary<K>) -> Vec<(K, u32)> {
    if after.is_empty() {
        return Vec::new();
    }
    let mut diff = before.clone();
    for (k, n) in after.stacks() {
        diff.add(k, -n);
    }
    diff.stacks()
        .iter()
        .filter(|e| e.1 < 0)
        .map(|e| (e.0.clone(), (-e.1) as u32))
        .collect()
}

/// What a redstone requester does on a rising signal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// Nothing encoded: nothing happens.
    Nothing,
    /// Not all in stock and partial requests not allowed (`RedstoneRequesterEffectPacket`
    /// false).
    Denied,
    /// The order goes out; whether any of its stacks was fully in stock.
    Send { any_in_stock: bool },
}

/// `RedstoneRequesterBlockEntity`.
#[derive(Clone, Debug, PartialEq)]
pub struct Requester<K> {
    pub order: Order<K>,
    pub address: String,
    pub allow_partial: bool,
    pub powered: bool,
    pub last_succeeded: bool,
}

impl<K> Default for Requester<K> {
    fn default() -> Self {
        Requester {
            order: Order::default(),
            address: String::new(),
            allow_partial: false,
            powered: false,
            last_succeeded: false,
        }
    }
}

impl<K: Clone + PartialEq> Requester<K> {
    /// `onRedstonePowerChanged`: true when the signal rose and the order should be looked at.
    pub fn power(&mut self, signal: bool) -> bool {
        if self.powered == signal {
            return false;
        }
        self.last_succeeded = false;
        self.powered = signal;
        signal
    }

    /// `triggerRequest` against the network's accurate stock.
    pub fn trigger(&mut self, stock: &Summary<K>) -> Trigger {
        if self.order.is_empty() {
            return Trigger::Nothing;
        }
        let mut ordered = Summary::default();
        for (k, n) in &self.order.stacks {
            ordered.add(k, *n as i32);
        }
        let mut any = false;
        for (k, n) in ordered.stacks() {
            if stock.count_of(k) >= *n {
                any = true;
                continue;
            }
            if !self.allow_partial {
                return Trigger::Denied;
            }
        }
        self.last_succeeded = true;
        Trigger::Send { any_in_stock: any }
    }
}

/// What a factory gauge asks of its network when it is looked at.
#[derive(Clone, Debug, PartialEq)]
pub enum GaugeStep<K> {
    Idle,
    /// A restocker (on a packager): order `amount` of its item from its network to its address,
    /// leaving out its own packager's inventory.
    Restock {
        item: K,
        demand: u32,
    },
    /// Request what the panels feeding it hold, by their networks, to its address; then it
    /// promises `recipe_output` of its item.
    Request,
}

/// `FactoryPanelBehaviour` (one panel per gauge): an item kept at `count` (items, or stacks
/// unless `up_to`) in its network.
#[derive(Clone, Debug, PartialEq)]
pub struct Panel<K> {
    pub filter: Option<K>,
    pub count: u32,
    pub up_to: bool,
    pub network: Network,
    pub recipe_address: String,
    pub recipe_output: u32,
    /// `targetedBy`: gauges feeding this one and the amount each asks for.
    pub inputs: Vec<([i32; 3], u32)>,
    /// `targeting`.
    pub outputs: Vec<[i32; 3]>,
    /// `activeCraftingArrangement`.
    pub craft: Vec<K>,
    /// `promiseClearingInterval` (-1: never, 0: 30 s, else minutes).
    pub promise_clearing: i32,
    /// `restocker`: attached to a packager.
    pub restocker: bool,
    pub restocker_promises: Promises<K>,
    pub satisfied: bool,
    pub promised_satisfied: bool,
    pub waiting: bool,
    pub last_level: i32,
    pub last_promised: u32,
    pub last_unloaded: u32,
}

impl<K> Panel<K> {
    pub fn new(network: Network) -> Self {
        Panel {
            filter: None,
            count: 0,
            up_to: false,
            network,
            recipe_address: String::new(),
            recipe_output: 1,
            inputs: Vec::new(),
            outputs: Vec::new(),
            craft: Vec::new(),
            promise_clearing: -1,
            restocker: false,
            restocker_promises: Promises::default(),
            satisfied: false,
            promised_satisfied: false,
            waiting: false,
            last_level: 0,
            last_promised: 0,
            last_unloaded: 0,
        }
    }

    /// `getPromiseExpiryTimeInTicks`.
    pub fn promise_expiry(&self) -> Option<u64> {
        match self.promise_clearing {
            -1 => None,
            0 => Some(20 * 30),
            m => Some(m.max(0) as u64 * 20 * 60),
        }
    }

    /// `getAmount() * (upTo ? 1 : maxStackSize)`.
    pub fn demand(&self, max_stack: u32) -> u32 {
        self.count * if self.up_to { 1 } else { max_stack }
    }
}

impl<K: Clone + PartialEq> Panel<K> {
    /// `tickStorageMonitor` then `tickRequests` at the gauge's interval: `in_storage` its item in
    /// the network (the packager's inventory for a restocker), `promised` what is on its way,
    /// `unloaded` the network's links not loaded. Whether it became satisfied (the bulb's
    /// confirm) is `satisfied` after.
    pub fn look(
        &mut self,
        max_stack: u32,
        in_storage: i32,
        promised: u32,
        unloaded: u32,
    ) -> GaugeStep<K> {
        let demand = self.demand(max_stack);
        let empty = self.filter.is_none();
        self.satisfied = empty || in_storage >= demand as i32;
        self.promised_satisfied = empty || in_storage + promised as i32 >= demand as i32;
        self.waiting = unloaded > 0;
        self.last_level = in_storage;
        self.last_promised = promised;
        self.last_unloaded = unloaded;
        if self.inputs.is_empty() && !self.restocker {
            return GaugeStep::Idle;
        }
        if self.satisfied || self.promised_satisfied || self.waiting {
            return GaugeStep::Idle;
        }
        if crate::package::is_blank(&self.recipe_address) {
            return GaugeStep::Idle;
        }
        if self.restocker {
            return match &self.filter {
                Some(item) => GaugeStep::Restock {
                    item: item.clone(),
                    demand,
                },
                None => GaugeStep::Idle,
            };
        }
        GaugeStep::Request
    }
}

/// `tryRestock`'s amount: what is missing (at most nine stacks), at most what the network has.
pub fn restock_amount(
    demand: u32,
    promised: u32,
    in_storage: i32,
    max_stack: u32,
    available: i32,
) -> u32 {
    let missing = (i64::from(demand) - i64::from(promised) - i64::from(in_storage))
        .clamp(0, i64::from(max_stack) * 9) as u32;
    missing.min(available.max(0) as u32)
}

/// `tickRequests`' consolidation: the inputs' items by network (in the order first seen) and the
/// total each is asked for; None when any input is not loaded or not active (the gauge waits).
pub fn consolidate<K: Clone + PartialEq>(
    inputs: &[([i32; 3], u32)],
    source: impl Fn([i32; 3]) -> Option<(Network, Option<K>)>,
) -> Option<Groups<K>> {
    let mut out: Groups<K> = Vec::new();
    for (pos, amount) in inputs {
        let (net, item) = source(*pos)?;
        let group = match out.iter_mut().position(|g| g.0 == net) {
            Some(i) => &mut out[i].1,
            None => {
                out.push((net, Vec::new()));
                &mut out.last_mut().unwrap().1
            }
        };
        match group.iter_mut().find(|e| e.0 == item) {
            Some(e) => e.1 += amount,
            None => group.push((item, *amount)),
        }
    }
    Some(out)
}

/// `LogisticsNetwork.owner` and `locked` (`GlobalLogisticsManager.linkAdded`: the first player
/// to link a block owns the network).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ownership {
    pub owner: Option<u128>,
    pub locked: bool,
}

/// A logistics network as `LogisticsNetworkSavedData` keeps it across restarts
/// (`LogisticsNetwork.write`): its links (`totalLinks`, loaded or not), its gauges' promises
/// (`panelPromises`: item, count and `ticksExisted`) and its owner.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedNetwork<K> {
    pub id: Network,
    /// Dimension index and position.
    pub links: Vec<(usize, [i32; 3])>,
    pub promises: Vec<(K, u32, u64)>,
    pub ownership: Ownership,
}

/// What a level knows of the networks: its stock links' networks (loaded and unloaded) and
/// the promises it keeps for them (each promise holding the tick it was made).
#[derive(Clone, Debug, PartialEq)]
pub struct LevelNetworks<K> {
    pub links: Vec<([i32; 3], Network)>,
    pub promises: Vec<(Network, Promises<K>)>,
}

impl<K> Default for LevelNetworks<K> {
    fn default() -> Self {
        LevelNetworks {
            links: Vec::new(),
            promises: Vec::new(),
        }
    }
}

/// The networks to save at tick `now`: every network with a link in any level (Create drops
/// one whose last link is gone, `linkRemoved`), by id, its links by level and position, its
/// promises from every level with their age.
pub fn save_networks<K: Clone>(
    levels: &[LevelNetworks<K>],
    owners: &HashMap<Network, Ownership>,
    now: u64,
) -> Vec<SavedNetwork<K>> {
    let mut by_id: HashMap<Network, SavedNetwork<K>> = HashMap::new();
    for (dim, level) in levels.iter().enumerate() {
        for (pos, net) in &level.links {
            let n = by_id.entry(*net).or_insert_with(|| SavedNetwork {
                id: *net,
                links: Vec::new(),
                promises: Vec::new(),
                ownership: owners.get(net).copied().unwrap_or_default(),
            });
            if !n.links.contains(&(dim, *pos)) {
                n.links.push((dim, *pos));
            }
        }
    }
    for level in levels {
        for (net, p) in &level.promises {
            if let Some(n) = by_id.get_mut(net) {
                n.promises.extend(
                    p.list
                        .iter()
                        .map(|(k, c, at)| (k.clone(), *c, now.saturating_sub(*at))),
                );
            }
        }
    }
    let mut out: Vec<SavedNetwork<K>> = by_id.into_values().collect();
    out.sort_by_key(|n| n.id);
    for n in &mut out {
        n.links.sort_unstable();
    }
    out
}

/// The saved networks read back at tick `now` (`LogisticsNetworkSavedData.load`): each level's
/// links (all unloaded until their columns load) and promises (kept by the level of the
/// network's first link, the overworld for none), and the owners.
pub fn load_networks<K: Clone>(
    saved: &[SavedNetwork<K>],
    levels: usize,
    now: u64,
) -> (Vec<LevelNetworks<K>>, HashMap<Network, Ownership>) {
    let mut out: Vec<LevelNetworks<K>> = (0..levels).map(|_| LevelNetworks::default()).collect();
    let mut owners = HashMap::new();
    for n in saved {
        for (dim, pos) in &n.links {
            if let Some(level) = out.get_mut(*dim) {
                level.links.push((*pos, n.id));
            }
        }
        if n.ownership != Ownership::default() {
            owners.insert(n.id, n.ownership);
        }
        if n.promises.is_empty() {
            continue;
        }
        let dim = n
            .links
            .first()
            .map_or(0, |l| l.0)
            .min(levels.saturating_sub(1));
        let list = n
            .promises
            .iter()
            .map(|(k, c, age)| (k.clone(), *c, now.saturating_sub(*age)))
            .collect();
        if let Some(level) = out.get_mut(dim) {
            level.promises.push((n.id, Promises { list }));
        }
    }
    (out, owners)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(items: &[(&'static str, i32)]) -> Summary<&'static str> {
        let mut s = Summary::default();
        for (k, n) in items {
            s.add(k, *n);
        }
        s
    }

    #[test]
    fn summaries_add_count_and_sort_as_create() {
        let mut s = summary(&[("iron", 10), ("gold", 3), ("iron", 54)]);
        assert_eq!(s.count_of(&"iron"), 64);
        assert_eq!(s.count_of(&"dirt"), 0);
        assert_eq!(s.total(), 67);
        assert_eq!(s.stacks(), &[("iron", 64), ("gold", 3)]);
        s.add(&"dirt", 100);
        assert_eq!(s.by_count(), vec![("dirt", 100), ("iron", 64), ("gold", 3)]);
        // A creative crate counts as INF, and stays there.
        s.add(&"cobble", INF);
        s.add(&"cobble", 5);
        assert_eq!(s.count_of(&"cobble"), INF);
        assert!(s.erase(&"gold"));
        assert_eq!(s.count_of(&"gold"), 0);
        assert!(!s.is_empty());
        let mut net = Summary::default();
        let mut a = summary(&[("iron", 1)]);
        a.contributing_links = 1;
        net.add_all(&a);
        net.add_all(&a);
        assert_eq!((net.count_of(&"iron"), net.contributing_links), (2, 2));
        assert_eq!(net.total_of_matching(|k| k.starts_with('i')), 2);
    }

    fn source(p: i32, inv: Option<i32>, stock: &[(&'static str, i32)]) -> Source<&'static str> {
        Source {
            packager: Some([p, 0, 0]),
            inventory: inv.map(|i| [i, 0, 0]),
            stock: summary(stock),
        }
    }

    #[test]
    fn orders_split_across_packagers_in_link_order() {
        // 100 iron from three links: 40, 30, then 30 of the third's 50; 5 gold from the second.
        let sources = [
            source(1, Some(1), &[("iron", 40)]),
            source(2, Some(2), &[("iron", 30), ("gold", 9)]),
            source(3, Some(3), &[("iron", 50)]),
        ];
        let order = Order::simple(vec![("iron", 100), ("gold", 5)]);
        let r = find_packagers(&sources, &order, "Base", 7, |_| 0);
        let view: Vec<_> = r
            .iter()
            .map(|(p, q)| (p[0], q.item, q.count, q.link_index, q.context.is_some()))
            .collect();
        assert_eq!(
            view,
            vec![
                (1, "iron", 40, 0, true),
                (2, "iron", 30, 1, false),
                (3, "iron", 30, 2, false),
                (2, "gold", 5, 1, false),
            ]
        );
        assert!(
            r.iter()
                .all(|(_, q)| q.address == "Base" && q.order_id == 7)
        );
        // The order ends at link 1, not the last link first used: the final flag goes to that
        // link's (link 2's) requests, as Create's tracker does; link 1's gold keeps its own.
        assert!(r[2].1.final_link.get());
        assert!(!r[3].1.final_link.get());
        assert!(!r[0].1.final_link.get());
        // Grouped by packager in first-named order.
        let g = by_packager(r);
        assert_eq!(
            g.iter().map(|(p, q)| (p[0], q.len())).collect::<Vec<_>>(),
            vec![(1, 1), (2, 2), (3, 1)]
        );
    }

    #[test]
    fn the_last_link_used_carries_the_final_flag() {
        let sources = [
            source(1, None, &[("iron", 10)]),
            source(2, None, &[("iron", 10), ("gold", 10)]),
        ];
        let order = Order::simple(vec![("iron", 15), ("gold", 4)]);
        let r = find_packagers(&sources, &order, "x", 1, |_| 0);
        // iron: 10 + 5 (link 1 is now the last used); gold from link 1 too: shares its flag.
        assert_eq!(r.len(), 3);
        assert!(Rc::ptr_eq(&r[1].1.final_link, &r[2].1.final_link));
        assert!(r[2].1.final_link.get());
        assert!(!r[0].1.final_link.get());
        // Links reaching one inventory: one of them is picked.
        let sources = [
            source(1, Some(9), &[("iron", 10)]),
            source(2, Some(9), &[("iron", 10)]),
        ];
        let r = find_packagers(&sources, &Order::simple(vec![("iron", 15)]), "x", 1, |n| {
            n - 1
        });
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].0[0], r[0].1.count), (2, 10));
        // A link without a packager, or without the item, is passed by.
        let mut none = source(1, None, &[("iron", 10)]);
        none.packager = None;
        let r = find_packagers(
            &[none, source(2, None, &[("gold", 1)])],
            &Order::simple(vec![("iron", 1)]),
            "x",
            1,
            |_| 0,
        );
        assert!(r.is_empty());
    }

    fn slot(key: &'static str, count: u32) -> Option<Slot<&'static str>> {
        Some(Slot {
            key,
            count,
            max: 64,
            bulky: false,
            package: false,
        })
    }

    fn request(
        item: &'static str,
        count: u32,
        address: &str,
        order: i32,
        link: i32,
    ) -> Request<&'static str> {
        Request {
            item,
            count,
            address: address.into(),
            link_index: link,
            final_link: Rc::new(Cell::new(false)),
            counter: Rc::new(Cell::new(0)),
            order_id: order,
            context: None,
        }
    }

    #[test]
    fn requests_are_packed_into_numbered_fragments() {
        // 700 cobble requested from 12 stacks: two boxes of 576 and 124.
        let mut slots: Vec<_> = (0..12).map(|_| slot("cobble", 64)).collect();
        let mut q = VecDeque::from([request("cobble", 700, "Base", 3, 0)]);
        q[0].context = Some(Order::simple(vec![("cobble", 700)]));
        let a = pack_requests(&mut slots, &mut q).unwrap();
        let taken: u32 = a.packed.taken.iter().map(|t| t.1).sum();
        assert_eq!(taken, 576);
        assert_eq!(
            (a.order.fragment_index, a.order.is_final, a.address.as_str()),
            (0, false, "Base")
        );
        assert!(a.order.context.is_some());
        let b = pack_requests(&mut slots, &mut q).unwrap();
        let taken: u32 = b.packed.taken.iter().map(|t| t.1).sum();
        assert_eq!(taken, 124);
        assert_eq!((b.order.fragment_index, b.order.is_final), (1, true));
        assert!(q.is_empty());
        // Two requests to one address and order share a box; another address starts a new one.
        let mut slots = vec![slot("iron", 10), slot("gold", 10)];
        let mut q = VecDeque::from([
            request("iron", 4, "A", 1, 0),
            request("gold", 3, "A", 1, 0),
            request("gold", 2, "B", 2, 0),
        ]);
        let a = pack_requests(&mut slots, &mut q).unwrap();
        assert_eq!(a.packed.contents[0], Some((0, 4)));
        assert_eq!(a.packed.contents[1], Some((1, 3)));
        assert!(a.order.is_final);
        let b = pack_requests(&mut slots, &mut q).unwrap();
        assert_eq!(
            (b.address.as_str(), b.packed.contents[0]),
            ("B", Some((1, 2)))
        );
        // Nothing there: the request is dropped.
        let mut q = VecDeque::from([request("dirt", 1, "A", 1, 0)]);
        assert_eq!(pack_requests(&mut slots, &mut q), None);
        assert!(q.is_empty());
    }

    fn data(link: i32, fragment: i32, final_link: bool, is_final: bool) -> OrderData<&'static str> {
        OrderData {
            order_id: 5,
            link_index: link,
            final_link,
            fragment_index: fragment,
            is_final,
            context: None,
        }
    }

    #[test]
    fn orders_are_complete_when_every_fragment_is_there() {
        let a = data(0, 0, false, false);
        let b = data(0, 1, false, true);
        let c = data(1, 0, true, true);
        assert!(is_order_complete(&[&a, &b, &c]));
        assert!(is_order_complete(&[&c, &b, &a]), "in any order");
        assert!(
            !is_order_complete(&[&a, &c]),
            "link 0's last fragment missing"
        );
        assert!(!is_order_complete(&[&a, &b]), "link 1 missing");
        assert!(is_order_complete(&[&data(0, 0, true, true)]));
    }

    #[test]
    fn fragments_repack_complete_in_the_orders_order() {
        let mut first = data(0, 0, false, true);
        first.context = Some(Order::simple(vec![("gold", 3), ("iron", 70)]));
        let frags = [
            Fragment {
                order: first,
                address: "Base".into(),
                contents: vec![("iron", 64), ("dirt", 2)],
            },
            Fragment {
                order: data(1, 0, true, true),
                address: "Base".into(),
                contents: vec![("iron", 6), ("gold", 3)],
            },
        ];
        let out = repack(&frags, 5, |_| 64, || 0);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].contents,
            vec![("gold", 3), ("iron", 64), ("iron", 6), ("dirt", 2)]
        );
        assert_eq!(out[0].address, "Base");
        assert_eq!(
            (out[0].order.order_id, out[0].order.is_final, out[0].count),
            (5, true, 1)
        );
        assert!(
            out[0].order.context.is_some(),
            "the last box carries the order"
        );
        // Crafts: one box per pattern, as many as the stock makes.
        let mut d = data(0, 0, true, true);
        d.context = Some(Order {
            stacks: vec![("plank", 4)],
            crafts: vec![(vec![("plank", 1), ("plank", 1)], 2)],
        });
        let frags = [Fragment {
            order: d,
            address: "Saw".into(),
            contents: vec![("plank", 5)],
        }];
        let out = repack(&frags, 9, |_| 64, || 42);
        assert_eq!(out[0].contents, vec![("plank", 1), ("plank", 1)]);
        assert_eq!((out[0].count, out[0].order.order_id), (2, 42));
        // The plank left over goes in a box of its own.
        assert_eq!(out[1].contents, vec![("plank", 1)]);
        assert_eq!(out[1].order.order_id, 9);
    }

    #[test]
    fn redstone_requesters_send_on_a_rising_signal_when_stocked() {
        let mut r = Requester {
            order: Order::simple(vec![("iron", 10), ("gold", 5)]),
            address: "Base".into(),
            ..Default::default()
        };
        assert!(r.power(true));
        assert!(!r.power(true), "only a change");
        let stock = summary(&[("iron", 20), ("gold", 2)]);
        assert_eq!(r.trigger(&stock), Trigger::Denied);
        assert!(!r.last_succeeded);
        r.allow_partial = true;
        assert_eq!(r.trigger(&stock), Trigger::Send { any_in_stock: true });
        assert!(r.last_succeeded);
        assert!(!r.power(false));
        assert!(!r.last_succeeded);
        assert_eq!(
            Requester::<&str>::default().trigger(&stock),
            Trigger::Nothing
        );
    }

    #[test]
    fn gauges_request_below_their_target_only() {
        let mut p = Panel::new(1);
        p.filter = Some("iron");
        p.count = 2;
        p.restocker = true;
        // Two stacks wanted: 128.
        assert_eq!(p.look(64, 100, 0, 0), GaugeStep::Idle, "no address");
        p.recipe_address = "Here".into();
        assert_eq!(
            p.look(64, 100, 0, 0),
            GaugeStep::Restock {
                item: "iron",
                demand: 128
            }
        );
        assert_eq!(restock_amount(128, 0, 100, 64, 1000), 28);
        assert_eq!(restock_amount(128, 0, 100, 64, 5), 5);
        assert_eq!(
            restock_amount(1000, 0, 0, 16, 1000),
            144,
            "nine stacks at most"
        );
        assert_eq!(p.look(64, 100, 28, 0), GaugeStep::Idle, "promised");
        assert!(p.promised_satisfied && !p.satisfied);
        assert_eq!(p.look(64, 128, 0, 0), GaugeStep::Idle);
        assert!(p.satisfied);
        assert_eq!(p.look(64, 0, 0, 1), GaugeStep::Idle, "waiting for links");
        p.up_to = true;
        assert_eq!(p.demand(64), 2);
        // A panel fed by others requests.
        let mut q = Panel::new(1);
        q.filter = Some("plate");
        q.count = 1;
        q.up_to = true;
        q.recipe_address = "Press".into();
        assert_eq!(q.look(64, 0, 0, 0), GaugeStep::Idle, "nothing feeds it");
        q.inputs.push(([1, 0, 0], 2));
        q.inputs.push(([2, 0, 0], 3));
        assert_eq!(q.look(64, 0, 0, 0), GaugeStep::Request);
        let c = consolidate(&q.inputs, |_| Some((7, Some("iron")))).unwrap();
        assert_eq!(c, vec![(7, vec![(Some("iron"), 5)])]);
        let c = consolidate(&q.inputs, |p| Some((p[0] as u128, Some("iron")))).unwrap();
        assert_eq!(c.len(), 2, "by network");
        assert_eq!(consolidate::<&str>(&q.inputs, |_| None), None);
    }

    #[test]
    fn promises_expire_and_are_paid_off_by_arrivals() {
        let mut p = Promises::default();
        p.add("iron", 10, 0);
        p.add("iron", 5, 50);
        p.add("gold", 1, 0);
        assert_eq!(p.total(&"iron", None, 1000), 15);
        p.arrived(&"iron", 12);
        assert_eq!(p.list, vec![("iron", 3, 50), ("gold", 1, 0)]);
        assert_eq!(p.total(&"iron", Some(600), 700), 0, "expired");
        assert_eq!(p.total(&"gold", Some(600), 700), 0);
        let before = summary(&[("iron", 5)]);
        let after = summary(&[("iron", 9), ("gold", 2)]);
        assert_eq!(arrivals(&before, &after), vec![("iron", 4), ("gold", 2)]);
    }

    #[test]
    fn network_ids_round_trip() {
        let id = network_id("123e4567-e89b-12d3-a456-426614174000");
        assert_eq!(network_text(id), "123e4567-e89b-12d3-a456-426614174000");
        assert_eq!(network_from_ints(&network_ints(id)), Some(id));
        assert_eq!(network_id("Mine"), network_id("Mine"));
        assert_ne!(network_id("Mine"), network_id("Farm"));
    }

    #[test]
    fn networks_save_and_load_round_trip() {
        let (a, b) = (network_id("Base"), network_id("Mine"));
        let mut promises = Promises::default();
        promises.add("iron", 10, 900);
        promises.add("gold", 3, 990);
        let levels = vec![
            LevelNetworks {
                links: vec![([1, 2, 3], a), ([0, 0, 0], a), ([1, 2, 3], a)],
                promises: vec![(a, promises), (network_id("Gone"), Promises::default())],
            },
            LevelNetworks {
                links: vec![([5, 5, 5], b)],
                promises: vec![],
            },
            LevelNetworks::default(),
        ];
        let owners = HashMap::from([(
            a,
            Ownership {
                owner: Some(7),
                locked: true,
            },
        )]);
        let saved = save_networks(&levels, &owners, 1000);
        assert_eq!(saved.len(), 2, "networks without links are not kept");
        let base = saved.iter().find(|n| n.id == a).unwrap();
        assert_eq!(base.links, vec![(0, [0, 0, 0]), (0, [1, 2, 3])]);
        assert_eq!(base.promises, vec![("iron", 10, 100), ("gold", 3, 10)]);
        assert_eq!(base.ownership.owner, Some(7));
        // Read back after a restart at tick 1000 (the game time is saved too): the same
        // links, promises of the same age, the same owner; saved again, the same networks.
        let (back, owners_back) = load_networks(&saved, 3, 1000);
        assert_eq!(owners_back, owners);
        assert_eq!(back[1].links, vec![([5, 5, 5], b)]);
        assert_eq!(
            back[0].promises[0].1.list,
            vec![("iron", 10, 900), ("gold", 3, 990)]
        );
        assert_eq!(save_networks(&back, &owners_back, 1000), saved);
        // Ages count on from the save.
        let (later, _) = load_networks(&saved, 3, 5000);
        assert_eq!(later[0].promises[0].1.list[0].2, 4900);
    }
}
