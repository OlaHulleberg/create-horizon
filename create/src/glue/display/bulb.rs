//! The display link's bulb flashing (`LinkBulbRenderer`): each time a link sends its source's
//! data (`sendPulseNextSync`), its glow starts at 2 and halves each tick (`Chaser.EXP` at 0.5),
//! and while it is at least 1/8 the link draws its tube full bright over the bulb. The glow
//! model drawn additively in a fading grey on top is left out (see `HORIZON_API.md`). The
//! server puts the tick of each pulse in the link's block entity data (`Pulse`).

use super::super::server::ServerLevel;
use super::super::states;
use create_engine::Block;
use create_engine::kinetics::Dir;
use mod_api::{BlockEntityPart, BlockEntitySubmitEvent, BlockPart, ClientHost, Compound, Tag};
use std::cell::RefCell;
use std::collections::HashMap;

/// The key of the last pulse's game tick in a display link's block entity data.
pub const PULSE: &str = "Pulse";

/// Light at full sky and block light (`LightTexture.FULL_BRIGHT`).
const FULL_BRIGHT: u32 = 0x00F0_00F0;

thread_local! {
    /// Each link's pulse a client has drawn to its end, by position.
    static DONE: RefCell<HashMap<[i32; 3], i64>> = RefCell::new(HashMap::new());
}

pub(super) fn forget() {
    DONE.with_borrow_mut(HashMap::clear);
}

/// The link at `pos` sent its data now.
pub(super) fn pulse(lv: &mut ServerLevel<'_>, pos: [i32; 3]) {
    let now = lv.game_time();
    if let Some(entity) = lv.host.block_entity(lv.level, pos) {
        let data = Compound::from([(PULSE.into(), Tag::Long(now))]);
        let _ = entity.merge_data(&mut *lv.host, data);
    }
}

fn pulse_of(nbt: &Compound) -> Option<i64> {
    match super::super::kinetics::synced(nbt, PULSE) {
        Some(Tag::Long(t)) => Some(*t),
        _ => None,
    }
}

/// Whether the link at `pos` still flashes.
pub(super) fn animated(pos: [i32; 3], entity: &mod_api::ModBlockEntity) -> bool {
    pulse_of(&entity.nbt).is_some_and(|t| DONE.with_borrow(|d| d.get(&pos) != Some(&t)))
}

pub(super) struct BulbDraw;

#[mod_api::horizon_part]
impl BlockPart for BulbDraw {}

#[mod_api::horizon_part]
impl BlockEntityPart for BulbDraw {
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        if event.state.block() != states::key(Block::DisplayLink) {
            return;
        }
        let Some(t0) = pulse_of(&event.entity.nbt) else {
            return;
        };
        let draw = &mut *event.draw;
        let since = (draw.game_time() - t0) as f32 + draw.partial();
        // `getGlow`: 2 at the pulse, halved each tick.
        let glow = 2.0 * 0.5f32.powf(since.max(0.0));
        if glow < 0.125 {
            DONE.with_borrow_mut(|d| d.insert(event.pos, t0));
            return;
        }
        let Some(id) = states::id(event.state) else {
            return;
        };
        let face = super::super::client_table(host, |t| t.get(id).and_then(|(_, i)| i.facing))
            .flatten()
            .unwrap_or(Dir::Up);
        let Some(tube) = states::partial("part_display_link_tube") else {
            return;
        };
        let vertical = match face {
            Dir::Up => -90.0,
            Dir::Down => 90.0,
            _ => 0.0,
        };
        // `getBulbOffset`.
        let offset = if face.axis() == create_engine::kinetics::Axis::Y {
            [5.0, 7.0, 11.0]
        } else {
            [11.0, 7.0, 5.0]
        }
        .map(|v: f32| v / 16.0);
        draw.push();
        draw.rotate_about(
            [0.0, 1.0, 0.0],
            create_engine::decor::horizontal_angle(face) + 180.0,
            [0.5; 3],
        );
        draw.rotate_about([1.0, 0.0, 0.0], -vertical - 90.0, [0.5; 3]);
        draw.translate(offset);
        let light = draw.light();
        draw.set_light(FULL_BRIGHT);
        draw.block(tube, &[("facing", "up")]);
        draw.set_light(light);
        draw.pop();
    }
}
