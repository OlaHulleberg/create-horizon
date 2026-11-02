// Riding and driving Create's trains in the game, injected as
// `crate::game::create_train_controls`; the rules are `create_engine::train_controls`'s and the
// railway's (`Railway::control`). The client half sees a use click aimed at a carriage drawn
// there (API 66's `native_interaction_key_mapping_triggered`, Create's
// `ContraptionHandlerClient`) and sends it; while its player controls a train it takes W, S, A,
// D, space and left shift from the game (API 57) and sends them as Create's `ControlsHandler`
// does. The server half seats players on a carriage's seats (mod seats, API 63, moved with the
// carriage every tick) and gives them its controls (`ControlsInteractionBehaviour`), keeps their
// keys alive (`ControlsServerHandler`) and drives the train by them. Nothing runs without a
// carriage seat in use, a key held, a controlling player's word or a click waiting: each hook's
// miss is one emptiness check.
use super::Game;
use super::create::*;
use super::native_entities::NativeEntity;
use create_engine::train_controls::{
    self as tc, Driver, Driving, Frame, Inputs, Prompt, Target, ToClient, ToServer,
};
use create_engine::world::dimension_index;
use create_engine::{Block, dye};
use horizon_client::text::Text;
use horizon_world::registry::BlockStateId;
use std::cell::{Cell, RefCell};

/// A carriage seat in use: its dimension, the seat block, the game's seat stand-in and
/// whether a player rides it (`Contraption.seatMapping`).
#[derive(Clone, Copy, Debug)]
struct CarSeat {
    dim: usize,
    at: Target,
    seat: u32,
    player: bool,
}

/// A carriage's controls held (`AbstractContraptionEntity.controllingPlayer`): by whom, whether
/// they face the train's back, and what `control` keeps between ticks.
#[derive(Clone, Copy, Debug)]
struct Controlling {
    dim: usize,
    at: Target,
    player: u128,
    inverted: bool,
    driving: Driving,
}

/// A rider waiting for the level's tick: to board a carriage seat from the world seat a train
/// took, or to leave a seat of a train taken apart for the seat block it left (if any).
#[derive(Clone, Copy, Debug)]
enum Pending {
    Board {
        dim: usize,
        from: [i32; 3],
        at: Target,
    },
    Alight {
        dim: usize,
        seat: u32,
        to: Option<[i32; 3]>,
    },
}

thread_local! {
    /// The client's hold on a carriage's controls, and whether it has one (the key hook's and
    /// the client tick's one test).
    static DRIVER: RefCell<Driver> = RefCell::new(Driver::default());
    static DRIVING: Cell<bool> = const { Cell::new(false) };
    /// The server's: clicks on carriages waiting for the level's tick (player, the block, a
    /// wrench in hand), every player's keys by dimension, the controls held and the carriage
    /// seats in use.
    static CLICKS: RefCell<Vec<(u128, usize, Target, bool)>> = const { RefCell::new(Vec::new()) };
    static INPUTS: RefCell<Vec<(usize, Inputs)>> = const { RefCell::new(Vec::new()) };
    static CONTROLLING: RefCell<Vec<Controlling>> = const { RefCell::new(Vec::new()) };
    static SEATS: RefCell<Vec<CarSeat>> = const { RefCell::new(Vec::new()) };
    /// Riders to seat on a train just assembled, or to let off one taken apart.
    static PENDING: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
}

/// The world closed: its seats and controls went with it.
pub(super) fn forget() {
    CLICKS.with_borrow_mut(Vec::clear);
    INPUTS.with_borrow_mut(Vec::clear);
    CONTROLLING.with_borrow_mut(Vec::clear);
    SEATS.with_borrow_mut(Vec::clear);
    PENDING.with_borrow_mut(Vec::clear);
}

fn block_of(state: u32) -> Option<Block> {
    with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()
}

// ---------------------------------------------------------------------------------------------
// Client

/// `ContraptionHandlerClient.rightClickingOnContraptionsGetsHandledLocally`: a use click by a
/// player who isn't a spectator, along their look as far as their reach or the block the
/// crosshair is on, that hits a carriage's block is the carriage's: it goes to the server and
/// nothing else happens, without a swing. Nothing is looked at without trains drawn.
pub(super) fn interaction(
    game: &mut Game<'_>,
    button: horizon_client::interaction_input::Button,
    canceled: &mut bool,
    swing: &mut bool,
) {
    if button != horizon_client::interaction_input::Button::Use
        || *canceled
        || !super::create_trains::client_draws_trains()
    {
        return;
    }
    let Some((eye, look)) = game.aim() else {
        return;
    };
    let mut reach = if game.creative { 5.0 } else { 4.5 };
    if let Some(hit) = game.pick() {
        let d: f64 = (0..3).map(|i| (hit.point[i] - eye[i]).powi(2)).sum();
        reach = d.sqrt().min(reach);
    }
    let target = std::array::from_fn(|i| eye[i] + look[i] * reach);
    let Some((at, _)) = super::create_trains::client_carriage_hit(game, eye, target) else {
        return;
    };
    let held = game.inventory.get(game.selected).and_then(Option::as_ref);
    let wrench = held.is_some_and(|s| &*s.item == "create:wrench");
    let click = ToServer::Click { target: at, wrench };
    let _ = game.native_net_send_to_server(tc::CHANNEL, &click.encode());
    *canceled = true;
    *swing = false;
}

/// `InputEvent.Key` while controlling (`ControlsHandler.tick`'s `setDown(false)`): the six
/// controls' presses are kept from the game, so the player neither walks, jumps nor sneaks
/// off. The miss is one flag.
pub(super) fn input_key(
    key: &str,
    action: horizon_client::debug_keys::KeyAction,
    screen: bool,
    consumed: &mut bool,
) {
    if !DRIVING.get() || screen {
        return;
    }
    if action != horizon_client::debug_keys::KeyAction::Release && tc::control(key).is_some() {
        *consumed = true;
    }
}

fn send_server(game: &mut Game<'_>, packets: Vec<ToServer>) {
    for p in packets {
        let _ = game.native_net_send_to_server(tc::CHANNEL, &p.encode());
    }
}

/// The server's word: this player controls a carriage's controls now, or no more
/// (`ControlsStopControllingPacket`: the keys held are released to the server).
pub(super) fn net_from_server(game: &mut Game<'_>, channel: &str, data: &[u8]) {
    if channel != tc::CHANNEL {
        return;
    }
    let packets = DRIVER.with_borrow_mut(|d| match ToClient::decode(data) {
        Some(ToClient::Start(at)) => {
            d.start(at);
            Vec::new()
        }
        Some(ToClient::Stop) => d.stop(),
        None => Vec::new(),
    });
    send_server(game, packets);
    DRIVING.set(DRIVER.with_borrow(|d| d.controls.is_some()));
}

/// `ControlsHandler.tick`: the six keys held on the keyboard itself (consumed or not) and
/// escape, sent as Create sends them. Nothing runs while not controlling.
pub(super) fn client_tick(game: &mut Game<'_>) {
    if !DRIVING.get() || !game.native_window() {
        return;
    }
    if !game.native_playing() {
        DRIVER.with_borrow_mut(|d| *d = Driver::default());
        DRIVING.set(false);
        return;
    }
    let down = create_engine::controller::KEYS
        .iter()
        .enumerate()
        .filter(|(_, k)| game.native_is_key_down(k))
        .fold(0u8, |m, (i, _)| m | (1 << i));
    let escape = game.native_is_key_down("key.keyboard.escape");
    let packets = DRIVER.with_borrow_mut(|d| d.tick(down, escape));
    send_server(game, packets);
    DRIVING.set(DRIVER.with_borrow(|d| d.controls.is_some()));
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

/// A server player, by UUID: their dimension's index, where they stand and their game mode.
fn server_player(game: &Game<'_>, uuid: u128) -> Option<(usize, String, [f64; 3], i32)> {
    let p = game
        .native_server_players()
        .into_iter()
        .find(|p| p.uuid == uuid)?;
    Some((
        dimension_index(&p.dimension)?,
        p.dimension,
        p.position,
        p.game_mode,
    ))
}

/// The player entity standing at `at` (a server player's position), nearest it.
fn player_entity(game: &Game<'_>, dimension: &str, at: [f64; 3]) -> Option<NativeEntity> {
    let distance = |e: &NativeEntity| (0..3).map(|a| (e.pos[a] - at[a]).powi(2)).sum::<f64>();
    game.native_entities_in(dimension, at.map(|v| v - 1.0), at.map(|v| v + 1.0))
        .into_iter()
        .filter(|e| e.kind == "minecraft:player")
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
}

/// Their eyes (`getEyePosition`, standing or seated) and reach (`blockInteractionRange`).
fn eye_and_reach(at: [f64; 3], game_mode: i32) -> ([f64; 3], f64) {
    (
        [at[0], at[1] + 1.62, at[2]],
        if game_mode == 1 { 5.0 } else { 4.5 },
    )
}

/// A player's message: a click on a carriage waits for the level's tick; keys go to the
/// player's controls (`ControlsInputPacket.handle`: a spectator presses nothing; the controls
/// within 16 past their reach), or let them go.
pub(super) fn net_from_client(game: &mut Game<'_>, player: u128, channel: &str, data: &[u8]) {
    if channel != tc::CHANNEL {
        return;
    }
    let Some(message) = ToServer::decode(data) else {
        return;
    };
    let Some((dim, _, at, game_mode)) = server_player(game, player) else {
        return;
    };
    match message {
        ToServer::Click { target, wrench } => {
            CLICKS.with_borrow_mut(|c| c.push((player, dim, target, wrench)));
        }
        ToServer::Input { target, stop, .. } if stop => stop_controlling(game, dim, target),
        ToServer::Input {
            target,
            keys,
            press,
            ..
        } => {
            if game_mode == 3 && press {
                return;
            }
            let (eye, reach) = eye_and_reach(at, game_mode);
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
                all[i].1.receive(player, target, keys, press);
            });
        }
    }
}

fn send_client(game: &mut Game<'_>, player: u128, m: ToClient) {
    let _ = game.native_net_send_to_player(player, tc::CHANNEL, &m.encode());
}

/// `AbstractContraptionEntity.stopControlling`: the carriage's controlling player is told
/// (`ControlsStopControllingPacket`) and it has none.
fn stop_controlling(game: &mut Game<'_>, dim: usize, at: Target) {
    let gone = CONTROLLING.with_borrow_mut(|c| {
        let i = c
            .iter()
            .position(|c| c.dim == dim && c.at.train == at.train && c.at.car == at.car)?;
        Some(c.remove(i).player)
    });
    if let Some(player) = gone {
        send_client(game, player, ToClient::Stop);
        let line = Text::translate("create.contraption.controls.stop_controlling", &[]);
        game.native_action_bar(player, line);
    }
}

/// Whether the controls' block faces away from the way the train was assembled toward
/// (`control`'s `inverted`: its facing isn't the carriage's `getInitialOrientation`
/// counterclockwise, the assembly direction, which is opposite the train's heading).
fn inverted(game: &Game<'_>, state: u32, heading: [i32; 2]) -> bool {
    let assembly = match [-heading[0], -heading[1]] {
        [1, _] => "east",
        [-1, _] => "west",
        [_, 1] => "south",
        _ => "north",
    };
    let registry = game.pack.assets.registry;
    registry
        .state(BlockStateId(state))
        .and_then(|s| s.properties.get("facing"))
        .is_none_or(|f| f != assembly)
}

/// `ControlsInteractionBehaviour.handlePlayerInteraction`: not with a wrench; whoever holds
/// the carriage's controls lets go (a second click of the same player only does that); else
/// the player takes them (`startControlling`: not a spectator, the train on the graph) and is
/// told.
fn use_controls(
    game: &mut Game<'_>,
    dim: usize,
    player: u128,
    game_mode: i32,
    at: Target,
    state: u32,
) {
    let holder = CONTROLLING.with_borrow(|c| {
        c.iter()
            .find(|c| c.dim == dim && c.at.train == at.train && c.at.car == at.car)
            .map(|c| c.player)
    });
    if let Some(holder) = holder {
        stop_controlling(game, dim, at);
        if holder == player {
            return;
        }
    }
    if game_mode == 3 {
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
    let inverted = inverted(game, state, heading);
    CONTROLLING.with_borrow_mut(|c| {
        c.push(Controlling {
            dim,
            at,
            player,
            inverted,
            driving: Driving::default(),
        })
    });
    send_client(game, player, ToClient::Start(at));
    let name = Text::translate("create.train.unnamed", &[]);
    let line = Text::translate("create.contraption.controls.start_controlling", &[name]);
    game.native_action_bar(player, line);
}

/// `AbstractContraptionEntity.handlePlayerInteraction` for a seat: a player already riding
/// passes; a player on it keeps it; a mob on it gets off where it sat; the player sits
/// (`addSittingPassenger`).
fn use_seat(game: &mut Game<'_>, dimension: &str, dim: usize, rider: &NativeEntity, at: Target) {
    if rider.vehicle.is_some() {
        return;
    }
    let taken = SEATS.with_borrow(|s| s.iter().find(|s| s.dim == dim && s.at == at).copied());
    if let Some(s) = taken {
        if s.player {
            return;
        }
        // `toDismount.stopRiding()` and `teleportTo` its passenger position.
        if let Some(f) = frame(dim, at.train, at.car) {
            game.native_move_seat(dimension, s.seat, f.seat(at.local), 0.0);
        }
        game.native_remove_seat(dimension, s.seat);
        SEATS.with_borrow_mut(|v| v.retain(|v| v.seat != s.seat));
    }
    sit(game, dimension, dim, at, rider);
}

/// `addSittingPassenger`: `rider` rides a seat at the carriage's seat block. Whether it sat.
fn sit(game: &mut Game<'_>, dimension: &str, dim: usize, at: Target, rider: &NativeEntity) -> bool {
    let Some(f) = frame(dim, at.train, at.car) else {
        return false;
    };
    let Some(seat) = game.native_spawn_seat(dimension, f.seat(at.local), 0.0) else {
        return false;
    };
    if !game.native_start_riding(dimension, rider.id, seat) {
        game.native_remove_seat(dimension, seat);
        return false;
    }
    let player = rider.kind == "minecraft:player";
    SEATS.with_borrow_mut(|s| {
        s.push(CarSeat {
            dim,
            at,
            seat,
            player,
        })
    });
    true
}

/// `ContraptionInteractionPacket.handle`: the clicking player within their reach, 10 and the
/// carriage's widest side of it (Create takes 3 off without a line of sight; not here), then
/// the carriage's block: its seat (not with a wrench) or its controls.
fn click(game: &mut Game<'_>, dimension: &str, dim: usize, player: u128, at: Target, wrench: bool) {
    let Some((_, _, pos, game_mode)) = server_player(game, player) else {
        return;
    };
    let (Some((state, size)), Some(f)) = (carriage_block(dim, at), frame(dim, at.train, at.car))
    else {
        return;
    };
    let reach = eye_and_reach(pos, game_mode).1 + 10.0 + size;
    let d2: f64 = (0..3).map(|i| (f.pivot[i] - pos[i]).powi(2)).sum();
    if d2 > reach * reach {
        return;
    }
    match block_of(state) {
        Some(b) if dye::is_seat(b) && !wrench => {
            if let Some(rider) = player_entity(game, dimension, pos) {
                use_seat(game, dimension, dim, &rider, at);
            }
        }
        Some(Block::Controls) if !wrench => use_controls(game, dim, player, game_mode, at, state),
        _ => {}
    }
}

/// What the train tells its driver, on their action bar (`TrainPromptPacket`).
fn prompt(game: &mut Game<'_>, player: u128, p: Prompt) {
    let station =
        |key: &str, name: String| Text::translate(key, &[Text::colored(name, 0x70_46_30)]);
    let line = match p {
        Prompt::None => return,
        Prompt::Arrived(name) => station("create.train.arrived_at", name),
        Prompt::Departing(name) => station("create.train.departing_from", name),
        Prompt::Approach(name) => Text::translate(
            "create.contraption.controls.approach_station",
            &[
                Text::translate("key.keyboard.space", &[]),
                Text::literal(name),
            ],
        ),
        Prompt::Progress(progress) => {
            let (mixed, rest) = tc::progress_colors(progress);
            let gone = "|".repeat(usize::from(30 - progress.min(30)));
            let to_go = "|".repeat(usize::from(progress.min(30)));
            Text::colored(gone, mixed).append(Text::colored(to_go, rest))
        }
        Prompt::Clear => Text::literal(""),
    };
    game.native_action_bar(player, line);
}

/// Before the trains move in a server level's tick: the clicks on carriages, then
/// `ControlsServerHandler.tick`: each player's keys count down; a player gone lets go; else
/// the train is driven by the keys still held (`control`), and the controls are let go when it
/// says so (a spectator, out of reach (8 past theirs), sneak held, the train gone); keys all
/// released end the word, and the train runs on by itself. Nothing without clicks or keys.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str, dim: usize) {
    if CLICKS.with_borrow(Vec::is_empty)
        && INPUTS.with_borrow(Vec::is_empty)
        && PENDING.with_borrow(Vec::is_empty)
    {
        return;
    }
    board_and_alight(game, dimension, dim);
    let clicks: Vec<(u128, usize, Target, bool)> = CLICKS.with_borrow_mut(|c| {
        let (here, rest) = std::mem::take(c).into_iter().partition(|c| c.1 == dim);
        *c = rest;
        here
    });
    for (player, _, at, wrench) in clicks {
        click(game, dimension, dim, player, at, wrench);
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
        let Some((_, _, pos, game_mode)) = server_player(game, *player).filter(|p| p.0 == dim)
        else {
            stop_controlling(game, dim, at);
            let_go.push(*player);
            continue;
        };
        let held = ctx.held();
        let (eye, reach) = eye_and_reach(pos, game_mode);
        let in_reach = frame(dim, at.train, at.car)
            .is_some_and(|f| f.within(at.local, eye, reach, tc::CONTROL_RANGE));
        // Its facing and what `control` kept: the holder's, else read again (Create drives by
        // keys that outlive the hold, from the block's state each tick).
        let kept = CONTROLLING.with_borrow(|c| {
            c.iter()
                .find(|c| c.dim == dim && c.at == at)
                .map(|c| (c.inverted, c.driving))
        });
        let (inverted, mut state) = match kept {
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
                    .is_some_and(|((state, _), h)| inverted(game, state, h));
                (facing, Driving::default())
            }
        };
        let result = if game_mode == 3 || !in_reach || held & (1 << tc::SNEAK) != 0 {
            None
        } else {
            with_world(|w| {
                w.dims[dim]
                    .railway
                    .control(at.train, held, inverted, &mut state)
            })
            .flatten()
        };
        CONTROLLING.with_borrow_mut(|c| {
            if let Some(c) = c.iter_mut().find(|c| c.dim == dim && c.at == at) {
                c.driving = state;
            }
        });
        match result {
            Some(p) => prompt(game, *player, p),
            None => stop_controlling(game, dim, at),
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

/// After the trains move in a server level's tick: each carriage seat in use follows its
/// carriage (`positionRider`; clients glide it); a seat whose rider got off goes, and so do
/// the seats of a train that is gone, letting their riders off. Nothing without a seat in use.
pub(super) fn tick_seats(game: &mut Game<'_>, dimension: &str, dim: usize) {
    if SEATS.with_borrow(Vec::is_empty) {
        return;
    }
    let seats: Vec<CarSeat> =
        SEATS.with_borrow(|s| s.iter().filter(|s| s.dim == dim).copied().collect());
    for s in seats {
        let f = frame(dim, s.at.train, s.at.car);
        if let Some(f) = f
            && !game.native_passengers(dimension, s.seat).is_empty()
        {
            game.native_move_seat(dimension, s.seat, f.seat(s.at.local), 0.0);
            continue;
        }
        game.native_remove_seat(dimension, s.seat);
        SEATS.with_borrow_mut(|v| v.retain(|v| v.seat != s.seat));
    }
}

/// NeoForge's `EntityMountEvent` for a player sneaking off a carriage seat: the seat moves to
/// where Create lets them off (`ContraptionDismountLocation`) before the game puts them where
/// it is; it goes at the level's next tick, riderless.
pub(super) fn entity_mount(game: &mut Game<'_>, dimension: &str, vehicle: u32, mounting: bool) {
    if mounting || SEATS.with_borrow(Vec::is_empty) {
        return;
    }
    let Some(dim) = dimension_index(dimension) else {
        return;
    };
    let at = SEATS.with_borrow(|s| {
        s.iter()
            .find(|s| s.dim == dim && s.seat == vehicle)
            .map(|s| s.at)
    });
    if let Some(at) = at
        && let Some(f) = frame(dim, at.train, at.car)
    {
        game.native_move_seat(dimension, vehicle, f.dismount(at.local), 0.0);
    }
}

/// `Contraption.moveSeat` and `onEntityInitialize`: train `train` was assembled; whoever sat on
/// the seat blocks it took (`seats`: carriage, the block in it, where it stood) sits on its
/// seats at the level's next tick (the assembling player is out of the helpers' reach now).
pub(super) fn assembled(dim: usize, train: u32, seats: &[(u16, [i32; 3], [i32; 3])]) {
    PENDING.with_borrow_mut(|p| {
        for &(car, local, from) in seats {
            let at = Target { train, car, local };
            p.push(Pending::Board { dim, from, at });
        }
    });
}

/// `ControlsMovementBehaviour.stopMoving` and `Contraption.addPassengersToWorld`: train `train`
/// was taken apart; its controls are let go and, at the level's next tick, each carriage
/// seat's rider sits on the seat block put back in the world (`seats`: carriage, the block in
/// it, where it went), if it is free.
pub(super) fn disassembled(
    game: &mut Game<'_>,
    dim: usize,
    train: u32,
    seats: &[(u16, [i32; 3], [i32; 3])],
) {
    let held: Vec<Target> = CONTROLLING.with_borrow(|c| {
        c.iter()
            .filter(|c| c.dim == dim && c.at.train == train)
            .map(|c| c.at)
            .collect()
    });
    for at in held {
        stop_controlling(game, dim, at);
    }
    let ours: Vec<CarSeat> = SEATS.with_borrow_mut(|s| {
        let (here, rest) = std::mem::take(s)
            .into_iter()
            .partition(|s| s.dim == dim && s.at.train == train);
        *s = rest;
        here
    });
    PENDING.with_borrow_mut(|p| {
        for s in ours {
            let to = seats
                .iter()
                .find(|(c, local, _)| *c == s.at.car && *local == s.at.local)
                .map(|x| x.2);
            p.push(Pending::Alight {
                dim,
                seat: s.seat,
                to,
            });
        }
    });
}

/// The riders waiting to board a new train's seats or to leave a train taken apart.
fn board_and_alight(game: &mut Game<'_>, dimension: &str, dim: usize) {
    let pending: Vec<Pending> = PENDING.with_borrow_mut(|p| {
        let (here, rest) = std::mem::take(p).into_iter().partition(|p| match p {
            Pending::Board { dim: d, .. } | Pending::Alight { dim: d, .. } => *d == dim,
        });
        *p = rest;
        here
    });
    for p in pending {
        match p {
            Pending::Board { from, at, .. } => {
                if let Some(rider) = super::create_seats::take_rider(game, dimension, dim, from) {
                    sit(game, dimension, dim, at, &rider);
                }
            }
            Pending::Alight { seat, to, .. } => {
                let riders = game.native_passengers(dimension, seat);
                game.native_remove_seat(dimension, seat);
                let rider = riders
                    .first()
                    .and_then(|&r| game.native_entity(dimension, r));
                if let (Some(to), Some(rider)) = (to, rider) {
                    super::create_seats::seat_rider(game, dimension, dim, to, &rider);
                }
            }
        }
    }
}
