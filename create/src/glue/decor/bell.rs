//! The desk bell drawn (`DeskBellRenderer`): while it is pressed or rings, its plunger and bell
//! are drawn as Create's partials over the pressed model (which has neither), the plunger
//! pushed in and the bell wobbling for the second after a ding (`DeskBellBlockEntity.animation`:
//! 1 at the ding, chased linearly to 0 at 0.05 a tick). The server puts the tick of each ding in
//! the bell's block entity data (`Ding`, Create's `ding` packet), which clients run on from.

use super::super::server::ServerLevel;
use super::super::states;
use create_engine::Block;
use create_engine::kinetics::Dir;
use mod_api::{BlockEntityPart, BlockEntitySubmitEvent, BlockPart, ClientHost, Compound, Tag};
use std::cell::RefCell;
use std::collections::HashMap;

/// The key of the last ding's game tick in a desk bell's block entity data.
pub const DING: &str = "Ding";

thread_local! {
    /// Each bell's ding a client has drawn to its end, by position.
    static RUNG: RefCell<HashMap<[i32; 3], i64>> = RefCell::new(HashMap::new());
}

pub(super) fn forget() {
    RUNG.with_borrow_mut(HashMap::clear);
}

/// The bell at `pos` rang now (`DeskBellBlockEntity.ding` on the server).
pub(super) fn rang(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let now = lv.game_time();
    if let Some(entity) = lv.host.block_entity(lv.level, pos) {
        let data = Compound::from([(DING.into(), Tag::Long(now))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
}

fn ding_of(nbt: &Compound) -> Option<i64> {
    match super::super::kinetics::synced(nbt, DING) {
        Some(Tag::Long(t)) => Some(*t),
        _ => None,
    }
}

/// Whether the bell at `pos` still rings: a ding not yet drawn to its end.
pub(super) fn animated(pos: [i32; 3], entity: &mod_api::ModBlockEntity) -> bool {
    ding_of(&entity.nbt).is_some_and(|t| RUNG.with_borrow(|r| r.get(&pos) != Some(&t)))
}

/// `AngleHelper.horizontalAngle`.
fn horizontal_angle(d: Dir) -> f32 {
    create_engine::decor::horizontal_angle(d)
}

/// `AngleHelper.verticalAngle`.
fn vertical_angle(d: Dir) -> f32 {
    match d {
        Dir::Up => -90.0,
        Dir::Down => 90.0,
        _ => 0.0,
    }
}

const UP: [f32; 3] = [0.0, 1.0, 0.0];
const EAST: [f32; 3] = [1.0, 0.0, 0.0];
const SOUTH: [f32; 3] = [0.0, 0.0, 1.0];
const CENTRE: [f32; 3] = [0.5; 3];

pub(super) struct BellDraw;

#[mod_api::horizon_part]
impl BlockPart for BellDraw {}

#[mod_api::horizon_part]
impl BlockEntityPart for BellDraw {
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        if event.state.block() != states::key(Block::DeskBell) {
            return;
        }
        let Some(id) = states::id(event.state) else {
            return;
        };
        let Some(info) = super::super::client_table(host, |t| t.get(id).map(|(_, i)| *i)).flatten()
        else {
            return;
        };
        let draw = &mut *event.draw;
        let pos = event.pos;
        // `animation.getValue(partialTicks)`.
        let ding = ding_of(&event.entity.nbt);
        let p = ding.map_or(0.0, |t| {
            let since = (draw.game_time() - t) as f32 + draw.partial();
            (1.0 - 0.05 * since.max(0.0)).clamp(0.0, 1.0)
        });
        if p <= 0.0
            && let Some(t) = ding
        {
            RUNG.with_borrow_mut(|r| r.insert(pos, t));
        }
        if p < 0.004 && !info.powered {
            return;
        }
        let f = 1.0 - 4.0 * ((p - 0.5).max(0.0) - 0.5).powi(2);
        let f2 = p.powf(1.25);
        let facing = info.facing.unwrap_or(Dir::Up);
        // `animationOffset`: a random phase per ding, the same on every frame of it.
        let offset = ding.map_or(0.0, |t| {
            let h =
                (t as u64 ^ (pos[0] as u64).wrapping_mul(31) ^ (pos[2] as u64).wrapping_mul(17))
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15);
            (h >> 40) as f32 / (1u64 << 24) as f32 * std::f32::consts::TAU
        });
        let turn = |draw: &mut dyn mod_api::BlockEntityDrawContext| {
            draw.rotate_about(UP, horizontal_angle(facing), CENTRE);
            draw.rotate_about(EAST, vertical_angle(facing) + 90.0, CENTRE);
        };
        if let Some(plunger) = states::partial("part_desk_bell_plunger") {
            draw.push();
            turn(draw);
            draw.translate([0.0, f * -0.75 / 16.0, 0.0]);
            draw.block(plunger, &[("facing", "up")]);
            draw.pop();
        }
        if let Some(bell) = states::partial("part_desk_bell_bell") {
            let angle = p * std::f32::consts::PI * 4.0 + offset;
            draw.push();
            turn(draw);
            draw.rotate_about(EAST, f2 * 8.0 * angle.sin(), CENTRE);
            draw.rotate_about(SOUTH, f2 * 8.0 * angle.cos(), CENTRE);
            draw.translate(CENTRE);
            draw.scale([0.995; 3]);
            draw.translate(CENTRE.map(|v| -v));
            draw.block(bell, &[("facing", "up")]);
            draw.pop();
        }
    }
}
