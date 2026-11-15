//! The factory gauge on clients: its panels drawn from its block entity's data
//! (`FactoryPanelModel`, `FactoryPanelRenderer`, `FilteringRenderer`), which panel the crosshair
//! is on, the bulbs' glow (`bulb`, a `LerpedFloat` chasing satisfaction), and joining two panels
//! (`FactoryPanelConnectionHandler`: the panel the screen's `+` was pressed on, outlined until a
//! second one is clicked).

use super::super::net;
use super::super::states;
use super::gauge::PANELS;
use super::gauge_screen::{self, CHANNEL};
use super::keeper::put_i32;
use create_engine::Block;
use create_engine::factory::{self as fx, SLOTS};
use create_engine::kinetics::Dir;
use create_engine::steam::Face;
use create_engine::stock::PanelPos;
use mod_api::{
    BlockEntityDrawContext, BlockEntityPart, BlockEntitySubmitEvent, BlockPart, ClientHost,
    Compound, Host, ItemDisplayContext, ItemStack, Tag, TerrainModelOptions,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// One input of a panel as its clients see it.
#[derive(Clone, Debug)]
pub(super) struct ShownInput {
    pub from: PanelPos,
    pub amount: u32,
    pub bend: i32,
    pub success: bool,
}

/// One panel as its clients see it (`FactoryPanelBehaviour`'s synced fields).
#[derive(Clone, Debug, Default)]
pub(super) struct Shown {
    pub slot: u8,
    pub filter: Option<ItemStack>,
    pub count: u32,
    pub up_to: bool,
    pub satisfied: bool,
    pub promised_satisfied: bool,
    pub waiting: bool,
    pub powered: bool,
    pub restocker: bool,
    pub address: String,
    pub output: u32,
    pub level: i32,
    pub promised: i32,
    pub clearing: i32,
    pub flash: i32,
    /// Whether it feeds another panel (`targeting`).
    pub feeds: bool,
    pub inputs: Vec<ShownInput>,
    /// `targetedByLinks`: the display and redstone links joined to it (their slot and arrow's
    /// bend).
    pub links: Vec<ShownInput>,
    pub craft: Vec<ItemStack>,
}

impl Shown {
    pub fn status(&self) -> fx::Status {
        fx::Status {
            count: self.count,
            missing_address: fx::missing_address(
                !self.inputs.is_empty(),
                self.restocker,
                self.count,
                &self.address,
            ),
            redstone_powered: self.powered,
            waiting: self.waiting,
            satisfied: self.satisfied,
            promised_satisfied: self.promised_satisfied,
        }
    }
}

fn int(c: &Compound, k: &str) -> i32 {
    match c.get(&k.into()) {
        Some(Tag::Int(v)) => *v,
        Some(Tag::Byte(v)) => i32::from(*v),
        _ => 0,
    }
}

fn flag(c: &Compound, k: &str) -> bool {
    int(c, k) != 0
}

/// A gauge's panels from its block entity's data.
pub(super) fn shown_of(host: &dyn Host, nbt: &Compound) -> Vec<Shown> {
    let Some(Tag::List(list)) = nbt.get(&PANELS.into()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|t| {
            let Tag::Compound(c) = t else { return None };
            let list = |key: &str| match c.get(&key.into()) {
                Some(Tag::List(l)) => l
                    .iter()
                    .filter_map(|t| {
                        let Tag::Compound(i) = t else { return None };
                        let p = super::ints(i.get(&"Pos".into()));
                        (p.len() == 3).then(|| ShownInput {
                            from: ([p[0], p[1], p[2]], int(i, "Slot").clamp(0, 3) as u8),
                            amount: int(i, "Amount").max(0) as u32,
                            bend: int(i, "Bend"),
                            success: flag(i, "Success"),
                        })
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let links = list("Links");
            let inputs = match c.get(&"Inputs".into()) {
                Some(Tag::List(l)) => l
                    .iter()
                    .filter_map(|t| {
                        let Tag::Compound(i) = t else { return None };
                        let p = super::ints(i.get(&"Pos".into()));
                        (p.len() == 3).then(|| ShownInput {
                            from: ([p[0], p[1], p[2]], int(i, "Slot").clamp(0, 3) as u8),
                            amount: int(i, "Amount").max(0) as u32,
                            bend: int(i, "Bend"),
                            success: flag(i, "Success"),
                        })
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let craft = match c.get(&"Craft".into()) {
                Some(Tag::List(l)) => l
                    .iter()
                    .filter_map(|t| super::body::from_nbt(host, t))
                    .collect(),
                _ => Vec::new(),
            };
            Some(Shown {
                slot: int(c, "Slot").clamp(0, 3) as u8,
                filter: c
                    .get(&"Filter".into())
                    .and_then(|t| super::body::from_nbt(host, t)),
                count: int(c, "Count").max(0) as u32,
                up_to: flag(c, "UpTo"),
                satisfied: flag(c, "Satisfied"),
                promised_satisfied: flag(c, "PromisedSatisfied"),
                waiting: flag(c, "Waiting"),
                powered: flag(c, "Powered"),
                restocker: flag(c, "Restocker"),
                address: super::text(c.get(&"Address".into())),
                output: int(c, "Output").max(1) as u32,
                level: int(c, "Level"),
                promised: int(c, "Promised"),
                clearing: int(c, "Clearing"),
                flash: int(c, "Flash"),
                feeds: flag(c, "Feeds"),
                inputs,
                links,
                craft,
            })
        })
        .collect()
}

/// The panels of the gauge at `pos` on this client.
pub(super) fn panels(host: &dyn ClientHost, pos: [i32; 3]) -> Vec<Shown> {
    host.level(host.dimension())
        .and_then(|l| host.block_entity(l, pos))
        .and_then(|e| e.data(host))
        .map(|d| shown_of(host, &d))
        .unwrap_or_default()
}

pub(super) fn panel(host: &dyn ClientHost, pp: PanelPos) -> Option<Shown> {
    panels(host, pp.0).into_iter().find(|p| p.slot == pp.1)
}

/// The face and facing of the gauge at `pos` on this client.
pub(super) fn orientation(host: &dyn ClientHost, pos: [i32; 3]) -> Option<(Face, Dir)> {
    let state = host.level(host.dimension())?.block_state(host, pos)?;
    let id = states::id(state)?;
    super::super::client_table(host, |t| {
        let (b, i) = t.get(id)?;
        (b == Block::FactoryGauge).then_some(())?;
        Some((i.face?, i.facing?))
    })
    .flatten()
}

/// The active panel whose value box a hit at `point` lands on, and whether it has an item.
pub(crate) fn hit(host: &dyn ClientHost, pos: [i32; 3], point: [f64; 3]) -> Option<(u8, bool)> {
    let (face, facing) = orientation(host, pos)?;
    let local = [0, 1, 2].map(|i| point[i] - f64::from(pos[i]));
    let shown = panels(host, pos);
    SLOTS.into_iter().find_map(|s| {
        let p = shown.iter().find(|p| p.slot == s)?;
        fx::hits_slot(face, facing, s, local).then_some((s, p.filter.is_some()))
    })
}

/// The board's starting pick for a panel (`getValueSettings`): row 0 items, 1 stacks.
pub(crate) fn board_pick(host: &dyn ClientHost, pos: [i32; 3], slot: u8) -> (i32, i32) {
    panel(host, (pos, slot)).map_or((1, 0), |p| (i32::from(!p.up_to), p.count as i32))
}

/// The board's pick, sent on release (`ValueSettingsPacket`).
pub(crate) fn send_amount(
    host: &mut dyn ClientHost,
    pos: [i32; 3],
    slot: u8,
    row: i32,
    value: i32,
) {
    let mut m = vec![b'V'];
    for v in pos {
        put_i32(&mut m, v);
    }
    m.push(slot);
    put_i32(&mut m, row);
    put_i32(&mut m, value);
    net::send_to_server(host, CHANNEL, &m);
}

fn put_pp(m: &mut Vec<u8>, pp: PanelPos) {
    for v in pp.0 {
        put_i32(m, v);
    }
    m.push(pp.1);
}

// ---------------------------------------------------------------------------------------------
// Joining panels

#[derive(Default)]
struct Client {
    /// `connectingFrom`, and the ticks since its message was last shown.
    connecting: Option<(PanelPos, u32)>,
    /// `relocating`, and `validRelocationTarget` this tick.
    relocating: bool,
    target: Option<PanelPos>,
    /// Each panel's bulb (`bulb`: value, last tick's value) and the flashes seen.
    bulbs: HashMap<PanelPos, (f32, f32, i32)>,
    models: HashMap<&'static str, mod_api::ModelRef>,
}

thread_local! {
    static CLIENT: RefCell<Client> = RefCell::new(Client::default());
}

pub(super) fn reset() {
    CLIENT.with_borrow_mut(|c| *c = Client::default());
}

/// `FactoryPanelConnectionHandler.startConnection` (the screen's `+`), or `startRelocating`
/// (its move button).
pub(super) fn start_connection(host: &mut dyn ClientHost, from: PanelPos, relocate: bool) {
    CLIENT.with_borrow_mut(|c| {
        c.connecting = Some((from, 0));
        c.relocating = relocate;
        c.target = None;
    });
    let mut m = vec![b'S'];
    put_pp(&mut m, from);
    m.push(u8::from(relocate));
    net::send_to_server(host, CHANNEL, &m);
}

/// Ends the joining or relocating.
fn stop() {
    CLIENT.with_borrow_mut(|c| {
        c.connecting = None;
        c.relocating = false;
        c.target = None;
    });
}

/// `clientTick` while relocating: the panel the crosshair is on, a 32nd of a block into the
/// face it hits (`getTargetedSlot`), on the gauge's surface (no step along its facing), where a
/// gauge as this one could hang (a support behind it: the server checks it is sturdy) and not on
/// a packager.
fn relocation_target(host: &dyn ClientHost, from: PanelPos) -> Option<PanelPos> {
    let Some(mod_api::CrosshairTarget::Block { hit, .. }) = host.crosshair_target() else {
        return None;
    };
    let (face, facing) = orientation(host, from.0)?;
    let offset = [0, 1, 2].map(|i| hit.point[i] + f64::from(hit.face[i]) / 32.0);
    let pos = offset.map(|v| v.floor() as i32);
    let local = [0, 1, 2].map(|i| offset[i] - f64::from(pos[i]));
    let slot = fx::targeted_slot(face, facing, local);
    let connected = fx::connected_direction(face, facing);
    let diff = [0, 1, 2].map(|i| pos[i] - from.0[i]);
    let along = match connected.axis() {
        create_engine::kinetics::Axis::X => diff[0],
        create_engine::kinetics::Axis::Y => diff[1],
        create_engine::kinetics::Axis::Z => diff[2],
    };
    if along != 0 {
        return None;
    }
    let level = host.level(host.dimension())?;
    let o = connected.offset();
    let behind = [pos[0] - o[0], pos[1] - o[1], pos[2] - o[2]];
    let here = level.block_state(host, pos)?;
    let free = mod_api::HostBlockStates::block_states(host).replaceable(here) == Some(true)
        || orientation(host, pos) == Some((face, facing));
    let support = level
        .block_state(host, behind)
        .is_some_and(|s| mod_api::HostBlockStates::block_states(host).replaceable(s) != Some(true));
    if !free || !support {
        return None;
    }
    let packager = level
        .block_state(host, behind)
        .and_then(states::id)
        .and_then(|id| super::super::client_table(host, |t| t.get(id).map(|(b, _)| b)).flatten());
    if packager == Some(Block::Packager) {
        return None;
    }
    Some((pos, slot))
}

/// A short click on a panel with an item (`onShortInteract` on the client): a second panel
/// for the one being joined, else its screen (unless the hand holds a wrench or a linking
/// item, which the server takes). The server is told either way.
pub(crate) fn short_click(host: &mut dyn ClientHost, pos: [i32; 3], slot: u8, wrench: bool) {
    let pp = (pos, slot);
    let held = host
        .local_player()
        .and_then(|p| super::super::items::held(host, p, mod_api::Hand::Main))
        .map(|s| s.item().id(host));
    let feeds = panel(host, pp).is_some_and(|p| p.feeds);
    let mut m = vec![b'H'];
    put_pp(&mut m, pp);
    m.push(u8::from(wrench));
    net::send_to_server(host, CHANNEL, &m);
    if wrench && feeds {
        return;
    }
    if CLIENT.with_borrow(|c| c.relocating) {
        return;
    }
    if let Some((from, _)) = CLIENT.with_borrow(|c| c.connecting) {
        stop();
        if from == pp {
            let m = vec![b'A', 0];
            net::send_to_server(host, CHANNEL, &m);
            return;
        }
        let mut m = vec![b'L'];
        put_pp(&mut m, from);
        put_pp(&mut m, pp);
        net::send_to_server(host, CHANNEL, &m);
        return;
    }
    if held.as_deref().is_some_and(super::linked_item) {
        return;
    }
    gauge_screen::open_config(host, pp);
}

/// `FactoryPanelConnectionHandler.onRightClick` on a display or redstone link (one with a
/// `FactoryPanelSupportBehaviour`) while joining: the use key is taken (the link's screen does
/// not open) and the server checks and joins them.
pub(super) fn interaction_key(
    host: &mut dyn ClientHost,
    event: &mut mod_api::InteractionKeyMappingTriggeredEvent,
) -> mod_api::Flow {
    if event.key != mod_api::InteractionKey::Use || event.hand != mod_api::Hand::Main {
        return mod_api::Flow::Continue;
    }
    let Some((from, _)) = CLIENT.with_borrow(|c| c.connecting) else {
        return mod_api::Flow::Continue;
    };
    // `onRightClick` while relocating: the panel goes to the target, unless sneaking or there is
    // none (`relocation_aborted`).
    if CLIENT.with_borrow(|c| c.relocating) {
        if event.attempt == mod_api::InteractionAttempt::Press {
            let sneaking = host
                .local_player()
                .and_then(|p| p.entity().is_shift_key_down(host))
                .unwrap_or(false);
            let target = CLIENT.with_borrow(|c| c.target).filter(|_| !sneaking);
            stop();
            match target {
                Some(to) => {
                    let mut m = vec![b'R'];
                    put_pp(&mut m, to);
                    put_pp(&mut m, from);
                    net::send_to_server(host, CHANNEL, &m);
                }
                None => {
                    net::send_to_server(host, CHANNEL, &[b'A', 2]);
                }
            }
        }
        return mod_api::Flow::Cancel;
    }
    let Some(mod_api::CrosshairTarget::Block { pos, .. }) = host.crosshair_target() else {
        return mod_api::Flow::Continue;
    };
    let pos = [pos.x, pos.y, pos.z];
    if !matches!(
        crate::glue::redstone::client_block_of(host, pos),
        Some(Block::DisplayLink | Block::RedstoneLink)
    ) {
        return mod_api::Flow::Continue;
    }
    stop();
    if event.attempt == mod_api::InteractionAttempt::Press {
        let mut m = vec![b'D'];
        put_pp(&mut m, from);
        for v in pos {
            put_i32(&mut m, v);
        }
        net::send_to_server(host, CHANNEL, &m);
    }
    mod_api::Flow::Cancel
}

/// `FactoryPanelConnectionHandler.onRightClick` for clicks off a panel: a click on any block
/// but a gauge, or while sneaking, ends the joining. Whether the click was taken.
pub(super) fn right_click_off_panel(
    host: &mut dyn ClientHost,
    pos: [i32; 3],
    sneaking: bool,
) -> bool {
    if CLIENT.with_borrow(|c| c.connecting.is_none()) {
        return false;
    }
    let gauge = orientation(host, pos).is_some();
    if gauge && !sneaking {
        return false;
    }
    stop();
    let m = vec![b'A', 1];
    net::send_to_server(host, CHANNEL, &m);
    true
}

/// `FactoryPanelConnectionHandler.clientTick`: the joining ends 16 blocks away; the message
/// is shown again while it lasts. The bulbs chase their targets.
pub(super) fn tick(host: &mut dyn ClientHost) {
    let connecting = CLIENT.with_borrow_mut(|c| {
        c.connecting.as_mut().map(|(pp, t)| {
            *t += 1;
            (*pp, *t)
        })
    });
    if let Some((pp, t)) = connecting {
        let near = host
            .local_player()
            .and_then(|p| p.entity().position(host))
            .is_some_and(|at| {
                let d2: f64 = (0..3)
                    .map(|i| (at[i] - f64::from(pp.0[i]) - 0.5).powi(2))
                    .sum();
                d2 < 16.0 * 16.0
            });
        if !near || panel(host, pp).is_none() {
            stop();
        } else {
            let relocating = CLIENT.with_borrow(|c| c.relocating);
            if t % 40 == 0 {
                let mut m = vec![b'S'];
                put_pp(&mut m, pp);
                m.push(u8::from(relocating));
                net::send_to_server(host, CHANNEL, &m);
            }
            let target = relocating.then(|| relocation_target(host, pp)).flatten();
            CLIENT.with_borrow_mut(|c| c.target = target);
        }
    }
    CLIENT.with_borrow_mut(|c| {
        for (v, prev, _) in c.bulbs.values_mut() {
            *prev = *v;
        }
    });
    let keys: Vec<PanelPos> = CLIENT.with_borrow(|c| c.bulbs.keys().copied().collect());
    for pp in keys {
        let Some(p) = panel(host, pp) else {
            CLIENT.with_borrow_mut(|c| c.bulbs.remove(&pp));
            continue;
        };
        let target = if p.powered || p.satisfied { 1.0 } else { 0.0 };
        CLIENT.with_borrow_mut(|c| {
            if let Some((v, _, flash)) = c.bulbs.get_mut(&pp) {
                if *flash != p.flash {
                    *flash = p.flash;
                    *v = 1.0;
                }
                // `Chaser.EXP` at 0.175.
                let d = (target - *v) * 0.175;
                *v = if (target - *v).abs() < 0.001 {
                    target
                } else {
                    *v + d
                };
            }
        });
    }
}

/// The glow of a panel's bulb this frame.
fn glow(pp: PanelPos, p: &Shown, t: f32) -> f32 {
    CLIENT.with_borrow_mut(|c| {
        let e = c.bulbs.entry(pp).or_insert_with(|| {
            let v = if p.powered || p.satisfied { 1.0 } else { 0.0 };
            (v, v, p.flash)
        });
        e.1 + (e.0 - e.1) * t
    })
}

/// Whether a gauge's bulbs are still moving (it is drawn again each frame).
pub(super) fn animated(pos: [i32; 3], nbt: &Compound, host: &dyn Host) -> bool {
    let shown = shown_of(host, nbt);
    CLIENT.with_borrow(|c| {
        shown.iter().any(|p| {
            let target = if p.powered || p.satisfied { 1.0 } else { 0.0 };
            c.bulbs.get(&(pos, p.slot)).is_some_and(|(v, prev, flash)| {
                (v - target).abs() > 0.001 || (prev - v).abs() > 0.0001 || *flash != p.flash
            })
        })
    }) || CLIENT.with_borrow(|c| c.connecting.is_some_and(|(pp, _)| pp.0 == pos))
}

/// The outline of the panel being joined (`Outliner.showAABB`, green blinking).
pub(super) fn render_level_stage(
    host: &dyn ClientHost,
    event: &mut mod_api::RenderLevelStageEvent<'_>,
) {
    if event.stage != mod_api::RenderLevelStage::AfterTranslucentBlocks {
        return;
    }
    let Some((pp, t)) = CLIENT.with_borrow(|c| c.connecting) else {
        return;
    };
    let Some((face, facing)) = orientation(host, pp.0) else {
        return;
    };
    let color = if t % 16 > 8 { 0xFF38_B764 } else { 0xFFA7_F070 };
    let mut lines = Vec::new();
    for (p, color) in [
        Some((pp, color)),
        CLIENT.with_borrow(|c| c.target).map(|t| (t, 0xFFEE_EEEE)),
    ]
    .into_iter()
    .flatten()
    {
        let (min, max) = panel_bb(face, facing, p);
        crate::glue::logistics::client::box_lines(&mut lines, min, max, color);
    }
    let _ = event.draw.lines(&lines);
}

/// `getBB`: the slot's centre, 3/16 out in the face's plane.
fn panel_bb(face: Face, facing: Dir, pp: PanelPos) -> ([f64; 3], [f64; 3]) {
    let b = fx::panel_box(face, facing, pp.1);
    let c = [0, 1, 2].map(|i| (b[i] + b[i + 3]) / 2.0);
    // `getBB`: the slot's centre, 3/16 out in the face's plane.
    let axis = fx::connected_direction(face, facing).axis();
    let mut min = [0.0; 3];
    let mut max = [0.0; 3];
    for (i, a) in [
        create_engine::kinetics::Axis::X,
        create_engine::kinetics::Axis::Y,
        create_engine::kinetics::Axis::Z,
    ]
    .into_iter()
    .enumerate()
    {
        let center = if a == axis {
            fx::slot_center(face, facing, pp.1)[i]
        } else {
            c[i]
        };
        let r = if a == axis { 0.0 } else { 3.0 / 16.0 };
        min[i] = f64::from(pp.0[i]) + center - r;
        max[i] = f64::from(pp.0[i]) + center + r;
    }
    (min, max)
}

// ---------------------------------------------------------------------------------------------
// Drawing

/// The standalone models a gauge is drawn with.
pub(super) const MODELS: [&str; 7] = [
    "create:block/factory_gauge/panel",
    "create:block/factory_gauge/panel_with_bulb",
    "create:block/factory_gauge/panel_restocker",
    "create:block/factory_gauge/panel_restocker_with_bulb",
    "create:block/factory_gauge/bulb_light",
    "create:block/factory_gauge/bulb_red",
    "create:block/factory_panel_sprites",
];

fn model(host: &dyn ClientHost, file: &'static str, fresh: bool) -> Option<mod_api::ModelRef> {
    if !fresh && let Some(m) = CLIENT.with_borrow(|c| c.models.get(file).copied()) {
        return Some(m);
    }
    let id = mod_api::Identifier::parse(file).ok()?;
    let m = host.render_assets().model(&id).ok()?;
    CLIENT.with_borrow_mut(|c| c.models.insert(file, m));
    Some(m)
}

fn draw_model(draw: &mut dyn BlockEntityDrawContext, host: &dyn ClientHost, file: &'static str) {
    draw_model_with(draw, host, file, TerrainModelOptions::default());
}

fn draw_model_with(
    draw: &mut dyn BlockEntityDrawContext,
    host: &dyn ClientHost,
    file: &'static str,
    options: TerrainModelOptions<'_>,
) {
    for fresh in [false, true] {
        let Some(m) = model(host, file, fresh) else {
            return;
        };
        match draw.terrain_model(m, options) {
            Ok(()) => return,
            Err(mod_api::TerrainModelError::Model(_)) if !fresh => continue,
            Err(_) => return,
        }
    }
}

/// The pose of a panel's own frame (`rotateCentered` by `yRot` about Y, `xRot + 90` about X,
/// 180 about Y), as the model and renderer turn it.
fn panel_frame(draw: &mut dyn BlockEntityDrawContext, face: Face, facing: Dir) {
    draw.rotate_about([0.0, 1.0, 0.0], fx::y_rot(face, facing) as f32, [0.5; 3]);
    draw.rotate_about([1.0, 0.0, 0.0], (fx::x_rot(face) + 90.0) as f32, [0.5; 3]);
    draw.rotate_about([0.0, 1.0, 0.0], 180.0, [0.5; 3]);
}

/// The tinted copy (`create-data prepare`'s) of a connection partial
/// (`AllPartialModels.FACTORY_PANEL_ARROWS`, `FACTORY_PANEL_LINES`) pointing `d`.
fn segment_model(d: Dir, arrow: bool) -> &'static str {
    match (d, arrow) {
        (Dir::North, true) => SEGMENTS[0],
        (Dir::South, true) => SEGMENTS[1],
        (Dir::West, true) => SEGMENTS[2],
        (_, true) => SEGMENTS[3],
        (Dir::North, false) => SEGMENTS[4],
        (Dir::South, false) => SEGMENTS[5],
        (Dir::West, false) => SEGMENTS[6],
        (_, false) => SEGMENTS[7],
    }
}

/// The connection partials, tinted (`prepare` makes them from the player's jar).
pub(super) const SEGMENTS: [&str; 8] = [
    "create:block/factory_gauge/connections/arrow_north_tinted",
    "create:block/factory_gauge/connections/arrow_south_tinted",
    "create:block/factory_gauge/connections/arrow_west_tinted",
    "create:block/factory_gauge/connections/arrow_east_tinted",
    "create:block/factory_gauge/connections/line_north_tinted",
    "create:block/factory_gauge/connections/line_south_tinted",
    "create:block/factory_gauge/connections/line_west_tinted",
    "create:block/factory_gauge/connections/line_east_tinted",
];

/// Whether a point of the world lies in another panel or a solid block (`level.noCollision`
/// of a tiny box, which a gauge's panels count in).
fn blocked(host: &dyn ClientHost, at: [f64; 3]) -> bool {
    let b = at.map(|v| v.floor() as i32);
    if let Some((face, facing)) = orientation(host, b) {
        let local = [0, 1, 2].map(|i| at[i] - f64::from(b[i]));
        return panels(host, b).iter().any(|p| {
            let x = fx::panel_box(face, facing, p.slot);
            (0..3).all(|i| local[i] > x[i] - 1.0 / 128.0 && local[i] < x[i + 3] + 1.0 / 128.0)
        });
    }
    let Some(level) = host.level(host.dimension()) else {
        return false;
    };
    let Some(state) = level.block_state(host, b) else {
        return false;
    };
    let local = [0, 1, 2].map(|i| at[i] - f64::from(b[i]));
    host.state_outline(state)
        .unwrap_or_default()
        .iter()
        .any(|x| (0..3).all(|i| local[i] > x[i] && local[i] < x[i + 3]))
}

/// `FactoryPanelRenderer.renderPath`: the arrow from panel `to` toward the panel feeding it.
#[allow(clippy::too_many_arguments)]
fn draw_path(
    draw: &mut dyn BlockEntityDrawContext,
    host: &dyn ClientHost,
    face: Face,
    facing: Dir,
    to: PanelPos,
    p: &Shown,
    input: &ShownInput,
    glow: f32,
) {
    let diff = fx::path_diff(face, facing, input.from, to);
    let start = fx::slot_center(face, facing, to.1);
    let origin = [0, 1, 2].map(|i| f64::from(to.0[i]) + start[i]);
    let path = fx::path(diff, input.bend, |t| {
        let o = fx::path_point(face, facing, t);
        blocked(host, [0, 1, 2].map(|i| origin[i] + o[i]))
    });
    let status = p.status();
    let (color, lift) = fx::path_look(status, glow, input.success);
    let moving = fx::path_moves(status);
    let (sx, sy) = fx::slot_offset(to.1);
    let (mut x, mut z) = (0.0f32, 0.0f32);
    for (i, d) in path.iter().enumerate() {
        let o = d.offset();
        x += o[0] as f32 * 0.5;
        z += o[2] as f32 * 0.5;
        let data2d = match d {
            Dir::South => 0.0,
            Dir::West => 1.0,
            Dir::North => 2.0,
            _ => 3.0,
        };
        let y = (lift + (data2d % 2.0) * 0.125) / 512.0;
        draw.push();
        panel_frame(draw, face, facing);
        draw.translate([sx as f32 * 0.5 + 0.25 + x, y, sy as f32 * 0.5 + 0.25 + z]);
        // `shiftUV(FACTORY_PANEL_CONNECTIONS)`: the animated sheet while it is asking.
        let mut options = TerrainModelOptions::default().with_tint(color);
        if moving {
            options = options.with_uv_shift(mod_api::TerrainUvShift::new(
                "create:block/factory_panel_connections",
                "create:block/factory_panel_connections_animated",
                [1.0, 1.0],
                [0.0, 0.0],
            ));
        }
        draw_model_with(draw, host, segment_model(d.opposite(), i == 0), options);
        draw.pop();
    }
}

/// `renderPath` for a display or redstone link joined to panel `to`. A display link
/// (`displayLinkMode`): green dots, from the panel toward the link (`pathReversed`, the link
/// being no output). A redstone link (`redstoneLinkMode`, half a step up): arrows and lines
/// toward the panel from a receiving link (red while it powers the panel, dark red else), from
/// the panel toward a transmitting one (red while the panel is satisfied, dark red while not,
/// grey with no amount set).
fn draw_link_path(
    draw: &mut dyn BlockEntityDrawContext,
    host: &dyn ClientHost,
    face: Face,
    facing: Dir,
    to: PanelPos,
    p: &Shown,
    link: &ShownInput,
) {
    let diff = fx::path_diff(face, facing, link.from, to);
    let start = fx::slot_center(face, facing, to.1);
    let origin = [0, 1, 2].map(|i| f64::from(to.0[i]) + start[i]);
    let path = fx::path(diff, link.bend, |t| {
        let o = fx::path_point(face, facing, t);
        blocked(host, [0, 1, 2].map(|i| origin[i] + o[i]))
    });
    // The link's block as this client sees it: a redstone link listens while a receiver.
    let redstone = host
        .level(host.dimension())
        .and_then(|l| l.block_state(host, link.from.0))
        .and_then(states::id)
        .and_then(|id| {
            crate::glue::client_table(host, |t| {
                t.get(id)
                    .filter(|(b, _)| *b == Block::RedstoneLink)
                    .map(|(_, i)| i.receiver)
            })
            .flatten()
        });
    let reversed = redstone != Some(true);
    let (color, lift) = match redstone {
        None => (0x3C9852, 0.0),
        Some(false) => (
            if p.count == 0 {
                0x888898
            } else if p.satisfied {
                0xEF0000
            } else {
                0x580101
            },
            0.5,
        ),
        Some(true) => (if p.powered { 0xEF0000 } else { 0x580101 }, 0.5),
    };
    let (sx, sy) = fx::slot_offset(to.1);
    let (mut x, mut z) = (0.0f32, 0.0f32);
    for (i, d) in path.iter().enumerate() {
        let o = d.offset();
        if !reversed {
            x += o[0] as f32 * 0.5;
            z += o[2] as f32 * 0.5;
        }
        let data2d = match d {
            Dir::South => 0.0,
            Dir::West => 1.0,
            Dir::North => 2.0,
            _ => 3.0,
        };
        let y = (lift + (data2d % 2.0) * 0.125) / 512.0;
        draw.push();
        panel_frame(draw, face, facing);
        draw.translate([sx as f32 * 0.5 + 0.25 + x, y, sy as f32 * 0.5 + 0.25 + z]);
        let options = TerrainModelOptions::default().with_tint(color);
        let model = if redstone.is_none() {
            dotted_model(*d)
        } else {
            let arrow = if reversed {
                i == path.len() - 1
            } else {
                i == 0
            };
            segment_model(if reversed { *d } else { d.opposite() }, arrow)
        };
        draw_model_with(draw, host, model, options);
        draw.pop();
        if reversed {
            x += o[0] as f32 * 0.5;
            z += o[2] as f32 * 0.5;
        }
    }
}

/// The tinted copy of `AllPartialModels.FACTORY_PANEL_DOTTED` pointing `d`.
fn dotted_model(d: Dir) -> &'static str {
    match d {
        Dir::North => DOTS[0],
        Dir::South => DOTS[1],
        Dir::West => DOTS[2],
        _ => DOTS[3],
    }
}

/// The dotted connection partials, tinted.
pub(super) const DOTS: [&str; 4] = [
    "create:block/factory_gauge/connections/dotted_north_tinted",
    "create:block/factory_gauge/connections/dotted_south_tinted",
    "create:block/factory_gauge/connections/dotted_west_tinted",
    "create:block/factory_gauge/connections/dotted_east_tinted",
];

/// The filter's item on its panel (`FilteringRenderer`, `ValueBoxRenderer.renderItemIntoValueBox`).
fn draw_item(
    draw: &mut dyn BlockEntityDrawContext,
    host: &dyn ClientHost,
    face: Face,
    facing: Dir,
    slot: u8,
    stack: &ItemStack,
) {
    let c = fx::slot_center(face, facing, slot);
    let id = super::body::item_id(host, stack);
    let solid = crate::glue::logistics::items::is_block_item(host, &id);
    let scale = 0.5 * (if solid { 1.0 } else { 0.5 } + 1.0 / 64.0);
    draw.push();
    draw.translate([c[0] as f32, c[1] as f32, c[2] as f32]);
    draw.rotate_about(
        [0.0, 1.0, 0.0],
        (fx::y_rot(face, facing) + 180.0) as f32,
        [0.0; 3],
    );
    draw.rotate_about([1.0, 0.0, 0.0], -fx::x_rot(face) as f32, [0.0; 3]);
    draw.scale([scale, scale, scale]);
    if !solid {
        draw.translate([0.0, 0.0, -0.15]);
    }
    draw.item(stack, ItemDisplayContext::Fixed, None, 0);
    draw.pop();
}

/// The gauge's drawing on the shared block entity type.
pub(super) struct GaugeDraw;

#[mod_api::horizon_part]
impl BlockPart for GaugeDraw {}

#[mod_api::horizon_part]
impl BlockEntityPart for GaugeDraw {
    fn block_entity_submit(&self, host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
        if event.block != "create:factory_gauge" {
            return;
        }
        let pos = event.pos;
        let Some((face, facing)) = orientation(host, pos) else {
            return;
        };
        let shown = shown_of(host, &event.entity.nbt);
        let draw = &mut *event.draw;
        let t = draw.partial();
        let light = draw.light();
        for p in &shown {
            let pp = (pos, p.slot);
            let (sx, sy) = fx::slot_offset(p.slot);
            // `FactoryPanelModel.addPanel`.
            let file = match (p.count == 0, p.restocker) {
                (true, false) => MODELS[0],
                (false, false) => MODELS[1],
                (true, true) => MODELS[2],
                (false, true) => MODELS[3],
            };
            draw.push();
            panel_frame(draw, face, facing);
            draw.translate([sx as f32 * 0.5, 0.0, sy as f32 * 0.5]);
            draw_model(draw, host, file);
            draw.pop();
            let g = glow(pp, p, t);
            // `renderBulb`.
            if p.count > 0 {
                let red = p.powered || p.status().missing_address;
                draw.push();
                panel_frame(draw, face, facing);
                draw.translate([sx as f32 * 0.5, 0.0, sy as f32 * 0.5]);
                draw.set_light(if g > 0.125 { 0xF000F0 } else { light });
                draw_model(draw, host, if red { MODELS[5] } else { MODELS[4] });
                draw.set_light(light);
                draw.pop();
            }
            for input in &p.inputs {
                draw_path(draw, host, face, facing, pp, p, input, g);
            }
            for link in &p.links {
                draw_link_path(draw, host, face, facing, pp, p, link);
            }
            if let Some(f) = &p.filter {
                draw_item(draw, host, face, facing, p.slot, f);
            }
        }
        if !animated(pos, &event.entity.nbt, host) {
            draw.keep();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The value box under the crosshair

/// The panel under the crosshair: its gauge, slot and what it shows.
fn hovered(host: &dyn ClientHost) -> Option<(PanelPos, Shown)> {
    let Some(mod_api::CrosshairTarget::Block { pos, hit, .. }) = host.crosshair_target() else {
        return None;
    };
    let pos = [pos.x, pos.y, pos.z];
    let player = host.local_player()?;
    if !crate::glue::interaction::values::may_use_value_box(host, player) {
        return None;
    }
    let (slot, _) = self::hit(host, pos, hit.point)?;
    Some(((pos, slot), panel(host, (pos, slot))?))
}

/// `FactoryPanelBehaviour.getLabel`, `getCountLabelForValueBox`, `getTip` and `getAmountTip`
/// of the panel under the crosshair, as the value box shows them: each line and its colour.
pub(crate) fn hover_lines(host: &dyn ClientHost) -> Option<Vec<(String, u32)>> {
    let (_, p) = hovered(host)?;
    let t = |k: &str, f: &str| host.translate(k).unwrap_or_else(|| f.to_owned());
    let status = p.status();
    let name = p
        .filter
        .as_ref()
        .map(|f| host.hover_name(f))
        .unwrap_or_default();
    let gold = 0xFBDC7D;
    let (label, color) = if !p.inputs.is_empty() && p.count == 0 {
        (
            t(
                "create.gui.factory_panel.no_target_amount_set",
                "Inactive: no target amount set",
            ),
            0xFF5555,
        )
    } else if status.missing_address {
        (
            t(
                "create.gui.factory_panel.address_missing",
                "Inactive: missing a target address",
            ),
            0xFF5555,
        )
    } else if p.filter.is_none() {
        (
            t("create.factory_panel.new_factory_task", "New factory task"),
            gold,
        )
    } else if p.waiting {
        (
            t(
                "create.factory_panel.some_links_unloaded",
                "Some links are not loaded",
            ),
            gold,
        )
    } else if p.count == 0 || p.inputs.is_empty() {
        (name.clone(), gold)
    } else if p.powered {
        let s = t("create.factory_panel.redstone_paused", "(Redstone Paused)");
        (format!("{name} {s}"), gold)
    } else if !p.satisfied {
        let s = t("create.factory_panel.in_progress", "(In Progress)");
        (format!("{name} {s}"), gold)
    } else {
        (name.clone(), gold)
    };
    let mut lines = vec![(label, color)];
    if let Some(f) = &p.filter {
        // The stock against the target, in items or stacks.
        let stacks = if p.up_to { "" } else { "\u{25a4}" };
        let per = if p.up_to {
            1
        } else {
            super::body::stack_limit(host, f).max(1) as i32
        };
        let have = if p.waiting {
            "?".to_owned()
        } else if p.level >= create_engine::stock::INF {
            "\u{221e}".to_owned()
        } else {
            format!("{}{stacks}", p.level / per)
        };
        if p.count == 0 {
            lines.push((have, 0xF1EFE8));
        } else {
            let c = if p.satisfied {
                0xD7FFA8
            } else if p.promised_satisfied {
                0xFFCD75
            } else {
                0xFFBFA8
            };
            let up = if p.promised == 0 { "" } else { "\u{23f6}" };
            lines.push((format!("{have}{up} / {}{stacks}", p.count), c));
        }
        lines.push((
            t(
                "create.factory_panel.click_to_configure",
                "Click to configure",
            ),
            0xFFFFFF,
        ));
        lines.push((
            t(
                "create.factory_panel.hold_to_set_amount",
                "Click and hold for target amount",
            ),
            0xFFFFFF,
        ));
    } else {
        lines.push((
            t("create.logistics.filter.click_to_set", "Click to set"),
            0xFFFFFF,
        ));
    }
    Some(lines)
}

/// The outline of the panel's value box under the crosshair; whether there is one.
pub(crate) fn hover_outline(host: &dyn ClientHost, lines: &mut Vec<mod_api::WorldLine>) -> bool {
    let Some((pp, _)) = hovered(host) else {
        return false;
    };
    let Some((face, facing)) = orientation(host, pp.0) else {
        return false;
    };
    let c = fx::slot_center(face, facing, pp.1);
    let axis = match fx::connected_direction(face, facing).axis() {
        create_engine::kinetics::Axis::X => 0,
        create_engine::kinetics::Axis::Y => 1,
        create_engine::kinetics::Axis::Z => 2,
    };
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let centre = [0, 1, 2].map(|i| f64::from(pp.0[i]) + c[i]);
    let corner = |a: f64, b: f64| {
        let mut p = centre;
        p[u] += a * 0.1875;
        p[v] += b * 0.1875;
        p
    };
    let square = [
        corner(-1.0, -1.0),
        corner(1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, 1.0),
    ];
    for i in 0..4 {
        lines.push(mod_api::WorldLine::new(
            square[i],
            square[(i + 1) % 4],
            0xFFFF_FFFF,
            2.0,
            mod_api::WorldDepth::Tested,
        ));
    }
    true
}
