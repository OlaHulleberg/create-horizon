//! Create's trains and minecart contraptions: tracks and their curves, stations, signals and
//! observers, the trains running between them over the railway graph (the engine's `trains`
//! and `network`), the train controls and their HUD (`train_controls`, `train_hud`), and cart
//! assemblers with the contraptions and couplings of minecarts (`cart`, `mounted`, `coupling`).
//!
//! - [`Trains`] (`railway`, `tracks`, `assembly`, `saved`): the railway's tick, the stations'
//!   assembly mode and assembly, the bogeys clicked onto their assembly track, the track item's
//!   placement, the curves and signal lights clients draw (both in the track blocks' own block
//!   entity data), and the trains' save with their columns.
//! - `station_screen`, `schedule_screen` (`gui`): the station's screens (`StationScreen`,
//!   `AssemblyScreen`: its name, assembly, the train's name) and the schedule's editor
//!   (`ScheduleScreen`), drawn as the mod's own screens.
//! - `conductor`: schedules handed to a mob on a carriage seat behind controls, or held by a
//!   station for the trains arriving there; `relocate`: moving a standing train with the
//!   wrench.
//! - `visuals`: trains and cart contraptions as players see them, each block a vanilla block
//!   display the server moves with its carriage, bogey or cart (`CarriageContraptionVisual`,
//!   `StandardBogeyVisual`, `OrientedContraptionEntity`'s drawing).
//! - `riding`: carriage seats, a player sitting on an item display stand-in the server keeps on
//!   the seat.
//! - [`TrainControls`] (`controls`, `client`): clicks on carriages, driving by the controls'
//!   keys and the train HUD.
//! - [`Carts`] (`carts`, `coupling_draw`): cart assemblers acting on the carts that pass them,
//!   contraptions riding carts, their items, couplings (drawn by clients as Create's
//!   `CouplingRenderer` draws them), controller rails turned by the wrench, and their save with
//!   the world.
//! - `carriage_draw`: the couplings between carriages, drawn by clients
//!   (`CarriageCouplingRenderer`).
//! - `station_draw`: a station's flag, its marker on the track and its assembly track
//!   (`StationRenderer`).
//! - `actors`: the drills, saws, harvesters, ploughs, rollers, deployers and contacts a
//!   carriage carries, working where it is each tick and stalling their train.
//! - `horn`: the driver's horn through the train's first steam whistle (`TrainHUD`, `HonkPacket`).
//! - `doors`: carriages' sliding doors opening at stations by their door control.
//!
//! Trains meeting on the track crash and derail (`Train.collideWithOtherTrains`, `crash`), standing
//! where they met until the wrench puts them back; a train hurts the grounded mobs it meets
//! (`handleDamageFromTrain`). Portal tracks are left out (`API_GAPS.md`: a portal's
//! destination).
//! - `cargo`: what carriages' storage blocks hold, and portable storage interfaces on slow
//!   carriages connecting to the stationary ones at a station.
//!
//! Every schedule instruction and condition is offered and run; carriages carry no fluid (a
//! tank keeps only its block when moved), so a fluid threshold reads none. The package
//! instructions go to the stations whose postboxes (`GlobalPackagePort`, the packages'
//! `station_ports`) take the packages aboard or hold packages to fetch, and a train standing at
//! a station trades packages with its postboxes.
//!
//! What the mod API cannot do, and is left out:
//!
//! - **Standing on and colliding with carriages** (`ContraptionCollider`):
//!   the API has collision exclusions (boxes where blocks don't stop a player), not moving
//!   colliders. Needs level colliders (`API_GAPS.md`, moving colliders).
//! - **Taking the controls' keys from the game** is done with `movement_input` (the player
//!   neither walks, jumps nor sneaks), as Create's `setDown(false)` does; `input_key` can't
//!   consume a key, so a key bound to something else still does it.

pub mod actors;
pub mod assembly;
mod cargo;
mod carriage_draw;
mod carts;
mod client;
mod conductor;
mod contents;
mod controls;
mod coupling_draw;
mod doors;
pub(crate) mod gui;
mod horn;
mod railway;
mod relocate;
mod riding;
mod saved;
mod schedule_screen;
mod station_draw;
mod station_screen;
mod structure;
mod tracks;
mod visuals;

pub(crate) use cargo::key as cargo_key;

pub use carts::{Carts, assembler_mode, set_assembler_mode};
pub(crate) use carts::{put_cart_carried, take_cart_carried};
pub use controls::TrainControls;
pub(crate) use controls::drivers;
pub use railway::Trains;
pub(crate) use structure::turned_state;
pub(crate) use tracks::signal_state_of;

use super::states;
use mod_api::{BlockState, Host, HostBlockStates, PlayerRef, ServerHost, Text};
use std::cell::RefCell;
use std::collections::HashMap;

/// `TrainStatusDisplaySource`: the schedule status of the train at the station at `pos`, in
/// Create's en_us (None: `EMPTY_LINE`).
pub(crate) fn waiting_status(lv: &super::server::ServerLevel<'_>, pos: [i32; 3]) -> Option<String> {
    let dim = lv.dim;
    let id = assembly::present_train(dim, pos)?;
    let day_time = lv.level.day_time(&*lv.host).unwrap_or(0);
    super::world::with_world(|w| {
        let t = w.dims[dim].railway.trains.iter().find(|t| t.id == id)?;
        create_engine::display::train_status(
            t.schedule.as_ref(),
            &t.runtime,
            t.players,
            day_time,
            &create_engine::display::Lang::EN_US,
        )
    })
    .flatten()
}

/// `ObservedTrainNameSource`: the name of the train the track observer at `pos` sees
/// (`TrackObserver.getCurrentTrain`), None (`EMPTY_LINE`) when none.
pub(crate) fn observed_train_name(
    lv: &super::server::ServerLevel<'_>,
    pos: [i32; 3],
) -> Option<String> {
    let dim = lv.dim;
    super::world::with_world(|w| {
        let r = &w.dims[dim].railway;
        let id = r.observers.get(&pos)?.train?;
        let t = r.trains.iter().find(|t| t.id == id)?;
        // `Train.name`: an unnamed train is `train.unnamed`, in en_us.
        Some(if t.name.is_empty() {
            "Unnamed Train".to_owned()
        } else {
            t.name.clone()
        })
    })
    .flatten()
}

/// The block states carriages and cart contraptions carry, as the engine numbers them: Create's
/// by their engine id, any other by its place in this side's own palette (`1..`, below Create's
/// ids), so the engine's `([i32; 3], u32)` blocks keep every block a train takes.
#[derive(Default)]
struct Palette {
    states: Vec<BlockState>,
    ids: HashMap<BlockState, u32>,
}

thread_local! {
    static PALETTE: RefCell<Palette> = RefCell::new(Palette::default());
}

/// Items moved in or out of contraption storage `key` through an interface: when it is a
/// carriage's, its train's cargo is not idle (`IdleCargoCondition`).
pub fn cargo_moved(dim: usize, key: [i32; 3]) {
    if let Some(train) = cargo::train_of(key) {
        cargo::moved(dim, train);
    }
}

/// The engine's number for `state` on this side.
pub fn car_id(state: BlockState) -> u32 {
    if let Some(id) = states::id(state) {
        return id;
    }
    PALETTE.with_borrow_mut(|p| {
        if let Some(&id) = p.ids.get(&state) {
            return id;
        }
        p.states.push(state);
        let id = p.states.len() as u32;
        p.ids.insert(state, id);
        id
    })
}

/// The state numbered `id` on this side.
pub fn car_state(id: u32) -> Option<BlockState> {
    if id >= states::FIRST {
        return states::state(id);
    }
    let i = id.checked_sub(1)? as usize;
    PALETTE.with_borrow(|p| p.states.get(i).copied())
}

/// The state numbered `id` as `/setblock` writes it (`minecraft:oak_stairs[facing=east]`).
pub fn car_text(host: &dyn Host, id: u32) -> Option<String> {
    host.block_state_string(car_state(id)?)
}

/// The number of the state `text` names, as saved.
pub fn car_of_text(host: &dyn Host, text: &str) -> Option<u32> {
    host.block_state_id(text).map(car_id)
}

/// The block of `state` and its properties, `("create:controls", [("facing", "north")])`.
pub fn state_parts(host: &dyn Host, state: BlockState) -> Option<(String, Vec<(String, String)>)> {
    let text = host.block_state_string(state)?;
    let (name, props) = states::split(&text);
    Some((
        name.to_owned(),
        props
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
    ))
}

/// Property `key` of `state`.
pub fn property(host: &dyn Host, state: BlockState, key: &str) -> Option<String> {
    state
        .property(&host.block_states(), key)
        .ok()
        .flatten()
        .map(str::to_owned)
}

/// `state` with `props` changed; None when it has no such property or value.
pub fn with_props(
    host: &dyn Host,
    state: BlockState,
    props: &[(&str, &str)],
) -> Option<BlockState> {
    let facts = host.block_states();
    props
        .iter()
        .try_fold(state, |s, (k, v)| s.with(&facts, k, v).ok())
}

/// The default state of block `name` (`minecraft:air`).
pub fn default_state(host: &dyn Host, name: &str) -> Option<BlockState> {
    host.block_state_id(name)
}

/// Tells `player` `line` on their action bar.
pub fn action_bar(host: &mut dyn ServerHost, player: PlayerRef, line: Text) {
    let _ = player.action_bar(host, line);
}

/// Whether they are a spectator or in creative.
pub fn player_mode(host: &dyn Host, player: PlayerRef) -> (bool, bool) {
    (
        player.is_spectator(host).unwrap_or(false),
        player.is_creative(host).unwrap_or(false),
    )
}
