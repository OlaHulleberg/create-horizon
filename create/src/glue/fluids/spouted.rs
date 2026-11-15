//! What clients draw of spouts (`SpoutRenderer`): the fluid in the spout, the nozzle's three
//! pieces squeezing while it fills, and the stream it pours, from the spout's contents (`TANK`)
//! and the game time its cycle started (`SPOUT_START`).

use super::super::states;
use super::client::fluid_box;
use super::{SPOUT_START, TANK};
use create_engine::spout::FILLING_TIME;
use create_engine::tank::Codes;
use mod_api::{BlockEntitySubmitEvent, ClientHost, ModBlockEntity, Tag};

/// The game time the spout's cycle started, while it runs.
fn start(entity: &ModBlockEntity) -> Option<i64> {
    match entity.nbt.get(&SPOUT_START.into()) {
        Some(Tag::IntArray(v)) => v.first().map(|t| i64::from(*t)),
        _ => None,
    }
}

/// Whether a spout moves this frame (its cycle runs).
pub(super) fn animated(entity: &ModBlockEntity) -> bool {
    start(entity).is_some()
}

pub(super) fn draw(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let tank = match event.entity.nbt.get(&TANK.into()) {
        Some(Tag::IntArray(v)) => match v.as_slice() {
            &[fluid, amount, capacity] if amount > 0 && capacity > 0 => {
                let keys = super::fluid_keys(&event.entity.nbt, TANK);
                Codes::fluid(&keys, fluid).map(|f| (f, amount, capacity))
            }
            _ => None,
        },
        _ => None,
    };
    if let Some((fluid, amount, capacity)) = tank {
        let level = (amount as f32 / capacity as f32).clamp(0.175, 1.0);
        let (min, max) = (2.5 / 16.0, 2.5 / 16.0 + 11.0 / 16.0);
        let top = min + 11.0 / 16.0 * level;
        fluid_box(
            host,
            &mut *event.draw,
            fluid,
            amount as u32,
            [min, min, min],
            [max, top, max],
        );
    }
    // `processingTicks` and the frame's part of a tick.
    let pt = start(event.entity).map(|s| {
        let draw = &*event.draw;
        let ticks = FILLING_TIME as f32 - (draw.game_time() - s) as f32;
        ticks - draw.partial()
    });
    let progress = pt.map_or(0.0, |pt| (1.0 - (pt - 5.0) / 10.0).clamp(0.0, 1.0));
    let radius = if pt.is_some() {
        (2.0 * progress - 1.0).powi(2) - 1.0
    } else {
        0.0
    };
    if let (Some((fluid, amount, _)), Some(_)) = (tank, pt) {
        let r = radius.abs() / 32.0;
        if r > 0.0 {
            fluid_box(
                host,
                &mut *event.draw,
                fluid,
                amount as u32,
                [0.5 - r, -1.2, 0.5 - r],
                [0.5 + r, 0.0, 0.5 + r],
            );
        }
    }
    let squeeze = match pt {
        None => 0.0,
        Some(pt) if pt < 0.0 => 0.0,
        Some(pt) if pt < 2.0 => -(pt / 2.0),
        Some(pt) if pt < 10.0 => -1.0,
        Some(_) => radius,
    };
    let draw = &mut *event.draw;
    draw.push();
    for name in ["part_spout_top", "part_spout_middle", "part_spout_bottom"] {
        if let Some(part) = states::partial(name) {
            draw.block(part, &[("facing", "up")]);
        }
        draw.translate([0.0, -3.0 * squeeze / 32.0, 0.0]);
    }
    draw.pop();
}
