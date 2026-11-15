//! Packagers and package ports as other blocks' automation reaches them. A packager
//! (`PackagerItemHandler`): hoppers, funnels, chutes and arms take the box it holds once it is
//! out (not while its tray moves), and put a package in to be unpacked while it holds none and
//! has none queued, when the package's contents fit behind it (`unwrapBox(.., simulate)`). A
//! frogport or postbox (`PackagePortAutomationInventoryWrapper` over its 18 slots): packages not
//! for it go in (to be sent), packages for it (its address) come out.
//!
//! Their state is the engine's, which a transaction cannot roll back, so what automation sees
//! lives in the block entity's slots, which the game journals: a packager's slot 0 mirrors its
//! held box and its slot 1 takes a package put in; a port's 18 slots mirror its packages. The
//! mirrors are written whenever the engine's state changes (`sync`). A handler's slot writes
//! give no `contents_changed` (the game gives none for a mod's own storage writes), so the
//! blocks a handler wrote are looked at again at the start of their level's next tick
//! (`reconcile`), when the move has committed or been rolled back, and the engine learns what
//! automation did: a packager's emptied slot 0 is its box taken (`extractItem`), a package in
//! its slot 1 is unpacked (`unwrapBox`); a port's slots that differ from what was last written
//! are its packages now. A broken block drops what
//! its slots hold, so the engine drops only what is not mirrored.

use super::{Block, ServerLevel, body, center, drop_stack, inv, unwrap, with};
use create_engine::package;
use mod_api::{
    BlockEntityRef, BlockSide, HostTransaction, ItemResource, ItemStack, ModContainer,
    ResourceHandler, ServerHost, Transaction,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// The held box's mirror.
const HELD: u32 = 0;
/// A package put in, unpacked at the commit.
const INCOMING: u32 = 1;

/// A block by its level and position.
type At = (usize, [i32; 3]);

thread_local! {
    /// What each packager's slot 0 was last set to: the held box, or none.
    static MIRRORED: RefCell<HashMap<At, Option<ItemStack>>> = RefCell::new(HashMap::new());
    /// What each port's slots were last set to.
    static PORTS: RefCell<HashMap<At, Vec<Option<ItemStack>>>> = RefCell::new(HashMap::new());
    /// The packagers and ports a handler wrote since their level last looked (`reconcile`).
    static WRITTEN: RefCell<Vec<At>> = const { RefCell::new(Vec::new()) };
}

/// Marks the block at `pos` as written by a handler, to be looked at in `reconcile`.
fn written(dim: usize, pos: [i32; 3]) {
    WRITTEN.with_borrow_mut(|w| {
        if !w.contains(&(dim, pos)) {
            w.push((dim, pos));
        }
    });
}

/// What automation did to this level's packagers and ports since the last tick (their
/// handlers' writes, committed or rolled back by now).
pub(super) fn reconcile(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    if WRITTEN.with_borrow(Vec::is_empty) {
        return;
    }
    let mine: Vec<[i32; 3]> = WRITTEN.with_borrow_mut(|w| {
        let mine = w.iter().filter(|x| x.0 == dim).map(|x| x.1).collect();
        w.retain(|x| x.0 != dim);
        mine
    });
    for pos in mine {
        let (packager, port) = with(dim, |pk| {
            (pk.packagers.contains_key(&pos), pk.ports.contains_key(&pos))
        });
        if packager {
            contents_changed(lv, pos);
        } else if port {
            port_contents_changed(lv, pos);
        }
    }
}

pub(super) fn reset() {
    MIRRORED.with_borrow_mut(HashMap::clear);
    PORTS.with_borrow_mut(HashMap::clear);
    WRITTEN.with_borrow_mut(Vec::clear);
}

/// The packagers' slots (two) and the ports' (18), reached by no face through the game's own
/// handler (`provide` answers for them first).
pub(super) fn register(r: &mut dyn mod_api::Registrar) {
    for &b in Block::ALL
        .iter()
        .filter(|&&b| super::is_packager(b) || super::is_port(b))
    {
        r.block_container(
            &crate::glue::states::key(b),
            ModContainer {
                slots: if super::is_port(b) { PORT_SLOTS } else { 2 },
                faces: std::array::from_fn(|_| Vec::new()),
                inputs: Vec::new(),
                outputs: Vec::new(),
                menu: None,
            },
        );
    }
}

const PORT_SLOTS: usize = create_engine::port::SLOTS;

/// Of `items` (a removed port's), those its slots don't hold: what the engine still drops.
pub(super) fn port_unmirrored(dim: usize, pos: [i32; 3], items: Vec<ItemStack>) -> Vec<ItemStack> {
    let mut mirrored: Vec<ItemStack> = PORTS
        .with_borrow_mut(|m| m.remove(&(dim, pos)))
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect();
    items
        .into_iter()
        .filter(
            |s| match mirrored.iter().position(|m| same(Some(m), Some(s))) {
                Some(i) => {
                    mirrored.swap_remove(i);
                    false
                }
                None => true,
            },
        )
        .collect()
}

/// Whether the box the packager at `pos` holds is in its slot 0 (a broken one drops it).
pub(super) fn mirrored(dim: usize, pos: [i32; 3]) -> bool {
    MIRRORED.with_borrow(|m| m.get(&(dim, pos)).is_some_and(Option::is_some))
}

/// Forgets a packager gone.
pub(super) fn forget(dim: usize, pos: [i32; 3]) {
    MIRRORED.with_borrow_mut(|m| m.remove(&(dim, pos)));
}

fn same(a: Option<&ItemStack>, b: Option<&ItemStack>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.is_stackable_with(b) && a.count() == b.count(),
        _ => false,
    }
}

/// Writes each packager's held box into its slot 0, and each port's packages into its slots,
/// where they changed since last written.
pub(super) fn sync(lv: &mut ServerLevel<'_>) {
    sync_packagers(lv);
    sync_ports(lv);
}

fn sync_ports(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let ports: Vec<([i32; 3], Vec<Option<ItemStack>>)> = with(dim, |pk| {
        pk.ports
            .iter()
            .map(|(p, port)| {
                let items = port
                    .items
                    .iter()
                    .map(|s| s.as_ref().map(|b| b.stack.clone()));
                (*p, items.collect())
            })
            .collect()
    });
    for (pos, items) in ports {
        let last = PORTS.with_borrow(|m| m.get(&(dim, pos)).cloned());
        let changed: Vec<usize> = (0..PORT_SLOTS)
            .filter(|&i| {
                let now = items.get(i).cloned().flatten();
                let was = last.as_ref().map(|l| l.get(i).cloned().flatten());
                was.is_none_or(|w| !same(w.as_ref(), now.as_ref()))
            })
            .collect();
        if changed.is_empty() {
            continue;
        }
        let Some(e) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let written = lv
            .host
            .transaction(|h, tx| {
                for &i in &changed {
                    let s = items.get(i).cloned().flatten();
                    e.set_item_slot(h, tx, i as u32, s).map_err(|_| ())?;
                }
                Ok::<(), ()>(())
            })
            .is_ok();
        if written {
            let mut all = items;
            all.resize(PORT_SLOTS, None);
            PORTS.with_borrow_mut(|m| m.insert((dim, pos), all));
        }
    }
}

/// `contents_changed` of a port: the slots automation changed since they were last written
/// are its packages now.
pub(super) fn port_contents_changed(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(e) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let Some(last) = PORTS.with_borrow(|m| m.get(&(dim, pos)).cloned()) else {
        return;
    };
    let now: Vec<Option<ItemStack>> = (0..PORT_SLOTS)
        .map(|i| e.item_slot(&*lv.host, i as u32).ok().flatten())
        .collect();
    let mut any = false;
    with(dim, |pk| {
        let Some(port) = pk.ports.get_mut(&pos) else {
            return;
        };
        for (i, stack) in now.iter().enumerate() {
            let was = last.get(i).cloned().flatten();
            if same(was.as_ref(), stack.as_ref()) {
                continue;
            }
            if let Some(slot) = port.items.get_mut(i) {
                *slot = stack.clone().map(super::parcel);
                any = true;
            }
        }
        if any {
            pk.touch(pos);
            pk.wake(pos);
        }
    });
    if any {
        PORTS.with_borrow_mut(|m| m.insert((dim, pos), now));
        super::mark_unsaved(dim, &[pos]);
    }
}

fn sync_packagers(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let held: Vec<([i32; 3], Option<ItemStack>)> = with(dim, |pk| {
        pk.packagers
            .iter()
            .map(|(p, k)| (*p, k.held.as_ref().map(|b| b.stack.clone())))
            .collect()
    });
    for (pos, box_) in held {
        let last = MIRRORED.with_borrow(|m| m.get(&(dim, pos)).cloned());
        if last
            .as_ref()
            .is_some_and(|l| same(l.as_ref(), box_.as_ref()))
        {
            continue;
        }
        let Some(e) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let written = lv
            .host
            .transaction(|h, tx| e.set_item_slot(h, tx, HELD, box_.clone()).map_err(|_| ()))
            .is_ok();
        if written {
            MIRRORED.with_borrow_mut(|m| m.insert((dim, pos), box_));
        }
    }
}

/// `contents_changed` of a packager: automation took its box, or put a package in.
pub(super) fn contents_changed(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(e) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let slot = |host: &dyn mod_api::Host, i: u32| e.item_slot(host, i).ok().flatten();
    let now = slot(&*lv.host, HELD);
    let last = MIRRORED.with_borrow(|m| m.get(&(dim, pos)).cloned().flatten());
    if now.is_none() && last.is_some() {
        // `extractItem`: the box went.
        let taken = with(dim, |pk| inv::take_box(pk, pos));
        MIRRORED.with_borrow_mut(|m| m.insert((dim, pos), None));
        if let (Some(t), Some(l)) = (&taken, &last)
            && !same(Some(&t.stack), Some(l))
        {
            crate::create_log!("[create] packager {pos:?}: its box changed as automation took it");
        }
    }
    let Some(incoming) = slot(&*lv.host, INCOMING) else {
        return;
    };
    let cleared = lv
        .host
        .transaction(|h, tx| e.set_item_slot(h, tx, INCOMING, None).map_err(|_| ()))
        .is_ok();
    if !cleared {
        return;
    }
    // `insertItem` (not simulated): unpacked; one that no longer fits drops.
    if !(with(dim, |pk| pk.packager(pos).can_unwrap()) && unwrap(lv, dim, pos, &incoming, false)) {
        drop_stack(lv, center(pos), incoming);
    }
}

/// The handler `side` asks a packager or a port for.
pub(crate) fn provide(
    host: &mut dyn ServerHost,
    side: BlockSide,
    dim: usize,
) -> Option<Box<dyn ResourceHandler<ItemResource>>> {
    host.block_entity(side.level, side.pos)?;
    let (packager, port) = with(dim, |pk| {
        (
            pk.packagers.contains_key(&side.pos),
            pk.ports.contains_key(&side.pos),
        )
    });
    let (level, pos) = (side.level, side.pos);
    if packager {
        Some(Box::new(PackagerItems { level, pos, dim }))
    } else if port {
        Some(Box::new(PortItems { level, pos, dim }))
    } else {
        None
    }
}

/// `PackagePortAutomationInventoryWrapper` over the port's 18 slots: in, a package not for it,
/// into an empty slot; out, a package for it.
struct PortItems {
    level: mod_api::LevelRef,
    pos: [i32; 3],
    dim: usize,
}

impl PortItems {
    fn slot(&self, host: &dyn mod_api::Host, i: usize) -> Option<ItemStack> {
        host.block_entity(self.level, self.pos)?
            .item_slot(host, i as u32)
            .ok()
            .flatten()
    }

    /// Whether the port's slots are mirrored (its engine state is known to them).
    fn known(&self) -> bool {
        PORTS.with_borrow(|m| m.contains_key(&(self.dim, self.pos)))
    }
}

impl ResourceHandler<ItemResource> for PortItems {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        PORT_SLOTS
    }
    fn resource(&self, host: &mut dyn ServerHost, index: usize) -> Option<ItemResource> {
        self.slot(&*host, index).map(|s| ItemResource::of(&s))
    }
    fn amount(&self, host: &mut dyn ServerHost, index: usize) -> u32 {
        self.slot(&*host, index).map_or(0, |s| s.count())
    }
    fn capacity(&self, _host: &mut dyn ServerHost, _index: usize, _r: &ItemResource) -> u32 {
        1
    }
    fn is_valid(&self, host: &mut dyn ServerHost, _index: usize, r: &ItemResource) -> bool {
        r.to_stack(1)
            .is_some_and(|s| package::is_package(&body::item_id(&*host, &s)))
    }
    fn insert(
        &self,
        host: &mut dyn ServerHost,
        tx: &Transaction,
        index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        if amount == 0 || index >= PORT_SLOTS || !self.known() {
            return 0;
        }
        let Some(stack) = resource.to_stack(1) else {
            return 0;
        };
        if !package::is_package(&body::item_id(&*host, &stack)) {
            return 0;
        }
        let address = body::address(&stack);
        let takes = with(self.dim, |pk| {
            pk.ports
                .get(&self.pos)
                .is_some_and(|p| p.automation_takes(&address))
        });
        if !takes || self.slot(&*host, index).is_some() {
            return 0;
        }
        let Some(e) = host.block_entity(self.level, self.pos) else {
            return 0;
        };
        let ok = e.set_item_slot(host, tx, index as u32, Some(stack)).is_ok();
        if ok {
            written(self.dim, self.pos);
        }
        u32::from(ok)
    }
    fn extract(
        &self,
        host: &mut dyn ServerHost,
        tx: &Transaction,
        index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        if amount == 0 || index >= PORT_SLOTS || !self.known() {
            return 0;
        }
        let Some(held) = self.slot(&*host, index) else {
            return 0;
        };
        if !resource.matches(&held) {
            return 0;
        }
        let address = body::address(&held);
        let gives = with(self.dim, |pk| {
            pk.ports
                .get(&self.pos)
                .is_some_and(|p| p.automation_gives(&address))
        });
        if !gives {
            return 0;
        }
        let Some(e) = host.block_entity(self.level, self.pos) else {
            return 0;
        };
        let n = held.count().min(amount);
        match e.set_item_slot(host, tx, index as u32, held.shrink(n)) {
            Ok(_) => {
                written(self.dim, self.pos);
                n
            }
            Err(_) => 0,
        }
    }
}

/// `PackagerItemHandler`: one slot, the held box.
struct PackagerItems {
    level: mod_api::LevelRef,
    pos: [i32; 3],
    dim: usize,
}

impl PackagerItems {
    fn entity(&self, host: &dyn mod_api::Host) -> Option<BlockEntityRef> {
        host.block_entity(self.level, self.pos)
    }

    fn slot(&self, host: &dyn mod_api::Host, i: u32) -> Option<ItemStack> {
        self.entity(host)?.item_slot(host, i).ok().flatten()
    }
}

impl ResourceHandler<ItemResource> for PackagerItems {
    fn size(&self, _host: &mut dyn ServerHost) -> usize {
        1
    }
    fn resource(&self, host: &mut dyn ServerHost, _index: usize) -> Option<ItemResource> {
        self.slot(&*host, HELD).map(|s| ItemResource::of(&s))
    }
    fn amount(&self, host: &mut dyn ServerHost, _index: usize) -> u32 {
        self.slot(&*host, HELD).map_or(0, |s| s.count())
    }
    fn capacity(&self, _host: &mut dyn ServerHost, _index: usize, _r: &ItemResource) -> u32 {
        1
    }
    fn is_valid(&self, host: &mut dyn ServerHost, _index: usize, r: &ItemResource) -> bool {
        r.to_stack(1)
            .is_some_and(|s| package::is_package(&body::item_id(&*host, &s)))
    }
    /// `insertItem`: one package, while nothing is held or queued, its contents fitting behind.
    fn insert(
        &self,
        host: &mut dyn ServerHost,
        tx: &Transaction,
        _index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        if amount == 0 {
            return 0;
        }
        let Some(stack) = resource.to_stack(1) else {
            return 0;
        };
        if !package::is_package(&body::item_id(&*host, &stack)) {
            return 0;
        }
        let free = with(self.dim, |pk| {
            let k = pk.packager(self.pos);
            k.held.is_none() && k.queue.is_empty() && k.can_unwrap()
        });
        if !free || self.slot(&*host, HELD).is_some() || self.slot(&*host, INCOMING).is_some() {
            return 0;
        }
        let Some(dimension) = self.level.dimension().id(&*host).map(|d| d.to_string()) else {
            return 0;
        };
        let fits = match ServerLevel::new(host, &dimension) {
            Some(mut lv) => unwrap(&mut lv, self.dim, self.pos, &stack, true),
            None => false,
        };
        let Some(e) = self.entity(&*host) else {
            return 0;
        };
        if fits && e.set_item_slot(host, tx, INCOMING, Some(stack)).is_ok() {
            written(self.dim, self.pos);
            1
        } else {
            0
        }
    }
    /// `extractItem`: the held box, not while the tray moves.
    fn extract(
        &self,
        host: &mut dyn ServerHost,
        tx: &Transaction,
        _index: usize,
        resource: &ItemResource,
        amount: u32,
    ) -> u32 {
        if amount == 0 || with(self.dim, |pk| pk.packager(self.pos).animation_ticks != 0) {
            return 0;
        }
        let Some(held) = self.slot(&*host, HELD) else {
            return 0;
        };
        if !resource.matches(&held) {
            return 0;
        }
        let Some(e) = self.entity(&*host) else {
            return 0;
        };
        let n = held.count().min(amount);
        match e.set_item_slot(host, tx, HELD, held.shrink(n)) {
            Ok(_) => {
                written(self.dim, self.pos);
                n
            }
            Err(_) => 0,
        }
    }
}
