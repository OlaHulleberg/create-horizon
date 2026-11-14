//! What clients draw of Create's logistics, from the server's last word on each channel and
//! run on tick by tick as the server runs it: depots' stacks sliding to the centre
//! (`DepotRenderer`), belts' segments, pulleys and items (`BeltRenderer`), arms
//! (`ArmRenderer`) and ejectors' plates and stacks in flight (`EjectorRenderer`). Each is drawn
//! by the kinetic block entity's renderer every frame, between the last tick and the next.

use super::super::states;
use super::items::{is_block_item, stack_of};
use create_engine::Block;
use create_engine::arm;
use create_engine::belt;
use create_engine::client::{self as parts, Model};
use create_engine::depot;
use create_engine::ejector;
use create_engine::kinetics::{Axis, Dir};
use create_engine::math::{Mat4, Vec3};
use create_engine::place::mth_sin;
use mod_api::{
    BlockEntityDrawContext, BlockEntityPart, BlockEntityRenderBoundingBoxEvent,
    BlockEntitySubmitEvent, BlockPart, ClientHost, Compound, Host, ItemDisplayContext, Tag,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// A depot a client draws: its contents and where its held stack was a tick ago.
struct ClientDepot {
    contents: depot::Contents,
    prev: depot::Held,
}

/// A belt a client draws: what the server last said, run on, and where its items were a tick
/// ago (for as long as the same items are on it).
struct ClientBelt {
    contents: belt::Contents,
    prev: Vec<f32>,
}

/// An arm a client draws: the server's last word and how far its move has come here.
struct ClientArm {
    shown: arm::Shown,
    progress: f32,
    prev: f32,
}

/// Whether the ejector at `pos` this client shows has its plate down and charged (an unknown
/// one counts as charged).
pub(crate) fn ejector_charged(pos: [i32; 3]) -> bool {
    CLIENT.with_borrow(|c| {
        c.ejectors
            .get(&pos)
            .is_none_or(|e| e.shown.state == ejector::State::Charged)
    })
}

/// An ejector a client draws: the server's last word and ticks since.
struct ClientEjector {
    shown: ejector::Shown,
    ticks: i32,
}

#[derive(Default)]
struct Client {
    depots: HashMap<[i32; 3], ClientDepot>,
    sliding: Vec<[i32; 3]>,
    belts: HashMap<[i32; 3], ClientBelt>,
    arms: HashMap<[i32; 3], ClientArm>,
    ejectors: HashMap<[i32; 3], ClientEjector>,
    /// The speeds the kinetic block entities last drew with, for the ticks.
    speeds: HashMap<[i32; 3], f32>,
    /// Whether an item draws as a block, by id, looked up once.
    block_items: HashMap<String, bool>,
}

thread_local! {
    static CLIENT: RefCell<Client> = RefCell::new(Client::default());
}

/// The client left its world: forget depots and belts.
pub fn reset_logistics() {
    CLIENT.with_borrow_mut(|c| {
        c.depots.clear();
        c.sliding.clear();
        c.belts.clear();
        c.speeds.clear();
        c.block_items.clear();
    });
}

/// Forget arms and ejectors.
pub fn reset_arms() {
    CLIENT.with_borrow_mut(|c| {
        c.arms.clear();
        c.ejectors.clear();
    });
    SELECTED.with_borrow_mut(Vec::clear);
}

thread_local! {
    /// The arm points this client's player chose with the arm item, and whether each is an input.
    static SELECTED: RefCell<Vec<([i32; 3], bool)>> = const { RefCell::new(Vec::new()) };
}

/// The server's word on the arm points this player chose.
pub fn selection_message(mut data: &[u8]) {
    let list = arm::decode_points(&mut data).unwrap_or_default();
    SELECTED.with_borrow_mut(|s| *s = list);
}

thread_local! {
    /// The block this player chose as the next ejector's target (`EjectorTargetHandler`'s
    /// `currentSelection`, kept on the server), and the hovered block the arc was last made for.
    static EJECTOR_SEL: std::cell::Cell<Option<[i32; 3]>> = const { std::cell::Cell::new(None) };
    static ARC: RefCell<Option<([i32; 3], ejector::Launcher)>> = const { RefCell::new(None) };
}

static DUST: std::sync::OnceLock<mod_api::ParticleKey> = std::sync::OnceLock::new();

/// The particle the ejector's arc is drawn with.
pub fn register_particles(r: &mut dyn mod_api::Registrar) {
    let _ = DUST.set(r.particle_key("minecraft:dust"));
}

/// The server's word on this player's ejector target (none: an empty message).
pub fn ejector_target_message(data: &[u8]) {
    let pos = (data.len() == 12).then(|| {
        let int = |i: usize| i32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        [int(0), int(4), int(8)]
    });
    EJECTOR_SEL.set(pos);
    ARC.with_borrow_mut(|a| *a = None);
}

/// What the main hand holds, by id.
fn main_hand(host: &dyn ClientHost) -> Option<String> {
    let player = host.local_player()?;
    super::super::items::held(host, player, mod_api::Hand::Main).map(|s| s.item().id(host))
}

/// The target of the ejector a wrench looks at (`checkForWrench`): `h` ahead along its facing
/// and `v` up, none while it targets itself.
fn wrench_target(host: &dyn ClientHost) -> Option<[i32; 3]> {
    let Some(mod_api::CrosshairTarget::Block { pos, .. }) = host.crosshair_target() else {
        return None;
    };
    let at = [pos.x, pos.y, pos.z];
    let state = host.level(host.dimension())?.block_state(host, at)?;
    let (block, info) = table_info(host, state)?;
    if block != Block::WeightedEjector {
        return None;
    }
    let (h, v) = CLIENT.with_borrow(|c| c.ejectors.get(&at).map(|e| (e.shown.h, e.shown.v)))?;
    let o = info.facing?.offset();
    let t = [at[0] + o[0] * h, at[1] + v, at[2] + o[2] * h];
    (t != at).then_some(t)
}

/// `EjectorTargetHandler.drawArc`, each client tick: with the ejector in hand (or a wrench on
/// an ejector), dust along the arc from where an ejector would stand to the target, green when
/// straight in line, red when not.
pub fn tick_ejector_arc(host: &mut dyn ClientHost) {
    let held = main_hand(&*host);
    let wrench = held.as_deref() == Some("create:wrench");
    let selection = match held.as_deref() {
        Some("create:weighted_ejector") => EJECTOR_SEL.get(),
        _ if wrench => wrench_target(&*host),
        _ => None,
    };
    let Some(sel) = selection else {
        return;
    };
    let Some(mod_api::CrosshairTarget::Block { pos, hit, .. }) = host.crosshair_target() else {
        return;
    };
    let mut pos = [pos.x, pos.y, pos.z];
    if !wrench {
        pos = [0, 1, 2].map(|k| pos[k] + hit.face[k]);
    }
    let Some((valid, d, launcher)) = arc_for(sel, pos) else {
        return;
    };
    let Some(&dust) = DUST.get() else {
        return;
    };
    let total = launcher.total_ticks() + 3.0;
    let segments = (total as i32) / 3 + 1;
    let offset = total / f64::from(segments);
    let color: u32 = if valid.0 { 0x9E_DE73 } else { 0xFF_7171 };
    let rgb = [16, 8, 0].map(|s| ((color >> s) & 255) as f32 / 255.0);
    let options = mod_api::ParticleOptions::dust(dust, rgb, 1.0);
    let now = host
        .level(host.dimension())
        .and_then(|l| l.game_time(&*host))
        .unwrap_or(0) as f64;
    for i in 0..segments {
        let ticks = ((now / 3.0) % offset) + f64::from(i) * offset;
        let v = launcher.global_pos(ticks, d, pos);
        let at = [
            v[0] + f64::from(valid.1[0]),
            v[1],
            v[2] + f64::from(valid.1[2]),
        ];
        host.add_particle(&options, at, [0.0; 3]);
    }
}

/// The arc from `pos` to `sel` (`drawArc`): whether `pos` is straight in line, the offset to
/// the nearest place that is, the direction thrown, and its launcher.
fn arc_for(sel: [i32; 3], pos: [i32; 3]) -> Option<((bool, [i32; 3]), Dir, ejector::Launcher)> {
    let diff = [0, 1, 2].map(|k| sel[k] - pos[k]);
    let valid_x = if diff[2].abs() > diff[0].abs() {
        0
    } else {
        diff[0]
    };
    let valid_z = if diff[2].abs() < diff[0].abs() {
        0
    } else {
        diff[2]
    };
    let valid_pos = [sel[0] - valid_x, sel[1] - diff[1], sel[2] - valid_z];
    // `getValidTargetDirection(validPos)`.
    let (dx, dz) = (sel[0] - valid_pos[0], sel[2] - valid_pos[2]);
    if (dx, dz) == (0, 0) || dx.abs() > ejector::MAX_DISTANCE || dz.abs() > ejector::MAX_DISTANCE {
        return None;
    }
    let d = match (dx, dz) {
        (0, z) if z < 0 => Dir::North,
        (0, _) => Dir::South,
        (x, 0) if x < 0 => Dir::West,
        (_, 0) => Dir::East,
        _ => return None,
    };
    let launcher = ejector::Launcher::new((valid_x + valid_z).abs(), diff[1]);
    let valid = diff[0] == valid_x && diff[2] == valid_z;
    let off = [diff[0] - valid_x, 0, diff[2] - valid_z];
    Some(((valid, off), d, launcher))
}

/// `EjectorTargetHandler.drawOutline` (the target, in its shape's bounds) and the arc's start
/// (`chaseAABB("valid", ..)`: a flat square where the ejector would stand).
fn draw_ejector_target(host: &dyn ClientHost, lines: &mut Vec<mod_api::WorldLine>) {
    let held = main_hand(host);
    let wrench = held.as_deref() == Some("create:wrench");
    let sel = match held.as_deref() {
        Some("create:weighted_ejector") => EJECTOR_SEL.get(),
        _ if wrench => wrench_target(host),
        _ => None,
    };
    let Some(sel) = sel else {
        return;
    };
    let boxes = host
        .level(host.dimension())
        .and_then(|l| l.block_state(host, sel))
        .and_then(|s| host.state_outline(s))
        .unwrap_or_default();
    let (mut lo, mut hi) = ([1.0f64; 3], [0.0f64; 3]);
    for b in &boxes {
        for k in 0..3 {
            lo[k] = lo[k].min(b[k]);
            hi[k] = hi[k].max(b[k + 3]);
        }
    }
    if boxes.is_empty() {
        (lo, hi) = ([0.0; 3], [1.0; 3]);
    }
    let min = [0, 1, 2].map(|k| f64::from(sel[k]) + lo[k] - 0.002);
    let max = [0, 1, 2].map(|k| f64::from(sel[k]) + hi[k] + 0.002);
    box_lines(lines, min, max, 0xFFFF_CB74);
    let Some(mod_api::CrosshairTarget::Block { pos, hit, .. }) = host.crosshair_target() else {
        return;
    };
    let mut pos = [pos.x, pos.y, pos.z];
    if !wrench {
        pos = [0, 1, 2].map(|k| pos[k] + hit.face[k]);
    }
    if let Some(((valid, off), _, _)) = arc_for(sel, pos) {
        let at = [pos[0] + off[0], pos[1], pos[2] + off[2]].map(f64::from);
        let color = if valid { 0xFF9E_DE73 } else { 0xFFFF_7171 };
        box_lines(lines, at, [at[0] + 1.0, at[1], at[2] + 1.0], color);
    }
}

/// `Mode.TAKE`'s and `Mode.DEPOSIT`'s colours.
const TAKE_COLOR: u32 = 0xFF7F_CDE0;
const DEPOSIT_COLOR: u32 = 0xFFDD_C166;

/// `ArmInteractionPointHandler.drawOutlines`: with the arm item in the main hand, the points
/// chosen for it; with a wrench, the points of the arm looked at (`checkForWrench`). Each is
/// outlined by its block's shape, in its mode's colour.
pub fn draw_arm_points(host: &dyn ClientHost, event: &mut mod_api::RenderLevelStageEvent<'_>) {
    if event.stage != mod_api::RenderLevelStage::AfterTranslucentBlocks {
        return;
    }
    let Some(player) = host.local_player() else {
        return;
    };
    let held =
        super::super::items::held(host, player, mod_api::Hand::Main).map(|s| s.item().id(host));
    let points = match held.as_deref() {
        Some("create:mechanical_arm") => SELECTED.with_borrow(Clone::clone),
        Some("create:wrench") => {
            let Some(mod_api::CrosshairTarget::Block { pos, .. }) = host.crosshair_target() else {
                return;
            };
            let at = [pos.x, pos.y, pos.z];
            CLIENT
                .with_borrow(|c| c.arms.get(&at).map(|a| a.shown.points.clone()))
                .unwrap_or_default()
        }
        _ => Vec::new(),
    };
    let level = host.level(host.dimension());
    let mut lines = Vec::new();
    for (p, input) in points {
        let boxes = level
            .and_then(|l| l.block_state(host, p))
            .and_then(|s| host.state_outline(s))
            .unwrap_or_default();
        let (mut lo, mut hi) = ([1.0f64; 3], [0.0f64; 3]);
        for b in &boxes {
            for k in 0..3 {
                lo[k] = lo[k].min(b[k]);
                hi[k] = hi[k].max(b[k + 3]);
            }
        }
        if boxes.is_empty() {
            (lo, hi) = ([0.0; 3], [1.0; 3]);
        }
        let min = [0, 1, 2].map(|k| f64::from(p[k]) + lo[k] - 0.002);
        let max = [0, 1, 2].map(|k| f64::from(p[k]) + hi[k] + 0.002);
        let color = if input { TAKE_COLOR } else { DEPOSIT_COLOR };
        box_lines(&mut lines, min, max, color);
    }
    draw_ejector_target(host, &mut lines);
    if !lines.is_empty() {
        let _ = event.draw.lines(&lines);
    }
}

/// The twelve edges of a box, as Create's outliner draws them (1/16 of a block wide).
pub(crate) fn box_lines(
    lines: &mut Vec<mod_api::WorldLine>,
    min: [f64; 3],
    max: [f64; 3],
    color: u32,
) {
    let c = |x: usize, y: usize, z: usize| {
        [
            if x == 0 { min[0] } else { max[0] },
            if y == 0 { min[1] } else { max[1] },
            if z == 0 { min[2] } else { max[2] },
        ]
    };
    for i in 0..2 {
        for j in 0..2 {
            for (a, b) in [
                (c(0, i, j), c(1, i, j)),
                (c(i, 0, j), c(i, 1, j)),
                (c(i, j, 0), c(i, j, 1)),
            ] {
                lines.push(mod_api::WorldLine::new(
                    a,
                    b,
                    color,
                    2.0,
                    mod_api::WorldDepth::Tested,
                ));
            }
        }
    }
}

fn speed(pos: [i32; 3]) -> f32 {
    CLIENT.with_borrow(|c| c.speeds.get(&pos).copied().unwrap_or(0.0))
}

fn block_item(host: &dyn Host, id: &str) -> bool {
    if let Some(b) = CLIENT.with_borrow(|c| c.block_items.get(id).copied()) {
        return b;
    }
    let b = is_block_item(host, id);
    CLIENT.with_borrow_mut(|c| c.block_items.insert(id.to_owned(), b));
    b
}

/// The facts of the Create state `state` on this client.
fn table_info(
    host: &dyn Host,
    state: mod_api::BlockState,
) -> Option<(Block, create_engine::parts::StateInfo)> {
    let id = states::id(state)?;
    super::super::client_table(host, |t| t.get(id).map(|(b, i)| (b, *i))).flatten()
}

/// Whether the logistics block at `pos` is drawn again next frame instead of kept: an
/// ejector or arm (they move their own parts), a depot whose held stack slides, a belt's
/// controller carrying items. The server touches the block entity (`SHOWN`) when what a kept
/// drawing shows changes, so it is drawn once more from the contents it sent first.
pub fn animated(block: Block, pos: [i32; 3]) -> bool {
    match block {
        Block::MechanicalArm | Block::WeightedEjector => true,
        Block::Depot => CLIENT.with_borrow(|c| c.sliding.contains(&pos)),
        Block::Belt => CLIENT.with_borrow(|c| {
            c.belts
                .get(&pos)
                .is_some_and(|b| !b.contents.items.is_empty())
        }),
        _ => false,
    }
}

/// Depots' and belts' contents from the server.
pub fn logistics_message(_host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    match channel {
        depot::CHANNEL => {
            depots_from_server(data);
            true
        }
        belt::CHANNEL => {
            belts_from_server(data);
            true
        }
        _ => false,
    }
}

/// Arms and ejectors from the server.
pub fn arms_message(_host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
    match channel {
        arm::CHANNEL => {
            for s in arm::decode(data) {
                CLIENT.with_borrow_mut(|c| {
                    if s.gone {
                        c.arms.remove(&s.pos);
                    } else {
                        let progress = s.progress;
                        c.arms.insert(
                            s.pos,
                            ClientArm {
                                shown: s,
                                progress,
                                prev: progress,
                            },
                        );
                    }
                });
            }
            true
        }
        ejector::CHANNEL => {
            for s in ejector::decode(data) {
                CLIENT.with_borrow_mut(|c| {
                    if s.gone {
                        c.ejectors.remove(&s.pos);
                    } else {
                        c.ejectors
                            .insert(s.pos, ClientEjector { shown: s, ticks: 0 });
                    }
                });
            }
            true
        }
        _ => false,
    }
}

fn depots_from_server(data: &[u8]) {
    for c in depot::decode(data) {
        let pos = c.pos;
        CLIENT.with_borrow_mut(|cl| {
            cl.sliding.retain(|p| *p != pos);
            if c.stacks.is_empty() {
                cl.depots.remove(&pos);
                return;
            }
            if c.held.sliding() && c.stacks.iter().any(|s| s.0 == 0) {
                cl.sliding.push(pos);
            }
            cl.depots.insert(
                pos,
                ClientDepot {
                    prev: c.held,
                    contents: c,
                },
            );
        });
    }
}

fn belts_from_server(data: &[u8]) {
    for contents in belt::decode(data) {
        let c = contents.controller;
        CLIENT.with_borrow_mut(|cl| {
            if contents.length == 0 {
                cl.belts.remove(&c);
                return;
            }
            let prev = contents.items.items.iter().map(|i| i.position).collect();
            cl.belts.insert(c, ClientBelt { contents, prev });
        });
    }
}

/// Each client tick: held stacks still sliding move on a tick (`DepotBehaviour.tick` on the
/// client).
pub fn tick_client_depots(host: &mut dyn ClientHost) {
    let sliding = CLIENT.with_borrow(|c| c.sliding.clone());
    for pos in sliding {
        let Some(held) = CLIENT.with_borrow(|c| {
            let d = c.depots.get(&pos)?;
            let item = d.contents.stacks.iter().find(|s| s.0 == 0)?.1.clone();
            Some((d.contents.held, item))
        }) else {
            CLIENT.with_borrow_mut(|c| c.sliding.retain(|p| *p != pos));
            continue;
        };
        let (mut now, item) = held;
        let prev = now;
        let up = super::items::upright(&*host, &item);
        now.slide(up);
        let still = now.sliding();
        CLIENT.with_borrow_mut(|c| {
            if let Some(d) = c.depots.get_mut(&pos) {
                d.contents.held = now;
                d.prev = prev;
            }
            if !still {
                c.sliding.retain(|p| *p != pos);
            }
        });
    }
}

/// A belt's surroundings on the client: its ending as the server last said, and the belt
/// funnels over it, which hold the items they face on the client too (`checkForFunnels` on the
/// client: blocking, never inserting).
struct ClientSide<'a> {
    host: &'a dyn ClientHost,
    level: Option<mod_api::LevelRef>,
    ending: belt::Ending,
    controller: [i32; 3],
    segment: belt::Segment,
    length: i32,
    movement: Dir,
}

impl belt::Surroundings for ClientSide<'_> {
    fn ending(&mut self) -> belt::Ending {
        self.ending
    }
    fn insert(&mut self, _: &belt::Item) -> Option<u8> {
        None
    }
    fn process(&mut self, _: i32, item: &mut belt::Item, locked: bool) -> Option<belt::Process> {
        (locked || item.locked).then_some(belt::Process::Hold)
    }
    fn funnel(&mut self, segment: i32) -> Option<(create_engine::funnel::OnBelt, Dir)> {
        let at = belt::position_for_offset(self.controller, &self.segment, self.length, segment);
        let state = self
            .level?
            .block_state(self.host, [at[0], at[1] + 1, at[2]])?;
        let (block, info) = table_info(self.host, state)?;
        if !matches!(block, Block::AndesiteBeltFunnel | Block::BrassBeltFunnel) {
            return None;
        }
        let on = create_engine::funnel::OnBelt {
            facing: info.facing?,
            shape: info.belt_funnel.unwrap_or_default(),
            powered: info.powered,
        };
        Some((on, self.movement))
    }
}

/// Each client tick: belts with items move them on (`BeltInventory.tick` on the client).
pub fn tick_client_belts(host: &mut dyn ClientHost) {
    let busy: Vec<[i32; 3]> = CLIENT.with_borrow(|c| {
        c.belts
            .iter()
            .filter(|(_, b)| !b.contents.items.items.is_empty())
            .map(|(k, _)| *k)
            .collect()
    });
    if busy.is_empty() {
        return;
    }
    let level = host.local_player().and_then(|p| p.level(&*host));
    for c in busy {
        let Some(mut b) = CLIENT.with_borrow_mut(|cl| cl.belts.remove(&c)) else {
            continue;
        };
        let rpm = speed(c);
        let contents = &mut b.contents;
        let speed = belt::direction_aware_speed(rpm, contents.segment.facing);
        let horizontal = contents.segment.slope == belt::Slope::Horizontal;
        let mut side = ClientSide {
            host: &*host,
            level,
            ending: contents.ending,
            controller: c,
            segment: contents.segment,
            length: i32::from(contents.length),
            movement: belt::movement_facing(rpm, contents.segment.facing),
        };
        b.prev = contents.items.items.iter().map(|i| i.position).collect();
        contents
            .items
            .tick(i32::from(contents.length), speed, horizontal, &mut side);
        CLIENT.with_borrow_mut(|cl| cl.belts.insert(c, b));
    }
}

/// Each client tick: moving arms move on; ejectors with stacks in flight move them on, and a
/// plate winding back stops when closed.
pub fn tick_client_arms(_host: &mut dyn ClientHost) {
    CLIENT.with_borrow_mut(|c| {
        let speeds = &c.speeds;
        for (pos, a) in c.arms.iter_mut() {
            a.prev = a.progress;
            if a.progress < 1.0 {
                let s = speeds.get(pos).copied().unwrap_or(0.0);
                a.progress = (a.progress + arm::step(s)).min(1.0);
            }
        }
        for e in c.ejectors.values_mut() {
            let busy = !e.shown.flying.is_empty()
                || (e.shown.state == ejector::State::Retracting
                    && e.shown.lid > 0.0
                    && e.shown.wind_up > 0.0);
            if !busy {
                continue;
            }
            e.ticks += 1;
            if e.shown.state == ejector::State::Retracting && e.shown.lid > 0.0 {
                e.shown.lid = (e.shown.lid - e.shown.wind_up).max(0.0);
            }
            let max = e.shown.max_time();
            let landed = e
                .shown
                .flying
                .iter()
                .all(|(t, _)| (*t + e.ticks) as f32 > max + 1.0);
            if landed {
                e.shown.flying.clear();
            }
        }
    });
}

/// `m` (a move, a turn and a scale along its own axes, as the engine's renderer poses are) as
/// those three: the move, the turn's axis and angle in degrees (None for none), the scale.
/// None for a pose that flattens everything.
#[allow(clippy::type_complexity)]
fn decompose(m: Mat4) -> Option<([f32; 3], Option<([f32; 3], f32)>, [f32; 3])> {
    let a = m.to_cols_array();
    let col = |i: usize| Vec3::new(a[4 * i], a[4 * i + 1], a[4 * i + 2]);
    let (c0, c1, c2) = (col(0), col(1), col(2));
    let mut s = [c0.length(), c1.length(), c2.length()];
    if s.iter().any(|v| v.abs() < 1e-6) {
        return None;
    }
    if c0.cross(c1).dot(c2) < 0.0 {
        s[0] = -s[0];
    }
    let (x, y, z) = (c0 * (1.0 / s[0]), c1 * (1.0 / s[1]), c2 * (1.0 / s[2]));
    // The turn's quaternion (Shepperd's method), then its axis and angle.
    let (m00, m11, m22) = (x.x, y.y, z.z);
    let trace = m00 + m11 + m22;
    let (qw, qx, qy, qz) = if trace > 0.0 {
        let t = (trace + 1.0).sqrt() * 2.0;
        (0.25 * t, (y.z - z.y) / t, (z.x - x.z) / t, (x.y - y.x) / t)
    } else if m00 > m11 && m00 > m22 {
        let t = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        ((y.z - z.y) / t, 0.25 * t, (y.x + x.y) / t, (z.x + x.z) / t)
    } else if m11 > m22 {
        let t = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        ((z.x - x.z) / t, (y.x + x.y) / t, 0.25 * t, (z.y + y.z) / t)
    } else {
        let t = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        ((x.y - y.x) / t, (z.x + x.z) / t, (z.y + y.z) / t, 0.25 * t)
    };
    let half = qw.clamp(-1.0, 1.0).acos();
    let sin = half.sin();
    let turn =
        (sin.abs() > 1e-6).then(|| ([qx / sin, qy / sin, qz / sin], (2.0 * half).to_degrees()));
    Some(([a[12], a[13], a[14]], turn, s))
}

/// Applies the pose `m` to what is drawn next.
fn pose(draw: &mut dyn BlockEntityDrawContext, m: Mat4) {
    let Some((at, turn, s)) = decompose(m) else {
        return;
    };
    draw.translate(at);
    if let Some((axis, degrees)) = turn {
        draw.rotate_about(axis, degrees, [0.0; 3]);
    }
    if s != [1.0; 3] {
        draw.scale(s);
    }
}

const UP: [f32; 3] = [0.0, 1.0, 0.0];
const CENTRE: [f32; 3] = [0.5, 0.5, 0.5];

fn axis_vector(axis: Axis) -> [f32; 3] {
    match axis {
        Axis::X => [1.0, 0.0, 0.0],
        Axis::Y => [0.0, 1.0, 0.0],
        Axis::Z => [0.0, 0.0, 1.0],
    }
}

/// A partial model's block (`create:part_*`), at its `up` state.
fn partial(draw: &mut dyn BlockEntityDrawContext, name: &str) {
    if let Some(b) = states::partial(name) {
        draw.block(b, &[("facing", "up")]);
    }
}

/// Draws `count` of item `id` as it lies (`ItemDisplayContext.FIXED`).
fn item(draw: &mut dyn BlockEntityDrawContext, host: &dyn Host, id: &str) {
    if let Some(stack) = stack_of(host, id, 1) {
        draw.item(&stack, ItemDisplayContext::Fixed, None, 0);
    }
}

/// The speeds a kinetic block entity's data holds (`kinetics`' `Speeds`).
fn speeds_of(data: &Compound) -> [f32; 2] {
    match data.get(&"Speeds".into()) {
        Some(Tag::List(l)) => {
            let at = |i: usize| match l.get(i) {
                Some(Tag::Float(bits)) => f32::from_bits(*bits),
                _ => 0.0,
            };
            [at(0), at(1)]
        }
        _ => [0.0; 2],
    }
}

/// The kinetic block entity's second renderer: what logistics draws.
pub struct LogisticsEntity;

#[mod_api::horizon_part]
impl BlockPart for LogisticsEntity {}

#[mod_api::horizon_part]
impl BlockEntityPart for LogisticsEntity {
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        let Some((block, info)) = table_info(host, event.state) else {
            return;
        };
        if matches!(
            block,
            Block::BrassFunnel | Block::BrassBeltFunnel | Block::SmartChute
        ) {
            draw_filter(&mut *event.draw, host, block, &info, &event.entity.nbt);
            return;
        }
        if !matches!(
            block,
            Block::Belt | Block::Depot | Block::WeightedEjector | Block::MechanicalArm
        ) {
            return;
        }
        let pos = event.pos;
        let speeds = speeds_of(&event.entity.nbt);
        CLIENT.with_borrow_mut(|c| c.speeds.insert(pos, speeds[0]));
        let draw = &mut *event.draw;
        let partial_tick = draw.partial();
        match block {
            Block::Belt => {
                draw_segment(draw, block, &info, pos, speeds);
                draw_belt_items(draw, host, pos, partial_tick);
            }
            Block::Depot => draw_depot(draw, host, pos, partial_tick),
            Block::WeightedEjector => {
                draw_depot(draw, host, pos, partial_tick);
                if let Some(facing) = info.facing {
                    draw_ejector(draw, host, pos, facing, partial_tick);
                }
            }
            Block::MechanicalArm => draw_arm(draw, host, pos, partial_tick),
            _ => {}
        }
    }

    /// A belt's controller draws all its items: it is in view while any of the belt is.
    fn block_entity_render_bounding_box(
        &self,
        _host: &dyn ClientHost,
        event: &mut BlockEntityRenderBoundingBoxEvent<'_>,
    ) {
        let pos = event.pos;
        let Some((seg, length)) = CLIENT.with_borrow(|c| {
            c.belts
                .get(&pos)
                .map(|b| (b.contents.segment, i32::from(b.contents.length)))
        }) else {
            return;
        };
        let ends = [0, length - 1].map(|i| belt::position_for_offset(pos, &seg, length, i));
        let b = &mut *event.bounds;
        for e in ends {
            for k in 0..3 {
                let rel = (e[k] - pos[k]) as f32;
                b[k] = b[k].min(rel);
                b[k + 3] = b[k + 3].max(rel + 1.0);
            }
        }
    }
}

/// `BeltRenderer`: the segment's halves (still: Create scrolls their texture instead), and the
/// pulley turning on the belt's axis, posed partials `kinetics` leaves to logistics.
fn draw_segment(
    draw: &mut dyn BlockEntityDrawContext,
    block: Block,
    info: &create_engine::parts::StateInfo,
    pos: [i32; 3],
    speeds: [f32; 2],
) {
    for d in parts::draws(block, info, pos, speeds) {
        match d.model {
            Model::Belt(seg, bottom) => {
                let Some(name) = belt::partial(&seg, bottom) else {
                    continue;
                };
                draw.push();
                pose(draw, belt::segment_pose(&seg));
                partial(draw, name);
                draw.pop();
            }
            Model::BeltPulley(seg) => {
                draw.push();
                draw.spin(axis_vector(d.axis), CENTRE, d.rpm * 0.3, d.phase);
                pose(draw, belt::pulley_pose(&seg));
                partial(draw, "part_belt_pulley");
                draw.still();
                draw.pop();
            }
            _ => {}
        }
    }
}

/// Draws a belt's items as `BeltRenderer.renderItem` does: each at its point on the belt
/// (between where it was a tick ago and where it is), tilted on a slope, turned by its angle,
/// its copies under it.
fn draw_belt_items(draw: &mut dyn BlockEntityDrawContext, host: &dyn Host, c: [i32; 3], t: f32) {
    let Some((contents, prev)) = CLIENT.with_borrow(|cl| {
        cl.belts
            .get(&c)
            .map(|b| (b.contents.clone(), b.prev.clone()))
    }) else {
        return;
    };
    let length = i32::from(contents.length);
    let same = prev.len() == contents.items.items.len();
    for (i, it) in contents.items.items.iter().enumerate() {
        let at = if same {
            prev[i] + (it.position - prev[i]) * t
        } else {
            it.position
        };
        let place = belt::item_place(&contents.segment, length, at, it.side_offset);
        let solid = block_item(host, &it.item);
        let copies = belt::item_copies(u32::from(it.count), solid, it.angle);
        draw.push();
        draw.translate([place.at.x, place.at.y, place.at.z]);
        if place.tilt != 0.0 {
            draw.rotate_about(axis_vector(place.tilt_axis), place.tilt, [0.0; 3]);
        }
        draw.rotate_about(UP, it.angle as f32, [0.0; 3]);
        for copy in copies {
            draw.push();
            pose(draw, copy);
            item(draw, host, &it.item);
            draw.pop();
        }
        draw.pop();
    }
}

/// Draws a depot's stacks as Create's `DepotRenderer` does: the held stack at its offset, turned
/// by its angle; each output 0.35 out at 45° steps.
fn draw_depot(draw: &mut dyn BlockEntityDrawContext, host: &dyn Host, pos: [i32; 3], t: f32) {
    let Some((contents, prev)) =
        CLIENT.with_borrow(|c| c.depots.get(&pos).map(|d| (d.contents.clone(), d.prev)))
    else {
        return;
    };
    for (slot, id, count) in &contents.stacks {
        let solid = block_item(host, id);
        let (poses, phase, offset) = if *slot == 0 {
            let (from, to) = (prev.offset(), contents.held.offset());
            let at = from + (to - from) * t;
            let poses =
                depot::stack_poses(u32::from(*count), solid, &mut depot::JavaRandom::new(0));
            (poses, contents.held.angle as f32, at)
        } else {
            let (poses, phase) =
                depot::output_place(usize::from(*slot) - 1, u32::from(*count), solid);
            (poses, phase, Vec3::new(0.0, 0.0, 0.0))
        };
        draw.push();
        draw.translate([offset.x, offset.y, offset.z]);
        draw.rotate_about(UP, phase, [0.5, 0.0, 0.5]);
        for p in poses {
            draw.push();
            pose(draw, depot::top_centre() * p);
            item(draw, host, id);
            draw.pop();
        }
        draw.pop();
    }
}

/// Draws an arm as `ArmRenderer` poses it at its progress: every part turned about the
/// block's vertical axis by the base's yaw, its joints, and the item in its claw.
fn draw_arm(draw: &mut dyn BlockEntityDrawContext, host: &dyn Host, pos: [i32; 3], t: f32) {
    let Some((s, progress)) = CLIENT.with_borrow(|c| {
        c.arms.get(&pos).map(|a| {
            let shown = arm::Shown {
                held: a.shown.held.clone(),
                points: Vec::new(),
                ..a.shown
            };
            (shown, a.prev + (a.progress - a.prev) * t)
        })
    }) else {
        return;
    };
    let mut now = arm::at_progress(s.from, s.from_base, s.to, progress);
    // `rave`: a dancing arm that turns sways to the music (Create also tints it with a rainbow,
    // which a partial block drawn here cannot take).
    let speed = CLIENT.with_borrow(|c| c.speeds.get(&pos).copied().unwrap_or(0.0));
    if s.phase == arm::Phase::Dancing && speed != 0.0 {
        let seed = (pos[0].wrapping_mul(31) ^ pos[1].wrapping_mul(17) ^ pos[2]).rem_euclid(64);
        let r = draw.game_time() as f32 + t + seed as f32;
        let lerp = |p: f32, a: f32, b: f32| a + (b - a) * p;
        let lower = lerp((mth_sin(r / 4.0) + 1.0) / 2.0, -45.0, 15.0);
        let upper = lerp((mth_sin(r / 8.0) + 1.0) / 4.0, -45.0, 95.0);
        now = arm::Angles {
            base: (r * 10.0) % 360.0,
            lower: lower + 135.0,
            upper: upper + 90.0,
            head: -lower,
        };
    }
    let held = s.held.as_ref().map(|i| block_item(host, i));
    let p = arm::poses(&now, s.ceiling, held);
    let sign = if s.ceiling { -1.0 } else { 1.0 };
    draw.push();
    draw.rotate_about(UP, sign * now.base, [0.5, 0.0, 0.5]);
    let parts = [
        ("part_arm_base", p.base),
        ("part_arm_lower_body", p.lower),
        ("part_arm_upper_body", p.upper),
        ("part_arm_claw_base", p.claw),
        ("part_arm_upper_claw_grip", p.upper_grip),
        ("part_arm_lower_claw_grip", p.lower_grip),
    ];
    for (name, m) in parts {
        draw.push();
        pose(draw, m);
        partial(draw, name);
        draw.pop();
    }
    if let Some(id) = &s.held {
        draw.push();
        pose(draw, p.item);
        item(draw, host, id);
        draw.pop();
    }
    draw.pop();
}

/// Draws an ejector's plate (`EjectorRenderer.applyLidAngle`: turned to its facing, swung about
/// its hinge by `lid × 70°`) and its stacks in flight along the arc, tumbling
/// (`rotateXDegrees(time × 40)`) about the axis across its facing.
fn draw_ejector(
    draw: &mut dyn BlockEntityDrawContext,
    host: &dyn Host,
    pos: [i32; 3],
    facing: Dir,
    t: f32,
) {
    let Some((s, ticks)) = CLIENT.with_borrow(|c| {
        c.ejectors.get(&pos).map(|e| {
            let shown = ejector::Shown {
                flying: e.shown.flying.clone(),
                ..e.shown
            };
            (shown, e.ticks)
        })
    }) else {
        return;
    };
    let centre = Vec3::new(0.5, 0.5, 0.5);
    let turn = ejector::horizontal_angle(facing);
    let (axis, sign, hinge) = ejector::lid_hinge(facing);
    let hinge = Vec3::new(hinge[0], hinge[1], hinge[2]);
    let axis_of = |i: u8| match i {
        0 => [1.0, 0.0, 0.0],
        1 => [0.0, 1.0, 0.0],
        _ => [0.0, 0.0, 1.0],
    };
    // Winding back toward closed at `wind_up` a tick.
    let lid = if s.state == ejector::State::Retracting && s.lid > 0.0 {
        (s.lid - s.wind_up * t).max(0.0)
    } else {
        s.lid
    };
    let plate = Mat4::from_translation(centre - hinge)
        * Mat4::from_translation(centre)
        * Mat4::from_rotation_y((180.0 + turn).to_radians())
        * Mat4::from_translation(-centre);
    let off = hinge - centre;
    draw.push();
    draw.translate([off.x, off.y, off.z]);
    draw.rotate_about(axis_of(axis), sign * lid * 70.0, CENTRE);
    pose(draw, plate);
    partial(draw, "part_ejector_top");
    draw.pop();
    // Stacks in flight.
    let launcher = ejector::Launcher::new(s.h, s.v);
    let max = s.max_time();
    let item_offset = Vec3::new(0.0, 2.0 / 16.0, -1.0 / 16.0);
    // The tumble's axis: x turned to the facing (`rotateYDegrees(horizontalAngle)`).
    let x = ejector::rotate_y([1.0, 0.0, 0.0], f64::from(turn));
    let (tumble, tsign) = if x[0].abs() > 0.5 {
        (0u8, x[0].signum() as f32)
    } else {
        (2u8, x[2].signum() as f32)
    };
    let away = facing.opposite();
    for (t0, id) in &s.flying {
        let time = (*t0 + ticks) as f32 + t;
        if time > max {
            continue;
        }
        let here = launcher.global_pos(f64::from(time), away, pos);
        let rel = Vec3::new(
            (here[0] - f64::from(pos[0])) as f32,
            (here[1] - f64::from(pos[1])) as f32,
            (here[2] - f64::from(pos[2])) as f32,
        );
        let m = Mat4::from_translation(centre)
            * Mat4::from_scale(Vec3::new(0.5, 0.5, 0.5))
            * Mat4::from_rotation_y(turn.to_radians())
            * Mat4::from_translation(-item_offset);
        let off = rel + item_offset - centre;
        draw.push();
        draw.translate([off.x, off.y, off.z]);
        draw.rotate_about(axis_of(tumble), tsign * time * 40.0, CENTRE);
        pose(draw, m);
        item(draw, host, id);
        draw.pop();
    }
}

/// Draws the filter a brass funnel or a smart chute shows in its slot (`FilteringRenderer`),
/// from its block entity's `FILTER`: on each side the slot is on, turned to face out of it
/// (`ValueBoxTransform.Sided.rotate`; a horizontal funnel's tilted back 22.5°), at half the
/// value box's half scale for a flat item and its half scale for a block.
fn draw_filter(
    draw: &mut dyn BlockEntityDrawContext,
    host: &dyn Host,
    block: Block,
    info: &create_engine::parts::StateInfo,
    nbt: &Compound,
) {
    let Some(Tag::String(id)) = nbt.get(&super::FILTER.into()) else {
        return;
    };
    let id = id.to_string_lossy();
    if id.is_empty() {
        return;
    }
    let Some(stack) = stack_of(host, &id, 1) else {
        return;
    };
    let solid = block_item(host, &id);
    let scale = 0.5 * (if solid { 1.0 } else { 0.5 } + 1.0 / 64.0);
    // A slot on the face of a block that occludes (a smart chute here) would take the dark light
    // inside it; the light outside isn't readable on a client, so it is drawn fully lit.
    if draw.light() == 0 {
        draw.set_light(0xF0F0);
    }
    for side in [Dir::South, Dir::West, Dir::North, Dir::East, Dir::Up] {
        let (slot, tilt) = match block {
            Block::SmartChute => (create_engine::chute::filter_slot(side), 0.0),
            _ => {
                let Some(facing) = info.facing else {
                    return;
                };
                let slot = create_engine::funnel::filter_slot(facing, info.belt_funnel, side);
                let horizontal = facing.axis() != Axis::Y;
                let tilted = horizontal
                    && info.belt_funnel.is_none_or(|s| {
                        matches!(
                            s,
                            create_engine::funnel::BeltShape::Pulling
                                | create_engine::funnel::BeltShape::Pushing
                        )
                    });
                (slot, if tilted { -22.5 } else { 0.0 })
            }
        };
        let Some(slot) = slot else {
            continue;
        };
        // `Sided.rotate`: the side's horizontal angle plus 180°, then 90° up for the top.
        let y_rot = match side {
            Dir::South | Dir::Up | Dir::Down => 0.0,
            Dir::West => 90.0,
            Dir::North => 180.0,
            Dir::East => 270.0,
        } + 180.0;
        let x_rot = if side == Dir::Up { 90.0 } else { 0.0 };
        draw.push();
        draw.translate(slot.map(|v| v as f32));
        draw.rotate_about(UP, y_rot, [0.0; 3]);
        draw.rotate_about([1.0, 0.0, 0.0], x_rot + tilt, [0.0; 3]);
        draw.scale([scale; 3]);
        if !solid {
            draw.translate([0.0, 0.0, -0.15]);
        }
        draw.item(&stack, ItemDisplayContext::Fixed, None, 0);
        draw.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::decompose;
    use create_engine::math::{Mat4, Quat, Vec3};

    /// The decomposed pose put back together moves points where the pose does.
    #[test]
    fn poses_decompose_into_move_turn_and_scale() {
        let poses = [
            Mat4::from_translation(Vec3::new(0.5, 15.0 / 16.0, 0.5))
                * Mat4::from_rotation_y(37f32.to_radians())
                * Mat4::from_rotation_x(200f32.to_radians())
                * Mat4::from_scale(Vec3::new(0.5, 0.5, 0.5)),
            Mat4::from_rotation_z(180f32.to_radians()) * Mat4::from_rotation_y(90f32.to_radians()),
            create_engine::belt::segment_pose(&create_engine::belt::Segment {
                facing: create_engine::kinetics::Dir::East,
                slope: create_engine::belt::Slope::Sideways,
                part: create_engine::belt::Part::Middle,
            }),
            Mat4::from_translation(Vec3::new(1.0, -2.0, 3.0)),
        ];
        for m in poses {
            let (at, turn, s) = decompose(m).expect("a pose");
            for p in [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.25, -0.5),
                Vec3::new(-0.3, 0.9, 0.1),
            ] {
                let scaled = Vec3::new(p.x * s[0], p.y * s[1], p.z * s[2]);
                let turned = match turn {
                    Some((axis, deg)) => {
                        let axis = Vec3::new(axis[0], axis[1], axis[2]).normalize();
                        Quat::from_axis_angle(axis, deg.to_radians()) * scaled
                    }
                    None => scaled,
                };
                let got = turned + Vec3::new(at[0], at[1], at[2]);
                let want = m.transform_point3(p);
                assert!((got - want).length() < 1e-4, "{got:?} != {want:?}");
            }
        }
    }
}
