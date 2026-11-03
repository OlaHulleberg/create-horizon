// Create's logistics in the game: depots, chutes, funnels, belts and brass and andesite tunnels.
// A sibling of the `create` module (`runtime.rs`).
use super::Game;
use super::create::*;
use super::create_arms::*;
use super::create_processing::*;
use create_engine::client;
use create_engine::kinetics::Dir;
use create_engine::states;
use create_engine::world::dimension_index;

/// `BeltHelper.isItemUpright`: items of `create:upright_on_belt` stand, unless also in
/// `create:not_upright_on_belt`. (Fluid-holding items stand in Create too: not yet.)
pub(super) fn upright(game: &Game<'_>, item: &str) -> bool {
    let tags = &game.items().item_tags;
    let has = |t: &str| tags.get(t).is_some_and(|m| m.contains(item));
    has("create:upright_on_belt") && !has("create:not_upright_on_belt")
}

/// `TransportedItemStack`'s `R.nextInt(360)`: Create's shared random; any angle will do.
pub(super) fn random_angle(game: &Game<'_>, pos: [i32; 3]) -> i32 {
    let mut x = (game.world.game_time as u64)
        ^ (pos[0] as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (pos[2] as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (pos[1] as u64) << 40;
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x % 360) as i32
}

/// `SharedDepotBlockMethods.onUse` on the depot's top: everything it holds goes to the
/// player, then the stack in hand is put down, sliding in from the player's side.
pub(super) fn use_depot(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    if face != [0, 1, 0] {
        return;
    }
    *canceled = true;
    // The main hand's use always succeeds, so the off hand is never asked.
    if game.hand != horizon_client::first_person::Hand::Main {
        return;
    }
    let hand = game.inventory.get_mut(game.selected).and_then(Option::take);
    let facing = create_engine::depot::Side::of_yaw(game.yaw);
    let put = hand.as_ref().map(|s| {
        create_engine::depot::Held::put_by_player(
            random_angle(game, pos),
            upright(game, &s.item),
            facing,
        )
    });
    let taken = game.native_with_mod_block_entity(pos, |be| {
        be.items
            .resize(be.items.len().max(create_engine::depot::SLOTS), None);
        let taken: Vec<_> = be.items.iter_mut().filter_map(Option::take).collect();
        be.items[0] = hand.clone();
        taken
    });
    let Some(taken) = taken else {
        // No block entity: the stack stays in hand.
        if let Some(slot) = game.inventory.get_mut(game.selected) {
            *slot = hand;
        }
        return;
    };
    for s in taken {
        game.native_give_items(&s.item, s.count);
    }
    with_world(|w| w.depot_put(dim, pos, put));
}

/// `DepotBehaviour.tick` for depots whose stack slides, then the depots whose contents
/// changed go to the players of the level.
pub(super) fn tick_depots(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let sliding = with_world(|w| w.dims[dim].sliding.clone()).unwrap_or_default();
    if !sliding.is_empty() {
        let standing: Vec<[i32; 3]> = sliding
            .iter()
            .copied()
            .filter(|p| {
                depot_items(game, *p)
                    .first()
                    .is_some_and(|(slot, item, _)| *slot == 0 && upright(game, item))
            })
            .collect();
        with_world(|w| w.tick_depots(dim, |p| standing.contains(&p)));
    }
    let changed = with_world(|w| w.take_depot_changes(dim)).unwrap_or_default();
    if changed.is_empty() {
        return;
    }
    // Arms using these depots and ejectors holding a new stack look again.
    arms_touched(dim, &changed, &[]);
    wake_ejectors(dim, &changed);
    // A depot that joined holding a stack (a saved world, a block placed with its items) has
    // it at rest in the centre, as Create reads `HeldItem` back.
    for &pos in &changed {
        let unknown = with_world(|w| matches!(w.dims[dim].depots.get(&pos), Some(None)));
        if unknown != Some(true) {
            continue;
        }
        if let Some((_, item, _)) = depot_items(game, pos).into_iter().find(|s| s.0 == 0) {
            let held =
                create_engine::depot::Held::new(random_angle(game, pos), upright(game, &item));
            with_world(|w| {
                if let Some(h) = w.dims[dim].depots.get_mut(&pos) {
                    *h = Some(held);
                }
                w.wake_depot(dim, pos);
            });
        }
    }
    let players: Vec<u128> = game
        .native_server_players()
        .into_iter()
        .filter(|p| p.dimension == dimension)
        .map(|p| p.uuid)
        .collect();
    send_depots(game, &players, dim, &changed);
}

/// The stacks of the depot at `pos`, by slot.
pub(super) fn depot_items(game: &Game<'_>, pos: [i32; 3]) -> Vec<(u8, String, u8)> {
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(horizon_client::containers::BlockEntity::Mod(m)) => m
            .items
            .iter()
            .take(create_engine::depot::SLOTS)
            .enumerate()
            .filter_map(|(i, s)| {
                let s = s.as_ref()?;
                Some((i as u8, s.item.to_string(), s.count.min(255) as u8))
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn send_depots(game: &mut Game<'_>, players: &[u128], dim: usize, depots: &[[i32; 3]]) {
    let contents: Vec<create_engine::depot::Contents> = depots
        .iter()
        .map(|&pos| {
            let held = with_world(|w| w.dims[dim].depots.get(&pos).copied().flatten()).flatten();
            let stacks = if with_world(|w| w.dims[dim].depots.contains_key(&pos)) == Some(true) {
                depot_items(game, pos)
            } else {
                Vec::new()
            };
            create_engine::depot::Contents {
                pos,
                held: held.unwrap_or(create_engine::depot::Held::new(0, false)),
                stacks,
            }
        })
        .collect();
    for m in create_engine::depot::encode(&contents) {
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::depot::CHANNEL, &m);
        }
    }
}

/// A key for copy `copy` of the stack in `slot` of the depot at `pos`, apart from the kinetic
/// parts' keys (a well-mixed hash of the three).
pub(super) fn depot_key(pos: [i32; 3], slot: u8, copy: u8) -> u64 {
    let mut x = client::key(pos, 0)
        ^ (u64::from(slot) << 8 | u64::from(copy)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31) | 1 << 63
}

pub(super) const COPIES: u8 = 4;

pub(super) fn hide_depot(game: &mut Game<'_>, pos: [i32; 3]) {
    for slot in 0..create_engine::depot::SLOTS as u8 {
        for copy in 0..COPIES {
            game.native_part_clear(depot_key(pos, slot, copy));
        }
    }
}

/// Draws a depot's stacks as Create's `DepotRenderer` does: the held stack at its offset, turned
/// by its angle; each output 0.35 out at 45° steps. `next` is where the held stack will be a
/// tick from now, for the GPU to glide it there.
pub(super) fn show_depot(
    game: &mut Game<'_>,
    c: &create_engine::depot::Contents,
    next: Option<create_engine::depot::Held>,
) {
    use create_engine::depot;
    hide_depot(game, c.pos);
    let now = game.native_parts_seconds();
    for (slot, item, count) in &c.stacks {
        let block_item = game.native_item_uses_block_light(item).unwrap_or(false);
        let (poses, phase, offset, velocity) = if *slot == 0 {
            let at = c.held.offset();
            let to = next.map_or(at, |n| n.offset());
            let poses = depot::stack_poses(
                u32::from(*count),
                block_item,
                &mut depot::JavaRandom::new(0),
            );
            (poses, c.held.angle as f32, at, (to - at) * 20.0)
        } else {
            let (poses, phase) =
                depot::output_place(usize::from(*slot) - 1, u32::from(*count), block_item);
            (poses, phase, glam::Vec3::ZERO, glam::Vec3::ZERO)
        };
        for (copy, pose) in poses.into_iter().take(usize::from(COPIES)).enumerate() {
            let Some(model) = game.native_part_item_model(item, depot::top_centre() * pose) else {
                continue;
            };
            game.native_part_glide(
                depot_key(c.pos, *slot, copy as u8),
                model,
                c.pos,
                1,
                0.0,
                phase,
                [offset.x, offset.y, offset.z, now],
                velocity.to_array(),
            );
        }
    }
}

/// Depots' contents from the server.
pub(super) fn depots_from_server(game: &mut Game<'_>, data: &[u8]) {
    for c in create_engine::depot::decode(data) {
        let pos = c.pos;
        if c.stacks.is_empty() {
            hide_depot(game, pos);
            DEPOTS.with_borrow_mut(|d| {
                d.shown.remove(&pos);
                d.sliding.retain(|p| *p != pos);
            });
            continue;
        }
        let sliding = c.held.sliding() && c.stacks.iter().any(|s| s.0 == 0);
        let mut next = c.held;
        next.slide(false);
        show_depot(game, &c, sliding.then_some(next));
        DEPOTS.with_borrow_mut(|d| {
            d.shown.insert(pos, c);
            d.sliding.retain(|p| *p != pos);
            if sliding {
                d.sliding.push(pos);
            }
        });
    }
}

/// Each client tick: held stacks still sliding move on a tick (`DepotBehaviour.tick` on the
/// client), drawn gliding to where they will be next.
pub(super) fn tick_client_depots(game: &mut Game<'_>) {
    let sliding = DEPOTS.with_borrow(|d| d.sliding.clone());
    for pos in sliding {
        let Some(mut c) = DEPOTS.with_borrow(|d| d.shown.get(&pos).cloned()) else {
            continue;
        };
        let up = c
            .stacks
            .iter()
            .find(|s| s.0 == 0)
            .is_some_and(|s| upright(game, &s.1));
        c.held.slide(up);
        let still = c.held.sliding();
        let mut next = c.held;
        next.slide(up);
        show_depot(game, &c, still.then_some(next));
        DEPOTS.with_borrow_mut(|d| {
            d.shown.insert(pos, c);
            if !still {
                d.sliding.retain(|p| *p != pos);
            }
        });
    }
}

/// `BeltProcessingBehaviour.isBlocked`: a block with a collision shape between a depot and the
/// press above it. (Funnels, which Create lets through, are not ported yet.)
pub(super) fn blocked(game: &Game<'_>, at: [i32; 3]) -> bool {
    let state = game.world.block(at[0], at[1], at[2]);
    game.res.behaviours().facts(state).flags
        & horizon_world::level::data::state_flags::COLLISION_EMPTY
        == 0
}

/// The players of `dimension`.
pub(super) fn players_in(game: &Game<'_>, dimension: &str) -> Vec<u128> {
    game.native_server_players()
        .into_iter()
        .filter(|p| p.dimension == dimension)
        .map(|p| p.uuid)
        .collect()
}

/// A stack's limit (at most 64).
pub(super) fn stack_limit(game: &Game<'_>, item: &str) -> u8 {
    game.items()
        .get(item)
        .map_or(64, |x| x.max_stack)
        .clamp(1, 64) as u8
}

/// `ChuteBlockEntity.canAcceptItem` for the chute or smart chute `block` at `pos` (its state
/// `powered`): a smart chute also not while powered and only what its filter passes
/// (`SmartChuteBlockEntity.canAcceptItem`). Emptiness is the caller's.
pub(super) fn chute_accepts(
    game: &Game<'_>,
    dim: usize,
    pos: [i32; 3],
    block: create_engine::Block,
    powered: bool,
    stack: &horizon_client::items::ItemStack,
) -> bool {
    match block {
        create_engine::Block::Chute => true,
        create_engine::Block::SmartChute => {
            !powered
                && with_world(|w| {
                    super::create_items::passes(game, w.dims[dim].chute_filters.get(&pos), stack)
                })
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// The chute or smart chute at `pos`, with whether it is powered.
pub(super) fn chute_at(game: &Game<'_>, pos: [i32; 3]) -> Option<(create_engine::Block, bool)> {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    states::index(state)?;
    with_world(|w| {
        let (b, i) = w.table.get(state)?;
        matches!(
            b,
            create_engine::Block::Chute | create_engine::Block::SmartChute
        )
        .then_some((b, i.powered))
    })
    .flatten()
}

/// `ChuteBlockEntity` taking an item that landed on it (`canAcceptItem`: only while empty; a
/// smart chute's filter and power): the whole stack, at the top.
pub(super) fn chute_take(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    stack: &mut horizon_client::items::ItemStack,
) {
    let Some((block, powered)) = chute_at(game, pos) else {
        return;
    };
    if !chute_accepts(game, dim, pos, block, powered, stack) {
        return;
    }
    let taken = game
        .native_with_mod_block_entity(pos, |be| {
            be.items.resize(be.items.len().max(1), None);
            if be.items[0].is_some() {
                return false;
            }
            be.items[0] = Some(stack.clone());
            true
        })
        .unwrap_or(false);
    if taken {
        stack.count = 0;
        with_world(|w| w.dims[dim].chutes.insert(pos, 1.0));
    }
}

pub(super) fn chute_stack(
    game: &Game<'_>,
    pos: [i32; 3],
) -> Option<horizon_client::items::ItemStack> {
    match game.world.block_entities.get(&(pos[0], pos[1], pos[2])) {
        Some(horizon_client::containers::BlockEntity::Mod(m)) => m.items.first().cloned().flatten(),
        _ => None,
    }
}

pub(super) fn set_chute_stack(
    game: &mut Game<'_>,
    pos: [i32; 3],
    stack: Option<horizon_client::items::ItemStack>,
) {
    game.native_with_mod_block_entity(pos, |be| {
        be.items.resize(be.items.len().max(1), None);
        be.items[0] = stack;
    });
    // Arms using the chute look again.
    if let Some(dim) = dimension_index(game.dim.id()) {
        with_world(|w| w.dims[dim].arms.touched(pos));
    }
}

/// Chutes holding a stack (`ChuteBlockEntity.tick` falling, `handleDownwardOutput`): into the
/// container below (as a hopper puts in; a vault's whole inventory as Create's chute stacks it),
/// else the chute below if it is empty (and a smart chute's filter passes it), else out of the
/// bottom when nothing solid is under it; it waits in the middle while the way is shut, or while
/// a smart chute is powered (`canActivate`). Then the empty chutes that may take from the
/// inventory above them do.
pub(super) fn tick_chutes(game: &mut Game<'_>, dim: usize) {
    use create_engine::chute::{Fall, fall};
    let moving: Vec<([i32; 3], f32)> =
        with_world(|w| w.dims[dim].chutes.iter().map(|(p, v)| (*p, *v)).collect())
            .unwrap_or_default();
    for (pos, position) in moving {
        let Some(stack) = chute_stack(game, pos) else {
            with_world(|w| {
                w.dims[dim].chutes.remove(&pos);
                // Emptied (a hopper below took it): it may take from above again.
                w.dims[dim].chute_pulls.wake(pos);
            });
            continue;
        };
        let active = !matches!(chute_at(game, pos), Some((_, true)));
        if !active && position <= 0.5 {
            // A powered smart chute holds its stack in the middle; it ticks again once its
            // `powered` changes.
            with_world(|w| w.dims[dim].chutes.remove(&pos));
            continue;
        }
        let below = [pos[0], pos[1] - 1, pos[2]];
        let below_chute = chute_at(game, below);
        let container = below_chute.is_none()
            && game
                .world
                .block_entities
                .contains_key(&(below[0], below[1], below[2]));
        let can = if !active {
            false
        } else if let Some((b, powered)) = below_chute {
            chute_stack(game, below).is_none()
                && chute_accepts(game, dim, below, b, powered, &stack)
        } else if container {
            true
        } else {
            // Block.canSupportRigidBlock: nothing to drop onto.
            let below_state = game.world.block(below[0], below[1], below[2]);
            game.res.behaviours().facts(below_state).flags
                & horizon_world::level::data::state_flags::COLLISION_EMPTY
                != 0
        };
        let next = match fall(position, can) {
            Fall::At(p) => Some(p),
            Fall::Out if below_chute.is_some() => {
                set_chute_stack(game, below, Some(stack));
                set_chute_stack(game, pos, None);
                with_world(|w| w.dims[dim].chutes.insert(below, 1.0));
                None
            }
            Fall::Out if container => {
                let count = stack.count;
                let left = insert_item(
                    game,
                    dim,
                    below,
                    horizon_core::geometry::Direction::Up,
                    stack.clone(),
                )
                .filter(|s| s.count > 0);
                if left.as_ref().is_some_and(|l| l.count == count) {
                    Some(0.5)
                } else {
                    let done = left.is_none();
                    set_chute_stack(game, pos, left);
                    (!done).then_some(0.5)
                }
            }
            Fall::Out => {
                // An item 12/16 below the centre, falling at a quarter block a tick.
                let (x, y, z) = (
                    f64::from(pos[0]) + 0.5,
                    f64::from(pos[1]) + 0.5 - 0.75,
                    f64::from(pos[2]) + 0.5,
                );
                game.native_run_command(&format!(
                    "summon minecraft:item {x} {y} {z} {{Item:{{id:'{}',count:{}}},Motion:[0d,-0.25d,0d]}}",
                    stack.item, stack.count
                ));
                set_chute_stack(game, pos, None);
                None
            }
        };
        with_world(|w| match next {
            Some(p) => {
                w.dims[dim].chutes.insert(pos, p);
            }
            None => {
                w.dims[dim].chutes.remove(&pos);
                w.dims[dim].chute_pulls.wake(pos);
            }
        });
    }
    tick_chute_pulls(game, dim);
}

/// `ChuteBlockEntity.handleInputFromAbove` for the empty chutes due to look: up to 16 of the
/// first item of the inventory above (a smart chute: up to 64 of what its filter passes, and
/// nothing while powered) start falling from the top. One that finds nothing sleeps until a
/// container of Create's says it changed (`invVersionTracker`), or looks again `RETRY` ticks
/// later under another container; with no container above it waits for a neighbour change.
pub(super) fn tick_chute_pulls(game: &mut Game<'_>, dim: usize) {
    let time = game.world.game_time;
    let due = with_world(|w| w.dims[dim].chute_pulls.take_due(time)).unwrap_or_default();
    for pos in due {
        let Some((block, powered)) = chute_at(game, pos) else {
            continue;
        };
        if powered || chute_stack(game, pos).is_some() {
            // Powered: it looks again when that changes; holding a stack: when it is empty.
            continue;
        }
        let above = [pos[0], pos[1] + 1, pos[2]];
        // A creative crate (no block entity here) is looked at again as a vanilla container.
        let supply = super::create_decor::crate_supply(game, dim, above);
        // A portable storage interface hands down its contraption's storage while connected;
        // it wakes the chute when it connects.
        let psi = super::create_actors::is_item_interface(game, above);
        let tells = match game
            .world
            .block_entities
            .get(&(above[0], above[1], above[2]))
        {
            _ if supply.is_some() => false,
            _ if psi => true,
            None => continue,
            Some(e) => matches!(e, horizon_client::containers::BlockEntity::Mod(_)),
        };
        // `grabCapability(UP)`: a chute above hands its items down itself.
        if chute_at(game, above).is_some() {
            continue;
        }
        let smart = block == create_engine::Block::SmartChute;
        let filter = smart
            .then(|| with_world(|w| w.dims[dim].chute_filters.get(&pos).cloned()).flatten())
            .flatten();
        let amount = if smart {
            create_engine::chute::SMART_AMOUNT
        } else {
            create_engine::chute::CHUTE_AMOUNT
        };
        let test = |game: &Game<'_>, s: &horizon_client::items::ItemStack| {
            super::create_items::passes(game, filter.as_ref(), s)
        };
        let taken = match vault_at(game, dim, above) {
            _ if supply.is_some() => {
                // The crate's supply is a plain item: the filter tests one of it.
                super::create_decor::crate_extract(
                    game,
                    supply.flatten().as_deref(),
                    |id| test(game, &horizon_client::items::ItemStack::new(id, 1)),
                    amount,
                )
            }
            _ if psi => super::create_actors::psi_extract(game, dim, above, test, amount)
                .ok()
                .flatten(),
            Some(parts) => extract_vault(game, &parts, test, amount),
            None => super::machines::create_extract::extract_matching(
                game,
                above,
                horizon_core::geometry::Direction::Down,
                test,
                amount,
            ),
        };
        match taken {
            Some(stack) => {
                set_chute_stack(game, pos, Some(stack));
                with_world(|w| w.dims[dim].chutes.insert(pos, 1.0));
            }
            None => {
                with_world(|w| {
                    let p = &mut w.dims[dim].chute_pulls;
                    if tells {
                        p.sleep(pos);
                    } else {
                        p.retry(pos, time);
                    }
                });
            }
        }
    }
}

/// Two stacks Create would merge (`isSameItemSameComponents`).
pub(super) fn same_stack(
    a: &horizon_client::items::ItemStack,
    b: &horizon_client::items::ItemStack,
) -> bool {
    a.item == b.item && a.damage == b.damage && a.extra == b.extra
}

/// A stack compared as Create compares stacks, for the engine's extraction.
struct StackKey<'a>(&'a horizon_client::items::ItemStack);

impl PartialEq for StackKey<'_> {
    fn eq(&self, other: &Self) -> bool {
        same_stack(self.0, other.0)
    }
}

/// The blocks whose slots make up the item vault at `pos`, in order, if one is there. A miss
/// is a state test.
pub(super) fn vault_at(game: &Game<'_>, dim: usize, pos: [i32; 3]) -> Option<Vec<[i32; 3]>> {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    states::index(state)?;
    with_world(|w| {
        (w.table.get(state)?.0 == create_engine::Block::ItemVault)
            .then(|| w.dims[dim].vaults.inventory(pos))
            .flatten()
    })
    .flatten()
}

/// The slots of vault blocks, in order (each `vault::SLOTS` long).
pub(super) fn vault_slots(
    game: &Game<'_>,
    parts: &[[i32; 3]],
) -> Vec<Option<horizon_client::items::ItemStack>> {
    let mut out = Vec::with_capacity(parts.len() * create_engine::vault::SLOTS);
    for p in parts {
        let items = match game.world.block_entities.get(&(p[0], p[1], p[2])) {
            Some(horizon_client::containers::BlockEntity::Mod(m)) => m.items.as_slice(),
            _ => &[],
        };
        for i in 0..create_engine::vault::SLOTS {
            out.push(items.get(i).cloned().flatten());
        }
    }
    out
}

/// Sets the slots of the vault blocks holding a `touched` slot to `now` (as `vault_slots` gave
/// them): each such block's contents change once.
fn set_vault_slots(
    game: &mut Game<'_>,
    parts: &[[i32; 3]],
    now: Vec<Option<horizon_client::items::ItemStack>>,
    touched: &[usize],
) {
    let n = create_engine::vault::SLOTS;
    for (i, p) in parts.iter().enumerate() {
        if !touched.iter().any(|t| t / n == i) {
            continue;
        }
        let slots = &now[i * n..(i + 1) * n];
        game.native_with_mod_block_entity(*p, |be| {
            be.items.resize(be.items.len().max(n), None);
            be.items[..n].clone_from_slice(slots);
        });
    }
}

/// `stack` into the item vault made of `parts` (`ItemHandlerHelper.insertItemStacked`, as
/// Create's chutes and funnels insert): onto the same items first, then into empty slots, 64 to a
/// slot at most. What did not fit.
pub(super) fn insert_vault(
    game: &mut Game<'_>,
    parts: &[[i32; 3]],
    stack: horizon_client::items::ItemStack,
) -> Option<horizon_client::items::ItemStack> {
    let mut slots = vault_slots(game, parts);
    let view: Vec<Option<(bool, u32)>> = slots
        .iter()
        .map(|s| s.as_ref().map(|s| (same_stack(s, &stack), s.count)))
        .collect();
    let limit = u32::from(stack_limit(game, &stack.item));
    let plan = create_engine::vault::insert_stacked(&view, stack.count, limit);
    let mut left = stack.count;
    let mut touched = Vec::with_capacity(plan.len());
    for &(i, n) in &plan {
        left -= n;
        touched.push(i);
        match &mut slots[i] {
            Some(s) => s.count += n,
            slot => {
                *slot = Some(horizon_client::items::ItemStack {
                    count: n,
                    ..stack.clone()
                })
            }
        }
    }
    set_vault_slots(game, parts, slots, &touched);
    (left > 0).then_some(horizon_client::items::ItemStack {
        count: left,
        ..stack
    })
}

/// Up to `amount` of the first item `test` passes from the item vault made of `parts`
/// (`ItemHelper.extract`, up to): gathered from its slots in order.
pub(super) fn extract_vault(
    game: &mut Game<'_>,
    parts: &[[i32; 3]],
    test: impl Fn(&Game<'_>, &horizon_client::items::ItemStack) -> bool,
    amount: u32,
) -> Option<horizon_client::items::ItemStack> {
    let mut slots = vault_slots(game, parts);
    let plan = {
        let keys: Vec<Option<(StackKey<'_>, u32, u32)>> = slots
            .iter()
            .map(|s| {
                s.as_ref()
                    .map(|s| (StackKey(s), s.count, u32::from(stack_limit(game, &s.item))))
            })
            .collect();
        create_engine::chute::extract(&keys, |k| test(game, k.0), false, amount)
    };
    let &(first, _) = plan.first()?;
    let mut out = slots[first].clone()?;
    out.count = 0;
    let mut touched = Vec::with_capacity(plan.len());
    for &(i, n) in &plan {
        touched.push(i);
        out.count += n;
        if let Some(s) = &mut slots[i] {
            s.count -= n;
            if s.count == 0 {
                slots[i] = None;
            }
        }
    }
    set_vault_slots(game, parts, slots, &touched);
    Some(out)
}

/// Puts `stack` into the container at `pos` through its side `face`: an item vault's whole
/// inventory, else as a hopper would (`native_insert_item`). What did not fit.
pub(super) fn insert_item(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    face: horizon_core::geometry::Direction,
    stack: horizon_client::items::ItemStack,
) -> Option<horizon_client::items::ItemStack> {
    // A packager unpacks what it is given; a port takes packages not for it.
    let stack = match super::create_packages::automation_insert(game, dim, pos, stack) {
        Ok(left) => return left,
        Err(stack) => stack,
    };
    // `BottomlessItemHandler.insertItem`: a creative crate takes everything.
    if super::create_decor::crate_supply(game, dim, pos).is_some() {
        return None;
    }
    // A portable storage interface: into its contraption's storage while connected.
    let stack = match super::create_actors::psi_insert(game, dim, pos, stack) {
        Ok(left) => return left,
        Err(stack) => stack,
    };
    match vault_at(game, dim, pos) {
        Some(parts) => insert_vault(game, &parts, stack),
        None => game.native_insert_item(pos, face, stack),
    }
}

/// How much of `stack` `insert_item` would put into the container at `pos` (simulated, nothing
/// changes); None when there is no container.
pub(super) fn insert_room(
    game: &Game<'_>,
    dim: usize,
    pos: [i32; 3],
    face: horizon_core::geometry::Direction,
    stack: &horizon_client::items::ItemStack,
) -> Option<u32> {
    if let Some(room) = super::create_packages::automation_room(game, dim, pos, stack) {
        return Some(room);
    }
    if super::create_decor::crate_supply(game, dim, pos).is_some() {
        return Some(stack.count);
    }
    if let Some(room) = super::create_actors::psi_room(game, dim, pos, stack) {
        return Some(room);
    }
    match vault_at(game, dim, pos) {
        Some(parts) => {
            let slots = vault_slots(game, &parts);
            let view: Vec<Option<(bool, u32)>> = slots
                .iter()
                .map(|s| s.as_ref().map(|s| (same_stack(s, stack), s.count)))
                .collect();
            let limit = u32::from(stack_limit(game, &stack.item));
            let plan = create_engine::vault::insert_stacked(&view, stack.count, limit);
            Some(plan.iter().map(|(_, n)| n).sum())
        }
        None => super::machines::create_extract::insert_room(game, pos, face, stack),
    }
}

/// A player's click on a smart chute's filter slot (`FilteringBehaviour.onShortInteract` through
/// `ValueSettingsInputHandler`): on a side, not sneaking, at the slot 11 px up; the held item
/// becomes its filter (a filter item taken from the hand, the one it held given back), an empty
/// hand clears it (no counts yet).
pub(super) fn use_smart_chute(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    face: [i32; 3],
    canceled: &mut bool,
) {
    if !may_use_value_box(game) {
        return;
    }
    let (Some(side), Some(hit)) = (Dir::from_offset(face), game.pick()) else {
        return;
    };
    let local = [0, 1, 2].map(|i| hit.point[i] - f64::from(pos[i]));
    let Some(slot) = create_engine::chute::filter_slot(side) else {
        return;
    };
    if !create_engine::board::hits_box(slot, local) {
        return;
    }
    let held = game.native_held().map(|h| h.0);
    // `canShortInteract`: a wrench or a mechanical arm is used on the block instead.
    if held
        .as_deref()
        .is_some_and(|h| matches!(h, "create:wrench" | "create:mechanical_arm"))
    {
        return;
    }
    *canceled = true;
    let filter = super::create_items::take_filter(game);
    let old = with_world(|w| w.set_chute_filter(dim, pos, filter)).flatten();
    super::create_items::return_filter(game, pos, old);
}

/// Item vaults whose blocks changed form or split (`updateConnectivity`), and their blocks
/// show it (`large`). Nothing when no vault block changed.
pub(super) fn tick_vaults(game: &mut Game<'_>, dim: usize) {
    let large = with_world(|w| w.tick_vaults(dim)).unwrap_or_default();
    for (pos, l) in large {
        let state = game.world.block(pos[0], pos[1], pos[2]).0;
        let vault = with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
            == Some(create_engine::Block::ItemVault);
        if vault {
            game.native_set_block_property_with(
                pos,
                "large",
                if l { "true" } else { "false" },
                super::native_hooks::flags::UPDATE_CLIENTS,
            );
        }
    }
}

/// Vault blocks' parts (`ItemVaultBlockEntity.write`: `Controller`, `Size`, `Length`,
/// `Uninitialized`) and smart chutes' filters saved with a column: `vaults` holds x, y, z, the
/// controller's x, y, z, width, length and 1 when it still looks for its vault, for each block;
/// `chute_filters` the positions and the filter stacks (an item id in saves from before filter
/// items).
pub(super) fn vaults_to_nbt(
    own: &mut horizon_world::nbt::Compound,
    vaults: &[([i32; 3], create_engine::vault::Part)],
    filters: &[([i32; 3], create_engine::filter::Filter)],
) {
    use horizon_world::nbt::Tag;
    if !vaults.is_empty() {
        let v = vaults
            .iter()
            .flat_map(|(p, x)| {
                let c = x.controller.unwrap_or(*p);
                [
                    p[0],
                    p[1],
                    p[2],
                    c[0],
                    c[1],
                    c[2],
                    x.width,
                    x.length,
                    i32::from(x.dirty),
                ]
            })
            .collect();
        own.insert("vaults".into(), Tag::IntArray(v));
    }
    if !filters.is_empty() {
        let mut c = horizon_world::nbt::Compound::new();
        let pos = filters.iter().flat_map(|(p, _)| *p).collect();
        let items = filters
            .iter()
            .map(|(_, f)| super::create_items::filter_to_nbt(f))
            .collect();
        c.insert("pos".into(), Tag::IntArray(pos));
        c.insert("items".into(), Tag::List(items));
        own.insert("chute_filters".into(), Tag::Compound(c));
    }
}

/// What `vaults_to_nbt` wrote.
#[allow(clippy::type_complexity)]
pub(super) fn vaults_from_nbt(
    data: Option<&horizon_world::nbt::Compound>,
) -> (
    Vec<([i32; 3], create_engine::vault::Part)>,
    Vec<([i32; 3], create_engine::filter::Filter)>,
) {
    use horizon_world::nbt::Tag;
    let Some(data) = data else {
        return Default::default();
    };
    let vaults = match data.get(&"vaults".into()) {
        Some(Tag::IntArray(v)) => v
            .chunks_exact(9)
            .map(|c| {
                let pos = [c[0], c[1], c[2]];
                let controller = [c[3], c[4], c[5]];
                let part = create_engine::vault::Part {
                    // The block's own state gives its axis and `large` when it is found.
                    axis: create_engine::kinetics::Axis::X,
                    controller: (controller != pos).then_some(controller),
                    width: c[6].clamp(1, create_engine::vault::MAX_WIDTH),
                    length: c[7].clamp(1, create_engine::vault::max_length(3)),
                    dirty: c[8] != 0,
                    large: false,
                };
                (pos, part)
            })
            .collect(),
        _ => Vec::new(),
    };
    let filters = match data.get(&"chute_filters".into()) {
        Some(Tag::Compound(c)) => match (c.get(&"pos".into()), c.get(&"items".into())) {
            (Some(Tag::IntArray(pos)), Some(Tag::List(items))) => pos
                .chunks_exact(3)
                .zip(items)
                .filter_map(|(p, i)| {
                    Some(([p[0], p[1], p[2]], super::create_items::filter_from_nbt(i)?))
                })
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    (vaults, filters)
}

/// A funnel's state as its rules read it: its kind (brass or not), facing, `powered`,
/// `extracting`, and a belt funnel's shape (None for a funnel on an inventory).
#[derive(Clone, Copy, Debug)]
pub(super) struct FunnelState {
    pub(super) brass: bool,
    pub(super) facing: Dir,
    pub(super) powered: bool,
    pub(super) extracting: bool,
    pub(super) belt: Option<create_engine::funnel::BeltShape>,
}

/// The funnel at `pos`, if one is there (a miss is a state test).
pub(super) fn funnel_at(game: &Game<'_>, pos: [i32; 3]) -> Option<FunnelState> {
    use create_engine::Block;
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    states::index(state)?;
    with_world(|w| {
        let (block, info) = w.table.get(state)?;
        let (brass, belt) = match block {
            Block::AndesiteFunnel => (false, false),
            Block::BrassFunnel => (true, false),
            Block::AndesiteBeltFunnel => (false, true),
            Block::BrassBeltFunnel => (true, true),
            _ => return None,
        };
        Some(FunnelState {
            brass,
            facing: info.facing?,
            powered: info.powered,
            extracting: info.extracting,
            belt: if belt {
                Some(info.belt_funnel.unwrap_or_default())
            } else {
                None
            },
        })
    })
    .flatten()
}

/// The way the belt below `pos` moves (`getMovementFacing`), when a belt that carries items is
/// there.
pub(super) fn belt_movement_below(game: &Game<'_>, dim: usize, pos: [i32; 3]) -> Option<Dir> {
    let below = [pos[0], pos[1] - 1, pos[2]];
    let (c, _, seg) = belt_segment(game, dim, below)?;
    if !seg.transports() {
        return None;
    }
    let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
    Some(create_engine::belt::movement_facing(rpm, seg.facing))
}

/// The facing of a belt that carries items below `pos` (`BeltFunnelBlock.isOnValidBelt`; Create
/// also lets saws and item drains carry belt funnels: not yet).
fn belt_facing_below(game: &Game<'_>, pos: [i32; 3]) -> Option<Dir> {
    let state = game.world.block(pos[0], pos[1] - 1, pos[2]).0;
    states::index(state)?;
    let seg = with_world(|w| w.table.get(state).and_then(|(_, i)| i.belt)).flatten()?;
    seg.transports().then_some(seg.facing)
}

/// Funnels whose block below changed (`FunnelBlock.updateShape`, `BeltFunnelBlock.updateShape`):
/// a horizontal funnel over a belt becomes its belt funnel (`getEquivalentBeltFunnel`, shaped by
/// `getShapeForPosition`), a belt funnel no longer on a belt becomes its funnel again (extracting
/// when it pushed), and a belt funnel's shape follows the belt below (wrenched shapes kept).
pub(super) fn tick_funnel_shapes(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::funnel::{BeltShape, shape_for_position, updated_shape};
    let checks = with_world(|w| std::mem::take(&mut w.dims[dim].funnel_shapes)).unwrap_or_default();
    for pos in checks {
        let Some(f) = funnel_at(game, pos) else {
            continue;
        };
        if f.facing.axis() == create_engine::kinetics::Axis::Y {
            continue;
        }
        let state = game.world.block(pos[0], pos[1], pos[2]);
        let water = game
            .pack
            .assets
            .registry
            .state(state)
            .and_then(|s| s.properties.get("waterlogged").cloned())
            .unwrap_or_else(|| "false".to_owned());
        let belt = belt_facing_below(game, pos);
        let metal = if f.brass { "brass" } else { "andesite" };
        let flag = |b: bool| if b { "true" } else { "false" };
        let (name, shape) = match (f.belt, belt) {
            (None, Some(b)) => (
                format!("create:{metal}_belt_funnel"),
                Some(shape_for_position(Some(b), f.facing, f.extracting)),
            ),
            (Some(shape), None) => (format!("create:{metal}_funnel"), Some(shape)),
            (Some(shape), Some(b)) => {
                let fresh = shape_for_position(Some(b), f.facing, shape == BeltShape::Pushing);
                let now = updated_shape(shape, fresh);
                if now == shape {
                    continue;
                }
                (format!("create:{metal}_belt_funnel"), Some(now))
            }
            (None, None) => continue,
        };
        let shape = shape.unwrap_or_default();
        let props: [(&str, &str); 4] = if f.belt.is_some() && belt.is_none() {
            [
                ("facing", dir_name(f.facing)),
                ("powered", flag(f.powered)),
                ("extracting", flag(shape == BeltShape::Pushing)),
                ("waterlogged", &water),
            ]
        } else {
            [
                ("facing", dir_name(f.facing)),
                ("powered", flag(f.powered)),
                ("shape", shape.name()),
                ("waterlogged", &water),
            ]
        };
        if let Some(id) = state_with(game.pack.assets.registry, &name, &props)
            && id != state
        {
            game.native_set_block(dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
        }
    }
}

/// Funnels due to look again (`FunnelBlockEntity.tick` once its cooldown has run out): an
/// extracting funnel on an inventory drops a stack in front while nothing lies there
/// (`activateExtractor`); a belt funnel pushing onto its belt puts one there
/// (`activateExtractingBeltFunnel`); either looks again 8 ticks after it moved items, the next
/// tick while blocked, and 8 ticks later when the inventory gave nothing (Create looks every
/// tick then). An inserting, pulling or powered funnel is dormant until its state changes; a
/// tick with no funnel due costs one look at the earliest.
pub(super) fn tick_funnels(game: &mut Game<'_>, dim: usize) {
    use create_engine::funnel::{EXTRACTION_COOLDOWN, Mode, amount, belt_mode};
    let now = game.world.game_time.max(0) as u64;
    let due = with_world(|w| {
        let d = &mut w.dims[dim];
        if d.funnel_timers.is_empty() {
            return Vec::new();
        }
        let funnels = &d.funnels;
        d.funnel_timers
            .due(now, |p, t| funnels.get(&p).is_some_and(|due| *due == t))
    })
    .unwrap_or_default();
    let schedule = |pos: [i32; 3], at: u64| {
        with_world(|w| {
            let d = &mut w.dims[dim];
            if let Some(due) = d.funnels.get_mut(&pos) {
                *due = at;
                if at != u64::MAX {
                    d.funnel_timers.at(at, pos);
                }
            }
        });
    };
    for pos in due {
        let Some(f) = funnel_at(game, pos) else {
            schedule(pos, u64::MAX);
            continue;
        };
        let mode = match f.belt {
            Some(shape) => belt_mode(
                f.powered,
                shape,
                belt_movement_below(game, dim, pos),
                f.facing,
            ),
            None if f.powered => Mode::Paused,
            None if f.extracting => Mode::Extract,
            None => Mode::Collect,
        };
        let face = horizon_core::geometry::Direction::ALL[f.facing as usize];
        let behind = create_engine::funnel::behind(pos, f.facing);
        let max = amount(f.brass);
        let next = match mode {
            Mode::Extract => extract_in_front(game, dim, pos, f.facing, face, behind, max),
            Mode::PushingToBelt => push_onto_belt(game, dim, pos, f.facing, face, behind, max),
            // Inserting, pulling from a belt (the belt does it), paused or invalid: dormant.
            _ => None,
        };
        schedule(
            pos,
            match next {
                Some(Some(false)) => now + 1,
                Some(_) => now + u64::from(EXTRACTION_COOLDOWN),
                None => u64::MAX,
            },
        );
    }
}

/// Takes up to `max` from the inventory behind a funnel through its face (a vault's whole
/// inventory, else as a hopper takes); None when nothing was there.
fn take_from_behind(
    game: &mut Game<'_>,
    dim: usize,
    behind: [i32; 3],
    face: horizon_core::geometry::Direction,
    max: u32,
) -> Option<horizon_client::items::ItemStack> {
    // A packager's box once it is out; a port's packages for it.
    if let Some(taken) = super::create_packages::automation_take(game, dim, behind) {
        return taken;
    }
    // `BottomlessItemHandler.extractItem`: a creative crate gives its supply, a stack at most.
    if let Some(supply) = super::create_decor::crate_supply(game, dim, behind) {
        return super::create_decor::crate_extract(game, supply.as_deref(), |_| true, max);
    }
    // A portable storage interface: its contraption's storage while connected.
    if let Ok(taken) = super::create_actors::psi_extract(game, dim, behind, |_, _| true, max) {
        return taken;
    }
    match vault_at(game, dim, behind) {
        Some(parts) => extract_vault(game, &parts, |_, _| true, max),
        None => super::machines::create_extract::extract_item(game, behind, face, max),
    }
}

/// `activateExtractor`: Some(Some(true)) when a stack left, Some(Some(false)) when an item in
/// front blocks it, Some(None) when the inventory gave nothing.
fn extract_in_front(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    facing: Dir,
    face: horizon_core::geometry::Direction,
    behind: [i32; 3],
    max: u32,
) -> Option<Option<bool>> {
    let (at, v) = create_engine::funnel::output(pos, facing);
    // Something still in front: wait (`lastObserved`, the overflow area).
    let mut blocked = false;
    let dimension = game.dim.id().to_owned();
    items_in(
        game,
        &dimension,
        [at[0] - 0.5, at[1] - 0.5, at[2] - 0.5],
        [at[0] + 0.5, at[1] + 0.5, at[2] + 0.5],
        |_, _, _, _| {
            blocked = true;
        },
    );
    if blocked {
        return Some(Some(false));
    }
    let Some(stack) = take_from_behind(game, dim, behind, face, max) else {
        return Some(None);
    };
    game.native_run_command(&format!(
        "summon minecraft:item {} {} {} {{Item:{{id:'{}',count:{}}},Motion:[{}d,{}d,{}d]}}",
        at[0], at[1], at[2], stack.item, stack.count, v[0], v[1], v[2]
    ));
    Some(Some(true))
}

/// `activateExtractingBeltFunnel`: up to `max` of the inventory behind onto the belt below,
/// inserted from the funnel's side (`handleInsertion(stack, facing)`) while the belt takes items
/// there (`canInsertFromSide`, `isOccupied`). Same answers as `extract_in_front`.
fn push_onto_belt(
    game: &mut Game<'_>,
    dim: usize,
    pos: [i32; 3],
    facing: Dir,
    face: horizon_core::geometry::Direction,
    behind: [i32; 3],
    max: u32,
) -> Option<Option<bool>> {
    let below = [pos[0], pos[1] - 1, pos[2]];
    let (c, index, _) = belt_segment(game, dim, below)?;
    let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
    let free = with_world(|w| {
        let b = w.dims[dim].belts.get(&c)?;
        let movement = create_engine::belt::movement_facing(rpm, b.segment.facing);
        Some(
            rpm != 0.0
                && movement != facing.opposite()
                && b.items.can_insert_at_from_side(index, facing, movement),
        )
    })
    .flatten()?;
    if !free {
        return Some(Some(false));
    }
    let Some(stack) = take_from_behind(game, dim, behind, face, max) else {
        return Some(None);
    };
    let count = stack.count.min(64) as u8;
    let item = create_engine::belt::Item::new(
        &stack.item,
        count,
        random_angle(game, pos),
        upright(game, &stack.item),
    );
    let left = insert_into_from(game, dim, below, facing, &item, false).unwrap_or(count);
    if left > 0 {
        // The belt took less after all: the rest goes back where it came from.
        let back = horizon_client::items::ItemStack {
            count: u32::from(left),
            ..stack
        };
        if let Some(rest) = insert_item(game, dim, behind, face, back) {
            game.native_give_items(&rest.item, rest.count);
        }
    }
    Some(Some(true))
}

/// A block entity slot as the processing rules see it.
pub(super) fn slot_of(
    s: &Option<horizon_client::items::ItemStack>,
) -> create_engine::processing::Slot {
    s.as_ref()
        .map(|s| (s.item.to_string(), s.count.min(255) as u8))
}

/// The belt segment at `pos`: its controller, its index along the chain, and its state.
pub(super) fn belt_segment(
    game: &Game<'_>,
    dim: usize,
    pos: [i32; 3],
) -> Option<([i32; 3], i32, create_engine::belt::Segment)> {
    let state = game.world.block(pos[0], pos[1], pos[2]).0;
    let seg = with_world(|w| w.table.get(state).and_then(|(_, i)| i.belt)).flatten()?;
    let (c, index) = with_world(|w| {
        let d = &w.dims[dim];
        let c = *d.belt_of.get(&pos)?;
        let index = d.belts.get(&c)?.chain.iter().position(|p| *p == pos)? as i32;
        Some((c, index))
    })
    .flatten()?;
    Some((c, index, seg))
}

/// `BeltBlock.useItemOn` with an empty main hand: every item within reach of the segment's
/// centre goes to the player.
pub(super) fn use_belt(game: &mut Game<'_>, dim: usize, pos: [i32; 3], canceled: &mut bool) {
    if game.native_held().is_some_and(|h| h.0 != "minecraft:air")
        || game.hand != horizon_client::first_person::Hand::Main
        || game.keys.contains(&winit::keyboard::KeyCode::ShiftLeft)
    {
        return;
    }
    let Some((c, index, _)) = belt_segment(game, dim, pos) else {
        return;
    };
    *canceled = true;
    let taken = with_world(|w| {
        let b = w.dims[dim].belts.get_mut(&c)?;
        let taken = b.items.take_within(index as f32 + 0.5, 0.55);
        if !taken.is_empty() {
            w.dims[dim].belt_changes.push(c);
        }
        Some(taken)
    })
    .flatten()
    .unwrap_or_default();
    for i in taken {
        game.native_give_items(&i.item, u32::from(i.count));
    }
}

/// `BeltBlock.updateEntityAfterFallOn` / `entityInside` for an item, and
/// `SharedDepotBlockMethods.onLanded`: a belt takes a landing stack onto its segment (when
/// nothing just put there is in the way), a depot takes it when it holds nothing.
pub(super) fn item_landed(
    game: &mut Game<'_>,
    pos: [i32; 3],
    stack: &mut horizon_client::items::ItemStack,
    at: [f64; 3],
) {
    // The entity's own block, else the one under it; neither a Create block: done.
    let own = [
        at[0].floor() as i32,
        at[1].floor() as i32,
        at[2].floor() as i32,
    ];
    let s_own = game.world.block(own[0], own[1], own[2]).0;
    let s_pos = game.world.block(pos[0], pos[1], pos[2]).0;
    if states::index(s_own).is_none() && states::index(s_pos).is_none() {
        return;
    }
    let Some(dim) = dimension_index(game.dim.id()) else {
        return;
    };
    let kind = |s: u32| with_world(|w| w.table.get(s).map(|(b, _)| b)).flatten();
    let (spot, block) = match (kind(s_own), kind(s_pos)) {
        (Some(b @ create_engine::Block::Belt), _) => (own, b),
        (_, Some(b @ create_engine::Block::Belt)) => (pos, b),
        (_, Some(b @ create_engine::Block::Depot)) => (pos, b),
        // EjectorBlock.updateEntityAfterFallOn: an item stack lands as on a depot.
        (_, Some(create_engine::Block::WeightedEjector)) => (pos, create_engine::Block::Depot),
        (Some(b @ create_engine::Block::Basin), _) => (own, b),
        (_, Some(b @ (create_engine::Block::Chute | create_engine::Block::SmartChute))) => (pos, b),
        (_, Some(b @ create_engine::Block::MechanicalSaw)) => (pos, b),
        (_, Some(b @ create_engine::Block::ItemDrain)) => (pos, b),
        (
            _,
            Some(b @ (create_engine::Block::AndesiteFunnel | create_engine::Block::BrassFunnel)),
        ) => (pos, b),
        _ => return,
    };
    if stack.extra.is_some() {
        // Stacks with components keep them only as entities, for now.
        return;
    }
    if block == create_engine::Block::Basin {
        basin_take(game, dim, spot, stack);
        return;
    }
    if matches!(
        block,
        create_engine::Block::Chute | create_engine::Block::SmartChute
    ) {
        chute_take(game, dim, spot, stack);
        return;
    }
    if block == create_engine::Block::MechanicalSaw {
        saw_take(game, dim, spot, stack);
        return;
    }
    if matches!(
        block,
        create_engine::Block::AndesiteFunnel | create_engine::Block::BrassFunnel
    ) {
        // FunnelBlock.entityInside, inserting: into the inventory it is on, as much as fits.
        let state = game.world.block(spot[0], spot[1], spot[2]);
        let Some(s) = game.pack.assets.registry.state(state) else {
            return;
        };
        let extracting = s.properties.get("extracting").is_some_and(|v| v == "true");
        let powered = s.properties.get("powered").is_some_and(|v| v == "true");
        let facing = with_world(|w| w.table.get(state.0).and_then(|(_, i)| i.facing)).flatten();
        if extracting || powered {
            return;
        }
        let Some(facing) = facing else {
            return;
        };
        let o = facing.offset();
        let behind = [spot[0] - o[0], spot[1] - o[1], spot[2] - o[2]];
        let face = horizon_core::geometry::Direction::ALL[facing as usize];
        let left = insert_item(game, dim, behind, face, stack.clone());
        stack.count = left.map_or(0, |l| l.count);
        return;
    }
    if block == create_engine::Block::ItemDrain {
        // ItemDrainBlockEntity's insertion: a stack it can hold, while it holds nothing.
        let took = with_world(|w| {
            let d = w.dims[dim].drains.entry(spot).or_default();
            if d.held.is_some() {
                return false;
            }
            d.held = Some((stack.item.to_string(), stack.count.min(64) as u8));
            true
        })
        .unwrap_or(false);
        if took {
            stack.count = stack.count.saturating_sub(64);
        }
        return;
    }
    let max = game
        .items()
        .get(&stack.item)
        .map_or(64, |i| i.max_stack)
        .clamp(1, 64);
    let count = stack.count.min(max) as u8;
    let up = upright(game, &stack.item);
    if block == create_engine::Block::Depot {
        // DirectBeltInputBehaviour from above: only into an empty depot (an ejector's while
        // charged).
        if !depot_items(game, spot).is_empty() || !ejector_accepts(dim, spot) {
            return;
        }
        let put = horizon_client::items::ItemStack {
            item: stack.item.clone(),
            count: u32::from(count),
            damage: 0,
            extra: None,
        };
        if game
            .native_with_mod_block_entity(spot, |be| {
                be.items
                    .resize(be.items.len().max(create_engine::depot::SLOTS), None);
                be.items[0] = Some(put);
            })
            .is_none()
        {
            return;
        }
        stack.count -= u32::from(count);
        let held = create_engine::depot::Held::new(random_angle(game, spot), up);
        with_world(|w| w.depot_put(dim, spot, Some(held)));
        return;
    }
    let Some((c, index, seg)) = belt_segment(game, dim, spot) else {
        return;
    };
    if !seg.transports() {
        return;
    }
    let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
    let angle = random_angle(game, spot);
    let added = with_world(|w| {
        let b = w.dims[dim].belts.get_mut(&c)?;
        let movement = create_engine::belt::movement_facing(rpm, b.segment.facing);
        // ItemHandlerBeltSegment.insertItem: canInsertAt(segment), from above.
        if !b
            .items
            .can_insert_at_from_side(index, create_engine::kinetics::Dir::Up, movement)
        {
            return None;
        }
        let mut item = create_engine::belt::Item::new(&stack.item, count, angle, up);
        item.inserted_at = index;
        item.inserted_from = Some(create_engine::kinetics::Dir::Up);
        item.position = index as f32 + 0.5 + if b.items.positive { -1.0 } else { 1.0 } / 16.0;
        b.items.add(item);
        w.dims[dim].belt_changes.push(c);
        Some(())
    })
    .flatten();
    if added.is_some() {
        stack.count -= u32::from(count);
    }
}

/// `DirectBeltInputBehaviour.handleInsertion` at `next`, the item moving toward `side`: a
/// depot or a belt there. What is left of it, None when nothing there takes items.
pub(super) fn insert_into(
    game: &mut Game<'_>,
    dim: usize,
    next: [i32; 3],
    side: create_engine::kinetics::Dir,
    item: &create_engine::belt::Item,
) -> Option<u8> {
    insert_into_from(game, dim, next, side, item, true)
}

/// `insert_into` for a stack that comes from a belt behind (`from_belt`: Create's smoother
/// hand-over starts it a quarter block back) or from elsewhere (a funnel).
pub(super) fn insert_into_from(
    game: &mut Game<'_>,
    dim: usize,
    next: [i32; 3],
    side: create_engine::kinetics::Dir,
    item: &create_engine::belt::Item,
    from_belt: bool,
) -> Option<u8> {
    let is_depot = with_world(|w| w.dims[dim].depots.contains_key(&next)) == Some(true);
    if is_depot {
        // DepotBehaviour.tryInsertingFromSide: only into an empty depot (an ejector's only
        // while charged, `canAcceptItems`).
        if !depot_items(game, next).is_empty() || !ejector_accepts(dim, next) {
            return Some(item.count);
        }
        let put = horizon_client::items::ItemStack {
            item: item.item.as_str().into(),
            count: u32::from(item.count),
            damage: 0,
            extra: None,
        };
        game.native_with_mod_block_entity(next, |be| {
            be.items
                .resize(be.items.len().max(create_engine::depot::SLOTS), None);
            be.items[0] = Some(put);
        })?;
        let held = create_engine::depot::Held {
            position: 0.0,
            from: create_engine::depot::Side::from_dir(side),
            angle: item.angle,
            side_offset: item.side_offset,
            locked: false,
            fan: item.fan,
        };
        with_world(|w| w.depot_put(dim, next, Some(held)));
        return Some(0);
    }
    // BeltBlockEntity.tryInsertingFromSide on the next belt.
    let (c2, index, seg2) = belt_segment(game, dim, next)?;
    if !seg2.transports() {
        return None;
    }
    let rpm2 = with_world(|w| w.speed(dim, c2)).flatten().unwrap_or(0.0);
    let speed2 = create_engine::belt::direction_aware_speed(rpm2, seg2.facing);
    with_world(|w| {
        let b = w.dims[dim].belts.get_mut(&c2)?;
        let movement2 = create_engine::belt::movement_facing(rpm2, b.segment.facing);
        if rpm2 == 0.0
            || movement2 == side.opposite()
            || !b.items.can_insert_at_from_side(index, side, movement2)
        {
            return Some(item.count);
        }
        let mut moved = item.clone();
        moved.locked = false;
        moved.position = index as f32 + 0.5 - speed2.signum() / 16.0;
        if movement2 != side {
            moved.side_offset = if side.positive() { 0.675 } else { -0.675 };
            if side.axis() == create_engine::kinetics::Axis::X {
                moved.side_offset = -moved.side_offset;
            }
        } else {
            // A smoother hand-over from the belt behind it.
            let extra = if from_belt { 0.26 } else { 0.0 };
            moved.position = if speed2 > 0.0 {
                index as f32 - extra
            } else {
                index as f32 + 1.0 + extra
            };
        }
        moved.inserted_at = index;
        moved.inserted_from = Some(side);
        b.items.add(moved);
        w.dims[dim].belt_changes.push(c2);
        Some(0)
    })
    .flatten()
}

/// `BeltInventory.eject`: an item thrown off the end, forward and a little up.
pub(super) fn eject_from_belt(
    game: &mut Game<'_>,
    controller: [i32; 3],
    seg: &create_engine::belt::Segment,
    length: i32,
    rpm: f32,
    item: &create_engine::belt::Item,
) {
    let at = create_engine::belt::vector_for_offset(controller, seg, length, item.position);
    let speed = create_engine::belt::movement_speed(rpm)
        .abs()
        .max(1.0 / 8.0);
    let d = create_engine::belt::movement_facing(rpm, seg.facing).offset();
    let motion = [
        f64::from(d[0]) * f64::from(speed),
        1.0 / 8.0,
        f64::from(d[2]) * f64::from(speed),
    ];
    game.native_run_command(&format!(
        "summon minecraft:item {:.4} {:.4} {:.4} {{Item:{{id:'{}',count:{}}},Motion:[{:.4}d,{:.4}d,{:.4}d]}}",
        at[0],
        at[1] + 6.0 / 16.0,
        at[2],
        item.item,
        item.count,
        motion[0],
        motion[1],
        motion[2]
    ));
}

/// Belts: segments broken next to a broken one become shafts or air, a broken belt's items
/// drop, then belts holding items move them (`BeltInventory.tick`); belts at rest or empty cost
/// nothing.
pub(super) fn tick_belts(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let (breaks, drops) = with_world(|w| {
        let d = &mut w.dims[dim];
        (
            std::mem::take(&mut d.belt_breaks),
            std::mem::take(&mut d.belt_drops),
        )
    })
    .unwrap_or_default();
    for b in breaks {
        let block = match b.shaft {
            Some(a) => format!(
                "create:shaft[axis={}]",
                ["x", "y", "z"][axis_index(a) as usize]
            ),
            None => "minecraft:air".to_owned(),
        };
        game.native_run_command(&format!(
            "execute in {dimension} run setblock {} {} {} {block}",
            b.pos[0], b.pos[1], b.pos[2]
        ));
    }
    for (c, seg, length, item) in drops {
        eject_from_belt(game, c, &seg, length, 0.0, &item);
    }
    let busy = with_world(|w| w.busy_belts(dim)).unwrap_or_default();
    if !busy.is_empty() {
        ensure_recipes(game);
    }
    for c in busy {
        let Some((seg, length, mut items)) = with_world(|w| {
            let b = w.dims[dim].belts.get_mut(&c)?;
            Some((
                b.segment,
                b.chain.len() as i32,
                std::mem::take(&mut b.items),
            ))
        })
        .flatten() else {
            continue;
        };
        let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
        let speed = create_engine::belt::direction_aware_speed(rpm, seg.facing);
        let mut side = BeltSide {
            game,
            dim,
            controller: c,
            segment: seg,
            length,
            rpm,
            positive: speed > 0.0,
        };
        let horizontal = seg.slope == create_engine::belt::Slope::Horizontal;
        let ticked = items.tick(length, speed, horizontal, &mut side);
        with_world(|w| {
            if let Some(b) = w.dims[dim].belts.get_mut(&c) {
                // Items another belt put here meanwhile go after ours.
                let added = std::mem::replace(&mut b.items, items);
                for i in added.items {
                    b.items.add(i);
                }
            }
            if ticked.changed {
                w.dims[dim].belt_changes.push(c);
            }
        });
        for item in &ticked.ejected {
            eject_from_belt(game, c, &seg, length, rpm, item);
        }
    }
    let changed = with_world(|w| {
        let mut v = std::mem::take(&mut w.dims[dim].belt_changes);
        v.sort();
        v.dedup();
        for c in &v {
            w.mark_unsaved(dim, *c);
        }
        v
    })
    .unwrap_or_default();
    if !changed.is_empty() {
        arms_touched(dim, &[], &changed);
        let players = players_in(game, dimension);
        send_belts(game, &players, dim, &changed);
    }
}

pub(super) fn send_belts(game: &mut Game<'_>, players: &[u128], dim: usize, belts: &[[i32; 3]]) {
    let all: Vec<create_engine::belt::Contents> = belts
        .iter()
        .map(|&c| {
            with_world(|w| w.dims[dim].belts.get(&c).cloned())
                .flatten()
                .map_or(
                    create_engine::belt::Contents {
                        controller: c,
                        segment: create_engine::belt::Segment {
                            facing: create_engine::kinetics::Dir::North,
                            slope: create_engine::belt::Slope::Horizontal,
                            part: create_engine::belt::Part::Start,
                        },
                        length: 0,
                        ending: create_engine::belt::Ending::Eject,
                        items: create_engine::belt::Inventory::default(),
                    },
                    |b| {
                        let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
                        let speed =
                            create_engine::belt::direction_aware_speed(rpm, b.segment.facing);
                        let mut side = BeltSide {
                            game,
                            dim,
                            controller: c,
                            segment: b.segment,
                            length: b.chain.len() as i32,
                            rpm,
                            positive: speed > 0.0,
                        };
                        let ending = create_engine::belt::Surroundings::ending(&mut side);
                        create_engine::belt::Contents {
                            controller: c,
                            segment: b.segment,
                            length: b.chain.len() as u16,
                            ending,
                            items: b.items,
                        }
                    },
                )
        })
        .collect();
    for m in create_engine::belt::encode(&all) {
        for &p in players {
            let _ = game.native_net_send_to_player(p, create_engine::belt::CHANNEL, &m);
        }
    }
}

pub(super) fn is_block_item(game: &Game<'_>, item: &str) -> bool {
    if let Some(b) = BLOCK_ITEMS.with_borrow(|m| m.get(item).copied()) {
        return b;
    }
    let b = game.native_item_uses_block_light(item).unwrap_or(false);
    BLOCK_ITEMS.with_borrow_mut(|m| m.insert(item.to_owned(), b));
    b
}

/// A key for copy `copy` of item `index` on the belt at `controller`, apart from the others.
pub(super) fn belt_key(controller: [i32; 3], index: usize, copy: u8) -> u64 {
    depot_key(controller, 200, copy) ^ (index as u64 + 1).wrapping_mul(0xD6E8_FEB8_6659_FD93)
        | 1 << 63
}

/// Draws the items of a belt: each copy a GPU item part gliding at its belt's speed; uploads
/// only what moves differently from before.
pub(super) fn draw_belt(
    game: &mut Game<'_>,
    contents: &create_engine::belt::Contents,
    shown: &mut Vec<Shown>,
    rpm: f32,
    redraw: bool,
) {
    use create_engine::belt;
    let c = contents.controller;
    let length = i32::from(contents.length);
    let speed = belt::direction_aware_speed(rpm, contents.segment.facing);
    let now = game.native_parts_seconds();
    if redraw {
        for (i, s) in shown.iter().enumerate() {
            for copy in 0..s.copies {
                game.native_part_clear(belt_key(c, i, copy));
            }
        }
        shown.clear();
    }
    for (i, item) in contents.items.items.iter().enumerate() {
        let place = belt::item_place(&contents.segment, length, item.position, item.side_offset);
        let moving = !item.locked && !item.at_funnel && !item.stalled && speed != 0.0;
        let velocity = if moving {
            belt::item_velocity(&contents.segment, length, item.position, speed).to_array()
        } else {
            [0.0; 3]
        };
        let flat = place.tilt == 0.0;
        // Moving as it was uploaded: the GPU already has it where it is.
        if let Some(s) = shown.get(i)
            && s.velocity == velocity
            && s.flat == flat
            && s.angle == item.angle
            && s.count == item.count
            && s.item == item.item
        {
            continue;
        }
        let block_item = is_block_item(game, &item.item);
        let copies = belt::item_copies(u32::from(item.count), block_item, item.angle);
        // Flat: baked about the block's centre, turned by the item's angle on the GPU; on a
        // slope the tilt comes first, so the angle is baked in.
        let (centre, phase) = if flat {
            (glam::Vec3::new(0.5, 0.0, 0.5), item.angle as f32)
        } else {
            (glam::Vec3::ZERO, 0.0)
        };
        let tilt = if flat {
            glam::Mat4::IDENTITY
        } else {
            let axis = if place.tilt_axis == create_engine::kinetics::Axis::Z {
                glam::Vec3::Z
            } else {
                glam::Vec3::X
            };
            glam::Mat4::from_axis_angle(axis, place.tilt.to_radians())
                * glam::Mat4::from_rotation_y((item.angle as f32).to_radians())
        };
        let old_copies = shown.get(i).map_or(0, |s| s.copies);
        for copy in copies.len() as u8..old_copies {
            game.native_part_clear(belt_key(c, i, copy));
        }
        for (copy, pose) in copies.iter().enumerate() {
            let Some(model) = game.native_part_item_model(
                &item.item,
                glam::Mat4::from_translation(centre) * tilt * *pose,
            ) else {
                continue;
            };
            let at = place.at - centre;
            game.native_part_glide(
                belt_key(c, i, copy as u8),
                model,
                c,
                1,
                0.0,
                phase,
                [at.x, at.y, at.z, now],
                velocity,
            );
        }
        let entry = Shown {
            item: item.item.clone(),
            count: item.count,
            angle: item.angle,
            flat,
            velocity,
            copies: copies.len() as u8,
        };
        if i < shown.len() {
            shown[i] = entry;
        } else {
            shown.push(entry);
        }
    }
}

pub(super) fn belts_from_server(game: &mut Game<'_>, data: &[u8]) {
    for contents in create_engine::belt::decode(data) {
        let c = contents.controller;
        let old = BELTS.with_borrow_mut(|b| b.belts.remove(&c));
        let mut shown = old.map(|o| o.1).unwrap_or_default();
        if contents.length == 0 {
            for (i, s) in shown.iter().enumerate() {
                for copy in 0..s.copies {
                    game.native_part_clear(belt_key(c, i, copy));
                }
            }
            continue;
        }
        let rpm = belt_rpm(c);
        draw_belt(game, &contents, &mut shown, rpm, true);
        BELTS.with_borrow_mut(|b| b.belts.insert(c, (contents, shown)));
    }
}

pub(super) fn belt_rpm(controller: [i32; 3]) -> f32 {
    CLIENT.with_borrow(|c| {
        c.as_ref()
            .and_then(|c| c.speeds.get(&controller))
            .map_or(0.0, |s| s[0])
    })
}

/// A belt's surroundings on the client: its ending as the server last said, and the belt
/// funnels over it, which hold the items they face on the client too (`checkForFunnels` on the
/// client: blocking, never inserting).
pub(super) struct ClientBelt<'a, 'g> {
    pub(super) game: &'a Game<'g>,
    pub(super) ending: create_engine::belt::Ending,
    pub(super) controller: [i32; 3],
    pub(super) segment: create_engine::belt::Segment,
    pub(super) length: i32,
    pub(super) movement: Dir,
}

impl create_engine::belt::Surroundings for ClientBelt<'_, '_> {
    fn ending(&mut self) -> create_engine::belt::Ending {
        self.ending
    }
    fn insert(&mut self, _: &create_engine::belt::Item) -> Option<u8> {
        None
    }
    fn process(
        &mut self,
        _: i32,
        item: &mut create_engine::belt::Item,
        locked: bool,
    ) -> Option<create_engine::belt::Process> {
        (locked || item.locked).then_some(create_engine::belt::Process::Hold)
    }
    fn funnel(&mut self, segment: i32) -> Option<(create_engine::funnel::OnBelt, Dir)> {
        let at = create_engine::belt::position_for_offset(
            self.controller,
            &self.segment,
            self.length,
            segment,
        );
        let state = self.game.world.block(at[0], at[1] + 1, at[2]).0;
        states::index(state)?;
        let info = CLIENT.with_borrow(|c| {
            let (block, info) = c.as_ref()?.table.get(state)?;
            matches!(
                block,
                create_engine::Block::AndesiteBeltFunnel | create_engine::Block::BrassBeltFunnel
            )
            .then_some(*info)
        })?;
        let on = create_engine::funnel::OnBelt {
            facing: info.facing?,
            shape: info.belt_funnel.unwrap_or_default(),
            powered: info.powered,
        };
        Some((on, self.movement))
    }
}

/// Each client tick: belts with items move them on (`BeltInventory.tick` on the client).
pub(super) fn tick_client_belts(game: &mut Game<'_>) {
    let busy: Vec<[i32; 3]> = BELTS.with_borrow(|b| {
        b.belts
            .iter()
            .filter(|(_, (c, _))| !c.items.items.is_empty())
            .map(|(k, _)| *k)
            .collect()
    });
    for c in busy {
        let Some((mut contents, mut shown)) = BELTS.with_borrow_mut(|b| b.belts.remove(&c)) else {
            continue;
        };
        let rpm = belt_rpm(c);
        let speed = create_engine::belt::direction_aware_speed(rpm, contents.segment.facing);
        let before = contents.items.items.len();
        let horizontal = contents.segment.slope == create_engine::belt::Slope::Horizontal;
        let mut side = ClientBelt {
            game,
            ending: contents.ending,
            controller: c,
            segment: contents.segment,
            length: i32::from(contents.length),
            movement: create_engine::belt::movement_facing(rpm, contents.segment.facing),
        };
        contents
            .items
            .tick(i32::from(contents.length), speed, horizontal, &mut side);
        let redraw = contents.items.items.len() != before;
        draw_belt(game, &contents, &mut shown, rpm, redraw);
        BELTS.with_borrow_mut(|b| b.belts.insert(c, (contents, shown)));
    }
}

/// Brass tunnels holding a stack (`BrassTunnelBlockEntity.tick`): two ticks after it arrives
/// (then every `brassTunnelTimer` while some is left), the stack is split among the outputs
/// beside and ahead of the segment: a belt or depot across each open side, the belt ahead.
pub(super) fn tick_brass(game: &mut Game<'_>, dim: usize) {
    use create_engine::tunnel;
    let Some(list) = with_world(|w| w.dims[dim].brass_running.clone()) else {
        return;
    };
    for t in list {
        let Some(mut b) = with_world(|w| w.dims[dim].brass.get(&t).cloned()).flatten() else {
            with_world(|w| w.dims[dim].brass_running.retain(|p| *p != t));
            continue;
        };
        let below = [t[0], t[1] - 1, t[2]];
        if b.progress > 0 {
            b.progress -= 1;
        }
        let belt = belt_segment(game, dim, below);
        let rpm = belt
            .as_ref()
            .and_then(|(c, ..)| with_world(|w| w.speed(dim, *c)).flatten())
            .unwrap_or(0.0);
        let save = |b: &tunnel::Brass| {
            with_world(|w| {
                let d = &mut w.dims[dim];
                if let Some(x) = d.brass.get_mut(&t) {
                    *x = b.clone();
                }
                if b.stack.is_none() {
                    d.brass_running.retain(|p| *p != t);
                }
            });
        };
        let (Some((_, _, seg)), Some((item, count))) = (belt, b.stack.clone()) else {
            save(&b);
            continue;
        };
        if rpm == 0.0 {
            save(&b);
            continue;
        }
        let movement = create_engine::belt::movement_facing(rpm, seg.facing);
        let Some((axis, shape)) = tunnel_state(game.world.block(t[0], t[1], t[2]).0) else {
            continue;
        };
        // `addValidOutputsOf`: the sides across first, then ahead; never back.
        let open = tunnel::sides(axis, shape);
        let order = [Dir::South, Dir::West, Dir::North, Dir::East];
        let outputs: Vec<Dir> = order
            .iter()
            .filter(|d| d.axis() != movement.axis())
            .chain(order.iter().filter(|d| **d == movement))
            .copied()
            .filter(|d| open.contains(d) && Some(*d) != b.entered_from)
            .filter(|d| {
                let o = d.offset();
                let p = [below[0] + o[0], below[1], below[2] + o[2]];
                with_world(|w| {
                    w.dims[dim].depots.contains_key(&p) || w.dims[dim].belt_of.contains_key(&p)
                }) == Some(true)
            })
            .collect();
        if b.progress == -1 {
            if outputs.is_empty() {
                save(&b);
                continue;
            }
            if b.new_item {
                b.new_item = false;
                b.progress = 2;
            } else {
                b.progress = tunnel::BRASS_TIMER;
                save(&b);
                continue;
            }
        }
        if b.progress != 0 {
            save(&b);
            continue;
        }
        // `insertIntoTunnel`: a depot takes a stack while empty, a belt one it has room for.
        let left = tunnel::distribute(
            count,
            outputs.len(),
            b.previous_output,
            b.mode,
            |i, n, simulate| {
                let d = outputs[i];
                let o = d.offset();
                let p = [below[0] + o[0], below[1], below[2] + o[2]];
                if simulate {
                    let depot = with_world(|w| w.dims[dim].depots.contains_key(&p)) == Some(true);
                    if depot && !depot_items(game, p).is_empty() {
                        return Some(n);
                    }
                    return Some(0);
                }
                insert_into(
                    game,
                    dim,
                    p,
                    d,
                    &create_engine::belt::Item::new(&item, n, 180, false),
                )
            },
        );
        b.stack = (left > 0).then(|| (item.clone(), left));
        if b.stack.is_none() {
            b.entered_from = None;
        }
        b.previous_output = (b.previous_output + 1) % outputs.len().max(1);
        b.progress = -1;
        save(&b);
        with_world(|w| w.mark_unsaved(dim, t));
    }
}

/// An andesite tunnel's axis and shape; None for any other block.
pub(super) fn tunnel_state(
    state: u32,
) -> Option<(create_engine::kinetics::Axis, create_engine::tunnel::Shape)> {
    states::index(state)?;
    WORLD.with(|w| {
        let w = w.try_borrow().ok()?;
        let (block, info) = w.as_ref()?.table.get(state)?;
        matches!(
            block,
            create_engine::Block::AndesiteTunnel | create_engine::Block::BrassTunnel
        )
        .then_some((info.axis?, info.tunnel?))
    })
}

/// Andesite tunnels: those just placed case their belt (`BeltTunnelItem`); those whose
/// surroundings changed break without a cased, flat belt below (`canSurvive`) or take the shape
/// the belts and depots beside their segment give (`updateShape`, `getTunnelState`).
pub(super) fn tick_tunnels(game: &mut Game<'_>, dimension: &str, dim: usize) {
    use create_engine::belt::Slope;
    use create_engine::kinetics::{Axis, Dir};
    let Some((new, kept, fresh)) = with_world(|w| {
        let d = &mut w.dims[dim];
        (
            std::mem::take(&mut d.tunnels_new),
            std::mem::take(&mut d.tunnel_checks),
            std::mem::take(&mut d.tunnel_fresh),
        )
    }) else {
        return;
    };
    if new.is_empty() && kept.is_empty() && fresh.is_empty() {
        return;
    }
    let belt_below = |game: &Game<'_>, p: [i32; 3]| {
        let state = game.world.block(p[0], p[1] - 1, p[2]).0;
        with_world(|w| w.table.get(state).and_then(|(_, i)| i.belt)).flatten()
    };
    let cased = |game: &Game<'_>, p: [i32; 3]| {
        game.pack
            .assets
            .registry
            .state(game.world.block(p[0], p[1] - 1, p[2]))
            .is_some_and(|s| s.properties.get("casing").is_some_and(|v| v == "true"))
    };
    for pos in new {
        if tunnel_state(game.world.block(pos[0], pos[1], pos[2]).0).is_none() {
            continue;
        }
        if belt_below(game, pos).is_some_and(|s| s.slope == Slope::Horizontal) && !cased(game, pos)
        {
            game.native_set_block_property_with(
                [pos[0], pos[1] - 1, pos[2]],
                "casing",
                "true",
                super::native_hooks::flags::UPDATE_ALL,
            );
        }
    }
    let mut checks: Vec<([i32; 3], bool)> = fresh
        .into_iter()
        .map(|p| (p, true))
        .chain(kept.into_iter().map(|p| (p, false)))
        .collect();
    // A fresh look wins over a kept one for the same tunnel.
    checks.sort_by_key(|c| (c.0, !c.1));
    checks.dedup_by_key(|c| c.0);
    for (pos, at_once) in checks {
        let Some(now) = tunnel_state(game.world.block(pos[0], pos[1], pos[2]).0) else {
            continue;
        };
        let seg = belt_below(game, pos).filter(|s| s.slope == Slope::Horizontal);
        let Some(seg) = seg.filter(|_| cased(game, pos)) else {
            game.native_run_command(&format!(
                "execute in {dimension} run setblock {} {} {} air destroy",
                pos[0], pos[1], pos[2]
            ));
            continue;
        };
        let axis = seg.facing.axis();
        // `hasValidOutput` beside the belt segment: a belt across it, or a depot.
        let output = |game: &Game<'_>, side: Dir| {
            let o = side.offset();
            let p = [pos[0] + o[0], pos[1] - 1, pos[2] + o[2]];
            let state = game.world.block(p[0], p[1], p[2]).0;
            with_world(|w| match w.table.get(state) {
                Some((create_engine::Block::Depot, _)) => true,
                Some((_, i)) => i.belt.is_some_and(|b| b.facing.axis() == side.axis()),
                None => false,
            })
            .unwrap_or(false)
        };
        let left = create_engine::tunnel::left(axis);
        let (on_left, on_right) = (output(game, left), output(game, left.opposite()));
        // `canHaveWindow`: tunnels on both ends.
        let fw = if axis == Axis::X {
            Dir::East
        } else {
            Dir::South
        };
        let window = [fw, fw.opposite()].into_iter().all(|d| {
            let o = d.offset();
            tunnel_state(game.world.block(pos[0] + o[0], pos[1], pos[2] + o[2]).0).is_some()
        });
        let fresh = (
            axis,
            create_engine::tunnel::shape_for(on_left, on_right, window),
        );
        let (axis2, shape2) = if at_once {
            fresh
        } else {
            create_engine::tunnel::updated(now, fresh)
        };
        if (axis2, shape2) == now {
            continue;
        }
        let registry = game.pack.assets.registry;
        let current = game.world.block(pos[0], pos[1], pos[2]);
        let Some(mut s) = registry.state(current).cloned() else {
            continue;
        };
        let axis_name = if axis2 == Axis::X { "x" } else { "z" };
        s.properties.insert("axis".into(), axis_name.into());
        s.properties.insert("shape".into(), shape2.name().into());
        if let Some(id) = registry.id(&s) {
            game.native_set_block(dimension, pos, id, super::native_hooks::flags::UPDATE_ALL);
        }
    }
}
