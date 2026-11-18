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

/// One depot's block-entity turn in the shared transport walk.
pub(super) fn tick_depot(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let standing = depot_items(lv, pos)
        .first()
        .is_some_and(|(slot, item, _)| *slot == 0 && upright(&*lv.host, item));
    if with_world(|w| w.tick_depot(lv.dim, pos, standing)) == Some(true) {
        crate::glue::fluids::machines::spout_on_depot(lv, pos);
    }
}

/// Publish client notifications after the shared block-entity walk.
pub fn send_changed_depots(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let changed = with_world(|w| w.take_depot_changes(dim)).unwrap_or_default();
    if changed.is_empty() {
        return;
    }
    // Arms using these depots and ejectors holding a new stack look again.
    arms_touched(dim, &changed, &[]);
    wake_ejectors(dim, &changed);
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
/// container separately persists Item and OutputBuffer, including item components.
pub(crate) fn saved_to_nbt(saved: depot::SavedDepot) -> mod_api::Compound {
    use mod_api::{Compound, Tag};
    let Some(h) = saved.held else {
        return Compound::new();
    };
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
    Compound::from([("HeldItem".into(), Tag::Compound(held))])
}

pub(crate) fn saved_from_nbt(data: &mod_api::Compound) -> depot::SavedDepot {
    use mod_api::Tag;
    let Some(Tag::Compound(h)) = data.get(&"HeldItem".into()) else {
        return depot::SavedDepot { held: None };
    };
    let int = |k: &str| match h.get(&k.into()) {
        Some(Tag::Int(v)) => *v,
        _ => 0,
    };
    let float = |k: &str| match h.get(&k.into()) {
        Some(Tag::Float(v)) => f32::from_bits(*v),
        _ => 0.0,
    };
    depot::SavedDepot {
        held: Some(depot::Held {
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
        }),
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
            let saved = depot::SavedDepot { held };
            assert_eq!(saved_from_nbt(&saved_to_nbt(saved)), saved);
        }
        let mut above = held;
        above.from = None;
        above.fan = Default::default();
        let saved = depot::SavedDepot { held: Some(above) };
        assert_eq!(saved_from_nbt(&saved_to_nbt(saved)), saved);
    }
}
