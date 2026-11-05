// Create's package logistics in the game: packagers, frogports, postboxes and chain conveyors.
// A sibling of the `create` module (`runtime.rs`); the rules are `create_engine`'s `package`,
// `packager`, `port` and `chain_conveyor`. Their state is typed, per dimension, on the server
// thread, saved with its column as `create_packages`; only packagers and frogports with
// something to do and conveyors carrying packages tick.
use super::Game;
use super::create::*;
use super::create_logistics::*;
use super::create_package_body as body;
use create_engine::Block;
use create_engine::chain_conveyor::{self as chain, Parcel};
use create_engine::kinetics::Dir;
use create_engine::package;
use create_engine::port::{Out, Packaging, Target};
use create_engine::world::dimension_index;
use horizon_client::items::ItemStack;
use horizon_core::geometry::Direction;
use horizon_world::nbt::{Compound, Tag};
use std::cell::RefCell;
use std::collections::HashMap;

type Pk = Packaging<ItemStack>;

/// Each dimension's package logistics, made when first needed.
type All = Option<Box<[Pk; 3]>>;
/// A block of ours placed or removed: dimension, position, was, is.
type Change = (usize, [i32; 3], Option<Block>, Option<Block>);
/// Part keys by conveyor.
type Drawn = HashMap<[i32; 3], Vec<u64>>;

thread_local! {
    static PACKAGES: RefCell<All> = const { RefCell::new(None) };
    /// Blocks of ours placed or removed (dimension, position, was, is), handled at the level's
    /// tick.
    static CHANGES: RefCell<Vec<Change>> =
        const { RefCell::new(Vec::new()) };
    /// Conveyors removed since the last send: clients clear what they drew there.
    static GONE: RefCell<Vec<(usize, [i32; 3])>> = const { RefCell::new(Vec::new()) };
    /// The client's drawn packages, by conveyor: their part keys.
    static DRAWN: RefCell<Drawn> = RefCell::new(HashMap::new());
}

fn with<R>(dim: usize, f: impl FnOnce(&mut Pk) -> R) -> R {
    PACKAGES.with_borrow_mut(|p| f(&mut p.get_or_insert_with(Default::default)[dim]))
}

/// The world closed: everything is forgotten.
pub(super) fn forget() {
    super::create_stock::forget();
    PACKAGES.with_borrow_mut(|p| *p = None);
    CHANGES.with_borrow_mut(Vec::clear);
    GONE.with_borrow_mut(Vec::clear);
    DRAWN.with_borrow_mut(HashMap::clear);
}

fn ours(b: Block) -> bool {
    matches!(
        b,
        Block::Packager | Block::Repackager | Block::PackageFrogport | Block::ChainConveyor
    ) || create_engine::dye::is_postbox(b)
        || super::create_stock::ours(b)
}

/// A packager or a re-packager.
fn is_packager(b: Block) -> bool {
    matches!(b, Block::Packager | Block::Repackager)
}

fn block_of(state: u32) -> Option<Block> {
    with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
}

/// A Create block changed (after the hot hook's range test): ours placed or gone are handled at
/// the level's tick.
pub(super) fn block_changed(dim: usize, pos: [i32; 3], old: u32, new: u32) {
    let (o, n) = (
        block_of(old).filter(|b| ours(*b)),
        block_of(new).filter(|b| ours(*b)),
    );
    if o == n {
        return;
    }
    CHANGES.with_borrow_mut(|c| c.push((dim, pos, o, n)));
}

/// `getAnalogOutputSignal` of a packager (`PackagerBlock`: 15 while it holds a box or animates)
/// or a frogport or postbox (`PackagePortBlockEntity.getComparatorOutput`: its 18 slots of
/// packages, as `calcRedstoneFromInventory` counts them); None for anything else.
pub(super) fn analog(
    dim: usize,
    pos: [i32; 3],
    items: &horizon_client::items::Items,
) -> Option<i32> {
    PACKAGES.with(|p| {
        let p = p.try_borrow().ok()?;
        let pk = &p.as_ref()?[dim];
        if let Some(lit) = super::create_stock::requester_succeeded(dim, pos) {
            return Some(if lit { 15 } else { 0 });
        }
        if let Some(k) = pk.packagers.get(&pos) {
            return Some(i32::from(k.comparator()));
        }
        let port = pk.ports.get(&pos)?;
        Some(create_engine::comparator::inventory(
            port.items.iter().map(|s| {
                s.as_ref().map(|p| {
                    let max = items.get(&p.stack.item).map_or(64, |i| i.max_stack);
                    (p.stack.count, max)
                })
            }),
            64,
        ))
    })
}

/// A neighbour of a Create block changed: a packager reads its power at the level's tick
/// (`PackagerBlock.neighborChanged`); a frogport looks at the inventory below.
pub(super) fn neighbor_changed(dim: usize, pos: [i32; 3], state: u32) {
    match block_of(state) {
        Some(Block::Packager | Block::Repackager) => with(dim, |pk| pk.redstone.push(pos)),
        Some(Block::PackageFrogport) => with(dim, |pk| pk.wake(pos)),
        Some(b) if super::create_stock::ours(b) => super::create_stock::neighbor_changed(dim, pos),
        _ => {}
    }
}

/// A container's contents changed: a frogport on top of it looks at it again.
pub(super) fn contents_changed(game: &mut Game<'_>, pos: [i32; 3]) {
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    if PACKAGES.with_borrow(|p| p.is_none()) {
        return;
    }
    with(dim, |pk| pk.wake([pos[0], pos[1] + 1, pos[2]]));
}

/// A part's speed changed after a solve: a chain conveyor's packages move at it, and the
/// frogports on it register again (a chain running the other way moves them).
pub(super) fn speed_changed(dim: usize, pos: [i32; 3], speed: f32) {
    if PACKAGES.with_borrow(|p| p.is_none()) {
        return;
    }
    with(dim, |pk| {
        if pk.chains.conveyors.contains_key(&pos) {
            pk.chains.set_speed(pos, speed);
            pk.conveyor_changed(pos);
        }
    });
}

fn add(p: [i32; 3], o: [i32; 3]) -> [i32; 3] {
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

fn center(p: [i32; 3]) -> [f64; 3] {
    p.map(|v| f64::from(v) + 0.5)
}

/// The kinetic network's chains of the conveyor at `pos`: the conveyors it joins turn with it.
fn sync_far(dim: usize, pos: [i32; 3]) {
    let to: Vec<[i32; 3]> = with(dim, |pk| {
        pk.chains
            .conveyors
            .get(&pos)
            .map(|c| c.connections.iter().map(|k| add(pos, *k)).collect())
            .unwrap_or_default()
    });
    with_world(|w| w.dims[dim].kinetics.set_far(pos, to));
}

fn speed_of(dim: usize, pos: [i32; 3]) -> f32 {
    with_world(|w| w.speed(dim, pos)).flatten().unwrap_or(0.0)
}

/// A package as the rules carry it: its stack and its address.
fn parcel(stack: ItemStack) -> Parcel<ItemStack> {
    Parcel {
        address: body::address(&stack),
        stack,
    }
}

/// `Block.popResource` / `PackageEntity.fromItemStack`: the stack drops as an item at `at` (a
/// package entity is not ported).
fn drop_stack(game: &mut Game<'_>, dimension: &str, at: [f64; 3], stack: &ItemStack) {
    game.native_run_command(&format!(
        "execute in {dimension} run summon minecraft:item {} {} {} {{Item:{}}}",
        at[0],
        at[1],
        at[2],
        body::snbt(stack)
    ));
}

/// The level's tick: blocks placed and removed, packagers' power, the packagers, chains and
/// frogports, then what clients draw. Nothing runs in a dimension with nothing to do.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    if PACKAGES.with_borrow(|p| p.is_none()) && CHANGES.with_borrow(Vec::is_empty) {
        return;
    }
    let changes: Vec<_> = CHANGES.with_borrow_mut(|c| {
        let mine = c.iter().filter(|x| x.0 == dim).cloned().collect();
        c.retain(|x| x.0 != dim);
        mine
    });
    for (_, pos, was, is) in changes {
        block_moved(game, dimension, dim, pos, was, is);
    }
    // What comparators read of packagers and ports that changed (since the last tick too: a
    // funnel or a menu took or gave a package) is looked at after this tick.
    for pos in with(dim, |pk| std::mem::take(&mut pk.changed)) {
        analog_touched(dim, pos);
    }
    let now = game.world.game_time.max(0) as u64;
    if with(dim, |pk| pk.is_idle()) && GONE.with_borrow(Vec::is_empty) {
        return;
    }
    // PackagerBlock.neighborChanged: `powered` follows the signal; a new one activates it.
    for pos in with(dim, |pk| std::mem::take(&mut pk.redstone)) {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let Some((Block::Packager | Block::Repackager, was)) =
            with_world(|w| w.table.get(state).map(|(b, i)| (b, i.powered))).flatten()
        else {
            continue;
        };
        let at = horizon_core::position::BlockPos::new(pos[0], pos[1], pos[2]);
        let now_powered =
            game.with_level(|b, level, ticks| b.has_neighbor_signal(level, ticks, at));
        if was == now_powered {
            continue;
        }
        game.native_set_block_property_with(
            pos,
            "powered",
            if now_powered { "true" } else { "false" },
            super::native_hooks::flags::UPDATE_CLIENTS,
        );
        if !was {
            activate(game, dim, pos);
        }
    }
    let mut out = Vec::new();
    with(dim, |pk| pk.tick(&mut out));
    for o in out {
        match o {
            Out::Recheck(pos) => {
                // `lazyTick`: `redstonePowered` read back from the block, then a try to send.
                let state = game.world.block(pos[0], pos[1], pos[2]).0;
                let powered = with_world(|w| w.table.get(state).map(|(_, i)| i.powered))
                    .flatten()
                    .unwrap_or(false);
                with(dim, |pk| pk.packager(pos).powered = powered);
                if redstone_mode(game, dim, pos) && with(dim, |pk| pk.packager(pos).can_send()) {
                    attempt_send(game, dim, pos);
                }
            }
            Out::PackagerDone(_) => {}
            Out::Drop(at, p) => drop_stack(game, dimension, at, &p.stack),
        }
    }
    for pos in with(dim, |pk| pk.due(now)) {
        frogport(game, dim, pos, now);
    }
    // Clients redraw the conveyors whose packages changed.
    let mut changed = with(dim, |pk| pk.chains.take_changed());
    changed.extend(GONE.with_borrow_mut(|g| {
        let mine: Vec<_> = g.iter().filter(|x| x.0 == dim).map(|x| x.1).collect();
        g.retain(|x| x.0 != dim);
        mine
    }));
    if !changed.is_empty() {
        let players = players_in(game, dimension);
        send_chains(game, &players, dim, &changed);
    }
    for pos in with(dim, |pk| std::mem::take(&mut pk.changed)) {
        analog_touched(dim, pos);
    }
    let mut unsaved = with(dim, |pk| std::mem::take(&mut pk.unsaved));
    unsaved.extend(changed.iter().map(|p| (p[0] >> 4, p[2] >> 4)));
    unsaved.sort_unstable();
    unsaved.dedup();
    for (cx, cz) in unsaved {
        game.native_mark_chunk_unsaved(dimension, [cx, cz]);
    }
}

/// One of our blocks placed or removed.
fn block_moved(
    game: &mut Game<'_>,
    dimension: &str,
    dim: usize,
    pos: [i32; 3],
    was: Option<Block>,
    is: Option<Block>,
) {
    let mut drops: Vec<([f64; 3], ItemStack)> = Vec::new();
    if was.is_some_and(super::create_stock::ours) || is.is_some_and(super::create_stock::ours) {
        super::create_stock::block_moved(game, dim, pos, was, is);
    }
    match was {
        // `PackagerBlockEntity.destroy`: its box and queued boxes drop.
        Some(Block::Packager | Block::Repackager) => with(dim, |pk| {
            if let Some(p) = pk.packagers.remove(&pos) {
                for b in p.held.into_iter().chain(p.sliding) {
                    drops.push((center(pos), b.stack));
                }
                for (b, n) in p.queue {
                    for _ in 0..n.max(1) {
                        drops.push((center(pos), b.stack.clone()));
                    }
                }
            }
            pk.active.retain(|x| *x != pos);
        }),
        Some(b) if super::create_stock::ours(b) => {}
        // `ChainConveyorBlockEntity.destroy`: its packages drop, the conveyors it joined let go.
        Some(Block::ChainConveyor) => {
            let joined: Vec<[i32; 3]> = with(dim, |pk| {
                let joined = pk
                    .chains
                    .conveyors
                    .get(&pos)
                    .map(|c| c.connections.iter().map(|k| add(pos, *k)).collect())
                    .unwrap_or_default();
                let mut ev = Vec::new();
                pk.chains.remove(pos, &mut ev);
                for e in ev {
                    if let chain::Event::Drop(at, p) = e {
                        drops.push((at, p.stack));
                    }
                }
                joined
            });
            with_world(|w| w.dims[dim].kinetics.set_far(pos, Vec::new()));
            for j in joined {
                sync_far(dim, j);
                with(dim, |pk| pk.conveyor_changed(j));
            }
            GONE.with_borrow_mut(|g| g.push((dim, pos)));
        }
        // `PackagePortBlockEntity.destroy`: its packages drop.
        Some(_) => with(dim, |pk| {
            for p in pk.remove_port(pos) {
                drops.push((center(pos), p.stack));
            }
        }),
        None => {}
    }
    match is {
        Some(b @ (Block::Packager | Block::Repackager)) => {
            with(dim, |pk| {
                pk.packager(pos).repackager = b == Block::Repackager;
                pk.touch(pos);
            });
            // `initialize`: `linked` follows a stock link on it.
            super::create_stock::recheck_linked(game, pos);
        }
        Some(b) if super::create_stock::ours(b) => {}
        Some(Block::ChainConveyor) => {
            let speed = speed_of(dim, pos);
            with(dim, |pk| {
                pk.chains.add(pos, Vec::new());
                pk.chains.set_speed(pos, speed);
                pk.touch(pos);
            });
        }
        Some(Block::PackageFrogport) => with(dim, |pk| {
            pk.port(pos, true);
            pk.touch(pos);
            pk.wake(pos);
        }),
        Some(_) => with(dim, |pk| {
            pk.port(pos, false);
            pk.touch(pos);
        }),
        None => {}
    }
    for (at, s) in drops {
        drop_stack(game, dimension, at, &s);
    }
}

/// An inventory as a packager or frogport reaches it: a packager (`PackagerItemHandler`), a
/// port (`PackagePortAutomationInventoryWrapper`), an item vault, or a vanilla or block-entity
/// container through one face.
enum Inv {
    Packager([i32; 3]),
    Port([i32; 3]),
    Vault(Vec<[i32; 3]>),
    Native([i32; 3], Direction),
}

fn inv_at(game: &Game<'_>, dim: usize, pos: [i32; 3], face: Direction) -> Option<Inv> {
    let (packager, port) = with(dim, |pk| {
        (pk.packagers.contains_key(&pos), pk.ports.contains_key(&pos))
    });
    if packager {
        return Some(Inv::Packager(pos));
    }
    if port {
        return Some(Inv::Port(pos));
    }
    if let Some(parts) = vault_at(game, dim, pos) {
        return Some(Inv::Vault(parts));
    }
    super::machines::create_extract::takeable_slots(game, pos, face).map(|_| Inv::Native(pos, face))
}

/// The stacks automation could take from `inv`, by slot (None where nothing may be taken).
fn inv_view(game: &Game<'_>, dim: usize, inv: &Inv) -> Vec<Option<ItemStack>> {
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
        Inv::Vault(parts) => vault_slots(game, parts),
        Inv::Native(p, face) => super::machines::create_extract::takeable_slots(game, *p, *face)
            .unwrap_or_default()
            .into_iter()
            .map(|(_, s)| s)
            .collect(),
    }
}

/// Up to `n` from slot `slot` of `inv` (as `inv_view` numbers them).
fn inv_take(
    game: &mut Game<'_>,
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
        Inv::Vault(parts) => {
            let mut slots = vault_slots(game, parts);
            let s = slots.get_mut(slot)?.as_mut()?;
            let n = n.min(s.count);
            let out = ItemStack {
                count: n,
                ..s.clone()
            };
            s.count -= n;
            if s.count == 0 {
                slots[slot] = None;
            }
            let per = create_engine::vault::SLOTS;
            let block = parts[slot / per];
            let mine = slots[slot / per * per..(slot / per + 1) * per].to_vec();
            game.native_with_mod_block_entity(block, |be| {
                be.items.resize(be.items.len().max(per), None);
                be.items[..per].clone_from_slice(&mine);
            });
            Some(parcel(out))
        }
        Inv::Native(p, face) => {
            let slots = super::machines::create_extract::takeable_slots(game, *p, *face)?;
            let (index, _) = *slots.get(slot)?;
            super::machines::create_extract::take_from_slot(game, *p, index, n).map(parcel)
        }
    }
}

/// `PackagerItemHandler.extractItem`: the held box; the next queued one may come out after.
fn take_box(pk: &mut Pk, pos: [i32; 3]) -> Option<Parcel<ItemStack>> {
    let k = pk.packager(pos);
    let b = k.take();
    let queued = !k.queue.is_empty();
    pk.touch(pos);
    if b.is_some() && queued {
        pk.activate(pos);
    }
    b
}

/// `insertItemStacked` / `insertItem` into `inv`: what is left (None when all went in).
fn inv_insert(
    game: &mut Game<'_>,
    dim: usize,
    inv: &Inv,
    p: Parcel<ItemStack>,
) -> Option<Parcel<ItemStack>> {
    match inv {
        Inv::Packager(at) => {
            // PackagerItemHandler.insertItem: a package, nothing held, and its contents fit.
            let free = with(dim, |pk| pk.packager(*at).can_unwrap());
            if !free
                || !package::is_package(&p.stack.item)
                || !unwrap(game, dim, *at, &p.stack, true)
            {
                return Some(p);
            }
            unwrap(game, dim, *at, &p.stack, false);
            None
        }
        Inv::Port(at) => with(dim, |pk| {
            let Some(port) = pk.ports.get_mut(at) else {
                return Some(p);
            };
            if !package::is_package(&p.stack.item) || !port.automation_takes(&p.address) {
                return Some(p);
            }
            let left = port.insert(p);
            pk.touch(*at);
            if left.is_none() {
                pk.wake(*at);
            }
            left
        }),
        Inv::Vault(parts) => {
            let address = p.address.clone();
            insert_vault(game, parts, p.stack).map(|stack| Parcel { stack, address })
        }
        Inv::Native(at, face) => {
            let address = p.address.clone();
            insert_item(game, dim, *at, *face, p.stack).map(|stack| Parcel { stack, address })
        }
    }
}

fn direction(d: Dir) -> Direction {
    Direction::ALL[d as usize]
}

/// The packager's facing and the inventory behind it (`oppositeOfBlockFacing`), reached through
/// the face toward the packager.
fn behind(game: &Game<'_>, pos: [i32; 3]) -> Option<([i32; 3], Direction)> {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    let facing = with_world(|w| w.table.get(state).and_then(|(_, i)| i.facing)).flatten()?;
    let o = facing.offset();
    Some((
        [pos[0] - o[0], pos[1] - o[1], pos[2] - o[2]],
        direction(facing),
    ))
}

/// `activate`: a pulse packs the inventory behind into a package (`attemptToSend`), and the
/// button cooldown starts.
fn activate(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    with(dim, |pk| pk.packager(pos).pulse());
    // `recheckIfLinksPresent`, then a linked packager waits for its network's orders.
    super::create_stock::recheck_linked(game, pos);
    if !redstone_mode(game, dim, pos) {
        with(dim, |pk| pk.activate(pos));
        return;
    }
    if with(dim, |pk| pk.packager(pos).can_send()) {
        attempt_send(game, dim, pos);
    }
    with(dim, |pk| {
        pk.packager(pos).after_pulse();
        pk.activate(pos);
    });
}

/// A stack compared as Create compares stacks.
struct Key<'a>(&'a ItemStack);

impl PartialEq for Key<'_> {
    fn eq(&self, o: &Self) -> bool {
        same_stack(self.0, o.0)
    }
}

/// `!canFitInsideContainerItems`: packages and shulker boxes.
fn bulky(item: &str) -> bool {
    package::is_package(item) || item.ends_with("shulker_box")
}

/// `PackagerBlockEntity.attemptToSend(null)`: the inventory behind packed into one box (a
/// package there goes as it is), addressed with the packager's address, sliding out.
fn attempt_send(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    if with(dim, |pk| pk.packager(pos).repackager) {
        attempt_repackage(game, dim, pos);
        return;
    }
    let Some((at, face)) = behind(game, pos) else {
        return;
    };
    let Some(inv) = inv_at(game, dim, at, face) else {
        return;
    };
    // A packager behind a packager is no inventory to it (`instanceof PackagerItemHandler`).
    if matches!(inv, Inv::Packager(_)) {
        return;
    }
    let view = inv_view(game, dim, &inv);
    let packed = {
        let slots: Vec<Option<package::Slot<Key<'_>>>> = view
            .iter()
            .map(|s| {
                s.as_ref().map(|s| package::Slot {
                    key: Key(s),
                    count: s.count,
                    max: u32::from(stack_limit(game, &s.item)),
                    bulky: bulky(&s.item),
                    package: package::is_package(&s.item),
                })
            })
            .collect();
        package::pack(&slots)
    };
    let Some(packed) = packed else { return };
    let mut taken: HashMap<usize, Parcel<ItemStack>> = HashMap::new();
    for &(slot, n) in &packed.taken {
        if let Some(p) = inv_take(game, dim, &inv, slot, n) {
            taken.insert(slot, p);
        }
    }
    let mut stack = match packed.package.and_then(|s| taken.remove(&s)) {
        Some(p) => p.stack,
        None => {
            let contents: Vec<Option<ItemStack>> = packed
                .contents
                .iter()
                .map(|c| {
                    let (from, n) = (*c)?;
                    let s = view.get(from)?.as_ref()?;
                    Some(ItemStack {
                        count: n,
                        ..s.clone()
                    })
                })
                .collect();
            new_box(game, pos, &contents)
        }
    };
    let address = with(dim, |pk| pk.packager(pos).address.clone());
    let address = (!package::is_blank(&address)).then_some(address);
    body::set_address(&mut stack, address.as_deref());
    with(dim, |pk| {
        pk.packager(pos).send(Parcel {
            stack,
            address: address.unwrap_or_default(),
        });
        pk.activate(pos);
    });
    // The inventory behind changed: frogports and chutes there look again.
    contents_changed(game, at);
}

/// `unwrapBox`: whether the package's contents fit the inventory behind (`simulate`), else
/// they go in (`DefaultUnpackingHandler.unpack`) and the tray slides in. An empty package
/// always unwraps, without the tray.
fn unwrap(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: &ItemStack,
    simulate: bool,
) -> bool {
    if !with(dim, |pk| pk.packager(pos).can_unwrap()) {
        return false;
    }
    if with(dim, |pk| pk.packager(pos).repackager) {
        return unwrap_whole(game, dim, pos, stack, simulate);
    }
    let items: Vec<ItemStack> = body::contents(stack).into_iter().flatten().collect();
    if items.is_empty() {
        return true;
    }
    let Some((at, face)) = behind(game, pos) else {
        return false;
    };
    let Some(inv) = inv_at(game, dim, at, face) else {
        return false;
    };
    if matches!(inv, Inv::Packager(_)) {
        return false;
    }
    if simulate {
        let slots: Vec<Option<ItemStack>> = match &inv {
            Inv::Vault(parts) => vault_slots(game, parts),
            Inv::Native(p, _) => {
                super::machines::create_extract::all_slots(game, *p).unwrap_or_default()
            }
            // A port takes packages only.
            _ => return items.iter().all(|i| package::is_package(&i.item)),
        };
        // Keys: an index per kind of stack.
        let mut kinds: Vec<ItemStack> = Vec::new();
        let mut key = |s: &ItemStack| -> usize {
            match kinds.iter().position(|k| same_stack(k, s)) {
                Some(i) => i,
                None => {
                    kinds.push(s.clone());
                    kinds.len() - 1
                }
            }
        };
        let targets: Vec<package::Target<usize>> = slots
            .iter()
            .map(|s| package::Target {
                stack: s.as_ref().map(|s| (key(s), s.count)),
                limit: 64,
            })
            .collect();
        let wanted: Vec<(usize, u32, u32)> = items
            .iter()
            .map(|i| (key(i), i.count, u32::from(stack_limit(game, &i.item))))
            .collect();
        return package::unpack_fits(&targets, &wanted);
    }
    for i in items {
        let p = parcel(i);
        if let Some(left) = inv_insert(game, dim, &inv, p) {
            // `unpack` trusts its simulation: what still did not fit drops.
            let dimension = game.dim.id().to_owned();
            drop_stack(game, &dimension, center(pos), &left.stack);
        }
    }
    with(dim, |pk| {
        pk.packager(pos).unwrapped();
        pk.activate(pos);
    });
    contents_changed(game, at);
    true
}

/// `RepackagerBlockEntity.unwrapBox`: the package goes into the inventory behind as it is, when
/// a slot takes it.
fn unwrap_whole(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: &ItemStack,
    simulate: bool,
) -> bool {
    let Some((at, face)) = behind(game, pos) else {
        return false;
    };
    let Some(inv) = inv_at(game, dim, at, face) else {
        return false;
    };
    let room = match &inv {
        Inv::Packager(_) => return false,
        Inv::Vault(parts) => vault_slots(game, parts).iter().any(Option::is_none),
        Inv::Native(p, _) => super::machines::create_extract::all_slots(game, *p)
            .unwrap_or_default()
            .iter()
            .any(Option::is_none),
        Inv::Port(p) => with(dim, |pk| {
            pk.ports.get(p).is_some_and(|port| {
                port.automation_takes(&body::address(stack)) && !port.backed_up()
            })
        }),
    };
    if !room || simulate {
        return room;
    }
    if let Some(left) = inv_insert(game, dim, &inv, parcel(stack.clone())) {
        let dimension = game.dim.id().to_owned();
        drop_stack(game, &dimension, center(pos), &left.stack);
    }
    with(dim, |pk| {
        pk.packager(pos).unwrapped();
        pk.activate(pos);
    });
    contents_changed(game, at);
    true
}

/// `RepackagerBlockEntity.attemptToSend`: a package without order data in the inventory behind
/// goes out as it is (with the sign's address); else, once every fragment of an order is there,
/// they are taken and the order goes out repacked (`PackageRepackageHelper`), queued.
fn attempt_repackage(game: &mut Game<'_>, dim: usize, pos: [i32; 3]) {
    if !with(dim, |pk| {
        let k = pk.packager(pos);
        k.can_send() && k.queue.is_empty()
    }) {
        return;
    }
    let Some((at, face)) = behind(game, pos) else {
        return;
    };
    let Some(inv) = inv_at(game, dim, at, face) else {
        return;
    };
    if matches!(inv, Inv::Packager(_)) {
        return;
    }
    let view = inv_view(game, dim, &inv);
    let mut fragments: Vec<(usize, create_engine::stock::OrderData<ItemStack>)> = Vec::new();
    let mut completed = None;
    for (slot, s) in view.iter().enumerate() {
        let Some(s) = s else { continue };
        if !package::is_package(&s.item) {
            continue;
        }
        let Some(order) = body::order(s) else {
            // Not fragmented: it goes out as it is.
            let Some(mut p) = inv_take(game, dim, &inv, slot, 1) else {
                return;
            };
            let address = with(dim, |pk| pk.packager(pos).address.clone());
            if !package::is_blank(&address) {
                body::set_address(&mut p.stack, Some(&address));
                p.address = address;
            }
            with(dim, |pk| {
                pk.packager(pos).send(p);
                pk.activate(pos);
            });
            contents_changed(game, at);
            return;
        };
        let id = order.order_id;
        fragments.push((slot, order));
        let mine: Vec<&create_engine::stock::OrderData<ItemStack>> = fragments
            .iter()
            .filter(|f| f.1.order_id == id)
            .map(|f| &f.1)
            .collect();
        if create_engine::stock::is_order_complete(&mine) {
            completed = Some(id);
            break;
        }
    }
    let Some(id) = completed else { return };
    let mut frags = Vec::new();
    for (slot, order) in fragments.into_iter().filter(|f| f.1.order_id == id) {
        let Some(s) = view[slot].as_ref() else {
            continue;
        };
        frags.push(create_engine::stock::Fragment {
            order: super::create_stock::order_data_key(&order),
            address: body::address(s),
            contents: body::contents(s)
                .into_iter()
                .flatten()
                .map(|i| (super::create_stock::Key::of(&i), i.count))
                .collect(),
        });
    }
    let boxes = create_engine::stock::repack(
        &frags,
        id,
        |k| u32::from(stack_limit(game, &k.0.item)),
        || super::create_stock::next_int(game),
    );
    // Every fragment of the order leaves the inventory.
    for (slot, s) in view.iter().enumerate() {
        if s.as_ref().is_some_and(|s| {
            package::is_package(&s.item) && body::order(s).is_some_and(|o| o.order_id == id)
        }) {
            inv_take(game, dim, &inv, slot, 1);
        }
    }
    for b in boxes {
        let contents: Vec<Option<ItemStack>> = b
            .contents
            .iter()
            .map(|(k, n)| {
                Some(ItemStack {
                    count: *n,
                    ..k.0.clone()
                })
            })
            .collect();
        let mut stack = new_box(game, pos, &contents);
        body::set_address(&mut stack, Some(&b.address));
        body::set_order(&mut stack, &super::create_stock::order_data_stack(&b.order));
        with(dim, |pk| {
            pk.packager(pos).queue.push_back((
                Parcel {
                    stack,
                    address: b.address.clone(),
                },
                b.count,
            ))
        });
    }
    with(dim, |pk| pk.activate(pos));
    contents_changed(game, at);
}

/// `PackageItem.containing`: a box of a random style holding `contents`.
fn new_box(game: &Game<'_>, pos: [i32; 3], contents: &[Option<ItemStack>]) -> ItemStack {
    let mut seed = (game.world.game_time as u64)
        ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (pos[2] as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (pos[1] as u64) << 40;
    let next = |n: u32| {
        seed ^= seed >> 31;
        seed = seed.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        seed ^= seed >> 29;
        (seed % u64::from(n.max(1))) as u32
    };
    let mut b = ItemStack::new(package::random_box(next), 1);
    body::set_contents(&mut b, contents);
    b
}

/// The packager's inventory as `getAvailableItems` sums it, and which inventory it is
/// (`InventoryIdentifier`: its first block), or None when nothing is behind it. A packager behind
/// it is an inventory of nothing.
pub(super) fn available(
    game: &Game<'_>,
    dim: usize,
    pos: [i32; 3],
) -> (
    create_engine::stock::Summary<super::create_stock::Key>,
    Option<[i32; 3]>,
) {
    let mut s = create_engine::stock::Summary::default();
    let Some((at, face)) = behind(game, pos) else {
        return (s, None);
    };
    let Some(inv) = inv_at(game, dim, at, face) else {
        return (s, None);
    };
    let id = match &inv {
        Inv::Vault(parts) => parts.iter().min().copied().unwrap_or(at),
        _ => at,
    };
    if matches!(inv, Inv::Packager(_)) {
        return (s, Some(id));
    }
    for stack in inv_view(game, dim, &inv).iter().flatten() {
        s.add(&super::create_stock::Key::of(stack), stack.count as i32);
    }
    (s, Some(id))
}

/// `isTooBusyFor`: a packager with that many boxes queued takes no order.
pub(super) fn too_busy(dim: usize, pos: [i32; 3], kind: create_engine::stock::RequestType) -> bool {
    with(dim, |pk| {
        pk.packagers.get(&pos).is_some_and(|k| k.too_busy(kind))
    })
}

/// `performPackageRequests` for one packager: up to 100 boxes of its requests
/// (`attemptToSend(requests)`), each sliding out or queued. The stacks put in boxes (not
/// packages sent as they are), for `deductFromAccurateSummary`.
pub(super) fn perform_requests(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    queue: &mut std::collections::VecDeque<create_engine::stock::Request<super::create_stock::Key>>,
) -> Vec<ItemStack> {
    let mut boxed = Vec::new();
    for _ in 0..create_engine::stock::MAX_PACKAGES {
        if queue.is_empty() {
            break;
        }
        let Some((at, face)) = behind(game, pos) else {
            break;
        };
        let Some(inv) = inv_at(game, dim, at, face) else {
            break;
        };
        if matches!(inv, Inv::Packager(_)) {
            break;
        }
        let view = inv_view(game, dim, &inv);
        let mut slots: Vec<Option<package::Slot<super::create_stock::Key>>> = view
            .iter()
            .map(|s| {
                s.as_ref().map(|s| package::Slot {
                    key: super::create_stock::Key::of(s),
                    count: s.count,
                    max: u32::from(stack_limit(game, &s.item)),
                    bulky: bulky(&s.item),
                    package: package::is_package(&s.item),
                })
            })
            .collect();
        let Some(r) = create_engine::stock::pack_requests(&mut slots, queue) else {
            continue;
        };
        let mut taken: HashMap<usize, Parcel<ItemStack>> = HashMap::new();
        for &(slot, n) in &r.packed.taken {
            if let Some(p) = inv_take(game, dim, &inv, slot, n) {
                taken.insert(slot, p);
            }
        }
        let mut stack = match r.packed.package.and_then(|s| taken.remove(&s)) {
            Some(p) => p.stack,
            None => {
                let contents: Vec<Option<ItemStack>> = r
                    .packed
                    .contents
                    .iter()
                    .map(|c| {
                        let (from, n) = (*c)?;
                        let s = view.get(from)?.as_ref()?;
                        Some(ItemStack {
                            count: n,
                            ..s.clone()
                        })
                    })
                    .collect();
                boxed.extend(contents.iter().flatten().cloned());
                new_box(game, pos, &contents)
            }
        };
        body::set_address(&mut stack, Some(&r.address));
        body::set_order(&mut stack, &super::create_stock::order_data_stack(&r.order));
        with(dim, |pk| {
            pk.packager(pos).send_or_queue(Parcel {
                stack,
                address: r.address.clone(),
            });
            pk.activate(pos);
        });
        contents_changed(game, at);
    }
    boxed
}

/// `redstoneModeActive`: a packager with a stock link on it packs for its network only, not on
/// a signal; a re-packager always on a signal.
fn redstone_mode(game: &Game<'_>, dim: usize, pos: [i32; 3]) -> bool {
    with(dim, |pk| {
        pk.packagers.get(&pos).is_some_and(|k| k.repackager)
    }) || super::create_stock::link_of(game, pos).is_none()
}

/// A frogport looks at its inventories (`lazyTick`): what it caught for its address goes down
/// into the inventory below (`tryPushingToAdjacentInventories`), then a package not for it,
/// its own or from below, is sent onto its chain (`tryPullingFromOwnAndAdjacentInventories`).
/// It looks again in 10 ticks only while something waits.
fn frogport(game: &mut Game<'_>, dim: usize, pos: [i32; 3], now: u64) {
    let Some(animating) = with(dim, |pk| pk.ports.get(&pos).map(|p| p.is_animating())) else {
        return;
    };
    if animating {
        return;
    }
    let below = [pos[0], pos[1] - 1, pos[2]];
    let below_is_frog = with(dim, |pk| pk.ports.get(&below).is_some_and(|p| p.frog));
    let inv = if below_is_frog {
        None
    } else {
        inv_at(game, dim, below, Direction::Up)
    };
    let mut failed = false;
    if let Some(inv) = &inv {
        for slot in 0..create_engine::port::SLOTS {
            let p = with(dim, |pk| {
                let port = pk.ports.get_mut(&pos)?;
                let gives = port.items[slot]
                    .as_ref()
                    .is_some_and(|b| port.automation_gives(&b.address));
                if gives { port.items[slot].take() } else { None }
            });
            let Some(p) = p else { continue };
            if let Some(back) = inv_insert(game, dim, inv, p) {
                with(dim, |pk| {
                    if let Some(port) = pk.ports.get_mut(&pos) {
                        port.items[slot] = Some(back);
                    }
                });
                failed = true;
            } else {
                with(dim, |pk| pk.touch(pos));
                contents_changed(game, below);
            }
        }
    }
    with(dim, |pk| {
        if let Some(port) = pk.ports.get_mut(&pos) {
            port.failed_export = failed;
        }
    });
    let mut waiting = failed;
    if with(dim, |pk| pk.can_send(pos)) {
        if !with(dim, |pk| pk.send_own(pos))
            && let Some(inv) = &inv
        {
            let filter = with(dim, |pk| port_filter(pk, pos));
            let view = inv_view(game, dim, inv);
            let packager = matches!(inv, Inv::Packager(_));
            let slot = view.iter().position(|s| {
                s.as_ref().is_some_and(|s| {
                    package::is_package(&s.item)
                        && (packager
                            || filter
                                .as_deref()
                                .is_none_or(|f| !package::match_address(&body::address(s), f)))
                })
            });
            if let Some(slot) = slot
                && let Some(p) = inv_take(game, dim, inv, slot, 1)
            {
                with(dim, |pk| pk.start(pos, p, true));
                contents_changed(game, below);
            }
        }
    } else {
        // Held back by its target: it waits only if it has something to send.
        let own = with(dim, |pk| {
            let port = pk.ports.get(&pos);
            port.is_some_and(|p| p.target.is_some() && p.has_outgoing())
        });
        let has_target = with(dim, |pk| {
            pk.ports.get(&pos).is_some_and(|p| p.target.is_some())
        });
        let from_below = has_target
            && inv.as_ref().is_some_and(|inv| {
                inv_view(game, dim, inv)
                    .iter()
                    .flatten()
                    .any(|s| package::is_package(&s.item))
            });
        waiting |= own || from_below;
    }
    if waiting {
        with(dim, |pk| pk.retry(pos, now));
    }
}

/// The frogport's filter (`getFilterString`).
fn port_filter(pk: &Pk, pos: [i32; 3]) -> Option<String> {
    let port = pk.ports.get(&pos)?;
    port.filter_string().map(str::to_owned)
}

/// Automation (funnels, chutes, belts' ends) putting into a packager or port: Some(what is
/// left) when `pos` is one, None when it is something else.
pub(super) fn automation_insert(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: ItemStack,
) -> Result<Option<ItemStack>, ItemStack> {
    if PACKAGES.with_borrow(|p| p.is_none()) {
        return Err(stack);
    }
    let inv = match with(dim, |pk| {
        (pk.packagers.contains_key(&pos), pk.ports.contains_key(&pos))
    }) {
        (true, _) => Inv::Packager(pos),
        (_, true) => Inv::Port(pos),
        _ => return Err(stack),
    };
    let count = stack.count;
    let one = ItemStack {
        count: 1,
        ..stack.clone()
    };
    match inv_insert(game, dim, &inv, parcel(one)) {
        Some(_) => Ok(Some(stack)),
        None if count > 1 => Ok(Some(ItemStack {
            count: count - 1,
            ..stack
        })),
        None => Ok(None),
    }
}

/// How much of `stack` a packager or port would take: Some for one of them.
pub(super) fn automation_room(
    game: &Game<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: &ItemStack,
) -> Option<u32> {
    if PACKAGES.with_borrow(|p| p.is_none()) {
        return None;
    }
    let _ = game;
    with(dim, |pk| {
        if let Some(p) = pk.packagers.get(&pos) {
            // The contents' fit is known only when it goes in.
            return Some(u32::from(
                p.can_unwrap() && package::is_package(&stack.item),
            ));
        }
        let port = pk.ports.get(&pos)?;
        let takes = package::is_package(&stack.item)
            && port.automation_takes(&body::address(stack))
            && !port.backed_up();
        Some(u32::from(takes))
    })
}

/// Automation taking from a packager or port: Some(the stack or nothing) for one of them.
pub(super) fn automation_take(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
) -> Option<Option<ItemStack>> {
    if PACKAGES.with_borrow(|p| p.is_none()) {
        return None;
    }
    let inv = match with(dim, |pk| {
        (pk.packagers.contains_key(&pos), pk.ports.contains_key(&pos))
    }) {
        (true, _) => Inv::Packager(pos),
        (_, true) => Inv::Port(pos),
        _ => return None,
    };
    let view = inv_view(game, dim, &inv);
    let Some(slot) = view.iter().position(Option::is_some) else {
        return Some(None);
    };
    let p = inv_take(game, dim, &inv, slot, 64);
    if p.is_some() && matches!(inv, Inv::Packager(_)) {
        // The box is gone: a frogport above may send the next.
        with(dim, |pk| pk.wake([pos[0], pos[1] + 1, pos[2]]));
    }
    Some(p.map(|p| p.stack))
}

/// A postbox's packages, for its menu (shown, not yet taken from: the menu's clicks are not
/// bound to the port).
pub(super) fn port_stacks(dim: usize, pos: [i32; 3]) -> Vec<Option<ItemStack>> {
    let mut v = with(dim, |pk| {
        let port = pk.ports.get(&pos);
        port.map(create_engine::port::Port::stacks)
            .unwrap_or_default()
    });
    v.resize(create_engine::port::SLOTS, None);
    v
}

/// A right click on a packager (`PackagerBlock.useItemOn`): a wrench or a frogport in hand
/// passes; with nothing held, a package in hand is unpacked into the inventory behind; a held
/// box goes to the player. True when the block is a packager.
pub(super) fn use_block(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    block: Block,
    canceled: &mut bool,
) -> bool {
    if super::create_stock::use_block(block, canceled) {
        return true;
    }
    if !is_packager(block) {
        return false;
    }
    let hand = game.native_held().map(|h| h.0);
    let linked = super::create_stock::link_of(game, pos).is_some();
    if hand.as_deref().is_some_and(|h| {
        matches!(
            h,
            "create:wrench" | "create:package_frogport" | "create:factory_gauge"
        ) || (h == "create:stock_link" && !linked)
    }) {
        return true;
    }
    *canceled = true;
    if game.hand != horizon_client::first_person::Hand::Main {
        return true;
    }
    let (animating, held) = with(dim, |pk| {
        let p = pk.packager(pos);
        (p.animation_ticks > 0, p.held.is_some())
    });
    if animating {
        return true;
    }
    if !held {
        let Some(stack) = game.inventory.get(game.selected).cloned().flatten() else {
            return true;
        };
        if !package::is_package(&stack.item) || !unwrap(game, dim, pos, &stack, true) {
            return true;
        }
        unwrap(game, dim, pos, &stack, false);
        super::create_stock::stock_check(game, dim, pos);
        if let Some(slot) = game.inventory.get_mut(game.selected) {
            match slot {
                Some(s) if s.count > 1 => s.count -= 1,
                _ => *slot = None,
            }
        }
        let c = center(pos);
        game.audio.play("create:depot_plop", Some(c), 1.0, 1.0);
        return true;
    }
    let Some(b) = with(dim, |pk| take_box(pk, pos)) else {
        return true;
    };
    // `placeItemBackInInventory`: into a free slot, else dropped.
    match game.inventory.iter_mut().take(36).find(|s| s.is_none()) {
        Some(slot) => *slot = Some(b.stack),
        None => {
            let dimension = game.dim.id().to_owned();
            drop_stack(game, &dimension, center(pos), &b.stack);
        }
    }
    with(dim, |pk| pk.wake([pos[0], pos[1] + 1, pos[2]]));
    true
}

/// Our debug commands, standing in for the clipboard and signs (`address`), the frogport
/// item's selection (`frogport`) and the chain item (`chain`, `unchain`).
pub(super) fn register_commands(commands: &mut Vec<super::native_server::NativeCommand>) {
    for usage in [
        "create_debug address <pos:block_pos> <address:text>",
        "create_debug frogport <pos:block_pos> <target:text>",
        "create_debug chain <from:block_pos> <to:block_pos>",
        "create_debug unchain <from:block_pos> <to:block_pos>",
        "create_debug packages <pos:block_pos>",
    ] {
        commands.push(super::native_server::NativeCommand::op(usage, 2));
    }
}

/// Coordinates from the command's words (`~` from the player's position).
pub(super) fn coords(words: &[&str], here: Option<[f64; 3]>) -> Option<[i32; 3]> {
    let mut out = [0; 3];
    for i in 0..3 {
        let t = words.get(i)?;
        out[i] = match t.strip_prefix('~') {
            Some(off) => here?[i].floor() as i32 + off.parse::<i32>().unwrap_or(0),
            None => t.parse().ok()?,
        };
    }
    Some(out)
}

pub(super) fn perform_command(game: &mut Game<'_>, command: &str, ran: &mut bool) {
    let Some(rest) = command.strip_prefix("create_debug ") else {
        return;
    };
    let (verb, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    if !matches!(
        verb,
        "address" | "frogport" | "chain" | "unchain" | "packages"
    ) {
        return;
    }
    *ran = true;
    let player = game.native_acting_player();
    let here = player.as_ref().map(|p| p.position);
    let dim = player
        .as_ref()
        .and_then(|p| dimension_index(&p.dimension))
        .unwrap_or(0);
    let words: Vec<&str> = rest.split_whitespace().collect();
    let line = match (verb, coords(&words, here)) {
        ("address", Some(pos)) => {
            // The rest of the line after the three coordinates.
            let mut text = rest.trim_start();
            for _ in 0..3 {
                text = text
                    .split_once(char::is_whitespace)
                    .map_or("", |x| x.1)
                    .trim_start();
            }
            set_address(dim, pos, text.trim_end())
        }
        ("frogport", Some(pos)) => set_frogport(dim, pos, &words[3..], here),
        ("chain" | "unchain", Some(a)) => match coords(&words[3..], here) {
            Some(b) => chain_command(game, dim, a, b, verb == "chain"),
            None => format!("usage: create_debug {verb} <x y z> <x y z>"),
        },
        ("packages", Some(pos)) => describe(dim, pos),
        _ => format!("usage: create_debug {verb} <x y z> ..."),
    };
    game.native_feedback(horizon_client::text::Text::literal(line));
}

/// A packager's address (its sign's) or a port's filter (`addressFilter`, `filterChanged`).
fn set_address(dim: usize, pos: [i32; 3], text: &str) -> String {
    with(dim, |pk| {
        if let Some(p) = pk.packagers.get_mut(&pos) {
            p.address = text.to_owned();
            pk.touch(pos);
            return format!("packager address \"{text}\"");
        }
        if let Some(p) = pk.ports.get_mut(&pos) {
            p.filter = text.to_owned();
            pk.register(pos);
            pk.wake(pos);
            pk.touch(pos);
            return format!("port filter \"{text}\"");
        }
        "no packager or port there".to_owned()
    })
}

/// A frogport's target: a chain conveyor, round its wheel at an angle or along one of its
/// chains (`ChainConveyorFrogportTarget`, `setup`: flipped when the chain runs backwards).
fn set_frogport(dim: usize, pos: [i32; 3], words: &[&str], here: Option<[f64; 3]>) -> String {
    let usage = "usage: create_debug frogport <x y z> <conveyor x y z> <angle | distance dx dy dz>";
    let Some(conveyor) = coords(words, here) else {
        return usage.into();
    };
    let Some(chain_pos) = words.get(3).and_then(|w| w.parse::<f32>().ok()) else {
        return usage.into();
    };
    let connection = if words.len() >= 7 {
        coords(&words[4..], None)
    } else {
        None
    };
    with(dim, |pk| {
        if !pk.ports.get(&pos).is_some_and(|p| p.frog) {
            return "no frogport there".to_owned();
        }
        let Some(c) = pk.chains.conveyors.get(&conveyor) else {
            return "no chain conveyor there".to_owned();
        };
        if connection.is_some_and(|k| !c.connections.contains(&k)) {
            return "no such chain on that conveyor".to_owned();
        }
        let flipped = c.speed < 0.0;
        if let Some(old) = pk.ports[&pos].target {
            pk.chains.deregister_port(pos, old.conveyor);
        }
        pk.ports.get_mut(&pos).unwrap().target = Some(Target {
            conveyor,
            chain_pos,
            connection,
            flipped,
        });
        pk.register(pos);
        pk.wake(pos);
        pk.touch(pos);
        format!("frogport on {conveyor:?} at {chain_pos} {connection:?}")
    })
}

/// `ChainConveyorConnectionHandler`'s connection (no chains taken: the command stands in for
/// the chain item) or its removal.
fn chain_command(game: &mut Game<'_>, dim: usize, a: [i32; 3], b: [i32; 3], on: bool) -> String {
    let mut drops = Vec::new();
    let ok = with(dim, |pk| {
        let ok = if on {
            pk.chains.connect(a, b)
        } else {
            let had = pk.chains.conveyors.get(&a).is_some_and(|c| {
                c.connections
                    .contains(&[b[0] - a[0], b[1] - a[1], b[2] - a[2]])
            });
            let mut ev = Vec::new();
            pk.chains.disconnect(a, b, &mut ev);
            for e in ev {
                if let chain::Event::Drop(at, p) = e {
                    drops.push((at, p.stack));
                }
            }
            had
        };
        if ok {
            pk.conveyor_changed(a);
            pk.conveyor_changed(b);
            pk.touch(a);
            pk.touch(b);
        }
        ok
    });
    sync_far(dim, a);
    sync_far(dim, b);
    let dimension = game.dim.id().to_owned();
    for (at, s) in drops {
        drop_stack(game, &dimension, at, &s);
    }
    match (ok, on) {
        (true, true) => format!("chain {a:?} - {b:?}"),
        (true, false) => format!("unchained {a:?} - {b:?}"),
        (false, _) => "no such conveyors or chain".to_owned(),
    }
}

/// A packager's, port's or conveyor's state, for tests.
fn describe(dim: usize, pos: [i32; 3]) -> String {
    with(dim, |pk| {
        if let Some(p) = pk.packagers.get(&pos) {
            let held = p.held.as_ref().or(p.sliding.as_ref());
            let queued: u32 = p.queue.iter().map(|q| q.1.max(1)).sum();
            return format!(
                "{} address=\"{}\" powered={} animation={} inward={} held={} queued={queued}",
                if p.repackager {
                    "repackager"
                } else {
                    "packager"
                },
                p.address,
                p.powered,
                p.animation_ticks,
                p.inward,
                held.map_or("-".to_owned(), |b| format!(
                    "{} to \"{}\"",
                    b.stack.item, b.address
                )),
            );
        }
        if let Some(p) = pk.ports.get(&pos) {
            let items: Vec<String> = p
                .items
                .iter()
                .flatten()
                .map(|b| format!("{} to \"{}\"", b.stack.item, b.address))
                .collect();
            return format!(
                "{} filter=\"{}\" target={:?} animating={} items=[{}]",
                if p.frog { "frogport" } else { "postbox" },
                p.filter,
                p.target.map(|t| (t.conveyor, t.chain_pos, t.connection)),
                p.is_animating(),
                items.join(", ")
            );
        }
        if let Some(c) = pk.chains.conveyors.get(&pos) {
            let travelling: usize = c.travelling.iter().map(|(_, l)| l.len()).sum();
            let routes: Vec<String> = c
                .routes()
                .iter()
                .map(|(f, d, k)| format!("{f}:{d}->{k:?}"))
                .collect();
            return format!(
                "chain_conveyor speed={} connections={:?} looping={} travelling={} ports={} routes=[{}]",
                c.speed,
                c.connections,
                c.looping.len(),
                travelling,
                c.loop_ports.len() + c.travel_ports.len(),
                routes.join(", ")
            );
        }
        "nothing of the package logistics there".to_owned()
    })
}

/// The conveyors' packages to `players`, each conveyor in one message.
fn send_chains(game: &mut Game<'_>, players: &[u128], dim: usize, conveyors: &[[i32; 3]]) {
    if players.is_empty() {
        return;
    }
    for &pos in conveyors {
        let drawn = with(dim, |pk| pk.chains.drawn(pos, |s| s.item.to_string()));
        let data = chain::encode(pos, &drawn);
        for &p in players {
            send_to_player(game, p, chain::CHANNEL, &data);
        }
    }
}

/// A client joined (`sync`): every conveyor carrying packages in its dimension.
pub(super) fn net_from_client(game: &mut Game<'_>, player: u128, channel: &str, data: &[u8]) {
    if channel != create_engine::client::CHANNEL
        || data != b"sync"
        || PACKAGES.with_borrow(|p| p.is_none())
    {
        return;
    }
    let Some(dim) = game
        .native_server_players()
        .into_iter()
        .find(|p| p.uuid == player)
        .and_then(|p| dimension_index(&p.dimension))
    else {
        return;
    };
    let conveyors: Vec<[i32; 3]> = with(dim, |pk| {
        pk.chains
            .conveyors
            .iter()
            .filter(|(_, c)| !c.is_empty())
            .map(|(p, _)| *p)
            .collect()
    });
    send_chains(game, &[player], dim, &conveyors);
}

fn part_key(conveyor: [i32; 3], id: u32) -> u64 {
    let mut h: u64 = 0xC4A1_5EED_0000_0000;
    for v in conveyor.into_iter().chain([id as i32]) {
        h ^= v as u32 as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h | 1 << 63
}

/// A conveyor's packages from the server, drawn as GPU item parts: round the wheel turning
/// about it, along a chain gliding at the chain's speed; drawn again only when they change.
pub(super) fn net_from_server(game: &mut Game<'_>, channel: &str, data: &[u8]) {
    if channel != chain::CHANNEL {
        return;
    }
    let Some((pos, drawn)) = chain::decode(data) else {
        return;
    };
    for k in DRAWN
        .with_borrow_mut(|d| d.remove(&pos))
        .unwrap_or_default()
    {
        game.native_part_clear(k);
    }
    let now = game.native_parts_seconds();
    let mut keys = Vec::new();
    for d in drawn {
        let key = part_key(pos, d.id);
        // `ChainConveyorRenderer.renderBox`: hung 10 px up from its target, by the style's hook.
        let lift = package::style(&d.item).map_or(0.0, |s| {
            17.0 / 16.0 - s.rigging / 16.0 + f32::from(s.height) / 32.0
        });
        if d.looping {
            // Round the wheel: posed at angle 0 (out along +z), the part turns it about the
            // conveyor's centre.
            let pose = glam::Mat4::from_translation(glam::Vec3::new(
                0.5,
                6.0 / 16.0 - 9.0 / 16.0 + lift,
                0.5 + 0.875,
            )) * glam::Mat4::from_rotation_y((d.yaw - d.angle).to_radians());
            let Some(model) = game.native_part_item_model(&d.item, pose) else {
                continue;
            };
            let rpm = d.degrees / 6.0;
            let phase = (d.angle - d.degrees * now).rem_euclid(360.0);
            game.native_part_set(key, model, pos, 1, rpm, phase);
        } else {
            let block = d.at.map(|v| v.floor() as i32);
            let pose = glam::Mat4::from_translation(glam::Vec3::new(0.5, 0.5 + lift, 0.5))
                * glam::Mat4::from_rotation_y(d.yaw.to_radians());
            let Some(model) = game.native_part_item_model(&d.item, pose) else {
                continue;
            };
            let offset = [
                d.at[0] - block[0] as f32 - 0.5,
                d.at[1] - block[1] as f32 - 0.5,
                d.at[2] - block[2] as f32 - 0.5,
                now,
            ];
            game.native_part_glide(key, model, block, 1, 0.0, 0.0, offset, d.velocity);
        }
        keys.push(key);
    }
    if !keys.is_empty() {
        DRAWN.with_borrow_mut(|d| d.insert(pos, keys));
    }
}

/// Packagers, ports and conveyors of a column saved (`write`) under `create_packages`.
pub(super) fn chunk_data_save(dimension: &str, chunk: [i32; 2], data: &mut Compound) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    if PACKAGES.with_borrow(|p| p.is_none()) {
        return;
    }
    let inside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk[0], chunk[1]);
    let parcel_tag = |p: &Parcel<ItemStack>| {
        Tag::compound([
            ("Item", body::to_nbt(&p.stack)),
            ("Address", Tag::String(p.address.as_str().into())),
        ])
    };
    let pos_tag = |p: [i32; 3]| Tag::IntArray(p.to_vec());
    let own = with(dim, |pk| {
        let mut packagers: Vec<Tag> = Vec::new();
        let mut list: Vec<_> = pk.packagers.iter().filter(|(p, _)| inside(p)).collect();
        list.sort_by_key(|(p, _)| **p);
        for (p, k) in list {
            let mut c = vec![
                ("Pos", pos_tag(*p)),
                ("SignAddress", Tag::String(k.address.as_str().into())),
                (
                    "State",
                    Tag::IntArray(vec![
                        i32::from(k.powered),
                        k.animation_ticks as i32,
                        i32::from(k.inward),
                        k.button_cooldown as i32,
                    ]),
                ),
            ];
            if let Some(b) = &k.held {
                c.push(("HeldBox", parcel_tag(b)));
            }
            if let Some(b) = &k.sliding {
                c.push(("SlidingBox", parcel_tag(b)));
            }
            if k.repackager {
                c.push(("Repackager", Tag::IntArray(vec![1])));
            }
            if !k.queue.is_empty() {
                // `QueuedExitingPackages`: each box with its count.
                let queue = k
                    .queue
                    .iter()
                    .filter_map(|(b, n)| {
                        let Tag::Compound(mut c) = parcel_tag(b) else {
                            return None;
                        };
                        c.insert("Count".into(), Tag::IntArray(vec![*n as i32]));
                        Some(Tag::Compound(c))
                    })
                    .collect();
                c.push(("QueuedExitingPackages", Tag::List(queue)));
            }
            packagers.push(Tag::compound(c));
        }
        let mut ports: Vec<Tag> = Vec::new();
        let mut list: Vec<_> = pk.ports.iter().filter(|(p, _)| inside(p)).collect();
        list.sort_by_key(|(p, _)| **p);
        for (p, port) in list {
            let items: Vec<Tag> = port
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    let b = s.as_ref()?;
                    let Tag::Compound(mut c) = parcel_tag(b) else {
                        return None;
                    };
                    c.insert("Slot".into(), Tag::IntArray(vec![i as i32]));
                    Some(Tag::Compound(c))
                })
                .collect();
            let mut c = vec![
                ("Pos", pos_tag(*p)),
                (
                    "Frog",
                    Tag::IntArray(vec![i32::from(port.frog), i32::from(port.accepts)]),
                ),
                ("AddressFilter", Tag::String(port.filter.as_str().into())),
                ("Inventory", Tag::List(items)),
            ];
            if let Some(t) = port.target {
                let k = t.connection.unwrap_or([0; 3]);
                c.push((
                    "Target",
                    Tag::IntArray(vec![
                        t.conveyor[0],
                        t.conveyor[1],
                        t.conveyor[2],
                        i32::from(t.connection.is_some()),
                        k[0],
                        k[1],
                        k[2],
                        i32::from(t.flipped),
                    ]),
                ));
                c.push(("ChainPos", Tag::Double(f64::from(t.chain_pos).to_bits())));
            }
            if let Some(b) = &port.animated {
                c.push(("AnimatedPackage", parcel_tag(b)));
                c.push(("Deposit", Tag::IntArray(vec![i32::from(port.deposit)])));
            }
            ports.push(Tag::compound(c));
        }
        let mut conveyors: Vec<Tag> = Vec::new();
        let mut list: Vec<_> = pk
            .chains
            .conveyors
            .iter()
            .filter(|(p, _)| inside(p))
            .collect();
        list.sort_by_key(|(p, _)| **p);
        let package_tag = |b: &chain::Package<ItemStack>| {
            let Tag::Compound(mut c) = parcel_tag(&b.parcel) else {
                unreachable!()
            };
            c.insert(
                "Position".into(),
                Tag::Double(f64::from(b.position).to_bits()),
            );
            Tag::Compound(c)
        };
        for (p, c) in list {
            let travelling: Vec<Tag> = c
                .travelling
                .iter()
                .map(|(k, l)| {
                    Tag::compound([
                        ("Target", Tag::IntArray(k.to_vec())),
                        ("Packages", Tag::List(l.iter().map(package_tag).collect())),
                    ])
                })
                .collect();
            conveyors.push(Tag::compound([
                ("Pos", pos_tag(*p)),
                (
                    "Connections",
                    Tag::IntArray(c.connections.iter().flatten().copied().collect()),
                ),
                (
                    "LoopingPackages",
                    Tag::List(c.looping.iter().map(package_tag).collect()),
                ),
                ("TravellingPackages", Tag::List(travelling)),
            ]));
        }
        (packagers, ports, conveyors)
    });
    let (packagers, ports, conveyors) = own;
    if packagers.is_empty() && ports.is_empty() && conveyors.is_empty() {
        data.remove(&KEY.into());
        return;
    }
    data.insert(
        KEY.into(),
        Tag::compound([
            ("packagers", Tag::List(packagers)),
            ("ports", Tag::List(ports)),
            ("conveyors", Tag::List(conveyors)),
        ]),
    );
}

const KEY: &str = "create_packages";

fn ints(t: Option<&Tag>) -> Vec<i32> {
    match t {
        Some(Tag::IntArray(v)) => v.clone(),
        _ => Vec::new(),
    }
}

fn text(t: Option<&Tag>) -> String {
    match t {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    }
}

fn float(t: Option<&Tag>) -> f32 {
    match t {
        Some(Tag::Double(bits)) => f64::from_bits(*bits) as f32,
        _ => 0.0,
    }
}

fn pos_of(c: &Compound) -> Option<[i32; 3]> {
    let v = ints(c.get(&"Pos".into()));
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

fn parcel_from(t: Option<&Tag>) -> Option<Parcel<ItemStack>> {
    let Some(Tag::Compound(c)) = t else {
        return None;
    };
    Some(Parcel {
        stack: body::from_nbt(c.get(&"Item".into())?)?,
        address: text(c.get(&"Address".into())),
    })
}

/// A column joined a level: its packagers, ports and conveyors back (`read`).
pub(super) fn chunk_load(dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let Some(data) = Game::native_chunk_data(chunk, KEY) else {
        return;
    };
    let list = |key: &str| -> Vec<Compound> {
        match data.get(&key.into()) {
            Some(Tag::List(l)) => l
                .iter()
                .filter_map(|t| match t {
                    Tag::Compound(c) => Some(c.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    let mut conveyors = Vec::new();
    with(dim, |pk| {
        for c in list("packagers") {
            let Some(pos) = pos_of(&c) else { continue };
            let s = ints(c.get(&"State".into()));
            let k = pk.packager(pos);
            k.address = text(c.get(&"SignAddress".into()));
            k.powered = s.first().is_some_and(|v| *v != 0);
            k.animation_ticks = s.get(1).copied().unwrap_or(0).clamp(0, 20) as u32;
            k.inward = s.get(2).is_none_or(|v| *v != 0);
            k.button_cooldown = s.get(3).copied().unwrap_or(0).clamp(0, 40) as u32;
            k.held = parcel_from(c.get(&"HeldBox".into()));
            k.sliding = parcel_from(c.get(&"SlidingBox".into()));
            k.repackager = !ints(c.get(&"Repackager".into())).is_empty();
            if let Some(Tag::List(l)) = c.get(&"QueuedExitingPackages".into()) {
                k.queue = l
                    .iter()
                    .filter_map(|t| {
                        let Tag::Compound(e) = t else { return None };
                        let n = ints(e.get(&"Count".into())).first().copied().unwrap_or(1);
                        Some((parcel_from(Some(t))?, n.max(0) as u32))
                    })
                    .collect();
            }
            if k.active() {
                pk.activate(pos);
            }
        }
        for c in list("conveyors") {
            let Some(pos) = pos_of(&c) else { continue };
            let connections: Vec<[i32; 3]> = ints(c.get(&"Connections".into()))
                .chunks_exact(3)
                .map(|k| [k[0], k[1], k[2]])
                .collect();
            pk.chains.add(pos, connections);
            let packages = |t: Option<&Tag>| -> Vec<(f32, Parcel<ItemStack>)> {
                match t {
                    Some(Tag::List(l)) => l
                        .iter()
                        .filter_map(|t| {
                            let Tag::Compound(c) = t else { return None };
                            Some((float(c.get(&"Position".into())), parcel_from(Some(t))?))
                        })
                        .collect(),
                    _ => Vec::new(),
                }
            };
            let looping = packages(c.get(&"LoopingPackages".into()));
            let travelling = match c.get(&"TravellingPackages".into()) {
                Some(Tag::List(l)) => l
                    .iter()
                    .filter_map(|t| {
                        let Tag::Compound(e) = t else { return None };
                        let k = ints(e.get(&"Target".into()));
                        (k.len() == 3)
                            .then(|| ([k[0], k[1], k[2]], packages(e.get(&"Packages".into()))))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            pk.chains.load_packages(pos, looping, travelling);
            conveyors.push(pos);
        }
        for c in list("ports") {
            let Some(pos) = pos_of(&c) else { continue };
            let f = ints(c.get(&"Frog".into()));
            let frog = f.first().is_some_and(|v| *v != 0);
            let port = pk.port(pos, frog);
            port.accepts = f.get(1).is_none_or(|v| *v != 0);
            port.filter = text(c.get(&"AddressFilter".into()));
            if let Some(Tag::List(items)) = c.get(&"Inventory".into()) {
                for t in items {
                    let Tag::Compound(e) = t else { continue };
                    let slot = ints(e.get(&"Slot".into())).first().copied().unwrap_or(-1);
                    if let (Some(s), Some(b)) = (
                        port.items.get_mut(slot.max(0) as usize),
                        parcel_from(Some(t)),
                    ) {
                        *s = Some(b);
                    }
                }
            }
            let t = ints(c.get(&"Target".into()));
            if t.len() == 8 {
                port.target = Some(Target {
                    conveyor: [t[0], t[1], t[2]],
                    chain_pos: float(c.get(&"ChainPos".into())),
                    connection: (t[3] != 0).then_some([t[4], t[5], t[6]]),
                    flipped: t[7] != 0,
                });
            }
            let deposit = ints(c.get(&"Deposit".into()))
                .first()
                .is_some_and(|v| *v != 0);
            if let Some(b) = parcel_from(c.get(&"AnimatedPackage".into())) {
                pk.start(pos, b, deposit);
            }
            pk.register(pos);
            pk.wake(pos);
        }
    });
    for pos in conveyors {
        sync_far(dim, pos);
        with(dim, |pk| pk.conveyor_changed(pos));
    }
}

/// A column left a level: what was in it is forgotten (kept in its save).
pub(super) fn chunk_unload(dimension: &str, chunk: &horizon_client::chunk::Chunk) {
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    if PACKAGES.with_borrow(|p| p.is_none()) {
        return;
    }
    let inside = |p: &[i32; 3]| (p[0] >> 4, p[2] >> 4) == (chunk.x, chunk.z);
    let conveyors = with(dim, |pk| {
        pk.packagers.retain(|p, _| !inside(p));
        pk.active.retain(|p| !inside(p));
        let ports: Vec<[i32; 3]> = pk.ports.keys().filter(|p| inside(p)).copied().collect();
        for p in ports {
            pk.remove_port(p);
        }
        let conveyors: Vec<[i32; 3]> = pk
            .chains
            .conveyors
            .keys()
            .filter(|p| inside(p))
            .copied()
            .collect();
        for p in &conveyors {
            pk.chains.unload(*p);
        }
        conveyors
    });
    for p in conveyors {
        with_world(|w| w.dims[dim].kinetics.set_far(p, Vec::new()));
    }
}
