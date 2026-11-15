//! Driving Create's trains: the rules are `create_engine::train_controls`'s and the railway's
//! (`Railway::control`). The client half sees a use click aimed at a carriage it knows
//! (`ContraptionHandlerClient`) and sends it; while its player controls a train it keeps W, S,
//! A, D, space and left shift from moving the player and sends them as Create's
//! `ControlsHandler` does, and draws Create's `TrainHUD` (speed, throttle set by the wheel, the
//! prompt box, the direction arrow). The server half gives players a carriage's controls
//! (`ControlsInteractionBehaviour`), keeps their keys alive (`ControlsServerHandler`) and
//! drives the train by them. Nothing runs without a controlling player's word or a click
//! waiting: each hook's miss is one emptiness check. Carriage seats are `riding`'s.

use super::super::net;
use super::super::server::ServerLevel;
use super::super::states;
use super::super::subsystem::Subsystem;
use super::super::world::with_world;
use super::client;
use create_engine::Block;
use create_engine::train_controls::{
    self as tc, Driver, Driving, Frame, Inputs, Prompt, Target, ToClient, ToServer,
};
use create_engine::train_hud::{self as th, Hud};
use mod_api::text::TextStyle;
use mod_api::{
    BlockState, ClientHost, CrosshairTarget, Flow, GuiDraw, Hand, Host, InteractionKey,
    InteractionKeyMappingTriggeredEvent, MouseScrollingEvent, MovementInputEvent, PlayerRef,
    RenderGuiPostEvent, ServerHost, Text,
};
use std::cell::{Cell, RefCell};

pub struct TrainControls;

/// A carriage's controls held (`AbstractContraptionEntity.controllingPlayer`): by whom, whether
/// they face the train's back, what `control` keeps between ticks, and the ticks to the
/// holder's next HUD word (`hudPacketCooldown`).
#[derive(Clone, Copy, Debug)]
struct Controlling {
    dim: usize,
    at: Target,
    player: u128,
    inverted: bool,
    driving: Driving,
    hud: u8,
}

thread_local! {
    /// The client's hold on a carriage's controls, and whether it has one (the key hooks', the
    /// client tick's, the HUD's and the wheel's one test); its HUD (`TrainHUD`) and the HUD's
    /// prompt (`currentPrompt`, with `currentPromptShadow`).
    static DRIVER: RefCell<Driver> = RefCell::new(Driver::default());
    static DRIVING: Cell<bool> = const { Cell::new(false) };
    static HUD: RefCell<Hud> = RefCell::new(Hud::default());
    static PROMPT: RefCell<Option<(Text, bool)>> = const { RefCell::new(None) };
    /// The server's: clicks on carriages waiting for the level's tick (player, level, the
    /// block, a wrench in hand), every player's keys by level, and the controls held.
    static CLICKS: RefCell<Vec<(u128, usize, Target, bool)>> = const { RefCell::new(Vec::new()) };
    static INPUTS: RefCell<Vec<(usize, Inputs)>> = const { RefCell::new(Vec::new()) };
    static CONTROLLING: RefCell<Vec<Controlling>> = const { RefCell::new(Vec::new()) };
}

/// The players holding train `train`'s controls in level `dim` (`getControllingPlayer`).
pub(crate) fn drivers(dim: usize, train: u32) -> Vec<u128> {
    CONTROLLING.with_borrow(|c| {
        c.iter()
            .filter(|c| c.dim == dim && c.at.train == train)
            .map(|c| c.player)
            .collect()
    })
}

impl Subsystem for TrainControls {
    fn world_closed(&self) {
        CLICKS.with_borrow_mut(Vec::clear);
        INPUTS.with_borrow_mut(Vec::clear);
        CONTROLLING.with_borrow_mut(Vec::clear);
    }

    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        if channel == super::horn::CHANNEL {
            super::horn::from_client(host, player, data);
            return true;
        }
        if channel != tc::CHANNEL {
            return false;
        }
        net_from_client(host, player, data);
        true
    }

    fn server_message(&self, host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        if channel == super::horn::CHANNEL {
            super::horn::from_server(data);
            return true;
        }
        if channel != tc::CHANNEL {
            return false;
        }
        net_from_server(host, data);
        true
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        client_tick(host);
        super::horn::client_tick(host);
    }

    fn client_reset(&self) {
        DRIVER.with_borrow_mut(|d| *d = Driver::default());
        DRIVING.set(false);
        HUD.with_borrow_mut(|h| *h = Hud::default());
        PROMPT.with_borrow_mut(|p| *p = None);
        super::horn::reset();
    }

    fn interaction_key(
        &self,
        host: &mut dyn ClientHost,
        event: &mut InteractionKeyMappingTriggeredEvent,
    ) -> Flow {
        interaction(host, event)
    }

    fn movement_input(&self, _host: &dyn ClientHost, event: &mut MovementInputEvent) {
        // `ControlsHandler.tick`'s `setDown(false)`: the six controls move nothing.
        if !DRIVING.get() {
            return;
        }
        event.forward = 0.0;
        event.strafe = 0.0;
        event.jump = false;
        event.sneak = false;
        event.forward_key = false;
        event.backward_key = false;
        event.left_key = false;
        event.right_key = false;
    }

    fn mouse_scrolling(&self, _host: &mut dyn ClientHost, event: &mut MouseScrollingEvent) -> Flow {
        // `TrainHUD.onScroll`: while controlling, the wheel sets the train's throttle (a notch
        // is 1/18) and nothing else hears it.
        let steps = event.scroll[1] as f32;
        if !DRIVING.get() || steps == 0.0 {
            return Flow::Continue;
        }
        HUD.with_borrow_mut(|h| h.scroll(steps));
        Flow::Cancel
    }

    fn render_gui_post(&self, host: &dyn ClientHost, event: &mut RenderGuiPostEvent<'_>) {
        hud(host, event);
    }
}

// ---------------------------------------------------------------------------------------------
// Client

/// A component in colour `rgb`.
fn colored(text: impl Into<String>, rgb: u32) -> Text {
    let mut style = TextStyle::default();
    style.color = Some(format!("#{:06X}", rgb & 0xFF_FF_FF));
    Text::literal(text).with_style(style)
}

/// `ContraptionHandlerClient.rightClickingOnContraptionsGetsHandledLocally`: a use click by a
/// player who isn't a spectator, along their look as far as their reach or the block the
/// crosshair is on, that hits a carriage's block is the carriage's: it goes to the server and
/// nothing else happens, without a swing. Nothing is looked at without trains known.
fn interaction(host: &mut dyn ClientHost, event: &mut InteractionKeyMappingTriggeredEvent) -> Flow {
    if event.key != InteractionKey::Use
        || event.hand != Hand::Main
        || event.cancelled
        || !client::knows_trains()
    {
        return Flow::Continue;
    }
    let (Some(player), Some(camera)) = (host.local_player(), host.camera()) else {
        return Flow::Continue;
    };
    if player.is_spectator(&*host) == Some(true) {
        return Flow::Continue;
    }
    let eye = camera.position;
    let (yaw, pitch) = (camera.yaw.to_radians(), camera.pitch.to_radians());
    let look = [
        f64::from(-yaw.sin() * pitch.cos()),
        f64::from(-pitch.sin()),
        f64::from(yaw.cos() * pitch.cos()),
    ];
    let mut reach: f64 = if player.is_creative(&*host) == Some(true) {
        5.0
    } else {
        4.5
    };
    // `getRayInputs`: no further than what the crosshair is on, a block or an entity (a mob on
    // a carriage seat is clicked, not the seat).
    let point = match host.crosshair_target() {
        Some(CrosshairTarget::Block { hit, .. }) => Some(hit.point),
        // An entity under the crosshair (a conductor on a carriage seat) is what is clicked:
        // it stands before the carriage's blocks behind it.
        Some(CrosshairTarget::Entity { .. }) => return Flow::Continue,
        _ => None,
    };
    if let Some(point) = point {
        let d: f64 = (0..3).map(|i| (point[i] - eye[i]).powi(2)).sum();
        reach = d.sqrt().min(reach);
    }
    let target = std::array::from_fn(|i| eye[i] + look[i] * reach);
    let Some((at, _)) = client::carriage_hit(&*host, eye, target) else {
        return Flow::Continue;
    };
    let wrench = player
        .selected_slot(&*host)
        .and_then(|slot| player.inventory(&*host)?.into_iter().nth(slot))
        .is_some_and(|s| s.item().id(&*host) == "create:wrench");
    let click = ToServer::Click { target: at, wrench };
    net::send_to_server(host, tc::CHANNEL, &click.encode());
    event.cancelled = true;
    event.swing_hand = false;
    Flow::Cancel
}

fn send_server(host: &mut dyn ClientHost, packets: Vec<ToServer>) {
    for p in packets {
        net::send_to_server(host, tc::CHANNEL, &p.encode());
    }
}

/// The server's word: this player controls a carriage's controls now, or no more
/// (`ControlsStopControllingPacket`: the keys held are released to the server).
fn net_from_server(host: &mut dyn ClientHost, data: &[u8]) {
    let message = ToClient::decode(data);
    if matches!(message, Some(ToClient::Start(_) | ToClient::Stop)) {
        HUD.with_borrow_mut(|h| *h = Hud::default());
        PROMPT.with_borrow_mut(|p| *p = None);
    }
    let line = match &message {
        Some(ToClient::Prompt(p)) => prompt_text(&*host, p),
        _ => None,
    };
    if let Some(line) = line {
        PROMPT.with_borrow_mut(|c| *c = Some(line));
        HUD.with_borrow_mut(Hud::prompted);
    }
    let packets = DRIVER.with_borrow_mut(|d| match message {
        Some(ToClient::Start(at)) => {
            d.start(at);
            Vec::new()
        }
        Some(ToClient::Stop) => d.stop(),
        Some(ToClient::Hud { speed, throttle }) => {
            HUD.with_borrow_mut(|h| {
                h.speed = speed;
                h.throttle = th::throttle(throttle) as f32;
            });
            Vec::new()
        }
        Some(ToClient::Prompt(_)) | None => Vec::new(),
    });
    send_server(host, packets);
    DRIVING.set(DRIVER.with_borrow(|d| d.controls.is_some()));
}

/// A prompt as the HUD shows it (`TrainPromptPacket`'s component), with whether it has a
/// shadow (an approach's progress).
fn prompt_text(host: &dyn ClientHost, p: &Prompt) -> Option<(Text, bool)> {
    let station = |key: &str, name: &str| Text::translate(key, vec![colored(name, 0x70_46_30)]);
    Some(match p {
        Prompt::None | Prompt::Clear => return None,
        Prompt::Arrived(name) => (station("create.train.arrived_at", name), false),
        Prompt::Departing(name) => (station("create.train.departing_from", name), false),
        Prompt::Approach(name) => {
            // `Component.keybind("key.jump")`: the key the player bound.
            let key = host.mapping("key.jump").ok().flatten().map_or_else(
                || Text::translate("key.keyboard.space", Vec::new()),
                |m| m.label,
            );
            let line = Text::translate(
                "create.contraption.controls.approach_station",
                vec![key, Text::literal(name.as_str())],
            );
            (line, false)
        }
        Prompt::Progress(progress) => {
            let (mixed, rest) = tc::progress_colors(*progress);
            let gone = "|".repeat(usize::from(30 - (*progress).min(30)));
            let to_go = "|".repeat(usize::from((*progress).min(30)));
            (colored(gone, mixed).append(colored(to_go, rest)), true)
        }
    })
}

/// `ControlsHandler.tick`: the six keys held on the keyboard itself and escape, sent as Create
/// sends them. Nothing runs while not controlling.
fn client_tick(host: &mut dyn ClientHost) {
    if !DRIVING.get() {
        return;
    }
    if !host.playing() {
        DRIVER.with_borrow_mut(|d| *d = Driver::default());
        DRIVING.set(false);
        return;
    }
    let down = create_engine::controller::KEYS
        .iter()
        .enumerate()
        .filter(|(_, k)| host.is_key_down(k))
        .fold(0u8, |m, (i, _)| m | (1 << i));
    let escape = host.is_key_down("key.keyboard.escape");
    let mut packets = DRIVER.with_borrow_mut(|d| d.tick(down, escape));
    // `TrainHUD.tick`: the bars chase the train, the prompt lives 30 ticks, and a throttle set
    // by the wheel goes to the server until it agrees.
    let width = PROMPT.with_borrow(|p| {
        p.as_ref()
            .and_then(|(t, _)| host.text_width(t).ok())
            .map(|w| w as f32)
    });
    let throttle = HUD.with_borrow_mut(|h| {
        let send = h.tick(width);
        if !h.prompt_alive() {
            PROMPT.with_borrow_mut(|p| *p = None);
        }
        send
    });
    if let (Some(steps), Some(target)) = (throttle, DRIVER.with_borrow(|d| d.controls)) {
        packets.push(ToServer::Throttle { target, steps });
    }
    if let Some(target) = DRIVER.with_borrow(|d| d.controls) {
        super::horn::driving_tick(host, target.train);
    }
    send_server(host, packets);
    DRIVING.set(DRIVER.with_borrow(|d| d.controls.is_some()));
}

/// `TrainHUD.renderOverlay`, only while controlling: the speed and throttle bars over the
/// hotbar and the prompt box above them, from Create's `widgets` texture, at the latest tick's
/// values (no partial tick here), and the direction arrow over its plate.
fn hud(host: &dyn ClientHost, event: &mut RenderGuiPostEvent<'_>) {
    if !DRIVING.get() {
        return;
    }
    let draw = &mut *event.draw;
    let size = draw.size();
    let prompt = PROMPT.with_borrow(Clone::clone);
    let width = prompt
        .as_ref()
        .and_then(|(t, _)| host.text_width(t).ok())
        .map(|w| w as f32);
    let drawn = HUD.with_borrow(|h| th::layout(h, size, 1.0, width));
    for (src, dst) in &drawn.parts {
        if !draw.sprite(th::TEXTURE, *src, *dst, None) {
            return;
        }
    }
    let driving = DRIVER.with_borrow(|d| d.controls.map(|at| (at, d.pressed)));
    if let Some((at, pressed)) = driving
        && let Some((yaw, heading, state)) = client::carriage_view(host, at)
        && let Some([player_yaw, _]) = host.local_player().and_then(|p| p.entity().rotation(host))
    {
        let angle = th::arrow_angle(player_yaw, yaw, inverted(host, state, heading), pressed);
        let (src, dst) = th::arrow(size, angle);
        draw.sprite(th::ARROW_TEXTURE, src, dst, None);
    }
    if let (Some([x, y]), Some((text, shadow))) = (drawn.text, prompt) {
        let _ = draw.gui_paint(&[GuiDraw::Text {
            text,
            x: x as i32,
            y: y as i32,
            color: 0xFF00_0000 | th::PROMPT_TEXT,
            shadow,
        }]);
    }
}

/// Whether the controls' block faces away from the way the train was assembled toward
/// (`control`'s `inverted`: its facing isn't the carriage's `getInitialOrientation`
/// counterclockwise, the assembly direction, which is opposite the train's heading).
fn inverted(host: &dyn Host, state: BlockState, heading: [i32; 2]) -> bool {
    let assembly = match [-heading[0], -heading[1]] {
        [1, _] => "east",
        [-1, _] => "west",
        [_, 1] => "south",
        _ => "north",
    };
    super::property(host, state, "facing").is_none_or(|f| f != assembly)
}

// ---------------------------------------------------------------------------------------------
// Server

/// A carriage's pose (`Carriage`'s entity): its frame, from the train on the graph.
fn frame(dim: usize, train: u32, car: u16) -> Option<Frame> {
    with_world(|w| {
        let r = &w.dims[dim].railway;
        let t = r.trains.iter().find(|t| t.id == train && t.train.bound)?;
        let c = usize::from(car);
        (c < t.train.carriages.len()).then(|| {
            let (pivot, front) = t.train.carriage_pose(&r.net.graph, c);
            Frame::new(pivot, front, t.heading)
        })
    })
    .flatten()
}

/// The state of a carriage's block, and the carriage's size along x and up
/// (`getBoundingBox`'s widest sides, for the click's reach).
fn carriage_block(dim: usize, at: Target) -> Option<(u32, f64)> {
    with_world(|w| {
        let t = w.dims[dim]
            .railway
            .trains
            .iter()
            .find(|t| t.id == at.train)?;
        let car = t.cars.get(usize::from(at.car))?;
        let state = car.iter().find(|(o, _)| *o == at.local)?.1;
        let size = |k: usize| {
            let (lo, hi) = car.iter().fold((i32::MAX, i32::MIN), |(lo, hi), (o, _)| {
                (lo.min(o[k]), hi.max(o[k]))
            });
            f64::from(hi - lo + 1)
        };
        Some((state, size(0).max(size(2)).max(size(1))))
    })
    .flatten()
}

/// A server player: their level's index, where they stand, and whether they are a spectator
/// or in creative.
fn server_player(host: &dyn ServerHost, uuid: u128) -> Option<(usize, [f64; 3], bool, bool)> {
    let p = host.player(uuid)?;
    let dimension = p.level(host)?.dimension().id(host)?;
    let dim = super::super::world::level_index(&dimension)?;
    let at = p.entity().position(host)?;
    let (spectator, creative) = super::player_mode(host, p);
    Some((dim, at, spectator, creative))
}

/// Their eyes (`getEyePosition`) and reach (`blockInteractionRange`).
fn eye_and_reach(at: [f64; 3], creative: bool) -> ([f64; 3], f64) {
    (
        [at[0], at[1] + 1.62, at[2]],
        if creative { 5.0 } else { 4.5 },
    )
}

/// A player's message: a click on a carriage waits for the level's tick; keys go to the
/// player's controls (`ControlsInputPacket.handle`: a spectator presses nothing; the controls
/// within 16 past their reach), or let them go.
fn net_from_client(host: &mut dyn ServerHost, player: PlayerRef, data: &[u8]) {
    let Some(message) = ToServer::decode(data) else {
        return;
    };
    let uuid = player.uuid();
    let Some((dim, at, spectator, creative)) = server_player(&*host, uuid) else {
        return;
    };
    match message {
        ToServer::Click { target, wrench } => {
            CLICKS.with_borrow_mut(|c| c.push((uuid, dim, target, wrench)));
        }
        ToServer::Input { target, stop, .. } if stop => stop_controlling(host, dim, target),
        // `TrainHUDUpdatePacket.Serverbound.handle`: the train's throttle.
        ToServer::Throttle { target, steps } => {
            with_world(|w| {
                let r = &mut w.dims[dim].railway;
                if let Some(t) = r.trains.iter_mut().find(|t| t.id == target.train) {
                    t.train.throttle = th::throttle(steps);
                }
            });
        }
        ToServer::Input {
            target,
            keys,
            press,
            ..
        } => {
            if spectator && press {
                return;
            }
            let (eye, reach) = eye_and_reach(at, creative);
            if !frame(dim, target.train, target.car)
                .is_some_and(|f| f.within(target.local, eye, reach, tc::INPUT_RANGE))
            {
                return;
            }
            INPUTS.with_borrow_mut(|all| {
                let i = match all.iter().position(|(d, _)| *d == dim) {
                    Some(i) => i,
                    None => {
                        all.push((dim, Inputs::default()));
                        all.len() - 1
                    }
                };
                all[i].1.receive(uuid, target, keys, press);
            });
        }
    }
}

fn send_client(host: &mut dyn ServerHost, player: u128, m: ToClient) {
    if let Some(p) = host.player(player) {
        net::send_to_player(host, p, tc::CHANNEL, &m.encode());
    }
}

/// `AbstractContraptionEntity.stopControlling`: the carriage's controlling player is told
/// (`ControlsStopControllingPacket`) and it has none.
fn stop_controlling(host: &mut dyn ServerHost, dim: usize, at: Target) {
    let gone = CONTROLLING.with_borrow_mut(|c| {
        let i = c
            .iter()
            .position(|c| c.dim == dim && c.at.train == at.train && c.at.car == at.car)?;
        Some(c.remove(i).player)
    });
    if let Some(player) = gone {
        send_client(host, player, ToClient::Stop);
        if let Some(p) = host.player(player) {
            let line = Text::translate("create.contraption.controls.stop_controlling", Vec::new());
            super::action_bar(host, p, line);
        }
    }
}

/// The state numbered `id`, as a host's handle.
fn state(id: u32) -> Option<BlockState> {
    super::car_state(id)
}

/// `ControlsInteractionBehaviour.handlePlayerInteraction`: not with a wrench; whoever holds
/// the carriage's controls lets go (a second click of the same player only does that); else
/// the player takes them (`startControlling`: not a spectator, the train on the graph) and is
/// told.
fn use_controls(
    host: &mut dyn ServerHost,
    dim: usize,
    player: u128,
    spectator: bool,
    at: Target,
    id: u32,
) {
    let holder = CONTROLLING.with_borrow(|c| {
        c.iter()
            .find(|c| c.dim == dim && c.at.train == at.train && c.at.car == at.car)
            .map(|c| c.player)
    });
    if let Some(holder) = holder {
        stop_controlling(host, dim, at);
        if holder == player {
            return;
        }
    }
    if spectator {
        return;
    }
    let Some(heading) = with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let heading = r.trains.iter().find(|t| t.id == at.train)?.heading;
        r.start_controlling(at.train).then_some(heading)
    })
    .flatten() else {
        return;
    };
    let inverted = state(id).is_some_and(|s| inverted(&*host, s, heading));
    CONTROLLING.with_borrow_mut(|c| {
        c.push(Controlling {
            dim,
            at,
            player,
            inverted,
            driving: Driving::default(),
            hud: 0,
        })
    });
    send_client(host, player, ToClient::Start(at));
    if let Some(p) = host.player(player) {
        let name = Text::translate("create.train.unnamed", Vec::new());
        let line = Text::translate("create.contraption.controls.start_controlling", vec![name]);
        super::action_bar(host, p, line);
    }
}

/// `ContraptionInteractionPacket.handle`: the clicking player within their reach, 10 and the
/// carriage's widest side of it (Create takes 3 off without a line of sight; not here), then
/// the carriage's block: its controls (not with a wrench), else a seat to sit on.
fn click(host: &mut dyn ServerHost, dim: usize, player: u128, at: Target, wrench: bool) {
    let Some((_, pos, spectator, creative)) = server_player(&*host, player) else {
        return;
    };
    let (Some((id, size)), Some(f)) = (carriage_block(dim, at), frame(dim, at.train, at.car))
    else {
        return;
    };
    let reach = eye_and_reach(pos, creative).1 + 10.0 + size;
    let d2: f64 = (0..3).map(|i| (f.pivot[i] - pos[i]).powi(2)).sum();
    if d2 > reach * reach {
        return;
    }
    // `handleSpecialInteractions`: the wrench picks the train to move (`carriageWrenched`).
    if wrench {
        if let Some(p) = host.player(player) {
            super::relocate::wrenched(host, dim, p, at.train, f.pivot);
        }
        return;
    }
    let controls = state(id).is_some_and(|s| s.block() == states::key(Block::Controls));
    if controls && !wrench {
        use_controls(host, dim, player, spectator, at, id);
        return;
    }
    // `handlePlayerInteraction`'s seat: the player sits on it (not a spectator).
    if !spectator && state(id).is_some_and(|s| super::riding::is_seat(&*host, s)) {
        super::riding::sit(host, dim, player, at);
    }
}

/// What the train tells its driver: the HUD's prompt (`TrainPromptPacket`), or the approach
/// prompt taken away from the action bar (`cleanUpApproachStationMessage`).
fn prompt(host: &mut dyn ServerHost, player: u128, p: Prompt) {
    match p {
        Prompt::None => {}
        Prompt::Clear => {
            if let Some(pl) = host.player(player) {
                super::action_bar(host, pl, Text::literal(""));
            }
        }
        p => send_client(host, player, ToClient::Prompt(p)),
    }
}

/// `CarriageContraptionEntity.control`'s `TrainHUDUpdatePacket.Clientbound`: every 5 ticks,
/// each player holding a carriage's controls is told its train's speed (as a share of the top
/// speed by hand) and throttle. Create sends it while keys are held, its clients running the
/// trains between; here it goes while the controls are held. Nothing without controls held.
pub fn after_trains(lv: &mut ServerLevel<'_>) {
    if CONTROLLING.with_borrow(Vec::is_empty) {
        return;
    }
    let dim = lv.dim;
    let due: Vec<(u128, u32)> = CONTROLLING.with_borrow_mut(|c| {
        c.iter_mut()
            .filter(|c| c.dim == dim)
            .filter_map(|c| {
                if c.hud > 0 {
                    c.hud -= 1;
                    return None;
                }
                c.hud = th::PACKET_COOLDOWN;
                Some((c.player, c.at.train))
            })
            .collect()
    });
    for (player, train) in due {
        let Some((speed, throttle)) = with_world(|w| {
            let t = w.dims[dim].railway.trains.iter().find(|t| t.id == train)?;
            let top = create_engine::trains::TOP_SPEED * create_engine::trains::MANUAL_SPEED;
            let speed = (t.train.speed.abs() / top) as f32;
            Some((speed, th::steps(t.train.throttle as f32)))
        })
        .flatten() else {
            continue;
        };
        send_client(lv.host, player, ToClient::Hud { speed, throttle });
    }
}

/// Before the trains move in a server level's tick: the clicks on carriages, then
/// `ControlsServerHandler.tick`: each player's keys count down; a player gone lets go; else
/// the train is driven by the keys still held (`control`), and the controls are let go when it
/// says so (a spectator, out of reach (8 past theirs), sneak held, the train gone); keys all
/// released end the word, and the train runs on by itself. Nothing without clicks or keys.
pub fn before_trains(lv: &mut ServerLevel<'_>) {
    if CLICKS.with_borrow(Vec::is_empty) && INPUTS.with_borrow(Vec::is_empty) {
        return;
    }
    let dim = lv.dim;
    let clicks: Vec<(u128, usize, Target, bool)> = CLICKS.with_borrow_mut(|c| {
        let (here, rest) = std::mem::take(c).into_iter().partition(|c| c.1 == dim);
        *c = rest;
        here
    });
    for (player, _, at, wrench) in clicks {
        click(lv.host, dim, player, at, wrench);
    }
    let Some(mut inputs) = INPUTS.with_borrow_mut(|all| {
        let i = all.iter().position(|(d, _)| *d == dim)?;
        Some(all.swap_remove(i).1)
    }) else {
        return;
    };
    inputs.count_down();
    let mut let_go = Vec::new();
    for (player, ctx) in &inputs.players {
        let at = ctx.controls;
        let Some((_, pos, spectator, creative)) =
            server_player(&*lv.host, *player).filter(|p| p.0 == dim)
        else {
            stop_controlling(lv.host, dim, at);
            let_go.push(*player);
            continue;
        };
        let held = ctx.held();
        let (eye, reach) = eye_and_reach(pos, creative);
        let in_reach = frame(dim, at.train, at.car)
            .is_some_and(|f| f.within(at.local, eye, reach, tc::CONTROL_RANGE));
        // Its facing and what `control` kept: the holder's, else read again (Create drives by
        // keys that outlive the hold, from the block's state each tick).
        let kept = CONTROLLING.with_borrow(|c| {
            c.iter()
                .find(|c| c.dim == dim && c.at == at)
                .map(|c| (c.inverted, c.driving))
        });
        let (inverted, mut driving) = match kept {
            Some(k) => k,
            None => {
                let heading = with_world(|w| {
                    let r = &w.dims[dim].railway;
                    r.trains
                        .iter()
                        .find(|t| t.id == at.train)
                        .map(|t| t.heading)
                })
                .flatten();
                let facing = carriage_block(dim, at)
                    .zip(heading)
                    .and_then(|((id, _), h)| Some(inverted(&*lv.host, state(id)?, h)))
                    .unwrap_or(false);
                (facing, Driving::default())
            }
        };
        let result = if spectator || !in_reach || held & (1 << tc::SNEAK) != 0 {
            None
        } else {
            with_world(|w| {
                w.dims[dim]
                    .railway
                    .control(at.train, held, inverted, &mut driving)
            })
            .flatten()
        };
        CONTROLLING.with_borrow_mut(|c| {
            if let Some(c) = c.iter_mut().find(|c| c.dim == dim && c.at == at) {
                c.driving = driving;
            }
        });
        match result {
            Some(p) => prompt(lv.host, *player, p),
            None => stop_controlling(lv.host, dim, at),
        }
    }
    inputs.players.retain(|(p, _)| !let_go.contains(p));
    let before: Vec<u32> = inputs
        .players
        .iter()
        .map(|(_, c)| c.controls.train)
        .collect();
    inputs.drop_released();
    for train in before {
        if !inputs
            .players
            .iter()
            .any(|(_, c)| c.controls.train == train)
        {
            with_world(|w| w.dims[dim].railway.release(train));
        }
    }
    if !inputs.is_empty() {
        INPUTS.with_borrow_mut(|all| all.push((dim, inputs)));
    }
}

/// The controls at block `local` of carriage `car` of train `train` in level `dim` while a
/// player holds them: the keys held (a bit per control) and whether they face the train's back.
pub fn held_at(dim: usize, train: u32, car: usize, local: [i32; 3]) -> Option<(u8, bool)> {
    let (player, inverted) = CONTROLLING.with_borrow(|c| {
        c.iter()
            .find(|c| {
                c.dim == dim
                    && c.at.train == train
                    && usize::from(c.at.car) == car
                    && c.at.local == local
            })
            .map(|c| (c.player, c.inverted))
    })?;
    let keys = INPUTS.with_borrow(|all| {
        all.iter()
            .filter(|(d, _)| *d == dim)
            .flat_map(|(_, i)| i.players.iter())
            .find(|(p, _)| *p == player)
            .map_or(0, |(_, c)| c.held())
    });
    Some((keys, inverted))
}

/// `ControlsMovementBehaviour.stopMoving`: train `train` was taken apart; its controls are let
/// go.
pub fn disassembled(lv: &mut ServerLevel<'_>, train: u32) {
    let dim = lv.dim;
    let held: Vec<Target> = CONTROLLING.with_borrow(|c| {
        c.iter()
            .filter(|c| c.dim == dim && c.at.train == train)
            .map(|c| c.at)
            .collect()
    });
    for at in held {
        stop_controlling(lv.host, dim, at);
    }
}
