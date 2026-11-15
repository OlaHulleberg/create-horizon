//! After a level's tick: blocks of ours that changed, the large water wheels' frames, timers that
//! ran out, girders and bells to look at, then what clients draw. Nothing runs with nothing to
//! do.

use super::super::persist;
use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::super::world::with_world;
use super::{
    DECOR, Timer, add, block_of, bracketed, brackets, clicks, flag, girders, placard, stack_of,
    with,
};
use create_engine::Block;
use create_engine::decor;
use create_engine::kinetics::Dir;
use mod_api::{Compound, HostBlockStates, Tag};

pub(super) fn tick(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let changes = with(dim, |d| std::mem::take(&mut d.changes));
    for (pos, old, new) in changes {
        changed(lv, pos, old, new);
    }
    large_wheels(lv);
    let now = lv.game_time();
    let due: Vec<([i32; 3], Timer)> = DECOR.with_borrow_mut(|d| {
        let Some(d) = d.get_mut(&dim) else {
            return Vec::new();
        };
        if d.timers.is_empty() {
            return Vec::new();
        }
        let due = d
            .timers
            .iter()
            .filter(|t| t.1 <= now)
            .map(|t| (t.0, t.2))
            .collect();
        d.timers.retain(|t| t.1 > now);
        due
    });
    for (pos, timer) in due {
        let want = match timer {
            Timer::Placard => Block::Placard,
            Timer::Bell => Block::DeskBell,
            Timer::Hatch => Block::ItemHatch,
        };
        if block_of(lv.block(pos)).map(|(b, _)| b) != Some(want) {
            continue;
        }
        if timer == Timer::Hatch {
            lv.set_property(pos, "open", "false", update_all());
        } else {
            clicks::power(lv, pos, false);
        }
    }
    let (mut girders_due, bells) = with(dim, |d| {
        (
            std::mem::take(&mut d.girder_checks),
            std::mem::take(&mut d.bell_checks),
        )
    });
    girders_due.sort_unstable();
    girders_due.dedup();
    for pos in girders_due {
        girders::reshape(lv, pos);
    }
    for pos in bells {
        haunt_bell(lv, pos);
    }
    let mut scaffolds = with(dim, |d| std::mem::take(&mut d.scaffold_checks));
    scaffolds.sort_unstable();
    scaffolds.dedup();
    for pos in scaffolds {
        super::scaffolding::check(lv, pos);
    }
    let mut ladders = with(dim, |d| std::mem::take(&mut d.ladder_checks));
    ladders.sort_unstable();
    ladders.dedup();
    for pos in ladders {
        super::ladders::check(lv, pos);
    }
    let mut cloths = with(dim, |d| std::mem::take(&mut d.cloth_checks));
    cloths.sort_unstable();
    cloths.dedup();
    for pos in cloths {
        super::cloth::check(lv, pos);
    }
    let mut shown = with(dim, |d| std::mem::take(&mut d.shown));
    shown.sort_unstable();
    shown.dedup();
    for pos in shown {
        show(lv, pos);
    }
}

/// What the block entity at `pos` shows of ours: a shaft's, cog's or pipe's bracket, a placard's
/// item, a hatch's filter. Its clients see it with the block entity's data.
fn show(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let Some((block, _)) = block_of(lv.block(pos)) else {
        return;
    };
    let Some(entity) = lv.host.block_entity(lv.level, pos) else {
        return;
    };
    if bracketed(block) {
        let b = with(lv.dim, |d| d.brackets.get(&pos).copied());
        let text = brackets::shown_text(b);
        let data = Compound::from([(super::BRACKET.into(), Tag::String(text.as_str().into()))]);
        let _ = entity.merge_data(&mut *lv.host, data);
        return;
    }
    // A stack is written whole: a merge would keep the components of the stack it replaces.
    let (key, nbt) = if block == Block::ItemHatch {
        let filter = with(lv.dim, |d| d.hatch_filters.get(&pos).cloned());
        let nbt = filter
            .and_then(|f| crate::glue::items::filters::filter_stack(&*lv.host, &f))
            .and_then(|s| s.to_nbt(&*lv.host))
            .unwrap_or_default();
        (super::hatch::FILTER, nbt)
    } else if block == Block::Placard {
        let item = with(lv.dim, |d| d.placards.get(&pos).cloned());
        let nbt = item.and_then(|s| s.to_nbt(&*lv.host)).unwrap_or_default();
        (placard::ITEM, nbt)
    } else {
        return;
    };
    let mut data = entity.data(&*lv.host).unwrap_or_default();
    if data.get(&key.into()) == Some(&Tag::Compound(nbt.clone())) {
        return;
    }
    data.insert(key.into(), Tag::Compound(nbt));
    let _ = entity.replace_data(&mut *lv.host, data);
}

/// Whether two states are the same block with the same properties but `waterlogged`.
fn only_water(lv: &ServerLevel<'_>, old: u32, new: u32) -> bool {
    let (Some(a), Some(b)) = (states::state(old), states::state(new)) else {
        return false;
    };
    if a.block() != b.block() {
        return false;
    }
    let (ta, tb) = (
        super::state_text(&*lv.host, a),
        super::state_text(&*lv.host, b),
    );
    let (_, pa) = states::split(&ta);
    let (_, pb) = states::split(&tb);
    pa.iter()
        .all(|(k, v)| *k == "waterlogged" || pb.iter().any(|(k2, v2)| k2 == k && v2 == v))
}

/// A block of ours placed, changed or removed.
fn changed(lv: &mut ServerLevel<'_>, pos: [i32; 3], old: u32, new: u32) {
    let dim = lv.dim;
    let (was, is) = (block_of(old).map(|x| x.0), block_of(new).map(|x| x.0));
    // `AbstractSimpleShaftBlock.onRemove`: any other state but a waterlogging change drops its
    // bracket.
    if was.is_some_and(bracketed)
        && !only_water(lv, old, new)
        && let Some(b) = with(dim, |d| d.brackets.remove(&pos))
    {
        super::brackets::removed_effect(lv, pos, b);
        with(dim, |d| d.shown.push(pos));
        girders::around(dim, pos);
        persist::mark_pos_unsaved(dim, pos);
        if let Some(stack) = stack_of(&*lv.host, b.block(), 1) {
            lv.drop_at(pos, stack);
        }
    }
    if was == is {
        return;
    }
    match was {
        // `PlacardBlock.onRemove`: its item drops.
        Some(Block::Placard) => {
            if let Some(s) = with(dim, |d| d.placards.remove(&pos)) {
                lv.drop_at(pos, s);
                persist::mark_pos_unsaved(dim, pos);
            }
            with(dim, |d| d.timers.retain(|t| t.0 != pos));
        }
        // ClipboardBlock: no loot; the clipboard drops with its pages.
        Some(Block::Clipboard) => {
            if let Some(s) = with(dim, |d| d.clipboards.remove(&pos)) {
                lv.drop_at(pos, s);
                persist::mark_pos_unsaved(dim, pos);
            }
        }
        // A crate's supply.
        Some(Block::CreativeCrate) => {
            if with(dim, |d| d.supplies.remove(&pos).is_some()) {
                persist::mark_pos_unsaved(dim, pos);
            }
        }
        // A hatch's filter: a filter item drops (`FilteringBehaviour.destroy`).
        Some(Block::ItemHatch) => {
            if let Some(f) = with(dim, |d| d.hatch_filters.remove(&pos)) {
                persist::mark_pos_unsaved(dim, pos);
                if f.is_filter_item()
                    && let Some(s) = crate::glue::items::filters::filter_stack(&*lv.host, &f)
                {
                    lv.drop_at(pos, s);
                }
            }
        }
        _ => {}
    }
    // A girder joins what is beside it when it is placed by other means than an item.
    if matches!(
        is,
        Some(Block::MetalGirder | Block::MetalGirderEncasedShaft)
    ) {
        with(dim, |d| d.girder_checks.push(pos));
    }
}

/// `PeculiarBellBlock.tryConvert`: soul fire or a soul campfire below haunts it, with
/// `AllSoundEvents.HAUNTED_BELL_CONVERT`.
fn haunt_bell(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let Some(state) = lv.block_state(pos) else {
        return;
    };
    if block_of(lv.block(pos)).map(|(b, _)| b) != Some(Block::PeculiarBell) {
        return;
    }
    let below = lv
        .block_state([pos[0], pos[1] - 1, pos[2]])
        .map(|s| s.block().id(&*lv.host))
        .unwrap_or_default();
    if !decor::haunts(&below) {
        return;
    }
    let text = super::state_text(&*lv.host, state);
    let (_, props) = states::split(&text);
    let Some(haunted) = super::state_with(&*lv.host, Block::HauntedBell, &props) else {
        return;
    };
    lv.put(pos, haunted);
    lv.block_sound("create:haunted_bell_convert", pos, 1.0, 1.0);
}

/// Large water wheels: a placed one (`onPlace`'s tick) builds its frame of structure blocks,
/// breaking without drops when one can't go; a broken one takes its frame; a broken frame block
/// breaks its wheel with drops (`WaterWheelStructuralBlock.onRemove`). `extension` follows a
/// wheel on its negative side (`updateShape`).
fn large_wheels(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let Some((checks, gone, frames)) = with_world(|w| {
        let d = &mut w.dims[dim];
        if d.large_wheel_checks.is_empty()
            && d.large_wheels_gone.is_empty()
            && d.wheel_structures_gone.is_empty()
        {
            return None;
        }
        Some((
            std::mem::take(&mut d.large_wheel_checks),
            std::mem::take(&mut d.large_wheels_gone),
            std::mem::take(&mut d.wheel_structures_gone),
        ))
    })
    .flatten() else {
        return;
    };
    let air = lv.host.block_state_id("minecraft:air");
    for (pos, axis) in gone {
        // `stillValid` no longer holds for its frame: each goes (`tick`: to air).
        for (o, f) in create_engine::waterwheel::structure(axis) {
            let p = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
            let is_frame = block_of(lv.block(p))
                .is_some_and(|(b, i)| b == Block::WaterWheelStructure && i.facing == Some(f));
            if is_frame && let Some(air) = air {
                lv.put(p, air);
            }
        }
    }
    for (pos, facing) in frames {
        let p1 = add(pos, facing);
        let master = match block_of(lv.block(p1)) {
            Some((Block::LargeWaterWheel, _)) => Some(p1),
            Some((Block::WaterWheelStructure, i)) => i.facing.map(|f| add(p1, f)).filter(|p| {
                block_of(lv.block(*p)).is_some_and(|(b, _)| b == Block::LargeWaterWheel)
            }),
            _ => None,
        };
        if let Some(m) = master {
            // `level.destroyBlock(master, true)`.
            lv.destroy_block(m, true);
        }
    }
    for pos in checks {
        let Some((Block::LargeWaterWheel, info)) = block_of(lv.block(pos)) else {
            continue;
        };
        let Some(axis) = info.axis else { continue };
        let mut broke = false;
        for (o, f) in create_engine::waterwheel::structure(axis) {
            let p = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
            let Some(want) = super::state_with(
                &*lv.host,
                Block::WaterWheelStructure,
                &[("facing", super::super::kinetics::dir_name(f))],
            ) else {
                continue;
            };
            let Some(here) = lv.block_state(p) else {
                continue;
            };
            if here == want {
                continue;
            }
            if lv.host.block_states().replaceable(here) != Some(true) {
                // `level.destroyBlock(pos, false)`.
                lv.destroy_block(pos, false);
                broke = true;
                break;
            }
            lv.put(p, want);
        }
        if broke {
            continue;
        }
        // `updateShape` toward its negative side: an extension of the wheel there.
        let behind = add(pos, Dir::from_axis(axis, false));
        let extension =
            block_of(lv.block(behind)).is_some_and(|(b, _)| b == Block::LargeWaterWheel);
        lv.set_property(pos, "extension", flag(extension), update_all());
    }
}
