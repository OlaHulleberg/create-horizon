//! Depots on the server: the held stack sliding to the centre (`DepotBehaviour.tick`), what a
//! player puts down and takes (`SharedDepotBlockMethods.onUse`), what hoppers change, and the
//! contents clients draw (`create:depot`).

use super::super::net;
use super::super::server::ServerLevel;
use super::super::world::with_world;
use super::arms::{arms_touched, wake_ejectors};
use super::items::{self, random_angle, upright};
use create_engine::depot;
use mod_api::PlayerRef;

/// The stacks of the depot at `pos`, by slot: (slot, item, count).
pub fn depot_items(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Vec<(u8, String, u8)> {
    let Some(slots) = items::slots(lv, pos, depot::SLOTS as u32) else {
        return Vec::new();
    };
    slots
        .into_iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let s = s?;
            Some((
                i as u8,
                items::id_of(&*lv.host, &s),
                s.count().min(255) as u8,
            ))
        })
        .collect()
}

/// `SharedDepotBlockMethods.onUse` on the depot's top: everything it holds goes to the
/// player, then the stack in hand is put down, sliding in from the player's side. True when
/// the click was the depot's.
pub fn use_depot(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    face: mod_api::Direction,
    player: u128,
    main_hand: bool,
) -> bool {
    if face != mod_api::Direction::Up {
        return false;
    }
    // The main hand's use always succeeds, so the off hand is never asked.
    if !main_hand {
        return true;
    }
    let Some(p) = lv.host.player(player) else {
        return true;
    };
    let Some(selected) = p.selected_slot(&*lv.host) else {
        return true;
    };
    let Some(e) = items::entity(lv, pos) else {
        // No block entity: the stack stays in hand.
        return true;
    };
    let hand = p
        .inventory_slots(&*lv.host)
        .unwrap_or_default()
        .into_iter()
        .find(|(i, _)| *i as usize == selected)
        .map(|(_, s)| s);
    let facing = player_yaw(lv, p).map_or(depot::Side::North, depot::Side::of_yaw);
    let put = hand.as_ref().map(|s| {
        let id = items::id_of(&*lv.host, s);
        depot::Held::put_by_player(random_angle(lv, pos), upright(&*lv.host, &id), facing)
    });
    let taken: Vec<_> = (0..depot::SLOTS as u32)
        .filter_map(|i| e.item_slot(&*lv.host, i).ok().flatten())
        .collect();
    let mut set: Vec<(u32, Option<mod_api::ItemStack>)> =
        (1..depot::SLOTS as u32).map(|i| (i, None)).collect();
    set.push((0, hand.clone()));
    if !items::set_slots(lv, pos, set) {
        return true;
    }
    if hand.is_some() {
        let _ = p.set_inventory_slot(&mut *lv.host, selected as u32, None);
    }
    for s in taken {
        items::give(&mut *lv.host, player, s);
    }
    let dim = lv.dim;
    with_world(|w| w.depot_put(dim, pos, put));
    true
}

/// The player's yaw.
pub fn player_yaw(lv: &ServerLevel<'_>, p: PlayerRef) -> Option<f32> {
    p.entity().rotation(&*lv.host).map(|r| r[0])
}

/// `DepotBehaviour.tick` for depots whose stack slides, then the depots whose contents changed
/// go to the players of the level.
pub fn tick_depots(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let sliding = with_world(|w| w.dims[dim].sliding.clone()).unwrap_or_default();
    if !sliding.is_empty() {
        let standing: Vec<[i32; 3]> = sliding
            .iter()
            .copied()
            .filter(|p| {
                depot_items(lv, *p)
                    .first()
                    .is_some_and(|(slot, item, _)| *slot == 0 && upright(&*lv.host, item))
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
        if let Some((_, item, _)) = depot_items(lv, pos).into_iter().find(|s| s.0 == 0) {
            let held = depot::Held::new(random_angle(lv, pos), upright(&*lv.host, &item));
            with_world(|w| {
                if let Some(h) = w.dims[dim].depots.get_mut(&pos) {
                    *h = Some(held);
                }
                w.wake_depot(dim, pos);
            });
        }
    }
    let players = lv.players();
    send_depots(lv, &players, &changed);
    for &pos in &changed {
        super::touch_shown(lv, pos);
    }
}

/// The depots `depots` to `players`.
pub fn send_depots(lv: &mut ServerLevel<'_>, players: &[PlayerRef], depots: &[[i32; 3]]) {
    let dim = lv.dim;
    let contents: Vec<depot::Contents> = depots
        .iter()
        .map(|&pos| {
            let held = with_world(|w| w.dims[dim].depots.get(&pos).copied().flatten()).flatten();
            let stacks = if with_world(|w| w.dims[dim].depots.contains_key(&pos)) == Some(true) {
                depot_items(lv, pos)
            } else {
                Vec::new()
            };
            depot::Contents {
                pos,
                held: held.unwrap_or(depot::Held::new(0, false)),
                stacks,
            }
        })
        .collect();
    for m in depot::encode(&contents) {
        net::send_to_players(&mut *lv.host, players, depot::CHANNEL, &m);
    }
}

/// A hopper (or any handler's commit) put into or took from the depot at `pos`: its held
/// stack arrives from above (`new TransportedItemStack(stack)`, sliding in from the edge of
/// nothing) or left.
pub fn depot_contents_changed(lv: &ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some(had) = with_world(|w| w.dims[dim].depots.get(&pos).copied()).flatten() else {
        return;
    };
    let held = items::slot(lv, pos, 0);
    let now = match (had, held) {
        (_, None) => None,
        (Some(h), Some(_)) => Some(h),
        (None, Some(s)) => {
            let id = items::id_of(&*lv.host, &s);
            Some(depot::Held {
                position: 0.0,
                ..depot::Held::new(random_angle(lv, pos), upright(&*lv.host, &id))
            })
        }
    };
    with_world(|w| w.depot_put(dim, pos, now));
}
