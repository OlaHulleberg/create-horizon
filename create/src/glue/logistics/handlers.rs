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

/// The handler `side` asks the block of: a depot's, a belt segment's, a vault's or a chute's;
/// None for the rest.
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
        Block::Belt => {
            // `BeltBlockEntity.registerCapabilities`: none where the belt cannot transport
            // (`canTransportObjects`: not sideways or vertical).
            let transports = with_world(|w| {
                let Some(c) = w.dims[dim].belt_of.get(&side.pos).copied() else {
                    return false;
                };
                w.dims[dim]
                    .belts
                    .get(&c)
                    .is_some_and(|b| b.segment.transports())
            })
            .unwrap_or(false);
            transports.then(|| Box::new(BeltItems(at)) as Box<dyn ResourceHandler<ItemResource>>)
        }
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

/// `ItemHandlerBeltSegment` (registered by `BeltBlockEntity.registerCapabilities`): the belt
/// segment's own slot. In: onto the segment while `canInsertAt` allows, limited to one stack,
/// from the next tick (`addItem`); out: `getStackAtOffset`, split in place, one it empties only
/// marked `toRemove`. The stack lives in the engine, so an insert the host's transaction later
/// aborts cannot be taken back (no caller of this mod does: `insert_room` answers belts from
/// the engine, and the game's own moves commit).
struct BeltItems(At);

impl BeltItems {
    /// `getStackAtOffset(offset)`: the engine's first item inside the segment at `pos`, with
    /// the segment's offset in its belt.
    fn stack_at(
        &self,
        host: &dyn mod_api::Host,
    ) -> Option<([i32; 3], i32, create_engine::belt::Item)> {
        let dim = dim_of(host, self.0.level)?;
        with_world(|w| {
            let d = &w.dims[dim];
            let c = d.belt_of.get(&self.0.pos).copied()?;
            let b = d.belts.get(&c)?;
            if !b.segment.transports() {
                return None;
            }
            let i = b.chain.iter().position(|p| *p == self.0.pos)? as i32;
            b.items.stack_at(i).map(|s| (c, i, s.clone()))
        })
        .flatten()
    }
}

impl ResourceHandler<ItemResource> for BeltItems {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        1
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        if index != 0 {
            return None;
        }
        let (_, _, item) = self.stack_at(&*host)?;
        let dimension = self.0.level.dimension().id(&*host)?;
        let lv = super::super::server::ServerLevel::new(host, &dimension)?;
        super::belt::belt_stack(&lv, &item).map(|s| ItemResource::of(&s))
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        if index != 0 {
            return 0;
        }
        self.stack_at(&*host)
            .map_or(0, |(_, _, s)| u32::from(s.count))
    }
    fn capacity(&self, host: &mut dyn ServerHost, _index: usize, resource: &ItemResource) -> u32 {
        limit(&*host, resource)
    }
    fn insert(
        &self,
        host: &mut dyn ServerHost,
        _tx: &Transaction,
        index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        if index != 0 {
            return 0;
        }
        let Some(stack) = resource.to_stack(amount.min(u32::from(u8::MAX))) else {
            return 0;
        };
        let Some(dimension) = self.0.level.dimension().id(&*host) else {
            return 0;
        };
        let Some(mut lv) = super::super::server::ServerLevel::new(host, &dimension) else {
            return 0;
        };
        // `insertItem`: canInsertAt, the count limited to one stack, the rest refused.
        let (id, progress) = super::belt::belt_name(&*lv.host, &stack);
        let count = stack.count().min(u32::from(u8::MAX)) as u8;
        match super::belt::put_on_from_above(&mut lv, self.0.pos, &id, count, progress, false) {
            Some(left) => u32::from(count - left),
            None => 0,
        }
    }
    fn extract(
        &self,
        host: &mut dyn ServerHost,
        _tx: &Transaction,
        index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        if index != 0 {
            return 0;
        }
        let Some((c, segment, item)) = self.stack_at(&*host) else {
            return 0;
        };
        let Some(dimension) = self.0.level.dimension().id(&*host) else {
            return 0;
        };
        let Some(lv) = super::super::server::ServerLevel::new(host, &dimension) else {
            return 0;
        };
        if !super::belt::belt_stack(&lv, &item).is_some_and(|s| resource.matches(&s)) {
            return 0;
        }
        let dim = lv.dim;
        with_world(|w| {
            let d = &mut w.dims[dim];
            let b = d.belts.get_mut(&c)?;
            let taken = b
                .items
                .extract_at(segment, amount.min(u32::from(u8::MAX)) as u8, false)?
                .1;
            if taken > 0 {
                d.belt_changes.push(c);
            }
            Some(u32::from(taken))
        })
        .flatten()
        .unwrap_or(0)
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
/// hands out with no synchronized group (`canTakeItems`); otherwise inserts pass to the belt
/// below (`getBeltCapability`), and extraction always delegates to the belt below.
struct TunnelItems(At);

impl TunnelItems {
    fn belt(&self) -> BeltItems {
        BeltItems(At {
            pos: [self.0.pos[0], self.0.pos[1] - 1, self.0.pos[2]],
            ..self.0
        })
    }

    fn target(&self, host: &mut dyn ServerHost) -> Option<create_engine::tunnel::HandlerTarget> {
        let dimension = self.0.level.dimension().id(&*host)?;
        let lv = super::super::server::ServerLevel::new(host, &dimension)?;
        let pos = self.0.pos;
        let target = super::tunnel::handler_target(&lv, pos)?;
        if target == create_engine::tunnel::HandlerTarget::Distribute
            && read(&*lv.host, lv.host.block_entity(lv.level, pos), 0).is_some()
        {
            return Some(create_engine::tunnel::HandlerTarget::Busy);
        }
        Some(target)
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
        match self.target(host) {
            Some(create_engine::tunnel::HandlerTarget::BeltBelow) => {
                self.belt().insert(host, tx, index, resource, amount)
            }
            Some(create_engine::tunnel::HandlerTarget::Distribute) if index == 0 => {
                let Some(e) = self.0.entity(&*host, self.0.pos) else {
                    return 0;
                };
                let room = limit(&*host, resource);
                put(host, tx, e, 0, resource, amount, room)
            }
            _ => 0,
        }
    }
    fn extract(
        &self,
        host: &mut dyn ServerHost,
        tx: &Transaction,
        index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        self.belt().extract(host, tx, index, resource, amount)
    }
}
