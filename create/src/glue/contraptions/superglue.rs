//! The super glue item (`SuperGlueSelectionHandler`, `SuperGlueSelectionPacket`,
//! `SuperGlueRemovalPacket`): a right click on a block picks the first corner, a second click
//! glues the area between them when the blocks there hold together (one charge of the glue
//! outside creative), sneaking forgets the first corner, and a left click with the glue in hand
//! takes the glue off the block hit. The areas are kept and saved as `actors` keeps them.
//!
//! Create picks the corners on its client and draws the selection and glue outlines there;
//! here the clicks are taken on the server (the corner a player picked is the server's), which
//! tells a player holding the glue of their first corner and the glue areas near them
//! (`create:glue`), and their client outlines them as Create's does: each area in Create's
//! passive green, the one looked at highlighted, and the selection from the first corner to the
//! block looked at, their faces covered with `AllSpecialTextures.GLUE` (the glue's textured
//! world pipeline, `pipelines`).

use super::super::items::{action_bar, creative, held, sneaking};
use super::super::persist::mark_unsaved;
use super::super::server::ServerLevel;
use super::super::world::with_world;
use super::{center, found};
use crate::glue::pipelines::Textured;
use create_engine::glue;
use mod_api::{
    EquipmentSlot, Flow, Hand, HostSide, InteractionResult, ItemPart, LeftClickBlockEvent,
    PlayerRef, Registrar, RightClickBlockEvent, ServerHost, SoundSource, TriState, UseOnEvent,
};
use std::cell::RefCell;
use std::collections::HashMap;

pub const ITEM: &str = "create:super_glue";

/// The glue a player holding super glue sees: their first corner and the areas near them,
/// from the server to that player.
pub(super) const CHANNEL: &str = "create:glue";

/// `SuperGlueSelectionHandler`'s colours: `PASSIVE`, `HIGHLIGHT`, `FAIL`.
const PASSIVE: u32 = 0xFF4D_9162;
const HIGHLIGHT: u32 = 0xFF68_C586;
const FAIL: u32 = 0xFFC5_B548;

/// What this client was told: its first corner and the glue areas near it.
type Seen = (Option<[i32; 3]>, Vec<([i32; 3], [i32; 3])>);

thread_local! {
    /// Each player's first corner (`firstPos`), by player.
    static FIRST: RefCell<HashMap<u128, [i32; 3]>> = RefCell::new(HashMap::new());
    /// The client's: what the server last told it.
    static SEEN: RefCell<Option<Seen>> = const { RefCell::new(None) };
}

pub(super) fn forget() {
    FIRST.with_borrow_mut(HashMap::clear);
}

pub(super) fn client_reset() {
    SEEN.with_borrow_mut(|s| *s = None);
}

/// `SuperGlueSelectionHandler.tick`'s scan (`inflate(32, 16, 32)`), from the server: each
/// player of the level holding the glue is told of their first corner and the areas near them,
/// every half second (and at once when they change, `send_glue`).
pub(super) fn tick(lv: &mut ServerLevel<'_>) {
    if lv.game_time() % 10 != 0 {
        return;
    }
    for player in lv.players() {
        if is_glue(&*lv.host, player) {
            send_glue(lv, player);
        }
    }
}

/// Tells `player` their first corner and the glue areas near them.
fn send_glue(lv: &mut ServerLevel<'_>, player: PlayerRef) {
    let Some(at) = player.entity().position(&*lv.host) else {
        return;
    };
    let first = FIRST.with_borrow(|f| f.get(&player.uuid()).copied());
    let dim = lv.dim;
    let near = |g: &glue::Glue| {
        let reach = [32.0, 16.0, 32.0];
        (0..3).all(|i| {
            f64::from(g.max[i]) + 1.0 >= at[i] - reach[i] && f64::from(g.min[i]) <= at[i] + reach[i]
        })
    };
    let areas: Vec<glue::Glue> = with_world(|w| {
        w.dims[dim]
            .glue
            .iter()
            .filter(|g| near(g))
            .copied()
            .collect()
    })
    .unwrap_or_default();
    let mut words = vec![i32::from(first.is_some())];
    words.extend(first.unwrap_or_default());
    for g in &areas {
        words.extend(g.min);
        words.extend(g.max);
    }
    let data: Vec<u8> = words.iter().flat_map(|v| v.to_le_bytes()).collect();
    super::super::net::send_to_player(lv.host, player, CHANNEL, &data);
}

/// What the server told this client (`CHANNEL`).
pub(super) fn from_server(data: &[u8]) {
    let w: Vec<i32> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| i32::from_le_bytes(*b))
        .collect();
    let [has, x, y, z, ref rest @ ..] = w[..] else {
        return;
    };
    let first = (has != 0).then_some([x, y, z]);
    let areas = rest
        .as_chunks::<6>()
        .0
        .iter()
        .map(|a| ([a[0], a[1], a[2]], [a[3], a[4], a[5]]))
        .collect();
    SEEN.with_borrow_mut(|s| *s = Some((first, areas)));
}

/// `SuperGlueSelectionHandler.tick`'s outlines, while the main hand holds the glue: the glue
/// areas near (the one the crosshair is in highlighted), and the selection from the first
/// corner to the block looked at (`getCurrentSelectionBox`; `FAIL` 24 blocks or more away, or
/// sneaking to discard).
pub(super) fn render(
    host: &dyn mod_api::ClientHost,
    event: &mut mod_api::RenderLevelStageEvent<'_>,
) {
    use mod_api::{CrosshairTarget, RenderLevelStage};
    if event.stage != RenderLevelStage::AfterTranslucentBlocks {
        return;
    }
    let Some(player) = host.local_player() else {
        return;
    };
    let holding = held(host, player, Hand::Main).is_some_and(|s| s.item().id(host) == ITEM);
    if !holding {
        return;
    }
    let Some((first, areas)) = SEEN.with_borrow(Clone::clone) else {
        return;
    };
    let hovered = match host.crosshair_target() {
        Some(CrosshairTarget::Block { pos, .. }) => Some([pos.x, pos.y, pos.z]),
        _ => None,
    };
    let inside = |(min, max): &([i32; 3], [i32; 3]), p: [i32; 3]| {
        (0..3).all(|i| min[i] <= p[i] && p[i] <= max[i])
    };
    let mut lines = Vec::new();
    // `withFaceTextures(GLUE, ..)`: the highlighted area's and the selection's faces.
    let mut faces = Vec::new();
    let camera = host.camera().map(|c| c.position);
    if first.is_none() {
        let lit = hovered.and_then(|h| areas.iter().position(|a| inside(a, h)));
        for (i, a) in areas.iter().enumerate() {
            let (colour, width) = if Some(i) == lit {
                box_faces(&mut faces, *a, HIGHLIGHT, camera);
                (HIGHLIGHT, 4.0)
            } else {
                (PASSIVE, 1.5)
            };
            box_lines(&mut lines, *a, colour, width);
        }
    }
    if let (Some(f), Some(h)) = (first, hovered) {
        let d2: i64 = (0..3).map(|i| i64::from(f[i] - h[i]).pow(2)).sum();
        let ok = d2 < 24 * 24 && !sneaking(host, player);
        let min = [0, 1, 2].map(|i| f[i].min(h[i]));
        let max = [0, 1, 2].map(|i| f[i].max(h[i]));
        let colour = if ok { HIGHLIGHT } else { FAIL };
        box_faces(&mut faces, (min, max), colour, camera);
        box_lines(&mut lines, (min, max), colour, 4.0);
    }
    if !faces.is_empty()
        && let Some(glue) = crate::glue::pipelines::resolve(host, Textured::Glue)
    {
        let _ = event.draw.pipeline_quads(glue, &faces);
    }
    if !lines.is_empty() {
        let _ = event.draw.lines(&lines);
    }
}

/// `AABBOutline.renderBoxFaces` with no highlighted face: the six faces of the blocks `min`
/// to `max`, 1/128 out (in, with the camera inside), the texture once a block, in `colour` at
/// half its alpha.
fn box_faces(
    faces: &mut Vec<mod_api::PipelineQuad>,
    (min, max): ([i32; 3], [i32; 3]),
    colour: u32,
    camera: Option<[f64; 3]>,
) {
    let mut lo = min.map(f64::from);
    let mut hi = max.map(|v| f64::from(v) + 1.0);
    let inside = camera.is_some_and(|c| (0..3).all(|i| lo[i] <= c[i] && c[i] <= hi[i]));
    let inflate = if inside { -1.0 / 128.0 } else { 1.0 / 128.0 };
    for i in 0..3 {
        lo[i] -= inflate;
        hi[i] += inflate;
    }
    let colour = (((colour >> 24) / 2) << 24) | (colour & 0xFF_FFFF);
    let (sx, sy, sz) = (
        (hi[0] - lo[0]) as f32,
        (hi[1] - lo[1]) as f32,
        (hi[2] - lo[2]) as f32,
    );
    let [x0, y0, z0] = lo;
    let [x1, y1, z1] = hi;
    for (corners, u, v) in [
        (
            [[x0, y0, z1], [x0, y0, z0], [x1, y0, z0], [x1, y0, z1]],
            sx,
            sz,
        ),
        (
            [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
            sx,
            sz,
        ),
        (
            [[x1, y1, z0], [x1, y0, z0], [x0, y0, z0], [x0, y1, z0]],
            sx,
            sy,
        ),
        (
            [[x0, y1, z1], [x0, y0, z1], [x1, y0, z1], [x1, y1, z1]],
            sx,
            sy,
        ),
        (
            [[x0, y1, z0], [x0, y0, z0], [x0, y0, z1], [x0, y1, z1]],
            sz,
            sy,
        ),
        (
            [[x1, y1, z1], [x1, y0, z1], [x1, y0, z0], [x1, y1, z0]],
            sz,
            sy,
        ),
    ] {
        faces.push(mod_api::PipelineQuad::new(
            corners,
            [[0.0, 0.0], [0.0, v], [u, v], [u, 0.0]],
            colour,
        ));
    }
}

/// The twelve edges of the blocks `min` to `max` (inclusive), a hair outside them.
fn box_lines(
    lines: &mut Vec<mod_api::WorldLine>,
    (min, max): ([i32; 3], [i32; 3]),
    colour: u32,
    width: f32,
) {
    use mod_api::{WorldDepth, WorldLine};
    let lo = min.map(|v| f64::from(v) - 0.005);
    let hi = max.map(|v| f64::from(v) + 1.005);
    let c = |x: usize, y: usize, z: usize| {
        [
            if x == 0 { lo[0] } else { hi[0] },
            if y == 0 { lo[1] } else { hi[1] },
            if z == 0 { lo[2] } else { hi[2] },
        ]
    };
    for i in 0..2 {
        for j in 0..2 {
            for (a, b) in [
                (c(0, i, j), c(1, i, j)),
                (c(i, 0, j), c(i, 1, j)),
                (c(i, j, 0), c(i, j, 1)),
            ] {
                lines.push(WorldLine::new(a, b, colour, width, WorldDepth::Tested));
            }
        }
    }
}

pub(super) fn register(r: &mut dyn Registrar) {
    let item = r.item_key(ITEM);
    r.item_part(item, GlueItem);
}

struct GlueItem;

#[mod_api::horizon_part]
impl ItemPart for GlueItem {
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        // `onMouseInput` reads the main hand only.
        if event.hand != Hand::Main {
            return None;
        }
        match host {
            HostSide::Server(host) => use_on(host, event),
            HostSide::Client(_) => Some(InteractionResult::Success),
        }
    }
}

/// `SuperGlueSelectionHandler.onMouseInput` takes a right click with the glue in the main hand
/// before the block hit is used (a deployer would take the glue, a depot would hold it), unless
/// the block is a chassis (its glueable sides take the glue as a slime ball) or the player
/// sneaks (which only forgets the first corner).
pub(super) fn right_click_block(host: &mut HostSide, event: &mut RightClickBlockEvent<'_>) -> Flow {
    let click = event.click;
    let Some(stack) = click.stack else {
        return Flow::Continue;
    };
    if click.hand != Hand::Main {
        return Flow::Continue;
    }
    let (id, state, player) = match host {
        HostSide::Server(h) => (
            stack.item().id(&**h),
            h.level(click.dimension.key().id())
                .and_then(|l| l.block_state(&**h, click.pos)),
            h.player(click.player)
                .map(|p| (p.may_build(&**h) == Some(true), sneaking(&**h, p))),
        ),
        HostSide::Client(h) => (
            stack.item().id(&**h),
            h.level(h.dimension())
                .and_then(|l| l.block_state(&**h, click.pos)),
            h.local_player()
                .map(|p| (p.may_build(&**h) == Some(true), sneaking(&**h, p))),
        ),
    };
    if id != ITEM || player != Some((true, false)) {
        return Flow::Continue;
    }
    let chassis = [
        create_engine::Block::LinearChassis,
        create_engine::Block::SecondaryLinearChassis,
        create_engine::Block::RadialChassis,
    ];
    if state.is_some_and(|s| {
        chassis
            .iter()
            .any(|b| s.block() == crate::glue::states::key(*b))
    }) {
        return Flow::Continue;
    }
    *event.use_block = TriState::False;
    // The server takes the click here, ahead of the block's value boxes and filter slots.
    let HostSide::Server(h) = host else {
        return Flow::Continue;
    };
    match click_at(&mut **h, click.player, click.pos) {
        Some(answer) => {
            *event.cancellation_result = answer;
            Flow::Cancel
        }
        None => Flow::Continue,
    }
}

fn is_glue(host: &dyn ServerHost, player: PlayerRef) -> bool {
    held(host, player, Hand::Main).is_some_and(|s| s.item().id(host) == ITEM)
}

fn use_on(host: &mut dyn ServerHost, event: &UseOnEvent<'_>) -> Option<InteractionResult> {
    click_at(host, event.player, event.pos)
}

fn click_at(host: &mut dyn ServerHost, id: u128, pos: [i32; 3]) -> Option<InteractionResult> {
    let player = host.player(id)?;
    if player.may_build(&*host) != Some(true) {
        return None;
    }
    let first = FIRST.with_borrow(|f| f.get(&id).copied());
    if sneaking(&*host, player) {
        // `discard`: sneaking forgets the first corner.
        first?;
        FIRST.with_borrow_mut(|f| f.remove(&id));
        action_bar(host, player, "create.super_glue.abort");
        let dimension = player.dimension(&*host)?.to_string();
        if let Some(mut lv) = ServerLevel::new(host, &dimension) {
            send_glue(&mut lv, player);
        }
        return Some(InteractionResult::Success);
    }
    let dimension = player.dimension(&*host)?.to_string();
    let mut lv = ServerLevel::new(host, &dimension)?;
    let Some(from) = first else {
        FIRST.with_borrow_mut(|f| f.insert(id, pos));
        action_bar(lv.host, player, "create.super_glue.first_pos");
        slime_sound(&mut lv, pos, 0.85);
        send_glue(&mut lv, player);
        return Some(InteractionResult::Success);
    };
    // `confirm`, then `SuperGlueSelectionPacket.handle`: the corners must hold together.
    let placed = glue::place(from, pos, |p| {
        found(&lv, p).map(|_| super::actors::sticky_sides(&lv, p))
    });
    let Ok(g) = placed else {
        let key = match placed {
            Err(glue::Refused::TooFar) => "create.super_glue.too_far",
            _ => "create.super_glue.cannot_reach",
        };
        action_bar(lv.host, player, key);
        return Some(InteractionResult::Success);
    };
    let dim = lv.dim;
    with_world(|w| w.dims[dim].glue.push(g));
    let (cx, cz) = g.column();
    mark_unsaved(dim, [cx, cz]);
    if !creative(&*lv.host, player)
        && let Some(living) = lv.host.living(player.entity())
    {
        living.hurt_and_break(lv.host, EquipmentSlot::MainHand, 1);
    }
    FIRST.with_borrow_mut(|f| f.remove(&id));
    crate::glue::advancements::award(lv.host, player, "super_glue");
    action_bar(lv.host, player, "create.super_glue.success");
    slime_sound(&mut lv, pos, 0.95);
    send_glue(&mut lv, player);
    Some(InteractionResult::Success)
}

/// `AllSoundEvents.SLIME_ADDED` and the item frame's click, as the selection plays them.
fn slime_sound(lv: &mut ServerLevel<'_>, pos: [i32; 3], pitch: f32) {
    for (sound, volume, pitch) in [
        ("create:slime_added", 0.5, pitch),
        ("minecraft:entity.item_frame.add_item", 0.75, 1.0),
    ] {
        lv.level.play_sound(
            &mut *lv.host,
            sound,
            center(pos),
            SoundSource::Blocks,
            volume,
            pitch,
        );
    }
}

/// `SuperGlueRemovalPacket`: a left click with the glue in hand takes off the glue holding the
/// block hit.
pub(super) fn left_click(host: &mut dyn ServerHost, event: &LeftClickBlockEvent) -> bool {
    let Some(player) = host.players().acting_player() else {
        return false;
    };
    if !is_glue(&*host, player) || player.may_build(&*host) != Some(true) {
        return false;
    }
    let Some(dimension) = player.dimension(&*host).map(|d| d.to_string()) else {
        return false;
    };
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return false;
    };
    let dim = lv.dim;
    let pos = event.pos;
    let removed = with_world(|w| {
        let areas = &mut w.dims[dim].glue;
        let i = areas.iter().position(|g| g.contains(pos))?;
        Some(areas.remove(i))
    })
    .flatten();
    let Some(g) = removed else {
        return false;
    };
    let (cx, cz) = g.column();
    mark_unsaved(dim, [cx, cz]);
    lv.level.play_sound(
        &mut *lv.host,
        "create:slime_added",
        center(pos),
        SoundSource::Blocks,
        0.5,
        0.5,
    );
    send_glue(&mut lv, player);
    true
}
