//! The creative crate as an inventory (`BottomlessItemHandler`): it gives its supply, a stack at a
//! time, as often as asked, and takes whatever it is given, which is gone.

use super::super::server::ServerLevel;
use super::super::states;
use super::{crate_supply, stack_of};
use create_engine::Block;
use create_engine::decor;
use mod_api::{
    BlockSide, ITEMS, ItemResource, Registrar, ResourceHandler, ServerHost, Transaction,
};

pub(super) fn register(r: &mut dyn Registrar) {
    r.block_capability(&ITEMS, &states::key(Block::CreativeCrate), |_, at, _, _| {
        Some(Box::new(Bottomless(at)) as Box<dyn ResourceHandler<ItemResource>>)
    });
}

struct Bottomless(BlockSide);

impl Bottomless {
    /// The supply, as one item, and the most of it a stack holds.
    fn supply(&self, host: &mut dyn ServerHost) -> Option<(ItemResource, u32)> {
        let dimension = self.0.level.dimension().id(&*host)?;
        let lv = ServerLevel::new(host, &dimension)?;
        let id = crate_supply(&lv, self.0.pos)??;
        let one = stack_of(&*lv.host, &id, 1)?;
        let max = lv.host.max_stack_size(one.item()).unwrap_or(64);
        Some((ItemResource::of(&one), max))
    }
}

impl ResourceHandler<ItemResource> for Bottomless {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        1
    }

    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        (index == 0)
            .then(|| self.supply(host).map(|s| s.0))
            .flatten()
    }

    /// `getStackInSlot`: a full stack of the supply.
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        if index != 0 {
            return 0;
        }
        self.supply(host).map_or(0, |s| s.1)
    }

    fn capacity(&self, host: &mut dyn ServerHost, _index: usize, resource: &ItemResource) -> u32 {
        host.max_stack_size(resource.item()).unwrap_or(64)
    }

    /// `insertItem`: everything is taken (and gone).
    fn insert(
        &self,
        _host: &mut dyn ServerHost,
        _tx: &Transaction,
        index: usize,
        _resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        if index == 0 { amount } else { 0 }
    }

    /// `extractItem`: the supply, up to a stack.
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
        let Some((supply, max)) = self.supply(host) else {
            return 0;
        };
        if supply.item() != resource.item() {
            return 0;
        }
        let id = supply.item().id(&*host);
        decor::crate_extract(Some(&id), amount, max).map_or(0, |(_, n)| n)
    }
}
