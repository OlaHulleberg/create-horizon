//! A station (`StationBlock`, `StationBlockEntity`): a click opens its screen
//! (`station_screen`), from which it enters assembly mode (`enterAssemblyMode`, taking apart a
//! train standing at it), assembles the bogeys along its track into a train (`assemble`) and
//! leaves assembly mode (`exitAssemblyMode`). In assembly mode, a click on its track places
//! bogeys (`trackClicked`).

use super::super::server::{ServerLevel, update_all};
use super::super::states;
use super::super::world::with_world;
use super::railway::{railway_ready, send_train_clear, send_trains};
use super::structure::{found_carrying, turned_state};
use create_engine::Block;
use create_engine::kinetics::{Axis, Dir};
use create_engine::schedule::Runtime;
use create_engine::track::TrackShape;
use create_engine::trains::{
    AssemblyError, BogeySize, MAX_ASSEMBLY_LENGTH, MAX_BOGEY_COUNT, Structure, Train, TrainOnTrack,
    layout,
};
use mod_api::{
    BlockClick, BlockPart, HostBlockStates, InteractionResult, PlayerRef, ServerHost, Text,
    UseItemOnEvent, UseWithoutItemEvent,
};
use std::cell::RefCell;
use std::collections::HashMap;

/// The station block's part: its click.
pub struct Station;

/// The off hand's turn after an empty main hand: Create's station and track answer the main
/// hand on the client too, so the off hand is never tried; here the client can't know.
fn after_empty_main(host: &dyn ServerHost, player: PlayerRef, click: &BlockClick<'_>) -> bool {
    click.hand == mod_api::Hand::Off
        && crate::glue::items::held(host, player, mod_api::Hand::Main)
            .is_none_or(|s| s.count() == 0)
}

/// `StationBlock.useItemOn`: sneaking, or with a wrench or a filled map in hand, the click is
/// not the station's; else its screen opens.
fn click_station(host: &mut dyn ServerHost, click: &BlockClick<'_>) -> Option<InteractionResult> {
    let player = host.player(click.player)?;
    if after_empty_main(&*host, player, click) {
        return None;
    }
    if crate::glue::items::sneaking(&*host, player) {
        return None;
    }
    if let Some(stack) = click.stack {
        let id = stack.item().id(&*host);
        if id == "create:wrench" || id == "minecraft:filled_map" {
            return None;
        }
        // `PackagePortTargetSelectionHandler.onUse`: a postbox chooses the station (its client
        // drew the choice), nothing opens.
        if id.starts_with("create:") && id.ends_with("_postbox") {
            let dimension = click.dimension.key().id().to_owned();
            let dim = crate::glue::world::level_index(&dimension)?;
            crate::glue::packages::choose_station(player, dim, click.pos);
            return Some(InteractionResult::SuccessServer);
        }
    }
    let dimension = click.dimension.key().id().to_owned();
    let mut lv = ServerLevel::new(host, &dimension)?;
    // A schedule the station holds comes back to the player first.
    let dim = lv.dim;
    let held = with_world(|w| w.dims[dim].railway.auto_schedules.remove(&click.pos)).flatten();
    if let Some(s) = held {
        super::super::persist::mark_pos_unsaved(dim, click.pos);
        super::conductor::give_schedule(lv.host, player, &s);
        return Some(InteractionResult::SuccessServer);
    }
    super::station_screen::open(&mut lv, player, click.pos);
    Some(InteractionResult::SuccessServer)
}

#[mod_api::horizon_part]
impl BlockPart for Station {
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        click_station(host, event.click)
    }

    fn use_without_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseWithoutItemEvent<'_>,
    ) -> Option<InteractionResult> {
        click_station(host, event.click)
    }

    /// `SharedDepotBlockMethods.onLanded` with the station's `DepotBehaviour`, which takes only
    /// schedules: a schedule landing on a station with none becomes its auto-schedule.
    fn item_after_vertical_collision(
        &self,
        host: mod_api::HostSide,
        event: &mut mod_api::ItemAfterVerticalCollisionEvent<'_>,
    ) -> mod_api::Flow {
        let mod_api::HostSide::Server(host) = host else {
            return mod_api::Flow::Continue;
        };
        if event.client {
            return mod_api::Flow::Continue;
        }
        let Some(stack) = event.stack.as_ref() else {
            return mod_api::Flow::Continue;
        };
        if stack.item().id(&*host) != "create:schedule" {
            return mod_api::Flow::Continue;
        }
        let schedule = super::schedule_screen::schedule_of(stack).unwrap_or_default();
        let Some(lv) = ServerLevel::new(host, event.dimension) else {
            return mod_api::Flow::Continue;
        };
        let (dim, pos) = (lv.dim, event.block_pos);
        let took = with_world(|w| {
            let r = &mut w.dims[dim].railway;
            if !r.stations.contains_key(&pos) || r.auto_schedules.contains_key(&pos) {
                return false;
            }
            r.auto_schedules.insert(pos, schedule);
            true
        })
        .unwrap_or(false);
        if took {
            super::super::persist::mark_pos_unsaved(dim, pos);
            *event.stack = None;
            // A train standing there takes it at once (`applyAutoSchedule` from the callback).
            super::conductor::apply_auto(dim, pos, true);
        }
        mod_api::Flow::Continue
    }
}

/// The size of the bogey whose state is engine id `state`, if it is one.
fn bogey_size(state: u32) -> Option<BogeySize> {
    match with_world(|w| w.table.get(state).map(|(b, _)| b)).flatten()? {
        Block::SmallBogey => Some(BogeySize::Small),
        Block::LargeBogey => Some(BogeySize::Large),
        _ => None,
    }
}

/// Why a station last failed to assemble (`lastException`, `failedCarriageIndex`), shown on
/// its assembly screen.
struct Failure {
    key: String,
    args: Vec<String>,
    carriage: i32,
}

thread_local! {
    /// Each station's last failure, by level and position.
    static FAILURES: RefCell<HashMap<(usize, [i32; 3]), Failure>> = RefCell::new(HashMap::new());
}

/// The world closed.
pub fn forget() {
    FAILURES.with_borrow_mut(HashMap::clear);
}

/// The station's last failure as its screen shows it: the lang key, its arguments and the
/// carriage (-1 for none).
pub fn failure(dim: usize, pos: [i32; 3]) -> Option<(String, Vec<String>, i32)> {
    FAILURES.with_borrow(|f| {
        f.get(&(dim, pos))
            .map(|f| (f.key.clone(), f.args.clone(), f.carriage))
    })
}

/// `exception`: the station keeps why it failed (`create.train_assembly.*`).
fn assembly_failed(lv: &ServerLevel<'_>, pos: [i32; 3], e: AssemblyError) {
    let failure = Failure {
        key: format!("create.train_assembly.{}", e.key()),
        args: e.args(),
        carriage: -1,
    };
    let dim = lv.dim;
    FAILURES.with_borrow_mut(|f| f.insert((dim, pos), failure));
}

fn clear_failure(dim: usize, pos: [i32; 3]) {
    FAILURES.with_borrow_mut(|f| f.remove(&(dim, pos)));
}

/// The straight track a station looks at, and its shape.
fn station_track(dim: usize, pos: [i32; 3]) -> Option<([i32; 3], TrackShape)> {
    with_world(|w| {
        let r = &w.dims[dim].railway;
        let target = create_engine::network::target_track(&r.tracks, pos)?;
        Some((target, r.tracks[&target]))
    })
    .flatten()
}

/// The unit along a straight track.
fn along(shape: TrackShape) -> [i32; 3] {
    if shape == TrackShape::Xo {
        [1, 0, 0]
    } else {
        [0, 0, 1]
    }
}

/// A bogey standing on the track at `p`.
fn bogey_at(lv: &ServerLevel<'_>, p: [i32; 3]) -> Option<BogeySize> {
    bogey_size(lv.block([p[0], p[1] + 1, p[2]]))
}

/// Whether the station has a straight track to assemble on (`edgePoint.isOrthogonal`).
pub fn has_track(dim: usize, pos: [i32; 3]) -> bool {
    station_track(dim, pos).is_some()
}

/// The train standing still at the station, if any (`getPresentTrain`).
fn stopped_at(r: &create_engine::trains::Railway, pos: [i32; 3]) -> Option<usize> {
    r.trains.iter().position(|t| {
        t.train.bound
            && t.train.nav.is_none()
            && t.train.speed == 0.0
            && t.train.at_station == Some(pos)
    })
}

/// The id of the train standing at the station (`getPresentTrain`).
pub fn present_train(dim: usize, pos: [i32; 3]) -> Option<u32> {
    with_world(|w| {
        let r = &w.dims[dim].railway;
        r.trains
            .iter()
            .find(|t| t.train.bound && t.train.at_station == Some(pos))
            .map(|t| t.id)
    })
    .flatten()
}

/// `Train.canDisassemble` for the train standing still at the station: each carriage level and
/// square to the grid.
pub fn can_disassemble(dim: usize, pos: [i32; 3]) -> bool {
    with_world(|w| {
        let r = &w.dims[dim].railway;
        let t = &r.trains[stopped_at(r, pos)?];
        let g = &r.net.graph;
        Some((0..t.train.carriages.len()).all(|c| {
            let (_, front) = t.train.carriage_pose(g, c);
            let (yaw, pitch) = create_engine::trains::yaw_pitch(front, t.heading);
            pitch.abs() < 1.0e-3 && ((yaw.rem_euclid(90.0) + 1.0e-3) % 90.0) < 2.0e-3
        }))
    })
    .flatten()
    .unwrap_or(false)
}

/// `tryDisassembleTrain`: the train standing at the station goes back into the world, each
/// carriage where it stands (turned to its quarter); its name is kept for the next train
/// assembled there. False when there is none, or it is not square to the grid.
pub fn disassemble(lv: &mut ServerLevel<'_>, pos: [i32; 3], player: Option<PlayerRef>) -> bool {
    let dim = lv.dim;
    railway_ready(dim);
    if !can_disassemble(dim, pos) {
        return false;
    }
    // `dropSchedule`: the train's schedule to the player's empty hand, else dropped above the
    // station.
    if let Some(train) = present_train(dim, pos)
        && let Some(Some(s)) = super::conductor::take_schedule(dim, train)
    {
        let empty_hand = player.is_some_and(|p| {
            crate::glue::items::held(&*lv.host, p, mod_api::Hand::Main)
                .is_none_or(|s| s.count() == 0)
        });
        match player {
            Some(p) if empty_hand => super::conductor::give_schedule(lv.host, p, &s),
            _ => super::conductor::drop_schedule(lv, [pos[0], pos[1] + 1, pos[2]], &s),
        }
    }
    let stopped = with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let i = stopped_at(r, pos)?;
        let g = &r.net.graph;
        let poses: Vec<_> = (0..r.trains[i].train.carriages.len())
            .map(|c| r.trains[i].train.carriage_pose(g, c))
            .collect();
        let t = r.trains.remove(i);
        if t.name.is_empty() {
            r.prev_train_names.remove(&pos);
        } else {
            r.prev_train_names.insert(pos, t.name.clone());
        }
        Some((t, poses))
    })
    .flatten();
    let Some((t, poses)) = stopped else {
        return false;
    };
    // A train taken apart here: its comparators read `trainPresent` again.
    crate::glue::redstone::analog_touched(dim, pos);
    for (c, (car, (pivot, front))) in t.cars.iter().zip(poses).enumerate() {
        let (yaw, _) = create_engine::trains::yaw_pitch(front, t.heading);
        let quarter = ((yaw / 90.0).round() as i32 * 90).rem_euclid(360);
        let place = |o: [i32; 3]| {
            let r = create_engine::contraption::rotate([o[0], 0, o[2]], quarter, Axis::Y);
            [
                (pivot[0] + f64::from(r[0])).floor() as i32,
                (pivot[1] + f64::from(o[1]) + 1.0).round() as i32,
                (pivot[2] + f64::from(r[2])).floor() as i32,
            ]
        };
        super::cargo::expect(lv, t.id, c, place);
        for (o, s) in car {
            // The block's centre from the pivot (the middle of the track under the
            // carriage's leading bogey), turned about it; its height from the track.
            let p = place(*o);
            let Some(state) = super::car_state(*s) else {
                continue;
            };
            let state = turned_state(&*lv.host, state, quarter, Axis::Y);
            // Without shape updates (`Contraption.addBlocksToWorld` places them as they were
            // carried): a door's half put back before its other half stays.
            lv.set_state(
                p,
                state,
                update_all() | mod_api::BlockUpdateFlags::KNOWN_SHAPE,
            );
        }
        // The carriage's storage blocks take their items back.
        super::cargo::release(lv, t.id, c, place);
    }
    super::super::persist::mark_unsaved(dim, [t.column.0, t.column.1]);
    super::super::persist::mark_pos_unsaved(dim, pos);
    super::controls::disassembled(lv, t.id);
    super::actors::disassembled(lv, t.id);
    super::riding::train_gone(lv, t.id);
    send_train_clear(lv, t.id);
    true
}

/// Whether the station is in assembly mode.
pub fn assembling(dim: usize, pos: [i32; 3]) -> bool {
    with_world(|w| w.dims[dim].railway.assembling.contains_key(&pos)).unwrap_or(false)
}

/// `enterAssemblyMode`: a train standing at the station is taken apart first; the station's
/// track must be straight (`tryEnterAssemblyMode`). Its assembly direction is the way along
/// the track bogeys stand on, else the way `player` looks (Create takes it from the look of
/// the player who placed the station on its track). Trains bound for it look for another way.
pub fn enter_assembly(lv: &mut ServerLevel<'_>, pos: [i32; 3], player: Option<PlayerRef>) -> bool {
    let dim = lv.dim;
    railway_ready(dim);
    if assembling(dim, pos) {
        return false;
    }
    if with_world(|w| stopped_at(&w.dims[dim].railway, pos).is_some()) == Some(true)
        && !disassemble(lv, pos, player)
    {
        return false;
    }
    let Some((target, shape)) = station_track(dim, pos) else {
        return false;
    };
    let along = along(shape);
    let with_bogeys = [1, -1]
        .into_iter()
        .map(|s| [along[0] * s, 0, along[2] * s])
        .find(|d| bogey_at(lv, [target[0] + d[0], target[1], target[2] + d[2]]).is_some());
    let looked = player
        .and_then(|p| p.entity().rotation(&*lv.host))
        .map_or(1, |[yaw, _]| {
            // The look's unit: yaw 0 faces +z, 90 faces -x.
            let r = f64::from(yaw).to_radians();
            let s = if along[0] != 0 { -r.sin() } else { r.cos() };
            if s < 0.0 { -1 } else { 1 }
        });
    let direction = with_bogeys.unwrap_or([along[0] * looked, 0, along[2] * looked]);
    with_world(|w| {
        let r = &mut w.dims[dim].railway;
        r.assembling.insert(pos, direction);
        // `startNavigation` again for trains bound here: the station is no destination now.
        for t in &mut r.trains {
            if t.train.nav.as_ref().is_some_and(|n| n.station == pos) {
                t.train.nav = None;
                t.runtime.interrupted();
            }
        }
    });
    clear_failure(dim, pos);
    super::super::persist::mark_pos_unsaved(dim, pos);
    true
}

/// `exitAssemblyMode` (`cancelAssembly`).
pub fn exit_assembly(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> bool {
    let dim = lv.dim;
    let was =
        with_world(|w| w.dims[dim].railway.assembling.remove(&pos).is_some()).unwrap_or(false);
    clear_failure(dim, pos);
    if was {
        super::super::persist::mark_pos_unsaved(dim, pos);
    }
    was
}

/// A bogey on an assembly track: its place from the station's track, block and size.
pub type Bogey = (i32, [i32; 3], BogeySize);

/// The station's assembly track (`refreshAssemblyInfo`): the track on from the station's in
/// its assembly direction while it runs the same way, and the bogeys on it, each with its place
/// from the station's track, block and size.
pub fn assembly_track(lv: &ServerLevel<'_>, pos: [i32; 3]) -> Option<(Vec<[i32; 3]>, Vec<Bogey>)> {
    let dim = lv.dim;
    let direction = with_world(|w| w.dims[dim].railway.assembling.get(&pos).copied()).flatten()?;
    let (target, shape) = station_track(dim, pos)?;
    let mut run = Vec::new();
    let mut bogeys = Vec::new();
    for i in 0..MAX_ASSEMBLY_LENGTH - 1 {
        let p = [
            target[0] + direction[0] * (i + 1),
            target[1],
            target[2] + direction[2] * (i + 1),
        ];
        if with_world(|w| w.dims[dim].railway.tracks.get(&p) != Some(&shape)) != Some(false) {
            break;
        }
        run.push(p);
        if bogeys.len() < MAX_BOGEY_COUNT
            && let Some(size) = bogey_at(lv, p)
        {
            bogeys.push((i, [p[0], p[1] + 1, p[2]], size));
        }
    }
    Some((run, bogeys))
}

/// `StationBlockEntity.assemble`: the bogeys along the station's track in its assembly
/// direction, and what each carries, become carriages of a train on the track graph, by
/// Create's rules; a failure is kept for the screen (`exception`). True when a train was made.
pub fn assemble(lv: &mut ServerLevel<'_>, pos: [i32; 3]) -> bool {
    let dim = lv.dim;
    railway_ready(dim);
    let Some(direction) =
        with_world(|w| w.dims[dim].railway.assembling.get(&pos).copied()).flatten()
    else {
        return false;
    };
    let Some((target, _)) = station_track(dim, pos) else {
        return false;
    };
    let Some((run, bogeys)) = assembly_track(lv, pos) else {
        return false;
    };
    let locations: Vec<i32> = bogeys.iter().map(|b| b.0).collect();
    let dir_name = match direction {
        [1, _, 0] => "east",
        [-1, _, 0] => "west",
        [0, _, 1] => "south",
        _ => "north",
    };
    // Each carriage's structure from its first bogey (`CarriageContraption.assemble`): what
    // sticks to it, not the track.
    let mut gathered: HashMap<usize, Vec<([i32; 3], u32)>> = HashMap::new();
    let mut refused = false;
    let controls = states::key(Block::Controls);
    let laid = layout(&locations, |i| {
        let b = bogeys[i].1;
        let Ok(blocks) =
            create_engine::piston::gather(b, Dir::Down, &run, |p| found_carrying(lv, p))
        else {
            refused = true;
            return Structure::default();
        };
        let mut s = Structure {
            blocks: blocks.len(),
            ..Structure::default()
        };
        for (o, state) in &blocks {
            let p = [b[0] + o[0], b[1] + o[1], b[2] + o[2]];
            if let Some(k) = bogeys.iter().find(|f| f.1 == p) {
                s.bogeys.push(k.0);
            }
            // `CarriageContraption.capture`: train controls by their facing.
            if let Some(st) = super::car_state(*state)
                && st.block() == controls
                && let Some(f) = super::property(&*lv.host, st, "facing")
            {
                let across = match direction[0] {
                    0 => matches!(f.as_str(), "east" | "west"),
                    _ => matches!(f.as_str(), "north" | "south"),
                };
                if across {
                    s.sideways = true;
                } else if f == dir_name {
                    s.forward = true;
                } else {
                    s.backward = true;
                }
            }
        }
        gathered.insert(i, blocks);
        s
    });
    let laid = match laid {
        // Something on it can't move: nothing is assembled.
        Ok(_) if refused => return false,
        Ok(l) => l,
        Err(e) => {
            assembly_failed(lv, pos, e);
            return false;
        }
    };
    let sizes: Vec<_> = bogeys.iter().map(|b| b.2).collect();
    let dir = [f64::from(direction[0]), 0.0, f64::from(direction[2])];
    // The end of the station's track block toward the assembly direction.
    let origin = [
        f64::from(target[0]) + 0.5 + dir[0] * 0.5,
        f64::from(target[1]),
        f64::from(target[2]) + 0.5 + dir[2] * 0.5,
    ];
    let cars: Vec<Vec<([i32; 3], u32)>> = laid
        .carriages
        .iter()
        .map(|c| gathered.remove(&c.first).unwrap_or_default())
        .collect();
    let heading = [-direction[0], -direction[2]];
    let origin_block = bogeys.first().map_or(target, |f| f.1);
    let made = with_world(|w| {
        let r = &mut w.dims[dim].railway;
        let mut train = Train::assembled(&r.net, origin, dir, &laid, &sizes)?;
        train.at_station = Some(pos);
        let id = r.next_id();
        // `lastDisassembledTrainName`.
        let name = r.prev_train_names.remove(&pos).unwrap_or_default();
        let mut t = TrainOnTrack {
            id,
            train,
            cars: cars.clone(),
            heading,
            schedule: None,
            runtime: Runtime::new(),
            idle: 0,
            column: (origin_block[0] >> 4, origin_block[2] >> 4),
            sent_tick: 0,
            sent_edge: usize::MAX,
            fresh: true,
            name,
            icon: 0,
            players: 0,
            probes: Default::default(),
        };
        t.column = t.column_on(&r.net);
        r.trains.push(t);
        Some(id)
    })
    .flatten();
    let Some(id) = made else {
        assembly_failed(lv, pos, AssemblyError::NoBogeys);
        return false;
    };
    clear_failure(dim, pos);
    crate::glue::advancements::train_assembled(lv, pos, &cars);
    // A train assembled here: its comparators read `trainPresent` again.
    crate::glue::redstone::analog_touched(dim, pos);
    super::super::persist::mark_pos_unsaved(dim, pos);
    // `Contraption.addPassengersToWorld`: mobs sitting on the carriages' seats ride along.
    let mut riders = Vec::new();
    for (c, (shape, car)) in laid.carriages.iter().zip(&cars).enumerate() {
        let b = bogeys[shape.first].1;
        for (o, s) in car {
            let p = [b[0] + o[0], b[1] + o[1], b[2] + o[2]];
            if super::car_state(*s).is_some_and(|st| super::riding::is_seat(&*lv.host, st))
                && let Some(mob) = crate::glue::contraptions::seats::seated_mob(lv, p)
            {
                riders.push((c, *o, mob));
            }
        }
    }
    // `MountedStorageManager`: the storage blocks' items ride with their carriage.
    for (c, (shape, car)) in laid.carriages.iter().zip(&cars).enumerate() {
        super::cargo::carry(lv, id, c, bogeys[shape.first].1, car);
    }
    let Some(air) = super::default_state(&*lv.host, "minecraft:air") else {
        return true;
    };
    for (c, car) in laid.carriages.iter().zip(&cars) {
        let b = bogeys[c.first].1;
        for (o, _) in car {
            let p = [b[0] + o[0], b[1] + o[1], b[2] + o[2]];
            // `removeBlocksFromWorld`: no drops and no shape updates, so the other half of a
            // door taken along doesn't break and drop.
            let flags = update_all()
                | mod_api::BlockUpdateFlags::KNOWN_SHAPE
                | mod_api::BlockUpdateFlags::SUPPRESS_DROPS
                | mod_api::BlockUpdateFlags::MOVE_BY_PISTON;
            lv.set_state(p, air, flags);
            super::super::persist::mark_pos_unsaved(dim, p);
        }
    }
    send_trains(lv, &[id], true);
    for (c, local, mob) in riders {
        let at = create_engine::train_controls::Target {
            train: id,
            car: c as u16,
            local,
        };
        super::riding::sit_entity(lv.host, dim, mob, at);
    }
    true
}

/// The track blocks' part: a click on a station's assembly track (`TrackBlock.useItemOn`).
pub struct TrackClick;

fn click_track(host: &mut dyn ServerHost, click: &BlockClick<'_>) -> Option<InteractionResult> {
    let player = host.player(click.player)?;
    if after_empty_main(&*host, player, click) {
        return None;
    }
    let dimension = click.dimension.key().id().to_owned();
    let mut lv = ServerLevel::new(host, &dimension)?;
    if super::relocate::picking(player)
        && super::relocate::track_clicked(&mut lv, player, click.pos)
    {
        return Some(InteractionResult::SuccessServer);
    }
    track_clicked(&mut lv, player, click.hand, click.stack, click.pos)
        .then_some(InteractionResult::SuccessServer)
}

#[mod_api::horizon_part]
impl BlockPart for TrackClick {
    fn use_item_on(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseItemOnEvent<'_>,
    ) -> Option<InteractionResult> {
        click_track(host, event.click)
    }

    fn use_without_item(
        &self,
        host: &mut dyn ServerHost,
        event: &mut UseWithoutItemEvent<'_>,
    ) -> Option<InteractionResult> {
        click_track(host, event.click)
    }
}

/// `StationBlockEntity.trackClicked` for each station in assembly mode whose assembly area
/// (`assemblyAreas`: its assembly track but the last block) holds `pos`: at a place a bogey
/// may stand (`isValidBogeyOffset`: the first right after the station's track, the others
/// three on from it and two clear of each other), a small bogey goes on the track (what stood
/// there broken, one railway casing used outside creative, which needs one); elsewhere a
/// bogey within a block of the click changes size.
fn track_clicked(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    hand: mod_api::Hand,
    stack: Option<&mod_api::ItemStack>,
    pos: [i32; 3],
) -> bool {
    let dim = lv.dim;
    railway_ready(dim);
    let mut stations: Vec<([i32; 3], [i32; 3])> = with_world(|w| {
        w.dims[dim]
            .railway
            .assembling
            .iter()
            .map(|(p, d)| (*p, *d))
            .collect()
    })
    .unwrap_or_default();
    stations.sort();
    for (station, d) in stations {
        let Some((run, found)) = assembly_track(lv, station) else {
            continue;
        };
        let area = &run[..run.len().saturating_sub(1)];
        let Some(offset) = area.iter().position(|p| *p == pos) else {
            continue;
        };
        if bogey_clicked(lv, player, hand, stack, pos, d, offset as i32, &found) {
            return true;
        }
    }
    false
}

/// What a click at place `offset` of a station's assembly track does, `bogeys` standing on it.
#[allow(clippy::too_many_arguments)]
fn bogey_clicked(
    lv: &mut ServerLevel<'_>,
    player: PlayerRef,
    hand: mod_api::Hand,
    stack: Option<&mod_api::ItemStack>,
    pos: [i32; 3],
    d: [i32; 3],
    offset: i32,
    found: &[Bogey],
) -> bool {
    let bogeys: Vec<i32> = found.iter().map(|b| b.0).collect();
    let valid = !((offset < 3 || bogeys.is_empty()) && offset != 0)
        && !bogeys.iter().any(|&j| offset >= j - 2 && offset <= j + 2);
    if !valid {
        // A bogey by the click changes size (`getNextSize`: small, large, small).
        for i in -1..=1 {
            let p = [pos[0] + d[0] * i, pos[1] + 1, pos[2] + d[2] * i];
            let next = match bogey_size(lv.block(p)) {
                Some(BogeySize::Small) => "create:large_bogey",
                Some(BogeySize::Large) => "create:small_bogey",
                None => continue,
            };
            let Some(state) = lv.block_state(p) else {
                continue;
            };
            let axis = super::property(&*lv.host, state, "axis").unwrap_or_else(|| "z".into());
            let Some(next) = super::default_state(&*lv.host, next)
                .and_then(|s| super::with_props(&*lv.host, s, &[("axis", axis.as_str())]))
            else {
                continue;
            };
            lv.set_state(p, next, update_all());
            // `IWrenchable.playRotateSound`.
            lv.level.play_sound(
                &mut *lv.host,
                "create:wrench_rotate",
                p.map(|v| f64::from(v) + 0.5),
                mod_api::SoundSource::Blocks,
                1.0,
                1.0,
            );
            return true;
        }
        return false;
    }
    let creative = player.is_creative(&*lv.host) == Some(true);
    let casing = stack.is_some_and(|s| s.item().id(&*lv.host) == "create:railway_casing");
    if !creative && !casing {
        super::action_bar(
            lv.host,
            player,
            Text::translate("create.train_assembly.requires_casing", Vec::new()),
        );
        return false;
    }
    let up = [pos[0], pos[1] + 1, pos[2]];
    let unbreakable = lv
        .block_state(up)
        .and_then(|s| lv.host.block_states().destroy_speed(s))
        .is_some_and(|h| h < 0.0);
    if unbreakable {
        return false;
    }
    lv.destroy_block(up, true);
    // `TrackBlock.getBogeyAnchor`: a small bogey along the track.
    let axis = if d[0] != 0 { "x" } else { "z" };
    let Some(bogey) = super::default_state(&*lv.host, "create:small_bogey")
        .and_then(|s| super::with_props(&*lv.host, s, &[("axis", axis)]))
    else {
        return false;
    };
    lv.set_state(up, bogey, update_all());
    super::action_bar(
        lv.host,
        player,
        Text::translate("create.train_assembly.bogey_created", Vec::new()),
    );
    lv.level.play_sound(
        &mut *lv.host,
        "minecraft:block.netherite_block.place",
        pos.map(|v| f64::from(v) + 0.5),
        mod_api::SoundSource::Blocks,
        1.0,
        0.8,
    );
    if !creative {
        crate::glue::items::shrink_held(lv.host, player, hand, 1);
    }
    true
}
