//! The station's screens (`StationScreen`, `AssemblyScreen`): the server opens them with what
//! the station shows (its name, assembly mode, bogeys, last failure, and the train at or bound
//! for it) and keeps a player's open screen up to date, as Create's screens read the block
//! entity each tick; the client sends what is done on them (`StationEditPacket`,
//! `TrainEditPacket`). Closing and sending are done right after the key or click that asked for
//! them (`Subsystem::input_handled`: the screen hooks get a read-only client).

use super::super::net;
use super::super::server::ServerLevel;
use super::super::world::with_world;
use super::assembly;
use super::gui::{
    Reader, click_sound, icon_button, inside, millis, put_f32, put_i32, put_pos, put_str, tooltip,
    tr, wide_button, word_wrap,
};
use mod_api::{
    ClientHost, DrawContext, Flow, PlayerRef, ScreenCharTypedPreEvent, ScreenClosingEvent,
    ScreenKeyPressedPreEvent, ScreenMouseButtonPressedPreEvent, ScreenRenderPostEvent, ServerHost,
};
use std::cell::RefCell;

/// Server to client: the station's view; client to server: what was done on the screen.
pub const CHANNEL: &str = "create:station_screen";
/// The mod whose own screen it is.
const MOD: &str = "create";

/// `StationEditPacket`'s name length (`nameBox.setMaxLength(25)`), a train's (35).
const NAME_MAX: usize = 25;
const TRAIN_NAME_MAX: usize = 35;

/// The train at or bound for the station, as its screen shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrainView {
    pub id: u32,
    pub name: String,
    pub icon: u8,
    /// Standing at the station (`trainPresent`), and fit to be taken apart there.
    pub present: bool,
    pub can_disassemble: bool,
    pub has_schedule: bool,
    /// Each carriage's bogey spacing (`Carriage.bogeySpacing`), and whether its controls face
    /// both ways (`doubleEnded`).
    pub spacings: Vec<i32>,
    pub double_ended: bool,
    /// Blocks left to the station (`distanceToDestination`).
    pub distance: f32,
}

/// What the station's screens show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub pos: [i32; 3],
    pub name: String,
    pub assembling: bool,
    /// Its track is straight (`edgePoint.isOrthogonal`): a train can be made there.
    pub orthogonal: bool,
    pub bogeys: i32,
    /// The last failure: its lang key, arguments and carriage (-1 none).
    pub failure: Option<(String, Vec<String>, i32)>,
    pub train: Option<TrainView>,
    /// `DoorControlBehaviour.mode` (`doors::MODES`).
    pub doors: u8,
}

impl View {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_pos(&mut out, self.pos);
        put_str(&mut out, &self.name);
        out.push(u8::from(self.assembling));
        out.push(u8::from(self.orthogonal));
        put_i32(&mut out, self.bogeys);
        match &self.failure {
            None => out.push(0),
            Some((key, args, carriage)) => {
                out.push(1);
                put_str(&mut out, key);
                out.push(args.len() as u8);
                for a in args {
                    put_str(&mut out, a);
                }
                put_i32(&mut out, *carriage);
            }
        }
        match &self.train {
            None => out.push(0),
            Some(t) => {
                out.push(1);
                put_i32(&mut out, t.id as i32);
                put_str(&mut out, &t.name);
                out.push(t.icon);
                out.push(u8::from(t.present));
                out.push(u8::from(t.can_disassemble));
                out.push(u8::from(t.has_schedule));
                out.push(u8::from(t.double_ended));
                put_i32(&mut out, t.spacings.len() as i32);
                for s in &t.spacings {
                    put_i32(&mut out, *s);
                }
                put_f32(&mut out, t.distance);
            }
        }
        out.push(self.doors);
        out
    }

    pub fn decode(data: &[u8]) -> Option<View> {
        let mut r = Reader(data);
        let pos = r.pos()?;
        let name = r.str()?;
        let assembling = r.bool()?;
        let orthogonal = r.bool()?;
        let bogeys = r.i32()?;
        let failure = match r.u8()? {
            0 => None,
            _ => {
                let key = r.str()?;
                let n = r.u8()?;
                let args = (0..n).map(|_| r.str()).collect::<Option<Vec<_>>>()?;
                Some((key, args, r.i32()?))
            }
        };
        let train = match r.u8()? {
            0 => None,
            _ => {
                let id = r.i32()? as u32;
                let name = r.str()?;
                let icon = r.u8()?;
                let present = r.bool()?;
                let can_disassemble = r.bool()?;
                let has_schedule = r.bool()?;
                let double_ended = r.bool()?;
                let n = r.i32()?.clamp(0, 256);
                let spacings = (0..n).map(|_| r.i32()).collect::<Option<Vec<_>>>()?;
                Some(TrainView {
                    id,
                    name,
                    icon,
                    present,
                    can_disassemble,
                    has_schedule,
                    spacings,
                    double_ended,
                    distance: r.f32()?,
                })
            }
        };
        let doors = r.u8()?;
        Some(View {
            pos,
            name,
            assembling,
            orthogonal,
            bogeys,
            failure,
            train,
            doors,
        })
    }
}

/// What a client does on the screen.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// `StationEditPacket.configure`: the name, and assembly mode on or off.
    Configure {
        pos: [i32; 3],
        assembly: bool,
        name: String,
    },
    /// `StationEditPacket.tryAssemble`.
    Assemble { pos: [i32; 3] },
    /// `TrainEditPacket`: a train's name (blank keeps it) and icon.
    Train {
        pos: [i32; 3],
        id: u32,
        name: String,
        icon: u8,
    },
    /// The screen closed.
    Closed { pos: [i32; 3] },
    /// `StationEditPacket.dropSchedule`: the present train's schedule comes back.
    DropSchedule { pos: [i32; 3] },
    /// `StationEditPacket`'s door control (`doors::MODES`).
    Doors { pos: [i32; 3], mode: u8 },
}

impl Edit {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Edit::Configure {
                pos,
                assembly,
                name,
            } => {
                out.push(0);
                put_pos(&mut out, *pos);
                out.push(u8::from(*assembly));
                put_str(&mut out, name);
            }
            Edit::Assemble { pos } => {
                out.push(1);
                put_pos(&mut out, *pos);
            }
            Edit::Train {
                pos,
                id,
                name,
                icon,
            } => {
                out.push(2);
                put_pos(&mut out, *pos);
                put_i32(&mut out, *id as i32);
                put_str(&mut out, name);
                out.push(*icon);
            }
            Edit::Closed { pos } => {
                out.push(3);
                put_pos(&mut out, *pos);
            }
            Edit::DropSchedule { pos } => {
                out.push(4);
                put_pos(&mut out, *pos);
            }
            Edit::Doors { pos, mode } => {
                out.push(5);
                put_pos(&mut out, *pos);
                out.push(*mode);
            }
        }
        out
    }

    pub fn decode(data: &[u8]) -> Option<Edit> {
        let mut r = Reader(data);
        Some(match r.u8()? {
            0 => Edit::Configure {
                pos: r.pos()?,
                assembly: r.bool()?,
                name: r.str()?,
            },
            1 => Edit::Assemble { pos: r.pos()? },
            2 => Edit::Train {
                pos: r.pos()?,
                id: r.i32()? as u32,
                name: r.str()?,
                icon: r.u8()?,
            },
            3 => Edit::Closed { pos: r.pos()? },
            4 => Edit::DropSchedule { pos: r.pos()? },
            5 => Edit::Doors {
                pos: r.pos()?,
                mode: r.u8()?,
            },
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Server

/// A player with a station's screen open: who, the level, the station, the view last sent.
type Watcher = (u128, usize, [i32; 3], Vec<u8>);

thread_local! {
    /// The players with a station's screen open: who, the level, the station, and the view
    /// they were last sent.
    static WATCHING: RefCell<Vec<Watcher>> = const { RefCell::new(Vec::new()) };
}

pub fn forget() {
    WATCHING.with_borrow_mut(Vec::clear);
}

/// The station's view now.
fn view(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<View> {
    let dim = lv.dim;
    let name = with_world(|w| w.dims[dim].railway.stations.get(&pos).cloned()).flatten()?;
    let assembling = assembly::assembling(dim, pos);
    let bogeys = if assembling {
        assembly::assembly_track(lv, pos).map_or(0, |(_, b)| b.len() as i32)
    } else {
        0
    };
    let can_disassemble = assembly::can_disassemble(dim, pos);
    let train = with_world(|w| {
        let r = &w.dims[dim].railway;
        // `getImminentTrain`: the train standing here, else the nearest bound here.
        let t = r
            .trains
            .iter()
            .find(|t| t.train.bound && t.train.at_station == Some(pos))
            .or_else(|| {
                r.trains
                    .iter()
                    .filter(|t| t.train.nav.as_ref().is_some_and(|n| n.station == pos))
                    .min_by(|a, b| {
                        let d = |t: &&create_engine::trains::TrainOnTrack| {
                            t.train.nav.as_ref().map_or(f64::MAX, |n| n.distance)
                        };
                        d(a).total_cmp(&d(b))
                    })
            })?;
        let present = t.train.at_station == Some(pos);
        Some(TrainView {
            id: t.id,
            name: t.name.clone(),
            icon: t.icon,
            present,
            can_disassemble: present && can_disassemble,
            has_schedule: present && t.schedule.is_some(),
            spacings: t.train.carriages.iter().map(|c| c.bogey_spacing).collect(),
            double_ended: t.train.double_ended,
            distance: t.train.nav.as_ref().map_or(0.0, |n| n.distance as f32),
        })
    })
    .flatten();
    Some(View {
        pos,
        name,
        assembling,
        orthogonal: assembly::has_track(dim, pos),
        bogeys,
        failure: assembly::failure(dim, pos),
        train,
        doors: super::doors::mode(lv, pos),
    })
}

/// `StationBlock.displayScreen`: the station's screen opens on `player`'s client.
pub fn open(lv: &mut ServerLevel<'_>, player: PlayerRef, pos: [i32; 3]) {
    super::railway::railway_ready(lv.dim);
    let Some(v) = view(lv, pos) else {
        return;
    };
    let data = v.encode();
    // The first byte: 1 opens the screen, 0 only updates an open one.
    let mut opening = vec![1];
    opening.extend_from_slice(&data);
    net::send_to_player(&mut *lv.host, player, CHANNEL, &opening);
    let (uuid, dim) = (player.uuid(), lv.dim);
    WATCHING.with_borrow_mut(|w| {
        w.retain(|(u, ..)| *u != uuid);
        w.push((uuid, dim, pos, data));
    });
}

/// Each few ticks, an open screen hears of what changed at its station (the screens read the
/// block entity each frame in Create).
pub fn tick(lv: &mut ServerLevel<'_>) {
    if WATCHING.with_borrow(Vec::is_empty) || lv.game_time() % 4 != 0 {
        return;
    }
    let dim = lv.dim;
    let watching: Vec<(u128, [i32; 3])> = WATCHING.with_borrow(|w| {
        w.iter()
            .filter(|(_, d, ..)| *d == dim)
            .map(|(u, _, p, _)| (*u, *p))
            .collect()
    });
    for (uuid, pos) in watching {
        let player = lv.host.player(uuid);
        let v = view(lv, pos);
        let (Some(player), Some(v)) = (player, v) else {
            WATCHING.with_borrow_mut(|w| w.retain(|(u, ..)| *u != uuid));
            continue;
        };
        let data = v.encode();
        let changed = WATCHING.with_borrow_mut(|w| {
            w.iter_mut().find(|(u, ..)| *u == uuid).is_some_and(|e| {
                let changed = e.3 != data;
                e.3.clone_from(&data);
                changed
            })
        });
        if changed {
            send_update(lv.host, player, &data);
        }
    }
}

/// `StationEditPacket.applySettings` and `TrainEditPacket.handleSided`, for a player near the
/// station (`BlockEntityConfigurationPacket`'s 20 blocks).
pub fn client_message(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let Some(edit) = Edit::decode(data) else {
        return;
    };
    let pos = match &edit {
        Edit::Configure { pos, .. }
        | Edit::Assemble { pos }
        | Edit::Train { pos, .. }
        | Edit::Closed { pos }
        | Edit::DropSchedule { pos }
        | Edit::Doors { pos, .. } => *pos,
    };
    if let Edit::Closed { .. } = edit {
        let uuid = player.uuid();
        WATCHING.with_borrow_mut(|w| w.retain(|(u, ..)| *u != uuid));
        return;
    }
    let Some(dimension) = player.level(&*host).and_then(|l| l.dimension().id(&*host)) else {
        return;
    };
    let near = player.entity().position(&*host).is_some_and(|p| {
        let d = [0, 1, 2].map(|i| p[i] - (f64::from(pos[i]) + 0.5));
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < 20.0 * 20.0
    });
    if !near {
        return;
    }
    let Some(mut lv) = ServerLevel::new(host, &dimension) else {
        return;
    };
    let dim = lv.dim;
    if with_world(|w| w.dims[dim].railway.stations.contains_key(&pos)) != Some(true) {
        return;
    }
    match edit {
        Edit::Configure { assembly, name, .. } => {
            let name: String = name.chars().take(NAME_MAX).collect();
            if !name.trim().is_empty() {
                rename(&lv, pos, name);
            }
            if assembly {
                assembly::enter_assembly(&mut lv, pos, Some(player));
            } else {
                assembly::exit_assembly(&mut lv, pos);
            }
        }
        Edit::Assemble { .. } => {
            if assembly::assembling(dim, pos) && assembly::assemble(&mut lv, pos) {
                assembly::exit_assembly(&mut lv, pos);
            }
        }
        Edit::Train { id, name, icon, .. } => {
            let name: String = name.chars().take(TRAIN_NAME_MAX).collect();
            let column = with_world(|w| {
                let t = w.dims[dim].railway.trains.iter_mut().find(|t| t.id == id)?;
                if !name.trim().is_empty() {
                    t.name = name;
                }
                t.icon = icon.min(2);
                Some(t.column)
            })
            .flatten();
            if let Some(c) = column {
                super::super::persist::mark_unsaved(dim, [c.0, c.1]);
            }
        }
        Edit::DropSchedule { .. } => {
            if let Some(train) = assembly::present_train(dim, pos) {
                super::conductor::give_back_schedule(lv.host, player, dim, train);
            }
        }
        Edit::Doors { mode, .. } => super::doors::set_mode(&mut lv, pos, mode),
        Edit::Closed { .. } => {}
    }
    // The screen hears at once.
    if let Some(v) = view(&lv, pos) {
        let data = v.encode();
        let uuid = player.uuid();
        WATCHING.with_borrow_mut(|w| {
            if let Some(e) = w.iter_mut().find(|(u, ..)| *u == uuid) {
                e.3.clone_from(&data);
            }
        });
        send_update(lv.host, player, &data);
    }
}

/// A view for a screen already open (first byte 0).
fn send_update(host: &mut dyn ServerHost, player: PlayerRef, view: &[u8]) {
    let mut out = vec![0];
    out.extend_from_slice(view);
    net::send_to_player(host, player, CHANNEL, &out);
}

/// `updateName`: the station's name, saved with its column.
fn rename(lv: &ServerLevel<'_>, pos: [i32; 3], name: String) {
    let dim = lv.dim;
    let changed = with_world(|w| {
        let r = &mut w.dims[dim].railway;
        match r.stations.get_mut(&pos) {
            Some(n) if *n != name => {
                *n = name;
                true
            }
            _ => false,
        }
    });
    if changed == Some(true) {
        super::super::persist::mark_pos_unsaved(dim, pos);
    }
}

// ---------------------------------------------------------------------------------------------
// Client

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    None,
    Name,
    TrainName,
}

struct Open {
    view: View,
    /// The name boxes' text (`nameBox`, `trainNameBox`) and which has the focus.
    name: String,
    train_name: String,
    focus: Focus,
    /// The focused box's text is all selected (`setHighlightPos(0)` as it takes the focus):
    /// typing replaces it.
    selected: bool,
    /// The train last shown (`displayedTrain`), for its name box.
    shown_train: Option<u32>,
    size: [f32; 2],
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    /// Edits to send and whether to close, at the next client tick.
    static LATER: RefCell<(Vec<Edit>, bool)> = const { RefCell::new((Vec::new(), false)) };
}

pub fn reset() {
    OPEN.with_borrow_mut(|o| *o = None);
    LATER.with_borrow_mut(|l| *l = (Vec::new(), false));
}

fn train_label(t: &TrainView) -> String {
    t.name.clone()
}

/// The server sent the station's view: the screen opens, or shows it.
pub fn server_message(host: &mut dyn ClientHost, data: &[u8]) {
    let Some((&opening, data)) = data.split_first() else {
        return;
    };
    let Some(view) = View::decode(data) else {
        return;
    };
    let fresh = OPEN.with_borrow_mut(|o| match o {
        Some(o) if o.view.pos == view.pos && host.mod_screen() == Some(MOD) => {
            if o.focus != Focus::Name {
                o.name.clone_from(&view.name);
            }
            let train = view.train.as_ref().map(|t| t.id);
            if train != o.shown_train || o.focus != Focus::TrainName {
                o.train_name = view.train.as_ref().map(train_label).unwrap_or_default();
                if train != o.shown_train {
                    o.focus = if o.focus == Focus::TrainName {
                        Focus::None
                    } else {
                        o.focus
                    };
                }
                o.shown_train = train;
            }
            o.view = view.clone();
            false
        }
        _ => true,
    });
    // An update for a screen closed since is dropped.
    if !fresh || opening == 0 {
        return;
    }
    OPEN.with_borrow_mut(|o| {
        *o = Some(Open {
            name: view.name.clone(),
            train_name: view.train.as_ref().map(train_label).unwrap_or_default(),
            shown_train: view.train.as_ref().map(|t| t.id),
            view,
            focus: Focus::None,
            selected: false,
            size: [0.0, 0.0],
        })
    });
    if !host.mod_screen_open(MOD) {
        OPEN.with_borrow_mut(|o| *o = None);
    }
}

pub fn tick_client(host: &mut dyn ClientHost) {
    let (send, close) = LATER.with_borrow_mut(std::mem::take);
    if close {
        host.mod_screen_close(MOD);
    }
    for e in send {
        net::send_to_server(host, CHANNEL, &e.encode());
    }
    if host.mod_screen() != Some(MOD)
        && let Some(open) = OPEN.with_borrow_mut(Option::take)
    {
        for e in closing_edits(&open) {
            net::send_to_server(host, CHANNEL, &e.encode());
        }
    }
}

fn ours(host: &dyn ClientHost) -> bool {
    host.mod_screen() == Some(MOD) && OPEN.with_borrow(Option::is_some)
}

/// `AllGuiTextures.STATION` / `STATION_ASSEMBLING`: the window's sheet and part.
fn background(assembling: bool) -> (&'static str, [f32; 4]) {
    if assembling {
        ("create:gui/assemble", [0.0, 0.0, 200.0, 178.0])
    } else {
        ("create:gui/schedule_2", [0.0, 111.0, 200.0, 127.0])
    }
}

fn corner(size: [f32; 2], assembling: bool) -> (f32, f32) {
    let (_, bg) = background(assembling);
    (
        ((size[0] - bg[2]) / 2.0).floor(),
        ((size[1] - bg[3]) / 2.0).floor(),
    )
}

/// `AllIcons`: `[x, y]` in `icons.png`.
const I_CONFIRM: [f32; 2] = [0.0, 16.0];
const I_PRIORITY_VERY_LOW: [f32; 2] = [64.0, 0.0];
const I_DISABLE: [f32; 2] = [128.0, 160.0];
const I_VIEW_SCHEDULE: [f32; 2] = [192.0, 48.0];
/// `AllGuiTextures` in `schedule_2.png`.
const I_NEW_TRAIN: [f32; 4] = [14.0, 239.0, 24.0, 16.0];
const I_DISASSEMBLE_TRAIN: [f32; 4] = [39.0, 239.0, 24.0, 16.0];
const I_ASSEMBLE_TRAIN: [f32; 4] = [64.0, 239.0, 24.0, 16.0];
const STATION_EDIT_NAME: [f32; 4] = [0.0, 239.0, 13.0, 13.0];
const STATION_EDIT_TRAIN_NAME: [f32; 4] = [89.0, 239.0, 13.0, 13.0];

/// The buttons, as `[x, y, w, h]` from the window's corner.
fn confirm(assembling: bool) -> [f32; 4] {
    let (_, bg) = background(assembling);
    [bg[2] - 33.0, bg[3] - 24.0, 18.0, 18.0]
}
const NEW_TRAIN: [f32; 4] = [84.0, 65.0, 26.0, 18.0];
const DISASSEMBLE: [f32; 4] = [94.0, 65.0, 26.0, 18.0];
const DROP_SCHEDULE: [f32; 4] = [73.0, 65.0, 18.0, 18.0];
const ASSEMBLE: [f32; 4] = [94.0, 154.0, 26.0, 18.0];
const QUIT_ASSEMBLY: [f32; 4] = [73.0, 154.0, 18.0, 18.0];

fn at(r: [f32; 4], x: f32, y: f32) -> [f32; 4] {
    [x + r[0], y + r[1], r[2], r[3]]
}

/// `TrainIconType.getIconWidth` / `getIconOffset`: -1 is the engine.
fn icon_width(length: i32) -> f32 {
    match length {
        -1 | -2 => 19.0,
        l if l < 3 => 7.0,
        l if l < 9 => 13.0,
        _ => 19.0,
    }
}

fn icon_offset(length: i32) -> f32 {
    match length {
        -2 => 0.0,
        -1 => 62.0,
        l if l < 3 => 34.0,
        l if l < 9 => 20.0,
        _ => 42.0,
    }
}

/// `TrainIconType.render`: an icon of the train's sheet row; its width.
fn draw_icon(draw: &mut dyn DrawContext, icon: u8, length: i32, x: f32, y: f32, alpha: f32) -> f32 {
    let row = 205.0 + 11.0 * f32::from(icon.min(2));
    let w = icon_width(length);
    let a = (alpha.clamp(0.0, 1.0) * 255.0) as u32;
    draw.sprite(
        "create:gui/assemble",
        [2.0 + icon_offset(length), row, w, 10.0],
        [x, y, w, 10.0],
        Some((a << 24) | 0x00FF_FFFF),
    );
    w
}

/// `getTrainIconWidth`.
fn train_icon_width(t: &TrainView) -> f32 {
    let mut w = icon_width(-1);
    let n = t.spacings.len();
    for i in 1..n {
        if i == n - 1 && t.double_ended {
            w += icon_width(-2) + 1.0;
            break;
        }
        w += icon_width(t.spacings[i]) + 1.0;
    }
    w
}

/// `nameBoxX`: the name box's left, its text centred in the window.
fn name_box_x(draw: &dyn DrawContext, text: &str, box_width: f32, left: f32) -> f32 {
    left + 100.0 - ((draw.text_width(text).min(box_width) + 10.0) / 2.0).floor()
}

pub fn screen_render_post(host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
    if !ours(host) {
        return;
    }
    let size = event.draw.size();
    let Some((view, name, train_name, (focus, selected))) = OPEN.with_borrow_mut(|o| {
        o.as_mut().map(|o| {
            o.size = size;
            (
                o.view.clone(),
                o.name.clone(),
                o.train_name.clone(),
                (o.focus, o.selected),
            )
        })
    }) else {
        return;
    };
    let mouse = event.screen.mouse;
    let draw = &mut *event.draw;
    let (x, y) = corner(size, view.assembling);
    let (sheet, bg) = background(view.assembling);
    draw.sprite(sheet, bg, [x, y, bg[2], bg[3]], None);
    let blink = (millis() / 300).is_multiple_of(2);
    let mut tips: Vec<String> = Vec::new();
    // The confirm button (`AbstractStationScreen`; the assembly screen's closes it).
    let c = at(confirm(view.assembling), x, y);
    let hovered = inside(c, mouse);
    let icon = if view.assembling {
        I_PRIORITY_VERY_LOW
    } else {
        I_CONFIRM
    };
    icon_button(draw, [c[0], c[1]], icon, true, hovered);
    if hovered && view.assembling {
        tips.push(tr(host, "create.station.close", &[]));
    }
    if view.assembling {
        draw_assembly(host, draw, &view, x, y, mouse, &mut tips);
    } else {
        draw_station(
            host,
            draw,
            &view,
            (&name, &train_name, focus, blink && !selected, selected),
            x,
            y,
            mouse,
            &mut tips,
        );
    }
    if !tips.is_empty() {
        tooltip(draw, &tips, mouse);
    }
}

/// `AssemblyScreen.renderWindow` and its buttons.
fn draw_assembly(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    view: &View,
    x: f32,
    y: f32,
    mouse: [f32; 2],
    tips: &mut Vec<String>,
) {
    let header = tr(host, "create.station.assembly_title", &[]);
    let w = draw.text_width(&header);
    draw.text(
        &header,
        [x + 100.0 - (w / 2.0).floor(), y + 4.0],
        0xFF0E_2233,
        false,
    );
    if let Some((key, args, carriage)) = &view.failure {
        let text = tr(host, "create.station.failed", &[]);
        let w = draw.text_width(&text);
        draw.text(
            &text,
            [x + 97.0 - (w / 2.0).floor(), y + 47.0],
            0xFF77_5B5B,
            false,
        );
        let mut offset = 0.0;
        if *carriage != -1 {
            let line = tr(
                host,
                "create.station.carriage_number",
                &[carriage.to_string()],
            );
            draw.text(&line, [x + 30.0, y + 67.0], 0xFF7A_7A7A, false);
            offset += 10.0;
        }
        let message = tr(host, key, args);
        let n = word_wrap(
            draw,
            &message,
            [x + 30.0, y + 67.0 + offset],
            134.0,
            0xFF77_5B5B,
        );
        offset += n as f32 * 9.0 + 5.0;
        let retry = tr(host, "create.station.retry", &[]);
        word_wrap(
            draw,
            &retry,
            [x + 30.0, y + 67.0 + offset],
            134.0,
            0xFF7A_7A7A,
        );
    } else {
        let key = match view.bogeys {
            0 => "create.station.no_bogeys",
            1 => "create.station.one_bogey",
            _ => "create.station.more_bogeys",
        };
        let text = tr(host, key, &[view.bogeys.to_string()]);
        let w = draw.text_width(&text);
        draw.text(
            &text,
            [x + 97.0 - (w / 2.0).floor(), y + 47.0],
            0xFF7A_7A7A,
            false,
        );
        let how = |k: &str| tr(host, k, &[]);
        word_wrap(
            draw,
            &how("create.station.how_to"),
            [x + 28.0, y + 62.0],
            134.0,
            0xFF7A_7A7A,
        );
        word_wrap(
            draw,
            &how("create.station.how_to_1"),
            [x + 28.0, y + 94.0],
            134.0,
            0xFF7A_7A7A,
        );
        word_wrap(
            draw,
            &how("create.station.how_to_2"),
            [x + 28.0, y + 117.0],
            138.0,
            0xFF7A_7A7A,
        );
    }
    let a = at(ASSEMBLE, x, y);
    let active = view.bogeys > 0;
    let hovered = inside(a, mouse);
    wide_button(
        draw,
        [a[0], a[1]],
        "create:gui/schedule_2",
        I_ASSEMBLE_TRAIN,
        active,
        hovered,
    );
    if hovered {
        tips.push(tr(host, "create.station.assemble_train", &[]));
    }
    let q = at(QUIT_ASSEMBLY, x, y);
    let hovered = inside(q, mouse);
    icon_button(draw, [q[0], q[1]], I_DISABLE, true, hovered);
    if hovered {
        tips.push(tr(host, "create.station.cancel", &[]));
    }
}

/// `StationScreen.renderWindow` and its buttons.
#[allow(clippy::too_many_arguments)]
fn draw_station(
    host: &dyn ClientHost,
    draw: &mut dyn DrawContext,
    view: &View,
    (name, train_name, focus, blink, selected): (&str, &str, Focus, bool, bool),
    x: f32,
    y: f32,
    mouse: [f32; 2],
    tips: &mut Vec<String>,
) {
    // The name box (no border, 0x592424) and its edit mark.
    let nx = name_box_x(draw, name, 180.0, x);
    let mut shown = name.to_owned();
    if focus == Focus::Name && blink {
        shown.push('_');
    }
    if focus == Focus::Name && selected {
        // `EditBox.renderHighlight`.
        let w = draw.text_width(name);
        draw.fill([nx + 4.0, y + 3.0, w, 10.0], 0xFF00_00FF);
    }
    draw.text(&shown, [nx + 4.0, y + 4.0], 0xFF59_2424, false);
    if focus != Focus::Name {
        let ex = nx + draw.text_width(name) + 5.0;
        draw.sprite(
            "create:gui/schedule_2",
            STATION_EDIT_NAME,
            [ex, y + 1.0, 13.0, 13.0],
            None,
        );
    }
    // The train door and the doors' mode (`DoorControl.createWidget`'s label: all open).
    if let Some(door) = mod_api::Identifier::parse("create:train_door")
        .ok()
        .and_then(|i| host.item_key(&i))
    {
        draw.item(door, [x + 14.0, y + 103.0], 16.0);
    }
    let mode = super::doors::MODES[usize::from(view.doors).min(5)];
    let doors = tr(
        host,
        &format!("create.contraption.door_control.{mode}.short"),
        &[],
    );
    draw.text(&doors, [x + 39.0, y + 108.0], 0xFFFF_FFFF, true);
    // `SelectionScrollInput`'s tooltip: its title and options, the chosen one marked.
    if inside([x + 35.0, y + 102.0, 53.0, 16.0], mouse) {
        tips.push(tr(host, "create.contraption.door_control", &[]));
        for (i, m) in super::doors::MODES.iter().enumerate() {
            let mark = if i == usize::from(view.doors) {
                "-> "
            } else {
                "> "
            };
            tips.push(format!(
                "{mark}{}",
                tr(host, &format!("create.contraption.door_control.{m}"), &[])
            ));
        }
        tips.push(tr(host, "create.gui.scrollInput.scrollToSelect", &[]));
    }
    let Some(t) = &view.train else {
        let header = tr(host, "create.station.idle", &[]);
        let w = draw.text_width(&header);
        draw.text(
            &header,
            [x + 97.0 - (w / 2.0).floor(), y + 47.0],
            0xFF7A_7A7A,
            false,
        );
        let b = at(NEW_TRAIN, x, y);
        let hovered = inside(b, mouse);
        wide_button(
            draw,
            [b[0], b[1]],
            "create:gui/schedule_2",
            I_NEW_TRAIN,
            view.orthogonal,
            hovered,
        );
        if hovered {
            assembly_tip(host, view, "create.station.create_train", tips);
        }
        return;
    };
    // The train's icons, coming in as it nears (`trainPosition`).
    let width = train_icon_width(t);
    let mut target = 100.0 - (width / 2.0).floor();
    if width > 130.0 {
        target -= width - 130.0;
    }
    let f = if t.present {
        0.0
    } else {
        (t.distance / 30.0).min(1.0)
    };
    let position = target - (target + width) * f;
    let mut offset = 0.0;
    let fade = |offset: f32| {
        ((position + offset - 10.0) / 30.0)
            .min((200.0 - 40.0 - position - offset) / 30.0)
            .min(1.0)
    };
    let n = t.spacings.len();
    for i in (1..n).rev() {
        let length = if i == n - 1 && t.double_ended {
            -2
        } else {
            t.spacings[i]
        };
        offset += draw_icon(
            draw,
            t.icon,
            length,
            x + position + offset,
            y + 20.0,
            fade(offset),
        ) + 1.0;
    }
    offset += draw_icon(
        draw,
        t.icon,
        -1,
        x + position + offset,
        y + 20.0,
        fade(offset),
    );
    // The speech box with the train's name.
    draw.sprite(
        "create:gui/assemble",
        [1.0, 179.0, 150.0, 18.0],
        [x + 21.0, y + 42.0, 150.0, 18.0],
        None,
    );
    draw.sprite(
        "create:gui/assemble",
        [1.0, 198.0, 150.0, 1.0],
        [x + 21.0, y + 60.0, 150.0, 26.0],
        None,
    );
    draw.sprite(
        "create:gui/assemble",
        [1.0, 200.0, 150.0, 4.0],
        [x + 21.0, y + 86.0, 150.0, 4.0],
        None,
    );
    let speech = (position + offset - 13.0).clamp(25.0, 159.0);
    draw.sprite(
        "create:gui/assemble",
        [152.0, 179.0, 8.0, 6.0],
        [x + speech, y + 38.0, 8.0, 6.0],
        None,
    );
    let shown_name = if train_name.is_empty() && focus != Focus::TrainName {
        tr(host, "create.train.unnamed", &[])
    } else {
        train_name.to_owned()
    };
    let tx = name_box_x(draw, &shown_name, 125.0, x);
    let mut line = shown_name.clone();
    if focus == Focus::TrainName && blink {
        line.push('_');
    }
    if focus == Focus::TrainName && selected {
        let w = draw.text_width(&shown_name);
        draw.fill([tx + 4.0, y + 46.0, w, 10.0], 0xFF00_00FF);
    }
    draw.text(&line, [tx + 4.0, y + 47.0], 0xFFC6_C6C6, true);
    if focus != Focus::TrainName {
        let bx = (tx + draw.text_width(&shown_name) + 5.0).min(x + 156.0);
        draw.sprite(
            "create:gui/schedule_2",
            STATION_EDIT_TRAIN_NAME,
            [bx, y + 44.0, 13.0, 13.0],
            None,
        );
    }
    let b = at(DISASSEMBLE, x, y);
    let hovered = inside(b, mouse);
    let active = t.present && t.can_disassemble && view.orthogonal;
    wide_button(
        draw,
        [b[0], b[1]],
        "create:gui/schedule_2",
        I_DISASSEMBLE_TRAIN,
        active,
        hovered,
    );
    if hovered {
        assembly_tip(host, view, "create.station.disassemble_train", tips);
    }
    let s = at(DROP_SCHEDULE, x, y);
    let hovered = inside(s, mouse);
    icon_button(draw, [s[0], s[1]], I_VIEW_SCHEDULE, t.has_schedule, hovered);
    if hovered && t.has_schedule {
        tips.push(tr(host, "create.station.remove_schedule", &[]));
    }
}

/// `updateAssemblyTooltip`: why a train can't be made or taken apart here, else what the
/// button does.
fn assembly_tip(host: &dyn ClientHost, view: &View, plain: &str, tips: &mut Vec<String>) {
    let why = if !view.orthogonal {
        Some("create.station.no_assembly_diagonal")
    } else if view
        .train
        .as_ref()
        .is_some_and(|t| t.present && !t.can_disassemble)
    {
        Some("create.station.train_not_aligned")
    } else {
        None
    };
    match why {
        None => tips.push(tr(host, plain, &[])),
        Some(k) => {
            tips.push(tr(host, k, &[]));
            tips.push(tr(host, &format!("{k}_1"), &[]));
        }
    }
}

/// What closing sends (`removed`): the station screen's name and the train's name and icon.
fn closing_edits(open: &Open) -> Vec<Edit> {
    let pos = open.view.pos;
    let mut out = Vec::new();
    if !open.view.assembling {
        out.push(Edit::Configure {
            pos,
            assembly: false,
            name: open.name.clone(),
        });
        if let Some(t) = &open.view.train {
            out.push(Edit::Train {
                pos,
                id: t.id,
                name: open.train_name.clone(),
                icon: t.icon,
            });
        }
    }
    out.push(Edit::Closed { pos });
    out
}

pub fn screen_mouse_pressed(
    host: &dyn ClientHost,
    event: &mut ScreenMouseButtonPressedPreEvent<'_>,
) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    if event.button != 0 {
        return Flow::Cancel;
    }
    let mouse = event.screen.mouse;
    let mut send = Vec::new();
    let mut close = false;
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        let (x, y) = corner(o.size, o.view.assembling);
        let pos = o.view.pos;
        if inside(at(confirm(o.view.assembling), x, y), mouse) {
            click_sound(host);
            close = true;
            return;
        }
        if o.view.assembling {
            if inside(at(ASSEMBLE, x, y), mouse) && o.view.bogeys > 0 {
                click_sound(host);
                send.push(Edit::Assemble { pos });
            } else if inside(at(QUIT_ASSEMBLY, x, y), mouse) {
                click_sound(host);
                send.push(Edit::Configure {
                    pos,
                    assembly: false,
                    name: o.view.name.clone(),
                });
            }
            return;
        }
        // `mouseClicked`: the title row focuses the name, the speech box the train's.
        if o.focus != Focus::Name
            && mouse[1] > y
            && mouse[1] < y + 14.0
            && mouse[0] > x
            && mouse[0] < x + 200.0
        {
            o.focus = Focus::Name;
            o.selected = true;
            return;
        }
        if o.view.train.is_some()
            && o.focus != Focus::TrainName
            && mouse[1] > y + 45.0
            && mouse[1] < y + 58.0
            && mouse[0] > x + 25.0
            && mouse[0] < x + 168.0
        {
            o.focus = Focus::TrainName;
            o.selected = true;
            if o.train_name.is_empty() {
                // The box holds the train's name as shown: `train.unnamed`.
                o.train_name = tr(host, "create.train.unnamed", &[]);
            }
            return;
        }
        o.focus = Focus::None;
        if let Some(t) = &o.view.train
            && t.has_schedule
            && inside(at(DROP_SCHEDULE, x, y), mouse)
        {
            click_sound(host);
            send.push(Edit::DropSchedule { pos });
            return;
        }
        let to_assembly = match &o.view.train {
            None => o.view.orthogonal && inside(at(NEW_TRAIN, x, y), mouse),
            Some(t) => {
                t.present
                    && t.can_disassemble
                    && o.view.orthogonal
                    && inside(at(DISASSEMBLE, x, y), mouse)
            }
        };
        if to_assembly {
            // `assemblyCallback`: the assembly screen, the station switching to assembly mode
            // as the station screen goes (`removed` with `switchingToAssemblyMode`).
            click_sound(host);
            send.push(Edit::Configure {
                pos,
                assembly: true,
                name: o.name.clone(),
            });
            o.view.assembling = true;
            o.view.train = None;
            o.shown_train = None;
        }
    });
    if close && let Some(open) = OPEN.with_borrow_mut(Option::take) {
        send.extend(closing_edits(&open));
    }
    LATER.with_borrow_mut(|l| {
        l.0.extend(send);
        l.1 |= close;
    });
    Flow::Cancel
}

/// The door control's `SelectionScrollInput`: a notch picks the next or last mode, sent at
/// once (`StationEditPacket.configure` with the door control).
pub fn screen_mouse_scrolled(
    host: &dyn ClientHost,
    event: &mut mod_api::ScreenMouseScrolledPreEvent<'_>,
) -> Flow {
    if !ours(host) || event.scroll[1] == 0.0 {
        return Flow::Continue;
    }
    let mouse = event.screen.mouse;
    let edit = OPEN.with_borrow_mut(|o| {
        let o = o.as_mut()?;
        if o.view.assembling {
            return None;
        }
        let (x, y) = corner(o.size, false);
        if !inside([x + 35.0, y + 102.0, 53.0, 16.0], mouse) {
            return None;
        }
        let n = super::doors::MODES.len() as i32;
        let step = if event.scroll[1] > 0.0 { -1 } else { 1 };
        let next = (i32::from(o.view.doors) + step).clamp(0, n - 1) as u8;
        if next == o.view.doors {
            return Some(None);
        }
        o.view.doors = next;
        Some(Some(Edit::Doors {
            pos: o.view.pos,
            mode: next,
        }))
    });
    match edit {
        None => Flow::Continue,
        Some(e) => {
            if let Some(e) = e {
                click_sound(host);
                LATER.with_borrow_mut(|l| l.0.push(e));
            }
            Flow::Cancel
        }
    }
}

pub fn screen_key_pressed(host: &dyn ClientHost, event: &mut ScreenKeyPressedPreEvent<'_>) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let focus = OPEN.with_borrow(|o| o.as_ref().map_or(Focus::None, |o| o.focus));
    if focus == Focus::None {
        return Flow::Continue;
    }
    match event.key {
        "key.keyboard.backspace" => {
            OPEN.with_borrow_mut(|o| {
                if let Some(o) = o.as_mut() {
                    let all = std::mem::take(&mut o.selected);
                    let field = match o.focus {
                        Focus::Name => &mut o.name,
                        Focus::TrainName => &mut o.train_name,
                        Focus::None => return,
                    };
                    if all {
                        field.clear();
                    } else {
                        field.pop();
                    }
                }
            });
            Flow::Cancel
        }
        "key.keyboard.enter" | "key.keyboard.keypad.enter" => {
            // `syncStationName` / `syncTrainNameAndColor`.
            let edit = OPEN.with_borrow_mut(|o| {
                let o = o.as_mut()?;
                let pos = o.view.pos;
                let focus = std::mem::replace(&mut o.focus, Focus::None);
                match focus {
                    Focus::Name if o.name != o.view.name => Some(Edit::Configure {
                        pos,
                        assembly: false,
                        name: o.name.clone(),
                    }),
                    Focus::TrainName => o.view.train.as_ref().map(|t| Edit::Train {
                        pos,
                        id: t.id,
                        name: o.train_name.clone(),
                        icon: t.icon,
                    }),
                    _ => None,
                }
            });
            if let Some(e) = edit {
                LATER.with_borrow_mut(|l| l.0.push(e));
            }
            Flow::Cancel
        }
        "key.keyboard.escape" => Flow::Continue,
        _ => Flow::Cancel,
    }
}

pub fn screen_char_typed(host: &dyn ClientHost, event: &mut ScreenCharTypedPreEvent<'_>) -> Flow {
    if !ours(host) {
        return Flow::Continue;
    }
    let c = event.c;
    if !super::gui::allowed(c) {
        return Flow::Cancel;
    }
    OPEN.with_borrow_mut(|o| {
        let Some(o) = o.as_mut() else {
            return;
        };
        let all = std::mem::take(&mut o.selected);
        let (field, max) = match o.focus {
            Focus::Name => (&mut o.name, NAME_MAX),
            Focus::TrainName => (&mut o.train_name, TRAIN_NAME_MAX),
            Focus::None => return,
        };
        if all {
            field.clear();
        }
        if field.chars().count() < max {
            field.push(c);
        }
    });
    Flow::Cancel
}

/// `removed`: what the screen sends as it goes, at the next tick.
pub fn screen_closing(host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
    if event.screen.id != "mod" || host.mod_screen().is_some_and(|m| m != MOD) {
        return;
    }
    if let Some(open) = OPEN.with_borrow_mut(Option::take) {
        let edits = closing_edits(&open);
        LATER.with_borrow_mut(|l| l.0.extend(edits));
    }
}

/// `create_debug station <x> <y> <z> <name NAME | assemble | exit | disassemble | go NAME>`, for tests
/// (operators only): what the station's screen does (`StationEditPacket`: its name, assembling
/// the bogeys on its track, taking apart the train at it), and the train standing at it given a
/// schedule to station `NAME` and back (`ScheduleItem.handScheduleTo`, `DestinationInstruction`
/// with an empty condition list each).
pub fn debug_command(
    host: &mut dyn mod_api::ServerHost,
    event: &mut mod_api::RunModCommandEvent<'_>,
) -> mod_api::Flow {
    let cmd = event.command;
    if cmd
        .literals
        .iter()
        .map(String::as_str)
        .ne(["create_debug", "station"])
    {
        return mod_api::Flow::Continue;
    }
    let args = match cmd.argument("args") {
        Some(mod_api::CommandArgumentValue::String(s)) => s.clone(),
        _ => String::new(),
    };
    let words: Vec<&str> = args.split_whitespace().collect();
    let dimension = cmd
        .dimension
        .id(&*host)
        .map(|d| d.to_string())
        .unwrap_or_else(|| "minecraft:overworld".to_owned());
    let line = (|| {
        let [x, y, z, op, rest @ ..] = words.as_slice() else {
            return "usage: <x> <y> <z> <name N | assemble | exit | disassemble | go N | packages F>".to_owned();
        };
        let pos = [x, y, z].map(|v| v.parse::<i32>().unwrap_or(0));
        let Some(mut lv) = ServerLevel::new(host, &dimension) else {
            return "no level".to_owned();
        };
        let dim = lv.dim;
        match *op {
            "name" => {
                rename(&lv, pos, rest.join(" "));
                format!("station {pos:?} named {}", rest.join(" "))
            }
            "assemble" => {
                if !assembly::assembling(dim, pos) {
                    assembly::enter_assembly(&mut lv, pos, None);
                }
                let ok = assembly::assemble(&mut lv, pos);
                format!(
                    "assemble {pos:?}: {ok} {:?}",
                    assembly::failure(dim, pos).map(|f| f.0)
                )
            }
            "exit" => {
                let ok = assembly::exit_assembly(&mut lv, pos);
                format!("exit assembly {pos:?}: {ok}")
            }
            "disassemble" => {
                let ok = assembly::disassemble(&mut lv, pos, None);
                format!("disassemble {pos:?}: {ok}")
            }
            // The schedule's package instructions, for tests: deliver, then fetch for `filter`.
            "packages" => {
                let Some(train) = assembly::present_train(dim, pos) else {
                    return format!("no train at {pos:?}");
                };
                use create_engine::schedule::{Condition, Entry, Instruction, Schedule, TimeUnit};
                // Each waits a second at its station (an entry with no condition waits forever).
                let wait = || {
                    vec![vec![Condition::Delay {
                        value: 1,
                        unit: TimeUnit::Seconds,
                    }]]
                };
                let schedule = Schedule {
                    entries: vec![
                        Entry {
                            instruction: Instruction::DeliverPackages,
                            conditions: wait(),
                        },
                        Entry {
                            instruction: Instruction::FetchPackages(rest.join(" ")),
                            conditions: wait(),
                        },
                    ],
                    cyclic: true,
                    saved_progress: 0,
                };
                let ok = super::conductor::set_schedule(dim, train, schedule, false);
                format!(
                    "train {train} delivers and fetches \"{}\": {ok}",
                    rest.join(" ")
                )
            }
            "go" => {
                let Some(train) = assembly::present_train(dim, pos) else {
                    return format!("no train at {pos:?}");
                };
                let here = with_world(|w| w.dims[dim].railway.stations.get(&pos).cloned())
                    .flatten()
                    .unwrap_or_default();
                let schedule = create_engine::schedule::Schedule {
                    entries: vec![
                        create_engine::schedule::Entry::station(&rest.join(" "), Vec::new()),
                        create_engine::schedule::Entry::station(&here, Vec::new()),
                    ],
                    cyclic: false,
                    saved_progress: 0,
                };
                let ok = super::conductor::set_schedule(dim, train, schedule, false);
                format!(
                    "train {train} to {} and back to {here}: {ok}",
                    rest.join(" ")
                )
            }
            _ => "unknown".to_owned(),
        }
    })();
    let _ = host.commands().feedback(mod_api::Text::literal(line));
    mod_api::Flow::Cancel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_and_edits_cross_the_wire() {
        let v = View {
            pos: [1, 2, 3],
            name: "Depot".into(),
            assembling: false,
            orthogonal: true,
            bogeys: 0,
            failure: Some((
                "create.train_assembly.too_many_bogeys".into(),
                vec!["21".into()],
                -1,
            )),
            doors: 3,
            train: Some(TrainView {
                id: 7,
                name: "Express".into(),
                icon: 1,
                present: true,
                can_disassemble: true,
                has_schedule: false,
                spacings: vec![3, 5],
                double_ended: true,
                distance: 12.5,
            }),
        };
        assert_eq!(View::decode(&v.encode()), Some(v));
        for e in [
            Edit::Configure {
                pos: [1, 2, 3],
                assembly: true,
                name: "North".into(),
            },
            Edit::Assemble { pos: [4, 5, 6] },
            Edit::Train {
                pos: [0, 0, 0],
                id: 9,
                name: "A".into(),
                icon: 2,
            },
            Edit::Closed { pos: [1, 1, 1] },
        ] {
            assert_eq!(Edit::decode(&e.encode()), Some(e));
        }
    }

    #[test]
    fn train_icons_are_as_wide_as_create_draws_them() {
        let t = TrainView {
            spacings: vec![3, 5, 10],
            double_ended: false,
            ..TrainView::default()
        };
        // The engine, a 5 long carriage and a 10 long one, a pixel apart.
        assert_eq!(train_icon_width(&t), 19.0 + 14.0 + 20.0);
    }
}
