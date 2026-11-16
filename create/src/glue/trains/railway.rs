//! The railway's tick and the trains' save: trains move over the graph (`Train.tick`,
//! `Navigation.tick`, `ScheduleRuntime.tick`), signals follow them (`SignalBoundary.tickState`),
//! track observers light (`TrackObserverBlockEntity.tick`), and clients hear of each train as
//! it starts, stops, arrives, runs a curve or passes onto another edge. A track observer's
//! cargo filter is set on its filter slot (`Observer`); it is saved with the contraptions'
//! `create_glue`.

use super::super::net;
use super::super::server::{ServerLevel, update_all};
use super::super::subsystem::Subsystem;
use super::super::world::with_world;
use super::super::{kinetics, persist, states};
use super::{assembly, riding, saved, tracks, visuals};
use create_engine::Block;
use create_engine::trains::{CURVE_UPDATE_TICKS, Event, Run};
use mod_api::{
    BlockClick, BlockPart, BlockSignalEvent, ClientHost, Compound, Flow, Host, InteractionResult,
    IsSignalSourceEvent, LevelHost, PlayerRef, Registrar, RegistrationContext,
    ScreenCharTypedPreEvent, ScreenClosingEvent, ScreenKeyPressedPreEvent,
    ScreenMouseButtonPressedPreEvent, ScreenMouseScrolledPreEvent, ScreenRenderPostEvent,
    ServerHost, UseItemOnEvent, UseWithoutItemEvent,
};
use std::cell::RefCell;

/// Create's channel for a train's motion (`create_engine::trains::Motion`).
pub const TRAIN_CHANNEL: &str = create_engine::trains::CHANNEL;

/// The states a train's blocks name that are not Create's, before the train's blocks reach a
/// client: each id, then its state as `/setblock` writes it.
pub const STATES_CHANNEL: &str = "create:train_states";

pub struct Trains;

thread_local! {
    /// Scratch for a tick's train events, kept to not allocate.
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
}

impl Subsystem for Trains {
    fn register(&self, r: &mut dyn Registrar, _cx: &RegistrationContext) {
        tracks::register(r);
        super::schedule_screen::register(r);
        super::conductor::register(r);
        r.block_part(states::key(Block::TrackStation), assembly::Station);
        r.block_part(states::key(Block::Track), assembly::TrackClick);
        r.block_part(states::key(Block::TrackObserver), Observer);
        let Some(kind) = kinetics::kind() else {
            return;
        };
        if let Err(e) = r.block_entity_part(&kind, tracks::TrackEntity) {
            crate::create_log!("[create] the track block entity's part was refused: {e:?}");
        }
    }

    fn world_closed(&self) {
        tracks::forget();
        super::cargo::forget();
        assembly::forget();
        super::conductor::forget();
        super::relocate::forget();
        super::station_screen::forget();
        super::station_draw::forget();
        super::carriage_draw::forget();
        visuals::forget();
        super::actors::forget();
        riding::forget();
    }

    /// A rider getting off a carriage's seat is let off where Create lets them off.
    fn entity_mount(
        &self,
        host: &mut dyn ServerHost,
        event: &mut mod_api::EntityMountEvent<'_>,
    ) -> mod_api::Flow {
        if !event.mounting && event.vehicle_kind == "minecraft:item_display" {
            riding::dismounted(host, event.vehicle);
        }
        mod_api::Flow::Continue
    }

    fn chunk_load(&self, lv: &mut ServerLevel<'_>, chunk: [i32; 2], data: &Compound) {
        let mut trains = saved::trains_from_nbt(&*lv.host, data);
        let riders = saved::riders_from_nbt(data);
        let cargo = saved::cargo_from_nbt(data);
        // Each is the column's it was saved with, until it moves: a train taken apart where it
        // stood saves that column again, and unloads with it.
        for t in &mut trains {
            t.column = (chunk[0], chunk[1]);
        }
        let names = saved::stations_from_nbt(data);
        let curves = saved::curves_from_nbt(data);
        if trains.is_empty() && names.is_empty() && curves.is_empty() {
            return;
        }
        let dim = lv.dim;
        let ids = with_world(|w| {
            let ids = w.load_trains(dim, trains);
            for (id, r) in ids.iter().zip(riders) {
                riding::riders_loaded(dim, *id, r);
            }
            w.dims[dim].railway.load_stations(names);
            for c in curves {
                w.dims[dim].track_curves.add(c);
            }
            ids
        })
        .unwrap_or_default();
        for (id, c) in ids.into_iter().zip(cargo) {
            super::cargo::from_nbt(lv, id, c.as_ref());
        }
    }

    fn chunk_data_save(&self, lv: &ServerLevel<'_>, chunk: [i32; 2], data: &mut Compound) {
        let dim = lv.dim;
        let Some((trains, names, curves)) = with_world(|w| {
            (
                w.column_trains(dim, chunk[0], chunk[1]),
                w.dims[dim].railway.column_stations(chunk[0], chunk[1]),
                w.dims[dim].track_curves.column(chunk[0], chunk[1]),
            )
        }) else {
            return;
        };
        if !trains.is_empty() {
            data.insert(
                "trains".into(),
                saved::trains_to_nbt(&*lv.host, &trains, &|id| riding::riders_of(dim, id), &|t| {
                    super::cargo::to_nbt(lv, t.id, t.cars.len())
                }),
            );
        }
        if !names.is_empty() {
            data.insert("station_names".into(), saved::stations_to_nbt(&names));
        }
        if !curves.is_empty() {
            data.insert("track_curves".into(), saved::curves_to_nbt(&curves));
        }
    }

    /// `Create.RAILWAYS.tick` runs from `LevelTickEvent.Post`, frozen or not.
    fn runs_frozen(&self) -> bool {
        true
    }

    fn level_tick(&self, lv: &mut ServerLevel<'_>) {
        // The controls drive before the trains move; their HUD hears of the move after.
        super::controls::before_trains(lv);
        tick_trains(lv);
        super::actors::tick(lv);
        super::cargo::tick_interfaces(lv);
        super::carriage_draw::tick(lv);
        if lv.game_time() % 4 == 0 {
            super::doors::tick(lv);
        }
        if lv.game_time() % 100 == 0 {
            super::cargo::prune(lv);
        }
        if lv.game_time() % 10 == 0 {
            super::station_draw::refresh(lv);
            super::station_draw::refresh_marks(lv);
        }
        visuals::tick(lv);
        riding::tick(lv);
        super::controls::after_trains(lv);
        tracks::sync(lv);
        super::conductor::tick(lv);
        super::station_screen::tick(lv);
        super::relocate::preview(lv);
    }

    fn entity_interact(
        &self,
        host: mod_api::HostSide,
        event: &mut mod_api::EntityInteractEvent<'_>,
    ) -> Option<InteractionResult> {
        let mod_api::HostSide::Server(host) = host else {
            return None;
        };
        super::conductor::interact(host, event)
    }

    fn client_message(
        &self,
        host: &mut dyn ServerHost,
        player: PlayerRef,
        channel: &str,
        data: &[u8],
    ) -> bool {
        match channel {
            super::station_screen::CHANNEL => {
                super::station_screen::client_message(host, player, data);
            }
            super::schedule_screen::CHANNEL => {
                super::schedule_screen::client_message(host, player, data);
            }
            super::relocate::CHANNEL => super::relocate::client_message(player, data),
            _ => return false,
        }
        true
    }

    fn server_message(&self, host: &mut dyn ClientHost, channel: &str, data: &[u8]) -> bool {
        match channel {
            TRAIN_CHANNEL => super::client::trains_from_server(&*host, data),
            super::carriage_draw::CHANNEL => super::carriage_draw::server_message(data),
            STATES_CHANNEL => super::client::states_from_server(&*host, data),
            super::station_screen::CHANNEL => super::station_screen::server_message(host, data),
            super::schedule_screen::CHANNEL => super::schedule_screen::server_message(host, data),
            super::relocate::CHANNEL => super::relocate::server_message(data),
            _ => return false,
        }
        true
    }

    fn client_tick(&self, host: &mut dyn ClientHost) {
        super::station_screen::tick_client(host);
        super::schedule_screen::tick_client(host);
        super::relocate::client_tick(host);
    }

    fn input_handled(&self, host: &mut dyn ClientHost) {
        super::station_screen::tick_client(host);
        super::schedule_screen::flush(host);
    }

    /// A station's flag, marker and assembly track, a track's curves, and a signal's light and
    /// state (its indicator, and a nixie tube on it), for its clients.
    fn synced_keys(&self) -> &'static [&'static str] {
        &[
            super::station_draw::KEY,
            super::station_draw::MARK,
            super::tracks::CURVES,
            super::tracks::LIGHT,
            super::tracks::SIGNAL_STATE,
        ]
    }

    fn standalone_models(&self, models: &mut Vec<String>) {
        models.extend(super::station_draw::MODELS.iter().map(|m| (*m).to_owned()));
        models.extend(super::carriage_draw::MODELS.iter().map(|m| (*m).to_owned()));
    }

    fn models_baked(&self, host: &mut dyn ClientHost) {
        super::carriage_draw::models_baked(&*host);
    }

    fn render_level_stage(
        &self,
        host: &dyn ClientHost,
        event: &mut mod_api::RenderLevelStageEvent<'_>,
    ) {
        super::carriage_draw::render_level_stage(host, event);
        super::relocate::render_level_stage(event);
    }

    /// A station's flag moving and an invalid signal's blink (`SignalRenderer`) are drawn every
    /// frame.
    fn animated(
        &self,
        block: create_engine::Block,
        pos: [i32; 3],
        entity: &mod_api::ModBlockEntity,
    ) -> bool {
        (block == create_engine::Block::TrackStation
            && super::station_draw::animating(pos, &entity.nbt))
            || (block == create_engine::Block::TrackSignal && super::tracks::blinking(&entity.nbt))
    }

    fn client_reset(&self) {
        super::station_draw::client_reset();
        super::relocate::client_reset();
        super::carriage_draw::reset();
        super::client::reset();
        super::station_screen::reset();
        super::schedule_screen::reset();
    }

    fn screen_render_post(&self, host: &dyn ClientHost, event: &mut ScreenRenderPostEvent<'_>) {
        super::station_screen::screen_render_post(host, event);
        super::schedule_screen::screen_render_post(host, event);
    }

    fn screen_mouse_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        if super::station_screen::screen_mouse_pressed(host, event) == Flow::Cancel {
            return Flow::Cancel;
        }
        super::schedule_screen::screen_mouse_pressed(host, event)
    }

    fn screen_mouse_scrolled(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenMouseScrolledPreEvent<'_>,
    ) -> Flow {
        if super::station_screen::screen_mouse_scrolled(host, event) == Flow::Cancel {
            return Flow::Cancel;
        }
        super::schedule_screen::screen_mouse_scrolled(host, event)
    }

    fn screen_key_pressed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenKeyPressedPreEvent<'_>,
    ) -> Flow {
        if super::station_screen::screen_key_pressed(host, event) == Flow::Cancel {
            return Flow::Cancel;
        }
        super::schedule_screen::screen_key_pressed(host, event)
    }

    fn screen_char_typed(
        &self,
        host: &dyn ClientHost,
        event: &mut ScreenCharTypedPreEvent<'_>,
    ) -> Flow {
        if super::station_screen::screen_char_typed(host, event) == Flow::Cancel {
            return Flow::Cancel;
        }
        super::schedule_screen::screen_char_typed(host, event)
    }

    fn screen_closing(&self, host: &dyn ClientHost, event: &mut ScreenClosingEvent<'_>) {
        super::station_screen::screen_closing(host, event);
        super::schedule_screen::screen_closing(host, event);
    }

    fn player_joined(&self, host: &mut dyn ServerHost, player: PlayerRef) {
        let Some(dimension) = player.level(&*host).and_then(|l| l.dimension().id(&*host)) else {
            return;
        };
        let Some(lv) = ServerLevel::new(host, &dimension) else {
            return;
        };
        let dim = lv.dim;
        let ids: Vec<u32> =
            with_world(|w| w.dims[dim].railway.trains.iter().map(|t| t.id).collect())
                .unwrap_or_default();
        let mut lv = lv;
        send_trains_to(&mut lv, &[player], &ids, true);
    }
}

/// Sends trains `ids` to the players in `lv` as they are (with their blocks when `full`, for
/// clients that don't know them yet).
pub fn send_trains(lv: &mut ServerLevel<'_>, ids: &[u32], full: bool) {
    let players = lv.players();
    send_trains_to(lv, &players, ids, full);
}

fn send_trains_to(lv: &mut ServerLevel<'_>, players: &[PlayerRef], ids: &[u32], full: bool) {
    if players.is_empty() || ids.is_empty() {
        return;
    }
    let tick = lv.game_time().max(0) as u64;
    let dim = lv.dim;
    let motions = with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let mut out = Vec::new();
        for t in r.trains.iter_mut().filter(|t| ids.contains(&t.id)) {
            if !t.train.bound {
                continue;
            }
            let full = full || t.fresh;
            t.fresh = false;
            t.sent_tick = tick;
            t.sent_edge = t.train.points.first().map_or(0, |p| p.edge);
            out.push(t.motion(&r.net, CURVE_UPDATE_TICKS, full));
        }
        out
    })
    .unwrap_or_default();
    for m in &motions {
        if let Some(cars) = &m.blocks {
            let names = state_names(&*lv.host, cars);
            if !names.is_empty() {
                net::send_to_players(lv.host, players, STATES_CHANNEL, &names);
            }
        }
        net::send_to_players(lv.host, players, TRAIN_CHANNEL, &m.encode());
    }
}

/// The states of `cars` that are not Create's, for `STATES_CHANNEL`.
fn state_names(host: &dyn Host, cars: &[Vec<([i32; 3], u32)>]) -> Vec<u8> {
    let mut seen: Vec<u32> = cars
        .iter()
        .flatten()
        .map(|(_, s)| *s)
        .filter(|&s| s < states::FIRST)
        .collect();
    seen.sort_unstable();
    seen.dedup();
    let mut out = Vec::new();
    for id in seen {
        let Some(text) = super::car_text(host, id) else {
            continue;
        };
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&(text.len() as u16).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
    }
    out
}

/// `STATES_CHANNEL`'s entries.
pub fn decode_state_names(mut d: &[u8]) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    while d.len() >= 6 {
        let id = u32::from_le_bytes([d[0], d[1], d[2], d[3]]);
        let n = usize::from(u16::from_le_bytes([d[4], d[5]]));
        let Some(text) = d.get(6..6 + n) else {
            break;
        };
        out.push((id, String::from_utf8_lossy(text).into_owned()));
        d = &d[6 + n..];
    }
    out
}

/// Clients forget train `id`.
pub fn send_train_clear(lv: &mut ServerLevel<'_>, id: u32) {
    let players = lv.players();
    net::send_to_players(lv.host, &players, TRAIN_CHANNEL, &id.to_le_bytes());
}

/// The graph as the world has it now (built again only if track changed).
pub fn railway_ready(dim: usize) {
    with_world(|w| {
        let d = &mut w.dims[dim];
        d.railway.ensure(&d.track_curves);
    });
}

/// The trains and signals: nothing without trains unless track or a signal changed; the graph
/// is built again first if track changed. Clients are told of a train when it starts, stops or
/// arrives, every few ticks on a curve, as it passes onto another edge, and every ten ticks
/// while its speed changes; trains found on the graph again (loaded) are sent whole; signal
/// lights only when one turns red or back.
fn tick_trains(lv: &mut ServerLevel<'_>) {
    let dim = lv.dim;
    if with_world(|w| {
        let d = &w.dims[dim];
        d.railway.idle(&d.track_curves)
    }) != Some(false)
    {
        return;
    }
    let tick = lv.game_time().max(0) as u64;
    // What the schedules' conditions read: the day time, the stations trains stand at with a
    // redstone signal, each train's seated players.
    let day_time = lv.level.day_time(&*lv.host).unwrap_or(0);
    let standing: Vec<[i32; 3]> = with_world(|w| {
        w.dims[dim]
            .railway
            .trains
            .iter()
            .filter(|t| t.schedule.is_some())
            .filter_map(|t| t.train.at_station)
            .collect()
    })
    .unwrap_or_default();
    let powered: std::collections::HashSet<[i32; 3]> = standing
        .into_iter()
        .filter(|p| lv.has_neighbor_signal(*p))
        .collect();
    let seated = riding::passengers(dim);
    let probes = probe_schedules(lv);
    with_world(|w| {
        let r = &mut w.dims[dim].railway;
        r.day_time = day_time;
        r.powered = powered;
        for t in &mut r.trains {
            t.players = seated.get(&t.id).copied().unwrap_or(0);
        }
        for (id, p) in probes {
            if let Some(t) = r.trains.iter_mut().find(|t| t.id == id) {
                t.probes = p;
            }
        }
    });
    let mut events = EVENTS.take();
    let mut observed = Vec::new();
    let mut crashes = Vec::new();
    let (due, fresh, lights) = with_world(|w| {
        let d = &mut w.dims[dim];
        d.railway.ensure(&d.track_curves);
        // The stations a train stands at, before and after: a station's comparators read
        // `trainPresent`, a signal's whether it is red.
        let at = |r: &create_engine::trains::Railway| -> Vec<[i32; 3]> {
            r.trains.iter().filter_map(|t| t.train.at_station).collect()
        };
        let before = at(&d.railway);
        d.railway.tick(&mut events);
        let after = at(&d.railway);
        for p in before.iter().filter(|p| !after.contains(p)) {
            d.analog_checks.push(*p);
        }
        for p in after.iter().filter(|p| !before.contains(p)) {
            d.analog_checks.push(*p);
        }
        crashes = std::mem::take(&mut d.railway.crashes);
        let lights = std::mem::take(&mut d.railway.lights_changed);
        d.analog_checks
            .extend(lights.iter().filter(|l| l.1.is_some()).map(|l| l.0));
        observed.append(&mut d.railway.observer_changes);
        let r = &d.railway;
        let due: Vec<u32> = events
            .iter()
            .filter(|e| {
                r.trains
                    .iter()
                    .find(|t| t.id == e.id)
                    .is_some_and(|t| t.due(&r.net, tick, e))
            })
            .map(|e| e.id)
            .collect();
        let fresh: Vec<u32> = r
            .trains
            .iter()
            .filter(|t| t.fresh && t.train.bound)
            .map(|t| t.id)
            .collect();
        (due, fresh, lights)
    })
    .unwrap_or_default();
    // Saved with the column it stands in: the one it left and the one it is in save again.
    for e in &events {
        if e.was != e.now || e.run == Run::Arrived {
            persist::mark_unsaved(dim, [e.was.0, e.was.1]);
            persist::mark_unsaved(dim, [e.now.0, e.now.1]);
        }
    }
    events.clear();
    EVENTS.set(events);
    // `TrackObserverBlockEntity.tick`: the block's `powered` follows its observer
    // (`setBlock(..., UPDATE_ALL)`).
    for (pos, lit) in observed {
        if observer_powered(lv.block(pos)).is_some_and(|p| p != lit) {
            let value = if lit { "true" } else { "false" };
            lv.set_property(pos, "powered", value, update_all());
        }
    }
    if !fresh.is_empty() {
        send_trains(lv, &fresh, true);
    }
    if !due.is_empty() {
        send_trains(lv, &due, false);
    }
    tracks::lights_changed(lv, &lights);
    for c in crashes {
        crashed(lv, &c);
    }
}

/// `Train.collideWithOtherTrains` and `crash` after two trains met: a blast that breaks nothing
/// where they met when they came together faster than 0.2 blocks a tick (`explode`, `NONE`,
/// `min(3 × speed, 5)`); each train's seated players but its drivers get `TRAIN_CRASH`, a
/// driver driving backwards `TRAIN_CRASH_BACKWARDS`. Their clients see them stop.
fn crashed(lv: &mut ServerLevel<'_>, c: &create_engine::trains::Crash) {
    if c.combined_speed > 0.2 {
        let radius = (3.0 * c.combined_speed).min(5.0) as f32;
        let _ = lv.level.explode(
            &mut *lv.host,
            None,
            c.at,
            radius,
            false,
            mod_api::ExplosionInteraction::None,
            mod_api::ExplosionSettings::default(),
        );
    }
    for (k, &train) in c.trains.iter().enumerate() {
        let drivers = super::controls::drivers(lv.dim, train);
        for (_, _, rider) in riding::riders_of(lv.dim, train) {
            if drivers.contains(&rider) {
                continue;
            }
            if let Some(p) = lv.host.player(rider) {
                crate::glue::advancements::award(&mut *lv.host, p, "train_crash");
            }
        }
        if c.backwards[k] {
            for d in drivers {
                if let Some(p) = lv.host.player(d) {
                    crate::glue::advancements::award(&mut *lv.host, p, "train_crash_backwards");
                }
            }
        }
    }
    send_trains(lv, &c.trains, false);
}

/// What the schedules' conditions about to be looked at read of the world (`Probes`): the
/// items or fluid the train carries matching a threshold's item, whether a link network is
/// powered (`hasAnyLoadedPower`), whether the station's chunk is unloaded; and the stations a
/// package instruction may go to (none: no postbox is linked to a station here).
fn probe_schedules(lv: &mut ServerLevel<'_>) -> Vec<(u32, create_engine::schedule::Probes)> {
    use create_engine::schedule::{Condition, Probes};
    let dim = lv.dim;
    // Each train's wanted measures: its id, carriages, station, the conditions and whether it
    // carries packages wanted.
    type Wanted = (
        u32,
        usize,
        Option<[i32; 3]>,
        Vec<((usize, usize), Condition)>,
        Option<create_engine::schedule::Instruction>,
    );
    let wanted: Vec<Wanted> = with_world(|w| {
        w.dims[dim]
            .railway
            .trains
            .iter()
            .filter_map(|t| {
                let s = t.schedule.as_ref()?;
                let at = t.runtime.probes_wanted(s);
                let packages = t.runtime.packages_wanted(s);
                if at.is_empty() && !packages {
                    return None;
                }
                let entry = s.entries.get(t.runtime.current)?;
                let conditions = at
                    .into_iter()
                    .filter_map(|(g, r)| Some(((g, r), entry.conditions.get(g)?.get(r)?.clone())))
                    .collect();
                let packages = packages.then(|| entry.instruction.clone());
                Some((t.id, t.cars.len(), t.train.at_station, conditions, packages))
            })
            .collect()
    })
    .unwrap_or_default();
    let mut out = Vec::new();
    for (id, cars, station, conditions, packages) in wanted {
        let mut p = Probes::default();
        let needs_items = packages.is_some()
            || conditions
                .iter()
                .any(|(_, c)| matches!(c, Condition::ItemThreshold { .. }));
        let stacks = if needs_items {
            super::cargo::stacks(dim, id, cars)
        } else {
            Vec::new()
        };
        for (at, c) in conditions {
            let v = match &c {
                Condition::ItemThreshold {
                    stacks: full, item, ..
                } => {
                    let filter = (!item.is_empty())
                        .then(|| crate::glue::items::filter_screen::stack_of(&*lv.host, item))
                        .flatten();
                    stacks
                        .iter()
                        .filter(|s| filter.as_ref().is_none_or(|f| matches_item(lv, f, s)))
                        .map(|s| {
                            if *full {
                                let max = lv.host.max_stack_size(s.item()).unwrap_or(64);
                                i32::from(s.count() == max)
                            } else {
                                s.count() as i32
                            }
                        })
                        .sum()
                }
                // Carriages carry no fluid here (tanks keep only their block when moved).
                Condition::FluidThreshold { .. } => 0,
                Condition::RedstoneLink { freq, .. } => {
                    let name = |b: &Vec<u8>| {
                        if b.is_empty() {
                            return String::new();
                        }
                        let s = crate::glue::items::filter_screen::stack_of(&*lv.host, b);
                        crate::glue::redstone::frequency_key(&*lv.host, s.as_ref())
                    };
                    let key = (name(&freq[0]), name(&freq[1]));
                    with_world(|w| w.dims[dim].links.any_power(&key)).map_or(0, i32::from)
                }
                Condition::Unloaded => station.map_or(0, |s| {
                    i32::from(!lv.level.entity_ticking(&*lv.host, s).unwrap_or(true))
                }),
                _ => 0,
            };
            p.measures.push((at, v));
        }
        if let Some(instruction) = packages {
            // The stations whose postboxes (`GlobalPackagePort`) take the packages aboard, or
            // hold packages to fetch; whether packages are aboard decides what a delivery with
            // none does.
            let any = stacks.iter().any(|s| {
                create_engine::package::is_package(&crate::glue::packages::body::item_id(
                    &*lv.host, s,
                ))
            });
            let fetch = match &instruction {
                create_engine::schedule::Instruction::FetchPackages(f) => Some(f.as_str()),
                _ => None,
            };
            let stations = crate::glue::packages::package_stations(lv, fetch, &stacks);
            p.packages = Some((stations, any));
        }
        out.push((id, p));
    }
    out
}

/// `FilterItemStack.test` for a threshold's item: a filter tests as filters do, any other
/// item matches itself.
fn matches_item(lv: &ServerLevel<'_>, filter: &mod_api::ItemStack, s: &mod_api::ItemStack) -> bool {
    let h: &dyn mod_api::Host = &*lv.host;
    let f = crate::glue::items::filters::filter_of(h, filter);
    crate::glue::items::filters::passes(h, Some(&f), s)
}

/// A track observer's `powered`, None for any other block.
fn observer_powered(state: u32) -> Option<bool> {
    with_world(|w| {
        let (b, info) = w.table.get(state)?;
        (b == Block::TrackObserver).then_some(info.powered)
    })
    .flatten()
}

/// A track observer's part: the click on its filter slot.
struct Observer;

#[mod_api::horizon_part]
impl BlockPart for Observer {
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        use_observer(host, event.click)
    }

    fn use_without_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseWithoutItemEvent<'_>,
    ) -> Option<InteractionResult> {
        use_observer(host, event.click)
    }

    /// `TrackObserverBlock.isSignalSource`: always.
    fn is_signal_source(&self, _level: &dyn LevelHost, event: &mut IsSignalSourceEvent) {
        event.source = true;
    }

    /// `TrackObserverBlock.getSignal`: 15 while powered, to every side (no direct signal).
    fn block_signal(&self, level: &dyn LevelHost, event: &mut BlockSignalEvent) {
        let powered = level
            .state_string(event.state)
            .is_some_and(|s| s.contains("powered=true"));
        if !event.direct && powered {
            event.signal = 15;
        }
    }
}

/// A click on a track observer: on its filter slot
/// (`TrackObserverBlockEntity.createFilter`: on top, 15.5 px up; `FilteringBehaviour
/// .onShortInteract` through `ValueSettingsInputHandler`) the held stack becomes its cargo filter
/// (a filter item taken from the hand, the one it held given back), an empty hand clears it
/// (`onFilterChanged` → `setFilterAndNotify`). Trains carry no cargo in this port: a filtered
/// observer sees none.
fn use_observer(host: &mut dyn ServerHost, click: &BlockClick<'_>) -> Option<InteractionResult> {
    use crate::glue::items::filters;
    if click.hit.face != [0, 1, 0] {
        return None;
    }
    let player = host.player(click.player)?;
    if !crate::glue::interaction::values::may_use_value_box(&*host, player) {
        return None;
    }
    let pos = click.pos;
    let local = [0, 1, 2].map(|i| click.hit.point[i] - f64::from(pos[i]));
    if !create_engine::board::hits_box([0.5, 15.5 / 16.0, 0.5], local) {
        return None;
    }
    // `canShortInteract`: a wrench or a mechanical arm is used on the block instead.
    let held = click.stack.map(|s| s.item().id(&*host));
    if held
        .as_deref()
        .is_some_and(|h| matches!(h, "create:wrench" | "create:mechanical_arm"))
    {
        return None;
    }
    let dimension = click.dimension.key().id().to_owned();
    let mut lv = ServerLevel::new(host, &dimension)?;
    let filter = filters::take_filter(lv.host, player, click.hand);
    let dim = lv.dim;
    let old = with_world(|w| {
        let d = &mut w.dims[dim];
        d.railway.set_observer_filtered(pos, filter.is_some());
        match filter {
            Some(f) => d.observer_filters.insert(pos, f),
            None => d.observer_filters.remove(&pos),
        }
    })
    .flatten();
    persist::mark_pos_unsaved(dim, pos);
    filters::return_filter(&mut lv, player, pos, old);
    Some(InteractionResult::Success)
}

#[cfg(test)]
mod tests {
    use super::decode_state_names;

    #[test]
    fn state_names_read_back() {
        let mut d = Vec::new();
        for (id, text) in [(3u32, "minecraft:oak_planks"), (7, "minecraft:stone")] {
            d.extend_from_slice(&id.to_le_bytes());
            d.extend_from_slice(&(text.len() as u16).to_le_bytes());
            d.extend_from_slice(text.as_bytes());
        }
        assert_eq!(
            decode_state_names(&d),
            vec![
                (3, "minecraft:oak_planks".to_owned()),
                (7, "minecraft:stone".to_owned())
            ]
        );
    }
}
