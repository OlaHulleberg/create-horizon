//! The placement helpers' feedback (catnip's `PlacementClient`): while a held item's placement
//! helper would place a block from the one looked at, the block it would place shows there as a
//! ghost (`ghost.rs`) and an arrow beside the crosshair points toward it, fading in over 10
//! ticks and out at twice that pace. The helpers are the server's (their rules read the
//! server's world), so the server finds each player's target as `PlacementClient.checkHelpers`
//! does (not sneaking; the main hand, then the off hand), with the ghost's state (the held
//! block's default state through the offset's transform, `IPlacementHelper.getOffset`), and
//! sends them when they change.
//!
//! The arrow is Ponder's default `TEXTURE` indicator (`PlacementClient.textured`): a frame of
//! Ponder's sheet of 16 arrows (`create:gui/placement_indicator`, MIT), the angle toward the
//! target chased each frame (`LerpedFloat.angular`, `Chaser.EXP` at 0.25) and snapped to 22.5
//! degrees, 18 GUI pixels square over the crosshair, its alpha the fade-in squared.

use super::super::net;
use super::super::server::ServerLevel;
use mod_api::{
    ClientHost, DrawContext, Hand, Host, HostBlockStates, ItemKey, PlayerRef, ServerHost,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

pub(super) const CHANNEL: &str = "create:placement";

/// The items whose placement helpers have a target (`matchesItem`).
fn helped(id: &str) -> bool {
    matches!(
        id,
        "create:shaft"
            | "create:cogwheel"
            | "create:large_cogwheel"
            | "create:metal_girder"
            | "create:copycat_panel"
            | "create:copycat_step"
            | "create:white_sail"
            | "create:sail_frame"
            | "create:display_board"
    ) || (id.starts_with("create:") && (id.ends_with("_table_cloth") || id.ends_with("_ladder")))
}

/// A target and its ghost's block state.
type Sent = ([i32; 3], String);

thread_local! {
    /// The target and ghost last sent to each player.
    static SENT: RefCell<HashMap<u128, Option<Sent>>> = RefCell::new(HashMap::new());
}

/// The ghost's state (`withGhostState(blockItem.getBlock().defaultBlockState())`, through the
/// offset's transform): `item`'s block's default state with `props` set, as its string.
fn ghost_state(host: &dyn Host, item: ItemKey, props: &[(&str, String)]) -> Option<String> {
    let facts = host.block_states();
    let mut state = host.block_state_default(host.item_block(item)?)?;
    for (k, v) in props {
        state = state.with(&facts, k, v).unwrap_or(state);
    }
    host.block_state_string(state)
}

/// `PlacementClient.checkHelpers` for `player`, on the server: the first helper of the main
/// hand's item, then the off hand's, with a target from the block looked at, and its ghost.
fn target(host: &mut dyn ServerHost, player: PlayerRef) -> Option<Sent> {
    let living = host.living(player.entity())?;
    let stacks: Vec<_> = [Hand::Main, Hand::Off]
        .into_iter()
        .filter_map(|h| living.hand_item(&*host, h))
        .filter(|s| helped(&s.item().id(&*host)))
        .collect();
    if stacks.is_empty() {
        return None;
    }
    // `if (mc.player.isShiftKeyDown()) return`.
    if super::super::items::sneaking(&*host, player) || player.may_build(&*host) != Some(true) {
        return None;
    }
    let reach = living
        .attribute_value(&*host, "minecraft:block_interaction_range")
        .unwrap_or(4.5);
    let hit = player.entity().pick(&*host, reach)?;
    if hit.miss {
        return None;
    }
    let dimension = player.dimension(&*host)?.to_string();
    let lv = ServerLevel::new(host, &dimension)?;
    let (pos, point) = (hit.block, hit.location);
    stacks.iter().find_map(|s| {
        let item = s.item();
        let id = item.id(&*lv.host);
        let (at, props) = super::super::transmission::helper_target(&lv, pos, point, &id)
            .or_else(|| super::super::copycat::helper_target(&lv, pos, point, &id))
            .or_else(|| super::super::display::helper_target(&lv, pos, point, &id))
            .or_else(|| super::super::decor::helper_target(&lv, player, pos, point, item))?;
        Some((at, ghost_state(&*lv.host, item, &props).unwrap_or_default()))
    })
}

/// Each player's tick: their target, sent when it changed.
pub(super) fn player_tick(host: &mut dyn ServerHost, player: PlayerRef) {
    let now = target(host, player);
    let key = player.uuid();
    if SENT.with_borrow(|s| s.get(&key).cloned().unwrap_or(None)) == now {
        return;
    }
    SENT.with_borrow_mut(|s| s.insert(key, now.clone()));
    let mut out = vec![u8::from(now.is_some())];
    if let Some((at, ghost)) = now {
        for v in at {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(ghost.as_bytes());
    }
    net::send_to_player(host, player, CHANNEL, &out);
}

pub(super) fn world_closed() {
    SENT.with_borrow_mut(HashMap::clear);
}

// ---------------------------------------------------------------------------------------------
// The client

thread_local! {
    /// `PlacementClient.target` (the server's), `lastTarget` and `animationTick`.
    static TARGET: Cell<Option<[i32; 3]>> = const { Cell::new(None) };
    static LAST: Cell<Option<[i32; 3]>> = const { Cell::new(None) };
    static ANIMATION: Cell<u32> = const { Cell::new(0) };
    /// `PlacementClient.angle`: its value and its value before its last chase.
    static ANGLE: Cell<(f32, f32)> = const { Cell::new((0.0, 0.0)) };
    /// The ghost's block state at the target.
    static GHOST: RefCell<String> = const { RefCell::new(String::new()) };
}

pub(super) fn reset() {
    TARGET.set(None);
    LAST.set(None);
    ANIMATION.set(0);
    GHOST.with_borrow_mut(String::clear);
}

pub(super) fn from_server(data: &[u8]) {
    let v = |i: usize| {
        data.get(1 + i * 4..5 + i * 4)
            .and_then(|b| b.try_into().ok())
            .map(i32::from_le_bytes)
    };
    let target = match (data.first(), v(0), v(1), v(2)) {
        (Some(1), Some(x), Some(y), Some(z)) => Some([x, y, z]),
        _ => None,
    };
    TARGET.set(target);
    if target.is_some() {
        LAST.set(target);
    }
    let ghost = data
        .get(13..)
        .map(String::from_utf8_lossy)
        .unwrap_or_default();
    GHOST.with_borrow_mut(|g| *g = ghost.into_owned());
}

/// The target's direction on the screen from its middle, as GUI pixels go (right, down), and
/// whether it is behind the camera.
fn on_screen(host: &dyn ClientHost, target: [i32; 3]) -> Option<[f32; 2]> {
    let camera = host.camera()?;
    // `camera.getPosition()`: the camera's position is the eye's.
    let eye = camera.position;
    let v: [f64; 3] = std::array::from_fn(|i| f64::from(target[i]) + 0.5 - eye[i]);
    let (yaw, pitch) = (
        f64::from(camera.yaw).to_radians(),
        f64::from(camera.pitch).to_radians(),
    );
    let forward = [
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    ];
    let right = [-yaw.cos(), 0.0, -yaw.sin()];
    // `right` x `forward`.
    let up = [
        right[1] * forward[2] - right[2] * forward[1],
        right[2] * forward[0] - right[0] * forward[2],
        right[0] * forward[1] - right[1] * forward[0],
    ];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (mut x, mut y) = (dot(v, right), dot(v, up));
    // `if (projTarget.z > 0) target = target.reverse()`: behind the view, the other way.
    if dot(v, forward) < 0.0 {
        (x, y) = (-x, -y);
    }
    Some([x as f32, -y as f32])
}

/// `PlacementClient.tick`'s animation, each client tick.
pub(super) fn tick() {
    let a = ANIMATION.get();
    if TARGET.get().is_none() {
        ANIMATION.set(a.saturating_sub(2));
    } else if a < 10 {
        ANIMATION.set(a + 1);
    }
}

/// `drawDirectionIndicator`'s angle, each frame: the way to the target, clockwise from up in
/// degrees, chased (`angle.chase(targetAngle, .25, EXP)`, `tickChaser`) unless still fading
/// in; the value before the chase (`getValue(0)`) snapped to 22.5 degrees.
fn snapped_angle(host: &dyn ClientHost) -> Option<f32> {
    let d = on_screen(host, LAST.get()?)?;
    if d[0] == 0.0 && d[1] == 0.0 {
        return None;
    }
    let target = d[0].atan2(-d[1]).to_degrees();
    let (mut value, _) = ANGLE.get();
    if ANIMATION.get() < 10 {
        value = target;
    }
    // `AngleHelper.getShortestAngleDiff`, a quarter of it each frame.
    let diff = (target - value + 540.0).rem_euclid(360.0) - 180.0;
    ANGLE.set((value + diff * 0.25, value));
    // `(snapSize * Math.round(angle / snapSize)) % 360`.
    Some((22.5 * (value / 22.5 + 0.5).floor()).rem_euclid(360.0))
}

/// `onRenderCrosshairOverlay` with `textured`: the sheet's frame for the snapped angle (one 16
/// pixel frame a 22.5 degrees, down the sheet, clockwise from up), on a quad from -1 to 1
/// scaled by `indicatorScale` (1) * 0.75 * 12, white at the fade-in squared.
pub(super) fn draw(host: &dyn ClientHost, draw: &mut dyn DrawContext) {
    let a = ANIMATION.get();
    if a == 0 || host.local_player().is_none() {
        return;
    }
    let Some(angle) = snapped_angle(host) else {
        return;
    };
    let progress = (a as f32 / 10.0).min(1.0);
    let alpha = progress * progress;
    let [w, h] = draw.size();
    let half = 0.75 * 12.0;
    let frame = ((angle / 22.5) as i32).rem_euclid(16) as f32;
    let tint = (((alpha * 255.0).round() as u32) << 24) | 0x00FF_FFFF;
    draw.sprite(
        INDICATOR,
        [0.0, frame * 16.0, 16.0, 16.0],
        [w / 2.0 - half, h / 2.0 - half, half * 2.0, half * 2.0],
        Some(tint),
    );
}

/// `PonderGuiTextures.PLACEMENT_INDICATOR_SHEET`.
const INDICATOR: &str = "create:gui/placement_indicator";

/// The ghost at the current target (`GhostBlocks.renderAll`): shown while a helper has one.
pub(super) fn draw_ghost(host: &dyn ClientHost, event: &mut mod_api::RenderLevelStageEvent<'_>) {
    if event.stage != mod_api::RenderLevelStage::AfterTranslucentBlocks {
        return;
    }
    let Some(target) = TARGET.get() else {
        return;
    };
    // `getCurrentAlpha`.
    let progress = (ANIMATION.get() as f32 / 10.0).min(1.0);
    GHOST.with_borrow(|g| {
        if !g.is_empty() {
            super::ghost::draw(host, event, g, target, progress);
        }
    });
}
