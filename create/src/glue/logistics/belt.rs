//! Belts on the server: their items moving (`BeltInventory.tick`) with what lies past their
//! end, the funnels and tunnels over them and the machines above (`BeltSide`); items put on
//! from beside, above or another belt (`DirectBeltInputBehaviour.handleInsertion`); what is
//! thrown off the end; the items clients draw (`create:belt`).

use super::super::net;
use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::super::world::with_world;
use super::arms::{arms_touched, ejector_accepts};
use super::depot::depot_items;
use super::funnel::funnel_at;
use super::items::{self, face};
use super::tunnel::tunnel_state;
use create_engine::belt;
use create_engine::kinetics::{Axis, Dir};
use mod_api::{BlockFace, HostBlockStates, ItemStack, PlayerRef, StateConstraint, SupportType};

/// The belt segment at `pos`: its controller, its index along the chain, and its state.
pub fn belt_segment(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<([i32; 3], i32, belt::Segment)> {
    let (_, info) = super::info_at(lv, pos)?;
    let seg = info.belt?;
    let dim = lv.dim;
    let (c, index) = with_world(|w| {
        let d = &w.dims[dim];
        let c = *d.belt_of.get(&pos)?;
        let index = d.belts.get(&c)?.chain.iter().position(|p| *p == pos)? as i32;
        Some((c, index))
    })
    .flatten()?;
    Some((c, index, seg))
}

/// `BeltBlock.useItemOn` for a player who may build and is not sneaking, clicked on `hit` (the
/// face and the point): a wrench or a belt connector slices the belt (`slicer`), a dye or water
/// (a bucket or a bottle, `held` naming a potion with its contents) colours or washes it, an
/// empty main hand takes the segment's items (`use_belt`), a shaft makes a middle segment a
/// pulley (used up outside creative), a casing cases the segment (`setCasingType`, nothing used
/// up). True when the click was the belt's.
pub fn use_belt_with(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: u128,
    hand: mod_api::Hand,
    held: Option<&str>,
    hit: (Dir, [f64; 3]),
) -> bool {
    let Some(p) = lv.host.player(player) else {
        return false;
    };
    if crate::glue::items::sneaking(&*lv.host, p) || p.may_build(&*lv.host) != Some(true) {
        return false;
    }
    // `BeltSlicer`: from the clicked point, measured from the block's centre.
    let local = [0, 1, 2].map(|i| hit.1[i] - f64::from(pos[i]) - 0.5);
    match held {
        Some("create:wrench") => {
            return super::slicer::use_wrench(lv, pos, p, hit.0, local).is_some();
        }
        Some("create:belt_connector") => {
            return super::slicer::use_connector(lv, pos, p, local).is_some();
        }
        _ => {}
    }
    // A dye colours the whole belt, water washes it (`applyColor`); neither is used up.
    if let Some(h) = held {
        let dye = create_engine::dye::DyeColor::of_item(h);
        if dye.is_some() || crate::glue::fluids::empties_water(h) {
            return apply_color(lv, pos, dye.map(|d| d.name()));
        }
    }
    match held {
        None => hand == mod_api::Hand::Main && use_belt(lv, pos, player),
        Some("create:shaft") => {
            let Some((_, _, seg)) = belt_segment(lv, pos) else {
                return false;
            };
            if seg.part != belt::Part::Middle {
                return false;
            }
            if !crate::glue::items::creative(&*lv.host, p) {
                crate::glue::items::shrink_held(&mut *lv.host, p, hand, 1);
            }
            crate::glue::items::wrench::switch_property(lv, pos, "part", "pulley");
            true
        }
        Some(c @ ("create:andesite_casing" | "create:brass_casing")) => {
            let kind = if c == "create:brass_casing" {
                "brass"
            } else {
                "andesite"
            };
            set_casing(lv, pos, Some(kind));
            // The casing's place sound (wood), at 0.8 of its pitch.
            let at = pos.map(|v| f64::from(v) + 0.5);
            lv.level.play_sound(
                &mut *lv.host,
                "minecraft:block.wood.place",
                at,
                mod_api::SoundSource::Blocks,
                1.0,
                0.8,
            );
            true
        }
        _ => false,
    }
}

/// The dye of the belt segment at `pos` (`BeltBlockEntity.color`), None when undyed.
pub fn belt_color(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<String> {
    let e = lv.host.block_entity(lv.level, pos)?;
    match e.data(&*lv.host)?.get(&super::BELT_COLOR.into()) {
        Some(mod_api::Tag::String(c)) => {
            Some(c.to_string_lossy().to_string()).filter(|c| !c.is_empty())
        }
        _ => None,
    }
}

/// Dyes the belt segment at `pos` `color` (None: undyed), for its clients to draw.
fn set_segment_color(lv: &mut ServerLevel<'_>, pos: [i32; 3], color: Option<&str>) {
    if let Some(e) = lv.host.block_entity(lv.level, pos) {
        let tag = mod_api::Tag::String(color.unwrap_or("").into());
        let _ = e.merge_data(
            &mut *lv.host,
            mod_api::Compound::from([(super::BELT_COLOR.into(), tag)]),
        );
    }
}

/// `BeltBlockEntity.applyColor`: every segment of the belt at `pos` takes dye `color` (None:
/// washed clean). False when it already has it (the click passes on).
fn apply_color(lv: &mut ServerLevel<'_>, pos: [i32; 3], color: Option<&str>) -> bool {
    if belt_color(lv, pos).as_deref() == color {
        return false;
    }
    let dim = lv.dim;
    let Some(chain) = with_world(|w| {
        let d = &w.dims[dim];
        Some(d.belts.get(d.belt_of.get(&pos)?)?.chain.clone())
    })
    .flatten() else {
        return false;
    };
    for p in chain {
        set_segment_color(lv, p, color);
    }
    true
}

/// `BeltBlockEntity.setCasingType`: cases the segment at `pos` in `kind` (`andesite` or
/// `brass`) or takes its casing off (None). The block entity keeps the kind for its clients, who
/// draw the casing (`client`, as `BeltModel` would) and whether a block covers it
/// (`updateCoverProperty`). A casing it had breaks with its particles and sound
/// (`levelEvent(PARTICLES_DESTROY_BLOCK)` of the casing block).
pub fn set_casing(lv: &mut ServerLevel<'_>, pos: [i32; 3], kind: Option<&str>) {
    let flag = if kind.is_some() { "true" } else { "false" };
    if let Some(e) = lv.host.block_entity(lv.level, pos) {
        let had = e
            .data(&*lv.host)
            .and_then(|d| match d.get(&super::BELT_CASING.into()) {
                Some(mod_api::Tag::String(k)) => {
                    Some(k.to_string_lossy()).filter(|k| !k.is_empty())
                }
                _ => None,
            });
        if let Some(had) = had
            && let Some(state) = lv.host.block_state_id(if had == "brass" {
                "create:brass_casing"
            } else {
                "create:andesite_casing"
            })
        {
            lv.destroy_effect(pos, state);
        }
        let tag = mod_api::Tag::String(kind.unwrap_or("").into());
        let _ = e.merge_data(
            &mut *lv.host,
            mod_api::Compound::from([(super::BELT_CASING.into(), tag)]),
        );
    }
    crate::glue::items::wrench::switch_property(lv, pos, "casing", flag);
    update_cover(lv, pos);
}

/// `BeltBlock.updateCoverProperty`: a cased horizontal segment at `pos` is covered while a block
/// stands on it whose collision box (`isBlockCoveringBelt`) is at least half a block wide each
/// way and starts at its bottom, but for funnels not facing up and tunnels.
pub fn update_cover(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let Some(seg) = super::info_at(lv, pos).and_then(|(_, i)| i.belt) else {
        return;
    };
    let cased = lv.block_state(pos).is_some_and(|s| {
        s.property(&lv.host.block_states(), "casing").ok().flatten() == Some("true")
    });
    if !cased || seg.slope != belt::Slope::Horizontal {
        return;
    }
    let above = [pos[0], pos[1] + 1, pos[2]];
    let excluded = match super::info_at(lv, above) {
        Some((b, i)) => {
            matches!(
                b,
                create_engine::Block::AndesiteTunnel | create_engine::Block::BrassTunnel
            ) || super::funnel::is_funnel(b) && i.facing != Some(Dir::Up)
        }
        None => false,
    };
    let cell = [
        f64::from(above[0]),
        f64::from(above[1]),
        f64::from(above[2]),
        f64::from(above[0]) + 1.0,
        f64::from(above[1]) + 1.0,
        f64::from(above[2]) + 1.0,
    ];
    let boxes = lv
        .level
        .collision_boxes(&*lv.host, cell)
        .unwrap_or_default();
    let covered = !excluded && !boxes.is_empty() && {
        let lo = |k: usize| boxes.iter().map(|b| b[k]).fold(f64::MAX, f64::min);
        let hi = |k: usize| boxes.iter().map(|b| b[k + 3]).fold(f64::MIN, f64::max);
        hi(0) - lo(0) >= 0.5 && hi(2) - lo(2) >= 0.5 && lo(1) <= cell[1] + 1e-6
    };
    let Some(e) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    let was = matches!(
        e.data(&*lv.host)
            .and_then(|d| d.get(&super::BELT_COVERED.into()).cloned()),
        Some(mod_api::Tag::Byte(1))
    );
    if was != covered {
        let tag = mod_api::Tag::Byte(i8::from(covered));
        let _ = e.merge_data(
            &mut *lv.host,
            mod_api::Compound::from([(super::BELT_COVERED.into(), tag)]),
        );
    }
}

/// `BeltBlock.onWrenched` after `BeltSlicer.useWrench` passed it on: a casing comes off when a
/// side or the bottom is clicked; a pulley clicked on a side becomes a middle segment again, its
/// shaft back to the player outside creative. True when the wrench did something.
pub fn wrench_belt(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: mod_api::PlayerRef,
    clicked: Dir,
) -> bool {
    let Some((_, _, seg)) = belt_segment(lv, pos) else {
        return false;
    };
    let cased = lv
        .block_state(pos)
        .and_then(|s| {
            s.property(&lv.host.block_states(), "casing")
                .ok()
                .flatten()
                .map(|v| v == "true")
        })
        .unwrap_or(false);
    if cased && clicked != Dir::Up {
        set_casing(lv, pos, None);
        return true;
    }
    if seg.part == belt::Part::Pulley && clicked.axis() != Axis::Y {
        crate::glue::items::wrench::switch_property(lv, pos, "part", "middle");
        if !crate::glue::items::creative(&*lv.host, player)
            && let Some(shaft) = items::stack_of(&*lv.host, "create:shaft", 1)
        {
            crate::glue::items::give_back(&mut *lv.host, player, shaft);
        }
        return true;
    }
    false
}

/// `BeltBlock.useItemOn` with an empty main hand: every item within reach of the segment's
/// centre goes to the player. True when the click was the belt's.
pub fn use_belt(lv: &mut ServerLevel<'_>, pos: [i32; 3], player: u128) -> bool {
    let Some((c, index, _)) = belt_segment(lv, pos) else {
        return false;
    };
    let dim = lv.dim;
    let taken = with_world(|w| {
        let b = w.dims[dim].belts.get_mut(&c)?;
        let taken = b.items.take_within(index as f32 + 0.5, 0.55);
        if !taken.is_empty() {
            w.dims[dim].belt_changes.push(c);
            w.belt_inventory_changed(dim, c);
        }
        Some(taken)
    })
    .flatten()
    .unwrap_or_default();
    for i in taken {
        if let Some(s) = belt_stack(lv, &i) {
            items::give(&mut *lv.host, player, s);
        }
    }
    true
}

/// How a belt names `stack` (`belt_stack` turns it back): a sequenced assembly's transitional
/// item by its id with its progress apart, any other by `items::key_of`,
/// its components kept (a package, a potion, a named or damaged item).
pub fn belt_name(
    host: &dyn mod_api::Host,
    stack: &ItemStack,
) -> (String, Option<create_engine::sequenced::Progress>) {
    let progress = (!stack.is_damaged())
        .then(|| crate::glue::processing::sequenced_progress(stack))
        .flatten();
    match progress {
        Some(p) => (items::id_of(host, stack), Some(p)),
        None => (items::key_of(host, stack), None),
    }
}

/// A belt item as a stack, its sequenced assembly progress with it.
pub fn belt_stack(lv: &ServerLevel<'_>, item: &belt::Item) -> Option<ItemStack> {
    let mut stack = items::stack_of(&*lv.host, &item.item, u32::from(item.count))?;
    if let Some((_, p, fraction)) = item
        .progress
        .as_ref()
        .and_then(crate::glue::processing::transitional_of)
    {
        crate::glue::processing::set_sequenced_progress(&mut stack, &p.id, p.step, fraction);
    }
    Some(stack)
}

/// Puts `count` of item `id` (an angle drawn for it, a sequenced assembly's `progress` with it)
/// onto belt segment `pos` from above (`ItemHandlerBeltSegment.insertItem`:
/// `canInsertAt(segment)`). The count left; None when no belt that carries items is there.
/// `simulate`: only how much it would take.
pub fn put_on_from_above(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    id: &str,
    count: u8,
    progress: Option<create_engine::sequenced::Progress>,
    simulate: bool,
) -> Option<u8> {
    let (c, index, seg) = belt_segment(lv, pos)?;
    if !seg.transports() {
        return None;
    }
    let dim = lv.dim;
    let n = count.min(items::stack_limit(&*lv.host, id));
    let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
    let angle = items::random_angle(lv, pos);
    let up = items::upright(&*lv.host, id);
    let added = with_world(|w| {
        let d = &mut w.dims[dim];
        let b = d.belts.get_mut(&c)?;
        let movement = belt::movement_facing(rpm, b.segment.facing);
        if !b.items.can_insert_at_from_side(index, Dir::Up, movement) {
            return None;
        }
        if simulate {
            return Some(());
        }
        let mut i = belt::Item::new(id, n, angle, up);
        i.progress = progress;
        i.inserted_at = index;
        i.inserted_from = Some(Dir::Up);
        i.position = index as f32 + 0.5 + if b.items.positive { -1.0 } else { 1.0 } / 16.0;
        b.items.add(i);
        d.belt_changes.push(c);
        w.belt_inventory_changed(dim, c);
        Some(())
    })
    .flatten();
    Some(if added.is_some() { count - n } else { count })
}

/// `DirectBeltInputBehaviour.handleInsertion` at `next`, the item moving toward `side`: a
/// depot or a belt there. What is left of it, None when nothing there takes items.
pub fn insert_into(
    lv: &mut ServerLevel<'_>,
    next: [i32; 3],
    side: Dir,
    item: &belt::Item,
) -> Option<u8> {
    insert_into_from(lv, next, side, item, true)
}

/// `insert_into` for a stack that comes from a belt behind (`from_belt`: Create's smoother
/// hand-over starts it a quarter block back) or from elsewhere (a funnel).
pub fn insert_into_from(
    lv: &mut ServerLevel<'_>,
    next: [i32; 3],
    side: Dir,
    item: &belt::Item,
    from_belt: bool,
) -> Option<u8> {
    handle_insertion(lv, next, side, item, from_belt, false)
}

/// `DirectBeltInputBehaviour.handleInsertion(stack, side, simulate)` at `next`: what is left of
/// `item` (moving toward `side`) after what takes items there takes it, or (`simulate`) would;
/// None when nothing there takes items.
pub fn handle_insertion(
    lv: &mut ServerLevel<'_>,
    next: [i32; 3],
    side: Dir,
    item: &belt::Item,
    from_belt: bool,
    simulate: bool,
) -> Option<u8> {
    // An item drain takes it as a depot would (the fluids subsystem's).
    if let Some(left) = super::super::fluids::insert_into_drain_sim(
        lv, next, side, &item.item, item.count, simulate,
    ) {
        return Some(left);
    }
    // So do upward saws, basins, millstones and crushing wheels (processing's).
    if let Some(left) = super::super::processing::belt_input_sim(lv, next, side, item, simulate) {
        return Some(left);
    }
    if let Some(f) = funnel_at(lv, next) {
        return Some(funnel_direct_input(lv, next, f, item, simulate));
    }
    if let Some((block, powered)) = super::chute::chute_at(lv, next) {
        return Some(chute_direct_input(lv, next, block, powered, item, simulate));
    }
    let dim = lv.dim;
    let is_depot = with_world(|w| w.dims[dim].depots.contains_key(&next)) == Some(true);
    if is_depot {
        // An ejector merges onto what it holds, up to its stack size (`enableMerging`).
        let mut item = item.clone();
        let mut rest = 0;
        if super::arms::is_ejector(dim, next) {
            let left = super::arms::depot_insert(lv, next, &item.item, item.count, true);
            if left == item.count || simulate {
                return Some(left);
            }
            if !depot_items(lv, next).is_empty() {
                return Some(super::arms::depot_insert(
                    lv, next, &item.item, item.count, false,
                ));
            }
            item.count -= left;
            rest = left;
        }
        let item = &item;
        // DepotBehaviour.tryInsertingFromSide: only into an empty depot (an ejector's only
        // while charged, `canAcceptItems`).
        if !depot_items(lv, next).is_empty() || !ejector_accepts(dim, next) {
            return Some(item.count);
        }
        let limit = items::stack_limit(&*lv.host, &item.item);
        if simulate {
            return Some(item.count.saturating_sub(limit));
        }
        let put = belt_stack(lv, item)?;
        if !items::set_slot(lv, next, 0, Some(put)) {
            return None;
        }
        let held = create_engine::depot::Held {
            position: if side.axis() == Axis::Y { 0.5 } else { 0.0 },
            from: create_engine::depot::Side::from_dir(side),
            angle: item.angle,
            side_offset: item.side_offset,
            locked: false,
            fan: item.fan,
        };
        with_world(|w| w.depot_put(dim, next, Some(held)));
        return Some(rest);
    }
    // BeltBlockEntity.tryInsertingFromSide on the next belt.
    let (c2, index, seg2) = belt_segment(lv, next)?;
    if !seg2.transports() {
        return Some(item.count);
    }
    // A brass tunnel over it that distributes takes the whole stack while it holds none and
    // its flap filter on that side passes it.
    let above = [next[0], next[1] + 1, next[2]];
    if super::tunnel::has_distribution(lv, above) {
        let from = side.opposite();
        if !super::tunnel::can_take(lv, above)
            || !super::tunnel::flap_filter_passes(lv, above, from, &item.item)
        {
            return Some(item.count);
        }
        if !simulate {
            super::flaps::tunnel(lv, next, from, true);
            super::tunnel::set_stack(lv, above, item, Some(from));
        }
        return Some(0);
    }
    let rpm2 = with_world(|w| w.speed(dim, c2)).flatten().unwrap_or(0.0);
    let speed2 = belt::direction_aware_speed(rpm2, seg2.facing);
    with_world(|w| {
        let b = w.dims[dim].belts.get_mut(&c2)?;
        let movement2 = belt::movement_facing(rpm2, b.segment.facing);
        if rpm2 == 0.0
            || movement2 == side.opposite()
            || !b.items.can_insert_at_from_side(index, side, movement2)
        {
            return Some(item.count);
        }
        if simulate {
            return Some(0);
        }
        let mut moved = item.clone();
        moved.locked = false;
        moved.position = index as f32 + 0.5 - speed2.signum() / 16.0;
        // From above or below: in the segment's middle.
        if side.axis() == Axis::Y {
        } else if movement2 != side {
            moved.side_offset = if side.positive() { 0.675 } else { -0.675 };
            if side.axis() == Axis::X {
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
        w.belt_inventory_changed(dim, c2);
        Some(0)
    })
    .flatten()
}

/// Whether something at `pos` takes items straight from a belt's end
/// (`DirectBeltInputBehaviour`, `resolveEnding`'s `INSERT`): a depot, a belt, an item drain, a
/// funnel or a chute. Only a floor funnel and a chute that may take items accept them; at the
/// others the items wait.
pub fn direct_input_at(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    let dim = lv.dim;
    let kept = with_world(|w| {
        let d = &w.dims[dim];
        d.depots.contains_key(&pos) || d.belt_of.contains_key(&pos) || d.drains.contains_key(&pos)
    });
    kept == Some(true)
        || funnel_at(lv, pos).is_some()
        || super::chute::chute_at(lv, pos).is_some()
        || super::super::processing::belt_input_side(lv, pos).is_some()
}

/// `DirectBeltInputBehaviour.canInsertFromSide` of what takes items at `pos`, for an item moving
/// toward `side`: a belt turning, not against it; a floor funnel inserting; a chute that may take
/// items (`canDirectlyInsert`); a depot or drain always.
pub fn direct_input_accepts(lv: &ServerLevel<'_>, pos: [i32; 3], side: Dir) -> bool {
    if let Some(f) = funnel_at(lv, pos) {
        return f.belt.is_none() && !f.extracting && f.facing == Dir::Up;
    }
    if super::chute::chute_at(lv, pos).is_some() {
        let above = [pos[0], pos[1] + 1, pos[2]];
        let below = [pos[0], pos[1] - 1, pos[2]];
        return super::chute::chute_at(lv, above).is_none()
            && !(super::chute::chute_motion(lv, pos) > 0.0
                && super::chute::chute_at(lv, below).is_none());
    }
    if let Some((c, _, seg)) = belt_segment(lv, pos) {
        let dim = lv.dim;
        let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
        // BeltBlockEntity.canInsertFrom: a running belt, not sideways or vertical, not moving
        // toward the side.
        return rpm != 0.0
            && !matches!(seg.slope, belt::Slope::Sideways | belt::Slope::Vertical)
            && belt::movement_facing(rpm, seg.facing) != side.opposite();
    }
    if let Some(only) = super::super::processing::belt_input_side(lv, pos) {
        return only.is_none_or(|d| d == side);
    }
    true
}

/// `FunnelBlockEntity`'s `DirectBeltInputBehaviour`: only an inserting floor funnel
/// (`supportsDirectBeltInput`) takes what its filter passes while unpowered
/// (`handleDirectBeltInput`), into the inventory under it. The count left.
fn funnel_direct_input(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    f: super::funnel::FunnelState,
    item: &belt::Item,
    simulate: bool,
) -> u8 {
    if f.belt.is_some() || f.extracting || f.facing != Dir::Up || f.powered {
        return item.count;
    }
    let Some(stack) = belt_stack(lv, item) else {
        return item.count;
    };
    if !super::funnel::funnel_passes(lv, pos, &stack) {
        return item.count;
    }
    let behind = create_engine::funnel::behind(pos, f.facing);
    if simulate {
        let room = items::insert_room(lv, behind, face(f.facing), &stack).unwrap_or(0);
        return item.count.saturating_sub(room.min(255) as u8);
    }
    let moved = stack.clone();
    let left = items::insert_item(lv, behind, face(f.facing), stack);
    let left = left.map_or(0, |l| l.count().min(255) as u8);
    if left != item.count {
        super::flaps::funnel(lv, pos, true);
        crate::glue::redstone::detectors::funnel_transfer(lv, pos, &moved);
    }
    left
}

/// `ChuteBlockEntity`'s `DirectBeltInputBehaviour`: while it may take items
/// (`canDirectlyInsert`: no chute above leading down into it, not blown up without a chute
/// feeding it) its handler takes the whole stack when empty (`ChuteItemHandler.insertItem`).
/// The count left.
fn chute_direct_input(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    block: create_engine::Block,
    powered: bool,
    item: &belt::Item,
    simulate: bool,
) -> u8 {
    let above = [pos[0], pos[1] + 1, pos[2]];
    let below = [pos[0], pos[1] - 1, pos[2]];
    if super::chute::chute_at(lv, above).is_some()
        || (super::chute::chute_motion(lv, pos) > 0.0
            && super::chute::chute_at(lv, below).is_none())
    {
        return item.count;
    }
    let Some(stack) = belt_stack(lv, item) else {
        return item.count;
    };
    if !super::chute::chute_accepts(lv, pos, block, powered, &stack) {
        return item.count;
    }
    if simulate {
        return if super::chute::chute_stack(lv, pos).is_none() {
            0
        } else {
            item.count
        };
    }
    if super::chute::chute_take(lv, pos, &stack) {
        0
    } else {
        item.count
    }
}

/// `BeltInventory.eject`: an item thrown off the end, forward and a little up.
pub(super) fn eject_item(
    lv: &mut ServerLevel<'_>,
    controller: [i32; 3],
    seg: &belt::Segment,
    length: i32,
    rpm: f32,
    item: &belt::Item,
) {
    let at = belt::vector_for_offset(controller, seg, length, item.position);
    let speed = belt::movement_speed(rpm).abs().max(1.0 / 8.0);
    let d = belt::movement_facing(rpm, seg.facing).offset();
    let motion = [
        f64::from(d[0]) * f64::from(speed),
        1.0 / 8.0,
        f64::from(d[2]) * f64::from(speed),
    ];
    if let Some(stack) = belt_stack(lv, item) {
        items::drop_stack(lv, [at[0], at[1] + 6.0 / 16.0, at[2]], motion, stack, 10);
    }
}

/// World writes for the engine's `BeltBlock.onRemove` cascade.
struct BeltBreakWorld<'a, 'l>(&'a mut ServerLevel<'l>);

impl belt::BreakWorld for BeltBreakWorld<'_, '_> {
    fn take_breaks(&mut self) -> Vec<create_engine::world::BeltBreak> {
        with_world(|w| std::mem::take(&mut w.dims[self.0.dim].belt_breaks)).unwrap_or_default()
    }

    fn replace(&mut self, b: create_engine::world::BeltBreak) {
        let lv = &mut *self.0;
        let state = {
            let facts = lv.host.block_states();
            match b.shaft {
                Some(a) => mod_api::BlockState::of(
                    &facts,
                    states::key(create_engine::Block::Shaft),
                    &[StateConstraint::new("axis", axis_name(a))],
                )
                .ok(),
                None => lv
                    .host
                    .block_key("minecraft:air")
                    .and_then(|air| mod_api::BlockState::of(&facts, air, &[]).ok()),
            }
        };
        if let Some(state) = state {
            lv.set_state(b.pos, state, update_all());
        }
    }
}

/// Publish only changed inventories after every subsystem has finished mutating them.
/// The column codec writes the current `BeltInventory.write` snapshot.
pub fn save_belts(lv: &mut ServerLevel<'_>) {
    let changed = with_world(|w| w.take_belt_saved_changes(lv.dim)).unwrap_or_default();
    for c in changed {
        crate::glue::persist::mark_pos_unsaved(lv.dim, c);
    }
}

/// Belts: segments broken next to a broken one become shafts or air, a broken belt's items
/// drop, then the engine visits depots and belts in level registration order,
/// applying each transfer before the next entity's turn (`BeltBlockEntity.tick`,
/// `DepotBehaviour.tick`), including empty inventories' direction updates.
pub fn tick_transport(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    belt::break_cascade(&mut BeltBreakWorld(lv));
    let drops = with_world(|w| std::mem::take(&mut w.dims[dim].belt_drops)).unwrap_or_default();
    for (c, seg, length, item) in drops {
        eject_item(lv, c, &seg, length, 0.0, &item);
    }
    crate::glue::fluids::pipes::tick_pipes(lv);
    let Some(mut phase) = with_world(|w| w.transport_ticks(dim)) else {
        return;
    };
    while let Some(turn) = with_world(|w| phase.next(w)).flatten() {
        let c = match turn {
            create_engine::world::TransportTick::Fluid(p) => {
                crate::glue::fluids::pipes::tick_transport_pipe(lv, p);
                crate::glue::fluids::plumbing::tick_entity(lv, p);
                continue;
            }
            create_engine::world::TransportTick::Spout(p) => {
                with_world(|w| {
                    if let Some(s) = w.dims[dim].spouts.get_mut(&p) {
                        create_engine::spout::tick(s);
                    }
                });
                continue;
            }
            create_engine::world::TransportTick::Drain(p) => {
                crate::glue::fluids::machines::tick_drain(lv, p);
                continue;
            }
            create_engine::world::TransportTick::Pump(p)
            | create_engine::world::TransportTick::Pipe(p) => {
                crate::glue::fluids::pipes::tick_transport_pipe(lv, p);
                continue;
            }
            create_engine::world::TransportTick::Depot(p) => {
                super::depot::tick_depot(lv, p);
                continue;
            }
            create_engine::world::TransportTick::Belt(c) => c,
        };
        let Some((seg, length, mut belt_items)) = with_world(|w| {
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
        let speed = belt::direction_aware_speed(rpm, seg.facing);
        let mut side = BeltSide {
            lv: &mut *lv,
            controller: c,
            segment: seg,
            length,
            rpm,
            positive: speed > 0.0,
        };
        let horizontal = seg.slope == belt::Slope::Horizontal;
        let ticked = belt_items.tick(length, speed, horizontal, &mut side);
        with_world(|w| {
            if let Some(b) = w.dims[dim].belts.get_mut(&c) {
                // Items another belt put here meanwhile go after ours.
                let added = std::mem::replace(&mut b.items, belt_items);
                for i in added.items {
                    b.items.add(i);
                }
            }
            if ticked.saved_changed {
                w.belt_inventory_changed(dim, c);
            }
            if ticked.changed {
                w.dims[dim].belt_changes.push(c);
            }
        });
        for item in &ticked.ejected {
            eject_item(lv, c, &seg, length, rpm, item);
        }
        if !ticked.ejected.is_empty() {
            let last = if speed > 0.0 { length - 1 } else { 0 };
            let last = belt::position_for_offset(c, &seg, length, last);
            super::flaps::tunnel(lv, last, belt::movement_facing(rpm, seg.facing), false);
        }
    }
    let changed = with_world(|w| {
        let mut v = std::mem::take(&mut w.dims[dim].belt_changes);
        v.sort();
        v.dedup();
        v
    })
    .unwrap_or_default();
    if !changed.is_empty() {
        arms_touched(dim, &[], &changed);
        let players = lv.players();
        send_belts(lv, &players, &changed);
        // A belt that starts carrying is drawn again (its drawing was kept while empty).
        for c in changed {
            let members = with_world(|w| {
                w.dims[dim]
                    .belts
                    .get(&c)
                    .map(|b| b.chain.clone())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
            for pos in members {
                if let Some(saved) = with_world(|w| w.saved_belt_segment(dim, pos)).flatten() {
                    lv.write_entity_field(
                        pos,
                        "BeltState",
                        mod_api::Compound::from([
                            (
                                "Controller".into(),
                                mod_api::Tag::IntArray(saved.controller.to_vec()),
                            ),
                            ("Index".into(), mod_api::Tag::Int(saved.index)),
                            ("Length".into(), mod_api::Tag::Int(saved.length)),
                        ]),
                    );
                }
            }
        }
    }
    crate::glue::fluids::pipes::finish_ticks(lv);
}

thread_local! {
    /// Belt segments whose block above changed (`BeltBlock.updateShape` from above), by level:
    /// their cover is looked at after the tick.
    static COVER_CHECKS: std::cell::RefCell<Vec<(usize, [i32; 3])>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The block above the belt segment at `pos` changed: its cover is looked at again.
pub fn cover_check(dim: usize, pos: [i32; 3]) {
    COVER_CHECKS.with_borrow_mut(|c| c.push((dim, pos)));
}

/// The cover checks of this level (`cover_check`).
pub fn tick_covers(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mine: Vec<[i32; 3]> = COVER_CHECKS.with_borrow_mut(|c| {
        if c.is_empty() {
            return Vec::new();
        }
        let mine = c.iter().filter(|e| e.0 == dim).map(|e| e.1).collect();
        c.retain(|e| e.0 != dim);
        mine
    });
    for pos in mine {
        update_cover(lv, pos);
    }
}

fn axis_name(a: Axis) -> &'static str {
    match a {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

/// The belts `belts` (controllers) to `players`.
pub fn send_belts(lv: &mut ServerLevel<'_>, players: &[PlayerRef], belts: &[[i32; 3]]) {
    let dim = lv.dim;
    let all: Vec<belt::Contents> = belts
        .iter()
        .map(|&c| {
            with_world(|w| w.dims[dim].belts.get(&c).cloned())
                .flatten()
                .map_or(
                    belt::Contents {
                        controller: c,
                        segment: belt::Segment {
                            facing: Dir::North,
                            slope: belt::Slope::Horizontal,
                            part: belt::Part::Start,
                        },
                        length: 0,
                        ending: belt::Ending::Eject,
                        items: belt::Inventory::default(),
                    },
                    |b| {
                        let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
                        let speed = belt::direction_aware_speed(rpm, b.segment.facing);
                        let mut side = BeltSide {
                            lv: &mut *lv,
                            controller: c,
                            segment: b.segment,
                            length: b.chain.len() as i32,
                            rpm,
                            positive: speed > 0.0,
                        };
                        let ending = belt::Surroundings::ending(&mut side);
                        belt::Contents {
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
    for m in belt::encode(&all) {
        net::send_to_players(&mut *lv.host, players, belt::CHANNEL, &m);
    }
    for contents in &all {
        super::super::drawn::publish(
            lv,
            contents.controller,
            super::super::drawn::snapshot(belt::encode(std::slice::from_ref(contents))),
        );
    }
}

/// A belt's surroundings on the server: what lies past its end, the funnels and tunnels over
/// its segments, and the machines above them.
pub struct BeltSide<'a, 'l> {
    pub lv: &'a mut ServerLevel<'l>,
    pub controller: [i32; 3],
    pub segment: belt::Segment,
    pub length: i32,
    pub rpm: f32,
    pub positive: bool,
}

impl BeltSide<'_, '_> {
    fn past_end(&self) -> [i32; 3] {
        belt::position_for_offset(
            self.controller,
            &self.segment,
            self.length,
            if self.positive { self.length } else { -1 },
        )
    }

    fn movement(&self) -> Dir {
        belt::movement_facing(self.rpm, self.segment.facing)
    }

    fn at(&self, segment: i32) -> [i32; 3] {
        belt::position_for_offset(self.controller, &self.segment, self.length, segment)
    }
}

/// The game's face `d`.
fn block_face(d: Dir) -> BlockFace {
    match d {
        Dir::Down => BlockFace::Down,
        Dir::Up => BlockFace::Up,
        Dir::North => BlockFace::North,
        Dir::South => BlockFace::South,
        Dir::West => BlockFace::West,
        Dir::East => BlockFace::East,
    }
}

impl belt::Surroundings for BeltSide<'_, '_> {
    fn ending(&mut self) -> belt::Ending {
        let next = self.past_end();
        if direct_input_at(self.lv, next) {
            return belt::Ending::Insert;
        }
        // BlockHelper.hasBlockSolidSide toward the belt.
        let face = block_face(self.movement().opposite());
        let sturdy = self
            .lv
            .level
            .block_face_sturdy(&*self.lv.host, next, face, SupportType::Full);
        if sturdy == Some(true) {
            return belt::Ending::Blocked;
        }
        belt::Ending::Eject
    }

    fn insert(&mut self, item: &belt::Item) -> Option<u8> {
        let (next, side) = (self.past_end(), self.movement());
        let left = insert_into(self.lv, next, side, item);
        if left.is_some_and(|l| l != item.count) {
            let last = self.at(if self.positive { self.length - 1 } else { 0 });
            super::flaps::tunnel(self.lv, last, side, false);
        }
        left
    }

    fn funnel(&mut self, segment: i32) -> Option<(create_engine::funnel::OnBelt, Dir)> {
        let at = self.at(segment);
        let f = funnel_at(self.lv, [at[0], at[1] + 1, at[2]])?;
        let on = create_engine::funnel::OnBelt {
            facing: f.facing,
            shape: f.belt?,
            powered: f.powered,
        };
        Some((on, self.movement()))
    }

    fn funnel_insert(&mut self, segment: i32, item: &belt::Item) -> Option<u8> {
        let at = self.at(segment);
        let pos = [at[0], at[1] + 1, at[2]];
        let f = funnel_at(self.lv, pos)?;
        let behind = create_engine::funnel::behind(pos, f.facing);
        let stack = belt_stack(self.lv, item)?;
        // `filtering.test`: a brass funnel lets only what its filter passes in.
        if !super::funnel::funnel_passes(self.lv, pos, &stack) {
            return Some(item.count);
        }
        // A brass funnel set to exactly N: only a stack of at least N, and only when all N
        // fit; then N of it (`getAmountToExtract`, `getModeToExtract`).
        let (amount, exact) = super::amount::funnel_extraction(self.lv, pos, &f);
        let mut stack = stack;
        let mut kept = 0;
        if exact {
            let n = amount.min(255) as u8;
            if n > item.count {
                return Some(item.count);
            }
            let Some(part) = items::with_count(&stack, u32::from(n)) else {
                return Some(item.count);
            };
            let room = items::insert_room(self.lv, behind, face(f.facing), &part).unwrap_or(0);
            if room < u32::from(n) {
                return Some(item.count);
            }
            kept = item.count - n;
            stack = part;
        }
        let moved = stack.clone();
        let left = items::insert_item(self.lv, behind, face(f.facing), stack);
        let left = left.map_or(0, |l| l.count().min(255) as u8) + kept;
        if left != item.count {
            super::flaps::funnel(self.lv, pos, true);
            crate::glue::redstone::detectors::funnel_transfer(self.lv, pos, &moved);
        }
        Some(left)
    }

    /// `flapTunnelsAndCheckIfStuck`: the tunnel over segment `segment` may take some of `item`;
    /// unless it must wait, the tunnels it leaves and enters flap.
    fn tunnel(&mut self, segment: i32, item: &mut belt::Item) -> bool {
        if self.segment.slope != belt::Slope::Horizontal {
            return false;
        }
        let before = item.count;
        let stuck = self.tunnel_takes(segment, item);
        if !stuck || item.count == 0 && before > 0 {
            let movement = self.movement();
            let step = if self.positive { 1 } else { -1 };
            super::flaps::tunnel(self.lv, self.at(segment - step), movement, false);
            super::flaps::tunnel(self.lv, self.at(segment), movement.opposite(), true);
        }
        stuck
    }

    /// The machines over segment `segment` (presses, deployers, spouts) are the processing
    /// subsystems': the first that answers acts.
    fn process(
        &mut self,
        segment: i32,
        item: &mut belt::Item,
        locked: bool,
    ) -> Option<belt::Process> {
        let at = self.at(segment);
        super::super::SUBSYSTEMS
            .iter()
            .find_map(|s| s.belt_process(self.lv, at, item, locked))
    }
}

impl BeltSide<'_, '_> {
    /// The tunnel over segment `segment` of a horizontal belt takes some of `item` (`tunnel`):
    /// true when it must wait.
    fn tunnel_takes(&mut self, segment: i32, item: &mut belt::Item) -> bool {
        let at = self.at(segment);
        let t = [at[0], at[1] + 1, at[2]];
        let Some((axis, shape)) = tunnel_state(self.lv, t) else {
            return false;
        };
        let movement = self.movement();
        // `transferred`: the stack as it entered, told to the tunnel's display links once it
        // goes on.
        let transferred = i32::from(item.count);
        let dim = self.lv.dim;
        // A brass tunnel that distributes (`hasDistributionBehaviour`: an open side across its
        // axis, or a brass tunnel beside it) takes the whole stack while it holds none, and
        // holds back the next.
        if with_world(|w| w.dims[dim].brass.contains_key(&t)) == Some(true) {
            // `stuckAtTunnel` (`canInsert`): its own filter on the side the item comes in by
            // must pass it.
            let from = movement.opposite();
            if !super::tunnel::own_filter_passes(self.lv, t, from, &item.item) {
                return true;
            }
            if !super::tunnel::has_distribution(self.lv, t) {
                return false;
            }
            if !super::tunnel::can_take(self.lv, t) {
                return true;
            }
            super::tunnel::set_stack(self.lv, t, item, Some(from));
            item.count = 0;
            return true;
        }
        if !create_engine::tunnel::splits(axis, shape, movement, item.count) {
            super::super::display::items_through_tunnel(self.lv, t, transferred);
            return false;
        }
        for d in create_engine::tunnel::sides(axis, shape) {
            if d.axis() == axis {
                continue;
            }
            // A flap: not where another tunnel continues it.
            let o = d.offset();
            if tunnel_state(self.lv, [t[0] + o[0], t[1], t[2] + o[2]]).is_some() {
                continue;
            }
            let out = [at[0] + o[0], at[1], at[2] + o[2]];
            // `canInsertFromSide`: a stopped belt, or one running into the tunnel, is passed by.
            if !direct_input_accepts(self.lv, out, d) {
                continue;
            }
            let one = belt::Item {
                count: 1,
                ..item.clone()
            };
            match insert_into(self.lv, out, d, &one) {
                None => continue,
                Some(0) => super::flaps::tunnel(self.lv, at, d, false),
                Some(_) => return true,
            }
            item.count -= 1;
            if item.count <= 1 {
                break;
            }
        }
        super::super::display::items_through_tunnel(self.lv, t, transferred);
        false
    }
}

/// An entity noted on a belt this tick: its level, the block, its id, and whether it stepped
/// on that block (`stepped_on`) rather than being inside a segment.
type Passenger = (usize, [i32; 3], u32, bool);

thread_local! {
    /// Entities inside a belt segment this tick (`BeltBlock.entityInside`), by level: noted in
    /// the entity's move, where it can't be moved, and carried at the end of the level's tick.
    static PASSENGERS: std::cell::RefCell<Vec<Passenger>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// `BeltBlock.entityInside` for an entity that is not an item (items land on the belt): noted
/// to be carried.
pub fn passenger_inside(dim: usize, pos: [i32; 3], entity: u32) {
    PASSENGERS.with_borrow_mut(|p| p.push((dim, pos, entity, false)));
}

/// `BeltBlock.updateEntityAfterFallOn` (`block_step_on`, the block under an entity on the
/// ground): noted to be carried by the segment at its feet or the one below them, found at the
/// end of the tick. On a slope the segment holding it up reaches above its own block, where
/// `entity_inside` can't see it.
pub fn stepped_on(dim: usize, on: [i32; 3], entity: u32) {
    PASSENGERS.with_borrow_mut(|p| p.push((dim, on, entity, true)));
}

/// `BeltMovementHandler.transportEntity` for the entities other than players on this level's
/// belts: each is moved as the belt runs (`passenger_motion`), toward the middle, up a slope
/// by stepping up its stairs (`BeltShapes`) and down it at the belt's speed. The mod API moves an
/// entity only by its motion (`EntityRef::set_velocity`), so a passenger is given the belt's
/// motion each tick rather than moved by it. Mobs are slowed (slowness II for half a second)
/// and take no fall damage while carried.
pub fn transport_passengers(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let mut mine: Vec<([i32; 3], u32, bool)> = PASSENGERS.with_borrow_mut(|p| {
        if p.is_empty() {
            return Vec::new();
        }
        let mine = p
            .iter()
            .filter(|e| e.0 == dim)
            .map(|e| (e.1, e.2, e.3))
            .collect();
        p.retain(|e| e.0 != dim);
        mine
    });
    if mine.is_empty() {
        return;
    }
    // One belt segment per entity: the one it stands in (`lastCollidedPos`).
    mine.sort_by_key(|e| (e.1, e.2));
    mine.dedup_by_key(|e| e.1);
    let slowness = lv.host.effect_key("minecraft:slowness");
    for (pos, id, stepped) in mine {
        let Some(e) = lv.host.entity(id) else {
            continue;
        };
        // Items land on the belt; players are carried by their own clients
        // (`carry_local_player`), which move them.
        let kind = e
            .kind(&*lv.host)
            .map(|k| k.id(&*lv.host))
            .unwrap_or_default();
        if kind == "minecraft:item" || kind == "minecraft:player" {
            continue;
        }
        let (Some(feet), Some(v)) = (e.position(&*lv.host), e.velocity(&*lv.host)) else {
            continue;
        };
        let at = feet.map(|v| v.floor() as i32);
        let below = [at[0], at[1] - 1, at[2]];
        let pos = if !stepped {
            pos
        } else if belt_segment(lv, at).is_some() {
            at
        } else if belt_segment(lv, below).is_some() {
            below
        } else {
            // Over the edge of the segment holding it up.
            pos
        };
        let Some((c, _, seg)) = belt_segment(lv, pos) else {
            continue;
        };
        let rpm = with_world(|w| w.speed(dim, c)).flatten().unwrap_or(0.0);
        let living = lv.host.living(e);
        let Some(m) = belt::passenger_motion(&seg, pos, feet, rpm, false) else {
            continue;
        };
        // Lock mobs in place: MOVEMENT_SLOWDOWN 10 ticks, amplifier 1, hidden.
        if let (Some(living), Some(key)) = (living, slowness) {
            living.add_effect(
                &mut *lv.host,
                &mod_api::ActiveEffect::new(key, 10, 1, false, false, false),
            );
        }
        e.reset_fall_distance(&mut *lv.host);
        // Up a slope it climbs the belt's stairs as it is pushed along (gravity keeps it on
        // them); down one it sinks at the belt's speed against gravity (0.08 a tick).
        let vy = if m[1] < 0.0 {
            m[1] + 0.08
        } else if m[1] > 0.0 {
            v[1].min(0.0)
        } else {
            v[1]
        };
        e.set_velocity(&mut *lv.host, [m[0], vy, m[2]]);
    }
}

/// Ground friction a walking entity's motion keeps each tick (a block's 0.6 times 0.91).
const GROUND_FRICTION: f64 = 0.6 * 0.91;

/// `BeltMovementHandler.transportEntity` for the window's own player, which its client moves:
/// standing on a running belt (not sneaking, not flying), it is pushed so that it goes the belt's
/// way at the belt's speed on top of its own walking (a steady push `m (1 - friction)` moves it
/// `m` a tick more).
pub fn carry_local_player(host: &mut dyn mod_api::ClientHost) {
    let Some(player) = host.local_player() else {
        return;
    };
    let e = player.entity();
    let (Some(feet), Some(level)) = (e.position(&*host), player.level(&*host)) else {
        return;
    };
    if e.is_shift_key_down(&*host) == Some(true)
        || player.abilities(&*host).is_some_and(|a| a.flying)
    {
        return;
    }
    // `updateEntityAfterFallOn`: the segment at its feet, else the one below them (a slope's
    // segment reaches above its block).
    let at = feet.map(|v| v.floor() as i32);
    let segment = |p: [i32; 3]| {
        level
            .block_state(&*host, p)
            .and_then(super::super::states::id)
            .and_then(|id| {
                super::super::client_table(&*host, |t| t.get(id).and_then(|(_, i)| i.belt))
                    .flatten()
            })
    };
    let below = [at[0], at[1] - 1, at[2]];
    // `entityInside` of the blocks its box is in (Create's `checkInsideBlocks` asks every block
    // the box reaches, whatever its shape): off the end of a belt, the segment still under the
    // box's edge carries it until it falls.
    let under_box = || {
        let [lo, hi] = e.bounding_box(&*host)?;
        let (x0, x1) = (
            (lo[0] + 1.0e-7).floor() as i32,
            (hi[0] - 1.0e-7).floor() as i32,
        );
        let (z0, z1) = (
            (lo[2] + 1.0e-7).floor() as i32,
            (hi[2] - 1.0e-7).floor() as i32,
        );
        (x0..=x1)
            .flat_map(|x| (z0..=z1).map(move |z| [x, at[1], z]))
            .find_map(|p| segment(p).map(|s| (p, s)))
    };
    let Some((pos, seg)) = segment(at)
        .map(|s| (at, s))
        .or_else(|| segment(below).map(|s| (below, s)))
        .or_else(under_box)
    else {
        return;
    };
    let rpm = super::super::kinetics::shown_speeds(&*host, level, pos)[0];
    let Some(m) = belt::passenger_motion(&seg, pos, feet, rpm, false) else {
        return;
    };
    let keep = 1.0 - GROUND_FRICTION;
    // Up a slope the player steps up the belt's stairs as it is pushed along; down one it is
    // pushed down at the belt's speed.
    host.push_player([
        (m[0] * keep) as f32,
        if m[1] < 0.0 {
            (m[1] * keep) as f32
        } else {
            0.0
        },
        (m[2] * keep) as f32,
    ]);
}
