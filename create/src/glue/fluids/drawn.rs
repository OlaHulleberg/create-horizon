//! What clients draw of item drains (`ItemDrainRenderer`): the fluid in the drain's basin and
//! the stack crossing it, rolling as it goes. The server shows a drain's stack in its block
//! entity data (`DRAIN_ITEM`) each tick it changes.

use super::super::server::ServerLevel;
use super::super::world::with_world;
use super::TANK;
use super::client::fluid_box;
use create_engine::kinetics::Dir;
use create_engine::tank::Codes;
use mod_api::{
    BlockEntitySubmitEvent, ClientHost, Compound, ItemDisplayContext, ModBlockEntity, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// The key of a drain's stack in its block entity data: a list of its item, count, position,
/// position a tick ago and way (`Dir` index); empty while it holds nothing.
pub const DRAIN_ITEM: &str = "DrainItem";

/// A drain by level and position.
type Shown = (usize, [i32; 3]);

thread_local! {
    /// What each drain's block entity last showed, by level and position.
    static SHOWN: RefCell<HashMap<Shown, Vec<Tag>>> = RefCell::new(HashMap::new());
}

pub(super) fn reset() {
    SHOWN.with_borrow_mut(HashMap::clear);
}

/// Drains whose stack moved or changed show it again.
pub(super) fn send(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let now: Vec<([i32; 3], Vec<Tag>)> = with_world(|w| {
        w.dims[dim]
            .drains
            .iter()
            .map(|(p, d)| {
                let c = d.held.as_ref().map_or_else(Vec::new, |h| {
                    vec![
                        Tag::String(h.item.as_str().into()),
                        Tag::Int(i32::from(h.count)),
                        Tag::Float(h.position.to_bits()),
                        Tag::Float(h.prev.to_bits()),
                        Tag::Int(h.from as i32),
                    ]
                });
                (*p, c)
            })
            .collect()
    })
    .unwrap_or_default();
    let changed: Vec<_> = SHOWN.with_borrow_mut(|shown| {
        shown.retain(|(d, p), _| *d != dim || now.iter().any(|(q, _)| q == p));
        let mut changed = Vec::new();
        for (p, c) in now {
            let before = shown.get(&(dim, p));
            if before != Some(&c) && !(before.is_none() && c.is_empty()) {
                shown.insert((dim, p), c.clone());
                changed.push((p, c));
            }
        }
        changed
    });
    for (pos, c) in changed {
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let data = Compound::from([(DRAIN_ITEM.into(), Tag::List(c))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
}

/// The stack a drain's data shows: item, count, position, previous position, way.
fn shown(entity: &ModBlockEntity) -> Option<(String, u32, f32, f32, Dir)> {
    let Some(Tag::List(l)) = entity.nbt.get(&DRAIN_ITEM.into()) else {
        return None;
    };
    let [
        Tag::String(item),
        Tag::Int(count),
        Tag::Float(position),
        Tag::Float(prev),
        Tag::Int(from),
    ] = l.as_slice()
    else {
        return None;
    };
    Some((
        item.to_string_lossy(),
        (*count).max(1) as u32,
        f32::from_bits(*position),
        f32::from_bits(*prev),
        Dir::ALL[(*from).clamp(0, 5) as usize],
    ))
}

/// Whether a drain's stack moves this frame.
pub(super) fn animated(entity: &ModBlockEntity) -> bool {
    shown(entity).is_some_and(|(_, _, p, q, _)| p != q)
}

/// `ItemDrainRenderer.renderFluid` (the basin's fluid) and `renderItem`.
pub(super) fn draw(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    if let Some(Tag::IntArray(v)) = event.entity.nbt.get(&TANK.into()).cloned()
        && let &[fluid, amount, capacity] = v.as_slice()
        && let Some(fluid) = Codes::fluid(&super::fluid_keys(&event.entity.nbt, TANK), fluid)
        && amount > 0
        && capacity > 0
    {
        let level = (amount as f32 / capacity as f32).min(1.0);
        let (min, max) = (2.0 / 16.0, 14.0 / 16.0);
        let y = 5.0 / 16.0;
        let top = y + 7.0 / 16.0 * level;
        fluid_box(
            host,
            &mut *event.draw,
            fluid,
            amount as u32,
            [min, y, min],
            [max, top, max],
        );
    }
    let Some((item, count, position, prev, from)) = shown(event.entity) else {
        return;
    };
    if from.axis() == create_engine::kinetics::Axis::Y {
        return;
    }
    let Some(stack) = mod_api::Identifier::parse(&item)
        .ok()
        .and_then(|i| host.item_key(&i))
        .and_then(|k| mod_api::ItemStack::new(k, 1))
    else {
        return;
    };
    let block = host.item_block(stack.item()).is_some();
    let upright = crate::glue::logistics::upright(host, &item);
    let draw = &mut *event.draw;
    let t = draw.partial();
    let offset = prev + (position - prev) * t;
    let o = from.offset().map(|v| v as f32);
    draw.push();
    draw.translate([0.5, 15.0 / 16.0, 0.5]);
    // Toward the side it leaves by, from the middle.
    let along = offset - 0.5;
    draw.translate([o[0] * along, 0.0, o[2] * along]);
    if upright {
        draw.translate([0.0, 3.0 / 32.0, 0.0]);
    }
    // It rolls as it goes (`verticalAngle`).
    let positive = if from.positive() { 1.0 } else { -1.0 };
    let angle = positive * offset * 360.0;
    if from.axis() != create_engine::kinetics::Axis::X {
        draw.rotate_about([1.0, 0.0, 0.0], angle, [0.0; 3]);
    }
    if from.axis() != create_engine::kinetics::Axis::Z {
        draw.rotate_about([0.0, 0.0, 1.0], -angle, [0.0; 3]);
    }
    let copies = ((count as f32).log2() as u32) / 2;
    for _ in 0..=copies {
        draw.push();
        draw.scale([0.5, 0.5, 0.5]);
        if !block && !upright {
            draw.rotate_about([1.0, 0.0, 0.0], 90.0, [0.0; 3]);
        }
        draw.item(&stack, ItemDisplayContext::Fixed, None, 0);
        draw.pop();
        if !upright {
            if !block {
                draw.rotate_about([0.0, 1.0, 0.0], 10.0, [0.0; 3]);
            }
            draw.translate([0.0, if block { 1.0 / 64.0 } else { 1.0 / 16.0 }, 0.0]);
        } else {
            draw.translate([0.0, 0.0, -1.0 / 16.0]);
        }
    }
    draw.pop();
}
