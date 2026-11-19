//! `BeltSlicer`: a wrench shortens a belt from an end or splits it in two, a belt connector
//! lengthens it at an end or joins it to another belt there. The belt is taken apart first
//! (`World::take_belt`), its segments' states are changed, and its items go to the belts its
//! segments form again after the tick (`World::reform_belt`).

use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::super::world::with_world;
use super::belt::{belt_color, belt_segment, eject_item};
use super::items;
use create_engine::Block;
use create_engine::belt::{Inventory, Item, Part, Segment};
use create_engine::belt_connector::MAX_LENGTH;
use create_engine::kinetics::{Axis, Dir};
use mod_api::{HostBlockStates, PlayerRef, SoundSource, StateConstraint};

fn add(p: [i32; 3], v: [i32; 3]) -> [i32; 3] {
    [p[0] + v[0], p[1] + v[1], p[2] + v[2]]
}

fn sub(p: [i32; 3], v: [i32; 3]) -> [i32; 3] {
    [p[0] - v[0], p[1] - v[1], p[2] - v[2]]
}

/// A boolean property of the block at `pos`.
fn flag(lv: &ServerLevel<'_>, pos: [i32; 3], key: &str) -> bool {
    lv.block_state(pos)
        .is_some_and(|s| s.property(&lv.host.block_states(), key).ok().flatten() == Some("true"))
}

/// Sets the belt segment at `pos` to `seg`, cased or not, keeping its water.
fn segment_state(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    seg: Segment,
    casing: bool,
) -> Option<mod_api::BlockState> {
    let b = |v: bool| if v { "true" } else { "false" };
    let water = flag(lv, pos, "waterlogged");
    let facing = super::super::kinetics::dir_name(seg.facing);
    let props = [
        ("facing", facing),
        ("slope", seg.slope.name()),
        ("part", seg.part.name()),
        ("casing", b(casing)),
        ("waterlogged", b(water)),
    ];
    let constraints: Vec<StateConstraint> = props
        .iter()
        .map(|(k, v)| StateConstraint::new(*k, *v))
        .collect();
    let state = {
        let facts = lv.host.block_states();
        mod_api::BlockState::of(&facts, states::key(Block::Belt), &constraints)
    };
    state.ok()
}

/// `BeltSlicer` changes existing segments through `switchToBlockState`.
fn set_segment(lv: &mut ServerLevel<'_>, pos: [i32; 3], seg: Segment, casing: bool) {
    if let Some(state) = segment_state(lv, pos, seg, casing) {
        crate::glue::items::wrench::switch_state(lv, pos, state);
    }
}

/// Empties the cell at `pos` (a belt segment the slicer takes away).
fn set_air(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let state = {
        let facts = lv.host.block_states();
        lv.host
            .block_key("minecraft:air")
            .and_then(|air| mod_api::BlockState::of(&facts, air, &[]).ok())
    };
    if let Some(state) = state {
        lv.set_state(pos, state, update_all());
    }
}

fn sound(lv: &mut ServerLevel<'_>, pos: [i32; 3], sound: &str, volume: f32, pitch: f32) {
    let at = pos.map(|v| f64::from(v) + 0.5);
    lv.level.play_sound(
        &mut *lv.host,
        sound,
        at,
        SoundSource::Players,
        volume,
        pitch,
        None,
    );
}

/// The segment state of the belt block at `pos`.
fn segment_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<Segment> {
    super::info_at(lv, pos).and_then(|(b, i)| (b == Block::Belt).then_some(i.belt).flatten())
}

/// Takes one belt connector and `shafts` shafts from a survival player's inventory
/// (`useWrench`'s search), all or nothing.
fn take_split_cost(lv: &mut ServerLevel<'_>, player: PlayerRef, shafts: u32) -> bool {
    let slots: Vec<(u32, mod_api::ItemStack, String)> = player
        .inventory_slots(&*lv.host)
        .unwrap_or_default()
        .into_iter()
        .map(|(i, s)| {
            let id = items::id_of(&*lv.host, &s);
            (i, s, id)
        })
        .collect();
    let connector = slots
        .iter()
        .find(|(_, _, id)| id == "create:belt_connector")
        .map(|(i, s, _)| (*i, s.clone()));
    let have: u32 = slots
        .iter()
        .filter(|(_, _, id)| id == "create:shaft")
        .map(|(_, s, _)| s.count())
        .sum();
    let Some((ci, cs)) = connector else {
        return false;
    };
    if have < shafts {
        return false;
    }
    let _ = player.set_inventory_slot(&mut *lv.host, ci, cs.shrink(1));
    let mut left = shafts;
    for (i, s, id) in slots {
        if left == 0 {
            break;
        }
        if id != "create:shaft" {
            continue;
        }
        let n = left.min(s.count());
        left -= n;
        let _ = player.set_inventory_slot(&mut *lv.host, i, s.shrink(n));
    }
    true
}

/// `BeltSlicer.useWrench` on the belt segment at `pos`, clicked on `face` at `hit` (from the
/// block's centre). None passes the click on to the wrench (`onWrenched`: a casing or pulley
/// comes off); Some when the slicer answered it (true: the belt changed).
pub fn use_wrench(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: PlayerRef,
    face: Dir,
    hit: [f64; 3],
) -> Option<bool> {
    let (_, index, seg) = belt_segment(lv, pos)?;
    if flag(lv, pos, "casing") && face != Dir::Up {
        return None;
    }
    if seg.part == Part::Pulley && face.axis() != Axis::Y {
        return None;
    }
    let dim = lv.dim;
    let length = with_world(|w| {
        let d = &w.dims[dim];
        d.belts
            .get(d.belt_of.get(&pos)?)
            .map(|b| b.chain.len() as i32)
    })
    .flatten()?;
    if length == 2 {
        return Some(false);
    }
    let creative = crate::glue::items::creative(&*lv.host, player);
    let v = seg.vector();

    // Shorten from an end.
    if seg.hovering_end(hit) {
        let clicked = lv.block_state(pos);
        // `BeltSlicer.java:86-93`: `detachKinetics`, `invalidateItemHandler` and
        // `beltLength = 0` on every segment of the belt; the belt is gone until its segments
        // form a new chain at their own tick.
        let (_, mut taken) = with_world(|w| w.take_belt(dim, pos)).flatten()?;
        let next = if seg.part == Part::End {
            sub(pos, v)
        } else {
            add(pos, v)
        };
        let replaced = segment_at(lv, next);
        let next_cased = flag(lv, next, "casing");
        set_segment(lv, next, seg, next_cased);
        with_world(|w| w.remove_belt_quietly(dim, pos));
        // `BeltSlicer.java:97` sets the clicked segment to air with `UPDATE_MOVE_BY_PISTON`,
        // which still runs `BeltBlockEntity.destroy`. Shortening from the START clicks the
        // controller, so `getInventory().ejectAll()` takes every item off the belt before the
        // transfer below, which then finds nothing to hand on.
        if seg.part == Part::Start {
            with_world(|w| w.destroy_belt(dim, &mut taken));
        }
        set_air(lv, pos);
        // `levelEvent(PARTICLES_DESTROY_BLOCK, pos, Block.getId(state))`.
        if let Some(state) = clicked {
            lv.destroy_effect(pos, state);
        }
        if !creative
            && replaced.is_some_and(|r| r.part == Part::Pulley)
            && let Some(shaft) = items::stack_of(&*lv.host, "create:shaft", 1)
        {
            crate::glue::items::give_back(&mut *lv.host, player, shaft);
        }
        let chain: Vec<[i32; 3]> = taken.chain.iter().copied().filter(|p| *p != pos).collect();
        let controller = chain[0];
        if seg.part == Part::End {
            // `BeltSlicer.java:106-113`: eject the items past the new end. The inventory keeps
            // the controller that stays, and its direction flag with it.
            let (gone, kept): (Vec<Item>, Vec<Item>) = std::mem::take(&mut taken.items.items)
                .into_iter()
                .partition(|i| i.position > (length - 1) as f32);
            taken.items.items = kept;
            for i in gone {
                // `BeltInventory.eject` reads the zeroed `beltLength` at this point.
                eject_item(lv, controller, &taken.segment, 0, 0.0, &i);
            }
            with_world(|w| w.reform_belt(dim, controller, &chain, taken.items));
        } else {
            // `BeltSlicer.java:116-132`: `controllerBE.inventory = null; segmentBE.inventory =
            // null; segmentBE.setController(next)`. The next segment is the controller now and
            // gets a fresh `BeltInventory` (`beltMovementPositive` false); the emptied one has
            // no item left to move.
            with_world(|w| w.reform_belt(dim, controller, &chain, Inventory::fresh(Vec::new())));
        }
        return Some(true);
    }

    // Split in two.
    let toward_positive: f64 = (0..3).map(|i| hit[i] * f64::from(v[i])).sum();
    let toward_positive = toward_positive > 0.0;
    let next = if toward_positive {
        add(pos, v)
    } else {
        sub(pos, v)
    };
    if index == 0 || index == 1 && !toward_positive {
        return Some(false);
    }
    if index == length - 1 || index == length - 2 && toward_positive {
        return Some(false);
    }
    let next_seg = segment_at(lv, next)?;
    if !creative {
        let mut shafts = 0;
        if !seg.has_pulley() {
            shafts += 1;
        }
        if next_seg.part == Part::Middle {
            shafts += 1;
        }
        if !take_split_cost(lv, player, shafts) {
            return Some(false);
        }
    }
    let (controller, taken) = with_world(|w| w.take_belt(dim, pos)).flatten()?;
    let (here, there) = if toward_positive {
        (Part::End, Part::Start)
    } else {
        (Part::Start, Part::End)
    };
    let here_cased = flag(lv, pos, "casing");
    let there_cased = flag(lv, next, "casing");
    set_segment(lv, pos, Segment { part: here, ..seg }, here_cased);
    set_segment(
        lv,
        next,
        Segment {
            part: there,
            ..next_seg
        },
        there_cased,
    );
    sound(lv, pos, "minecraft:block.wool.hit", 0.5, 2.3);
    let new_controller = if toward_positive { next } else { pos };
    let cut = index + i32::from(toward_positive);
    let cut = (cut.max(0) as usize).min(taken.chain.len());
    let (first, second) = taken.chain.split_at(cut);
    // `BeltSlicer.java:222-236`: the items past the cut leave the old controller's inventory
    // for `newController`, whose inventory the slicer nulls — a fresh `BeltInventory`
    // (`beltMovementPositive` false) that the items are added to. The first half keeps the old
    // inventory, and its direction flag.
    let mut kept = taken.items;
    let shift = index as f32 + if toward_positive { 1.0 } else { 0.0 };
    let (moved, rest): (Vec<Item>, Vec<Item>) = std::mem::take(&mut kept.items)
        .into_iter()
        .partition(|i| i.position - shift > 0.0);
    kept.items = rest;
    let moved = moved
        .into_iter()
        .map(|mut i| {
            i.position -= shift;
            i
        })
        .collect();
    with_world(|w| {
        w.reform_belt(dim, controller, first, kept);
        w.reform_belt(dim, new_controller, second, Inventory::fresh(moved));
    });
    Some(true)
}

/// `BeltSlicer.useConnector` on the belt segment at `pos` clicked at `hit` (from the block's
/// centre): None when it is no click on the belt's end (the connector is used as an item);
/// Some when the slicer answered it (true: the belt changed).
pub fn use_connector(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: PlayerRef,
    hit: [f64; 3],
) -> Option<bool> {
    let (_, _, seg) = belt_segment(lv, pos)?;
    let dim = lv.dim;
    let length = with_world(|w| {
        let d = &w.dims[dim];
        d.belts
            .get(d.belt_of.get(&pos)?)
            .map(|b| b.chain.len() as i32)
    })
    .flatten()?;
    if length == MAX_LENGTH {
        return Some(false);
    }
    if !seg.hovering_end(hit) {
        return None;
    }
    let creative = crate::glue::items::creative(&*lv.host, player);
    let v = seg.vector();
    let next = if seg.part == Part::Start {
        sub(pos, v)
    } else {
        add(pos, v)
    };
    let color = belt_color(lv, pos);
    let next_state = lv.block_state(next);
    let replaceable =
        next_state.is_none_or(|s| lv.host.block_states().replaceable(s) != Some(false));
    if !replaceable {
        // Join the belt there.
        let other = segment_at(lv, next)?;
        if !seg.compatible(&other) {
            return Some(false);
        }
        let other_length = with_world(|w| {
            let d = &w.dims[dim];
            d.belts
                .get(d.belt_of.get(&next)?)
                .map(|b| b.chain.len() as i32)
        })
        .flatten();
        let Some(other_length) = other_length else {
            return Some(false);
        };
        if other_length + length > MAX_LENGTH {
            return Some(false);
        }
        let (_, mine) = with_world(|w| w.take_belt(dim, pos)).flatten()?;
        let (_, theirs) = with_world(|w| w.take_belt(dim, next)).flatten()?;
        // Their belt turns to face the same way when it faced the other.
        let flip = seg.facing != other.facing;
        let mut their_items = theirs.items.to_save();
        for p in &theirs.chain {
            if flip && let Some(s) = segment_at(lv, *p) {
                let cased = flag(lv, *p, "casing");
                set_segment_direct(lv, *p, s.flipped(), cased);
            }
        }
        if flip {
            for i in &mut their_items.items {
                i.position = other_length as f32 - i.position;
            }
        }
        let mut their_chain = theirs.chain.clone();
        if flip {
            their_chain.reverse();
        }
        let cased = flag(lv, pos, "casing");
        set_segment(
            lv,
            pos,
            Segment {
                part: Part::Middle,
                ..seg
            },
            cased,
        );
        let next_cased = flag(lv, next, "casing");
        set_segment(
            lv,
            next,
            Segment {
                part: Part::Middle,
                ..seg
            },
            next_cased,
        );
        sound(lv, pos, "minecraft:block.wool.hit", 0.5, 1.3);
        if !creative {
            for (id, n) in [("create:shaft", 2), ("create:belt_connector", 1)] {
                if let Some(s) = items::stack_of(&*lv.host, id, n) {
                    crate::glue::items::give_back(&mut *lv.host, player, s);
                }
            }
        }
        // The joined belt's start is the controller: the items of the belt after it move on
        // by the length of the one before.
        let my_items = mine.items.to_save();
        let (chain, mut all) = if seg.part == Part::End {
            let mut chain = mine.chain.clone();
            chain.extend(their_chain);
            let mut all = my_items.items;
            all.extend(their_items.items.into_iter().map(|mut i| {
                i.position += length as f32;
                i
            }));
            (chain, all)
        } else {
            let mut chain = their_chain;
            chain.extend(mine.chain.iter().copied());
            let mut all = their_items.items;
            all.extend(my_items.items.into_iter().map(|mut i| {
                i.position += other_length as f32;
                i
            }));
            (chain, all)
        };
        all.sort_by(|a, b| b.position.total_cmp(&a.position));
        for p in &chain {
            set_color(lv, *p, color.as_deref());
        }
        let controller = chain[0];
        with_world(|w| w.reform_belt(dim, controller, &chain, Inventory::from_saved(all, true)));
        return Some(true);
    }
    // Lengthen it at this end.
    let (_, mut taken) = with_world(|w| w.take_belt(dim, pos)).flatten()?;
    let cased = flag(lv, pos, "casing");
    set_segment(
        lv,
        pos,
        Segment {
            part: Part::Middle,
            ..seg
        },
        cased,
    );
    set_segment_new(lv, next, seg);
    set_color(lv, next, color.as_deref());
    sound(lv, pos, "minecraft:block.wool.place", 0.5, 1.0);
    let mut chain = taken.chain.clone();
    if seg.part == Part::Start {
        // `BeltSlicer.java:352-361`: the placed segment is the controller now; its `getInventory`
        // is a fresh `BeltInventory` (`beltMovementPositive` false) and the items move on by one
        // segment.
        chain.insert(0, next);
        let items = std::mem::take(&mut taken.items.items);
        let mut items = items;
        for i in &mut items {
            i.position += 1.0;
        }
        with_world(|w| w.reform_belt(dim, next, &chain, Inventory::fresh(items)));
    } else {
        // The controller stays, and so does its inventory and direction flag.
        chain.push(next);
        let controller = chain[0];
        with_world(|w| w.reform_belt(dim, controller, &chain, taken.items));
    }
    Some(true)
}

/// A new segment `seg` at `pos`, uncased (`state.setValue(CASING, false)` placed there).
fn set_segment_new(lv: &mut ServerLevel<'_>, pos: [i32; 3], seg: Segment) {
    set_segment_direct(lv, pos, seg, false);
}

/// `BeltSlicer` uses ordinary `setBlock` for flipped or newly placed segments.
fn set_segment_direct(lv: &mut ServerLevel<'_>, pos: [i32; 3], seg: Segment, casing: bool) {
    if let Some(state) = segment_state(lv, pos, seg, casing) {
        lv.set_state(pos, state, update_all());
    }
}

/// The segment at `pos` takes dye `color`.
fn set_color(lv: &mut ServerLevel<'_>, pos: [i32; 3], color: Option<&str>) {
    if let Some(e) = lv.host.block_entity(lv.level, pos) {
        let tag = mod_api::Tag::String(color.unwrap_or("").into());
        let _ = e.merge_data(
            &mut *lv.host,
            mod_api::Compound::from([(super::BELT_COLOR.into(), tag)]),
        );
    }
}
