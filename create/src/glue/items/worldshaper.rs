//! The handheld worldshaper: its use shapes the block it looks at within 128 blocks
//! (`ZapperItem.use` → `WorldshaperItem.activate`), its left click picks the block it paints
//! with (`ZapperInteractionHandler.trySelect`), its sneaking use opens its screen on the client
//! (`worldshaper_screen`, `openHandgunGUI`), whose settings come back in
//! `ConfigureWorldshaperPacket` and are kept in its custom data under Create's component names.
//! While held, the client outlines the blocks a use would reach (`WorldshaperRenderHandler`).
//!
//! Its faces are covered with `AllSpecialTextures.CHECKERED` through the glue's textured world
//! pipeline (`pipelines`).

use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::worldshaper_screen::{self as screen, Configure};
use super::{Item, action_bar, custom, held, is, set_custom, set_held, sneaking};
use crate::glue::pipelines::Textured;
use create_engine::kinetics::Dir;
use create_engine::worldshaper as ws;
use mod_api::{
    BlockFace, BlockState, ClientHost, ClipBlock, ClipFluid, Hand, Host, HostBlockStates, HostSide,
    InteractionResult, ItemPart, ItemStack, LevelRef, PipelineQuad, PlayerRef, RenderLevelStage,
    RenderLevelStageEvent, ServerHost, SoundSource, SupportType, Tag, UseItemEvent, UseOnEvent,
    WorldDepth, WorldLine,
};
use std::collections::{BTreeMap, HashSet};

pub(super) struct Shaper;

#[mod_api::horizon_part]
impl ItemPart for Shaper {
    /// `ZapperItem.useOn`: sneaking opens the screen; otherwise the use goes on to `use`.
    fn use_on(&self, host: HostSide, event: &mut UseOnEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Client(host) if client_sneaking(&*host) => {
                screen::request(event.stack.clone(), event.hand);
                Some(InteractionResult::Success)
            }
            HostSide::Server(host) if server_sneaking(&*host, event.player) => {
                if let Some(p) = host.player(event.player) {
                    p.set_item_cooldown(host, SHAPER, 10);
                }
                Some(InteractionResult::Success)
            }
            _ => None,
        }
    }

    /// `ZapperItem.use`: sneaking opens the screen on the client; otherwise it shapes.
    fn use_item(&self, host: HostSide, event: &mut UseItemEvent<'_>) -> Option<InteractionResult> {
        match host {
            HostSide::Server(host) => {
                use_shaper(host, event);
            }
            HostSide::Client(host) => {
                if client_sneaking(&*host) {
                    screen::request(event.stack.clone(), event.hand);
                }
            }
        }
        Some(InteractionResult::Success)
    }
}

const SHAPER: &str = "create:handheld_worldshaper";

fn client_sneaking(host: &dyn ClientHost) -> bool {
    host.local_player()
        .and_then(|p| p.entity().is_shift_key_down(host))
        == Some(true)
}

fn server_sneaking(host: &dyn ServerHost, uuid: u128) -> bool {
    host.player(uuid).is_some_and(|p| sneaking(host, p))
}

fn text(t: Option<&Tag>) -> Option<String> {
    match t {
        Some(Tag::String(s)) => Some(s.to_string_lossy()),
        _ => None,
    }
}

/// The worldshaper's settings and the block it uses (`SHAPER_BLOCK_USED` as a state's text).
pub(super) fn shaper(stack: &ItemStack) -> (ws::Settings, Option<String>) {
    let c = custom(stack);
    let name = |k: &str| text(c.get(&k.into())).unwrap_or_default();
    let settings = ws::Settings {
        brush: ws::Brush::parse(&name(ws::BRUSH_KEY)).unwrap_or_default(),
        params: match c.get(&ws::PARAMS_KEY.into()) {
            Some(Tag::IntArray(v)) if v.len() == 3 => Some([v[0], v[1], v[2]]),
            _ => None,
        },
        tool: ws::Tool::parse(&name(ws::TOOL_KEY)).unwrap_or_default(),
        placement: ws::Placement::parse(&name(ws::PLACEMENT_KEY)).unwrap_or_default(),
        pattern: ws::Pattern::parse(&name(ws::PATTERN_KEY)).unwrap_or_default(),
    };
    (settings, text(c.get(&ws::BLOCK_KEY.into())))
}

/// `WorldshaperItem.configureSettings` (and the block, when given: `trySelect`).
fn set_shaper(stack: &mut ItemStack, s: &ws::Settings, block: Option<&str>) {
    let mut entries = vec![
        (ws::BRUSH_KEY, Some(Tag::String(s.brush.name().into()))),
        (ws::PARAMS_KEY, s.params.map(|p| Tag::IntArray(p.to_vec()))),
        (ws::TOOL_KEY, Some(Tag::String(s.tool.name().into()))),
        (
            ws::PLACEMENT_KEY,
            Some(Tag::String(s.placement.name().into())),
        ),
        (ws::PATTERN_KEY, Some(Tag::String(s.pattern.name().into()))),
    ];
    if let Some(b) = block {
        entries.push((ws::BLOCK_KEY, Some(Tag::String(b.into()))));
    }
    set_custom(stack, &entries);
}

/// `state`'s properties.
fn props_of(host: &dyn Host, state: BlockState) -> BTreeMap<String, String> {
    let text = host.block_state_string(state).unwrap_or_default();
    let (_, own) = states::split(&text);
    own.iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// `state` with `props` set.
fn with_props(host: &dyn Host, state: BlockState, props: &BTreeMap<String, String>) -> BlockState {
    let facts = host.block_states();
    props
        .iter()
        .fold(state, |s, (k, v)| s.with(&facts, k, v).unwrap_or(s))
}

/// The state a pick takes (`trySelect`): a state that can be picked, its age set to 0 when
/// used; None when it can't (`selected_state`).
fn picked(host: &dyn Host, state: BlockState) -> Option<BlockState> {
    let picked = ws::selected_state(&props_of(host, state))?;
    Some(with_props(host, state, &picked))
}

fn dir_of(face: BlockFace) -> Option<Dir> {
    Some(match face {
        BlockFace::Down => Dir::Down,
        BlockFace::Up => Dir::Up,
        BlockFace::North => Dir::North,
        BlockFace::South => Dir::South,
        BlockFace::West => Dir::West,
        BlockFace::East => Dir::East,
        _ => return None,
    })
}

/// The block the player looks at within 128 blocks (`Level.clip` with `Block.OUTLINE`, no
/// fluids): its position and the face entered.
fn aimed(lv: &ServerLevel<'_>, player: PlayerRef) -> Option<([i32; 3], Dir)> {
    let entity = player.entity();
    let eye = entity.eye_position(&*lv.host)?;
    let look = entity.view_vector(&*lv.host)?;
    let hit = clip_far(&*lv.host, lv.level, eye, look)?;
    Some((hit.block, dir_of(hit.face)?))
}

/// `Level.clip` from `eye` along `look` for `ws::RANGE` blocks (`Block.OUTLINE`, no fluids), in
/// steps: the API's clip answers None for a ray entering a column the level doesn't hold, which
/// vanilla's reads as air, so the ray stops there as a miss.
fn clip_far(
    host: &dyn Host,
    level: mod_api::LevelRef,
    eye: [f64; 3],
    look: [f64; 3],
) -> Option<mod_api::ClipHit> {
    const STEP: f64 = 8.0;
    let mut from = 0.0;
    while from < ws::RANGE {
        let to = (from + STEP).min(ws::RANGE);
        let a = [0, 1, 2].map(|i| eye[i] + look[i] * from);
        let b = [0, 1, 2].map(|i| eye[i] + look[i] * to);
        let hit = level.clip(host, a, b, ClipBlock::Outline, ClipFluid::None)?;
        if !hit.miss {
            return Some(hit);
        }
        from = to;
    }
    None
}

/// Whether `state` is air (`BlockState.isAir`).
fn is_air(host: &dyn Host, state: BlockState) -> bool {
    matches!(
        state.block().id(host).as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

/// `ZapperInteractionHandler.leftClickingBlocksWithTheZapperSelectsTheBlock` → `trySelect`: the
/// worldshaper in the main hand (not sneaking) picks the block it looks at within 128 blocks,
/// unless it can't be picked (no item, a block entity: Create takes those of `create:safe_nbt`
/// with their data, which the port doesn't copy) or it already uses that state; the click
/// then breaks nothing. Whether it picked.
pub(super) fn left_click(host: &mut dyn ServerHost) -> bool {
    select(host).is_some()
}

fn select(host: &mut dyn ServerHost) -> Option<()> {
    let player = host.players().acting_player()?;
    let mut stack = held(&*host, player, Hand::Main)?;
    if !is(Some(&stack), Item::Worldshaper) || sneaking(&*host, player) {
        return None;
    }
    let dimension = player.dimension(&*host)?.to_string();
    let lv = ServerLevel::new(host, &dimension)?;
    let (hit, _) = aimed(&lv, player)?;
    let state = lv.block_state(hit)?;
    let facts = lv.host.block_states();
    if lv.host.block_item(state.block()).is_none() || facts.has_block_entity(state) == Some(true) {
        return None;
    }
    let text = lv.host.block_state_string(picked(&*lv.host, state)?)?;
    let (settings, used) = shaper(&stack);
    if used.as_deref() == Some(text.as_str()) {
        return None;
    }
    set_shaper(&mut stack, &settings, Some(&text));
    set_held(lv.host, player, Hand::Main, Some(stack));
    if let Some(at) = player.entity().position(&*lv.host) {
        lv.level.play_sound(
            &mut *lv.host,
            "create:confirm",
            at,
            SoundSource::Players,
            1.0,
            1.0,
        );
    }
    Some(())
}

/// `ZapperItem.use` (not sneaking) for the worldshaper: the block it looks at within 128
/// blocks is shaped by its brush and tool (`WorldshaperItem.activate`).
fn use_shaper(host: &mut dyn ServerHost, event: &UseItemEvent<'_>) -> Option<()> {
    let player = host.player(event.player)?;
    if sneaking(&*host, player) {
        // The screen opens on the client, with a 10-tick cooldown.
        player.set_item_cooldown(host, SHAPER, 10);
        return None;
    }
    let (settings, block) = shaper(event.stack);
    // `validateUsage`.
    let key = if settings.params.is_none() {
        Some("create.terrainzapper.shiftRightClickToSet")
    } else if settings.tool.requires_block() && block.is_none() {
        Some("create.terrainzapper.leftClickToSet")
    } else {
        None
    };
    if let Some(key) = key {
        action_bar(host, player, key);
        return None;
    }
    let dimension = player.dimension(&*host)?.to_string();
    let mut lv = ServerLevel::new(host, &dimension)?;
    let aim = aimed(&lv, player);
    // `ShootableGadgetItemMethods.applyCooldown`: `getCooldownDelay` (2), two thirds of it with
    // another worldshaper in the other hand; after a miss too.
    let other = if event.hand == Hand::Main {
        Hand::Off
    } else {
        Hand::Main
    };
    let two = is(held(&*lv.host, player, other).as_ref(), Item::Worldshaper);
    player.set_item_cooldown(
        lv.host,
        "create:handheld_worldshaper",
        if two { 1 } else { 2 },
    );
    let (hit, face) = aim?;
    if lv.block_state(hit).is_none_or(|s| is_air(&*lv.host, s)) {
        return None;
    }
    // `ZapperBeamPacket` → `ZapperRenderHandler.playSound`: `WORLDSHAPER_PLACE` at 0.1, pitched
    // 0.1 from the main hand and 0.9 from the off hand, for everyone near.
    if let Some(at) = player.entity().position(&*lv.host) {
        let pitch = if event.hand == Hand::Main { 0.1 } else { 0.9 };
        lv.level.play_sound(
            &mut *lv.host,
            "create:worldshaper_place",
            at,
            SoundSource::Players,
            0.1,
            pitch,
        );
    }
    let paint = match &block {
        Some(b) => lv.host.block_state_id(b)?,
        None => lv.host.block_state_id("minecraft:air")?,
    };
    // `BlockHelper.setZeroAge`.
    let paint = {
        let mut props = props_of(&*lv.host, paint);
        ws::zero_age(&mut props);
        with_props(&*lv.host, paint, &props)
    };
    let mut seed = (lv.game_time() as u64) ^ 0x2545_f491_4f6c_dd1d;
    let positions = ws::positions(
        &settings,
        hit,
        face,
        &mut Ground::new(&*lv.host, lv.level),
        || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed & 1 == 1
        },
    );
    let air = lv.host.block_state_id("minecraft:air");
    if settings.brush.redirect(settings.tool) == ws::Tool::Flatten {
        if let Some(air) = air {
            ws::flatten(&mut Flattening { lv: &mut lv, air }, &positions, face);
        }
        return Some(());
    }
    let writes = ws::run(settings.brush.redirect(settings.tool), &positions, |p| {
        let here = lv.block_state(p);
        ws::Seen {
            replaceable: here
                .and_then(|s| lv.host.block_states().replaceable(s))
                .unwrap_or(false),
            painted: here == Some(paint),
        }
    });
    for (p, w) in writes {
        let to = match w {
            ws::Write::Paint => paint,
            ws::Write::Air => match air {
                Some(a) => a,
                None => continue,
            },
        };
        lv.set_state(p, to, update_all());
    }
    Some(())
}

/// The level as the flatten tool reads and writes it.
struct Flattening<'a, 'b> {
    lv: &'a mut ServerLevel<'b>,
    air: BlockState,
}

impl ws::FlattenWorld for Flattening<'_, '_> {
    type State = BlockState;
    fn state(&mut self, pos: [i32; 3]) -> BlockState {
        self.lv.block_state(pos).unwrap_or(self.air)
    }
    fn replaceable(&mut self, pos: [i32; 3]) -> bool {
        let s = self.state(pos);
        self.lv.host.block_states().replaceable(s).unwrap_or(false)
    }
    fn set(&mut self, pos: [i32; 3], state: BlockState) {
        self.lv.set_state(pos, state, update_all());
    }
    /// `getFluidState().createLegacyBlock()`: a water or lava block keeps its level, a
    /// waterlogged block gives a source, anything dry air.
    fn fluid_block(&mut self, state: &BlockState) -> BlockState {
        let host = &*self.lv.host;
        let Some(f) = host.state_fluid_state(*state).filter(|f| !f.is_empty()) else {
            return self.air;
        };
        let id = f.fluid.id(host);
        let block = id.replace("flowing_", "");
        let level = if f.is_source() {
            0
        } else {
            8 - i32::from(f.amount.min(8)) + if f.falling { 8 } else { 0 }
        };
        host.block_state_id(&format!("{block}[level={level}]"))
            .or_else(|| host.block_state_id(&block))
            .unwrap_or(self.air)
    }
    fn liquid(&mut self, state: &BlockState) -> bool {
        let name = self.lv.host.block_state_string(*state).unwrap_or_default();
        let name = name.split('[').next().unwrap_or_default();
        matches!(name, "minecraft:water" | "minecraft:lava")
    }
}

/// `ConfigureWorldshaperPacket.handle`: the settings into the worldshaper still in that hand
/// (`configureSettings`), each parameter kept in its brush's range.
pub(super) fn configure(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let Some(Configure { hand, mut settings }) = Configure::decode(data) else {
        return;
    };
    let Some(mut stack) = held(&*host, player, hand) else {
        return;
    };
    if !is(Some(&stack), Item::Worldshaper) {
        return;
    }
    if let Some(p) = settings.params.as_mut() {
        for (i, v) in p.iter_mut().enumerate() {
            let (lo, hi) = settings.brush.bounds(i);
            // The dynamic brushes' toggles are 0 or 1.
            *v = if i > 0 && settings.brush.has_connectivity() {
                (*v).clamp(0, 1)
            } else {
                (*v).clamp(lo, hi)
            };
        }
    }
    let (_, block) = shaper(&stack);
    set_shaper(&mut stack, &settings, block.as_deref());
    set_held(host, player, hand, Some(stack));
}

/// What the dynamic brushes read of a level, on either side.
struct Ground<'a> {
    host: &'a dyn Host,
    level: Option<LevelRef>,
}

impl<'a> Ground<'a> {
    fn new(host: &'a dyn Host, level: LevelRef) -> Ground<'a> {
        Ground {
            host,
            level: Some(level),
        }
    }

    fn state(&self, pos: [i32; 3]) -> Option<BlockState> {
        self.level?.block_state(self.host, pos)
    }
}

impl ws::Ground for Ground<'_> {
    type Block = Option<mod_api::BlockKey>;

    fn block(&mut self, pos: [i32; 3]) -> Self::Block {
        self.state(pos).map(|s| s.block())
    }

    fn replaceable(&mut self, pos: [i32; 3]) -> bool {
        self.state(pos)
            .and_then(|s| self.host.block_states().replaceable(s))
            .unwrap_or(false)
    }

    fn unbreakable(&mut self, pos: [i32; 3]) -> bool {
        self.state(pos)
            .and_then(|s| self.host.block_states().destroy_speed(s))
            .is_some_and(|h| h < 0.0)
    }

    fn solid_side(&mut self, pos: [i32; 3], side: Dir) -> bool {
        let Some(state) = self.state(pos) else {
            return false;
        };
        if is_air(self.host, state) {
            return false;
        }
        let face = match side {
            Dir::Down => BlockFace::Down,
            Dir::Up => BlockFace::Up,
            Dir::North => BlockFace::North,
            Dir::South => BlockFace::South,
            Dir::West => BlockFace::West,
            Dir::East => BlockFace::East,
        };
        self.level
            .and_then(|l| l.block_face_sturdy(self.host, pos, face, SupportType::Full))
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------------------------
// Client: the outline

thread_local! {
    /// `WorldshaperRenderHandler.renderedPositions`, gathered each client tick.
    static OUTLINED: std::cell::RefCell<Vec<[i32; 3]>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(super) fn reset() {
    OUTLINED.with_borrow_mut(Vec::clear);
}

/// `WorldshaperRenderHandler.gatherSelectedBlocks`: the worldshaper in the main hand, else the
/// off hand, outlines what its use would reach from the block looked at within 128 blocks.
pub(super) fn tick(host: &dyn ClientHost) {
    let found = outlined(host).unwrap_or_default();
    OUTLINED.with_borrow_mut(|o| *o = found);
}

fn outlined(host: &dyn ClientHost) -> Option<Vec<[i32; 3]>> {
    let player = host.local_player()?;
    let living = host.living(player.entity())?;
    let stack = [Hand::Main, Hand::Off]
        .into_iter()
        .filter_map(|h| living.hand_item(host, h))
        .find(|s| is(Some(s), Item::Worldshaper))?;
    let (settings, _) = shaper(&stack);
    settings.params?;
    let level = host.level(host.dimension())?;
    // `RaycastHelper.rayTraceRange(.., 128)` from the camera.
    let hit = host.raycast(ws::RANGE as f32)?;
    let block = [hit.block_x, hit.block_y, hit.block_z];
    let face = super::super::schematics::face_of(block, [hit.x, hit.y, hit.z]);
    Some(ws::reached(
        &settings,
        block,
        face,
        &mut Ground::new(host, level),
    ))
}

/// `Outliner.showCluster(..).colored(0xbfbfbf).lineWidth(1 / 32f)`: the outer edges of the
/// blocks' union, and its outer faces.
pub(super) fn render_level_stage(host: &dyn ClientHost, event: &mut RenderLevelStageEvent<'_>) {
    if event.stage != RenderLevelStage::AfterTranslucentBlocks {
        return;
    }
    OUTLINED.with_borrow(|cells| {
        if cells.is_empty() {
            return;
        }
        let (lines, faces, uvs) = cluster(cells);
        // `withFaceTexture(AllSpecialTextures.CHECKERED)`: each face the checkerboard from the
        // block atlas, in the outline's colour; without the pipeline the edges alone.
        if let Some(p) = crate::glue::pipelines::resolve(host, Textured::Checkered) {
            let faces: Vec<PipelineQuad> = faces
                .iter()
                .zip(&uvs)
                .map(|(q, uv)| PipelineQuad::new(*q, *uv, OUTLINE))
                .collect();
            let _ = event.draw.pipeline_quads(p, &faces);
        }
        let _ = event.draw.lines(&lines);
    });
}

const OUTLINE: u32 = 0xFFBF_BFBF;

/// `BlockClusterOutline.loadFaceData`: each face's first corner (the texture's `(0, 0)`), in
/// `Dir::ALL`'s order (down, up, north, south, west, east).
const FIRST_CORNER: [[i32; 3]; 6] = [
    [0, 0, 1],
    [0, 1, 0],
    [1, 1, 0],
    [0, 1, 1],
    [0, 1, 0],
    [1, 1, 1],
];

/// A face's four corners.
type Corners = [[f64; 3]; 4];

/// The faces between a member and a non-member, and the edges of those faces that don't run on
/// into a flat continuation of the same face; each face's corners with their texture
/// coordinates (`(0, 0)` at the face's first corner).
fn cluster(cells: &[[i32; 3]]) -> (Vec<WorldLine>, Vec<Corners>, Vec<[[f32; 2]; 4]>) {
    let set: HashSet<[i32; 3]> = cells.iter().copied().collect();
    let add = |a: [i32; 3], b: [i32; 3]| [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
    let exposed = |p: [i32; 3], d: Dir| set.contains(&p) && !set.contains(&add(p, d.offset()));
    let mut lines = Vec::new();
    let mut faces = Vec::new();
    let mut uvs = Vec::new();
    for &p in &set {
        for d in Dir::ALL {
            if !exposed(p, d) {
                continue;
            }
            let n = d.offset();
            let axis = d.axis() as usize;
            let (u, v) = match axis {
                0 => (1, 2),
                1 => (0, 2),
                _ => (0, 1),
            };
            // The face's plane, 1/128 outside the block (`bufferBlockFace`).
            let plane = f64::from(p[axis])
                + if n[axis] > 0 { 1.0 } else { 0.0 }
                + f64::from(n[axis]) / 128.0;
            let corner = |a: i32, b: i32| {
                let mut c = [0.0; 3];
                c[axis] = plane;
                c[u] = f64::from(p[u] + a);
                c[v] = f64::from(p[v] + b);
                c
            };
            // `BlockClusterOutline.bufferBlockFace` with the checkerboard: its `(0, 0)` at the
            // face's first corner, its `(1, 1)` at the opposite one.
            let first = FIRST_CORNER[d as usize];
            let (fu, fv) = (first[u], first[v]);
            let ends = [(0, 0), (1, 0), (1, 1), (0, 1)];
            faces.push(ends.map(|(a, b)| corner(a, b)));
            uvs.push(ends.map(|(a, b)| [(a - fu).abs() as f32, (b - fv).abs() as f32]));
            // Each edge: (step to the neighbour across it, its two ends).
            for (step, a, b) in [
                ((u, -1), corner(0, 0), corner(0, 1)),
                ((u, 1), corner(1, 0), corner(1, 1)),
                ((v, -1), corner(0, 0), corner(1, 0)),
                ((v, 1), corner(0, 1), corner(1, 1)),
            ] {
                let mut o = [0; 3];
                o[step.0] = step.1;
                let beside = add(p, o);
                // The same face going on flat: no edge there.
                if exposed(beside, d) {
                    continue;
                }
                lines.push(WorldLine::new(a, b, OUTLINE, 1.5, WorldDepth::Tested));
            }
        }
    }
    (lines, faces, uvs)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_cluster_outlines_its_outer_edges_once_per_face() {
        // One block: 6 faces, 4 edges each.
        let (lines, faces, uvs) = super::cluster(&[[0, 0, 0]]);
        assert_eq!((lines.len(), faces.len(), uvs.len()), (24, 6, 6));
        // Two side by side: 10 faces; the shared face's edges are gone from the four faces
        // that run on, which keep 3 edges each.
        let (lines, faces, _) = super::cluster(&[[0, 0, 0], [1, 0, 0]]);
        assert_eq!(faces.len(), 10);
        assert_eq!(lines.len(), 2 * 4 + 8 * 3);
    }
}
