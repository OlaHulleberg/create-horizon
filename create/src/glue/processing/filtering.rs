//! The filter slots of the processing machines (`FilteringBehaviour`): the basin's
//! (`BasinValueBox`, on its sides) and the upward saw's (`SawFilterSlot`, on its top) choose
//! which recipes run by their result (`forRecipes`); the deployer's (`DeployerFilterSlot`, on
//! the sides off its axes) keeps only what passes in its hand and holds on to it. A click with
//! an item sets the filter (`onShortInteract`), an empty hand clears it; the filter is drawn in
//! the slot (`FilteringRenderer`). The filters are kept with the logistics blocks' (the
//! engine's `chute_filters`), saved with them and dropped when the block goes.

use crate::glue::items::filters;
use crate::glue::server::ServerLevel;
use crate::glue::world::with_world;
use create_engine::Block;
use create_engine::filter::Filter;
use create_engine::kinetics::{Axis, Dir};
use create_engine::parts::StateInfo;

/// The centre of the filter slot on `side` of a `block` in state `info` (`flipped`: a saw's
/// `FLIPPED`), in the block; None where it has none.
pub fn slot(block: Block, info: &StateInfo, flipped: bool, side: Dir) -> Option<[f64; 3]> {
    match block {
        // `BasinValueBox`: (8, 12, 16.05) turned to each horizontal side.
        Block::Basin => {
            if side.axis() == Axis::Y {
                return None;
            }
            let o = side.offset();
            let out = 0.05 / 16.0 + 0.5;
            Some([
                0.5 + f64::from(o[0]) * out,
                0.75,
                0.5 + f64::from(o[2]) * out,
            ])
        }
        // `SawFilterSlot`: on top of a saw facing up, 3 px off the middle across its blade
        // (the other way when flipped); any face reaches it.
        Block::MechanicalSaw => {
            if info.facing != Some(Dir::Up) {
                return None;
            }
            let offset = if flipped { -3.0 } else { 3.0 };
            let (x, z) = if info.along_first {
                (8.0 + offset, 8.0)
            } else {
                (8.0, 8.0 + offset)
            };
            Some([x / 16.0, 12.5 / 16.0, z / 16.0])
        }
        // `DeployerFilterSlot`: (8, 8, 15.5) turned to a side off the facing's and the
        // shaft's axes, 2 px back from the facing.
        Block::Deployer => {
            let facing = info.facing?;
            let shaft = create_engine::parts::directional_axis(facing, info.along_first);
            if side.axis() == facing.axis() || side.axis() == shaft {
                return None;
            }
            let o = side.offset();
            let f = facing.offset();
            let out = 7.5 / 16.0;
            Some(std::array::from_fn(|i| {
                0.5 + f64::from(o[i]) * out - f64::from(f[i]) * 2.0 / 16.0
            }))
        }
        _ => None,
    }
}

/// The sides a slot is drawn on, and the slot's turn on each (`ValueBoxTransform.rotate`):
/// y and x degrees.
pub fn drawn(block: Block, info: &StateInfo, flipped: bool) -> Vec<(Dir, [f64; 3], f32, f32)> {
    let mut out = Vec::new();
    if block == Block::MechanicalSaw {
        if let Some(s) = slot(block, info, flipped, Dir::Up) {
            let y = if info.along_first { 90.0 } else { 0.0 } + if flipped { 0.0 } else { 180.0 };
            out.push((Dir::Up, s, y, 90.0));
        }
        return out;
    }
    for side in Dir::ALL {
        let Some(s) = slot(block, info, flipped, side) else {
            continue;
        };
        // `Sided.rotate`: the side's horizontal angle and a half turn; up and down tipped.
        let y = create_engine::decor::horizontal_angle(side) + 180.0;
        let x = match side {
            Dir::Up => 90.0,
            Dir::Down => 270.0,
            _ => 0.0,
        };
        out.push((side, s, y, x));
    }
    out
}

/// Whether a hit at `point` on `face` of the block at `pos` lands on its filter slot.
pub fn hit(
    block: Block,
    info: &StateInfo,
    flipped: bool,
    pos: [i32; 3],
    face: Dir,
    point: [f64; 3],
) -> bool {
    let local = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]));
    // The saw's slot is not sided: any face of it reaches it.
    let side = if block == Block::MechanicalSaw {
        Dir::Up
    } else {
        face
    };
    slot(block, info, flipped, side).is_some_and(|s| create_engine::board::hits_box(s, local))
}

/// The filter of the machine at `pos`.
pub fn filter_at(dim: usize, pos: [i32; 3]) -> Option<Filter> {
    with_world(|w| w.dims[dim].chute_filters.get(&pos).cloned()).flatten()
}

/// `FilteringBehaviour.test` of a recipe's result for a `forRecipes` filter `f` (none: all
/// pass): its first item result, or without one its first fluid result.
pub fn result_passes(
    host: &dyn mod_api::Host,
    f: Option<&Filter>,
    item: Option<&str>,
    fluid: Option<create_engine::tank::Fluid>,
) -> bool {
    let Some(f) = f else {
        return true;
    };
    match (item, fluid) {
        (Some(i), _) => filters::passes_item(host, Some(f), i),
        (None, Some(fl)) => f.test_fluid(fl),
        (None, None) => false,
    }
}

/// `result_passes` for a basin recipe.
pub fn basin_recipe_passes(
    host: &dyn mod_api::Host,
    f: Option<&Filter>,
    r: &create_engine::basin::Recipe,
) -> bool {
    result_passes(
        host,
        f,
        r.results.first().map(|o| o.item.as_str()),
        r.fluid_results.first().map(|x| x.0),
    )
}

/// A player's click on the filter slot of the basin, upward saw or deployer at `pos`
/// (`FilteringBehaviour.onShortInteract` through `ValueSettingsInputHandler`): the held item
/// becomes its filter (a filter item taken from the hand, the one it held given back), an empty
/// hand clears it. True when the click was the slot's.
pub fn click(lv: &mut ServerLevel<'_>, pos: [i32; 3], click: &mod_api::BlockClick<'_>) -> bool {
    let Some((block, info)) = lv.info(pos) else {
        return false;
    };
    let flipped = lv.property(pos, "flipped").as_deref() == Some("true");
    let Some(face) = Dir::from_offset(click.hit.face) else {
        return false;
    };
    if !hit(block, &info, flipped, pos, face, click.hit.point) {
        return false;
    }
    let Some(p) = lv.host.player(click.player) else {
        return false;
    };
    if !crate::glue::interaction::values::may_use_value_box(&*lv.host, p) {
        return false;
    }
    // `canShortInteract`: a wrench or an arm is used on the block instead.
    let held = click.stack.map(|s| super::stacks::id(&*lv.host, s));
    if held
        .as_deref()
        .is_some_and(|h| matches!(h, "create:wrench" | "create:mechanical_arm"))
    {
        return false;
    }
    let filter = filters::take_filter(lv.host, p, click.hand);
    let dim = lv.dim;
    let old = with_world(|w| {
        let d = &mut w.dims[dim];
        let old = match filter {
            Some(f) => d.chute_filters.insert(pos, f),
            None => d.chute_filters.remove(&pos),
        };
        d.unsaved.push((pos[0] >> 4, pos[2] >> 4));
        // `withCallback`: a basin's mixer or press looks again.
        if block == Block::Basin {
            d.mixer_checks.push([pos[0], pos[1] + 2, pos[2]]);
        }
        old
    })
    .flatten();
    let now = filter_at(dim, pos);
    crate::glue::logistics::show_filter(lv, pos, now.as_ref());
    filters::return_filter(lv, p, pos, old);
    true
}
