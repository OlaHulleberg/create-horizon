//! An inventory as a packager or a frogport reaches it: a packager (`PackagerItemHandler`), a
//! port (`PackagePortAutomationInventoryWrapper`), or any other block's `ITEMS` capability
//! through one face (a chest, a vault, a depot: whatever answers). Moves through a capability
//! run in a transaction each; Create's `simulate` is a transaction that is aborted.

use super::{Pk, body, parcel, with};
use crate::glue::server::ServerLevel;
use create_engine::chain_conveyor::Parcel;
use create_engine::package;
use mod_api::{
    Capability, Direction, HostTransaction, ITEMS, ItemResource, ItemStack, ResourceHandler,
};

pub type Items = Capability<dyn ResourceHandler<ItemResource>>;

pub enum Inv {
    Packager([i32; 3]),
    Port([i32; 3]),
    Cap(Items),
}

impl Inv {
    pub fn is_packager(&self) -> bool {
        matches!(self, Inv::Packager(_))
    }
}

/// What is at `pos` as an inventory, reached through its `face`.
pub fn at(lv: &mut ServerLevel<'_>, dim: usize, pos: [i32; 3], face: Direction) -> Option<Inv> {
    let (packager, port) = with(dim, |pk| {
        (pk.packagers.contains_key(&pos), pk.ports.contains_key(&pos))
    });
    if packager {
        return Some(Inv::Packager(pos));
    }
    if port {
        return Some(Inv::Port(pos));
    }
    lv.level
        .capability(&mut *lv.host, &ITEMS, pos, Some(face))
        .map(Inv::Cap)
}

/// The stacks of a capability, by index.
pub fn stacks(lv: &mut ServerLevel<'_>, items: &Items) -> Vec<Option<ItemStack>> {
    let host = &mut *lv.host;
    (0..items.size(host))
        .map(|i| {
            let r = items.resource(host, i)?;
            r.to_stack(items.amount(host, i))
        })
        .collect()
}

/// The stacks automation could take from `inv`, by slot (None where nothing may be taken).
pub fn view(lv: &mut ServerLevel<'_>, dim: usize, inv: &Inv) -> Vec<Option<ItemStack>> {
    match inv {
        Inv::Packager(p) => with(dim, |pk| {
            let k = pk.packager(*p);
            vec![if k.animation_ticks == 0 {
                k.held.as_ref().map(|b| b.stack.clone())
            } else {
                None
            }]
        }),
        Inv::Port(p) => with(dim, |pk| {
            let port = pk.ports.get(p);
            port.map(|port| {
                port.items
                    .iter()
                    .map(|s| {
                        s.as_ref()
                            .filter(|b| port.automation_gives(&b.address))
                            .map(|b| b.stack.clone())
                    })
                    .collect()
            })
            .unwrap_or_default()
        }),
        Inv::Cap(items) => stacks(lv, items),
    }
}

/// Up to `n` from slot `slot` of `inv` (as `view` numbers them).
pub fn take(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    inv: &Inv,
    slot: usize,
    n: u32,
) -> Option<Parcel<ItemStack>> {
    match inv {
        Inv::Packager(p) => with(dim, |pk| take_box(pk, *p)),
        Inv::Port(p) => with(dim, |pk| {
            let port = pk.ports.get_mut(p)?;
            let b = port.items.get_mut(slot)?.take();
            pk.touch(*p);
            b
        }),
        Inv::Cap(items) => {
            let host = &mut *lv.host;
            let r = items.resource(host, slot)?;
            let got = host
                .transaction(|h, tx| match items.extract(h, tx, slot, &r, n) {
                    0 => Err(()),
                    got => Ok(got),
                })
                .ok()?;
            r.to_stack(got).map(parcel)
        }
    }
}

/// `PackagerItemHandler.extractItem`: the held box; the next queued one may come out after.
pub fn take_box(pk: &mut Pk, pos: [i32; 3]) -> Option<Parcel<ItemStack>> {
    let k = pk.packager(pos);
    let b = k.take();
    let queued = !k.queue.is_empty();
    pk.touch(pos);
    if b.is_some() && queued {
        pk.activate(pos);
    }
    b
}

/// `insertItemStacked` into a capability: what is left.
pub fn insert_items(
    lv: &mut ServerLevel<'_>,
    items: &Items,
    stack: ItemStack,
) -> Option<ItemStack> {
    let host = &mut *lv.host;
    let r = ItemResource::of(&stack);
    let count = stack.count();
    let went = host
        .transaction(|h, tx| Ok::<u32, ()>(items.insert_any(h, tx, &r, count)))
        .unwrap_or(0);
    stack.shrink(went)
}

/// Whether every one of `stacks` would go into `items` (`insertItemStacked(.., simulate)`):
/// inserted in a transaction that is then aborted.
pub fn fits(lv: &mut ServerLevel<'_>, items: &Items, stacks: &[ItemStack]) -> bool {
    lv.host
        .transaction(|h, tx| -> Result<(), bool> {
            for s in stacks {
                let r = ItemResource::of(s);
                if items.insert_any(h, tx, &r, s.count()) < s.count() {
                    return Err(false);
                }
            }
            Err(true)
        })
        .err()
        .unwrap_or(false)
}

/// Whether some index of `items` is empty (a whole package goes in there).
pub fn has_empty(lv: &mut ServerLevel<'_>, items: &Items) -> bool {
    stacks(lv, items).iter().any(Option::is_none)
}

/// `insertItemStacked` / `insertItem` into `inv`: what is left (None when all went in).
pub fn insert(
    lv: &mut ServerLevel<'_>,
    dim: usize,
    inv: &Inv,
    p: Parcel<ItemStack>,
) -> Option<Parcel<ItemStack>> {
    match inv {
        Inv::Packager(at) => {
            // PackagerItemHandler.insertItem: a package, nothing held, and its contents fit.
            let free = with(dim, |pk| pk.packager(*at).can_unwrap());
            let id = body::item_id(&*lv.host, &p.stack);
            if !free || !package::is_package(&id) || !super::unwrap(lv, dim, *at, &p.stack, true) {
                return Some(p);
            }
            super::unwrap(lv, dim, *at, &p.stack, false);
            None
        }
        Inv::Port(at) => {
            let id = body::item_id(&*lv.host, &p.stack);
            with(dim, |pk| {
                let Some(port) = pk.ports.get_mut(at) else {
                    return Some(p);
                };
                if !package::is_package(&id) || !port.automation_takes(&p.address) {
                    return Some(p);
                }
                let left = port.insert(p);
                pk.touch(*at);
                if left.is_none() {
                    pk.wake(*at);
                }
                left
            })
        }
        Inv::Cap(items) => {
            let address = p.address.clone();
            insert_items(lv, items, p.stack).map(|stack| Parcel { stack, address })
        }
    }
}
