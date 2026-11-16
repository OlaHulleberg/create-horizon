//! What Create's depots, item vaults and chutes give and take through `ITEMS`, for hoppers,
//! pipes and other mods (their `IItemHandler`s), ahead of the game's plain slots: their stacks
//! live in their block entities' slots, so the game's journal covers every write. The engine
//! learns of a change at its commit (`contents_changed`), never from inside a transaction.

use super::super::states;
use super::super::world::{level_index, with_world};
use crate::glue::items::filters;
use create_engine::Block;
use create_engine::ejector;
use mod_api::{
    BlockEntityRef, BlockSide, ItemResource, ItemStack, LevelRef, ResourceHandler, ServerHost,
    Transaction,
};

/// The engine's index of `level`.
fn dim_of(host: &dyn mod_api::Host, level: LevelRef) -> Option<usize> {
    level_index(&level.dimension().id(host)?)
}

/// The Create block at `pos` and its state's facts.
fn block_at(
    host: &dyn mod_api::Host,
    level: LevelRef,
    pos: [i32; 3],
) -> Option<(Block, create_engine::parts::StateInfo)> {
    let id = states::id(level.block_state(host, pos)?)?;
    with_world(|w| w.table.get(id).map(|(b, i)| (b, *i))).flatten()
}

/// The handler `side` asks the block of: a depot's, a vault's or a chute's; None for the rest.
pub fn provide(
    host: &mut dyn ServerHost,
    side: BlockSide,
) -> Option<Box<dyn ResourceHandler<ItemResource>>> {
    let (block, _) = block_at(&*host, side.level, side.pos)?;
    let dim = dim_of(&*host, side.level)?;
    let at = At {
        level: side.level,
        pos: side.pos,
        dim,
    };
    match block {
        Block::Depot | Block::WeightedEjector => Some(Box::new(DepotItems(at))),
        Block::ItemVault => {
            let parts = with_world(|w| w.dims[dim].vaults.inventory(side.pos)).flatten()?;
            Some(Box::new(VaultItems { at, parts }))
        }
        Block::Chute | Block::SmartChute => Some(Box::new(ChuteItems(at))),
        Block::BrassTunnel => Some(Box::new(TunnelItems(at))),
        Block::Packager | Block::Repackager | Block::PackageFrogport => {
            crate::glue::packages::packager_items(host, side, dim)
        }
        b if create_engine::dye::is_postbox(b) => {
            crate::glue::packages::packager_items(host, side, dim)
        }
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct At {
    level: LevelRef,
    pos: [i32; 3],
    dim: usize,
}

impl At {
    fn entity(&self, host: &dyn mod_api::Host, pos: [i32; 3]) -> Option<BlockEntityRef> {
        host.block_entity(self.level, pos)
    }
}

fn read(host: &dyn mod_api::Host, e: Option<BlockEntityRef>, slot: usize) -> Option<ItemStack> {
    e?.item_slot(host, slot as u32).ok().flatten()
}

fn limit(host: &dyn mod_api::Host, resource: &ItemResource) -> u32 {
    host.max_stack_size(resource.item())
        .unwrap_or(64)
        .clamp(1, 64)
}

/// Takes up to `amount` of `resource` out of slot `slot` of `e`.
fn take(
    host: &mut dyn ServerHost,
    tx: &Transaction,
    e: Option<BlockEntityRef>,
    slot: usize,
    resource: &ItemResource,
    amount: u32,
) -> u32 {
    let Some(e) = e else { return 0 };
    let Some(held) = read(&*host, Some(e), slot) else {
        return 0;
    };
    if !resource.matches(&held) {
        return 0;
    }
    let n = held.count().min(amount);
    if n == 0 {
        return 0;
    }
    let rest = held.shrink(n);
    match e.set_item_slot(host, tx, slot as u32, rest) {
        Ok(_) => n,
        Err(_) => 0,
    }
}

/// Puts up to `amount` of `resource` onto slot `slot` of `e`, up to `room` in all there.
fn put(
    host: &mut dyn ServerHost,
    tx: &Transaction,
    e: BlockEntityRef,
    slot: usize,
    resource: &ItemResource,
    amount: u32,
    room: u32,
) -> u32 {
    let held = read(&*host, Some(e), slot);
    let now = held.as_ref().map_or(0, ItemStack::count);
    if held.as_ref().is_some_and(|h| !resource.matches(h)) {
        return 0;
    }
    let n = amount.min(room.saturating_sub(now));
    if n == 0 {
        return 0;
    }
    let Some(stack) = resource.to_stack(now + n) else {
        return 0;
    };
    match e.set_item_slot(host, tx, slot as u32, Some(stack)) {
        Ok(_) => n,
        Err(_) => 0,
    }
}

/// `DepotItemHandler` (the ejector's `DepotBehaviour` too): in only to the held slot; a depot
/// while it holds nothing (`canMergeItems` false), an ejector only while charged, merging onto
/// what it holds up to its stack size (`enableMerging`, `getRemainingSpace`); out of every slot.
struct DepotItems(At);

impl ResourceHandler<ItemResource> for DepotItems {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        create_engine::depot::SLOTS
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        read(&*host, self.0.entity(&*host, self.0.pos), index).map(|s| ItemResource::of(&s))
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        read(&*host, self.0.entity(&*host, self.0.pos), index).map_or(0, |s| s.count())
    }
    fn capacity(&self, host: &mut dyn ServerHost, _index: usize, resource: &ItemResource) -> u32 {
        limit(&*host, resource)
    }
    fn is_valid(&self, _host: &mut dyn ServerHost, index: usize, _: &ItemResource) -> bool {
        index == 0
    }
    fn insert(
        &self,
        host: &mut dyn ServerHost,
        tx: &Transaction,
        index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        if index != 0 {
            return 0;
        }
        let Some(e) = self.0.entity(&*host, self.0.pos) else {
            return 0;
        };
        let slots: Vec<Option<ItemStack>> = (0..create_engine::depot::SLOTS)
            .map(|i| read(&*host, Some(e), i))
            .collect();
        let max = limit(&*host, resource);
        let ejector = with_world(|w| {
            w.dims[self.0.dim]
                .ejectors
                .get(&self.0.pos)
                .map(|e| (e.state, e.max_stack))
        })
        .flatten();
        let room = match ejector {
            None => {
                if slots.iter().any(Option::is_some) {
                    return 0;
                }
                max
            }
            Some((state, max_stack)) => {
                if state != ejector::State::Charged {
                    return 0;
                }
                let limit = if max_stack == 0 {
                    64
                } else {
                    u32::from(max_stack)
                }
                .min(max);
                let others: u32 = slots.iter().skip(1).flatten().map(ItemStack::count).sum();
                limit.saturating_sub(others)
            }
        };
        put(host, tx, e, 0, resource, amount, room)
    }
    fn extract(
        &self,
        host: &mut dyn ServerHost,
        tx: &Transaction,
        index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        let e = self.0.entity(&*host, self.0.pos);
        take(host, tx, e, index, resource, amount)
    }
}

/// `ItemVaultBlockEntity`'s capability: the whole vault's stacks (`CombinedInvWrapper` of each
/// block's `ItemStackHandler`, `vaultCapacity` stacks a block), in and out through any face.
struct VaultItems {
    at: At,
    parts: Vec<[i32; 3]>,
}

impl VaultItems {
    fn slot(&self, index: usize) -> Option<([i32; 3], usize)> {
        let n = create_engine::vault::SLOTS;
        Some((*self.parts.get(index / n)?, index % n))
    }
}

impl ResourceHandler<ItemResource> for VaultItems {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        self.parts.len() * create_engine::vault::SLOTS
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        let (pos, slot) = self.slot(index)?;
        read(&*host, self.at.entity(&*host, pos), slot).map(|s| ItemResource::of(&s))
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        let Some((pos, slot)) = self.slot(index) else {
            return 0;
        };
        read(&*host, self.at.entity(&*host, pos), slot).map_or(0, |s| s.count())
    }
    fn capacity(&self, host: &mut dyn ServerHost, _index: usize, resource: &ItemResource) -> u32 {
        limit(&*host, resource)
    }
    fn insert(
        &self,
        host: &mut dyn ServerHost,
        tx: &Transaction,
        index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        let Some((pos, slot)) = self.slot(index) else {
            return 0;
        };
        let Some(e) = self.at.entity(&*host, pos) else {
            return 0;
        };
        let room = limit(&*host, resource);
        put(host, tx, e, slot, resource, amount, room)
    }
    fn extract(
        &self,
        host: &mut dyn ServerHost,
        tx: &Transaction,
        index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        let Some((pos, slot)) = self.slot(index) else {
            return 0;
        };
        let e = self.at.entity(&*host, pos);
        take(host, tx, e, slot, resource, amount)
    }
}

/// `ChuteItemHandler`: one stack, taken only while the chute holds nothing and it may take
/// the item (`canAcceptItem`: a smart chute's filter, and not while powered).
struct ChuteItems(At);

impl ResourceHandler<ItemResource> for ChuteItems {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        1
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        read(&*host, self.0.entity(&*host, self.0.pos), index).map(|s| ItemResource::of(&s))
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        read(&*host, self.0.entity(&*host, self.0.pos), index).map_or(0, |s| s.count())
    }
    fn capacity(&self, host: &mut dyn ServerHost, _index: usize, resource: &ItemResource) -> u32 {
        limit(&*host, resource)
    }
    fn is_valid(&self, host: &mut dyn ServerHost, index: usize, resource: &ItemResource) -> bool {
        index == 0 && accepts(&*host, self.0, resource.as_stack())
    }
    fn insert(
        &self,
        host: &mut dyn ServerHost,
        tx: &Transaction,
        index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        if index != 0 || !accepts(&*host, self.0, resource.as_stack()) {
            return 0;
        }
        let Some(e) = self.0.entity(&*host, self.0.pos) else {
            return 0;
        };
        if read(&*host, Some(e), 0).is_some() {
            return 0;
        }
        let room = limit(&*host, resource);
        let n = put(host, tx, e, 0, resource, amount, room);
        if n > 0 {
            chute_given(host, self.0);
        }
        n
    }
    fn extract(
        &self,
        host: &mut dyn ServerHost,
        tx: &Transaction,
        index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        let e = self.0.entity(&*host, self.0.pos);
        take(host, tx, e, index, resource, amount)
    }
}

/// `ChuteItemHandler.insertItem` → `ChuteBlockEntity.setItem`: the stack starts moving from the
/// top, or from the bottom while the fans blow up the chute. A vanilla hopper's insert reaches
/// the chute at no commit it hears of (no `contents_changed`), so it is told here; should the
/// transaction abort, the chute's next tick finds it empty and lets it go again.
fn chute_given(host: &mut dyn ServerHost, at: At) {
    let Some(dimension) = at.level.dimension().id(&*host) else {
        return;
    };
    let Some(lv) = super::super::server::ServerLevel::new(host, &dimension) else {
        return;
    };
    let start = if super::chute::chute_motion(&lv, at.pos) > 0.0 {
        0.0
    } else {
        1.0
    };
    with_world(|w| {
        w.dims[at.dim].chutes.entry(at.pos).or_insert(start);
    });
}

/// `ChuteBlockEntity.canAcceptItem` / `SmartChuteBlockEntity.canAcceptItem`.
fn accepts(host: &dyn mod_api::Host, at: At, stack: &ItemStack) -> bool {
    match block_at(host, at.level, at.pos) {
        Some((Block::Chute, _)) => true,
        Some((Block::SmartChute, info)) => {
            !info.powered
                && with_world(|w| {
                    filters::passes(host, w.dims[at.dim].chute_filters.get(&at.pos), stack)
                })
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// `BrassTunnelItemHandler`: one slot, the stack the tunnel hands out. A distributing tunnel
/// (`hasDistributionBehaviour`) takes a stack, up to its stack size, while it holds none and
/// hands out with no synchronized group (`canTakeItems`); the rest (Create's: into the belt
/// below, and every extraction) go to the belt's handler, which belts have none of here.
struct TunnelItems(At);

impl TunnelItems {
    /// Whether the tunnel takes a stack now: distributing, its slot empty, not handing out
    /// with its synchronized group.
    fn takes(&self, host: &mut dyn ServerHost) -> bool {
        let Some(dimension) = self.0.level.dimension().id(&*host) else {
            return false;
        };
        let Some(lv) = super::super::server::ServerLevel::new(host, &dimension) else {
            return false;
        };
        let pos = self.0.pos;
        if !super::tunnel::has_distribution(&lv, pos) {
            return false;
        }
        let synced = with_world(|w| w.dims[self.0.dim].brass.get(&pos).is_some_and(|b| b.synced));
        synced == Some(false) && read(&*lv.host, lv.host.block_entity(lv.level, pos), 0).is_none()
    }
}

impl ResourceHandler<ItemResource> for TunnelItems {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        1
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        read(&*host, self.0.entity(&*host, self.0.pos), index).map(|s| ItemResource::of(&s))
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        read(&*host, self.0.entity(&*host, self.0.pos), index).map_or(0, |s| s.count())
    }
    fn capacity(&self, host: &mut dyn ServerHost, _index: usize, resource: &ItemResource) -> u32 {
        limit(&*host, resource)
    }
    fn insert(
        &self,
        host: &mut dyn ServerHost,
        tx: &Transaction,
        index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        if index != 0 || !self.takes(host) {
            return 0;
        }
        let Some(e) = self.0.entity(&*host, self.0.pos) else {
            return 0;
        };
        let room = limit(&*host, resource);
        put(host, tx, e, 0, resource, amount, room)
    }
    fn extract(
        &self,
        _host: &mut dyn ServerHost,
        _tx: &Transaction,
        _index: usize,
        _resource: &ItemResource,
        _amount: u32,
    ) -> u32 {
        0
    }
}
