//! The amount board of brass funnels' and smart chutes' filter slots (`FilteringBehaviour`'s
//! `count` and `upTo`, shown `showCountWhen` the block extracts): held use on the slot opens an
//! "Up to" / "Exactly" board of 1 to the filter's stack size (`interaction`'s board), a short
//! click sets the filter; the block then takes that many at once (`getAmountToExtract`,
//! `getModeToExtract`). The setting lives in the engine's `filter_amounts`, saved with its
//! column. A smart chute's client tells whether it extracts as if its stack falls (it cannot
//! see the fans' pull); the server's answer is the one that counts.

use super::super::items::filters;
use super::super::server::ServerLevel;
use super::super::states;
use super::super::world::with_world;
use super::items;
use create_engine::Block;
use create_engine::board::FilterAmount;
use create_engine::filter::Filter;
use create_engine::funnel::BeltShape;
use create_engine::kinetics::Dir;
use mod_api::{ClientHost, Host, HostBlockStates, PlayerRef, Tag};

/// `getMaxStackSize(filter)`: 64 for an empty slot, else the filter stack's size.
pub(crate) fn filter_max(host: &dyn Host, filter: Option<&Filter>) -> i32 {
    filter
        .and_then(|f| filters::filter_stack(host, f))
        .map_or(64, |s| {
            i32::from(items::stack_limit(host, &items::id_of(host, &s)))
        })
}

/// The amount set on the filter slot at `pos` (up to a stack without one).
fn amount_at(dim: usize, pos: [i32; 3]) -> FilterAmount {
    with_world(|w| w.dims[dim].filter_amounts.get(&pos).copied())
        .flatten()
        .unwrap_or_default()
}

/// `FunnelBlockEntity.supportsAmountOnFilter`: a belt funnel pushing onto its belt or over a
/// belt, an extracting funnel. `belt_below`: a belt segment under it.
fn funnel_supports(belt: Option<BeltShape>, extracting: bool, belt_below: bool) -> bool {
    match belt {
        Some(BeltShape::Pushing) => true,
        Some(_) => belt_below,
        None => extracting,
    }
}

/// `SmartChuteBlockEntity.isExtracting`: the block its stacks come from (above while they fall,
/// below while fans blow them up) is neither a chute nor replaceable.
fn chute_extracting(lv: &ServerLevel<'_>, pos: [i32; 3]) -> bool {
    let up = super::chute::chute_motion(lv, pos) < 0.0;
    let from = [pos[0], pos[1] + if up { 1 } else { -1 }, pos[2]];
    if super::chute::chute_at(lv, from).is_some() {
        return false;
    }
    lv.block_state(from)
        .is_some_and(|s| lv.host.block_states().replaceable(s) != Some(true))
}

/// `FilteringBehaviour.isCountVisible` of the block at `pos` (`acceptsValueSettings`): a brass
/// funnel that supports an amount, a smart chute that extracts, with a filter that stacks. The
/// board's top value (the filter's stack size) when it shows.
pub(crate) fn shown(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<i32> {
    let (block, _) = super::info_at(lv, pos)?;
    let visible = match block {
        Block::BrassFunnel | Block::BrassBeltFunnel => {
            let f = super::funnel::funnel_at(lv, pos)?;
            let below = [pos[0], pos[1] - 1, pos[2]];
            let belt_below = super::belt::belt_segment(lv, below).is_some();
            funnel_supports(f.belt, f.extracting, belt_below)
        }
        Block::SmartChute => chute_extracting(lv, pos),
        _ => false,
    };
    if !visible {
        return None;
    }
    let dim = lv.dim;
    let filter = with_world(|w| w.dims[dim].chute_filters.get(&pos).cloned()).flatten();
    let max = filter_max(&*lv.host, filter.as_ref());
    (max > 1).then_some(max)
}

/// `FilteringBehaviour.getValueSettings` of the filter slot at `pos` while its count shows
/// (`acceptsValueSettings`).
pub(crate) fn settings(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
) -> Option<create_engine::board::Settings> {
    let max = shown(lv, pos)?;
    Some(amount_at(lv.dim, pos).settings(max))
}

/// `FunnelBlockEntity.getAmountToExtract` and `getModeToExtract` for the funnel `f` at `pos`:
/// a stack when it supports no amount, one item at a time for an andesite funnel (its filter
/// is never active), else the board's count, exactly on its second row.
pub(crate) fn funnel_extraction(
    lv: &ServerLevel<'_>,
    pos: [i32; 3],
    f: &super::funnel::FunnelState,
) -> (u32, bool) {
    let below = [pos[0], pos[1] - 1, pos[2]];
    let belt_below = f.belt.is_some() && super::belt::belt_segment(lv, below).is_some();
    if !funnel_supports(f.belt, f.extracting, belt_below) {
        return (64, false);
    }
    if !f.brass {
        return (1, false);
    }
    let dim = lv.dim;
    let filter = with_world(|w| w.dims[dim].chute_filters.get(&pos).cloned()).flatten();
    let a = amount_at(dim, pos);
    let count = a.count(filter_max(&*lv.host, filter.as_ref()));
    (count.max(1) as u32, !a.up_to)
}

/// `SmartChuteBlockEntity.getExtractionAmount` and `getExtractionMode`: the board's count
/// while it shows, exactly on its second row; else up to a stack.
pub(crate) fn chute_extraction(lv: &ServerLevel<'_>, pos: [i32; 3]) -> (u32, bool) {
    let Some(max) = shown(lv, pos) else {
        return (create_engine::chute::SMART_AMOUNT, false);
    };
    let a = amount_at(lv.dim, pos);
    (a.count(max).max(1) as u32, !a.up_to)
}

/// `ValueSettingsPacket` on a filter slot (only while its board shows, `acceptsValueSettings`):
/// a short click (`onShortInteract`) sets the filter from the main hand, as the click would
/// have, but for a wrench or an arm (`canShortInteract`); a pick (`setValueSettings`) sets the
/// amount, with Create's feedback sound when it changed. The setting as `create:value` carries
/// it (`FilterAmount::word`), None for no filter slot here.
pub(crate) fn value_settings(
    lv: &mut ServerLevel<'_>,
    pos: [i32; 3],
    player: PlayerRef,
    pick: Option<create_engine::board::Settings>,
    short: bool,
) -> Option<i32> {
    let (block, _) = super::info_at(lv, pos)?;
    let brass_funnel = matches!(block, Block::BrassFunnel | Block::BrassBeltFunnel);
    if !brass_funnel && block != Block::SmartChute {
        return None;
    }
    let max = shown(lv, pos)?;
    let dim = lv.dim;
    if short {
        let held = super::super::items::held(&*lv.host, player, mod_api::Hand::Main)
            .map(|s| items::id_of(&*lv.host, &s));
        let bypass = held
            .as_deref()
            .is_some_and(|h| matches!(h, "create:wrench" | "create:mechanical_arm"));
        if !bypass {
            if brass_funnel {
                super::funnel::set_funnel_filter(lv, pos, player, mod_api::Hand::Main);
            } else {
                super::chute::set_smart_chute_filter(lv, pos, player, mod_api::Hand::Main);
            }
        }
    } else if let Some(pick) = pick {
        let now = amount_at(dim, pos);
        let next = FilterAmount::from_settings(pick, max);
        if now.settings(max) != next.settings(max) {
            with_world(|w| {
                let d = &mut w.dims[dim];
                d.filter_amounts.insert(pos, next);
                d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
                d.chute_pulls.wake(pos);
            });
            super::funnel::wake_funnel(dim, pos);
            feedback_sound(lv, pos);
        }
    }
    // The stack size may have changed with the filter.
    let max = shown(lv, pos).unwrap_or(max);
    Some(amount_at(dim, pos).word(max))
}

/// `ValueSettingsBehaviour.playFeedbackSound`.
fn feedback_sound(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let at = pos.map(|v| f64::from(v) + 0.5);
    let blocks = mod_api::SoundSource::Blocks;
    let sounds = [
        ("minecraft:entity.item_frame.add_item", 0.25, 2.0),
        ("minecraft:block.note_block.iron_xylophone", 0.03, 1.125),
    ];
    for (sound, volume, pitch) in sounds {
        lv.level
            .play_sound(&mut *lv.host, sound, at, blocks, volume, pitch);
    }
}

/// The block at `pos` of the level this client shows, with its state facts.
fn client_info(
    host: &dyn ClientHost,
    pos: [i32; 3],
) -> Option<(Block, create_engine::parts::StateInfo)> {
    let state = host
        .level(host.dimension())
        .and_then(|l| l.block_state(host, pos))
        .map_or(0, states::id_or_none);
    if state == 0 {
        return None;
    }
    super::super::client_table(host, |t| t.get(state).map(|(b, i)| (b, *i))).flatten()
}

/// The filter slot's stack size as this client sees it (the filter its block entity shows).
fn client_filter_max(host: &dyn ClientHost, pos: [i32; 3]) -> i32 {
    let id = host
        .level(host.dimension())
        .and_then(|l| host.block_entity(l, pos))
        .and_then(|e| e.data(host))
        .and_then(|d| match d.get(&super::FILTER.into()) {
            Some(Tag::String(s)) => Some(s.to_string_lossy()),
            _ => None,
        })
        .filter(|s| !s.is_empty());
    id.map_or(64, |id| i32::from(items::stack_limit(host, &id)))
}

/// `isCountVisible` as this client sees it: the board's top value when it shows (`shown`; a
/// smart chute's stacks taken to fall).
pub(crate) fn client_shown(host: &dyn ClientHost, pos: [i32; 3]) -> Option<i32> {
    let (block, info) = client_info(host, pos)?;
    let below = [pos[0], pos[1] - 1, pos[2]];
    let visible = match block {
        Block::BrassFunnel => funnel_supports(None, info.extracting, false),
        Block::BrassBeltFunnel => {
            let belt_below = client_info(host, below).is_some_and(|(b, _)| b == Block::Belt);
            funnel_supports(
                Some(info.belt_funnel.unwrap_or_default()),
                false,
                belt_below,
            )
        }
        Block::SmartChute => {
            let above = [pos[0], pos[1] + 1, pos[2]];
            let chute = client_info(host, above)
                .is_some_and(|(b, _)| matches!(b, Block::Chute | Block::SmartChute));
            let replaceable = host
                .level(host.dimension())
                .and_then(|l| l.block_state(host, above))
                .is_none_or(|s| host.block_states().replaceable(s) == Some(true));
            !chute && !replaceable
        }
        _ => false,
    };
    if !visible {
        return None;
    }
    let max = client_filter_max(host, pos);
    (max > 1).then_some(max)
}

/// Whether a hit at `point` on face `face` of the block at `pos` lands on a filter slot of a
/// brass funnel, smart chute or brass tunnel side, as this client sees it.
pub(crate) fn client_filter_slot_hit(
    host: &dyn ClientHost,
    pos: [i32; 3],
    face: [i32; 3],
    point: [f64; 3],
) -> bool {
    let Some((block, info)) = client_info(host, pos) else {
        return false;
    };
    let Some(side) = Dir::from_offset(face) else {
        return false;
    };
    let local = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]));
    super::client::filter_slot(block, &info, side)
        .is_some_and(|s| create_engine::board::hits_box(s, local))
}

/// The filter slot's hover tip (`FilteringRenderer.tick`): "Filter", how to set it, and with
/// its amount board how to open it.
pub(crate) fn client_tip(host: &dyn ClientHost, pos: [i32; 3]) -> Vec<String> {
    let t = |key: &str, en: &str| host.translate(key).unwrap_or_else(|| en.to_owned());
    let filled = host
        .level(host.dimension())
        .and_then(|l| host.block_entity(l, pos))
        .and_then(|e| e.data(host))
        .is_some_and(|d| {
            matches!(d.get(&super::FILTER.into()), Some(Tag::String(s)) if !s.to_string_lossy().is_empty())
        });
    let mut lines = vec![t("create.logistics.filter", "Filter")];
    lines.push(if filled {
        t(
            "create.logistics.filter.click_to_replace",
            "Click with item to replace",
        )
    } else {
        t(
            "create.logistics.filter.click_to_set",
            "Click with item to set",
        )
    });
    if client_shown(host, pos).is_some() {
        lines.push(t(
            "create.logistics.filter.hold_to_set_amount",
            "Click and hold for amount",
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn funnels_take_an_amount_where_create_s_do() {
        // An extracting funnel on an inventory; an inserting one never.
        assert!(funnel_supports(None, true, false));
        assert!(!funnel_supports(None, false, true));
        // A belt funnel pushing onto its belt, or any belt funnel over a belt (not a depot).
        assert!(funnel_supports(Some(BeltShape::Pushing), false, false));
        assert!(funnel_supports(Some(BeltShape::Pulling), false, true));
        assert!(!funnel_supports(Some(BeltShape::Retracted), false, false));
    }
}
