//! The placement helpers' indicator round the crosshair (catnip's `PlacementClient`): while a
//! held item's placement helper would place a block from the one looked at, an arrow beside the
//! crosshair points toward where it would go, fading in over 10 ticks and out at twice that
//! pace. The helpers are the server's (their rules read the server's world), so the server finds
//! each player's target as `PlacementClient.checkHelpers` does (not sneaking; the main hand, then
//! the off hand) and sends it when it changes.
//!
//! The arrow is Ponder's `TRIANGLE` indicator (`fadedArrow`: a fan from a tip 20 pixels out, its
//! colour 0.8 grey, fading to nothing at its base), turned toward the target and chasing it
//! (`LerpedFloat.angular`, `Chaser.EXP` at 0.25). Ponder's default is `TEXTURE`, a sprite sheet
//! of 16 arrows in Ponder's assets, which are not Create's and are not shipped with the port.

use super::super::net;
use super::super::server::ServerLevel;
use mod_api::{ClientHost, DrawContext, Hand, PlayerRef, ServerHost};
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

thread_local! {
    /// The target last sent to each player.
    static SENT: RefCell<HashMap<u128, Option<[i32; 3]>>> = RefCell::new(HashMap::new());
}

/// `PlacementClient.checkHelpers` for `player`, on the server: the first helper of the main
/// hand's item, then the off hand's, with a target from the block looked at.
fn target(host: &mut dyn ServerHost, player: PlayerRef) -> Option<[i32; 3]> {
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
        super::super::transmission::helper_target(&lv, pos, point, &id)
            .or_else(|| super::super::copycat::helper_target(&lv, pos, point, &id))
            .or_else(|| super::super::display::helper_target(&lv, pos, point, &id))
            .or_else(|| super::super::decor::helper_target(&lv, player, pos, point, item))
    })
}

/// Each player's tick: their target, sent when it changed.
pub(super) fn player_tick(host: &mut dyn ServerHost, player: PlayerRef) {
    let now = target(host, player);
    let key = player.uuid();
    if SENT.with_borrow(|s| s.get(&key).copied().unwrap_or(None)) == now {
        return;
    }
    SENT.with_borrow_mut(|s| s.insert(key, now));
    let mut out = vec![u8::from(now.is_some())];
    for v in now.unwrap_or_default() {
        out.extend_from_slice(&v.to_le_bytes());
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
    /// `PlacementClient.angle`: its value and its value a tick ago.
    static ANGLE: Cell<(f32, f32)> = const { Cell::new((0.0, 0.0)) };
}

pub(super) fn reset() {
    TARGET.set(None);
    LAST.set(None);
    ANIMATION.set(0);
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
}

/// The target's direction on the screen from its middle, as GUI pixels go (right, down), and
/// whether it is behind the camera.
fn on_screen(host: &dyn ClientHost, target: [i32; 3]) -> Option<[f32; 2]> {
    let camera = host.camera()?;
    let eye = [
        camera.position[0],
        camera.position[1] + f64::from(camera.eye_height),
        camera.position[2],
    ];
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

/// `PlacementClient.tick`'s animation and `drawDirectionIndicator`'s angle, each client tick.
pub(super) fn tick(host: &dyn ClientHost) {
    let a = ANIMATION.get();
    if TARGET.get().is_none() {
        ANIMATION.set(a.saturating_sub(2));
    } else if a < 10 {
        ANIMATION.set(a + 1);
    }
    let Some(last) = LAST.get() else { return };
    let Some(d) = on_screen(host, last) else {
        return;
    };
    if d[0] == 0.0 && d[1] == 0.0 {
        return;
    }
    // Clockwise from up, in degrees.
    let target = d[0].atan2(-d[1]).to_degrees();
    let (value, _) = ANGLE.get();
    let next = if ANIMATION.get() < 10 {
        target
    } else {
        // `AngleHelper.angleLerp`-style chase: the short way round, a quarter each tick.
        let diff = (target - value + 540.0).rem_euclid(360.0) - 180.0;
        value + diff * 0.25
    };
    ANGLE.set((next, value));
}

/// `onRenderCrosshairOverlay` with `fadedArrow`.
pub(super) fn draw(host: &dyn ClientHost, draw: &mut dyn DrawContext) {
    let a = ANIMATION.get();
    if a == 0 || host.local_player().is_none() {
        return;
    }
    let progress = (a as f32 / 10.0).min(1.0);
    let alpha = progress * progress;
    let [w, h] = draw.size();
    let centre = [w / 2.0, h / 2.0];
    let (value, before) = ANGLE.get();
    let partial = host.camera().map_or(1.0, |c| c.partial_tick);
    let diff = (value - before + 540.0).rem_euclid(360.0) - 180.0;
    let angle = (before + diff * partial).to_radians();
    let (sin, cos) = angle.sin_cos();
    // Turned clockwise on the screen (y down).
    let turn = |p: [f32; 2]| {
        [
            centre[0] + p[0] * cos - p[1] * sin,
            centre[1] + p[0] * sin + p[1] * cos,
        ]
    };
    let length = 10.0;
    let tip = [0.0f32, -(10.0 + length)];
    let base: [[f32; 2]; 7] = [
        [-9.0, -3.0],
        [-6.0, -6.0],
        [-3.0, -8.0],
        [0.0, -8.5],
        [3.0, -8.0],
        [6.0, -6.0],
        [9.0, -3.0],
    ];
    // The fan's colour fades from the tip to the base: drawn as bands across it.
    const BANDS: usize = 8;
    let at = |p: [f32; 2], t: f32| [tip[0] + (p[0] - tip[0]) * t, tip[1] + (p[1] - tip[1]) * t];
    for k in 0..BANDS {
        let (t0, t1) = (k as f32 / BANDS as f32, (k + 1) as f32 / BANDS as f32);
        let band_alpha = alpha * (1.0 - (k as f32 + 0.5) / BANDS as f32);
        let argb = (((band_alpha * 255.0).round() as u32) << 24) | 0x00CC_CCCC;
        for i in 0..base.len() - 1 {
            let (p, q) = (base[i], base[i + 1]);
            draw.quad(
                [
                    turn(at(p, t0)),
                    turn(at(q, t0)),
                    turn(at(q, t1)),
                    turn(at(p, t1)),
                ],
                argb,
            );
        }
    }
}
