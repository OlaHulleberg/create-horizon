//! The display link's bulb flashing (`LinkBulbRenderer`): each time a link sends its source's
//! data (`sendPulseNextSync`), its glow starts at 2 and halves each tick (`Chaser.EXP` at 0.5),
//! and while it is at least 1/8 the link draws its tube full bright over the bulb, and the glow
//! model (`DISPLAY_LINK_GLOW`, read by `models`) added over it in a fading grey: world quads of
//! the glue's additive pipeline (`pipelines`), placed as the block entity's pose would place
//! them, as a block entity's own drawing has no additive layer. The server puts the tick of each pulse in
//! the link's block entity data (`Pulse`).

use super::super::pipelines::{Pose, Textured};
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
    /// The links flashing now: their pulse's tick and the pose their bulb is drawn in.
    static GLOWING: RefCell<HashMap<[i32; 3], (i64, Pose)>> = RefCell::new(HashMap::new());
}

pub(super) fn forget() {
    DONE.with_borrow_mut(HashMap::clear);
    GLOWING.with_borrow_mut(HashMap::clear);
}

/// `getGlow`: 2 at the pulse, halved each tick.
fn glow_at(since: f32) -> f32 {
    2.0 * 0.5f32.powf(since.max(0.0))
}

/// The glows of the flashing links (`DISPLAY_LINK_GLOW`, `color(c, c, c, 255)`,
/// `RenderTypes.additive()`, full bright, no diffuse shading).
pub(super) fn render_level_stage(
    host: &dyn ClientHost,
    event: &mut mod_api::RenderLevelStageEvent<'_>,
) {
    if event.stage != mod_api::RenderLevelStage::AfterTranslucentBlocks {
        return;
    }
    let now = event.view.game_time;
    let partial = event.view.partial_tick;
    let Some(glow_model) = super::super::models::get(host, super::super::models::DISPLAY_LINK_GLOW)
    else {
        return;
    };
    let mut quads = Vec::new();
    GLOWING.with_borrow_mut(|g| {
        g.retain(|pos, (t0, pose)| {
            let still = host
                .level(host.dimension())
                .and_then(|l| l.block_state(host, *pos))
                .and_then(super::super::states::id)
                .and_then(|id| {
                    super::super::client_table(host, |t| t.get(id).map(|(b, _)| b)).flatten()
                })
                == Some(Block::DisplayLink);
            let glow = glow_at((now - *t0) as f32 + partial);
            if !still || glow < 0.125 {
                return false;
            }
            let g = (1.0 - 2.0 * (glow - 0.75).powi(2)).clamp(-1.0, 1.0);
            let c = (200.0 * g).max(0.0) as u32;
            let color = 0xFF00_0000 | c << 16 | c << 8 | c;
            super::super::models::quads(pose, &glow_model, color, &mut quads);
            true
        });
    });
    if !quads.is_empty()
        && let Some(p) = super::super::pipelines::resolve(host, Textured::LinkGlow)
    {
        let _ = event.draw.pipeline_quads(p, &quads);
    }
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
        let glow = glow_at(since);
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
        let pos = event.pos;
        let pose = Pose::at(pos.map(f64::from))
            .rotate(
                [0.0, 1.0, 0.0],
                f64::from(create_engine::decor::horizontal_angle(face) + 180.0),
                [0.5; 3],
            )
            .rotate([1.0, 0.0, 0.0], f64::from(-vertical - 90.0), [0.5; 3])
            .translate(offset.map(f64::from));
        GLOWING.with_borrow_mut(|g| g.insert(pos, (t0, pose)));
        let light = draw.light();
        draw.set_light(FULL_BRIGHT);
        draw.block(tube, &[("facing", "up")]);
        draw.set_light(light);
        draw.pop();
    }
}
