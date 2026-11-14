//! Glass fluid pipes show what flows through them (`TransparentStraightPipeRenderer`): a pump
//! moving fluid marks the glass pipes on its way (`pipes`), each shows that fluid to its clients
//! (`FLOW`) and stops a few ticks after the flow does. Drawn as Create's steady stream: a box of
//! the fluid along the pipe, 3/16 of a block from its middle (`renderFluidStream`'s radius); the
//! stream's growth as a flow starts is not drawn.

use super::super::server::ServerLevel;
use create_engine::parts::StateInfo;
use create_engine::tank::Fluid;
use mod_api::{BlockEntitySubmitEvent, ClientHost, Compound, Tag};
use std::cell::RefCell;
use std::collections::HashMap;

/// The key of the fluid a glass pipe shows (its index; none: empty) in its block entity's data.
pub const FLOW: &str = "Flow";

/// Ticks a glass pipe keeps showing a flow after it last carried it.
const LINGER: i64 = 4;

thread_local! {
    /// Glass pipes carrying fluid, by level and position: the fluid, when it last went through,
    /// and whether clients were told.
    static FLOWS: RefCell<HashMap<(usize, [i32; 3]), (Fluid, i64, bool)>> =
        RefCell::new(HashMap::new());
}

pub(super) fn reset() {
    FLOWS.with_borrow_mut(HashMap::clear);
}

/// `fluid` went through the glass pipes `way` at game time `now`.
pub(super) fn flowing(dim: usize, way: &[[i32; 3]], fluid: Fluid, now: i64) {
    FLOWS.with_borrow_mut(|f| {
        for p in way {
            let e = f.entry((dim, *p)).or_insert((fluid, now, false));
            if e.0 != fluid {
                *e = (fluid, now, false);
            }
            e.1 = now;
        }
    });
}

/// Glass pipes that started or stopped showing a flow tell their clients.
pub(super) fn send(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    let now = lv.game_time();
    let changed: Vec<([i32; 3], Option<Fluid>)> = FLOWS.with_borrow_mut(|f| {
        if f.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        f.retain(|(d, p), (fluid, at, told)| {
            if *d != dim {
                return true;
            }
            if now - *at > LINGER {
                out.push((*p, None));
                return false;
            }
            if !*told {
                *told = true;
                out.push((*p, Some(*fluid)));
            }
            true
        });
        out
    });
    for (pos, fluid) in changed {
        if !lv.is_loaded(pos) {
            continue;
        }
        let Some(entity) = lv.host.block_entity(lv.level, pos) else {
            continue;
        };
        let v = fluid.map_or_else(Vec::new, |f| vec![i32::from(f.index())]);
        let data = Compound::from([(FLOW.into(), Tag::IntArray(v))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
}

/// The stream in a glass pipe its block entity shows, along the pipe's axis.
pub(super) fn draw(
    host: &dyn ClientHost,
    event: &mut BlockEntitySubmitEvent<'_>,
    info: &StateInfo,
) {
    let Some(Tag::IntArray(v)) = event.entity.nbt.get(&FLOW.into()) else {
        return;
    };
    let Some(fluid) = v
        .first()
        .and_then(|i| Fluid::from_index((*i).clamp(0, 255) as u8))
    else {
        return;
    };
    let Some(axis) = info.axis else {
        return;
    };
    let r = 3.0 / 16.0;
    let (mut min, mut max) = ([0.5 - r; 3], [0.5 + r; 3]);
    let i = match axis {
        create_engine::kinetics::Axis::X => 0,
        create_engine::kinetics::Axis::Y => 1,
        create_engine::kinetics::Axis::Z => 2,
    };
    min[i] = 0.0;
    max[i] = 1.0;
    super::client::fluid_box(host, &mut *event.draw, fluid, 250, min, max);
}
