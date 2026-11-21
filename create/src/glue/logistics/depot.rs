//! Depots on the server: the held stack sliding to the centre (`DepotBehaviour.tick`), what a
//! player puts down and takes (`SharedDepotBlockMethods.onUse`), what hoppers change, an
//! ejector's stacks merging onto what it holds (`insert`, `incoming`), and the contents clients
//! draw (`create:depot`).

use super::super::net;
use super::super::server::ServerLevel;
use super::super::world::with_world;
use super::arms::arms_touched;
use super::items::{self, random_angle, upright};
use create_engine::{Block, depot};
use mod_api::PlayerRef;

/// The stacks of the depot at `pos`, by slot: (slot, item named with its components
/// (`items::key_of`, so clients draw a potion's colour and a package's box), count).
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
                items::key_of(&*lv.host, &s),
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
    // `canAcceptItems`: an ejector only while charged (its plate down).
    if !super::arms::ejector_accepts(lv.dim, pos) {
        return true;
    }
    let Some(p) = lv.host.player(player) else {
        return true;
    };
    let Some(selected) = p.selected_slot(&*lv.host) else {
        return true;
    };
    let Some(_) = items::entity(lv, pos) else {
        // No block entity: the stack stays in hand.
        return true;
    };
    let hand = selected_stack(lv, p, selected);
    let mechanical_arm = hand
        .as_ref()
        .is_some_and(|s| items::id_of(&*lv.host, s) == "create:mechanical_arm");
    depot::use_top(
        &mut DepotUse {
            lv,
            pos,
            player: p,
            player_id: player,
            selected,
        },
        hand.is_none(),
        mechanical_arm,
    );
    // DepotBehaviour.write saves HeldItem's motion at the accepted interaction.
    super::super::persist::publish(lv);
    true
}

fn selected_stack(
    lv: &ServerLevel<'_>,
    p: PlayerRef,
    selected: usize,
) -> Option<mod_api::ItemStack> {
    p.inventory_slots(&*lv.host)?
        .into_iter()
        .find(|(i, _)| *i as usize == selected)
        .map(|(_, s)| s)
}

struct DepotUse<'a, 'b> {
    lv: &'a mut ServerLevel<'b>,
    pos: [i32; 3],
    player: PlayerRef,
    player_id: u128,
    selected: usize,
}

impl depot::PlayerUse for DepotUse<'_, '_> {
    type Stack = mod_api::ItemStack;

    fn take_contents(&mut self) -> Option<Vec<Self::Stack>> {
        let stacks = items::slots(self.lv, self.pos, depot::SLOTS as u32)?;
        if !items::set_slots(
            self.lv,
            self.pos,
            (0..depot::SLOTS as u32).map(|i| (i, None)),
        ) {
            return None;
        }
        let dim = self.lv.dim;
        with_world(|w| w.depot_put(dim, self.pos, None));
        Some(stacks.into_iter().flatten().collect())
    }

    fn give_back(&mut self, stack: Self::Stack) {
        items::give(&mut *self.lv.host, self.player_id, stack);
    }

    fn place_hand(&mut self) {
        let hand = selected_stack(self.lv, self.player, self.selected);
        let facing =
            player_yaw(self.lv, self.player).map_or(depot::Side::North, depot::Side::of_yaw);
        let put = hand.as_ref().map(|s| {
            let id = items::id_of(&*self.lv.host, s);
            depot::Held::put_by_player(
                random_angle(self.lv, self.pos),
                upright(&*self.lv.host, &id),
                facing,
            )
        });
        if !items::set_slots(self.lv, self.pos, vec![(0, hand)]) {
            return;
        }
        let _ = self
            .player
            .set_inventory_slot(&mut *self.lv.host, self.selected as u32, None);
        let dim = self.lv.dim;
        with_world(|w| w.depot_put(dim, self.pos, put));
    }
}

/// The player's yaw.
pub fn player_yaw(lv: &ServerLevel<'_>, p: PlayerRef) -> Option<f32> {
    p.entity().rotation(&*lv.host).map(|r| r[0])
}

/// `new TransportedItemStack(stack)` for a stack put onto the depot at `pos` through its item
/// handler (`DepotItemHandler.insertItem`): from above, at the edge of nothing (position 0), at
/// a random angle (180 for upright items).
pub(super) fn handler_motion(lv: &ServerLevel<'_>, pos: [i32; 3], item: &str) -> depot::Held {
    depot::Held {
        position: 0.0,
        ..depot::Held::new(random_angle(lv, pos), upright(&*lv.host, item))
    }
}

/// `DepotBehaviour.insert` on the merging depot (an ejector's) at `pos`: `count` of `item`
/// arriving with `motion`. Only while charged (`canAcceptItems`); as much as
/// `getRemainingSpace` leaves (`depot::merge_take`, the stacks still sliding in counted). With
/// nothing held it is the held stack; else it slides in on its own (`incoming.add`) and joins
/// at the centre. The count left.
pub(super) fn merge_insert(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    item: &str,
    count: u8,
    motion: depot::Held,
    simulate: bool,
) -> u8 {
    let dim = lv.dim;
    let Some(max_stack) = with_world(|w| {
        w.dims[dim]
            .ejectors
            .get(&pos)
            .filter(|e| e.state == create_engine::ejector::State::Charged)
            .map(|e| e.max_stack)
    })
    .flatten() else {
        return count;
    };
    let stacks = depot_items(lv, pos);
    let held = stacks
        .iter()
        .find(|s| s.0 == 0)
        .map(|(_, key, n)| depot::HeldStack {
            same: key == item,
            count: *n,
            max: items::stack_limit(&*lv.host, key),
        });
    let incoming: u32 = with_world(|w| {
        w.depot_incoming(dim, pos)
            .iter()
            .map(|i| u32::from(i.count))
            .sum()
    })
    .unwrap_or(0);
    let used = stacks.iter().map(|s| u32::from(s.2)).sum::<u32>() + incoming;
    let n = depot::merge_take(max_stack, held, used, count);
    if n == 0 || simulate {
        return count - n;
    }
    if held.is_some() {
        let i = depot::Incoming {
            item: item.to_owned(),
            count: n,
            motion,
        };
        with_world(|w| w.depot_add_incoming(dim, pos, i));
    } else {
        let stack = items::stack_of(&*lv.host, item, u32::from(n));
        if stack.is_none() || !items::set_slot(lv, pos, 0, stack) {
            return count;
        }
        with_world(|w| w.depot_put(dim, pos, Some(motion)));
    }
    count - n
}

/// `DepotBehaviour.tick`'s incoming stacks: each slides, and one at the centre becomes the
/// held stack (none was held) or joins it; one that cannot stack with it
/// (`canItemStackAmountsStack`) drops at the centre, half a block up
/// (`Containers.dropItemStack`).
fn land_incoming(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let arrivals = {
        let host = &*lv.host;
        with_world(|w| w.tick_incoming(dim, pos, |item| upright(host, item))).unwrap_or_default()
    };
    for a in arrivals {
        match a {
            depot::Arrival::Held { item, count } => {
                let stack = items::stack_of(&*lv.host, &item, u32::from(count));
                items::set_slot(lv, pos, 0, stack);
            }
            depot::Arrival::Onto { item, count } => {
                let held = items::slot(lv, pos, 0);
                let joins = held.as_ref().is_some_and(|h| {
                    let key = items::key_of(&*lv.host, h);
                    depot::joins(
                        depot::HeldStack {
                            same: key == item,
                            count: h.count().min(255) as u8,
                            max: items::stack_limit(&*lv.host, &key),
                        },
                        count,
                    )
                });
                match held {
                    Some(h) if joins => {
                        let grown = items::with_count(&h, h.count() + u32::from(count));
                        items::set_slot(lv, pos, 0, grown);
                    }
                    _ => {
                        let at = [
                            f64::from(pos[0]) + 0.5,
                            f64::from(pos[1] + 1),
                            f64::from(pos[2]) + 0.5,
                        ];
                        lv.spawn_items(at, &item, u32::from(count));
                    }
                }
            }
        }
    }
}

/// One depot's block-entity turn in the shared transport walk: its incoming stacks first, then
/// the held one.
pub(super) fn tick_depot(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    land_incoming(lv, pos);
    let standing = depot_items(lv, pos)
        .first()
        .is_some_and(|(slot, item, _)| *slot == 0 && upright(&*lv.host, item));
    let arrived = with_world(|w| w.tick_depot(lv.dim, pos, standing)) == Some(true);
    // `handleBeltFunnelOutput` before the processing above.
    if !super::funnel::depot_output(lv, pos) && arrived {
        process_held(lv, pos);
    }
}

/// `DepotBehaviour.tick`'s `BeltProcessingBehaviour` two above an arrived stack: none, or an
/// unlocked stack with the block over the depot in the way (`isBlocked`), and nothing happens;
/// else the spout's or press's callbacks run now. A deployer's run with the deployers
/// (`processing::Tools`).
fn process_held(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let dim = lv.dim;
    let Some((block, _)) = super::info_at(lv, [pos[0], pos[1] + 2, pos[2]]) else {
        return;
    };
    if !create_engine::belt::processes_below(block) {
        return;
    }
    let locked = with_world(|w| matches!(w.dims[dim].depots.get(&pos), Some(Some(h)) if h.locked));
    if locked != Some(true) && lv.processing_blocked(pos) {
        return;
    }
    match block {
        Block::Spout => crate::glue::fluids::machines::spout_on_depot(lv, pos),
        Block::MechanicalPress => crate::glue::processing::press_depot_turn(lv, pos),
        _ => {}
    }
}

/// Publish client notifications after the shared block-entity walk.
pub fn send_changed_depots(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    // `DepotBehaviour.destroy`: a removed depot's incoming stacks drop where it was.
    let drops = with_world(|w| std::mem::take(&mut w.dims[dim].depot_drops)).unwrap_or_default();
    for (pos, item, count) in drops {
        if let Some(stack) = items::stack_of(&*lv.host, &item, u32::from(count)) {
            let _ = lv.level.pop_resource(&mut *lv.host, pos, stack);
        }
    }
    let changed = with_world(|w| w.take_depot_changes(dim)).unwrap_or_default();
    if changed.is_empty() {
        return;
    }
    // Arms using these depots look again.
    arms_touched(dim, &changed, &[]);
    // A depot that joined holding a stack (a saved world, a block placed with its items) has
    // it at rest only for legacy saves without transport state.
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
    // Ejectors are drawn every frame; a depot's kept drawing is redrawn.
    for &pos in &changed {
        if super::info_at(lv, pos).is_some_and(|(b, _)| b == create_engine::Block::Depot) {
            show_contents(lv, pos);
        }
    }
}

pub(super) fn save_depots(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    // Persist motion even on ticks that need no client notification. Empty records clear
    // the old HeldItem when the host inventory has been emptied.
    let records = with_world(|w| {
        w.dims[dim]
            .depots
            .keys()
            .filter_map(|&p| w.saved_depot(dim, p).map(|s| (p, s)))
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    for (pos, saved) in records {
        lv.write_entity_field(pos, "DepotState", saved_to_nbt(saved));
    }
}

/// The depots `depots` to `players`.
pub fn send_depots(lv: &mut ServerLevel<'_>, players: &[PlayerRef], depots: &[[i32; 3]]) {
    let contents: Vec<_> = depots.iter().map(|&pos| contents(lv, pos)).collect();
    for m in depot::encode(&contents) {
        net::send_to_players(&mut *lv.host, players, depot::CHANNEL, &m);
    }
}

/// The transport stack and output buffer sent by DepotBehaviour.write's client packet.
fn contents(lv: &ServerLevel<'_>, pos: [i32; 3]) -> depot::Contents {
    let held = with_world(|w| w.dims[lv.dim].depots.get(&pos).copied().flatten()).flatten();
    let stacks = if with_world(|w| w.dims[lv.dim].depots.contains_key(&pos)) == Some(true) {
        depot_items(lv, pos)
    } else {
        Vec::new()
    };
    depot::Contents {
        pos,
        held: held.unwrap_or(depot::Held::new(0, false)),
        stacks,
    }
}

/// DepotBehaviour.write's client contents, also used for kept-drawing invalidation.
fn drawn_contents(contents: &depot::Contents) -> mod_api::Tag {
    super::super::drawn::snapshot(depot::encode(std::slice::from_ref(contents)))
}

fn show_contents(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    super::super::drawn::publish(lv, pos, drawn_contents(&contents(lv, pos)));
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

/// `TransportedItemStack.serializeNBT/read` for the engine's depot fields. The host
/// container separately persists Item and OutputBuffer, including item components; the
/// incoming stacks (`Incoming`, a merging depot's) live in the engine, so each keeps its item
/// as the engine names it (`Item`, `Count`).
pub(crate) fn saved_to_nbt(saved: depot::SavedDepot) -> mod_api::Compound {
    use mod_api::{Compound, Tag};
    let mut out = Compound::new();
    if let Some(h) = saved.held {
        out.insert("HeldItem".into(), Tag::Compound(motion_to_nbt(h)));
    }
    if !saved.incoming.is_empty() {
        let list = saved
            .incoming
            .into_iter()
            .map(|i| {
                let mut c = motion_to_nbt(i.motion);
                c.insert("Item".into(), Tag::String(i.item.as_str().into()));
                c.insert("Count".into(), Tag::Int(i32::from(i.count)));
                Tag::Compound(c)
            })
            .collect();
        out.insert("Incoming".into(), Tag::List(list));
    }
    out
}

fn motion_to_nbt(h: depot::Held) -> mod_api::Compound {
    use mod_api::{Compound, Tag};
    let mut held = Compound::from([
        ("Pos".into(), Tag::Float(h.position.to_bits())),
        ("Offset".into(), Tag::Float(h.side_offset.to_bits())),
        ("Angle".into(), Tag::Int(h.angle)),
        (
            "InDirection".into(),
            Tag::Int(h.from.map_or(-1, |s| i32::from(s.index()))),
        ),
        ("Locked".into(), Tag::Byte(i8::from(h.locked))),
    ]);
    if let Some(by) = h.fan.by {
        use create_engine::fan::Kind;
        let id = match by {
            Kind::Splashing => "create:splashing",
            Kind::Haunting => "create:haunting",
            Kind::Smoking => "create:smoking",
            Kind::Blasting => "create:blasting",
        };
        held.insert("FanProcessingType".into(), Tag::String(id.into()));
        held.insert("FanProcessingTime".into(), Tag::Int(h.fan.time));
    }
    held
}

pub(crate) fn saved_from_nbt(data: &mod_api::Compound) -> depot::SavedDepot {
    use mod_api::Tag;
    let held = match data.get(&"HeldItem".into()) {
        Some(Tag::Compound(h)) => Some(motion_from_nbt(h)),
        _ => None,
    };
    let incoming = match data.get(&"Incoming".into()) {
        Some(Tag::List(list)) => list
            .iter()
            .filter_map(|t| {
                let Tag::Compound(c) = t else { return None };
                let Some(Tag::String(item)) = c.get(&"Item".into()) else {
                    return None;
                };
                let Some(Tag::Int(count)) = c.get(&"Count".into()) else {
                    return None;
                };
                let count = u8::try_from(*count).ok().filter(|n| *n > 0)?;
                Some(depot::Incoming {
                    item: item.to_string_lossy(),
                    count,
                    motion: motion_from_nbt(c),
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    depot::SavedDepot { held, incoming }
}

fn motion_from_nbt(h: &mod_api::Compound) -> depot::Held {
    use mod_api::Tag;
    let int = |k: &str| match h.get(&k.into()) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    };
    let float = |k: &str| match h.get(&k.into()) {
        Some(Tag::Float(v)) => f32::from_bits(*v),
        _ => 0.0,
    };
    depot::Held {
        position: float("Pos"),
        side_offset: float("Offset"),
        angle: int("Angle"),
        from: u8::try_from(int("InDirection"))
            .ok()
            .and_then(depot::Side::from_index),
        locked: matches!(h.get(&"Locked".into()), Some(Tag::Byte(v)) if *v != 0),
        fan: create_engine::fan::Progress {
            by: match h.get(&"FanProcessingType".into()) {
                Some(Tag::String(id)) => {
                    use create_engine::fan::Kind;
                    match id.to_string_lossy().as_str() {
                        "create:splashing" => Some(Kind::Splashing),
                        "create:haunting" => Some(Kind::Haunting),
                        "create:smoking" => Some(Kind::Smoking),
                        "create:blasting" => Some(Kind::Blasting),
                        _ => None,
                    }
                }
                _ => None,
            },
            time: int("FanProcessingTime"),
        },
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;

    #[test]
    fn settled_depot_contents_notifications_converge() {
        use mod_api::Tag;
        // t42's settled two-diamond depot, including the old counter on disk.
        let mut contents = depot::Contents {
            pos: [1, 56, 6],
            held: depot::Held::new(172, false),
            stacks: vec![(0, "minecraft:diamond".into(), 2)],
        };
        contents.held.position = 0.49812075;
        contents.held.side_offset = -0.014814815;
        contents.held.from = Some(depot::Side::North);
        let mut shown = Tag::Int(9914);
        let mut writes = 0;
        for _ in 0..2000 {
            if let Some(update) =
                super::super::super::drawn::update(Some(&shown), drawn_contents(&contents))
            {
                shown = update;
                writes += 1;
            }
        }
        assert_eq!(
            writes, 1,
            "a data-write notification must not perpetuate itself"
        );
        // Count changes, item replacements and removal still invalidate the drawing.
        contents.stacks[0].2 = 3;
        shown = super::super::super::drawn::update(Some(&shown), drawn_contents(&contents))
            .expect("count changed");
        contents.stacks[0].1 = "minecraft:emerald".into();
        shown = super::super::super::drawn::update(Some(&shown), drawn_contents(&contents))
            .expect("item changed");
        contents.stacks.clear();
        assert!(
            super::super::super::drawn::update(Some(&shown), drawn_contents(&contents)).is_some()
        );
    }

    #[test]
    fn depot_saved_transport_round_trips() {
        let mut held = depot::Held::put_by_player(73, false, depot::Side::West);
        held.position = 0.48592162;
        held.locked = true;
        held.fan = create_engine::fan::Progress {
            by: Some(create_engine::fan::Kind::Haunting),
            time: 37,
        };
        for held in [Some(held), None] {
            let saved = depot::SavedDepot {
                held,
                incoming: Vec::new(),
            };
            assert_eq!(saved_from_nbt(&saved_to_nbt(saved.clone())), saved);
        }
        let mut above = held;
        above.from = None;
        above.fan = Default::default();
        let saved = depot::SavedDepot {
            held: Some(above),
            incoming: Vec::new(),
        };
        assert_eq!(saved_from_nbt(&saved_to_nbt(saved.clone())), saved);
        // An ejector's stacks still sliding in (`Incoming`), with or without a held one.
        for held in [Some(above), None] {
            let saved = depot::SavedDepot {
                held,
                incoming: vec![
                    depot::Incoming {
                        item: "minecraft:iron_ingot".into(),
                        count: 16,
                        motion: depot::Held {
                            position: 0.125,
                            ..depot::Held::new(37, false)
                        },
                    },
                    depot::Incoming {
                        item: "minecraft:diamond".into(),
                        count: 2,
                        motion: held.unwrap_or(above),
                    },
                ],
            };
            assert_eq!(saved_from_nbt(&saved_to_nbt(saved.clone())), saved);
        }
    }
}
